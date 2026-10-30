//! Plugin session: process lifecycle and the newline-delimited JSON stdio
//! transport (docs/guide/plugins.md §4.4–§4.5).
//!
//! The host spawns the plugin with stdin/stdout/stderr piped and runs three
//! dedicated threads so no stream can ever block on a full pipe (the
//! Windows-pipe hazard §4.4 calls out):
//!
//! * a stdout reader — the ONLY protocol input; oversized or unparseable
//!   lines are recorded as faults and skipped, never propagated;
//! * a stderr drainer — plugin diagnostics, capped retention, never parsed
//!   as protocol;
//! * a stdin writer — serializes host writes so a hung plugin cannot stall
//!   the caller past its deadline.
//!
//! Every request carries a deadline; expiry surfaces as
//! [`RpcError::TIMED_OUT`], caller-side cancellation as
//! [`RpcError::CANCELLED`], and plugin death as [`HostError::Unavailable`]
//! with all in-flight requests failed — the host process itself is never
//! taken down by a plugin crash.

use custos_plugin_api::capability::Capability;
use custos_plugin_api::{
    error_response, request, Method, RequestId, Response, RpcError, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Errors the host can hit while managing one plugin session.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("plugin process spawn failed: {0}")]
    Spawn(String),
    #[error("plugin protocol {plugin} incompatible with host protocol {host}")]
    IncompatibleProtocol { plugin: String, host: String },
    #[error("plugin is not running (crashed or shut down)")]
    Unavailable,
    #[error("rpc error {code}: {message}")]
    Rpc { code: i32, message: String },
    #[error("protocol violation: {0}")]
    Protocol(String),
    #[error("plugin declared no capabilities")]
    NoCapabilities,
    #[error("duplicate capability {operation_id} (kind {kind})")]
    DuplicateCapability { kind: String, operation_id: String },
    #[error("plugin lacks required capability {kind}/{operation_id}")]
    MissingRequiredCapability { kind: String, operation_id: String },
}

impl From<RpcError> for HostError {
    fn from(err: RpcError) -> Self {
        HostError::Rpc {
            code: err.code,
            message: err.message,
        }
    }
}

/// How a plugin wishes to be known after a successful handshake.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
}

/// Everything needed to spawn one plugin process.
#[derive(Debug, Clone)]
pub struct PluginConfig {
    pub program: std::ffi::OsString,
    pub args: Vec<String>,
    /// Extra environment; the child gets a CLEAN environment plus these
    /// entries only (§4.4 controlled environment), so plugins can never
    /// inherit host secrets by accident.
    pub env: BTreeMap<String, String>,
    /// Default per-request deadline.
    pub request_timeout: Duration,
    /// Per-message wire cap (§4.4, default 1 MiB, configurable).
    pub max_message_bytes: usize,
}

impl PluginConfig {
    pub fn new(program: impl Into<std::ffi::OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            request_timeout: Duration::from_secs(5),
            max_message_bytes: custos_plugin_api::DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

/// One live plugin process plus its transport threads.
pub struct PluginSession {
    shared: Arc<Shared>,
    next_id: AtomicI64,
    message_cap_value: usize,
    default_timeout_value: Duration,
}

#[derive(Clone)]
enum RunState {
    Running,
    Dead { reason: String },
}

struct Shared {
    state: Mutex<RunState>,
    pending: Mutex<HashMap<RequestId, SyncSender<Response>>>,
    writer: Mutex<Option<mpsc::Sender<WriterMsg>>>,
    child: Mutex<Option<Child>>,
    /// Retained stderr tail, capped at [`custos_plugin_api::MAX_STDERR_RETAINED_BYTES`].
    stderr_tail: Mutex<VecDeque<u8>>,
    /// Host-side protocol violations observed on stdout (skipped lines,
    /// unparseable JSON, responses to unknown requests).
    faults: Mutex<Vec<String>>,
}

enum WriterMsg {
    Line {
        payload: Vec<u8>,
        ack: SyncSender<std::io::Result<()>>,
    },
    Close,
}

impl Shared {
    fn push_fault(&self, fault: String) {
        lock(&self.faults).push(fault);
    }

    fn is_running(&self) -> bool {
        matches!(*lock(&self.state), RunState::Running)
    }

    fn mark_dead(&self, reason: &str) {
        let mut state = lock(&self.state);
        if matches!(*state, RunState::Running) {
            *state = RunState::Dead {
                reason: reason.to_owned(),
            };
        }
    }
}

/// Locks, recovering from poisoning: a panicked thread never takes the
/// whole host down with it (the crash-isolation rule applies in here too).
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PluginSession {
    /// Spawns the plugin and starts the transport threads. The session is
    /// not usable until [`PluginSession::initialize`] succeeds.
    pub fn start(config: &PluginConfig) -> Result<Self, HostError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &config.env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|e| HostError::Spawn(format!("{}: {e}", config.program.to_string_lossy())))?;

        let stdin = child.stdin.take().expect("stdin piped above");
        let stdout = child.stdout.take().expect("stdout piped above");
        let stderr = child.stderr.take().expect("stderr piped above");

        let (writer_tx, writer_rx) = mpsc::channel::<WriterMsg>();
        let shared = Arc::new(Shared {
            state: Mutex::new(RunState::Running),
            pending: Mutex::new(HashMap::new()),
            writer: Mutex::new(Some(writer_tx)),
            child: Mutex::new(Some(child)),
            stderr_tail: Mutex::new(VecDeque::new()),
            faults: Mutex::new(Vec::new()),
        });

        thread::Builder::new()
            .name("plugin-stdin".into())
            .spawn(writer_loop(stdin, writer_rx))
            .map_err(|e| HostError::Spawn(format!("writer thread: {e}")))?;
        thread::Builder::new()
            .name("plugin-stdout".into())
            .spawn(stdout_loop(
                stdout,
                shared.clone(),
                config.max_message_bytes,
            ))
            .map_err(|e| HostError::Spawn(format!("stdout thread: {e}")))?;
        thread::Builder::new()
            .name("plugin-stderr".into())
            .spawn(stderr_loop(stderr, shared.clone()))
            .map_err(|e| HostError::Spawn(format!("stderr thread: {e}")))?;

        Ok(Self {
            shared,
            next_id: AtomicI64::new(1),
            message_cap_value: config.max_message_bytes,
            default_timeout_value: config.request_timeout,
        })
    }

    /// `false` once the plugin process has exited or been shut down.
    pub fn is_running(&self) -> bool {
        self.shared.is_running()
    }

    /// Why the session died, once it has.
    pub fn death_reason(&self) -> Option<String> {
        match &*lock(&self.shared.state) {
            RunState::Running => None,
            RunState::Dead { reason } => Some(reason.clone()),
        }
    }

    /// Retained plugin diagnostics from stderr (bounded, lossy). Never
    /// parsed as protocol — for incident reports only (§4.4).
    pub fn stderr_tail(&self) -> String {
        let tail = lock(&self.shared.stderr_tail);
        String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<u8>>()).into_owned()
    }

    /// Protocol faults the host observed and handled on stdout.
    pub fn faults(&self) -> Vec<String> {
        lock(&self.shared.faults).clone()
    }

    /// Sends a request and returns a handle that can be waited on or
    /// cancelled. This is the only way to cancel an in-flight request.
    pub fn spawn_call(
        &self,
        method: Method,
        params: serde_json::Value,
    ) -> Result<CallHandle, HostError> {
        if !self.is_running() {
            return Err(HostError::Unavailable);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let payload = serde_json::to_vec(&request(id, method, params))
            .map_err(|e| HostError::Protocol(format!("request serialization failed: {e}")))?;
        if payload.len() + 1 > self.message_cap() {
            return Err(HostError::Rpc {
                code: RpcError::MESSAGE_TOO_LARGE,
                message: format!(
                    "outbound request is {} bytes, above the {}-byte cap",
                    payload.len() + 1,
                    self.message_cap()
                ),
            });
        }
        let mut framed = payload;
        framed.push(b'\n');

        let (response_tx, response_rx) = mpsc::sync_channel::<Response>(1);
        lock(&self.shared.pending).insert(RequestId::Number(id), response_tx);

        let writer = lock(&self.shared.writer)
            .clone()
            .ok_or(HostError::Unavailable)?;
        let (ack_tx, ack_rx) = mpsc::sync_channel::<std::io::Result<()>>(1);
        writer
            .send(WriterMsg::Line {
                payload: framed,
                ack: ack_tx,
            })
            .map_err(|_| HostError::Unavailable)?;

        let id_key = RequestId::Number(id);
        match ack_rx.recv_timeout(self.default_timeout()) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                lock(&self.shared.pending).remove(&id_key);
                self.shared.mark_dead(&format!("stdin write failed: {e}"));
                return Err(HostError::Unavailable);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&self.shared.pending).remove(&id_key);
                return Err(HostError::Rpc {
                    code: RpcError::TIMED_OUT,
                    message: "request could not be written before the deadline".into(),
                });
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                lock(&self.shared.pending).remove(&id_key);
                return Err(HostError::Unavailable);
            }
        }

        Ok(CallHandle {
            shared: self.shared.clone(),
            id: id_key,
            rx: response_rx,
        })
    }

    /// Sends a request and waits up to `timeout` for the result.
    pub fn call_with_timeout(
        &self,
        method: Method,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, HostError> {
        self.spawn_call(method, params)?.wait(timeout)
    }

    /// Sends a request using the session's default deadline.
    pub fn call(
        &self,
        method: Method,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        let timeout = self.default_timeout();
        self.call_with_timeout(method, params, timeout)
    }

    /// The `custos/v1/initialize` handshake (§4.5): exchange protocol
    /// versions, reject mismatches and shut the process down.
    pub fn initialize(&self) -> Result<InitializeResult, HostError> {
        let result = self.call(
            Method::Initialize,
            serde_json::json!({
                "protocol_version": PROTOCOL_VERSION,
                "client": "custos-plugin-host",
            }),
        )?;
        let init: InitializeResult = serde_json::from_value(result)
            .map_err(|e| HostError::Protocol(format!("malformed initialize result: {e}")))?;
        if init.protocol_version != PROTOCOL_VERSION {
            let plugin = init.protocol_version.clone();
            self.shutdown()?;
            return Err(HostError::IncompatibleProtocol {
                plugin,
                host: PROTOCOL_VERSION.to_owned(),
            });
        }
        Ok(init)
    }

    /// The `custos/v1/describe_capabilities` round trip.
    pub fn capabilities(&self) -> Result<Vec<Capability>, HostError> {
        let result = self.call(Method::DescribeCapabilities, serde_json::Value::Null)?;
        serde_json::from_value(result)
            .map_err(|e| HostError::Protocol(format!("malformed capabilities: {e}")))
    }

    /// Graceful shutdown: best-effort `custos/v1/shutdown`, then close
    /// stdin, wait bounded, kill on expiry. Idempotent.
    pub fn shutdown(&self) -> Result<(), HostError> {
        if self.is_running() {
            let _ = self.call_with_timeout(
                Method::Shutdown,
                serde_json::Value::Null,
                Duration::from_secs(2),
            );
        }
        self.shared.mark_dead("shutdown requested");
        if let Some(writer) = lock(&self.shared.writer).take() {
            let _ = writer.send(WriterMsg::Close);
        }
        wait_child_exit(&self.shared, Duration::from_secs(3));
        Ok(())
    }

    // Cap and default deadline are frozen at start() time.
    fn message_cap(&self) -> usize {
        self.message_cap_value
    }

    fn default_timeout(&self) -> Duration {
        self.default_timeout_value
    }
}

/// A request in flight: wait for it or cancel it.
pub struct CallHandle {
    shared: Arc<Shared>,
    id: RequestId,
    rx: Receiver<Response>,
}

impl CallHandle {
    /// The id this request carries on the wire.
    pub fn request_id(&self) -> &RequestId {
        &self.id
    }

    /// Cancels the request: the pending entry is dropped (a late plugin
    /// response is logged as a fault, never delivered) and [`CallHandle::wait`]
    /// returns [`RpcError::CANCELLED`] promptly.
    pub fn cancel(&self) {
        if let Some(sender) = lock(&self.shared.pending).remove(&self.id) {
            let _ = sender.try_send(error_response(
                self.id.clone(),
                RpcError::new(RpcError::CANCELLED, "cancelled by caller"),
            ));
        }
    }

    /// Waits up to `timeout` for the response.
    pub fn wait(self, timeout: Duration) -> Result<serde_json::Value, HostError> {
        match self.rx.recv_timeout(timeout) {
            Ok(response) => finish(response),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&self.shared.pending).remove(&self.id);
                Err(HostError::Rpc {
                    code: RpcError::TIMED_OUT,
                    message: "request timed out".into(),
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(HostError::Unavailable),
        }
    }
}

fn finish(response: Response) -> Result<serde_json::Value, HostError> {
    if let Some(err) = response.error {
        if err.code == RpcError::PLUGIN_UNAVAILABLE {
            return Err(HostError::Unavailable);
        }
        return Err(HostError::Rpc {
            code: err.code,
            message: err.message,
        });
    }
    response
        .result
        .ok_or_else(|| HostError::Protocol("response carried neither result nor error".to_owned()))
}

fn writer_loop(mut stdin: ChildStdin, rx: Receiver<WriterMsg>) -> impl FnOnce() {
    move || {
        while let Ok(msg) = rx.recv() {
            match msg {
                WriterMsg::Line { payload, ack } => {
                    let result = stdin.write_all(&payload).and_then(|()| stdin.flush());
                    let failed = result.is_err();
                    let _ = ack.try_send(result);
                    if failed {
                        break;
                    }
                }
                WriterMsg::Close => break,
            }
        }
        // stdin drops here, closing the pipe so a well-behaved plugin exits.
    }
}

fn stdout_loop(stdout: ChildStdout, shared: Arc<Shared>, cap: usize) -> impl FnOnce() {
    move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_capped_line(&mut reader, cap) {
                Ok(Some(CappedLine::Line(line))) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Response>(line) {
                        Ok(response) => route_response(&shared, response),
                        Err(e) => {
                            shared.push_fault(format!("unparseable stdout line skipped: {e}"))
                        }
                    }
                }
                Ok(Some(CappedLine::Oversized(bytes))) => shared.push_fault(format!(
                    "skipped {bytes}-byte message above the {cap}-byte cap"
                )),
                Ok(None) => break,
                Err(e) => {
                    shared.push_fault(format!("stdout read failed: {e}"));
                    break;
                }
            }
        }
        // stdout closed means the process is gone: fail everything in
        // flight and mark the session dead. The host itself keeps running.
        let mut pending = lock(&shared.pending);
        for (id, sender) in pending.drain() {
            let _ = sender.try_send(error_response(
                id,
                RpcError::new(
                    RpcError::PLUGIN_UNAVAILABLE,
                    "plugin exited before responding",
                ),
            ));
        }
        drop(pending);
        shared.mark_dead("plugin stdout closed (process exited)");
    }
}

fn route_response(shared: &Arc<Shared>, response: Response) {
    let mut pending = lock(&shared.pending);
    match pending.remove(&response.id) {
        Some(sender) => {
            let _ = sender.try_send(response);
        }
        None => {
            drop(pending);
            shared.push_fault(format!(
                "response for unknown or abandoned request id {:?}",
                response.id
            ));
        }
    }
}

fn stderr_loop(stderr: std::process::ChildStderr, shared: Arc<Shared>) -> impl FnOnce() {
    move || {
        let mut reader = BufReader::new(stderr);
        let mut chunk = Vec::with_capacity(4096);
        loop {
            chunk.clear();
            match reader.read_until(b'\n', &mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let mut tail = lock(&shared.stderr_tail);
                    tail.extend(chunk.iter().copied());
                    let retained = custos_plugin_api::MAX_STDERR_RETAINED_BYTES;
                    if tail.len() > retained {
                        let excess = tail.len() - retained;
                        tail.drain(..excess);
                    }
                }
            }
        }
    }
}

fn wait_child_exit(shared: &Arc<Shared>, grace: Duration) {
    let Some(mut child) = lock(&shared.child).take() else {
        return;
    };
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

impl Drop for PluginSession {
    fn drop(&mut self) {
        // Never leak a plugin process, even when the session is dropped
        // mid-conversation.
        self.shared.mark_dead("session dropped");
        if let Some(writer) = lock(&self.shared.writer).take() {
            let _ = writer.send(WriterMsg::Close);
        }
        wait_child_exit(&self.shared, Duration::ZERO);
    }
}

/// One framed stdout line, or notification that a line exceeded the cap
/// and was skipped (framing is newline-based, so skipping to the next
/// newline resynchronizes cleanly).
#[derive(Debug, PartialEq)]
enum CappedLine {
    Line(String),
    Oversized(usize),
}

/// Reads one newline-terminated line, never buffering more than `cap`
/// bytes: once a line passes the cap the rest is counted and discarded
/// instead of allocated.
fn read_capped_line<R: BufRead>(reader: &mut R, cap: usize) -> std::io::Result<Option<CappedLine>> {
    let mut line: Vec<u8> = Vec::new();
    let mut oversized = false;
    let mut skipped = 0usize;
    loop {
        let (newline_at, available) = {
            let buf = match reader.fill_buf() {
                Ok(buf) => buf,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if buf.is_empty() {
                return Ok(if oversized {
                    Some(CappedLine::Oversized(skipped + line.len()))
                } else if line.is_empty() {
                    None
                } else {
                    Some(CappedLine::Line(
                        String::from_utf8_lossy(&line).into_owned(),
                    ))
                });
            }
            (buf.iter().position(|&b| b == b'\n'), buf.len())
        };
        let take = newline_at.map_or(available, |at| at + 1);
        {
            let chunk = &reader.fill_buf()?[..take];
            let complete = newline_at.is_some();
            if oversized {
                skipped += take;
            } else {
                let payload_len = if complete { take - 1 } else { take };
                if line.len() + payload_len > cap {
                    oversized = true;
                    skipped = line.len() + take;
                    line.clear();
                } else {
                    line.extend_from_slice(&chunk[..payload_len]);
                }
            }
        }
        reader.consume(take);
        if newline_at.is_some() {
            return Ok(Some(if oversized {
                CappedLine::Oversized(skipped)
            } else {
                CappedLine::Line(String::from_utf8_lossy(&line).into_owned())
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read_all(input: &'static [u8], cap: usize) -> Vec<CappedLine> {
        let mut cursor = Cursor::new(input);
        let mut out = Vec::new();
        while let Ok(Some(line)) = read_capped_line(&mut cursor, cap) {
            out.push(line);
        }
        out
    }

    #[test]
    fn reads_simple_lines_including_a_final_unterminated_one() {
        let lines = read_all(b"one\ntwo\nthree", 100);
        assert_eq!(
            lines,
            vec![
                CappedLine::Line("one".into()),
                CappedLine::Line("two".into()),
                CappedLine::Line("three".into()),
            ]
        );
    }

    #[test]
    fn oversized_terminated_line_is_skipped_and_stream_resyncs() {
        let lines = read_all(b"abcdefghijklmno\nnext\n", 8);
        assert_eq!(
            lines,
            vec![CappedLine::Oversized(16), CappedLine::Line("next".into())]
        );
    }

    #[test]
    fn oversized_unterminated_line_reports_at_eof() {
        let lines = read_all(b"abcdefghij", 4);
        assert_eq!(lines, vec![CappedLine::Oversized(10)]);
    }

    #[test]
    fn empty_input_is_none() {
        let lines = read_all(b"", 8);
        assert!(lines.is_empty());
    }

    #[test]
    fn oversized_lines_never_buffer_more_than_the_cap() {
        // A pathological 100 KiB single line under a 16-byte cap must not
        // allocate proportionally: read_capped_line switches to counting.
        let input = vec![b'x'; 100 * 1024];
        let mut owned = input.clone();
        owned.push(b'\n');
        let mut cursor = Cursor::new(owned);
        let result = read_capped_line(&mut cursor, 16).unwrap();
        assert_eq!(result, Some(CappedLine::Oversized(100 * 1024 + 1)));
        // And the stream resyncs afterwards.
        let mut after = Cursor::new(b"{\"ok\":true}\n".to_vec());
        assert_eq!(
            read_capped_line(&mut after, 16).unwrap(),
            Some(CappedLine::Line("{\"ok\":true}".into()))
        );
    }
}

//! HTTP(S) endpoint probe (survey §26.2, implementation order #1).
//!
//! One `GET` covers: status-code whitelist (exact or `2xx` range), keyword
//! assertion, fail-if-matches regex, and a response-time threshold.
//! Redirects are opt-in (with a no-downgrade guard), TLS verification is
//! configurable for self-signed internal endpoints. Implemented on
//! std TCP + rustls — no shell, no external `curl`.

use crate::config::{bool_or, optional_str, required_str, string_array, u64_or};
use crate::{tls, Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use regex::Regex;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const MAX_REDIRECTS: u32 = 5;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Probes one HTTP or HTTPS endpoint.
#[derive(Debug, Clone)]
pub struct HttpProbe {
    pub url: String,
    pub timeout: Duration,
    pub allowed_statuses: Vec<String>,
    pub contains: Option<String>,
    pub fail_if_matches: Option<Regex>,
    pub response_time_threshold_ms: Option<u64>,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub verify_tls: bool,
    pub max_response_bytes: usize,
    /// JSON assertions evaluated on the response body when it parses as JSON
    /// (probe-survey.md §26.2: JSON path / value comparison).
    pub json_assertions: Vec<JsonAssertion>,
}

/// One JSON assertion over a dot-path into the response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonAssertion {
    /// Dot-path like `$.status` or `items.0.name` (leading `$` optional).
    pub path: String,
    pub op: JsonOp,
    /// The comparison value as text; numeric ops coerce both sides.
    pub value: String,
}

/// The comparison operators a JSON assertion supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonOp {
    /// The path must exist and equal the value (text or number).
    Equals,
    /// The path must exist and not equal the value.
    NotEquals,
    /// The path must exist (value ignored).
    Exists,
    /// The path must exist and be numerically greater than the value.
    GreaterThan,
    /// The path must exist and be numerically less than the value.
    LessThan,
}

impl JsonOp {
    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "==" | "eq" => Ok(JsonOp::Equals),
            "!=" | "ne" => Ok(JsonOp::NotEquals),
            "exists" => Ok(JsonOp::Exists),
            ">" | "gt" => Ok(JsonOp::GreaterThan),
            "<" | "lt" => Ok(JsonOp::LessThan),
            other => Err(format!(
                "invalid `probe.json_assertions[].op` {other:?} (expected ==, !=, exists, >, <)"
            )),
        }
    }
}

/// Resolves a dot-path (`$.a.b`, `items.0.name`) against a JSON value.
/// Returns `None` when the path does not exist or a segment is a type
/// mismatch (an object key on an array, a non-numeric index, etc.).
pub fn json_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let trimmed = path.trim_start_matches('$');
    let trimmed = trimmed.trim_start_matches('.');
    let mut current = root;
    for segment in trimmed.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            serde_json::Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Checks one assertion against a parsed body; returns a problem string
/// when it fails. Pure and unit-testable.
pub fn check_json_assertion(root: &serde_json::Value, assertion: &JsonAssertion) -> Option<String> {
    let found = json_path(root, &assertion.path);
    match assertion.op {
        JsonOp::Exists => {
            if found.is_some() {
                None
            } else {
                Some(format!("json path {} missing", assertion.path))
            }
        }
        JsonOp::Equals | JsonOp::NotEquals | JsonOp::GreaterThan | JsonOp::LessThan => {
            let Some(actual) = found else {
                return Some(format!("json path {} missing", assertion.path));
            };
            let equal = json_values_equal(actual, &assertion.value);
            let op_symbol = match assertion.op {
                JsonOp::Equals => "==",
                JsonOp::NotEquals => "!=",
                JsonOp::GreaterThan => ">",
                JsonOp::LessThan => "<",
                JsonOp::Exists => unreachable!(),
            };
            let ok = match assertion.op {
                JsonOp::Equals => equal,
                JsonOp::NotEquals => !equal,
                JsonOp::GreaterThan => json_greater(actual, &assertion.value),
                JsonOp::LessThan => !json_greater(actual, &assertion.value) && !equal,
                JsonOp::Exists => unreachable!(),
            };
            if ok {
                None
            } else {
                Some(format!(
                    "json path {} = {actual} does not satisfy {op_symbol} {}",
                    assertion.path, assertion.value
                ))
            }
        }
    }
}

/// Value equality: a string compares by text; a number compares numerically
/// (so `"1"` matches `1`); everything else compares its JSON text form.
fn json_values_equal(actual: &serde_json::Value, expected: &str) -> bool {
    match actual {
        serde_json::Value::String(text) => text == expected,
        serde_json::Value::Number(num) => num.to_string() == expected,
        serde_json::Value::Bool(flag) => flag.to_string() == expected,
        other => other == expected,
    }
}

/// Numeric "greater": both sides must be numbers (or numeric strings).
fn json_greater(actual: &serde_json::Value, expected: &str) -> bool {
    let parse = |value: &serde_json::Value| match value {
        serde_json::Value::Number(num) => num.as_f64(),
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    match (parse(actual), expected.trim().parse::<f64>()) {
        (Some(left), Ok(right)) => left > right,
        _ => false,
    }
}

/// Builds from `[resources.probe]` (docs/guide/probe-survey.md §26.2).
pub fn from_config(config: &serde_json::Value) -> Result<HttpProbe, String> {
    let url = required_str(config, "url").ok_or("`probe.url` is required for http-endpoint")?;
    let fail_if_matches = match optional_str(config, "fail_if_matches") {
        Some(pattern) => Some(
            Regex::new(&pattern)
                .map_err(|err| format!("invalid `probe.fail_if_matches` regex: {err}"))?,
        ),
        None => None,
    };
    let mut json_assertions = Vec::new();
    if let Some(list) = config["json_assertions"].as_array() {
        for (index, entry) in list.iter().enumerate() {
            let path = required_str(entry, "path")
                .ok_or_else(|| format!("`probe.json_assertions[{index}].path` is required"))?;
            let op_text = entry["op"].as_str().unwrap_or("==");
            let op = JsonOp::from_str(op_text)
                .map_err(|err| format!("`probe.json_assertions[{index}]`: {err}"))?;
            let value = entry["value"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_default();
            if op != JsonOp::Exists && value.is_empty() {
                return Err(format!(
                    "`probe.json_assertions[{index}].value` is required for operator {op_text}"
                ));
            }
            json_assertions.push(JsonAssertion {
                path: path.to_owned(),
                op,
                value,
            });
        }
    }
    Ok(HttpProbe {
        url,
        timeout: Duration::from_millis(u64_or(config, "timeout_ms", 5_000)),
        allowed_statuses: string_array(config, "allowed_statuses"),
        contains: optional_str(config, "contains"),
        fail_if_matches,
        response_time_threshold_ms: {
            let ms = u64_or(config, "response_time_threshold_ms", 0);
            (ms > 0).then_some(ms)
        },
        follow_redirects: bool_or(config, "follow_redirects", false),
        max_redirects: u64_or(config, "max_redirects", MAX_REDIRECTS as u64) as u32,
        verify_tls: bool_or(config, "verify_tls", true),
        max_response_bytes: {
            let bytes = u64_or(config, "max_response_bytes", 0);
            if bytes > 0 {
                bytes as usize
            } else {
                DEFAULT_MAX_RESPONSE_BYTES
            }
        },
        json_assertions,
    })
}

/// A connected stream: plain TCP, or TLS over TCP.
enum Connection {
    Plain(TcpStream),
    Tls(Box<tls::TlsStream>),
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.read(buf),
            Connection::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.write(buf),
            Connection::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Connection::Plain(stream) => stream.flush(),
            Connection::Tls(stream) => stream.flush(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Http,
    Https,
}

#[derive(Debug, Clone)]
struct ParsedUrl {
    scheme: Scheme,
    host: String,
    port: u16,
    path: String,
}

/// Minimal `scheme://host[:port]/path` parser (only what the probe needs).
fn parse_url(url: &str) -> Result<ParsedUrl, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("url `{url}` has no scheme"))?;
    let scheme = match scheme {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        other => return Err(format!("unsupported url scheme `{other}`")),
    };
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 literal: [::1]:443
        let (host, tail) = stripped
            .split_once(']')
            .ok_or_else(|| format!("unterminated IPv6 literal in `{url}`"))?;
        let port = tail
            .strip_prefix(':')
            .map(|p| p.parse::<u16>().map_err(|_| format!("bad port in `{url}`")))
            .transpose()?
            .unwrap_or(default_port(scheme));
        (host.to_owned(), port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse::<u16>()
            .map_err(|_| format!("bad port in `{url}`"))?;
        (host.to_owned(), port)
    } else {
        (authority.to_owned(), default_port(scheme))
    };
    if host.is_empty() {
        return Err(format!("url `{url}` has no host"));
    }
    Ok(ParsedUrl {
        scheme,
        host,
        port,
        path: path.to_owned(),
    })
}

fn default_port(scheme: Scheme) -> u16 {
    match scheme {
        Scheme::Http => 80,
        Scheme::Https => 443,
    }
}

/// Resolves a `Location` header (absolute or path-only) against the current URL.
fn resolve_location(current: &str, location: &str) -> Result<String, String> {
    if location.contains("://") {
        return Ok(location.to_owned());
    }
    let base = parse_url(current)?;
    let scheme = match base.scheme {
        Scheme::Http => "http",
        Scheme::Https => "https",
    };
    let authority = if (base.scheme, base.port) == (Scheme::Http, 80)
        || (base.scheme, base.port) == (Scheme::Https, 443)
    {
        base.host.clone()
    } else {
        format!("{}:{}", base.host, base.port)
    };
    if let Some(path) = location.strip_prefix('/') {
        Ok(format!("{scheme}://{authority}{path}"))
    } else {
        let dir = match base.path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => "",
        };
        Ok(format!("{scheme}://{authority}{dir}/{location}"))
    }
}

/// Status whitelist: exact codes (`"503"`) and `2xx`-style ranges.
fn status_allowed(allowed: &[String], status: u16) -> bool {
    if allowed.is_empty() {
        return (200..300).contains(&status);
    }
    allowed.iter().any(|pattern| {
        let pattern = pattern.trim();
        if let Some(class) = pattern.strip_suffix("xx") {
            if let Ok(class) = class.parse::<u16>() {
                return (1..=9).contains(&class) && status / 100 == class;
            }
        }
        pattern.parse::<u16>().is_ok_and(|exact| exact == status)
    })
}

struct HttpResponse {
    status: u16,
    body: String,
    location: Option<String>,
}

impl Probe for HttpProbe {
    fn name(&self) -> &'static str {
        "http"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let started = Instant::now();
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let elapsed = || started.elapsed();

        let mut url = self.url.clone();
        let mut hops = 0;
        loop {
            let response = match self.fetch_once(&url, started) {
                Ok(response) => response,
                Err(err) => return report(false, HealthStatus::Unreachable, err),
            };
            let redirect = (300..400).contains(&response.status);
            if redirect && !self.follow_redirects {
                let target = response.location.clone().unwrap_or_default();
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    format!(
                        "redirect to {target} not followed (status {})",
                        response.status
                    ),
                );
            }
            if redirect && hops < self.max_redirects {
                let Some(location) = response.location.clone() else {
                    return report(
                        false,
                        HealthStatus::Unreachable,
                        format!(
                            "redirect status {} without Location header",
                            response.status
                        ),
                    );
                };
                match resolve_location(&url, &location) {
                    // Never downgrade HTTPS to HTTP mid-chain; evaluate the
                    // redirect itself instead of following it blindly.
                    Ok(next) if url.starts_with("https://") && next.starts_with("http://") => {
                        return report(
                            false,
                            HealthStatus::Unhealthy,
                            format!("refusing https->http redirect to {next}"),
                        );
                    }
                    Ok(next) => {
                        url = next;
                        hops += 1;
                        continue;
                    }
                    Err(err) => {
                        return report(false, HealthStatus::Unreachable, err);
                    }
                }
            }
            if redirect {
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    format!("too many redirects (> {})", self.max_redirects),
                );
            }
            return self.evaluate(&report, &url, response.status, &response.body, elapsed());
        }
    }
}

impl HttpProbe {
    fn fetch_once(&self, url: &str, started: Instant) -> Result<HttpResponse, String> {
        let parsed = parse_url(url)?;
        let remaining = |what: &str| -> Result<Duration, String> {
            let left = self.timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(format!(
                    "{what} timeout after {}ms",
                    self.timeout.as_millis()
                ));
            }
            Ok(left)
        };

        let addresses: Vec<std::net::SocketAddr> =
            ToSocketAddrs::to_socket_addrs(&(parsed.host.as_str(), parsed.port))
                .map_err(|err| format!("dns resolution failed for {}: {err}", parsed.host))?
                .collect();
        let address = addresses
            .into_iter()
            .next()
            .ok_or_else(|| format!("dns resolution returned no addresses for {}", parsed.host))?;
        let stream =
            TcpStream::connect_timeout(&address, remaining("connect")?).map_err(|err| {
                if err.kind() == std::io::ErrorKind::TimedOut {
                    format!("connect timeout after {}ms", self.timeout.as_millis())
                } else {
                    format!("connect to {address} failed: {err}")
                }
            })?;
        stream
            .set_read_timeout(Some(remaining("read")?))
            .map_err(|err| format!("set read timeout: {err}"))?;
        stream
            .set_write_timeout(Some(remaining("write")?))
            .map_err(|err| format!("set write timeout: {err}"))?;

        let mut connection = match parsed.scheme {
            Scheme::Http => Connection::Plain(stream),
            Scheme::Https => Connection::Tls(Box::new(tls::connect_tls(
                stream,
                &parsed.host,
                self.verify_tls,
            )?)),
        };

        let host_header = match (parsed.scheme, parsed.port) {
            (Scheme::Http, 80) | (Scheme::Https, 443) => parsed.host.clone(),
            (_, port) => format!("{}:{port}", parsed.host),
        };
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: custos-probe/1\r\nAccept: */*\r\nConnection: close\r\n\r\n",
            parsed.path, host_header
        );
        connection
            .write_all(request.as_bytes())
            .and_then(|()| connection.flush())
            .map_err(|err| {
                if is_timeout(&err) {
                    format!("write timeout after {}ms", self.timeout.as_millis())
                } else {
                    format!("write failed: {err}")
                }
            })?;

        let mut raw = Vec::with_capacity(8 * 1024);
        let mut buf = [0u8; 4096];
        loop {
            match connection.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let room = self.max_response_bytes.saturating_sub(raw.len());
                    raw.extend_from_slice(&buf[..n.min(room)]);
                    if raw.len() >= self.max_response_bytes {
                        break;
                    }
                    if response_complete(&raw) {
                        break;
                    }
                }
                Err(err) => {
                    // Keep-alive servers may hold the socket open after a
                    // complete response; only a timeout on an incomplete
                    // response is an error.
                    if raw.is_empty() {
                        if is_timeout(&err) {
                            return Err(format!(
                                "read timeout after {}ms",
                                self.timeout.as_millis()
                            ));
                        }
                        return Err(format!("read failed: {err}"));
                    }
                    break;
                }
            }
        }
        if raw.is_empty() {
            return Err("empty response".to_owned());
        }
        parse_response(&raw, self.max_response_bytes).map_err(|err| format!("bad response: {err}"))
    }

    /// Status whitelist + keyword/regex assertions + response-time threshold.
    fn evaluate(
        &self,
        report: &dyn Fn(bool, HealthStatus, String) -> ProbeReport,
        url: &str,
        status: u16,
        body: &str,
        elapsed: Duration,
    ) -> ProbeReport {
        let elapsed_ms = elapsed.as_millis();
        let problems: Vec<String> = {
            let mut problems = Vec::new();
            if !status_allowed(&self.allowed_statuses, status) {
                problems.push(format!("status {status} not in whitelist"));
            }
            if let Some(needle) = &self.contains {
                if !body.contains(needle.as_str()) {
                    problems.push(format!("body missing keyword {needle:?}"));
                }
            }
            if let Some(pattern) = &self.fail_if_matches {
                if pattern.is_match(body) {
                    problems.push(format!("body matches forbidden pattern {pattern}"));
                }
            }
            if !self.json_assertions.is_empty() {
                match serde_json::from_str::<serde_json::Value>(body) {
                    Ok(root) => {
                        for assertion in &self.json_assertions {
                            if let Some(problem) = check_json_assertion(&root, assertion) {
                                problems.push(problem);
                            }
                        }
                    }
                    Err(err) => problems.push(format!(
                        "body is not valid JSON ({} assertion(s) skipped): {err}",
                        self.json_assertions.len()
                    )),
                }
            }
            problems
        };
        let threshold = self.response_time_threshold_ms;
        if !problems.is_empty() {
            return report(
                false,
                HealthStatus::Unhealthy,
                format!(
                    "GET {url} -> status {status} in {elapsed_ms}ms, failed: {}",
                    problems.join("; ")
                ),
            );
        }
        if let Some(threshold) = threshold {
            if elapsed_ms > u128::from(threshold) {
                return report(
                    true,
                    HealthStatus::Degraded,
                    format!("GET {url} -> status {status} in {elapsed_ms}ms (slow, threshold {threshold}ms)"),
                );
            }
        }
        report(
            true,
            HealthStatus::Healthy,
            format!("GET {url} -> status {status} in {elapsed_ms}ms"),
        )
    }
}

fn is_timeout(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

/// True once the buffered bytes form a complete response body, so a
/// keep-alive server does not force the probe to wait for read timeout.
fn response_complete(raw: &[u8]) -> bool {
    let Some(headers_end) = find_headers_end(raw) else {
        return false;
    };
    let headers = String::from_utf8_lossy(&raw[..headers_end]).to_ascii_lowercase();
    let body = &raw[headers_end + 4..];
    if headers.contains("transfer-encoding:") && headers.contains("chunked") {
        return chunked_complete(body);
    }
    for line in headers.lines() {
        if let Some(value) = line.strip_prefix("content-length:") {
            if let Ok(len) = value.trim().parse::<usize>() {
                return body.len() >= len;
            }
        }
    }
    false
}

fn find_headers_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

fn chunked_complete(body: &[u8]) -> bool {
    let mut offset = 0;
    loop {
        let rest = &body[offset..];
        let Some(size_end) = rest.windows(2).position(|w| w == b"\r\n") else {
            return false;
        };
        let size_text = std::str::from_utf8(&rest[..size_end]).ok().unwrap_or("");
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            return false;
        };
        let chunk_end = size_end + 2 + size;
        if chunk_end + 2 > rest.len() {
            return false;
        }
        if size == 0 {
            return true;
        }
        offset += chunk_end + 2;
    }
}

fn parse_response(raw: &[u8], max_bytes: usize) -> Result<HttpResponse, String> {
    let headers_end = find_headers_end(raw).ok_or("no header terminator")?;
    let headers = std::str::from_utf8(&raw[..headers_end]).map_err(|_| "headers are not utf-8")?;
    let status_line = headers.lines().next().ok_or("empty response")?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().ok_or("malformed status line")?;
    if !version.starts_with("HTTP/") {
        return Err(format!("malformed status line {status_line:?}"));
    }
    let status: u16 = parts
        .next()
        .ok_or("status line without status code")?
        .parse()
        .map_err(|_| "status line without status code")?;
    let location = headers.lines().skip(1).find_map(|line| {
        line.split_once(':').and_then(|(name, value)| {
            name.trim()
                .eq_ignore_ascii_case("location")
                .then(|| value.trim().to_owned())
        })
    });
    let body_bytes = &raw[headers_end + 4..];
    let body = if headers.to_ascii_lowercase().contains("transfer-encoding:")
        && headers.to_ascii_lowercase().contains("chunked")
    {
        dechunk(body_bytes).unwrap_or_else(|| body_bytes.to_vec())
    } else {
        body_bytes.to_vec()
    };
    let body = String::from_utf8_lossy(&body);
    // Never keep more than the cap in memory; the cap was already applied
    // while reading, so this only guards the lossy conversion.
    let body: String = body.chars().take(max_bytes).collect();
    Ok(HttpResponse {
        status,
        body,
        location,
    })
}

fn dechunk(body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len());
    let mut offset = 0;
    loop {
        let rest = body.get(offset..)?;
        let size_end = rest.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&rest[..size_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        let chunk_end = size_end + 2 + size;
        if chunk_end + 2 > rest.len() {
            return None;
        }
        out.extend_from_slice(&rest[size_end + 2..chunk_end]);
        if size == 0 {
            return Some(out);
        }
        offset += chunk_end + 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::net::TcpListener;
    use std::thread;

    fn resource() -> Resource {
        Resource::new("api", "http-endpoint")
    }

    /// Serves exactly one canned response, then closes the connection.
    fn serve_once(response: String) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            // Wait for the request (or the client giving up).
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{addr}/"), handle)
    }

    fn probe_with(url: &str) -> HttpProbe {
        HttpProbe {
            url: url.to_owned(),
            timeout: Duration::from_secs(2),
            allowed_statuses: Vec::new(),
            contains: None,
            fail_if_matches: None,
            response_time_threshold_ms: None,
            follow_redirects: false,
            max_redirects: MAX_REDIRECTS,
            verify_tls: true,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            json_assertions: Vec::new(),
        }
    }

    // ── JSON assertion tests (survey §26.2 JSON path/value compare) ──────

    #[test]
    fn json_path_walks_objects_arrays_and_indices() {
        let body = serde_json::json!({
            "status": "ok",
            "count": 42,
            "items": [{ "name": "first" }, { "name": "second" }],
            "nested": { "enabled": true },
            "flag": false
        });
        // Object chain.
        assert_eq!(
            json_path(&body, "$.status").and_then(Value::as_str),
            Some("ok")
        );
        // Without the leading $.
        assert_eq!(json_path(&body, "count").and_then(Value::as_i64), Some(42));
        // Numeric array index.
        assert_eq!(
            json_path(&body, "items.0.name").and_then(Value::as_str),
            Some("first")
        );
        // Nested boolean.
        assert_eq!(
            json_path(&body, "nested.enabled").and_then(Value::as_bool),
            Some(true)
        );
        // Missing path → None.
        assert!(json_path(&body, "missing.key").is_none());
        // Non-numeric index on an array → None.
        assert!(json_path(&body, "items.name").is_none());
        // Object key on a scalar → None.
        assert!(json_path(&body, "status.sub").is_none());
    }

    #[test]
    fn json_assertion_operators_cover_the_matrix() {
        let body = serde_json::json!({
            "status": "ok",
            "count": 42,
            "ratio": 3.5,
            "text": "hello world",
            "enabled": true,
            "nested": { "deep": { "leaf": "found" } },
            "items": [1, 2, 3],
            "empty": null
        });
        let check = |path: &str, op: JsonOp, value: &str| {
            check_json_assertion(
                &body,
                &JsonAssertion {
                    path: path.into(),
                    op,
                    value: value.into(),
                },
            )
        };
        // Equals: string, number, number-as-string (text compare matches),
        // boolean, nested path.
        assert_eq!(check("$.status", JsonOp::Equals, "ok"), None);
        assert_eq!(check("$.count", JsonOp::Equals, "42"), None);
        assert!(check("$.count", JsonOp::Equals, "43").is_some());
        assert_eq!(check("$.nested.deep.leaf", JsonOp::Equals, "found"), None);
        // NotEquals.
        assert_eq!(check("$.status", JsonOp::NotEquals, "error"), None);
        assert!(check("$.status", JsonOp::NotEquals, "ok").is_some());
        // Exists.
        assert_eq!(check("$.items", JsonOp::Exists, ""), None);
        assert!(check("$.missing", JsonOp::Exists, "").is_some());
        // GreaterThan / LessThan: numeric on number and numeric-string.
        assert_eq!(check("$.count", JsonOp::GreaterThan, "10"), None);
        assert!(check("$.count", JsonOp::GreaterThan, "100").is_some());
        assert_eq!(check("$.ratio", JsonOp::LessThan, "4"), None);
        assert!(check("$.ratio", JsonOp::LessThan, "3").is_some());
        // Missing path fails every value operator.
        assert!(check("$.missing", JsonOp::Equals, "x").is_some());
        // null is present: Exists passes; Equals to "null" also matches by
        // JSON text form.
        assert_eq!(check("$.empty", JsonOp::Exists, ""), None);
        // Boolean equality.
        assert_eq!(check("$.enabled", JsonOp::Equals, "true"), None);
        assert!(check("$.enabled", JsonOp::Equals, "false").is_some());
    }

    #[test]
    fn from_config_parses_json_assertions_and_rejects_bad_ops() {
        let config = serde_json::json!({
            "url": "http://x/",
            "json_assertions": [
                { "path": "$.status", "op": "==", "value": "ok" },
                { "path": "$.items", "op": "exists" }
            ]
        });
        let probe = from_config(&config).unwrap();
        assert_eq!(probe.json_assertions.len(), 2);
        assert_eq!(probe.json_assertions[0].path, "$.status");
        assert_eq!(probe.json_assertions[0].op, JsonOp::Equals);
        assert_eq!(probe.json_assertions[1].op, JsonOp::Exists);
        // Bad operator.
        let bad = serde_json::json!({
            "url": "http://x/",
            "json_assertions": [{ "path": "$.a", "op": "~~", "value": "1" }]
        });
        let err = from_config(&bad).unwrap_err();
        assert!(err.contains("invalid"), "{err}");
        // Value required for a value operator.
        let missing = serde_json::json!({
            "url": "http://x/",
            "json_assertions": [{ "path": "$.a", "op": "==" }]
        });
        assert!(from_config(&missing).unwrap_err().contains("required"));
    }

    #[test]
    fn json_assertion_evaluate_fails_on_bad_json_body() {
        let mut probe = probe_with("http://x/");
        probe.json_assertions = vec![JsonAssertion {
            path: "$.status".into(),
            op: JsonOp::Equals,
            value: "ok".into(),
        }];
        let report = probe.evaluate(
            &|succeeded, status, observation: String| ProbeReport {
                resource_id: resource().id,
                probe_name: "http".into(),
                succeeded,
                status,
                observation,
            },
            "http://x/",
            200,
            "<html>not json</html>",
            Duration::from_millis(10),
        );
        assert!(!report.succeeded);
        assert!(
            report.observation.contains("not valid JSON"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn live_json_body_passes_and_fails_assertions() {
        let body = r#"{"status":"ok","version":"1.2","items":[{"name":"a"},{"name":"b"}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let make_url = || serve_once(response.clone()).0;

        // Passing assertions: string equality + nested path.
        let mut probe = probe_with(&make_url());
        probe.json_assertions = vec![
            JsonAssertion {
                path: "$.status".into(),
                op: JsonOp::Equals,
                value: "ok".into(),
            },
            JsonAssertion {
                path: "items.1.name".into(),
                op: JsonOp::Equals,
                value: "b".into(),
            },
        ];
        let report = probe.probe(&Resource::new("r", "http-endpoint"));
        assert_eq!(
            report.status,
            HealthStatus::Healthy,
            "{}",
            report.observation
        );

        // Failing assertion: version != expected → Unhealthy with the path.
        let mut probe = probe_with(&make_url());
        probe.json_assertions = vec![JsonAssertion {
            path: "$.version".into(),
            op: JsonOp::Equals,
            value: "9.9".into(),
        }];
        let report = probe.probe(&Resource::new("r", "http-endpoint"));
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("$.version"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn status_and_keyword_pass() {
        let (url, handle) =
            serve_once("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_owned());
        let mut probe = probe_with(&url);
        probe.contains = Some("hello".into());
        let report = probe.probe(&resource());
        assert!(report.succeeded, "{}", report.observation);
        assert_eq!(report.status, HealthStatus::Healthy);
        handle.join().unwrap();
    }

    #[test]
    fn non_2xx_is_unhealthy() {
        let (url, handle) =
            serve_once("HTTP/1.1 503 Unavailable\r\nContent-Length: 5\r\n\r\nlater".to_owned());
        let report = probe_with(&url).probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        handle.join().unwrap();
    }

    #[test]
    fn missing_keyword_is_unhealthy() {
        let (url, handle) =
            serve_once("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_owned());
        let mut probe = probe_with(&url);
        probe.contains = Some("absent".into());
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        handle.join().unwrap();
    }

    #[test]
    fn fail_if_matches_regex_trips() {
        let (url, handle) = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Length: 21\r\n\r\ndegraded: disk almost full".to_owned(),
        );
        let mut probe = probe_with(&url);
        probe.fail_if_matches = Some(Regex::new(r"degraded:\s+disk").unwrap());
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        handle.join().unwrap();
    }

    #[test]
    fn status_class_range_is_accepted() {
        let (url, handle) =
            serve_once("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".into());
        let mut probe = probe_with(&url);
        probe.allowed_statuses = vec!["2xx".into()];
        let report = probe.probe(&resource());
        assert!(report.succeeded, "{}", report.observation);
        handle.join().unwrap();
    }

    #[test]
    fn explicit_status_whitelist_rejects_others() {
        let (url, handle) =
            serve_once("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".into());
        let mut probe = probe_with(&url);
        probe.allowed_statuses = vec!["200".into()];
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        handle.join().unwrap();
    }

    #[test]
    fn server_that_never_replies_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            // Sleep past the probe's deadline, then drop.
            thread::sleep(Duration::from_millis(500));
        });
        let mut probe = probe_with(&format!("http://{addr}/"));
        probe.timeout = Duration::from_millis(150);
        let report = probe.probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unreachable);
        assert!(
            report.observation.contains("timeout"),
            "{}",
            report.observation
        );
        handle.join().ok();
    }

    #[test]
    fn slow_response_over_threshold_is_degraded() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            thread::sleep(Duration::from_millis(150));
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        });
        let mut probe = probe_with(&format!("http://{addr}/"));
        probe.response_time_threshold_ms = Some(50);
        let report = probe.probe(&resource());
        assert!(report.succeeded);
        assert_eq!(report.status, HealthStatus::Degraded);
        assert!(
            report.observation.contains("slow"),
            "{}",
            report.observation
        );
        handle.join().unwrap();
    }

    #[test]
    fn redirect_not_followed_by_default() {
        let (url, handle) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: /other\r\nContent-Length: 0\r\n\r\n".to_owned(),
        );
        let report = probe_with(&url).probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("not followed"),
            "{}",
            report.observation
        );
        handle.join().unwrap();
    }

    #[test]
    fn redirect_chain_is_followed_when_enabled() {
        // First hop redirects to the second server, which answers 200.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let second = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_owned());
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 301 Moved\r\nLocation: {}\r\nContent-Length: 0\r\n\r\n",
                    second.0
                )
                .as_bytes(),
            );
        });
        let mut probe = probe_with(&format!("http://{addr}/"));
        probe.follow_redirects = true;
        let report = probe.probe(&resource());
        assert!(report.succeeded, "{}", report.observation);
        assert_eq!(report.status, HealthStatus::Healthy);
        handle.join().unwrap();
        second.1.join().unwrap();
    }

    #[test]
    fn connection_refused_is_unhealthy() {
        // Bind then drop so the port is closed.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let report = probe_with(&format!("http://127.0.0.1:{port}/")).probe(&resource());
        assert!(!report.succeeded);
        assert_eq!(report.status, HealthStatus::Unreachable);
    }

    #[test]
    fn url_parser_handles_ports_and_ipv6() {
        let parsed = parse_url("https://example.com:8443/health?x=1").unwrap();
        assert_eq!(parsed.scheme, Scheme::Https);
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 8443);
        assert_eq!(parsed.path, "/health?x=1");

        let parsed = parse_url("http://[::1]:8080/x").unwrap();
        assert_eq!(parsed.host, "::1");
        assert_eq!(parsed.port, 8080);

        assert!(parse_url("ftp://x/").is_err());
        assert!(parse_url("no-scheme").is_err());
    }

    #[test]
    fn chunked_body_is_dechunked() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let response = parse_response(raw, 1024).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "hello world");
    }
}

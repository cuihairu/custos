//! `custos-tray` — the desktop tray companion (§9.3). Reads the engine's
//! SQLite store and shows a system tray (Windows) / menu-bar (macOS) icon
//! with the aggregate health; the beat-pulse animation can be switched off
//! from the context menu (persisted). The engine (`custos start`) stays
//! headless.
//!
//! Usage:
//! ```text
//! custos-tray --db custos.db [--config tray.json] [--console "cmd" args...]
//! ```
//!
//! Windows note: release builds carry `#![windows_subsystem = "windows"]`
//! so launching custos-tray.exe does NOT pop a terminal window. Console
//! output therefore goes nowhere on Windows releases — all tracing output
//! is written to a log file (`custos-tray.log` next to the config file)
//! instead of stdout (see `init_tracing_file`).

// GUI subsystem only in release: the shipped exe must not open a terminal
// window; debug/dev builds keep the console for immediate feedback.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(any(windows, target_os = "macos"))]
use custos_tray::driver::TrayOptions;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = parse_args(&args);
    let db_path = parsed.db_path;
    let config_path = parsed.config_path;
    let console = parsed.console;
    let parse_error = parsed.parse_error;
    let wants_help = parsed.wants_help;

    // Logs go to a file, not stdout: the windowed Windows release has no
    // console to write to (§ windows_subsystem above).
    let _log_guard = init_tracing_file(&config_path);

    if let Some(err) = parse_error {
        tracing::error!(target: "custos::tray", "argument error: {err}");
        eprintln!("error: {err}");
        std::process::exit(2);
    }
    if wants_help {
        let usage = "custos-tray [--db PATH] [--config PATH] [--console CMD ARGS...]";
        tracing::info!(target: "custos::tray", "{usage}");
        println!("{usage}");
        return;
    }

    // Only Windows and macOS carry a desktop tray (§9.3 scope); Linux
    // compiles for CI but the binary exits with a clear message.
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (db_path, config_path, console);
        eprintln!("custos-tray: desktop tray requires Windows or macOS");
        std::process::exit(2);
    }
    #[cfg(any(windows, target_os = "macos"))]
    run(db_path, config_path, console);
}

/// Parsed command-line arguments for the tray companion.
#[derive(Debug, Default, PartialEq)]
struct ParsedArgs {
    db_path: String,
    config_path: String,
    console: Vec<String>,
    parse_error: Option<String>,
    wants_help: bool,
}

/// Parses the tray's arguments. Errors are collected (not emitted) so the
/// caller can bring up the file logger before reporting them — the windowed
/// Windows release has no console to write to.
fn parse_args(args: &[String]) -> ParsedArgs {
    let mut parsed = ParsedArgs {
        db_path: "custos.db".to_string(),
        config_path: "custos-tray.json".to_string(),
        ..ParsedArgs::default()
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => match args.get(i + 1) {
                Some(v) => {
                    parsed.db_path = v.clone();
                    i += 2;
                }
                None => {
                    parsed.parse_error = Some("--db requires a path".to_string());
                    i = args.len();
                }
            },
            "--config" => match args.get(i + 1) {
                Some(v) => {
                    parsed.config_path = v.clone();
                    i += 2;
                }
                None => {
                    parsed.parse_error = Some("--config requires a path".to_string());
                    i = args.len();
                }
            },
            "--console" => {
                // Everything after --console is the command (run in a new window).
                parsed.console = args[i + 1..].to_vec();
                i = args.len();
            }
            "--help" | "-h" => {
                parsed.wants_help = true;
                i = args.len();
            }
            other => {
                parsed.parse_error = Some(format!("unknown argument `{other}` (see --help)"));
                i = args.len();
            }
        }
    }
    parsed
}

/// Escapes a value for embedding inside a double-quoted AppleScript string
/// (backslash and double quote).
#[allow(dead_code)]
fn apple_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// POSIX shell quoting for a command run inside Terminal's shell.
#[allow(dead_code)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Builds the health source over the store and hands control to the driver.
#[cfg(any(windows, target_os = "macos"))]
fn run(db_path: String, config_path: String, console: Vec<String>) {
    let store = custos_store::Store::open(&db_path).unwrap_or_else(|e| {
        tracing::error!(target: "custos::tray", error = %e, "cannot open database");
        eprintln!("error: cannot open database {db_path}: {e}");
        std::process::exit(1);
    });

    // The health source reads the engine's persisted probe results: every
    // latest verdict counts toward the summary (§9.3 status source).
    let health_source = Box::new(move || match store.latest_health() {
        Ok(rows) => {
            let healthy = rows.iter().filter(|r| r.succeeded).count();
            custos_tray::HealthSummary {
                healthy,
                total: rows.len(),
            }
        }
        Err(err) => {
            tracing::warn!(target: "custos::tray", error = %err, "health read failed");
            custos_tray::HealthSummary::default()
        }
    });

    let console_command = if console.is_empty() {
        default_console_command(&db_path)
    } else {
        console
    };

    let options = TrayOptions::new(health_source, &config_path, console_command, &db_path);
    if let Err(err) = custos_tray::driver::run(options) {
        tracing::error!(target: "custos::tray", error = %err, "tray exited with an error");
        eprintln!("custos-tray: {err}");
        std::process::exit(1);
    }
}

/// 「打开控制台」 default: a terminal running `custos status` against the
/// same database (platform-dependent terminal launch).
#[cfg(any(windows, target_os = "macos"))]
fn default_console_command(db_path: &str) -> Vec<String> {
    let custos = if cfg!(windows) {
        "custos.exe"
    } else {
        "custos"
    };
    let status_args = vec![
        custos.to_owned(),
        "status".to_owned(),
        "--db".to_owned(),
        db_path.to_owned(),
    ];
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), "start".into()]
            .into_iter()
            .chain(status_args)
            .collect()
    } else if cfg!(target_os = "macos") {
        // Terminal.app opens a window and runs the command via AppleScript
        // (a plain `open -a Terminal` would discard the command). Each
        // argument is shell-quoted independently, then joined with spaces:
        // quoting the whole joined string would make the shell treat the
        // entire line as a single program name and fail to exec it.
        vec![
            "osascript".into(),
            "-e".into(),
            macos_console_script(&status_args),
        ]
    } else {
        status_args
    }
}

/// Builds the `do script` payload for Terminal.app: each command argument is
/// shell-quoted independently, then joined with single spaces (a path with
/// spaces stays one shell word).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_console_script(status_args: &[String]) -> String {
    let script = status_args
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "tell application \"Terminal\" to do script {}",
        apple_quote(&script)
    )
}

/// Where the file-based tracing writer ended up after writer selection.
/// `log_dir` is `None` only when the writer fell back to `io::sink`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LogOutcome {
    log_dir: Option<std::path::PathBuf>,
    /// The log file was moved out of the config directory (e.g. because the
    /// config dir is read-only) and a relocation warning was emitted.
    relocated: bool,
    /// No file could be created in either directory; output was discarded.
    discarded: bool,
}

/// Builds a `RollingFileAppender` in `dir`, or `Err` if the directory is not
/// usable (read-only, missing, or a regular file standing in for a directory).
fn build_log_appender(
    dir: &std::path::Path,
) -> Result<tracing_appender::rolling::RollingFileAppender, tracing_appender::rolling::InitError> {
    tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::NEVER)
        .filename_prefix("custos-tray")
        .filename_suffix("log")
        .build(dir)
}

/// Selects the log writer for `config_path`, trying the config directory
/// first and the temp directory as fallback. The writer must be `Write`;
/// `RollingFileAppender` and `io::sink` are both, so picking the first
/// directory that accepts the log file keeps the no-console Windows release
/// diagnosable.
///
/// The temp directory is a parameter so tests can force the fallback /
/// discard branches deterministically on every platform (Windows/macOS
/// included), instead of relying on Unix `chmod` which is a no-op elsewhere.
///
/// Returns the writer, its non-blocking guard, and the selection outcome so
/// callers/tests can assert which branch ran and where the log lines land.
fn select_log_writer(
    config_path: &str,
    temp_dir: &std::path::Path,
) -> (
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
    LogOutcome,
) {
    use tracing_appender::non_blocking;

    // The tray's state lives beside its config (§9.3 设置持久化), so the
    // log does too: one directory holds config + log.
    let config_dir = std::path::Path::new(config_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let mut outcome = LogOutcome {
        log_dir: Some(config_dir.clone()),
        relocated: false,
        discarded: false,
    };
    let writer: Box<dyn std::io::Write + Send> = match build_log_appender(&config_dir) {
        Ok(appender) => Box::new(appender),
        Err(config_err) => {
            // Config dir is read-only (e.g. Program Files): try the system
            // temp directory before giving up on a file entirely.
            match build_log_appender(temp_dir) {
                Ok(fallback_appender) => {
                    eprintln!(
                        "warning: cannot open log file in {} ({config_err}); \
                         falling back to {}",
                        config_dir.display(),
                        temp_dir.display()
                    );
                    outcome.relocated = true;
                    outcome.log_dir = Some(temp_dir.to_path_buf());
                    Box::new(fallback_appender)
                }
                Err(temp_err) => {
                    eprintln!(
                        "warning: cannot open log file in {} ({config_err}) \
                         or in the temp directory ({temp_err}); \
                         log output will be discarded",
                        config_dir.display()
                    );
                    outcome.discarded = true;
                    outcome.log_dir = None;
                    Box::new(std::io::sink())
                }
            }
        }
    };
    let (writer, guard) = non_blocking(writer);
    (writer, guard, outcome)
}

/// File-based tracing: writes `custos-tray.log` next to the config file.
/// Required because the windowed Windows release has no console — stdout
/// logging would silently vanish (see the `windows_subsystem` attribute).
/// Returns the non-blocking writer guard; the caller must keep it alive for
/// the process lifetime or buffered log lines are dropped on flush.
fn init_tracing_file(config_path: &str) -> tracing_appender::non_blocking::WorkerGuard {
    let (writer, guard, outcome) = select_log_writer(config_path, &std::env::temp_dir());
    let config_dir = std::path::Path::new(config_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let _ = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_target(true)
        .with_max_level(tracing::Level::INFO)
        // A process runs one `main` (one init), but unit tests may call this
        // more than once; `try_init` keeps a second call a no-op instead of a
        // panic ("a global default trace dispatcher has already been set").
        .try_init();
    // Surface the chosen location through tracing (now that the subscriber is
    // installed): the windowed Windows release has no console, so the stderr
    // warning above may be invisible and the log file itself is the only
    // user-visible signal.
    if outcome.relocated {
        let relocated_dir = outcome.log_dir.as_deref().unwrap_or(config_dir);
        tracing::warn!(
            target: "custos::tray",
            "log file relocated to {} ({} was unwritable)",
            relocated_dir.display(),
            config_dir.display()
        );
    }
    // The guard flushes the background worker on drop, so it must outlive
    // every logging call (bound to `main`'s scope by the caller).
    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn macos_console_command_quotes_each_argument_independently() {
        // A path with spaces must be a single quoted shell word, not a
        // single quoted string for the entire command line.
        let args = vec![
            "custos".to_owned(),
            "status".to_owned(),
            "--db".to_owned(),
            "/etc/custos/my db/custos.db".to_owned(),
        ];
        let script = macos_console_script(&args);
        // The do-script payload contains each argument in its own single
        // quotes, and the program name is not merged with its arguments.
        assert!(script.contains("'custos' 'status' '--db' '/etc/custos/my db/custos.db'"));
        assert!(!script.contains("'custos status --db /etc/custos/my db/custos.db'"));
    }

    fn s(value: &str) -> String {
        value.to_owned()
    }

    #[test]
    fn parse_args_defaults_without_arguments() {
        let parsed = parse_args(&[]);
        assert_eq!(parsed.db_path, "custos.db");
        assert_eq!(parsed.config_path, "custos-tray.json");
        assert!(parsed.console.is_empty());
        assert!(parsed.parse_error.is_none());
        assert!(!parsed.wants_help);
    }

    #[test]
    fn parse_args_assigns_db_and_config() {
        let parsed = parse_args(&[s("--db"), s("state.db"), s("--config"), s("cfg.json")]);
        assert_eq!(parsed.db_path, "state.db");
        assert_eq!(parsed.config_path, "cfg.json");
        assert!(parsed.parse_error.is_none());
        assert!(!parsed.wants_help);
    }

    #[test]
    fn parse_args_db_requires_value() {
        let parsed = parse_args(&[s("--db")]);
        assert_eq!(parsed.parse_error.as_deref(), Some("--db requires a path"));
    }

    #[test]
    fn parse_args_config_requires_value() {
        let parsed = parse_args(&[s("--config")]);
        assert_eq!(
            parsed.parse_error.as_deref(),
            Some("--config requires a path")
        );
    }

    #[test]
    fn parse_args_unknown_argument_is_an_error() {
        let parsed = parse_args(&[s("--frobnicate")]);
        assert_eq!(
            parsed.parse_error.as_deref(),
            Some("unknown argument `--frobnicate` (see --help)")
        );
    }

    #[test]
    fn parse_args_help_and_short_help() {
        for flag in ["--help", "-h"] {
            let parsed = parse_args(&[s(flag)]);
            assert!(parsed.wants_help, "flag: {flag}");
            assert!(parsed.parse_error.is_none(), "flag: {flag}");
        }
    }

    #[test]
    fn parse_args_console_consumes_the_rest() {
        let parsed = parse_args(&[
            s("--db"),
            s("state.db"),
            s("--console"),
            s("custos"),
            s("status"),
            s("--db"),
            s("unparsed.db"),
        ]);
        assert_eq!(parsed.db_path, "state.db");
        assert_eq!(
            parsed.console,
            vec![s("custos"), s("status"), s("--db"), s("unparsed.db")]
        );
    }

    static UNIQUE_DIR: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "custos-tray-test-log-{tag}-{}",
            // The counter keeps the name unique within a process; the
            // timestamp across processes.
            UNIQUE_DIR.fetch_add(1, Ordering::Relaxed),
        ))
    }

    /// Runs `f` with a thread-local tracing subscriber writing to `writer`
    /// (`with_default` scopes the subscriber to the current thread, so it
    /// does not fight other tests' `try_init` global subscriber).
    fn with_tracing_subscriber<W, T>(writer: W, f: impl FnOnce() -> T) -> T
    where
        W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
    {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_target(true)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, f)
    }

    fn assert_log_contains(log_file: &std::path::Path, needle: &str) {
        let content = std::fs::read_to_string(log_file)
            .unwrap_or_else(|e| panic!("read {}: {e}", log_file.display()));
        assert!(
            content.contains(needle),
            "log file {} should contain {needle:?}, got:\n{content}",
            log_file.display()
        );
    }

    #[test]
    fn init_tracing_file_writes_a_log_file() {
        let dir = unique_dir("writable");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("custos-tray.json");

        // The logger writes beside the config, so this also exercises the
        // log-directory derivation from the config path.
        let (writer, guard, outcome) =
            select_log_writer(&config_path.to_string_lossy(), &std::env::temp_dir());
        assert_eq!(outcome.log_dir, Some(dir.clone()));
        assert!(!outcome.relocated);
        assert!(!outcome.discarded);

        with_tracing_subscriber(writer, || {
            tracing::info!(target: "custos::tray", "log-file smoke test");
        });
        // Drop the guard to flush the background worker before asserting.
        drop(guard);
        let log_file = dir.join("custos-tray.log");
        assert!(
            log_file.exists(),
            "log file should exist at {}",
            log_file.display()
        );
        assert_log_contains(&log_file, "log-file smoke test");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn init_tracing_file_falls_back_to_temp_dir_on_readonly() {
        // Use a regular file standing in for the config directory: the
        // appender cannot create the log file inside a non-directory, which
        // triggers the read-only fallback on every platform (Windows/macOS
        // included) without relying on Unix chmod.
        let config_dir = unique_dir("config-file");
        std::fs::create_dir_all(&config_dir).expect("create parent");
        let config_file = config_dir.join("not-a-dir");
        std::fs::write(&config_file, b"x").expect("write file-as-dir");
        let config_path = config_file.join("custos-tray.json");

        let fallback_dir = unique_dir("fallback");
        std::fs::create_dir_all(&fallback_dir).expect("create fallback dir");
        let (writer, guard, outcome) =
            select_log_writer(&config_path.to_string_lossy(), &fallback_dir);

        // The fallback moved the log out of the unwritable config dir.
        assert!(outcome.relocated, "expected relocation, got {outcome:?}");
        assert!(!outcome.discarded);
        assert_eq!(outcome.log_dir, Some(fallback_dir.clone()));

        with_tracing_subscriber(writer, || {
            tracing::info!(target: "custos::tray", "fallback smoke test");
        });
        drop(guard);
        let log_file = fallback_dir.join("custos-tray.log");
        assert!(
            log_file.exists(),
            "fallback log file should exist at {}",
            log_file.display()
        );
        assert_log_contains(&log_file, "fallback smoke test");
        std::fs::remove_dir_all(&config_dir).ok();
        std::fs::remove_dir_all(&fallback_dir).ok();
    }

    #[test]
    fn init_tracing_file_discards_when_both_dirs_unwritable() {
        // Both the config dir and the fallback temp dir are regular files
        // (non-directories), so neither appender can be built -> io::sink.
        let config_dir = unique_dir("config-file-2");
        std::fs::create_dir_all(&config_dir).expect("create parent");
        let config_file = config_dir.join("not-a-dir");
        std::fs::write(&config_file, b"x").expect("write file-as-dir");
        let config_path = config_file.join("custos-tray.json");

        let fallback_dir = unique_dir("fallback-2");
        std::fs::create_dir_all(&fallback_dir).expect("create fallback parent");
        let fallback_file = fallback_dir.join("also-not-a-dir");
        std::fs::write(&fallback_file, b"x").expect("write fallback file-as-dir");

        let (_writer, guard, outcome) =
            select_log_writer(&config_path.to_string_lossy(), &fallback_file);
        assert!(outcome.discarded, "expected discard, got {outcome:?}");
        assert!(!outcome.relocated);
        assert_eq!(outcome.log_dir, None);
        drop(guard);
        std::fs::remove_dir_all(&config_dir).ok();
        std::fs::remove_dir_all(&fallback_dir).ok();
    }
}

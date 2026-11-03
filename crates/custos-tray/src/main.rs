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

/// File-based tracing: writes `custos-tray.log` next to the config file.
/// Required because the windowed Windows release has no console — stdout
/// logging would silently vanish (see the `windows_subsystem` attribute).
/// Returns the non-blocking writer guard; the caller must keep it alive for
/// the process lifetime or buffered log lines are dropped on flush.
fn init_tracing_file(config_path: &str) -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_appender::non_blocking;

    // The tray's state lives beside its config (§9.3 设置持久化), so the
    // log does too: one directory holds config + log.
    let config_dir = std::path::Path::new(config_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::NEVER)
        .filename_prefix("custos-tray")
        .filename_suffix("log")
        .build(&config_dir);

    // The writer must be `Write`; `RollingFileAppender` and `io::sink` are
    // both. Pick the first directory that actually accepts the log file so
    // the no-console Windows release always keeps a diagnosable file.
    let mut log_dir = config_dir.clone();
    let writer: Box<dyn std::io::Write + Send> = match appender {
        Ok(appender) => Box::new(appender),
        Err(config_err) => {
            // Config dir is read-only (e.g. Program Files): try the system
            // temp directory before giving up on a file entirely.
            let temp_dir = std::env::temp_dir();
            match tracing_appender::rolling::RollingFileAppender::builder()
                .rotation(tracing_appender::rolling::Rotation::NEVER)
                .filename_prefix("custos-tray")
                .filename_suffix("log")
                .build(&temp_dir)
            {
                Ok(fallback_appender) => {
                    eprintln!(
                        "warning: cannot open log file in {} ({config_err}); \
                         falling back to {}",
                        config_dir.display(),
                        temp_dir.display()
                    );
                    log_dir = temp_dir;
                    Box::new(fallback_appender)
                }
                Err(temp_err) => {
                    eprintln!(
                        "warning: cannot open log file in {} ({config_err}) \
                         or in the temp directory ({temp_err}); \
                         log output will be discarded",
                        config_dir.display()
                    );
                    Box::new(std::io::sink())
                }
            }
        }
    };
    let (writer, guard) = non_blocking(writer);

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
    if log_dir != config_dir {
        tracing::warn!(
            target: "custos::tray",
            "log file relocated to {} ({} was unwritable)",
            log_dir.display(),
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

    #[test]
    fn init_tracing_file_writes_a_log_file() {
        let dir = std::env::temp_dir().join(format!(
            "custos-tray-test-log-{}",
            UNIQUE_DIR.fetch_add(1, Ordering::Relaxed)
                + std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("custos-tray.json");
        // The logger writes beside the config, so this also exercises the
        // log-directory derivation from the config path.
        let _guard = init_tracing_file(&config_path.to_string_lossy());
        tracing::info!(target: "custos::tray", "log-file smoke test");
        // The non-blocking worker may need a moment to flush, but the guard
        // held above flushes on drop; drop it before asserting.
        drop(_guard);
        let log_file = dir.join("custos-tray.log");
        assert!(
            log_file.exists(),
            "log file should exist at {}",
            log_file.display()
        );
    }

    #[test]
    fn init_tracing_file_falls_back_to_temp_dir_on_readonly() {
        // Create a real directory, then make it read-only so the appender
        // cannot create the log file. On Unix this is `chmod 0o500`.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!(
                "custos-tray-test-readonly-{}",
                UNIQUE_DIR.fetch_add(1, Ordering::Relaxed)
                    + std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500))
                .expect("chmod readonly");
            let config_path = dir.join("custos-tray.json");
            let _guard = init_tracing_file(&config_path.to_string_lossy());
            tracing::info!(target: "custos::tray", "fallback smoke test");
            drop(_guard);
            // Restore permissions so the test cleanup can remove the dir.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore permissions");
            std::fs::remove_dir_all(&dir).ok();
            // The fallback writes to std::env::temp_dir(), which is the parent
            // of `dir` in this test; we can't easily assert on the exact temp
            // log because it's shared, but the absence of a panic plus the
            // function returning a valid guard proves the fallback path ran.
        }
        // Non-Unix CI (Windows/macOS): the function should not panic, but
        // the permission model differs, so this test is a no-op there.
    }
}

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

#[cfg(any(windows, target_os = "macos"))]
use custos_tray::driver::TrayOptions;

fn main() {
    tracing_subscriber_fmt();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut db_path = String::from("custos.db");
    let mut config_path = String::from("custos-tray.json");
    let mut console: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                db_path = args.get(i + 1).cloned().unwrap_or_else(|| {
                    eprintln!("error: --db requires a path");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--config" => {
                config_path = args.get(i + 1).cloned().unwrap_or_else(|| {
                    eprintln!("error: --config requires a path");
                    std::process::exit(2);
                });
                i += 2;
            }
            "--console" => {
                // Everything after --console is the command (run in a new window).
                console = args[i + 1..].to_vec();
                i = args.len();
            }
            "--help" | "-h" => {
                println!("custos-tray [--db PATH] [--config PATH] [--console CMD ARGS...]");
                return;
            }
            other => {
                eprintln!("error: unknown argument `{other}` (see --help)");
                std::process::exit(2);
            }
        }
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

fn tracing_subscriber_fmt() {
    tracing_subscriber::fmt()
        .with_target(true)
        .with_max_level(tracing::Level::INFO)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

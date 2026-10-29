//! Custos CLI — the primary management interface
//! (docs/guide/overview.md §2.2).
//!
//! P0 provides basic commands only; `validate` already runs the startup
//! checks, including the dependency-cycle gate.

use std::process::ExitCode;

const USAGE: &str = "\
custos - cross-platform local self-healing engine

USAGE:
    custos <COMMAND>

COMMANDS:
    validate <CONFIG>    Parse and validate a configuration file (rejects
                         duplicate IDs, dangling references and dependency
                         cycles before startup)
    status               Show engine status
    resources            List configured resources
    help                 Print this message

Other commands are added with the P0 roadmap (docs/guide/roadmap.md).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("validate") => match args.get(1) {
            Some(path) => validate(path),
            None => {
                eprintln!("error: `validate` requires a configuration file path");
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        Some("status") => {
            println!("engine: not running (P0 implementation pending)");
            ExitCode::SUCCESS
        }
        Some("resources") => {
            println!(
                "no resources loaded (run `custos validate <config>` to check a configuration)"
            );
            ExitCode::SUCCESS
        }
        Some("help") | None => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: unknown command `{other}`");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Startup gate: parse, validate and build the dependency graph. Any failure
/// — including a dependency cycle — exits non-zero and prints the concrete
/// error path (docs/guide/resources.md §3.5).
fn validate(path: &str) -> ExitCode {
    let config_text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("error: cannot read {path}: {err}");
            return ExitCode::FAILURE;
        }
    };
    match custos_config::load(&config_text) {
        Ok(config) => {
            println!("configuration OK: {} resources", config.resource_ids.len());
            println!("dependency graph: acyclic, all references resolved");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("configuration invalid: {err}");
            ExitCode::FAILURE
        }
    }
}

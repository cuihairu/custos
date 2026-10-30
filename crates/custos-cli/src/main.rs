//! Custos CLI — the primary management interface
//! (docs/guide/overview.md §2.2).
//!
//! P0 commands: `validate` (startup gate incl. dependency cycles), `start`
//! (run the engine loop), `status` (store counters + recent audit), and
//! `resources`/`graph` for configuration introspection.

use custos_config::ValidatedConfig;
use custos_engine::{BuiltinExecExecutor, Clock, Engine, SystemClock};
use custos_model::ResourceId;
use custos_probe::Probe;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "\
custos - cross-platform local self-healing engine

USAGE:
    custos <COMMAND> [ARGS]

COMMANDS:
    validate <CONFIG>          Parse and validate a configuration file
                               (rejects duplicate IDs, dangling references
                               and dependency cycles before startup)
    start <CONFIG> [OPTIONS]   Run the engine loop
        --db PATH              Database file (default: custos.db)
        --interval MS          Tick interval override (default: 5s)
        --once                 Run one observation tick and exit
    status [--db PATH]         Store counters and recent audit events
    resources <CONFIG>         List configured resources
    graph <CONFIG>             Dependency graph in topological order
    help                       Print this message";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("validate") => match args.get(1) {
            Some(path) => validate(path),
            None => missing_arg("validate", "CONFIG"),
        },
        Some("start") => match args.get(1) {
            Some(path) => start(path, &args[2..]),
            None => missing_arg("start", "CONFIG"),
        },
        Some("status") => status(&args[1..]),
        Some("resources") => match args.get(1) {
            Some(path) => resources(path),
            None => missing_arg("resources", "CONFIG"),
        },
        Some("graph") => match args.get(1) {
            Some(path) => graph(path),
            None => missing_arg("graph", "CONFIG"),
        },
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

fn missing_arg(command: &str, what: &str) -> ExitCode {
    eprintln!("error: `{command}` requires {what}");
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn load_config(path: &str) -> Result<ValidatedConfig, ExitCode> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("error: cannot read {path}: {err}");
            return Err(ExitCode::FAILURE);
        }
    };
    custos_config::load(&text).map_err(|err| {
        eprintln!("configuration invalid: {err}");
        ExitCode::FAILURE
    })
}

/// Startup gate: parse, validate and build the dependency graph. Any failure
/// — including a dependency cycle — exits non-zero and prints the concrete
/// error path (docs/guide/resources.md §3.5).
fn validate(path: &str) -> ExitCode {
    match load_config(path) {
        Ok(config) => {
            println!(
                "configuration OK: {} resources, {} alternative groups",
                config.resources.len(),
                config.alternative_groups.len()
            );
            println!("dependency graph: acyclic, all references resolved");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

struct StartOptions {
    db_path: String,
    interval_ms: u64,
    once: bool,
}

fn parse_start_options(args: &[String]) -> Result<StartOptions, ExitCode> {
    let mut options = StartOptions {
        db_path: "custos.db".to_owned(),
        interval_ms: 5_000,
        once: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                options.db_path = args
                    .get(i + 1)
                    .ok_or_else(|| {
                        eprintln!("error: --db requires a path");
                        ExitCode::from(2)
                    })?
                    .clone();
                i += 2;
            }
            "--interval" => {
                let raw = args.get(i + 1).ok_or_else(|| {
                    eprintln!("error: --interval requires a value in milliseconds");
                    ExitCode::from(2)
                })?;
                options.interval_ms = raw.parse().map_err(|_| {
                    eprintln!("error: --interval expects milliseconds, got `{raw}`");
                    ExitCode::from(2)
                })?;
                i += 2;
            }
            "--once" => {
                options.once = true;
                i += 1;
            }
            other => {
                eprintln!("error: unknown option `{other}` for `start`");
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(options)
}

/// Runs the engine: config → probes → engine, crash resume first, then the
/// observe/recover tick loop.
fn start(path: &str, args: &[String]) -> ExitCode {
    let options = match parse_start_options(args) {
        Ok(options) => options,
        Err(code) => return code,
    };
    let config = match load_config(path) {
        Ok(config) => config,
        Err(code) => return code,
    };

    // Builtin probe per resource (user ruling 2026-10-29); a resource type
    // without a builtin probe is a startup error, not a runtime surprise.
    let mut probes: HashMap<ResourceId, Box<dyn Probe>> = HashMap::new();
    for resource in &config.resources {
        match custos_probe::factory::build_probe(resource) {
            Ok(probe) => {
                probes.insert(resource.id.clone(), probe);
            }
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    let store = match custos_store::Store::open(&options.db_path) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            eprintln!("error: cannot open database {}: {err}", options.db_path);
            return ExitCode::FAILURE;
        }
    };
    let clock = Arc::new(SystemClock);
    let mut engine = Engine::new(
        &config,
        probes,
        Box::new(BuiltinExecExecutor),
        store.clone(),
        clock.clone(),
    );

    // Crash recovery first (§6.6): mid-execution attempts become
    // OutcomeUnknown; they are never blindly re-executed.
    let resumed = engine.resume_unfinished();
    if resumed > 0 {
        println!("crash recovery: {resumed} unfinished run(s) marked outcome-unknown");
    }

    println!(
        "custos: watching {} resource(s), tick interval {} ms, db {}",
        config.resources.len(),
        options.interval_ms,
        options.db_path
    );
    loop {
        let started = engine.tick();
        for run in &started {
            println!(
                "[{}] recovery {} for `{}`: {:?}",
                clock.now_unix_ms(),
                run.id.as_str(),
                run.resource_id.as_str(),
                run.status
            );
        }
        if options.once {
            break;
        }
        std::thread::sleep(Duration::from_millis(options.interval_ms));
    }
    ExitCode::SUCCESS
}

/// Store counters and the newest audit events.
fn status(args: &[String]) -> ExitCode {
    let mut path = "custos.db".to_owned();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                path = args.get(i + 1).cloned().unwrap_or_else(|| {
                    eprintln!("error: --db requires a path");
                    std::process::exit(2);
                });
                i += 2;
            }
            other => {
                eprintln!("error: unknown option `{other}` for `status`");
                return ExitCode::from(2);
            }
        }
    }
    let store = match custos_store::Store::open(&path) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("error: cannot open database {path}: {err}");
            return ExitCode::FAILURE;
        }
    };
    match (store.counts(), store.recent_audit(10)) {
        (Ok(counts), Ok(events)) => {
            println!(
                "runs: {}, attempts: {}, verifications: {}, audit events: {}",
                counts.runs, counts.attempts, counts.verifications, counts.audit_events
            );
            if events.is_empty() {
                println!("no audit events yet");
            }
            for event in events {
                let run = event.run_id.as_deref().unwrap_or("-");
                println!(
                    "#{:<4} {:>13} {} {} run={}",
                    event.id, event.severity, event.event_type, event.resource_id, run
                );
            }
            ExitCode::SUCCESS
        }
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("error: query failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn resources(path: &str) -> ExitCode {
    let config = match load_config(path) {
        Ok(config) => config,
        Err(code) => return code,
    };
    for resource in &config.resources {
        println!(
            "{:<24} type={:<18} enabled={:<3} plans={:<2} dependencies={}",
            resource.id.as_str(),
            resource.resource_type,
            resource.enabled,
            resource.recovery_policy.plans.len(),
            resource
                .dependencies
                .iter()
                .map(|d| d.target_resource_id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    ExitCode::SUCCESS
}

fn graph(path: &str) -> ExitCode {
    let config = match load_config(path) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let order = config.graph.topological_order();
    println!("topological order (dependencies first):");
    for (i, id) in order.iter().enumerate() {
        let deps: Vec<&str> = config
            .graph
            .dependencies_of(id)
            .iter()
            .map(|d| d.as_str())
            .collect();
        println!(
            "  {:>2}. {:<24} <- [{}]",
            i + 1,
            id.as_str(),
            deps.join(", ")
        );
    }
    ExitCode::SUCCESS
}

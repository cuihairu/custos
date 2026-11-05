//! `custos init <TYPE>` — configuration scaffolding
//! (docs/guide/dashboard-cli-config.md §9.2).
//!
//! Every template is a complete, valid `version = 1` document that passes
//! `custos validate`: full skeleton (resource, type-correct probe, health
//! policy, one recovery plan, verification) with placeholder values the
//! operator edits before use. Each template also carries the fault-mapping
//! rule the engine's `diagnose()` applies (crates/custos-engine/src/
//! engine.rs): a recovery plan whose `applicable_faults` does not contain
//! the fault actually diagnosed never runs — recovery exhausts with zero
//! attempts.

use std::process::ExitCode;

/// Resource types `custos init` scaffolds — the factory-wired set
/// (crates/custos-probe/src/factory.rs:38-148).
pub const TYPES: &[&str] = &[
    "process",
    "custom-command",
    "windows-service",
    "service",
    "http-endpoint",
    "network-connectivity",
    "disk",
    "memory",
    "cpu",
    "docker-container",
];

/// Parses `init` arguments and writes the template (stdout, or `--out FILE`
/// when given; an existing file is refused without `--force`).
pub fn run(args: &[String], usage: &str) -> ExitCode {
    let kind = match args.first() {
        Some(kind) if !kind.starts_with("--") => kind.as_str(),
        _ => return usage_error("init requires a resource TYPE", usage),
    };
    let mut id: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut force = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => match args.get(i + 1) {
                Some(value) => {
                    id = Some(value.as_str());
                    i += 2;
                }
                None => return usage_error("--id requires a value", usage),
            },
            "--out" => match args.get(i + 1) {
                Some(value) => {
                    out = Some(value.as_str());
                    i += 2;
                }
                None => return usage_error("--out requires a path", usage),
            },
            "--force" => {
                force = true;
                i += 1;
            }
            other => return usage_error(&format!("unknown option `{other}` for `init`"), usage),
        }
    }
    let id = match id {
        Some(id) => id,
        None => return usage_error("`init` requires --id <ID>", usage),
    };

    let text = match generate(kind, id) {
        Some(text) => text,
        None => {
            eprintln!("error: unknown resource type `{kind}`");
            eprintln!("valid types: {}", TYPES.join(", "));
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };

    match out {
        None => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Some(path) => {
            if std::path::Path::new(path).exists() && !force {
                eprintln!("error: {path} already exists (use --force to overwrite)");
                return ExitCode::FAILURE;
            }
            match std::fs::write(path, &text) {
                Ok(()) => {
                    println!("wrote {path} ({kind}, id {id}) — edit the REPLACE_ME values, then run `custos validate {path}`");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("error: cannot write {path}: {err}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn usage_error(message: &str, usage: &str) -> ExitCode {
    eprintln!("error: {message}");
    eprintln!("{usage}");
    ExitCode::from(2)
}

/// Generates the template for `kind`, substituting `id` as the resource id.
/// `None` when `kind` is not one of [`TYPES`].
pub fn generate(kind: &str, id: &str) -> Option<String> {
    match kind {
        "process" => Some(process(id)),
        "custom-command" => Some(custom_command(id)),
        "windows-service" => Some(windows_service(id)),
        "service" => Some(service(id)),
        "http-endpoint" => Some(http_endpoint(id)),
        "network-connectivity" => Some(network_connectivity(id)),
        "disk" => Some(disk(id)),
        "memory" => Some(memory(id)),
        "cpu" => Some(cpu(id)),
        "docker-container" => Some(docker_container(id)),
        _ => None,
    }
}

/// Escapes a value for a TOML basic string (`"…"`).
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Assembles the full document from the per-type header, probe table and
/// action. Everything else (resource table, health policy, plan metadata,
/// verification) is shared by every type.
fn template(doc: &str, kind: &str, id: &str, probe: &str, faults: &[&str], action: &str) -> String {
    let faults = faults
        .iter()
        .map(|fault| format!("\"{fault}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let id = quote(id);
    format!(
        "{doc}\
version = 1

[[resources]]
id = {id}
type = \"{kind}\"
display_name = {id}

{probe}

[resources.health_policy]
failure_threshold = 3
success_threshold = 2

[[resources.recovery_policy.plans]]
plan_id = \"restart\"
applicable_faults = [{faults}]
risk = \"low\"
idempotent = false
timeout_ms = 10000
max_attempts = 2
cooldown_ms = 30000

[resources.recovery_policy.plans.action]
{action}

[resources.recovery_policy.plans.verification]
# Recovery counts as done only once the probe above reports healthy again
# (docs/guide/resources.md §3.1).
status_is = \"healthy\"
"
    )
}

fn process(id: &str) -> String {
    template(
        "# Process supervision: watch a PID file, restart via the plan below
# (docs/guide/resources.md §3.1, docs/guide/probe-diagnosis.md §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): a failing
# `process` probe diagnoses `process_exited`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: pid_file, the restart command.
",
        "process",
        id,
        r#"[resources.probe]
pid_file = "/var/run/REPLACE_ME.pid"
interval_ms = 5000
timeout_ms = 2000"#,
        &["process_exited"],
        r#"# Detached spawn (see examples/basic-process.toml): setsid frees the
# process group, redirections release the output pipes, and $$ + exec keep
# the PID stable for the pid file above.
program = "sh"
args = ["-c", "setsid sh -c 'echo $$ > /var/run/REPLACE_ME.pid; exec <restart REPLACE_ME>' >/dev/null 2>&1 </dev/null &"]"#,
    )
}

fn custom_command(id: &str) -> String {
    template(
        "# Command-probed resource: the exec probe runs `command` and judges the
# exit code (docs/guide/resources.md §3.1, docs/guide/probe-diagnosis.md
# §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): a failing
# exec/custom-command probe diagnoses `remote_endpoint_unreachable`
# (Unhealthy verdict) or `probe_timeout` (Unreachable/timeout verdict). A
# recovery plan whose `applicable_faults` does not contain the fault actually
# diagnosed NEVER runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: command, the recovery command.
",
        "custom-command",
        id,
        r#"[resources.probe]
command = ["REPLACE_ME-check-command", "--arg"]
timeout_ms = 3000
max_output_bytes = 65536"#,
        &[
            "remote_endpoint_unreachable",
            "probe_timeout",
        ],
        r#"program = "sh"
args = ["-c", "echo 'REPLACE_ME: recovery command'"]"#,
    )
}

fn windows_service(id: &str) -> String {
    template(
        "# Windows SCM service. The factory-wired resource type is `service`: on
# Windows a `unit` routes to the native Windows-SCM probe (factory.rs:114-141,
# windows_service.rs); `windows-service` is that probe's name, not a separate
# resource type. docs/guide/resources.md §3.1 lists it among the example
# types for this reason.
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# service probes currently diagnose `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts
# (docs/guide/probe-diagnosis.md §5.4).
#
# Placeholders to edit: unit (the SCM service name), the recovery command.
",
        "service",
        id,
        r#"[resources.probe]
unit = "REPLACE_ME"
timeout_ms = 3000"#,
        &["unknown_failure"],
        r#"# SCM control on Windows: `sc start` or `Start-Service` (PowerShell).
program = "powershell"
args = ["-Command", "Start-Service -Name 'REPLACE_ME'"]"#,
    )
}

fn service(id: &str) -> String {
    template(
        "# Service resource. A `unit` uses the native service-manager probe
# (systemd on Linux, Windows SCM on Windows); a `command` instead falls back
# to the exec probe (factory.rs:114-141, docs/guide/resources.md §3.1,
# docs/guide/probe-diagnosis.md §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# service probes currently diagnose `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: unit, the recovery command.
",
        "service",
        id,
        r#"[resources.probe]
unit = "REPLACE_ME.service"
timeout_ms = 3000"#,
        &["unknown_failure"],
        r#"program = "systemctl"
args = ["restart", "REPLACE_ME.service"]"#,
    )
}

fn http_endpoint(id: &str) -> String {
    template(
        "# HTTP(S) endpoint health: one GET judged against the status whitelist,
# keyword assertions and the response-time threshold (http.rs,
# docs/guide/resources.md §3.1, docs/guide/probe-diagnosis.md §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# http probe currently diagnoses `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: url, the recovery command.
",
        "http-endpoint",
        id,
        r#"[resources.probe]
url = "https://REPLACE_ME/health"
timeout_ms = 3000
allowed_statuses = ["2xx"]
verify_tls = true"#,
        &["unknown_failure"],
        r#"program = "systemctl"
args = ["restart", "REPLACE_ME.service"]"#,
    )
}

fn network_connectivity(id: &str) -> String {
    template(
        "# TCP connectivity: connect to host:port, optionally over TLS
# (tcp.rs, docs/guide/resources.md §3.1, docs/guide/probe-diagnosis.md §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# tcp probe currently diagnoses `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: host, port, the recovery command.
",
        "network-connectivity",
        id,
        r#"[resources.probe]
host = "REPLACE_ME.example.com"
port = 443
timeout_ms = 3000
tls = false"#,
        &["unknown_failure"],
        r#"program = "sh"
args = ["-c", "echo 'REPLACE_ME: recovery command'"]"#,
    )
}

fn disk(id: &str) -> String {
    template(
        "# Disk usage: warn/crit percentage thresholds on one mount point
# (disk.rs, docs/guide/resources.md §3.1, docs/guide/probe-diagnosis.md §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# disk probe currently diagnoses `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: mount_point, the recovery command (cleanup must be
# safe to repeat — disk pressure is not fixed by one blind delete).
",
        "disk",
        id,
        r#"[resources.probe]
mount_point = "/"
warn_pct = 85
crit_pct = 95"#,
        &["unknown_failure"],
        r#"program = "sh"
args = ["-c", "echo 'REPLACE_ME: cleanup command'"]"#,
    )
}

fn memory(id: &str) -> String {
    template(
        "# Memory usage: warn/crit percentage thresholds, optionally counting
# swap (mem.rs, docs/guide/resources.md §3.1, docs/guide/probe-diagnosis.md
# §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# memory probe currently diagnoses `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: the recovery command.
",
        "memory",
        id,
        r#"[resources.probe]
warn_pct = 80
crit_pct = 90
check_swap = false"#,
        &["unknown_failure"],
        r#"program = "sh"
args = ["-c", "echo 'REPLACE_ME: recovery command'"]"#,
    )
}

fn cpu(id: &str) -> String {
    template(
        "# CPU load: warn/crit load-average thresholds (cpu.rs,
# docs/guide/resources.md §3.1, docs/guide/probe-diagnosis.md §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# cpu probe currently diagnoses `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: the recovery command.
",
        "cpu",
        id,
        r#"[resources.probe]
warn_load = 0.6
crit_load = 0.9"#,
        &["unknown_failure"],
        r#"program = "sh"
args = ["-c", "echo 'REPLACE_ME: recovery command'"]"#,
    )
}

fn docker_container(id: &str) -> String {
    template(
        "# Docker container state: running, and restart count under the
# crash-loop threshold (docker.rs, docs/guide/resources.md §3.1,
# docs/guide/probe-diagnosis.md §5.4).
#
# Fault mapping (crates/custos-engine/src/engine.rs `diagnose()`): the native
# docker probe currently diagnoses `unknown_failure`. A recovery plan whose
# `applicable_faults` does not contain the fault actually diagnosed NEVER
# runs — recovery exhausts with 0 attempts.
#
# Placeholders to edit: container, the recovery command.
",
        "docker-container",
        id,
        r#"[resources.probe]
container = "REPLACE_ME"
max_restarts = 5"#,
        &["unknown_failure"],
        r#"program = "docker"
args = ["restart", "REPLACE_ME"]"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate every template must pass: the generated TOML parses and
    /// validates through the same startup path `custos validate` uses
    /// (custos_config::load → ValidatedConfig).
    #[test]
    fn every_template_validates_through_the_startup_path() {
        for &kind in TYPES {
            let text = generate(kind, "demo").expect("template exists");
            custos_config::load(&text)
                .unwrap_or_else(|err| panic!("`{kind}` template invalid: {err}"));
        }
    }

    #[test]
    fn unknown_type_is_rejected() {
        assert!(generate("quantum-router", "x").is_none());
        assert!(TYPES.iter().any(|kind| generate(kind, "x").is_some()));
    }

    #[test]
    fn id_is_substituted_into_resource_and_display_name() {
        let text = generate("process", "my-worker").expect("process template");
        assert!(text.contains("id = \"my-worker\""), "got:\n{text}");
        assert!(text.contains("display_name = \"my-worker\""));
    }

    #[test]
    fn quotes_in_id_do_not_break_the_toml() {
        let text = generate("process", "weird\"id\\path").expect("template");
        custos_config::load(&text).expect("escaped id still parses");
    }

    #[test]
    fn full_skeleton_is_present() {
        let text = generate("service", "svc").expect("service template");
        for marker in [
            "version = 1",
            "[[resources]]",
            "[resources.probe]",
            "unit = ",
            "[resources.health_policy]",
            "failure_threshold = 3",
            "success_threshold = 2",
            "[[resources.recovery_policy.plans]]",
            "plan_id = \"restart\"",
            "applicable_faults = [",
            "risk = \"low\"",
            "idempotent = false",
            "timeout_ms = 10000",
            "max_attempts = 2",
            "cooldown_ms = 30000",
            "[resources.recovery_policy.plans.action]",
            "program = ",
            "[resources.recovery_policy.plans.verification]",
            "status_is = \"healthy\"",
        ] {
            assert!(text.contains(marker), "missing `{marker}` in:\n{text}");
        }
    }

    #[test]
    fn process_template_lists_process_exited() {
        let text = generate("process", "worker").expect("process template");
        assert!(
            text.contains("applicable_faults = [\"process_exited\""),
            "process plans must match the `process_exited` diagnosis, got:\n{text}"
        );
        assert!(
            !text.contains("probe_timeout"),
            "`process` never diagnoses `probe_timeout`, got:\n{text}"
        );
    }

    #[test]
    fn exec_template_lists_the_exec_fault_mapping() {
        let text = generate("custom-command", "check").expect("custom-command template");
        assert!(
            text.contains(
                "applicable_faults = [\"remote_endpoint_unreachable\", \"probe_timeout\"]"
            ),
            "exec-probed plans must match the exec fault mapping, got:\n{text}"
        );
        assert!(
            !text.contains("unknown_failure"),
            "`custom-command` never diagnoses `unknown_failure`, got:\n{text}"
        );
    }

    #[test]
    fn native_probe_templates_list_unknown_failure() {
        // Native probes (http/tcp/disk/memory/cpu/docker/systemd/SCM) are
        // diagnosed as `unknown_failure` today; a plan missing that fault
        // never runs.
        for kind in [
            "http-endpoint",
            "network-connectivity",
            "disk",
            "memory",
            "cpu",
            "docker-container",
            "service",
            "windows-service",
        ] {
            let text = generate(kind, "x").expect("template");
            assert!(
                text.contains("applicable_faults = [\"unknown_failure\"]"),
                "`{kind}` must list the fault its probe actually diagnoses, got:\n{text}"
            );
        }
    }

    #[test]
    fn every_template_states_the_applicable_faults_rule() {
        for &kind in TYPES {
            let text = generate(kind, "x").expect("template");
            assert!(
                text.contains("NEVER") && text.contains("0 attempts"),
                "`{kind}` must state the applicable_faults rule, got:\n{text}"
            );
        }
    }

    #[test]
    fn every_template_points_at_the_authoritative_docs() {
        for &kind in TYPES {
            let text = generate(kind, "x").expect("template");
            assert!(
                text.contains("docs/guide/resources.md"),
                "`{kind}` must reference docs/guide/resources.md"
            );
            assert!(
                text.contains("docs/guide/probe-diagnosis.md"),
                "`{kind}` must reference docs/guide/probe-diagnosis.md"
            );
        }
    }
}

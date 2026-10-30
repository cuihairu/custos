//! Builtin probe wiring (user ruling 2026-10-29: builtin base adapters,
//! keep the core small). P0 maps `process` and `custom-command` resources
//! (docs/guide/probe-diagnosis.md §5.4); more types arrive with P2/P3.

use crate::exec::ExecProbe;
use crate::http::HttpProbe;
use crate::process::ProcessProbe;
use crate::tcp::TcpProbe;
use crate::{http, Probe};
use custos_model::Resource;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
#[error("cannot build probe for resource `{id}` (type `{kind}`): {reason}")]
pub struct ProbeBuildError {
    pub id: String,
    pub kind: String,
    pub reason: String,
}

/// Builds the builtin probe for one resource from its `config` JSON
/// (written by `custos-config` from the `[resources.probe]` table).
pub fn build_probe(resource: &Resource) -> Result<Box<dyn Probe>, ProbeBuildError> {
    let err = |reason: &str| ProbeBuildError {
        id: resource.id.to_string(),
        kind: resource.resource_type.clone(),
        reason: reason.to_owned(),
    };
    let config = &resource.config;
    match resource.resource_type.as_str() {
        "process" => {
            let pid_file = config["pid_file"]
                .as_str()
                .ok_or_else(|| err("`probe.pid_file` is required for process resources"))?;
            Ok(Box::new(ProcessProbe {
                pid_file: pid_file.into(),
            }))
        }
        "http-endpoint" => http::from_config(config)
            .map(|probe: HttpProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "network-connectivity" => {
            let host = config["host"].as_str().ok_or_else(|| {
                err("`probe.host` is required for network-connectivity resources")
            })?;
            let port = config["port"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .ok_or_else(|| err("`probe.port` must be an integer in 0..=65535"))?;
            let timeout_ms = config["timeout_ms"].as_u64().unwrap_or(3_000);
            Ok(Box::new(TcpProbe {
                host: host.into(),
                port,
                timeout: Duration::from_millis(timeout_ms),
            }))
        }
        "custom-command"
        | "proxy"
        | "tunnel"
        | "service" => {
            // Any type may fall back to an exec check command in P0.
            let command = config["command"]
                .as_array()
                .map(|argv| {
                    argv.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect::<Vec<String>>()
                })
                .filter(|argv| !argv.is_empty())
                .ok_or_else(|| err("`probe.command` is required for exec-probed resources"))?;
            let timeout_ms = config["timeout_ms"].as_u64().unwrap_or(3_000);
            Ok(Box::new(ExecProbe::from_argv(
                command,
                Duration::from_millis(timeout_ms),
            )))
        }
        other => Err(err(&format!(
            "no builtin probe for type `{other}` in P0 (docs/guide/probe-diagnosis.md §5.4)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_process_probe_from_pid_file() {
        let mut resource = Resource::new("worker", "process");
        resource.config = json!({"pid_file": "/tmp/x.pid"});
        assert!(build_probe(&resource).is_ok());
    }

    #[test]
    fn builds_exec_probe_from_command() {
        let mut resource = Resource::new("check", "custom-command");
        resource.config = json!({"command": ["true"], "timeout_ms": 1000});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "exec");
    }

    #[test]
    fn unknown_type_is_rejected() {
        let resource = Resource::new("weird", "quantum-router");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_http_probe_from_url() {
        let mut resource = Resource::new("web", "http-endpoint");
        resource.config = json!({"url": "http://127.0.0.1:8080/health", "timeout_ms": 2000});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "http");
    }

    #[test]
    fn http_probe_requires_url() {
        let resource = Resource::new("web", "http-endpoint");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_tcp_probe_from_host_and_port() {
        let mut resource = Resource::new("db", "network-connectivity");
        resource.config = json!({"host": "127.0.0.1", "port": 5432});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "tcp");
    }

    #[test]
    fn tcp_probe_requires_host_and_port() {
        let resource = Resource::new("db", "network-connectivity");
        assert!(build_probe(&resource).is_err());

        let mut resource = Resource::new("db", "network-connectivity");
        resource.config = json!({"host": "127.0.0.1"});
        assert!(build_probe(&resource).is_err());
    }
}

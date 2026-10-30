//! Builtin probe wiring (user ruling 2026-10-29: builtin base adapters,
//! keep the core small). P0 maps `process` and `custom-command` resources
//! (docs/guide/probe-diagnosis.md §5.4); more types arrive with P2/P3.

use crate::cpu::CpuProbe;
use crate::database::DatabaseProbe;
use crate::disk::DiskProbe;
use crate::dns::DnsProbe;
use crate::exec::ExecProbe;
use crate::http::HttpProbe;
use crate::mem::MemProbe;
use crate::process::ProcessProbe;
use crate::tcp::TcpProbe;
use crate::tls_cert::TlsCertProbe;
use crate::{
    cpu, database, disk, dns, docker, http, icmp, log, mem, netif, proxy, tls_cert, Probe,
};
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
            let tls = config["tls"].as_bool().unwrap_or(false);
            let server_name = config["server_name"].as_str().unwrap_or(host);
            Ok(Box::new(TcpProbe {
                host: host.into(),
                port,
                timeout: Duration::from_millis(timeout_ms),
                tls,
                server_name: server_name.into(),
            }))
        }
        "dns" => dns::from_config(config)
            .map(|probe: DnsProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "tls-certificate" => tls_cert::from_config(config)
            .map(|probe: TlsCertProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "disk" => disk::from_config(config)
            .map(|probe: DiskProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "memory" => mem::from_config(config)
            .map(|probe: MemProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "cpu" => cpu::from_config(config)
            .map(|probe: CpuProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "log" => log::from_config(config)
            .map(|probe: crate::log::LogProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "docker-container" => docker::from_config(config)
            .map(|probe: crate::docker::DockerProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "icmp" => icmp::from_config(config)
            .map(|probe: crate::icmp::PingProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "network-interface" => netif::from_config(config)
            .map(|probe: crate::netif::NetifProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "default-gateway" => netif::gateway_from_config(config)
            .map(|probe: crate::netif::GatewayProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "proxy" => proxy::from_config(config)
            .map(|probe: crate::proxy::ProxyProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        "database" => database::from_config(config)
            .map(|probe: DatabaseProbe| Box::new(probe) as Box<dyn Probe>)
            .map_err(|reason| err(reason.as_str())),
        // `service` with a `unit` uses the native service-manager probe
        // (systemd on Linux, SCM on Windows — survey §26.11 #7/#11); with a
        // `command` it falls back to exec.
        "service" => {
            if config["unit"].is_string() {
                #[cfg(target_os = "linux")]
                {
                    crate::systemd::from_config(config)
                        .map(|probe: crate::systemd::SystemdProbe| {
                            Box::new(probe) as Box<dyn Probe>
                        })
                        .map_err(|reason| err(reason.as_str()))
                }
                #[cfg(windows)]
                {
                    crate::windows_service::from_config(config)
                        .map(|probe: crate::windows_service::WindowsServiceProbe| {
                            Box::new(probe) as Box<dyn Probe>
                        })
                        .map_err(|reason| err(reason.as_str()))
                }
                #[cfg(not(any(target_os = "linux", windows)))]
                {
                    Err(err(
                        "native service probing is unsupported on this platform",
                    ))
                }
            } else {
                exec_probe(config, resource)
            }
        }
        "custom-command" | "tunnel" => exec_probe(config, resource),
        other => Err(err(&format!(
            "no builtin probe for type `{other}` in P0 (docs/guide/probe-diagnosis.md §5.4)"
        ))),
    }
}

/// Builds the fallback exec probe for command-probed resource types.
fn exec_probe(
    config: &serde_json::Value,
    resource: &Resource,
) -> Result<Box<dyn Probe>, ProbeBuildError> {
    let err = |reason: &str| ProbeBuildError {
        id: resource.id.to_string(),
        kind: resource.resource_type.clone(),
        reason: reason.to_owned(),
    };
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

    #[test]
    fn builds_dns_probe_from_config() {
        let mut resource = Resource::new("dns-api", "dns");
        resource.config = json!({"name": "example.com", "server": "1.1.1.1"});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "dns");
    }

    #[test]
    fn dns_probe_requires_name() {
        let resource = Resource::new("dns-api", "dns");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_tls_cert_probe_from_host() {
        let mut resource = Resource::new("cert", "tls-certificate");
        resource.config = json!({"host": "example.com", "warn_days": 45, "crit_days": 14});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "tls-cert");
    }

    #[test]
    fn tls_cert_probe_requires_host() {
        let resource = Resource::new("cert", "tls-certificate");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn network_connectivity_reads_tls_option() {
        let mut resource = Resource::new("db", "network-connectivity");
        resource.config = json!({
            "host": "127.0.0.1", "port": 443, "tls": true, "server_name": "example.com"
        });
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "tcp");
    }

    #[test]
    fn builds_disk_probe() {
        let mut resource = Resource::new("root", "disk");
        resource.config = json!({"mount_point": "/", "warn_pct": 85, "crit_pct": 95});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "disk");
    }

    #[test]
    fn disk_probe_requires_mount_point() {
        let resource = Resource::new("root", "disk");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_memory_probe() {
        let mut resource = Resource::new("mem", "memory");
        resource.config = json!({"warn_pct": 75, "crit_pct": 90});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "memory");
    }

    #[test]
    fn builds_cpu_probe() {
        let mut resource = Resource::new("cpu", "cpu");
        resource.config = json!({"warn_load": 0.6, "crit_load": 0.9});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "cpu");
    }

    #[test]
    fn builds_log_probe() {
        let mut resource = Resource::new("svc-log", "log");
        resource.config = json!({"path": "/tmp/service.log", "pattern": "panic"});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "log");
    }

    #[test]
    fn log_probe_requires_path_and_pattern() {
        let resource = Resource::new("svc-log", "log");
        assert!(build_probe(&resource).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn service_with_unit_builds_systemd_probe() {
        let mut resource = Resource::new("nginx", "service");
        resource.config = json!({"unit": "nginx.service"});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "systemd");
    }

    #[test]
    fn service_with_command_falls_back_to_exec() {
        let mut resource = Resource::new("svc", "service");
        resource.config = json!({"command": ["systemctl", "is-active", "nginx"]});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "exec");
    }

    #[test]
    fn builds_docker_probe() {
        let mut resource = Resource::new("web", "docker-container");
        resource.config = json!({"container": "web", "max_restarts": 3});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "docker");
    }

    #[test]
    fn docker_probe_requires_container() {
        let resource = Resource::new("web", "docker-container");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_icmp_probe_from_host() {
        let mut resource = Resource::new("gw", "icmp");
        resource.config = json!({"host": "example.com", "timeout_ms": 2000, "fallback_port": 443});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "icmp");
    }

    #[test]
    fn icmp_probe_requires_host() {
        let resource = Resource::new("gw", "icmp");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_proxy_probe() {
        let mut resource = Resource::new("px", "proxy");
        resource.config = json!({"proxy": "127.0.0.1:7890"});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "proxy");
        let resource = Resource::new("px", "proxy");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_network_interface_probe() {
        let mut resource = Resource::new("eth", "network-interface");
        resource.config = json!({"interface": "eth0"});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "network-interface");
        let resource = Resource::new("eth", "network-interface");
        assert!(build_probe(&resource).is_err());
    }

    #[test]
    fn builds_default_gateway_probe() {
        let mut resource = Resource::new("gw", "default-gateway");
        resource.config = json!({"connect_port": 80});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "default-gateway");
    }

    #[test]
    fn builds_database_probe_from_config() {
        let mut resource = Resource::new("db", "database");
        resource.config = json!({"kind": "postgres", "host": "db.local", "port": 5432});
        let probe = build_probe(&resource).unwrap();
        assert_eq!(probe.name(), "database");
    }

    #[test]
    fn database_probe_requires_kind_and_host() {
        let resource = Resource::new("db", "database");
        assert!(build_probe(&resource).is_err());

        let mut resource = Resource::new("db", "database");
        resource.config = json!({"kind": "mysql"});
        assert!(build_probe(&resource).is_err());

        let mut resource = Resource::new("db", "database");
        resource.config = json!({"kind": "oracle", "host": "x"});
        assert!(build_probe(&resource).is_err());
    }
}

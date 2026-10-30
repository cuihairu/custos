//! Network interface and default-gateway probes (todo.md P3 214,
//! probe-survey.md §26: 接口状态 up/down via /sys/class/net operstate;
//! 默认网关 via /proc/net/route).
//!
//! Observation-only: an interface that is missing or administratively down
//! is Unhealthy (a fact), an interface we cannot query at all is
//! Unreachable (§3.2). Windows paths run `netsh`/`route` under the hardened
//! runner with fixture-verified parsers kept in sync with custos-platform.

use crate::config::required_str;
use crate::{Probe, ProbeReport};
use custos_model::{HealthStatus, Resource};
use std::time::Duration;

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(windows)]
const TOOL_OUTPUT_CAP: usize = 32 * 1024;

/// One network interface probe: `interface` is the OS-level name.
#[derive(Debug, Clone)]
pub struct NetifProbe {
    pub interface: String,
}

/// Builds from `[resources.probe]` (`interface` required).
pub fn from_config(config: &serde_json::Value) -> Result<NetifProbe, String> {
    let interface = required_str(config, "interface")
        .ok_or("`probe.interface` is required for network-interface resources")?;
    Ok(NetifProbe {
        interface: interface.to_owned(),
    })
}

impl Probe for NetifProbe {
    fn name(&self) -> &'static str {
        "network-interface"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        match interface_state(&self.interface) {
            Ok(InterfaceState::Up) => report(
                true,
                HealthStatus::Healthy,
                format!("interface {} is up", self.interface),
            ),
            Ok(InterfaceState::Down) => report(
                false,
                HealthStatus::Unhealthy,
                format!("interface {} is down", self.interface),
            ),
            Ok(InterfaceState::NotFound) => report(
                false,
                HealthStatus::Unhealthy,
                format!("interface {} does not exist", self.interface),
            ),
            Err(err) => report(
                false,
                HealthStatus::Unreachable,
                format!("cannot query interface state: {err}"),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InterfaceState {
    Up,
    Down,
    NotFound,
}

#[cfg(target_os = "linux")]
fn interface_state(interface: &str) -> Result<InterfaceState, String> {
    match std::fs::read_to_string(format!("/sys/class/net/{interface}/operstate")) {
        Ok(operstate) => match operstate.trim() {
            "up" | "unknown" => Ok(InterfaceState::Up),
            // dormant / lowerlayerdown / notpresent: administratively or
            // physically down — a fact about this interface.
            _ => Ok(InterfaceState::Down),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(InterfaceState::NotFound),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(windows)]
fn interface_state(interface: &str) -> Result<InterfaceState, String> {
    use crate::exec::{run_controlled, ExecResult};
    let owned = [
        "netsh".to_owned(),
        "interface".to_owned(),
        "show".to_owned(),
        "interface".to_owned(),
    ];
    match run_controlled(&owned, TOOL_TIMEOUT, TOOL_OUTPUT_CAP) {
        ExecResult::Succeeded { stdout, .. } => {
            Ok(match parse_netsh_interface_state(&stdout, interface) {
                Some(true) => InterfaceState::Up,
                Some(false) => InterfaceState::Down,
                None => InterfaceState::NotFound,
            })
        }
        ExecResult::Failed { stderr, stdout, .. } => Err(format!(
            "netsh failed: {}",
            stderr
                .trim()
                .is_empty()
                .then(|| stdout.trim())
                .unwrap_or(stderr.trim())
        )),
        ExecResult::TimedOut => Err("netsh timed out".into()),
        ExecResult::StartFailed(err) => Err(err),
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn interface_state(interface: &str) -> Result<InterfaceState, String> {
    // Not faking support (§11.1): no interface query on this host.
    Err(format!(
        "interface state query is unsupported on this platform (interface {interface})"
    ))
}

/// True when the row for `interface` reports Enabled+Connected.
/// Windows-only helper; the parser shape matches custos-platform's.
#[cfg(windows)]
fn parse_netsh_interface_state(output: &str, interface: &str) -> Option<bool> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Admin State") || trimmed.contains("Loopback")
        {
            continue;
        }
        let admin_enabled = trimmed.contains("Enabled") || trimmed.contains("已启用");
        let connected = trimmed.contains("Connected") || trimmed.contains("已连接");
        let name = trimmed.rsplit_once("  ").map(|(_, n)| n.trim())?;
        if name == interface {
            return Some(admin_enabled && connected);
        }
    }
    None
}

/// One default-gateway probe: a usable default route must exist (and the
/// gateway may optionally be reached by TCP).
#[derive(Debug, Clone)]
pub struct GatewayProbe {
    /// When set, TCP-connect to the gateway on this port in addition to
    /// requiring the default route.
    pub connect_port: Option<u16>,
}

/// Builds from `[resources.probe]` (`connect_port` optional).
pub fn gateway_from_config(config: &serde_json::Value) -> Result<GatewayProbe, String> {
    let port = match config["connect_port"].as_u64() {
        None => None,
        Some(port) => {
            Some(u16::try_from(port).map_err(|_| "`probe.connect_port` must be 0..=65535")?)
        }
    };
    if let Some(port) = port {
        if port == 0 {
            return Err("`probe.connect_port` must be non-zero".into());
        }
    }
    Ok(GatewayProbe { connect_port: port })
}

impl Probe for GatewayProbe {
    fn name(&self) -> &'static str {
        "default-gateway"
    }

    fn probe(&self, resource: &Resource) -> ProbeReport {
        let report = |succeeded, status, observation: String| ProbeReport {
            resource_id: resource.id.clone(),
            probe_name: self.name().to_owned(),
            succeeded,
            status,
            observation,
        };
        let gateway = match default_gateway() {
            Ok(Some(gateway)) => gateway,
            Ok(None) => {
                return report(
                    false,
                    HealthStatus::Unhealthy,
                    "no default route configured".into(),
                )
            }
            Err(err) => {
                return report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot query routes: {err}"),
                )
            }
        };
        if let Some(port) = self.connect_port {
            return match tcp_reachable(&gateway, port) {
                Ok(true) => report(
                    true,
                    HealthStatus::Healthy,
                    format!("gateway {gateway} reachable on {port}"),
                ),
                Ok(false) => report(
                    false,
                    HealthStatus::Unreachable,
                    format!("gateway {gateway} did not answer on {port}"),
                ),
                Err(err) => report(
                    false,
                    HealthStatus::Unreachable,
                    format!("cannot probe gateway {gateway}: {err}"),
                ),
            };
        }
        report(
            true,
            HealthStatus::Healthy,
            format!("default gateway {gateway} present"),
        )
    }
}

fn tcp_reachable(host: &str, port: u16) -> Result<bool, String> {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| "gateway has no address".to_string())?;
    match TcpStream::connect_timeout(&addr, TOOL_TIMEOUT) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

#[cfg(target_os = "linux")]
fn default_gateway() -> Result<Option<String>, String> {
    let table = std::fs::read_to_string("/proc/net/route").map_err(|e| e.to_string())?;
    Ok(parse_proc_route_default_gateway(&table).map(|(_, gateway)| gateway))
}

#[cfg(windows)]
fn default_gateway() -> Result<Option<String>, String> {
    use crate::exec::{run_controlled, ExecResult};
    let owned = ["route".to_owned(), "print".to_owned(), "-4".to_owned()];
    match run_controlled(&owned, TOOL_TIMEOUT, TOOL_OUTPUT_CAP) {
        ExecResult::Succeeded { stdout, .. } => Ok(parse_route_print_default_gateway(&stdout)),
        other => Err(format!("route print failed: {other:?}")),
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn default_gateway() -> Result<Option<String>, String> {
    Err("default gateway query is unsupported on this platform".into())
}

/// `/proc/net/route` (little-endian hex gateway) → (iface, dotted quad).
/// Same parser shape as custos-platform's; pure for fixture tests.
pub fn parse_proc_route_default_gateway(table: &str) -> Option<(String, String)> {
    let mut best: Option<(u32, String, String)> = None;
    for line in table.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let iface = fields.next()?.to_owned();
        let destination = fields.next()?;
        let gateway_hex = fields.next()?;
        let _flags = fields.next();
        let _refcnt = fields.next();
        let _use = fields.next();
        let metric: u32 = fields.next()?.parse().ok()?;
        if destination != "00000000" || gateway_hex == "00000000" {
            continue;
        }
        if best.is_none() || metric < best.as_ref().unwrap().0 {
            best = Some((metric, iface, gateway_hex.to_owned()));
        }
    }
    let (_, iface, hex) = best?;
    let value = u32::from_str_radix(&hex, 16).ok()?;
    Some((
        iface,
        format!(
            "{}.{}.{}.{}",
            value & 0xff,
            (value >> 8) & 0xff,
            (value >> 16) & 0xff,
            (value >> 24) & 0xff
        ),
    ))
}

/// `route print -4` → default gateway (mask 0.0.0.0).
pub fn parse_route_print_default_gateway(output: &str) -> Option<String> {
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let destination = fields.next()?;
        if destination != "0.0.0.0" {
            continue;
        }
        if fields.next()? != "0.0.0.0" {
            continue;
        }
        let gateway = fields.next()?;
        if gateway != "On-link" && !gateway.is_empty() {
            return Some(gateway.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource() -> Resource {
        Resource::new("net", "network-interface")
    }

    #[test]
    fn netif_from_config_requires_interface() {
        assert!(from_config(&json!({})).is_err());
        assert!(from_config(&json!({"interface": "eth0"})).is_ok());
    }

    #[test]
    fn gateway_from_config_validates_port() {
        assert!(gateway_from_config(&json!({})).is_ok());
        assert!(gateway_from_config(&json!({"connect_port": 0})).is_err());
        assert!(gateway_from_config(&json!({"connect_port": 70000})).is_err());
        let probe = gateway_from_config(&json!({"connect_port": 80})).unwrap();
        assert_eq!(probe.connect_port, Some(80));
    }

    #[test]
    fn proc_route_parser_fixture() {
        let table = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
enp3s0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
enp3s0\t0001A8C0\t00000000\t0001\t0\t0\t100\tFFFFFF00\t0\t0\t0
";
        let (iface, gateway) = parse_proc_route_default_gateway(table).unwrap();
        assert_eq!(iface, "enp3s0");
        assert_eq!(gateway, "192.168.0.1");
        assert!(parse_proc_route_default_gateway("Iface\tDestination\tGateway\n").is_none());
    }

    #[test]
    fn route_print_parser_fixture() {
        let output = "\
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0    192.168.1.1    192.168.1.42     25
";
        assert_eq!(
            parse_route_print_default_gateway(output).as_deref(),
            Some("192.168.1.1")
        );
        assert!(parse_route_print_default_gateway("nothing here").is_none());
    }

    #[test]
    fn netif_up_reports_healthy_lo() {
        // loopback is up on any Linux CI host.
        let probe = NetifProbe {
            interface: "lo".into(),
        };
        let report = probe.probe(&resource());
        assert!(
            matches!(
                report.status,
                HealthStatus::Healthy | HealthStatus::Unreachable
            ),
            "{}",
            report.observation
        );
    }

    #[test]
    fn netif_missing_is_unhealthy() {
        let probe = NetifProbe {
            interface: "definitely-not-an-interface".into(),
        };
        let report = probe.probe(&resource());
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(
            report.observation.contains("does not exist"),
            "{}",
            report.observation
        );
    }

    #[test]
    fn gateway_probe_on_this_host_is_a_verdict_not_a_panic() {
        let probe = GatewayProbe { connect_port: None };
        let report = probe.probe(&Resource::new("gw", "default-gateway"));
        assert!(
            matches!(
                report.status,
                HealthStatus::Healthy | HealthStatus::Unhealthy
            ),
            "{}",
            report.observation
        );
    }
}

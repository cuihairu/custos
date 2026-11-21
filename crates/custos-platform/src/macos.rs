//! macOS adapters (docs/guide/platforms.md §11.x, todo.md 241): launchd
//! service control, interface status via `ifconfig -a`, connectivity via
//! std, reboot via `shutdown -r now`. Anything the host cannot serve is
//! declared Unsupported with the concrete reason — never guess-executed
//! (§11.1). Wi-Fi association is deliberately not served: the tool needs
//! the network password/keychain entry this adapter does not supply.

use crate::capability::PlatformCapabilitySet;
use crate::controller::{
    Connectivity, ConnectivityTarget, InterfaceStatus, NetworkConnectivityProbe,
    NetworkInterfaceController, RebootController, ServiceController,
};
use crate::error::PlatformResult;
use custos_probe::exec::{run_controlled, ExecResult};
use std::time::Duration;

/// Controlled-execution profile shared by the macOS adapters.
const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
const TOOL_OUTPUT_CAP: usize = 64 * 1024;

fn run(argv: &[&str]) -> ExecResult {
    let owned: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
    run_controlled(&owned, TOOL_TIMEOUT, TOOL_OUTPUT_CAP)
}

fn expect_success(result: ExecResult) -> PlatformResult<()> {
    match result {
        ExecResult::Succeeded { .. } => Ok(()),
        other => Err(other.into()),
    }
}

/// True when an `ifconfig` block header carries the `UP` flag (admin up).
fn flags_have_up(header: &str) -> bool {
    // header like "flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST>"
    let Some(open) = header.find('<') else {
        return false;
    };
    let Some(close) = header[open..].find('>') else {
        return false;
    };
    header[open + 1..open + close].split(',').any(|f| f == "UP")
}

/// Parses `ifconfig -a` output into interface statuses. Pure so fixture
/// strings test it anywhere. An interface is `up` when its flags carry UP
/// AND (when a `status:` line exists) the link is `active` — a radio with
/// no carrier reports `status: inactive` and must not read as up. The
/// `%zone` suffix on IPv6 link-local addresses is stripped for clean
/// reporting.
pub fn parse_ifconfig(output: &str) -> Vec<InterfaceStatus> {
    let mut interfaces: Vec<InterfaceStatus> = Vec::new();
    let mut name: Option<String> = None;
    let mut flags_up = false;
    let mut status_active = true;
    let mut has_status = false;
    let mut addresses: Vec<String> = Vec::new();

    let flush = |interfaces: &mut Vec<InterfaceStatus>,
                 name: &mut Option<String>,
                 flags_up: &mut bool,
                 status_active: &mut bool,
                 has_status: &mut bool,
                 addresses: &mut Vec<String>| {
        if let Some(n) = name.take() {
            let up = *flags_up && (!*has_status || *status_active);
            interfaces.push(InterfaceStatus {
                name: n,
                up,
                addresses: std::mem::take(addresses),
            });
        }
        *flags_up = false;
        *status_active = true;
        *has_status = false;
    };

    for line in output.lines() {
        if line.starts_with(char::is_whitespace) {
            // Continuation line of the current block (indented).
            if name.is_none() {
                continue;
            }
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("status:") {
                has_status = true;
                status_active = rest.trim() == "active";
            } else if let Some(rest) = t.strip_prefix("inet ") {
                if let Some(addr) = rest.split_whitespace().next() {
                    addresses.push(addr.split('%').next().unwrap_or(addr).to_owned());
                }
            } else if let Some(rest) = t.strip_prefix("inet6 ") {
                if let Some(addr) = rest.split_whitespace().next() {
                    addresses.push(addr.split('%').next().unwrap_or(addr).to_owned());
                }
            }
        } else if let Some(colon) = line.find(':') {
            // New block header: "<device>: flags=..."
            flush(
                &mut interfaces,
                &mut name,
                &mut flags_up,
                &mut status_active,
                &mut has_status,
                &mut addresses,
            );
            name = Some(line[..colon].trim().to_owned());
            flags_up = flags_have_up(&line[colon + 1..]);
        }
    }
    flush(
        &mut interfaces,
        &mut name,
        &mut flags_up,
        &mut status_active,
        &mut has_status,
        &mut addresses,
    );
    interfaces
}

/// The default macOS service controller: launchd is macOS's init system,
/// always present. The controller is the launchd adapter from
/// [`crate::init_systems`] (its live tests belong to macOS hosts).
pub fn service_controller() -> Box<dyn ServiceController> {
    Box::new(crate::init_systems::launchd::LaunchdController {
        domain: "system".into(),
    })
}

/// macOS network status via `ifconfig -a` (read-only listing) and the
/// `ifconfig <name> down|up` reset path. Wi-Fi association is not served
/// (no password/keychain path; §11.1 绝不误执行).
pub struct MacosNetworkController;

impl NetworkInterfaceController for MacosNetworkController {
    fn list(&self) -> PlatformResult<Vec<InterfaceStatus>> {
        match run(&["ifconfig", "-a"]) {
            ExecResult::Succeeded { stdout, .. } => Ok(parse_ifconfig(&stdout)),
            other => Err(other.into()),
        }
    }

    fn reset(&self, name: &str) -> PlatformResult<()> {
        // High risk: the capability metadata marks interrupts_*; the
        // permission gate lives in the dispatch layer (ADR-0012).
        expect_success(run(&["ifconfig", name, "down"]))?;
        expect_success(run(&["ifconfig", name, "up"]))
    }
}

impl NetworkConnectivityProbe for MacosNetworkController {
    fn probe_tcp(&self, target: &ConnectivityTarget) -> PlatformResult<Connectivity> {
        use std::net::TcpStream;
        let start = std::time::Instant::now();
        // Bare IPv6 literals resolve via the tuple form (R16); the old
        // `format!("{}:{}", ...).parse()` string form rejected them.
        let addr = crate::controller::resolve_target(&target.host, target.port)?;
        match TcpStream::connect_timeout(&addr, TOOL_TIMEOUT) {
            Ok(_) => Ok(Connectivity::Reachable {
                latency_ms: start.elapsed().as_millis() as u64,
            }),
            Err(err) => match err.kind() {
                std::io::ErrorKind::ConnectionRefused => Ok(Connectivity::Refused),
                _ => Ok(Connectivity::Unreachable(err.to_string())),
            },
        }
    }
}

/// DNS resolution check (todo.md 202): the platform resolver, normalized.
pub fn dns_resolves(host: &str, port: u16) -> PlatformResult<bool> {
    use std::net::ToSocketAddrs;
    Ok((host, port)
        .to_socket_addrs()
        .map(|mut addrs| addrs.next().is_some())
        .unwrap_or(false))
}

/// Authorized reboot: `shutdown -r now`. Gated at dispatch (ADR-0012).
pub struct MacosRebootController;

impl RebootController for MacosRebootController {
    fn reboot(&self) -> PlatformResult<()> {
        expect_success(run(&["shutdown", "-r", "now"]))
    }
}

/// The macOS adapter's declared support (todo.md 193: 不伪造平台支持).
/// launchd is always present on macOS; network/reboot use the tools above.
/// Wi-Fi association is not served (no password path), so it stays out of
/// the set rather than declared native without a working path.
pub fn capabilities() -> PlatformCapabilitySet {
    PlatformCapabilitySet::new()
        .native("service.status")
        .native("service.start")
        .native("service.stop")
        .native("service.restart")
        .native("network.interface.list")
        .native("network.interface.reset")
        .native("network.connectivity.tcp")
        .native("system.reboot")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PlatformError;

    #[test]
    fn parses_up_interface_with_addresses() {
        let output = "\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
	ether b8:09:da:8a:55:b2
	inet6 fe80::ba09:daff:fe8a:55b2%en0 prefixlen 64 secured scopeid 0x8
	inet 192.168.1.5 netmask 0xffffff00 broadcast 192.168.1.255
	status: active
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 1);
        let iface = &list[0];
        assert_eq!(iface.name, "en0");
        assert!(iface.up, "{iface:?}");
        assert!(iface.addresses.contains(&"192.168.1.5".to_string()));
        assert!(iface
            .addresses
            .contains(&"fe80::ba09:daff:fe8a:55b2".to_string()));
    }

    #[test]
    fn inactive_link_is_down_despite_up_flag() {
        // Radio off / no carrier: the UP flag is set but the link is not
        // active — the interface must not read as up.
        let output = "\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
	status: inactive
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 1);
        assert!(!list[0].up, "{list:?}");
    }

    #[test]
    fn down_interface_has_no_up_flag() {
        let output = "\
en1: flags=8822<BROADCAST,SMART,SIMPLEX,MULTICAST> mtu 1500
	status: inactive
";
        let list = parse_ifconfig(output);
        assert_eq!(list[0].name, "en1");
        assert!(!list[0].up, "{list:?}");
    }

    #[test]
    fn loopback_is_listed_and_up() {
        let output = "\
lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384
	inet 127.0.0.1 netmask 0xff000000
	status: active
";
        let list = parse_ifconfig(output);
        assert!(list.iter().any(|i| i.name == "lo0" && i.up), "{list:?}");
    }

    #[test]
    fn multiple_blocks_and_empty_output() {
        let output = "\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
	status: active
utun0: flags=8081<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380
";
        let list = parse_ifconfig(output);
        assert_eq!(list.len(), 2);
        assert!(list[0].up);
        // utun0 has no status line: the UP flag alone stands.
        assert!(list[1].up);
        assert!(parse_ifconfig("").is_empty());
    }

    #[test]
    fn capabilities_declare_launchd_and_network_natively() {
        let set = capabilities();
        assert!(set.is_native("service.status"));
        assert!(set.is_native("service.restart"));
        assert!(set.is_native("network.interface.list"));
        assert!(set.is_native("network.interface.reset"));
        assert!(set.is_native("network.connectivity.tcp"));
        assert!(set.is_native("system.reboot"));
        // Wi-Fi association has no path on macOS: undeclared, not guessed.
        assert!(!set.is_native("wifi.reconnect"));
        assert!(matches!(
            set.require("wifi.reconnect"),
            Err(PlatformError::Unsupported(_))
        ));
    }
}

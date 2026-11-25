//! FreeBSD adapters (docs/guide/platforms.md §11.x, todo.md 242): rc.d
//! service control via `/usr/sbin/service <unit> onestatus|onestart|onestop|
//! onerestart`, interface status via `ifconfig -a`, connectivity via std,
//! reboot via `shutdown -r now`. Anything the host cannot serve is declared
//! Unsupported with the concrete reason — never guess-executed (§11.1).
//! Wi-Fi association is deliberately not served: the tool needs the network
//! password this adapter does not supply.

use crate::capability::PlatformCapabilitySet;
use crate::controller::{
    Connectivity, ConnectivityTarget, InterfaceStatus, NetworkConnectivityProbe,
    NetworkInterfaceController, RebootController, ServiceController,
};
use crate::error::PlatformResult;
use crate::ifconfig::parse_ifconfig;
use custos_probe::exec::{run_controlled, ExecResult};
use std::time::Duration;

/// Controlled-execution profile shared by the FreeBSD adapters.
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

/// The default FreeBSD service controller: rc.d via the `service` command,
/// present on every FreeBSD host. The controller is the rc.d adapter from
/// [`crate::init_systems`] (its live tests belong to FreeBSD hosts).
pub fn service_controller() -> Box<dyn ServiceController> {
    Box::new(crate::init_systems::rcd::RcdController)
}

/// FreeBSD network status via `ifconfig -a` (read-only listing) and the
/// `ifconfig <name> down|up` reset path. Wi-Fi association is not served
/// (no password path; §11.1: never guess-execute).
pub struct BsdNetworkController;

impl NetworkInterfaceController for BsdNetworkController {
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

impl NetworkConnectivityProbe for BsdNetworkController {
    fn probe_tcp(&self, target: &ConnectivityTarget) -> PlatformResult<Connectivity> {
        crate::controller::tcp_connectivity(target, TOOL_TIMEOUT)
    }
}

/// DNS resolution check (todo.md 202): the platform resolver, normalized.
pub use crate::controller::dns_resolves;

/// Authorized reboot: `shutdown -r now`. Gated at dispatch (ADR-0012).
pub struct BsdRebootController;

impl RebootController for BsdRebootController {
    fn reboot(&self) -> PlatformResult<()> {
        expect_success(run(&["shutdown", "-r", "now"]))
    }
}

/// The FreeBSD adapter's declared support (todo.md 193: never fake
/// platform support).
/// rc.d is always present on FreeBSD; network/reboot use the tools above.
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
    fn capabilities_declare_rcd_and_network_natively() {
        let set = capabilities();
        assert!(set.is_native("service.status"));
        assert!(set.is_native("service.restart"));
        assert!(set.is_native("network.interface.list"));
        assert!(set.is_native("network.interface.reset"));
        assert!(set.is_native("network.connectivity.tcp"));
        assert!(set.is_native("system.reboot"));
        // Wi-Fi association has no path on FreeBSD: undeclared, not guessed.
        assert!(!set.is_native("wifi.reconnect"));
        assert!(matches!(
            set.require("wifi.reconnect"),
            Err(PlatformError::Unsupported(_))
        ));
    }
}

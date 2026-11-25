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
use crate::ifconfig::parse_ifconfig;
use crate::power_parsers::{parse_date_r, pmset_disablesleep_argv, pmset_schedule_wake_argv};
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
/// (no password/keychain path; §11.1: never guess-execute).
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
        crate::controller::tcp_connectivity(target, TOOL_TIMEOUT)
    }
}

/// DNS resolution check (todo.md 202): the platform resolver, normalized.
pub use crate::controller::dns_resolves;

/// Authorized reboot: `shutdown -r now`. Gated at dispatch (ADR-0012).
pub struct MacosRebootController;

impl RebootController for MacosRebootController {
    fn reboot(&self) -> PlatformResult<()> {
        expect_success(run(&["shutdown", "-r", "now"]))
    }
}

/// The macOS adapter's declared support (todo.md 193: never fake platform
/// support).
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
        .native("power.sleep.prevent")
        .native("power.sleep.allow")
        .native("power.wake.arm")
        .unsupported(
            "power.wake.disarm",
            "pmset schedule cancel needs the exact original schedule string",
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PlatformError;

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

/// The macOS power policy adapter (todo.md power probe family): sleep via
/// `pmset -a disablesleep`, scheduled wake via `pmset schedule` after the
/// epoch is rendered to pmset's local datetime through `date -r`. HIGH
/// RISK: changes host power behavior; permission gate plus a second
/// confirm token live in toolbox dispatch (ADR-0012). Disarming has no
/// symmetrical one-call surface (cancel needs the exact original schedule
/// string), so it is declared unsupported instead of guessed (§11.1).
pub struct MacosPowerPolicyController;

impl crate::controller::PowerPolicyController for MacosPowerPolicyController {
    fn prevent_sleep(&self) -> PlatformResult<()> {
        let argv = pmset_disablesleep_argv(true);
        expect_success(run(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
    }

    fn allow_sleep(&self, _minutes: u64) -> PlatformResult<()> {
        let argv = pmset_disablesleep_argv(false);
        expect_success(run(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
    }

    fn arm_wake(&self, epoch_secs: u64) -> PlatformResult<()> {
        // Two controlled commands: render the epoch in pmset's expected
        // local format, then schedule. No shell, no string interpolation
        // into a shell.
        let when = match run(&[
            "date",
            "-r",
            epoch_secs.to_string().as_str(),
            "+%m/%d/%y %H:%M:%S",
        ]) {
            ExecResult::Succeeded { stdout, .. } => parse_date_r(&stdout).ok_or_else(|| {
                PlatformError::Failed(format!("unrecognized `date -r` output: {}", stdout.trim()))
            })?,
            other => return Err(other.into()),
        };
        let argv = pmset_schedule_wake_argv("wake", &when);
        expect_success(run(&argv.iter().map(String::as_str).collect::<Vec<_>>()))
    }

    fn disarm_wake(&self) -> PlatformResult<()> {
        Err(PlatformError::Unsupported(
            "pmset schedule cancel needs the exact original schedule string, which this \
             adapter does not track; not guessing (§11.1)"
                .into(),
        ))
    }
}

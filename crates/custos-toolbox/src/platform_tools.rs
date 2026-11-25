//! High-risk platform tools (todo.md P2 permission checks, ADR-0012): reboot
//! and interface reset wrapped as toolbox capabilities whose
//! `required_permission` the dispatch layer enforces — an unauthorized call
//! is refused before the platform adapter ever runs. The adapters
//! themselves never self-authorize. The two network-affecting operations
//! also share a global mutex (todo.md P3 223): at most one network
//! operation runs at a time, across resources.

/// The global network-operations mutex (todo.md P3 223, network-operation
/// mutual exclusion).
/// AT MOST ONE network operation (reset, reboot, any future
/// network-affecting action) may run at a time, regardless of which
/// resource requested it — concurrent interface changes would race with
/// each other. A contender waits (bounded, C9) for the in-flight one
/// instead of being reported as a failed operation; only a contender
/// that outlives [`NETWORK_OPS_WAIT_MS`] gets a concrete error.
static NETWORK_OPS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// How long a contending network operation waits for the in-flight one
/// before refusing (C9). Interface bounces and Wi-Fi reconnects are slow
/// platform I/O; contention must not surface as a failed attempt that
/// burns the engine's ladder. The bound keeps a wedged in-flight
/// operation from hanging the contender forever (the wedged side is the
/// executor watchdog's problem, C20).
const NETWORK_OPS_WAIT_MS: u64 = 30_000;

/// Locks the global network-operations mutex, waiting up to `wait_ms`
/// for the in-flight operation to finish (C9). `std::sync::Mutex` has no
/// timed lock, so this polls short try-lock sleeps; the error case means
/// the wait ran out, not that the operation failed.
fn lock_network_ops(wait_ms: u64) -> Result<std::sync::MutexGuard<'static, ()>, ()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    loop {
        match NETWORK_OPS.try_lock() {
            Ok(guard) => return Ok(guard),
            // A panic inside a platform adapter poisons the guard. The
            // panicking operation is dead and the guard carries no state,
            // so recovering the unit lock is safe.
            Err(std::sync::TryLockError::Poisoned(err)) => return Ok(err.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
}

use crate::registry::{BuiltinTool, ToolOutcome, ToolboxError, ToolboxErrorKind};
use custos_plugin_api::{AffectedScope, Capability, CapabilityKind, OperationParams};

/// `system.reboot`: reboots the host through the platform adapter.
/// Host-wide scope, interrupts everything — the highest-risk operation.
pub struct RebootTool {
    controller: std::sync::Arc<dyn custos_platform::RebootController>,
}

impl RebootTool {
    pub fn new(controller: std::sync::Arc<dyn custos_platform::RebootController>) -> Self {
        Self { controller }
    }
}

impl BuiltinTool for RebootTool {
    fn capability(&self) -> Capability {
        Capability {
            kind: CapabilityKind::RecoveryActionProvider,
            operation_id: "system.reboot".into(),
            summary: "reboot the host (authorized only)".into(),
            read_only: false,
            required_permission: "host.reboot".into(),
            affected_scope: AffectedScope::Host,
            interrupts_network: true,
            interrupts_remote_admin: true,
            cancelable: false,
            idempotent: false,
            verifiable: false,
            possible_failures: vec!["permission_denied".into(), "unsupported".into()],
            compensation_operation: None,
        }
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        // Guard: tests and accidental dispatch pass confirm=true explicitly.
        if params.args.get("confirm").and_then(|v| v.as_str()) != Some("reboot") {
            return Err(ToolboxError::new(
                ToolboxErrorKind::InvalidParams,
                "reboot requires confirm=\"reboot\" (a second, explicit gate)",
            ));
        }
        let _guard = lock_network_ops(NETWORK_OPS_WAIT_MS).map_err(|()| {
            ToolboxError::new(
                ToolboxErrorKind::Failed,
                format!(
                    "another network operation is still running after \
                     {NETWORK_OPS_WAIT_MS} ms; refusing to race it"
                ),
            )
        })?;
        self.controller
            .reboot()
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: "reboot command issued".into(),
            })
            .map_err(|e| ToolboxError::new(ToolboxErrorKind::Failed, e.to_string()))
    }
}

/// `network.interface.reset`: bounces one interface (may cut the management
/// channel — declared in the capability metadata, gated by permission).
pub struct InterfaceResetTool {
    controller: std::sync::Arc<dyn custos_platform::NetworkInterfaceController>,
}

impl InterfaceResetTool {
    pub fn new(
        controller: std::sync::Arc<dyn custos_platform::NetworkInterfaceController>,
    ) -> Self {
        Self { controller }
    }
}

impl BuiltinTool for InterfaceResetTool {
    fn capability(&self) -> Capability {
        Capability {
            kind: CapabilityKind::RecoveryActionProvider,
            operation_id: "network.interface.reset".into(),
            summary: "bounce one network interface (may cut remote admin)".into(),
            read_only: false,
            required_permission: "host.network".into(),
            affected_scope: AffectedScope::NamedResource,
            interrupts_network: true,
            interrupts_remote_admin: true,
            cancelable: false,
            idempotent: true,
            verifiable: true,
            possible_failures: vec![
                "permission_denied".into(),
                "interface_not_found".into(),
                "management_channel_lost".into(),
            ],
            compensation_operation: None,
        }
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        let name = params
            .args
            .get("interface")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`interface` must be a non-empty interface name",
                )
            })?;
        let _guard = lock_network_ops(NETWORK_OPS_WAIT_MS).map_err(|()| {
            ToolboxError::new(
                ToolboxErrorKind::Failed,
                format!(
                    "another network operation is still running after \
                     {NETWORK_OPS_WAIT_MS} ms; refusing to race it"
                ),
            )
        })?;
        self.controller
            .reset(name)
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: format!("interface {name} bounced"),
            })
            .map_err(|e| ToolboxError::new(ToolboxErrorKind::Failed, e.to_string()))
    }
}

/// `wifi.reconnect`: connects the Wi-Fi radio to a named profile through
/// the platform adapter. The adapter itself probes for a wireless adapter
/// before executing (never guesses), so the static capability set keeps
/// `wifi.reconnect` as unsupported and this tool supplies the runtime
/// execution path behind the permission gate.
pub struct WifiReconnectTool {
    controller: std::sync::Arc<dyn custos_platform::NetworkInterfaceController>,
}

impl WifiReconnectTool {
    pub fn new(
        controller: std::sync::Arc<dyn custos_platform::NetworkInterfaceController>,
    ) -> Self {
        Self { controller }
    }
}

impl BuiltinTool for WifiReconnectTool {
    fn capability(&self) -> Capability {
        Capability {
            kind: CapabilityKind::RecoveryActionProvider,
            operation_id: "wifi.reconnect".into(),
            summary: "connect the Wi-Fi radio to a named profile".into(),
            read_only: false,
            required_permission: "host.network".into(),
            affected_scope: AffectedScope::NamedResource,
            interrupts_network: true,
            interrupts_remote_admin: true,
            cancelable: false,
            idempotent: true,
            verifiable: true,
            possible_failures: vec![
                "permission_denied".into(),
                "unsupported".into(),
                "profile_not_found".into(),
            ],
            compensation_operation: None,
        }
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        let profile = params
            .args
            .get("profile")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`profile` must be a non-empty Wi-Fi profile name",
                )
            })?;
        let _guard = lock_network_ops(NETWORK_OPS_WAIT_MS).map_err(|()| {
            ToolboxError::new(
                ToolboxErrorKind::Failed,
                format!(
                    "another network operation is still running after \
                     {NETWORK_OPS_WAIT_MS} ms; refusing to race it"
                ),
            )
        })?;
        self.controller
            .connect_wifi(profile)
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: format!("wifi profile {profile} connect issued"),
            })
            .map_err(|e| ToolboxError::new(ToolboxErrorKind::Failed, e.to_string()))
    }
}

/// High-risk power policy tools (todo.md power probe family, ADR-0012):
/// sleep prevention/restoration and RTC wake arming/clearing. Each call
/// needs BOTH the dispatch permission gate (`host.power`) AND a second
/// explicit confirm token in `params.args.confirm`, mirroring the reboot
/// tool's two-gate discipline. The adapters never self-authorize.
/// Validates the second confirm gate for power operations. The token is
/// operation-specific: a stale `confirm=sleep` cannot arm a wake.
fn require_power_confirm(params: &OperationParams, expected: &str) -> Result<(), ToolboxError> {
    if params.args.get("confirm").and_then(|v| v.as_str()) != Some(expected) {
        return Err(ToolboxError::new(
            ToolboxErrorKind::InvalidParams,
            format!("power operation requires confirm=\"{expected}\" (explicit second gate)"),
        ));
    }
    Ok(())
}

fn power_capability(operation_id: &str, summary: &str) -> Capability {
    Capability {
        kind: CapabilityKind::RecoveryActionProvider,
        operation_id: operation_id.into(),
        summary: summary.into(),
        read_only: false,
        required_permission: "host.power".into(),
        affected_scope: AffectedScope::Host,
        // Power policy affects the whole host, including any remote
        // management channel that goes to sleep with it.
        interrupts_network: true,
        interrupts_remote_admin: true,
        cancelable: false,
        idempotent: true,
        verifiable: true,
        possible_failures: vec![
            "permission_denied".into(),
            "unsupported".into(),
            "confirm_required".into(),
        ],
        compensation_operation: None,
    }
}

/// Blocks automatic suspend through the host adapter.
pub struct PreventSleepTool {
    controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>,
}

impl PreventSleepTool {
    pub fn new(controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>) -> Self {
        Self { controller }
    }
}

impl BuiltinTool for PreventSleepTool {
    fn capability(&self) -> Capability {
        power_capability("power.sleep.prevent", "block automatic host suspend")
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        require_power_confirm(params, "sleep")?;
        self.controller
            .prevent_sleep()
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: "sleep prevention applied".into(),
            })
            .map_err(|e| ToolboxError::new(ToolboxErrorKind::Failed, e.to_string()))
    }
}

/// Restores the ability to suspend; `minutes` comes from the caller as an
/// explicit restore value (no hidden state).
pub struct AllowSleepTool {
    controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>,
}

impl AllowSleepTool {
    pub fn new(controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>) -> Self {
        Self { controller }
    }
}

impl BuiltinTool for AllowSleepTool {
    fn capability(&self) -> Capability {
        power_capability("power.sleep.allow", "restore automatic host suspend")
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        require_power_confirm(params, "sleep")?;
        let minutes = params
            .args
            .get("minutes")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`minutes` must be a non-negative idle timeout (0 disables sleep)",
                )
            })?;
        self.controller
            .allow_sleep(minutes)
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: format!("sleep restored with standby timeout {minutes} minutes"),
            })
            .map_err(|e| ToolboxError::new(ToolboxErrorKind::Failed, e.to_string()))
    }
}

/// Arms the RTC wake alarm at an explicit epoch (unix seconds).
pub struct ArmWakeTool {
    controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>,
}

impl ArmWakeTool {
    pub fn new(controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>) -> Self {
        Self { controller }
    }
}

impl BuiltinTool for ArmWakeTool {
    fn capability(&self) -> Capability {
        power_capability("power.wake.arm", "arm a hardware wake alarm")
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        require_power_confirm(params, "wake")?;
        let epoch = params
            .args
            .get("epoch_secs")
            .and_then(|v| v.as_u64())
            .filter(|v| *v > 0)
            .ok_or_else(|| {
                ToolboxError::new(
                    ToolboxErrorKind::InvalidParams,
                    "`epoch_secs` must be a positive unix timestamp",
                )
            })?;
        self.controller
            .arm_wake(epoch)
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: format!("wake alarm armed for {epoch}"),
            })
            .map_err(|e| ToolboxError::new(ToolboxErrorKind::Failed, e.to_string()))
    }
}

/// Clears the armed wake alarm.
pub struct DisarmWakeTool {
    controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>,
}

impl DisarmWakeTool {
    pub fn new(controller: std::sync::Arc<dyn custos_platform::PowerPolicyController>) -> Self {
        Self { controller }
    }
}

impl BuiltinTool for DisarmWakeTool {
    fn capability(&self) -> Capability {
        power_capability("power.wake.disarm", "clear the armed wake alarm")
    }

    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
        require_power_confirm(params, "wake")?;
        self.controller
            .disarm_wake()
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: "wake alarm cleared".into(),
            })
            .map_err(|e| ToolboxError::new(ToolboxErrorKind::Failed, e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ToolRegistry;
    use custos_platform::{PlatformError, PlatformResult};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `NETWORK_OPS` is process-global: the two tests below hold it, so they
    /// must be serialized or they observe each other's in-flight guard and
    /// panic on the free-mutex precondition (previously flaky under full
    /// parallel runs).
    static NETWORK_OPS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct RefusingReboot;

    impl custos_platform::RebootController for RefusingReboot {
        fn reboot(&self) -> PlatformResult<()> {
            Err(PlatformError::PermissionDenied(
                "adapter double-check refused (test)".into(),
            ))
        }
    }

    #[test]
    fn reboot_requires_explicit_confirmation_even_when_authorized() {
        let tool = RebootTool::new(std::sync::Arc::new(RefusingReboot));
        let params = OperationParams {
            operation_id: "system.reboot".into(),
            resource_id: "host".into(),
            run_id: None,
            args: Default::default(),
        };
        let err = tool.execute(&params).unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::InvalidParams, "{err}");
        assert!(err.message.contains("confirm"), "{err}");
    }

    #[test]
    fn unauthorized_reboot_is_refused_before_the_adapter_runs() {
        // The adapter counts invocations: the permission gate must fire first.
        struct Counting(AtomicUsize);
        impl custos_platform::RebootController for Counting {
            fn reboot(&self) -> PlatformResult<()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let adapter: std::sync::Arc<Counting> = std::sync::Arc::new(Counting(AtomicUsize::new(0)));
        let mut registry = ToolRegistry::new();
        registry
            .register_builtin(Box::new(RebootTool::new(adapter.clone())))
            .unwrap();

        let params = OperationParams {
            operation_id: "system.reboot".into(),
            resource_id: "host".into(),
            run_id: None,
            args: [("confirm".into(), serde_json::json!("reboot"))].into(),
        };
        // Permission NOT granted.
        let err = registry.dispatch(&|_| false, &params).unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::Denied, "{err}");
        assert!(err.message.contains("host.reboot"), "{err}");
        assert_eq!(adapter.0.load(Ordering::SeqCst), 0, "adapter never ran");
    }

    #[test]
    fn reboot_capability_declares_host_scope_and_interruptions() {
        let cap = RebootTool::new(std::sync::Arc::new(RefusingReboot)).capability();
        assert_eq!(cap.affected_scope, AffectedScope::Host);
        assert!(cap.interrupts_network);
        assert!(cap.interrupts_remote_admin);
        assert_eq!(cap.required_permission, "host.reboot");
    }

    #[test]
    fn concurrent_network_operations_never_race() {
        let _order = NETWORK_OPS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Hold the global network mutex like an in-flight operation, then
        // exercise the timeout refusal through the lock helper with a
        // zero wait (a full execute() would block for NETWORK_OPS_WAIT_MS).
        let guard = NETWORK_OPS.try_lock().expect("mutex free at test start");
        assert!(
            lock_network_ops(0).is_err(),
            "a held mutex must refuse a zero-wait contender"
        );
        drop(guard);
        // With the mutex free the same call succeeds.
        struct FastReset;
        impl custos_platform::NetworkInterfaceController for FastReset {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                Ok(())
            }
        }
        let tool = InterfaceResetTool::new(std::sync::Arc::new(FastReset));
        let outcome = tool
            .execute(&OperationParams {
                operation_id: "network.interface.reset".into(),
                resource_id: "wan".into(),
                run_id: None,
                args: [("interface".into(), serde_json::json!("eth0"))].into(),
            })
            .unwrap();
        assert!(matches!(
            outcome,
            ToolOutcome::Action {
                succeeded: true,
                ..
            }
        ));
    }

    #[test]
    fn contending_network_operation_waits_for_the_in_flight_one() {
        let _order = NETWORK_OPS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // C9: with another network operation in flight, a contender used
        // to be refused instantly — the engine recorded a failed attempt
        // (burning the ladder) for what is only contention. It must wait,
        // bounded, for the in-flight operation to finish.
        struct SlowReset;
        impl custos_platform::NetworkInterfaceController for SlowReset {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                std::thread::sleep(std::time::Duration::from_millis(250));
                Ok(())
            }
        }
        let slow = std::sync::Arc::new(InterfaceResetTool::new(std::sync::Arc::new(SlowReset)));
        let worker = std::thread::spawn(move || {
            slow.execute(&OperationParams {
                operation_id: "network.interface.reset".into(),
                resource_id: "wan".into(),
                run_id: None,
                args: [("interface".into(), serde_json::json!("eth0"))].into(),
            })
        });
        // Let the worker claim the global network mutex first.
        std::thread::sleep(std::time::Duration::from_millis(80));

        struct QuickReset;
        impl custos_platform::NetworkInterfaceController for QuickReset {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                Ok(())
            }
        }
        let tool = InterfaceResetTool::new(std::sync::Arc::new(QuickReset));
        let started = std::time::Instant::now();
        let outcome = tool.execute(&OperationParams {
            operation_id: "network.interface.reset".into(),
            resource_id: "lan".into(),
            run_id: None,
            args: [("interface".into(), serde_json::json!("eth1"))].into(),
        });
        let elapsed = started.elapsed();
        assert!(
            matches!(
                outcome,
                Ok(ToolOutcome::Action {
                    succeeded: true,
                    ..
                })
            ),
            "contender must wait for the in-flight operation, got {outcome:?}"
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(60),
            "contender returned in {elapsed:?} — it did not wait"
        );
        assert!(
            worker.join().expect("worker").is_ok(),
            "in-flight operation must succeed"
        );
    }

    #[test]
    fn interface_reset_requires_a_name() {
        struct Noop;
        impl custos_platform::NetworkInterfaceController for Noop {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                Ok(())
            }
        }
        let tool = InterfaceResetTool::new(std::sync::Arc::new(Noop));
        let err = tool
            .execute(&OperationParams {
                operation_id: "network.interface.reset".into(),
                resource_id: "wan".into(),
                run_id: None,
                args: Default::default(),
            })
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::InvalidParams);
        assert_eq!(tool.capability().required_permission, "host.network");
    }

    #[test]
    fn wifi_reconnect_requires_a_profile() {
        struct Noop;
        impl custos_platform::NetworkInterfaceController for Noop {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                Ok(())
            }
        }
        let tool = WifiReconnectTool::new(std::sync::Arc::new(Noop));
        let err = tool
            .execute(&OperationParams {
                operation_id: "wifi.reconnect".into(),
                resource_id: "wan".into(),
                run_id: None,
                args: Default::default(),
            })
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::InvalidParams);
        assert_eq!(tool.capability().required_permission, "host.network");
    }

    #[test]
    fn wifi_reconnect_declares_network_scope_and_interruptions() {
        struct Noop;
        impl custos_platform::NetworkInterfaceController for Noop {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                Ok(())
            }
        }
        let cap = WifiReconnectTool::new(std::sync::Arc::new(Noop)).capability();
        assert_eq!(cap.operation_id, "wifi.reconnect");
        assert!(cap.interrupts_network);
        assert!(cap.interrupts_remote_admin);
    }

    #[test]
    fn wifi_reconnect_unsupported_adapter_is_surfaced() {
        struct NoWifi;
        impl custos_platform::NetworkInterfaceController for NoWifi {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                Ok(())
            }
            fn connect_wifi(&self, _profile: &str) -> PlatformResult<()> {
                Err(PlatformError::Unsupported("no Wi-Fi adapter (test)".into()))
            }
        }
        let tool = WifiReconnectTool::new(std::sync::Arc::new(NoWifi));
        let err = tool
            .execute(&OperationParams {
                operation_id: "wifi.reconnect".into(),
                resource_id: "wan".into(),
                run_id: None,
                args: [("profile".into(), serde_json::json!("HomeNet"))].into(),
            })
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::Failed, "{err}");
        assert!(err.message.contains("no Wi-Fi adapter"), "{err}");
    }
}

#[cfg(test)]
mod power_tests {
    use super::*;
    use crate::registry::ToolRegistry;
    use custos_platform::PlatformError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fake adapter records each operation so the gates can be proven to
    /// refuse BEFORE the adapter runs (no real power policy is touched).
    #[derive(Default)]
    struct SpyPower {
        prevent: AtomicUsize,
        allow: AtomicUsize,
        arm: AtomicUsize,
        disarm: AtomicUsize,
        allow_minutes: std::sync::Mutex<Option<u64>>,
        arm_epoch: std::sync::Mutex<Option<u64>>,
    }

    impl custos_platform::PowerPolicyController for SpyPower {
        fn prevent_sleep(&self) -> custos_platform::PlatformResult<()> {
            self.prevent.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn allow_sleep(&self, minutes: u64) -> custos_platform::PlatformResult<()> {
            self.allow.fetch_add(1, Ordering::SeqCst);
            *self.allow_minutes.lock().unwrap() = Some(minutes);
            Ok(())
        }
        fn arm_wake(&self, epoch_secs: u64) -> custos_platform::PlatformResult<()> {
            self.arm.fetch_add(1, Ordering::SeqCst);
            *self.arm_epoch.lock().unwrap() = Some(epoch_secs);
            Ok(())
        }
        fn disarm_wake(&self) -> custos_platform::PlatformResult<()> {
            self.disarm.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct RefusingPower;
    impl custos_platform::PowerPolicyController for RefusingPower {
        fn prevent_sleep(&self) -> custos_platform::PlatformResult<()> {
            Err(PlatformError::Unsupported("no wired surface".into()))
        }
        fn allow_sleep(&self, _minutes: u64) -> custos_platform::PlatformResult<()> {
            Err(PlatformError::Unsupported("no wired surface".into()))
        }
        fn arm_wake(&self, _epoch_secs: u64) -> custos_platform::PlatformResult<()> {
            Err(PlatformError::Unsupported("no wired surface".into()))
        }
        fn disarm_wake(&self) -> custos_platform::PlatformResult<()> {
            Err(PlatformError::Unsupported("no wired surface".into()))
        }
    }

    fn params(op: &str, args: &[(&str, serde_json::Value)]) -> OperationParams {
        OperationParams {
            operation_id: op.into(),
            resource_id: "host".into(),
            run_id: None,
            args: args
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn power_ops_require_the_second_confirm_token() {
        let spy: std::sync::Arc<SpyPower> = std::sync::Arc::new(SpyPower::default());
        let mut registry = ToolRegistry::new();
        registry
            .register_builtin(Box::new(PreventSleepTool::new(spy.clone())))
            .unwrap();
        registry
            .register_builtin(Box::new(AllowSleepTool::new(spy.clone())))
            .unwrap();
        registry
            .register_builtin(Box::new(ArmWakeTool::new(spy.clone())))
            .unwrap();
        registry
            .register_builtin(Box::new(DisarmWakeTool::new(spy.clone())))
            .unwrap();
        let granted = |_: &str| true;

        for op in [
            "power.sleep.prevent",
            "power.sleep.allow",
            "power.wake.arm",
            "power.wake.disarm",
        ] {
            let err = registry
                .dispatch(&granted, &params(op, &[("minutes", 30.into())]))
                .unwrap_err();
            assert_eq!(err.kind, ToolboxErrorKind::InvalidParams, "{op}: {err}");
            assert!(err.message.contains("confirm"), "{op}: {err}");
        }
        assert_eq!(spy.prevent.load(Ordering::SeqCst), 0);
        assert_eq!(spy.allow.load(Ordering::SeqCst), 0);
        assert_eq!(spy.arm.load(Ordering::SeqCst), 0);
        assert_eq!(spy.disarm.load(Ordering::SeqCst), 0);

        // A stale token from the sleep operation cannot arm a wake.
        let err = registry
            .dispatch(
                &granted,
                &params(
                    "power.wake.arm",
                    &[
                        ("confirm", "sleep".into()),
                        ("epoch_secs", 1_764_000_000u64.into()),
                    ],
                ),
            )
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::InvalidParams, "{err}");
        assert_eq!(spy.arm.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn confirmed_power_ops_reach_the_adapter_with_explicit_params() {
        let spy: std::sync::Arc<SpyPower> = std::sync::Arc::new(SpyPower::default());
        let prevent = PreventSleepTool::new(spy.clone());
        let allow = AllowSleepTool::new(spy.clone());
        let arm = ArmWakeTool::new(spy.clone());
        let disarm = DisarmWakeTool::new(spy.clone());

        let out = prevent
            .execute(&params(
                "power.sleep.prevent",
                &[("confirm", "sleep".into())],
            ))
            .unwrap();
        assert_eq!(
            out,
            ToolOutcome::Action {
                succeeded: true,
                detail: "sleep prevention applied".into()
            }
        );
        assert_eq!(spy.prevent.load(Ordering::SeqCst), 1);

        allow
            .execute(&params(
                "power.sleep.allow",
                &[("confirm", "sleep".into()), ("minutes", 45u64.into())],
            ))
            .unwrap();
        assert_eq!(*spy.allow_minutes.lock().unwrap(), Some(45));

        arm.execute(&params(
            "power.wake.arm",
            &[
                ("confirm", "wake".into()),
                ("epoch_secs", 1_764_000_000u64.into()),
            ],
        ))
        .unwrap();
        assert_eq!(*spy.arm_epoch.lock().unwrap(), Some(1_764_000_000));

        disarm
            .execute(&params("power.wake.disarm", &[("confirm", "wake".into())]))
            .unwrap();
        assert_eq!(spy.disarm.load(Ordering::SeqCst), 1);

        // Missing explicit restore/wake values are refused even with the
        // confirm token.
        let err = allow
            .execute(&params("power.sleep.allow", &[("confirm", "sleep".into())]))
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::InvalidParams);
        let err = arm
            .execute(&params(
                "power.wake.arm",
                &[("confirm", "wake".into()), ("epoch_secs", 0u64.into())],
            ))
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::InvalidParams);
    }

    #[test]
    fn permission_gate_refuses_power_ops_before_the_adapter_runs() {
        let spy: std::sync::Arc<SpyPower> = std::sync::Arc::new(SpyPower::default());
        let mut registry = ToolRegistry::new();
        registry
            .register_builtin(Box::new(PreventSleepTool::new(spy.clone())))
            .unwrap();
        let err = registry
            .dispatch(
                &|_| false,
                &params("power.sleep.prevent", &[("confirm", "sleep".into())]),
            )
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::Denied, "{err}");
        assert!(err.message.contains("host.power"), "{err}");
        assert_eq!(spy.prevent.load(Ordering::SeqCst), 0, "adapter never ran");
    }

    #[test]
    fn power_capabilities_declare_host_scope_and_interruptions() {
        let adapter: std::sync::Arc<dyn custos_platform::PowerPolicyController> =
            std::sync::Arc::new(RefusingPower);
        for (tool, id) in [
            (
                Box::new(PreventSleepTool::new(adapter.clone())) as Box<dyn BuiltinTool>,
                "power.sleep.prevent",
            ),
            (
                Box::new(AllowSleepTool::new(adapter.clone())),
                "power.sleep.allow",
            ),
            (
                Box::new(ArmWakeTool::new(adapter.clone())),
                "power.wake.arm",
            ),
            (Box::new(DisarmWakeTool::new(adapter)), "power.wake.disarm"),
        ] {
            let cap = tool.capability();
            assert_eq!(cap.operation_id, id);
            assert_eq!(cap.affected_scope, AffectedScope::Host);
            assert!(cap.interrupts_network);
            assert!(cap.interrupts_remote_admin);
            assert!(!cap.read_only);
            assert_eq!(cap.required_permission, "host.power");
        }
    }

    #[test]
    fn unsupported_adapter_errors_surface_without_panicking() {
        let tool = PreventSleepTool::new(std::sync::Arc::new(RefusingPower));
        let err = tool
            .execute(&params(
                "power.sleep.prevent",
                &[("confirm", "sleep".into())],
            ))
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::Failed);
        assert!(err.message.contains("no wired surface"), "{err}");
    }

    #[test]
    fn default_registry_wires_all_four_power_ops() {
        let registry = ToolRegistry::with_builtin_tools();
        for id in [
            "power.sleep.prevent",
            "power.sleep.allow",
            "power.wake.arm",
            "power.wake.disarm",
        ] {
            assert!(registry.knows(id), "{id} must be registered");
        }
    }
}

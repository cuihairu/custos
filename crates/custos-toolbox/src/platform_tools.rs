//! High-risk platform tools (todo.md P2 权限检查, ADR-0012): reboot and
//! interface reset wrapped as toolbox capabilities whose `required_permission`
//! the dispatch layer enforces — an unauthorized call is refused before the
//! platform adapter ever runs. The adapters themselves never self-authorize.
//! The two network-affecting operations also share a global mutex (todo.md
//! P3 223): at most one network operation runs at a time, across resources.

/// The global network-operations mutex (todo.md P3 223 网络操作互斥):
/// AT MOST ONE network operation (reset, reboot, any future
/// network-affecting action) may run at a time, regardless of which
/// resource requested it — concurrent interface changes would race with
/// each other. A second caller gets a concrete error immediately (never
/// queues and surprises later).
static NETWORK_OPS: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _guard = match NETWORK_OPS.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Err(ToolboxError::new(
                    ToolboxErrorKind::Failed,
                    "another network operation is already running; refusing to race it",
                ))
            }
        };
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
        let _guard = match NETWORK_OPS.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Err(ToolboxError::new(
                    ToolboxErrorKind::Failed,
                    "another network operation is already running; refusing to race it",
                ))
            }
        };
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
        let _guard = match NETWORK_OPS.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Err(ToolboxError::new(
                    ToolboxErrorKind::Failed,
                    "another network operation is already running; refusing to race it",
                ))
            }
        };
        self.controller
            .connect_wifi(profile)
            .map(|_| ToolOutcome::Action {
                succeeded: true,
                detail: format!("wifi profile {profile} connect issued"),
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
        struct SlowReset;
        impl custos_platform::NetworkInterfaceController for SlowReset {
            fn list(&self) -> PlatformResult<Vec<custos_platform::InterfaceStatus>> {
                Ok(vec![])
            }
            fn reset(&self, _name: &str) -> PlatformResult<()> {
                std::thread::sleep(std::time::Duration::from_millis(200));
                Ok(())
            }
        }
        // Hold the global network mutex like an in-flight operation.
        let guard = NETWORK_OPS.try_lock().expect("mutex free at test start");
        let tool = InterfaceResetTool::new(std::sync::Arc::new(SlowReset));
        let err = tool
            .execute(&OperationParams {
                operation_id: "network.interface.reset".into(),
                resource_id: "wan".into(),
                run_id: None,
                args: [("interface".into(), serde_json::json!("eth0"))].into(),
            })
            .unwrap_err();
        assert!(err.message.contains("already running"), "{err}");
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

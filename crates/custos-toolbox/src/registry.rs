//! The unified dispatch table (ADR-0015): one place answers "who serves
//! this operation id" — a built-in tool or a plugin subprocess — and one
//! place enforces the permission gate before anything executes.

use custos_plugin_api::{Capability, OperationParams};
use std::collections::BTreeMap;

/// Result of one tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    /// A probe provider's verdict.
    Probe { healthy: bool, observation: String },
    /// An action provider's outcome.
    Action { succeeded: bool, detail: String },
}

/// Why a dispatch failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolboxErrorKind {
    /// Unknown operation id.
    UnknownOperation,
    /// The permission gate refused the call (ADR-0012).
    Denied,
    /// The parameters do not match the tool's contract.
    InvalidParams,
    /// The executor ran and failed.
    Failed,
    /// The executor hit its deadline.
    TimedOut,
}

/// One dispatch failure. Never panics; carries no secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolboxError {
    pub kind: ToolboxErrorKind,
    pub message: String,
}

impl ToolboxError {
    pub fn new(kind: ToolboxErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ToolboxError {}

/// A built-in capability executor: declares itself with the eleven-field
/// `Capability` metadata and runs parameterized operations in-process.
pub trait BuiltinTool: Send + Sync {
    fn capability(&self) -> Capability;

    /// Executes one operation. The dispatch layer has already checked the
    /// permission gate; the tool must still validate its own parameters.
    fn execute(&self, params: &OperationParams) -> Result<ToolOutcome, ToolboxError>;
}

/// What dispatch decided for one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchedOutcome {
    /// A built-in tool ran in-process.
    Builtin(ToolOutcome),
    /// The operation belongs to a plugin; the runtime must forward the call
    /// to the named plugin subprocess.
    ToPlugin { plugin: String },
}

/// A plugin-declared operation and the capability metadata needed to apply
/// the same permission gate as built-ins before the host forwards the call.
#[derive(Debug, Clone)]
struct PluginOperation {
    plugin: String,
    required_permission: String,
}

/// The one dispatch table. Built-in tools and plugin-declared operations
/// register here; lookups answer with the executor (or the plugin to call).
#[derive(Default)]
pub struct ToolRegistry {
    builtins: BTreeMap<String, Box<dyn BuiltinTool>>,
    /// operation id → owning plugin and declared capability.
    delegated: BTreeMap<String, PluginOperation>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the core's built-in executors (ADR-0015): controlled
    /// command execution and process status today.
    pub fn with_builtin_tools() -> Self {
        let mut registry = Self::new();
        registry
            .register_builtin(Box::new(crate::exec_tool::ExecTool::new()))
            .expect("builtin operation ids are unique");
        registry
            .register_builtin(Box::new(crate::process_tool::ProcessStatusTool::new()))
            .expect("builtin operation ids are unique");
        registry
    }

    /// Registers a built-in tool; duplicate operation ids are a wiring bug.
    pub fn register_builtin(&mut self, tool: Box<dyn BuiltinTool>) -> Result<(), String> {
        let operation_id = tool.capability().operation_id.clone();
        if self.builtins.contains_key(&operation_id) || self.delegated.contains_key(&operation_id) {
            return Err(format!(
                "operation `{operation_id}` is already registered; operation ids must be unique across builtins and plugins"
            ));
        }
        self.builtins.insert(operation_id, tool);
        Ok(())
    }

    /// Registers a plugin-declared operation in the same table.
    pub fn register_plugin_operation(
        &mut self,
        operation_id: impl Into<String>,
        plugin: impl Into<String>,
        required_permission: impl Into<String>,
    ) -> Result<(), String> {
        let operation_id = operation_id.into();
        if self.builtins.contains_key(&operation_id) || self.delegated.contains_key(&operation_id) {
            return Err(format!(
                "operation `{operation_id}` is already registered; operation ids must be unique across builtins and plugins"
            ));
        }
        self.delegated.insert(
            operation_id,
            PluginOperation {
                plugin: plugin.into(),
                required_permission: required_permission.into(),
            },
        );
        Ok(())
    }

    /// True when the operation id is known (builtin or delegated).
    pub fn knows(&self, operation_id: &str) -> bool {
        self.builtins.contains_key(operation_id) || self.delegated.contains_key(operation_id)
    }

    /// All capability declarations currently registered, builtins first.
    pub fn capabilities(&self) -> Vec<Capability> {
        let mut all: Vec<Capability> = self.builtins.values().map(|t| t.capability()).collect();
        all.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        all
    }

    /// Dispatches one operation. The permission gate runs BEFORE any
    /// executor: `granted` answers whether a permission string (e.g.
    /// `host.exec`) is currently allowed (ADR-0012 — high-risk actions
    /// without authorization are refused, not attempted).
    pub fn dispatch(
        &self,
        granted: &dyn Fn(&str) -> bool,
        params: &OperationParams,
    ) -> Result<DispatchedOutcome, ToolboxError> {
        if let Some(operation) = self.delegated.get(&params.operation_id) {
            if !granted(&operation.required_permission) {
                return Err(ToolboxError::new(
                    ToolboxErrorKind::Denied,
                    format!(
                        "operation `{}` requires permission `{}` which is not granted",
                        params.operation_id, operation.required_permission
                    ),
                ));
            }
            return Ok(DispatchedOutcome::ToPlugin {
                plugin: operation.plugin.clone(),
            });
        }
        let Some(tool) = self.builtins.get(&params.operation_id) else {
            return Err(ToolboxError::new(
                ToolboxErrorKind::UnknownOperation,
                format!("no builtin or plugin serves `{}`", params.operation_id),
            ));
        };
        let capability = tool.capability();
        if !granted(&capability.required_permission) {
            return Err(ToolboxError::new(
                ToolboxErrorKind::Denied,
                format!(
                    "operation `{}` requires permission `{}` which is not granted",
                    capability.operation_id, capability.required_permission
                ),
            ));
        }
        tool.execute(params).map(DispatchedOutcome::Builtin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use custos_plugin_api::{AffectedScope, CapabilityKind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingTool {
        calls: AtomicUsize,
        permission: &'static str,
    }

    impl BuiltinTool for CountingTool {
        fn capability(&self) -> Capability {
            Capability {
                kind: CapabilityKind::ProbeProvider,
                operation_id: "test.count".into(),
                summary: "counts calls".into(),
                read_only: true,
                required_permission: self.permission.into(),
                affected_scope: AffectedScope::NamedResource,
                interrupts_network: false,
                interrupts_remote_admin: false,
                cancelable: false,
                idempotent: true,
                verifiable: false,
                possible_failures: vec![],
                compensation_operation: None,
            }
        }

        fn execute(&self, _params: &OperationParams) -> Result<ToolOutcome, ToolboxError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutcome::Probe {
                healthy: true,
                observation: "ok".into(),
            })
        }
    }

    fn params(operation_id: &str) -> OperationParams {
        OperationParams {
            operation_id: operation_id.into(),
            resource_id: "r".into(),
            run_id: None,
            args: Default::default(),
        }
    }

    #[test]
    fn builtin_dispatch_runs_in_process() {
        let tool = Box::new(CountingTool {
            calls: AtomicUsize::new(0),
            permission: "probe.run",
        });
        let weak = tool.calls.fetch_add(0, Ordering::SeqCst);
        let mut registry = ToolRegistry::new();
        registry.register_builtin(tool).unwrap();
        let outcome = registry.dispatch(&|_| true, &params("test.count")).unwrap();
        assert!(matches!(
            outcome,
            DispatchedOutcome::Builtin(ToolOutcome::Probe { healthy: true, .. })
        ));
        assert_eq!(weak, 0);
    }

    #[test]
    fn permission_gate_refuses_before_execution() {
        let tool = Box::new(CountingTool {
            calls: AtomicUsize::new(0),
            permission: "host.exec",
        });
        let counter = tool.calls.fetch_add(0, Ordering::SeqCst);
        let mut registry = ToolRegistry::new();
        registry.register_builtin(tool).unwrap();
        let err = registry
            .dispatch(&|perm| perm == "probe.run", &params("test.count"))
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::Denied);
        assert!(err.message.contains("host.exec"), "{err}");
        // The executor never ran.
        assert_eq!(counter, 0);
    }

    #[test]
    fn plugin_operations_require_permission_before_dispatch() {
        let mut registry = ToolRegistry::new();
        registry
            .register_plugin_operation("mihomo.switch_node", "mihomo", "proxy.switch")
            .unwrap();
        let err = registry
            .dispatch(
                &|perm| perm != "proxy.switch",
                &params("mihomo.switch_node"),
            )
            .unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::Denied);
        assert!(err.message.contains("proxy.switch"), "{err}");

        let outcome = registry
            .dispatch(
                &|perm| perm == "proxy.switch",
                &params("mihomo.switch_node"),
            )
            .unwrap();
        assert_eq!(
            outcome,
            DispatchedOutcome::ToPlugin {
                plugin: "mihomo".into()
            }
        );
    }

    #[test]
    fn duplicate_operation_ids_are_rejected() {
        let mut registry = ToolRegistry::new();
        registry
            .register_builtin(Box::new(CountingTool {
                calls: AtomicUsize::new(0),
                permission: "p",
            }))
            .unwrap();
        assert!(registry
            .register_plugin_operation("test.count", "other", "permission")
            .is_err());
    }

    #[test]
    fn unknown_operations_report_clearly() {
        let registry = ToolRegistry::new();
        let err = registry.dispatch(&|_| true, &params("nope")).unwrap_err();
        assert_eq!(err.kind, ToolboxErrorKind::UnknownOperation);
        assert!(err.message.contains("nope"), "{err}");
    }
}

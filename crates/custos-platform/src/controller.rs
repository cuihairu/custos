//! The six cross-platform interfaces (docs/guide/platforms.md §11.1).
//! Each defines only the COMMON semantics; platform-specific capabilities
//! are described by [`crate::PlatformCapabilitySet`], never faked for the
//! sake of a unified interface.

use crate::error::PlatformResult;

/// A live process on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

/// Detects processes on this host (read-only).
pub trait ProcessController: Send + Sync {
    /// True when the pid exists and belongs to a live process.
    fn is_alive(&self, pid: u32) -> bool;

    /// All live processes whose name matches exactly.
    fn find_by_name(&self, name: &str) -> PlatformResult<Vec<ProcessInfo>>;
}

/// A service's lifecycle state, normalized across platforms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Stopped,
    /// Transitional (start pending, stop pending, activating…).
    Transitioning,
    /// The platform reports the unit in a failed state.
    Failed,
    /// The platform answered, but with a state this host's vocabulary maps
    /// to nothing standard — carried verbatim for the human, never guessed.
    Unknown(String),
}

/// Controls one service (start/stop/restart/status) under the host's init
/// system. Whether an adapter serves an operation at all is declared in its
/// capability set; adapters never guess-execute on the wrong init system.
pub trait ServiceController: Send + Sync {
    fn status(&self, unit: &str) -> PlatformResult<ServiceState>;
    fn start(&self, unit: &str) -> PlatformResult<()>;
    fn stop(&self, unit: &str) -> PlatformResult<()>;
    fn restart(&self, unit: &str) -> PlatformResult<()>;
}

/// A network interface's observable status (read-only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceStatus {
    pub name: String,
    pub up: bool,
    /// Addresses as reported by the platform (may be empty).
    pub addresses: Vec<String>,
}

/// Observes and (where the platform allows) resets network interfaces.
/// Resetting can cut the management channel — the capability metadata must
/// carry the `interrupts_*` flags and the operation stays gated (ADR-0012).
pub trait NetworkInterfaceController: Send + Sync {
    fn list(&self) -> PlatformResult<Vec<InterfaceStatus>>;

    /// Brings one named interface down and up again (high risk).
    fn reset(&self, name: &str) -> PlatformResult<()>;
}

/// One connectivity target the platform layer can check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectivityTarget {
    pub host: String,
    pub port: u16,
}

/// The answer of a connectivity check: normalized, not raw tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connectivity {
    Reachable { latency_ms: u64 },
    Refused,
    Unreachable(String),
}

/// Checks network connectivity using this platform's primitives.
pub trait NetworkConnectivityProbe: Send + Sync {
    fn probe_tcp(&self, target: &ConnectivityTarget) -> PlatformResult<Connectivity>;
}

/// Reads one named credential. Implementations must never log the value
/// (docs/guide/notification.md §8.7 carries over to platform secrets).
pub trait CredentialProvider: Send + Sync {
    /// The credential, or None when unset. Failures surface as errors.
    fn get(&self, name: &str) -> PlatformResult<Option<String>>;
}

/// Reboots the host. Gated by authorization at the dispatch layer
/// (ADR-0012: an unauthorized reboot is refused, not attempted).
pub trait RebootController: Send + Sync {
    fn reboot(&self) -> PlatformResult<()>;
}

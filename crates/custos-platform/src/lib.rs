//! Cross-platform capability interfaces and their platform implementations
//! (docs/guide/platforms.md §11.1, ADR-0012, ADR-0015).
//!
//! Each interface defines only the cross-platform COMMON semantics.
//! Platform support is never faked: an adapter that cannot serve an
//! operation returns [`PlatformError::Unsupported`] with the concrete
//! reason, and the caller must surface it — never guess-execute
//! (§11.1, todo.md 服务控制矩阵: 绝不误执行).
//!
//! Dangerous primitives (interface reset, reboot) declare their risk via
//! the toolbox [`Capability`] metadata; the permission gate lives in the
//! dispatch layer (`custos-toolbox`), adapters never self-authorize.

pub mod capability;
pub mod controller;
pub mod error;
#[cfg(target_os = "linux")]
pub mod init_systems;
#[cfg(windows)]
pub mod init_systems;
#[cfg(not(target_os = "macos"))]
pub mod linux;
pub mod windows;

pub use capability::PlatformCapabilitySet;
pub use controller::{
    Connectivity, ConnectivityTarget, CredentialProvider, InterfaceStatus,
    NetworkConnectivityProbe, NetworkInterfaceController, ProcessController, ProcessInfo,
    RebootController, ServiceController, ServiceState,
};
pub use error::{PlatformError, PlatformResult};

/// Placeholder retained for compatibility; the adapters have landed.
pub fn not_yet_implemented() -> &'static str {
    "custos-platform: platform adapters are planned for P2 (docs/guide/roadmap.md)"
}

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
pub mod openwrt;
pub mod windows;

pub use capability::PlatformCapabilitySet;
pub use controller::{
    Connectivity, ConnectivityTarget, CredentialProvider, InterfaceStatus,
    NetworkConnectivityProbe, NetworkInterfaceController, ProcessController, ProcessInfo,
    RebootController, ServiceController, ServiceState,
};
pub use error::{PlatformError, PlatformResult};

/// The default reboot controller for THIS host (todo.md 208 重启矩阵).
/// The returned adapter refuses `reboot` with a concrete reason on hosts
/// without a wired implementation; authorization still lives in toolbox
/// dispatch (ADR-0012), never in the adapter.
pub fn default_reboot_controller() -> std::sync::Arc<dyn RebootController> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(windows::WindowsRebootController)
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::sync::Arc::new(linux::LinuxRebootController::detect())
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(MacosRebootController)
    }
}

#[cfg(target_os = "macos")]
struct MacosRebootController;

#[cfg(target_os = "macos")]
impl RebootController for MacosRebootController {
    fn reboot(&self) -> PlatformResult<()> {
        Err(PlatformError::Unsupported(
            "no macOS reboot controller is implemented yet".into(),
        ))
    }
}

/// The default network interface controller for THIS host (todo.md 209
/// 网络控制矩阵). On Windows the `netsh` adapter; on Unix the sysfs/proc
/// controller; macOS has no adapter yet, so it answers `Unsupported` for
/// every operation (绝不误执行). The toolbox registers its built-in
/// platform tools with this controller so the permission-gated dispatch
/// path runs the real adapter rather than nothing.
pub fn default_network_controller() -> std::sync::Arc<dyn NetworkInterfaceController> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(windows::NetshNetworkController)
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::sync::Arc::new(linux::SysnetInterfaceController)
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(MacosNetworkController)
    }
}

#[cfg(target_os = "macos")]
struct MacosNetworkController;

#[cfg(target_os = "macos")]
impl NetworkInterfaceController for MacosNetworkController {
    fn list(&self) -> PlatformResult<Vec<InterfaceStatus>> {
        Err(PlatformError::Unsupported(
            "no macOS network adapter is implemented yet".into(),
        ))
    }
    fn reset(&self, _name: &str) -> PlatformResult<()> {
        Err(PlatformError::Unsupported(
            "no macOS network adapter is implemented yet".into(),
        ))
    }
}

/// Placeholder retained for compatibility; the adapters have landed.
pub fn not_yet_implemented() -> &'static str {
    "custos-platform: platform adapters are planned for P2 (docs/guide/roadmap.md)"
}

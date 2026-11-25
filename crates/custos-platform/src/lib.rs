//! Cross-platform capability interfaces and their platform implementations
//! (docs/guide/platforms.md §11.1, ADR-0012, ADR-0015).
//!
//! Each interface defines only the cross-platform COMMON semantics.
//! Platform support is never faked: an adapter that cannot serve an
//! operation returns [`PlatformError::Unsupported`] with the concrete
//! reason, and the caller must surface it — never guess-execute
//! (§11.1, todo.md service-control matrix: never guess-execute).
//!
//! Dangerous primitives (interface reset, reboot) declare their risk via
//! the toolbox [`Capability`] metadata; the permission gate lives in the
//! dispatch layer (`custos-toolbox`), adapters never self-authorize.

pub mod capability;
pub mod controller;
pub mod error;
// Per-init-system adapters, partitioned inside: systemd/SysV (Linux),
// launchd (macOS), SCM (Windows). macOS compiles it too — before this gate
// the launchd module was unreachable: its parent never compiled on macOS
// (found by the macos CI leg).
#[cfg(any(unix, windows))]
pub mod init_systems;
#[cfg(all(not(target_os = "macos"), not(target_os = "freebsd")))]
pub mod linux;
// FreeBSD adapters (rc.d services, ifconfig listing, shutdown reboot) are a
// parallel platform, not a linux variant: the rc.d vocabulary is its own
// (init_systems::rcd), the ifconfig listing is shared via the parser below.
#[cfg(target_os = "freebsd")]
pub mod bsd;
// The pure BSD-family ifconfig parser: ungated so its fixture tests run on
// every platform; the live adapters are cfg-gated in macos/bsd.
pub mod ifconfig;
// macOS adapters (ifconfig listing, shutdown reboot) live here instead of
// the linux module: they are a parallel platform, not a linux variant.
#[cfg(target_os = "macos")]
pub mod macos;
pub mod openwrt;
pub mod power_parsers;
pub mod windows;

pub use capability::PlatformCapabilitySet;
pub use controller::{
    Connectivity, ConnectivityTarget, CredentialProvider, InterfaceStatus,
    NetworkConnectivityProbe, NetworkInterfaceController, PowerPolicyController, ProcessController,
    ProcessInfo, RebootController, ServiceController, ServiceState,
};
pub use error::{PlatformError, PlatformResult};

/// The default reboot controller for THIS host (todo.md 208 reboot matrix).
/// The returned adapter refuses `reboot` with a concrete reason on hosts
/// without a wired implementation; authorization still lives in toolbox
/// dispatch (ADR-0012), never in the adapter.
pub fn default_reboot_controller() -> std::sync::Arc<dyn RebootController> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(windows::WindowsRebootController)
    }
    #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "freebsd")))]
    {
        std::sync::Arc::new(linux::LinuxRebootController::detect())
    }
    #[cfg(target_os = "freebsd")]
    {
        std::sync::Arc::new(bsd::BsdRebootController)
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacosRebootController)
    }
}

/// The default network interface controller for THIS host (todo.md 209
/// network control matrix). On Windows the `netsh` adapter; on Unix the
/// sysfs/proc controller; on macOS the `ifconfig` adapter. The toolbox
/// registers its
/// built-in platform tools with this controller so the permission-gated
/// dispatch path runs the real adapter rather than nothing.
pub fn default_network_controller() -> std::sync::Arc<dyn NetworkInterfaceController> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(windows::NetshNetworkController)
    }
    #[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "freebsd")))]
    {
        std::sync::Arc::new(linux::SysnetInterfaceController)
    }
    #[cfg(target_os = "freebsd")]
    {
        std::sync::Arc::new(bsd::BsdNetworkController)
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacosNetworkController)
    }
}

/// The default power policy controller for THIS host (todo.md power probe
/// family). Sleep masking and RTC wake arming are platform-specific; hosts
/// without a wired implementation get the adapter that refuses each call
/// with a concrete reason (authorization still lives in toolbox dispatch,
/// ADR-0012).
pub fn default_power_policy() -> std::sync::Arc<dyn PowerPolicyController> {
    #[cfg(all(not(target_os = "macos"), not(target_os = "freebsd")))]
    {
        std::sync::Arc::new(linux::LinuxPowerPolicyController::detect())
    }
    #[cfg(target_os = "freebsd")]
    {
        std::sync::Arc::new(bsd::BsdPowerPolicyController)
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacosPowerPolicyController)
    }
}

/// Placeholder retained for compatibility; the adapters have landed.
pub fn not_yet_implemented() -> &'static str {
    "custos-platform: platform adapters are planned for P2 (docs/guide/roadmap.md)"
}

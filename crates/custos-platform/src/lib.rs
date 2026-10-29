//! Cross-platform capability interfaces and their platform implementations
//! (docs/guide/platforms.md §11.1).
//!
//! Each interface only defines cross-platform common semantics; platform
//! support must never be faked for the sake of a unified interface.
//!
//! P2 scope: ProcessController, ServiceController, NetworkInterfaceController,
//! NetworkConnectivityProbe, CredentialProvider, SystemRebootController —
//! with Linux and Windows first.

/// Placeholder until the platform traits land in P2.
pub fn not_yet_implemented() -> &'static str {
    "custos-platform: platform adapters are planned for P2 (docs/guide/roadmap.md)"
}

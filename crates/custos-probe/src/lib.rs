//! Probe scheduling and result normalization
//! (docs/guide/workspace.md: `custos-probe`).
//!
//! P0 scope (docs/guide/probe-diagnosis.md §5.4): Process Probe and Exec
//! Probe only. Exec Probe must limit runtime and output size and clean up the
//! process tree on timeout.

/// Placeholder until the P0 probes land.
pub fn not_yet_implemented() -> &'static str {
    "custos-probe: process/exec probes are planned for P0 (docs/guide/roadmap.md)"
}

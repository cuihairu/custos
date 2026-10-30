//! Structured logging bootstrap. `custos-core` owns the one global
//! `tracing` subscriber init (docs/guide/workspace.md): binaries call
//! [`init_tracing`] once at startup, libraries only emit through `tracing`.
//!
//! The level comes from `RUST_LOG` holding a single keyword
//! (`error|warn|info|debug|trace`); anything else falls back to `info`.
//! That covers custos today without pulling the full env-filter machinery.

use tracing::Level;

/// Installs the global subscriber. Idempotent: a second call (or an
/// embedded custos with its own subscriber) is ignored, never a panic.
pub fn init_tracing() {
    let level = level_from_env();
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .compact()
        .try_init();
}

/// The level `RUST_LOG` asks for, or `info` when unset/unknown.
pub fn level_from_env() -> Level {
    std::env::var("RUST_LOG")
        .ok()
        .and_then(|raw| parse_level(&raw))
        .unwrap_or(Level::INFO)
}

fn parse_level(raw: &str) -> Option<Level> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "error" => Some(Level::ERROR),
        "warn" => Some(Level::WARN),
        "info" => Some(Level::INFO),
        "debug" => Some(Level::DEBUG),
        "trace" => Some(Level::TRACE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keywords_map_to_levels() {
        assert_eq!(parse_level("error"), Some(Level::ERROR));
        assert_eq!(parse_level("TRACE"), Some(Level::TRACE));
        assert_eq!(parse_level(" info "), Some(Level::INFO));
    }

    #[test]
    fn unknown_or_empty_falls_back() {
        assert_eq!(parse_level(""), None);
        // Full env-filter syntax is not supported on purpose; it degrades
        // to info rather than pretending to honor per-target directives.
        assert_eq!(parse_level("custos=debug,hyper=off"), None);
    }

    #[test]
    fn init_is_idempotent() {
        init_tracing();
        init_tracing(); // must not panic
    }
}

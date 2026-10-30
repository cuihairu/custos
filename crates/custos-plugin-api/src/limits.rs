//! Wire limits for the stdio protocol (docs/guide/plugins.md §4.4).

/// Default per-message cap: 1 MiB, configurable per host. Messages above
/// the cap are rejected with `MESSAGE_TOO_LARGE`; large evidence must be
/// summarized, truncated or chunked instead.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// Upper bound for stderr diagnostic buffering: the reader keeps draining
/// (so a chatty plugin can never block on a full pipe) but only retains
/// this much for incident reports.
pub const MAX_STDERR_RETAINED_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        assert_eq!(DEFAULT_MAX_MESSAGE_BYTES, 1024 * 1024, "§4.4: 1 MiB");
        assert!(MAX_STDERR_RETAINED_BYTES < DEFAULT_MAX_MESSAGE_BYTES);
    }
}

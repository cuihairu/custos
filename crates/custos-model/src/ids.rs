//! Stable identifiers (docs/guide/resources.md §3.1: resource IDs must not
//! change across process, plugin or machine restarts).

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

string_id!(
    /// Unique, stable identifier of a managed resource.
    ResourceId
);
string_id!(
    /// Identifier of one fault episode; multiple recovery attempts may share it.
    IncidentId
);
string_id!(
    /// Identifier of one complete recovery execution.
    RecoveryRunId
);
string_id!(
    /// Explicit identifier of a recovery action invocation.
    ActionId
);
string_id!(
    /// Unique, stable identifier of one notification event. Producers assign
    /// it once and reuse it across retries so channels can deduplicate
    /// (docs/guide/notification.md §8.6: stable event ID + best-effort dedup).
    EventId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_serde() {
        let id = ResourceId::new("proxy.egress");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"proxy.egress\"");
        assert_eq!(serde_json::from_str::<ResourceId>(&json).unwrap(), id);
    }

    #[test]
    fn displays_as_inner_string() {
        assert_eq!(RecoveryRunId::new("run-1").to_string(), "run-1");
    }
}

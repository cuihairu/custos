//! Configuration parsing and validation (docs/guide/dashboard-cli-config.md
//! §10, docs/guide/resources.md §3.5).
//!
//! Configuration load time is the gate: unknown fields are errors, duplicate
//! IDs are errors, dangling dependency references are errors, and dependency
//! cycles are errors — startup is rejected instead of recursing forever
//! during recovery.

use custos_model::{DependencyGraph, GraphError, ResourceId};
use serde::Deserialize;

/// Supported configuration schema version.
pub const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported configuration version {found} (supported: {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("duplicate resource id `{0}`")]
    DuplicateResource(String),
    #[error("configuration validation failed: {0}")]
    Graph(#[from] GraphError),
}

/// One resource entry as written in the configuration file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResource {
    id: String,
    // The resource type is carried into the runtime registry in P0; the
    // startup gate only validates the graph today.
    #[allow(dead_code)]
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    depends_on: Vec<String>,
}

/// Top-level configuration document. Unknown fields must not be silently
/// interpreted as valid configuration (docs/guide/dashboard-cli-config.md §10.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    version: u32,
    #[serde(default)]
    resources: Vec<RawResource>,
}

/// Validated configuration ready for the runtime registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedConfig {
    pub resource_ids: Vec<ResourceId>,
    pub graph: DependencyGraph,
}

/// Parses, validates and builds the dependency graph. This is the startup
/// path: any error here must abort startup (docs/guide/resources.md §3.5 —
/// "reject at configuration stage, never recurse during recovery").
pub fn load(config_text: &str) -> Result<ValidatedConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(config_text)?;

    if raw.version != SUPPORTED_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            found: raw.version,
            supported: SUPPORTED_VERSION,
        });
    }

    // Unique resource IDs.
    let mut seen = std::collections::HashSet::new();
    for resource in &raw.resources {
        if !seen.insert(resource.id.as_str()) {
            return Err(ConfigError::DuplicateResource(resource.id.clone()));
        }
    }

    // Build the graph; DependencyGraph::build rejects unknown references and
    // cycles with the concrete error path.
    let edges = raw
        .resources
        .iter()
        .map(|r| {
            (
                ResourceId::new(r.id.clone()),
                r.depends_on.iter().map(ResourceId::new).collect::<Vec<_>>(),
            )
        })
        .collect();

    let graph = DependencyGraph::build(edges)?;

    Ok(ValidatedConfig {
        resource_ids: raw
            .resources
            .iter()
            .map(|r| ResourceId::new(&r.id))
            .collect(),
        graph,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
version = 1

[[resources]]
id = "network"
type = "network-connectivity"

[[resources]]
id = "proxy.egress"
type = "proxy"
depends_on = ["network"]

[[resources]]
id = "tunnel"
type = "tunnel"
depends_on = ["proxy.egress"]
"#;

    #[test]
    fn accepts_valid_configuration() {
        let config = load(VALID).unwrap();
        assert_eq!(config.resource_ids.len(), 3);
        assert_eq!(config.graph.nodes().count(), 3);
    }

    #[test]
    fn rejects_unknown_fields_instead_of_silently_ignoring() {
        // docs/guide/dashboard-cli-config.md §10.1: unknown fields must not
        // be silently interpreted as valid configuration.
        let result = load("version = 1\nunknown_key = true");
        assert!(result.is_err(), "unknown fields must be rejected");
    }

    #[test]
    fn rejects_unsupported_version() {
        let err = load("version = 99").unwrap_err();
        assert!(matches!(
            err,
            ConfigError::UnsupportedVersion {
                found: 99,
                supported: 1
            }
        ));
    }

    #[test]
    fn rejects_duplicate_resource_ids() {
        let config = r#"
version = 1

[[resources]]
id = "proxy"
type = "proxy"

[[resources]]
id = "proxy"
type = "proxy"
"#;
        let err = load(config).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateResource(id) if id == "proxy"));
    }

    #[test]
    fn rejects_dangling_dependency_reference() {
        let config = r#"
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
depends_on = ["ghost"]
"#;
        let err = load(config).unwrap_err();
        assert!(
            err.to_string().contains("ghost"),
            "error must name the missing resource, got: {err}"
        );
    }

    #[test]
    fn rejects_dependency_cycle_at_startup() {
        // The user ruling: on startup the graph must be checked for cycles;
        // a cycle is an error, never an infinite recovery loop.
        let config = r#"
version = 1

[[resources]]
id = "tunnel"
type = "tunnel"
depends_on = ["proxy"]

[[resources]]
id = "proxy"
type = "proxy"
depends_on = ["network"]

[[resources]]
id = "network"
type = "network-connectivity"
depends_on = ["tunnel"]
"#;
        let err = load(config).unwrap_err();
        assert!(
            err.to_string().contains("dependency cycle detected"),
            "startup must fail with a cycle error, got: {err}"
        );
        assert!(
            err.to_string().contains("->"),
            "cycle error must show the path, got: {err}"
        );
    }

    #[test]
    fn rejects_self_dependency() {
        let config = r#"
version = 1

[[resources]]
id = "proxy"
type = "proxy"
depends_on = ["proxy"]
"#;
        assert!(load(config).is_err());
    }
}

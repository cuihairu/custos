//! Python script plugin loader (todo.md P1: Python 入向) — runs a user
//! script under the SAME plugin contract as native plugins: the shim
//! (`plugins/python/custos_python_plugin.py`) speaks `custos/v1` stdio
//! JSON-RPC; timeout, restart policy, permission gates and budgets are
//! enforced by the host machinery, so scripts get identical semantics
//! (ADR-0002, ADR-0015).
//!
//! The loader only builds the [`PluginConfig`]: interpreter detection
//! (`CUSTOS_PYTHON_INTERPRETER` override, else `python3`), shim resolution
//! (`CUSTOS_PYTHON_SHIM` override, else the repository layout) and script
//! validation. Running and supervising stays with [`PluginSupervisor`].

use crate::session::PluginConfig;
use std::time::Duration;

/// The bundled shim, at its repository location.
pub const PYTHON_SHIM_PATH: &str = "plugins/python/custos_python_plugin.py";

/// Environment override for the interpreter (deployments may pin a venv).
pub const INTERPRETER_ENV: &str = "CUSTOS_PYTHON_INTERPRETER";

/// Environment override for the shim location.
pub const SHIM_ENV: &str = "CUSTOS_PYTHON_SHIM";

/// Locates the loader shim: the `CUSTOS_PYTHON_SHIM` override, then the
/// repository layout (via this crate's manifest, which also works from any
/// cwd inside the repo), then the bare relative path.
pub fn resolve_shim() -> Result<String, String> {
    if let Ok(path) = std::env::var(SHIM_ENV) {
        if std::path::Path::new(&path).is_file() {
            return Ok(path);
        }
        return Err(format!("{SHIM_ENV} set but `{path}` is not a file"));
    }
    let manifest_relative = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../",
        "plugins/python/custos_python_plugin.py"
    );
    if std::path::Path::new(manifest_relative).is_file() {
        return Ok(manifest_relative.to_owned());
    }
    if std::path::Path::new(PYTHON_SHIM_PATH).is_file() {
        return Ok(PYTHON_SHIM_PATH.to_owned());
    }
    Err(format!(
        "python loader shim not found; set {SHIM_ENV} to `{PYTHON_SHIM_PATH}`"
    ))
}

/// Builds the plugin config for one user script, or a concrete reason when
/// the script, shim or interpreter is unusable. The child gets a clean
/// environment (§4.4); scripts import only stdlib unless the deployment
/// pins a venv via `CUSTOS_PYTHON_INTERPRETER`.
pub fn script_plugin_config(script_path: &str) -> Result<PluginConfig, String> {
    if script_path.trim().is_empty() {
        return Err("script path must not be empty".into());
    }
    if !std::path::Path::new(script_path).is_file() {
        return Err(format!("script `{script_path}` does not exist"));
    }
    let shim = resolve_shim()?;
    let interpreter = std::env::var(INTERPRETER_ENV).unwrap_or_else(|_| "python3".into());
    let mut config = PluginConfig::new(interpreter);
    config.args = vec![shim, script_path.to_owned()];
    config.request_timeout = Duration::from_secs(10);
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_and_empty_scripts() {
        assert!(script_plugin_config("").is_err());
        let err = script_plugin_config("/definitely/not/here.py").unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn builds_config_with_shim_and_script() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("probe.py");
        std::fs::write(
            &script,
            "def probe(params):\n    return {'healthy': True, 'observation': 'ok'}\n",
        )
        .unwrap();
        let config = script_plugin_config(script.to_str().unwrap()).unwrap();
        assert_eq!(config.args.len(), 2);
        assert!(
            config.args[0].ends_with("custos_python_plugin.py"),
            "{}",
            config.args[0]
        );
        assert_eq!(config.args[1], script.to_str().unwrap());
        assert!(config.program.to_string_lossy().contains("python"));
    }
}

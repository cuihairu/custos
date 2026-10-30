//! Python binding tests: exercised through the GIL against real objects —
//! query handle over a real store, and one `run_once` trigger pass.

use pyo3::prelude::*;

fn write_config(path: &std::path::Path) {
    let pid_file = path.with_extension("pid");
    std::fs::write(
        path,
        format!(
            r#"
version = 1

[[resources]]
id = "app"
type = "process"

[resources.probe]
pid_file = "{}"

[resources.health_policy]
failure_threshold = 1
success_threshold = 1
"#,
            pid_file.display()
        ),
    )
    .unwrap();
    // No pid file: the probe fails on the first pass, deterministically.
}

#[test]
fn version_is_exposed() {
    Python::attach(|py| {
        let version = custos_python::version();
        assert!(version.starts_with("custos "), "{version}");
        let _ = py; // version needs no interpreter state
    });
}

#[test]
fn query_handle_reports_empty_fresh_database() {
    Python::attach(|py| {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("custos.db");
        let handle = custos_python::Custos::open(db.to_str().unwrap()).unwrap();
        let health = handle.health(py).unwrap();
        let health = health.bind(py);
        let len = health.len().unwrap();
        assert_eq!(len, 0, "fresh database has no health rows");
    });
}

#[test]
fn run_once_reports_unhealthy_and_persists_evidence() {
    Python::attach(|py| {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("custos.toml");
        let db = dir.path().join("custos.db");
        write_config(&config_path);

        let result =
            custos_python::run_once(py, config_path.to_str().unwrap(), db.to_str().unwrap())
                .unwrap();

        // Health after the pass: the missing pid file is unhealthy.
        let health = result.bind(py).get_item("health").unwrap();
        let first = health.get_item(0).unwrap();
        let resource = first.get_item("resource_id").unwrap();
        assert_eq!(
            resource.extract::<String>().unwrap(),
            "app",
            "health entry keys are plain dict fields"
        );
        let succeeded = first.get_item("succeeded").unwrap();
        assert!(!succeeded.extract::<bool>().unwrap());

        // The same evidence is persisted for later query handles.
        let handle = custos_python::Custos::open(db.to_str().unwrap()).unwrap();
        let rows = handle.health(py).unwrap();
        assert_eq!(rows.bind(py).len().unwrap(), 1);
    });
}

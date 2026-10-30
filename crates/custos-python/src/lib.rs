//! Python bindings for Custos (todo.md P1: Python 出向, ADR-0015): expose
//! queries (resources' latest health, recovery runs, incidents, audit,
//! notification queue) and one-shot probe/recovery triggering over the same
//! runtime assembly the CLI uses (`custos_core::assemble`).
//!
//! The bindings are read/trigger-only by design (ADR-0015): Python sees
//! tool calls over the assembled engine, never arbitrary execution. The
//! recovery engine keeps its authority — Python cannot bypass budgets,
//! mutexes or verification.
//!
//! Module name: `custos`. Built with maturin:
//!
//! ```bash
//! maturin build --release --features extension-module
//! ```

use custos_config::load;
use custos_store::Store;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use serde_json::Value;

/// Human-readable version of the Rust core.
#[pyfunction]
pub fn version() -> &'static str {
    concat!("custos ", env!("CARGO_PKG_VERSION"))
}

/// Converts a `serde_json::Value` into native Python objects.
fn to_python(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    Ok(match value {
        Value::Null => py.None(),
        Value::Bool(b) => pyo3::types::PyBool::new(py, *b)
            .to_owned()
            .unbind()
            .into_any(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.unbind().into_any()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.unbind().into_any()
            } else {
                n.as_f64()
                    .into_pyobject(py)
                    .map_err(|e| PyValueError::new_err(e.to_string()))?
                    .unbind()
                    .into_any()
            }
        }
        Value::String(s) => s.into_pyobject(py)?.unbind().into_any(),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(to_python(py, item)?)?;
            }
            list.into_any().unbind()
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, item) in map {
                dict.set_item(key, to_python(py, item)?)?;
            }
            dict.into_any().unbind()
        }
    })
}

fn rows_to_python(py: Python<'_>, rows: Vec<Value>) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for row in rows {
        list.append(to_python(py, &row)?)?;
    }
    Ok(list.into_any().unbind())
}

fn probe_row(row: &custos_store::ProbeRow) -> Value {
    serde_json::json!({
        "resource_id": row.resource_id,
        "probe_name": row.probe_name,
        "succeeded": row.succeeded,
        "observation": row.observation,
        "collected_at_unix_ms": row.collected_at_unix_ms,
    })
}

fn store_error(err: custos_store::StoreError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

/// A read handle over one Custos SQLite database: health, history and the
/// notification queue. Open the file the engine (`custos start`) writes.
#[pyclass]
pub struct Custos {
    store: Store,
}

#[pymethods]
impl Custos {
    /// Opens (creating if needed) the database file.
    #[new]
    pub fn open(path: &str) -> PyResult<Self> {
        let store = Store::open(path).map_err(store_error)?;
        Ok(Self { store })
    }

    /// Latest probe verdict per resource (the persisted health view).
    pub fn health(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let rows = self.store.latest_health().map_err(store_error)?;
        let values = rows.iter().map(probe_row).collect();
        rows_to_python(py, values)
    }

    /// Recent probe evidence, newest first.
    #[pyo3(signature = (limit = 50))]
    pub fn probes(&self, py: Python<'_>, limit: u32) -> PyResult<Py<PyAny>> {
        let rows = self
            .store
            .recent_probe_results(limit)
            .map_err(store_error)?;
        let values = rows.iter().map(probe_row).collect();
        rows_to_python(py, values)
    }

    /// Recent recovery runs, newest first.
    #[pyo3(signature = (limit = 20))]
    pub fn runs(&self, py: Python<'_>, limit: u32) -> PyResult<Py<PyAny>> {
        let rows = self.store.list_runs(limit).map_err(store_error)?;
        let values = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "run_id": r.id,
                    "resource_id": r.resource_id,
                    "status": r.status,
                    "updated_at_unix_ms": r.updated_at_unix_ms,
                })
            })
            .collect();
        rows_to_python(py, values)
    }

    /// Incident history, newest first.
    #[pyo3(signature = (limit = 20))]
    pub fn incidents(&self, py: Python<'_>, limit: u32) -> PyResult<Py<PyAny>> {
        let rows = self.store.list_incidents(limit).map_err(store_error)?;
        let values = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "incident_id": r.id,
                    "resource_id": r.resource_id,
                    "fault_code": r.fault_code,
                    "summary": r.summary,
                    "opened_at_unix_ms": r.opened_at_unix_ms,
                    "closed_at_unix_ms": r.closed_at_unix_ms,
                })
            })
            .collect();
        rows_to_python(py, values)
    }

    /// Recent audit events, newest first.
    #[pyo3(signature = (limit = 50))]
    pub fn audit(&self, py: Python<'_>, limit: u32) -> PyResult<Py<PyAny>> {
        let rows = self.store.recent_audit(limit).map_err(store_error)?;
        let values = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "at_unix_ms": r.at_unix_ms,
                    "event_type": r.event_type,
                    "severity": r.severity,
                    "resource_id": r.resource_id,
                    "run_id": r.run_id,
                })
            })
            .collect();
        rows_to_python(py, values)
    }

    /// Notification queue: five-state counters plus the newest entries.
    #[pyo3(signature = (limit = 20))]
    pub fn notifications(&self, py: Python<'_>, limit: u32) -> PyResult<Py<PyAny>> {
        let counts = self.store.notification_counts().map_err(store_error)?;
        let rows = self.store.list_notifications(limit).map_err(store_error)?;
        let value = serde_json::json!({
            "counts": {
                "pending": counts.pending,
                "sending": counts.sending,
                "delivered": counts.delivered,
                "retry_scheduled": counts.retry_scheduled,
                "dead_letter": counts.dead_letter,
                "total": counts.total(),
            },
            "entries": rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "channel": r.channel,
                        "event_type": r.event.event_type.as_str(),
                        "state": r.state.as_str(),
                        "attempts": r.attempts,
                        "last_error": r.last_error,
                        "next_attempt_at_unix_ms": r.next_attempt_at_unix_ms,
                    })
                })
                .collect::<Vec<_>>(),
        });
        to_python(py, &value)
    }
}

/// One trigger result: what one observe/recover pass produced.
fn run_once_summary(
    py: Python<'_>,
    store: &Store,
    runs: Vec<custos_model::RecoveryRun>,
    delivery: custos_notify::DeliverReport,
    queue: &custos_store::NotificationCounts,
) -> PyResult<Py<PyAny>> {
    let value = serde_json::json!({
        "runs": runs
            .iter()
            .map(|r| {
                serde_json::json!({
                    "run_id": r.id.as_str(),
                    "resource_id": r.resource_id.as_str(),
                    "status": r.status,
                })
            })
            .collect::<Vec<_>>(),
        "health": store
            .latest_health()
            .map_err(store_error)?
            .iter()
            .map(probe_row)
            .collect::<Vec<_>>(),
        "notifications": {
            "delivered": delivery.delivered,
            "retried": delivery.retried,
            "dead_letter": delivery.dead,
            "queue": {
                "pending": queue.pending,
                "retry_scheduled": queue.retry_scheduled,
                "dead_letter": queue.dead_letter,
                "total": queue.total(),
            },
        },
    });
    to_python(py, &value)
}

/// Triggers one probe → diagnose → recover pass with the notification
/// delivery, exactly like `custos start --once`. Returns the runs finished,
/// the per-resource health after the pass, and the notification queue
/// counters. This goes through the real engine: budgets, mutexes,
/// dependency ordering and verification are all enforced (ADR-0005).
#[pyfunction]
pub fn run_once(py: Python<'_>, config_path: &str, db_path: &str) -> PyResult<Py<PyAny>> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| PyValueError::new_err(format!("cannot read {config_path}: {e}")))?;
    let config = load(&text).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let assembled = custos_core::assemble(&config, db_path).map_err(PyValueError::new_err)?;
    let mut engine = assembled.engine;
    engine.resume_unfinished();
    let runs = engine.tick();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let delivery = assembled.notification.deliver_due(now, 32);
    let queue = assembled.store.notification_counts().map_err(store_error)?;
    run_once_summary(py, &assembled.store, runs, delivery, &queue)
}

#[pymodule]
fn custos(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Custos>()?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(run_once, m)?)?;
    Ok(())
}

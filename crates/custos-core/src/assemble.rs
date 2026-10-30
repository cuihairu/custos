//! Runtime assembly (docs/guide/workspace.md: `custos-core`) — the one
//! place that wires config → probes → engine → store → notification engine.
//! The CLI loop and the Python bindings build the SAME system through it.

use custos_config::ValidatedConfig;
use custos_engine::{BuiltinExecExecutor, Engine};
use custos_notify::NotificationEngine;
use custos_probe::Probe;
use custos_store::Store;
use custos_model::ids::ResourceId;
use std::collections::HashMap;
use std::sync::Arc;

/// One assembled runtime: the engine plus its notification sink.
pub struct Assembled {
    pub engine: Engine,
    pub store: Arc<Store>,
    pub notification: Arc<NotificationEngine<Arc<Store>>>,
}

/// Wires the notification sink into the engine: dispatch enqueues to the
/// queue, never blocking recovery (§8.1).
fn attach_notifications(
    engine: &mut Engine,
    notification: Arc<NotificationEngine<Arc<Store>>>,
) {
    let sink = notification.clone();
    engine.set_event_emitter(Arc::new(move |event| {
        let report = sink.dispatch(event, event.occurred_at_unix_ms);
        if report.enqueued > 0 || report.suppressed > 0 {
            tracing::debug!(
                target: "custos::runtime",
                event = event.event_type.as_str(),
                enqueued = report.enqueued,
                suppressed = report.suppressed,
                "notification dispatched"
            );
        }
    }));
}

/// Builds builtin probes per resource; a resource type without a builtin
/// probe is a startup error, not a runtime surprise (user ruling
/// 2026-10-29: builtin base adapters).
pub fn build_probes(
    config: &ValidatedConfig,
) -> Result<HashMap<ResourceId, Box<dyn Probe>>, String> {
    let mut probes = HashMap::new();
    for resource in &config.resources {
        match custos_probe::factory::build_probe(resource) {
            Ok(probe) => {
                probes.insert(resource.id.clone(), probe);
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(probes)
}

/// Full assembly over a shared SQLite store.
pub fn assemble(config: &ValidatedConfig, db_path: &str) -> Result<Assembled, String> {
    let probes = build_probes(config)?;
    let store = Arc::new(Store::open(db_path).map_err(|e| e.to_string())?);
    let notification = Arc::new(NotificationEngine::new(
        config.notification.clone(),
        store.clone(),
    ));
    let mut engine = Engine::new(
        config,
        probes,
        Box::new(BuiltinExecExecutor),
        store.clone(),
        Arc::new(custos_engine::SystemClock),
    );
    attach_notifications(&mut engine, notification.clone());
    Ok(Assembled {
        engine,
        store,
        notification,
    })
}

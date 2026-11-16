//! Runtime assembly (docs/guide/workspace.md: `custos-core`) — the one
//! place that wires config → probes → engine → store → notification engine.
//! The CLI loop and the Python bindings build the SAME system through it.

use custos_config::ValidatedConfig;
use custos_engine::{Engine, PluginRoutingExecutor};
use custos_model::ids::ResourceId;
use custos_notify::NotificationEngine;
use custos_plugin_host::{PluginConfig, PluginProbe, PluginSupervisor, RestartPolicy};
use custos_probe::Probe;
use custos_store::Store;
use std::collections::HashMap;
use std::sync::Arc;

/// One assembled runtime: the engine plus its notification sink.
pub struct Assembled {
    pub engine: Engine,
    pub store: Arc<Store>,
    pub notification: Arc<NotificationEngine<Arc<Store>>>,
    /// Supervised plugin processes (docs/guide/plugins.md §5.4). Held so
    /// they shut down with the runtime instead of leaking orphaned children.
    plugins: Vec<Arc<PluginSupervisor>>,
}

impl Drop for Assembled {
    fn drop(&mut self) {
        // Graceful shutdown is idempotent; a plugin that ignores it is
        // reaped by the session's bounded kill (§4.5, §5.4).
        for plugin in &self.plugins {
            plugin.shutdown();
        }
    }
}

impl Assembled {
    /// Mutable engine access. Field moves out of a `Drop` type are not
    /// possible, so consumers borrow the engine and clone the `Arc`s
    /// ([`Assembled::store`], [`Assembled::notification`]) first.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Cloned shared store (cheap `Arc` clone, no move out of `self`).
    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    /// Cloned shared notification engine (cheap `Arc` clone, no move).
    pub fn notification(&self) -> Arc<NotificationEngine<Arc<Store>>> {
        Arc::clone(&self.notification)
    }
}

/// Spool capacity: pending events the dispatch worker can absorb before
/// the emitter degrades to inline dispatch (backpressure, no loss).
const DISPATCH_SPOOL_CAPACITY: usize = 256;

/// Wires the notification sink into the engine: dispatch enqueues to the
/// queue, never blocking recovery (§8.1). The emitter runs on the tick
/// thread and dispatch does synchronous SQLite inserts, so events are
/// spooled to a dedicated worker instead of serializing fsyncs on the one
/// tick (C18).
fn attach_notifications(engine: &mut Engine, notification: Arc<NotificationEngine<Arc<Store>>>) {
    let sink = Arc::new(custos_notify::SpooledDispatch::spawn(
        notification,
        DISPATCH_SPOOL_CAPACITY,
    ));
    engine.set_event_emitter(Arc::new(move |event| {
        sink.dispatch(event);
    }));
}

/// One supervised plugin per declared `[[plugins]]` entry. Spawning is
/// lazy — a supervisor starts its process on first probe or action call.
pub fn build_plugin_supervisors(
    config: &ValidatedConfig,
) -> HashMap<String, Arc<PluginSupervisor>> {
    config
        .plugins
        .iter()
        .map(|plugin| {
            let mut plugin_config = PluginConfig::new(plugin.program.clone());
            plugin_config.args = plugin.args.clone();
            plugin_config.env = plugin.env.clone();
            plugin_config.request_timeout = plugin.request_timeout;
            (
                plugin.name.clone(),
                Arc::new(PluginSupervisor::new(
                    plugin.name.clone(),
                    plugin_config,
                    RestartPolicy::default(),
                )),
            )
        })
        .collect()
}

/// Builds probes per resource: plugin-backed resources get a
/// [`PluginProbe`] over their supervisor, everything else a builtin probe.
/// A resource type without a builtin probe is a startup error, not a
/// runtime surprise (user ruling 2026-10-29: builtin base adapters).
pub fn build_probes(
    config: &ValidatedConfig,
    supervisors: &HashMap<String, Arc<PluginSupervisor>>,
) -> Result<HashMap<ResourceId, Box<dyn Probe>>, String> {
    let mut probes = HashMap::new();
    for resource in &config.resources {
        let probe: Box<dyn Probe> = match &resource.plugin_ref {
            Some(plugin) => {
                let supervisor = supervisors.get(plugin).ok_or_else(|| {
                    format!(
                        "resource `{}` references undeclared plugin `{plugin}`",
                        resource.id
                    )
                })?;
                // The operation id is validated at config load; the default
                // matches the single-probe convention (docs/guide/plugins.md §5.2).
                let operation = resource
                    .config
                    .get("probe_operation")
                    .and_then(|value| value.as_str())
                    .unwrap_or("probe");
                Box::new(PluginProbe::new(supervisor.clone(), operation, "plugin"))
            }
            None => custos_probe::factory::build_probe(resource).map_err(|err| err.to_string())?,
        };
        probes.insert(resource.id.clone(), probe);
    }
    Ok(probes)
}

/// Full assembly over a shared SQLite store.
pub fn assemble(config: &ValidatedConfig, db_path: &str) -> Result<Assembled, String> {
    let supervisors = build_plugin_supervisors(config);
    let probes = build_probes(config, &supervisors)?;
    let store = Arc::new(Store::open(db_path).map_err(|e| e.to_string())?);
    let notification = Arc::new(NotificationEngine::new(
        config.notification.clone(),
        store.clone(),
    ));
    let mut engine = Engine::new(
        config,
        probes,
        Box::new(PluginRoutingExecutor::new(supervisors.clone())),
        store.clone(),
        Arc::new(custos_engine::SystemClock),
    );
    attach_notifications(&mut engine, notification.clone());
    Ok(Assembled {
        engine,
        store,
        notification,
        plugins: supervisors.into_values().collect(),
    })
}

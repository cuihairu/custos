---
title: Source Architecture
---

# Source architecture

This is the implementation map behind Custos. Layers are responsibilities: a layer may span several crates, and a crate may expose more than one layer.

## 1. Configuration and domain contract layer

**Responsibility:** turn operator input into validated domain objects and shared vocabulary before any runtime side effect.

- **Implementation:** `crates/custos-config/src/lib.rs` parses strict TOML, validates references, alternatives, recovery plans and permissions, then builds `ValidatedConfig`, `Resource`, `Dependency` and `DependencyGraph` (`crates/custos-config/src/build.rs:42`).
- **Domain model:** resource/dependency/recovery/event types live in `crates/custos-model`; the graph owns cycle detection and topological order (`crates/custos-model/src/graph.rs:35`).
- **Configuration schema:** the top-level unknown-field boundary is `RawConfig` (`crates/custos-config/src/schema.rs:254`); all probe fields are declared in `RawProbe` (`crates/custos-config/src/schema.rs:17`).
- **Protocol files:** none; this layer uses TOML plus serializable domain structs.
- **Copy attachment:** `docs/guide/resources.md`, `docs/guide/dashboard-cli-config.md`, and `docs/guide/overview.md` §2.

## 2. Coordination and recovery-state layer

**Responsibility:** own observation scheduling, dependency ordering, failure diagnosis, recovery state transitions, budgets and final health verdicts.

- **Implementation:** `Engine` owns managed resources, topological probe order, active runs, waiters and the event sink (`crates/custos-engine/src/engine/mod.rs:49`); `tick` probes due resources in dependency order (`crates/custos-engine/src/engine/orchestrate.rs:9`); recovery starts at `try_recover` (`crates/custos-engine/src/engine/recover.rs:8`).
- **State:** in-process `ManagedResource`, snapshots, active-run set and waiters; durable run state is delegated through `RunStore` (`crates/custos-engine/src/events.rs:130`).
- **Budgets:** start, duration, restart-rate and high-risk limits are enforced before work is recorded (`crates/custos-engine/src/budget.rs:59`).
- **Configuration schema:** this layer consumes the validated graph and per-resource recovery policy; there is no separate protocol file.
- **Copy attachment:** `docs/guide/recovery-engine.md` and `docs/guide/security.md`.

## 3. Capability discovery layer

**Responsibility:** normalize observations without deciding recovery, and give native and external capabilities one auditable permission boundary.

- **Native probes:** `Probe::probe` returns a normalized report; it never chooses an action (`crates/custos-probe/src/lib.rs:70`). `custos-core::build_probes` turns every configured resource into a builtin probe at startup (`crates/custos-core/src/assemble.rs:42`).
- **Protocol:** native probes have no wire protocol; each probe reads its typed slice of `Resource.config`.
- **Plugin protocol:** newline-framed JSON-RPC methods and capability declarations are defined in `crates/custos-plugin-api/src/rpc.rs:23` and `crates/custos-plugin-api/src/capability.rs:38`. The host session frames both pipe directions with output caps and deadlines (`crates/custos-plugin-host/src/session.rs:101`); example plugins call `serve` from `crates/custos-plugin-api/src/server.rs:32`.
- **Permission/dispatch:** builtin and plugin operations meet at `ToolRegistry`; permission is checked before a builtin tool or plugin is reached (`crates/custos-toolbox/src/registry.rs:181`).
- **Configuration schema:** plugin executables and resource-specific probe parameters arrive through the main TOML contract; external controller settings are read by the example plugins themselves (`plugins/mihomo/src/main.rs:28`).
- **Copy attachment:** `docs/guide/plugins.md`, `docs/guide/probe-survey.md`, `docs/guide/security.md`, and `docs/adr/ADR-0003-why-jsonrpc-over-stdio.md`.

## 4. Execution and platform layer

**Responsibility:** perform selected local actions through controlled OS primitives while preserving capability metadata and verification boundaries.

- **Recovery boundary:** `ActionExecutor` exposes execute and verify; the builtin executor runs the configured argv and verification is a probe, not an exit code (`crates/custos-engine/src/executor.rs:10`).
- **OS interfaces:** process, service, network-interface, connectivity, credential and reboot contracts are declared in `crates/custos-platform/src/controller.rs:16` through `crates/custos-platform/src/controller.rs:104`; adapters implement systemd/Windows SCM/netsh/reboot paths (`crates/custos-platform/src/linux.rs:142`, `crates/custos-platform/src/windows.rs:165`).
- **Capability gate:** each adapter reports what it supports; unsupported operations are explicit errors (`crates/custos-platform/src/capability.rs:19`).
- **Configuration schema:** platform operations have no TOML schema of their own; operation IDs and risk/permission semantics are supplied by tools and recovery plans.
- **Copy attachment:** `docs/guide/platforms.md` and `docs/adr/ADR-0012-why-unauthorized-high-risk-network-actions-are-forbidden.md`.

## 5. Persistence and event layer

**Responsibility:** preserve evidence and run state before, during and after recovery; expose the same durable queue to observers.

- **Engine port:** `RunStore` persists runs, attempts, verifications, evidence, diagnoses, incidents, steps, events and unfinished runs (`crates/custos-engine/src/events.rs:130`).
- **SQLite implementation:** `Store::open` establishes the shared database (`crates/custos-store/src/store/lifecycle.rs:8`) and implements both `RunStore` and `NotificationQueue` (`crates/custos-store/src/store/runs.rs:5`, `crates/custos-store/src/migrations.rs:180`).
- **Schema:** numbered migrations create the durable tables (`crates/custos-store/src/migrations.rs:61`); migration application itself is recorded at `crates/custos-store/src/migrations.rs:249`.
- **State:** file-backed SQLite only. There is no Redis, MySQL or daemon-wide cache state in this repository.
- **Copy attachment:** `docs/guide/persistence.md` and `docs/adr/ADR-0007-why-sqlite.md`.

## 6. Notification delivery layer

**Responsibility:** decouple user-visible reporting from recovery, keep outages from blocking self-healing, and make retries auditable.

- **Implementation:** the engine matches rules, suppresses repeats and enqueues (`crates/custos-notify/src/engine.rs:203`); a background worker claims due entries and sends them (`crates/custos-notify/src/engine.rs:293`).
- **Channels:** webhook, SMTP, Herald, Telegram and local log implement the `Channel` port (`crates/custos-notify/src/channel.rs:49`); webhook egress additionally has an SSRF policy boundary (`crates/custos-config/src/notification/mod.rs:114`).
- **State:** channel settings are memory-resident validated config; queue and attempts are SQLite state.
- **Configuration schema:** channel discriminated tables and rules are in `crates/custos-config/src/notification/mod.rs:54` and `crates/custos-config/src/notification/mod.rs:33`; secrets use a redacted wrapper (`crates/custos-config/src/notification/mod.rs:236`).
- **Copy attachment:** `docs/guide/notification.md` and `docs/adr/ADR-0009-why-notification-decoupled.md`.

## 7. Runtime assembly and process lifecycle layer

**Responsibility:** compose all layers once, then run/supervise processes with crash-safe semantics.

- **Assembly:** `assemble` is the single construction point for probes, SQLite, notifications, engine, builtin executor and clock (`crates/custos-core/src/assemble.rs:59`); CLI startup resumes unfinished runs and starts the delivery worker (`crates/custos-cli/src/main.rs:221`, `crates/custos-cli/src/main.rs:243`).
- **Supervision:** plugin restart budget, handshake and degraded status are isolated in `PluginSupervisor` (`crates/custos-plugin-host/src/supervisor.rs:66`).
- **State:** engine and supervisor in-memory state is bounded and process-local; durable crash state is SQLite. The process owns both workers; there is no Redis lease or MySQL coordinator.
- **Configuration schema:** CLI options are parsed at `crates/custos-cli/src/main.rs:128`; store policy is typed in `crates/custos-store/src/config.rs:21`.
- **Copy attachment:** `docs/guide/workspace.md` and `docs/adr/ADR-0013-why-unknown-outcomes-are-never-blindly-retried.md`.

## 8. Interface and observer layer

**Responsibility:** expose read/introspection/control surfaces without allowing a client to bypass coordination, permissions or durable state.

- **CLI:** the command surface validates, introspects, starts, inspects runs/incidents/resources and graphs (`crates/custos-cli/src/main.rs:50`); the main loop owns tick cadence and notification worker lifecycle (`crates/custos-cli/src/main.rs:273`).
- **Dashboard:** optional, disabled by default, loopback-only and read-only (`crates/custos-dashboard/src/lib.rs:26`, route table at `crates/custos-dashboard/src/lib.rs:180`).
- **Tray:** a separate process reads the same SQLite file and renders system menu state (`crates/custos-tray/src/main.rs:25`).
- **Python binding:** embedders call `run_once`, which goes through the same `assemble` path (`crates/custos-python/src/lib.rs:259`).
- **Configuration schema:** dashboard fields live in `crates/custos-config/src/dashboard.rs`; the dashboard itself has no write API.
- **Copy attachment:** `docs/guide/dashboard-cli-config.md`, `docs/adr/ADR-0010-why-dashboard-off-by-default.md`, and the homepage architecture diagram.

## Why this split

1. **Reject invalid topology before execution.** Configuration parsing constructs and validates the dependency graph at startup, preventing unknown resources and cycles from becoming recovery-time recursion (`crates/custos-model/src/graph.rs:59`; `crates/custos-config/src/build.rs:42`).
2. **Keep capability providers out of control flow.** Probes return observations only (`crates/custos-probe/src/lib.rs:70`), while the engine owns dependency resolution, planning, execution and final health verdicts (`crates/custos-engine/src/engine/orchestrate.rs:9`; `crates/custos-engine/src/engine/recover.rs:8`).
3. **Normalize native and external capabilities at one permission boundary.** Builtin tools and plugin operations are registered in one table, and denial happens before either implementation runs (`crates/custos-toolbox/src/registry.rs:181`), so plugin subprocesses cannot acquire an unreviewed control path.
4. **Make OS execution replaceable without making it policy-bearing.** Recovery delegates execution through `ActionExecutor` (`crates/custos-engine/src/executor.rs:10`), while concrete OS capabilities are declared and implemented behind platform contracts (`crates/custos-platform/src/controller.rs:41`; `crates/custos-platform/src/capability.rs:19`).
5. **Persist state before and after risky transitions.** The engine does not depend on SQL directly; it writes through `RunStore` (`crates/custos-engine/src/events.rs:130`), and SQLite implements durable runs plus the notification queue in one file (`crates/custos-store/src/store/runs.rs:5`; `crates/custos-store/src/migrations.rs:180`).
6. **Keep reporting failure out of the recovery path.** Recovery emits events to a non-blocking notification sink (`crates/custos-core/src/assemble.rs:30`); notification rules enqueue (`crates/custos-notify/src/engine.rs:203`), and a separate worker delivers later (`crates/custos-notify/src/engine.rs:293`).
7. **Isolate crash blast radius.** A plugin process death is absorbed by a bounded restart/degraded supervisor (`crates/custos-plugin-host/src/supervisor.rs:66`), while an interrupted engine run is marked unknown and re-driven only by explicit resume logic (`crates/custos-engine/src/engine/orchestrate.rs:224`).
8. **Preserve observability when management surfaces fail.** The engine loop remains independent of optional CLI/dashboard/tray/Python callers; dashboard is bound only when enabled (`crates/custos-cli/src/main.rs:199`) and the tray reads a file rather than owning engine state (`crates/custos-tray/src/main.rs:25`).

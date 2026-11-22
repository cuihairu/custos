---
title: Dashboard, CLI and Configuration
---

# 9. Web Dashboard and CLI

## 9.1 Dashboard disabled by default

The Dashboard is an optional component and is disabled by default; when not enabled there are zero listening ports and zero HTTP code paths.

Implemented and disabled by default (`enabled` defaults to false, `crates/custos-config/src/dashboard.rs:12-19`); the listener is bound only when enabled, at startup (`crates/custos-cli/src/main.rs:199-200`):

```toml
[dashboard]
enabled = false
listen = "127.0.0.1:8080"
```

Enabling prerequisites and acceptance constraints:

1. When disabled, no port may be listened on.
2. Dashboard unavailability must not affect the core engine.
3. The Dashboard must not bypass the Core Engine to execute recovery directly.
4. High-risk operations must still go through the unified authorization and audit.
5. Transport security is the open end: the endpoints are read-only and carry no authentication and no CSRF token, so the only current protection is the loopback bind refusal — a non-loopback `listen` address is rejected outright (`crates/custos-dashboard/src/lib.rs:39-53`). Remote access is expected to go through a local reverse proxy that supplies its own authentication. Auth and CSRF hardening with the toolbox (ADR-0015) is still pending.

## 9.2 CLI commands

The CLI is the P0/P1 operating surface (implemented):

```text
custos validate <CONFIG>           Parse and validate the configuration (rejects duplicate IDs, dangling references, cycles)
custos init <TYPE> --id <ID> [--out PATH] [--force]
                                   Scaffold a complete configuration template for a resource type
                                   (placeholder values; edit before use, then validate)
custos start <CONFIG> [OPTIONS]    Run the engine loop (--db PATH, --interval MS, --once, --strict-config)
custos status [--db PATH] [--json]     Store counters and recent audit events
custos runs [--db PATH] [--json]       Recent recovery runs (newest first)
custos incidents [--db PATH] [--json]  Incident history
custos notify status [--db PATH]   Notification queue counters (five states) and recent entries
custos notify retry <ID> [--db PATH]  Manually requeue a dead-lettered notification
custos resources <CONFIG>          List configured resources
custos graph <CONFIG>              Dependency graph in topological order
```

`start` performs crash recovery first (unfinished runs are marked outcome-unknown and never blindly retried), then enters the probe/recover tick loop; the notification engine is wired into the same process (event enqueue + background delivery, offline drain).

**Config last-known-good fallback** (2026-11-22 user rule: 验证不过不可以切换重启): every successful `start` atomically records the validated config text to a sibling `<CONFIG>.last-good` snapshot. When the live file fails validation at startup and a usable snapshot exists, the engine **continues running under the previous config with a loud warning** instead of dying — a broken edit must never take a supervised machine from "watched" to "unwatched, no signal". The snapshot is written only from a clean load, so a broken file can never poison it. `--strict-config` restores fail-fast (no fallback). `custos validate` stays a pure checker and never touches the snapshot.

`--json` on the three read-only history commands (`status` / `runs` / `incidents`) emits the same rows as a single machine-readable JSON document — it is the data source of the LuCI status dashboard.

---

# 10. Configuration Specification

## 10.1 Basic principles

Configuration must be:

- Validatable.
- Versionable.
- Able to report clear error paths.
- Unwilling to silently interpret unknown fields as valid configuration.
- Supportive of secure credential references.
- Supportive of explicit dependencies between resources.
- Supportive of per-resource recovery policies.
- Supportive of notification rules. Global recovery budgets are engine-internal defaults in v1, not configuration keys (`crates/custos-engine/src/budget.rs:9-25`).

The first version uses TOML (one primary format, `version = 1`).

## 10.2 Example: a resource wired to a plugin

The target shape below is now the implemented TOML schema. Plugin processes are declared under `[[plugins]]`; a resource references one with `plugin` (+ optional `probe_operation`), and recovery plans dispatch plugin operations with `plugin` + `operation_id` in the plan's action. The YAML form below is illustrative only — the canonical, validated form is TOML; see [docs/guide/plugins.md](plugins.md) and `examples/openclash-failover.toml` in the repo root for the working example.

```toml
version = 1

[[plugins]]
name = "mihomo"
binary = "/usr/local/bin/mihomo-plugin"
request_timeout_ms = 10000

[plugins.env]
MIHOMO_API = "http://127.0.0.1:9090"
MIHOMO_GROUP = "CHANGE_ME"

[[resources]]
id = "proxy.egress"
type = "mihomo"
plugin = "mihomo"
probe_operation = "mihomo.probe"
```

Note that:

1. Proxy health checks distinguish proxy authentication failures (407 → `proxy_authentication_failed`), connection failures (`proxy_connection_failed`) and target service failures (`remote_endpoint_unreachable`) — the builtin proxy probe classifies them, and plan ladders select by these codes.
2. If the proxy is not a required dependency of the Tunnel, it must not be declared Required.
3. Plugin wiring is validated at load: unknown plugin names, `probe_operation` without `plugin`, and action forms mixing argv with plugin operation are startup errors, never runtime surprises.
4. Recovery actions must match the plugin's declared capabilities.
5. For node switching, plugins share one client (`plugins/clash-api`) whose selection policy filters subscription info nodes — members like 剩余流量/套餐到期 carry healthy delays but are not proxies.

## 10.3 Configuration loading process

1. Parse the configuration.
2. Validate the version.
3. Validate the structure.
4. Validate resource IDs.
5. Validate dependency references.
6. Validate the dependency graph.
7. Validate recovery action references.
8. Validate plugin capabilities. (Planned: plugins are not wired into configuration yet — the loader performs no capability check; `plugin_ref` is always unset, `crates/custos-config/src/build.rs:356`.)
9. Validate permission and risk policies.
10. Validate notification channel configuration.
11. Validate database migration compatibility. (Planned: migrations run at store open, `crates/custos-store/src/store/lifecycle.rs:39`; the config loader performs no migration check.)
12. Create or update the runtime resource registry.

When configuration validation fails, the specific field and reason must be reported.

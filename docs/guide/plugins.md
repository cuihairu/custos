---
title: Plugin Architecture
---

# 4. Plugin Architecture

## 4.1 Plugin responsibilities

Plugins encapsulate specific resources or capabilities; they do not implement an independent self-healing system of their own.

A plugin may provide:

- Resource Provider.
- Probe Provider.
- Diagnosis Provider.
- Recovery Action Provider.
- Verification Provider.
- Notification Channel Provider.

One plugin may provide multiple capabilities.

For example, the OpenClash plugin could provide:

- The OpenClash service resource.
- Proxy port probing.
- Proxy authentication verification.
- Node state diagnosis.
- Node switching actions.
- Service restart actions.
- Node recovery verification.

The Core Engine decides when these capabilities are invoked and whether execution is allowed.

## 4.2 Plugin execution model

The first version uses:

**Rust Core + external executable plugins + JSON-RPC 2.0 over stdio.**

Advantages:

- A plugin crash does not directly crash the core process.
- Plugins can be implemented in different languages.
- No need to design a stable Rust dynamic library ABI right away.
- The plugin protocol can be versioned independently.
- Easier to isolate plugin lifecycles and enforce timeouts.

But be clear about this: an external process does not automatically constitute a security sandbox. Plugins still require least privilege, controlled working directories, controlled environment variables and explicit operation authorization.

## 4.3 Plugin protocol

The first version needs at least:

```text
initialize
describe_capabilities
probe
diagnose
list_recovery_actions
execute_recovery_action
verify
shutdown
```

These are logical operation names; they should ultimately map to versioned JSON-RPC methods.

A request must carry:

- A JSON-RPC ID.
- The protocol version.
- The method name.
- Parameters.
- A request timeout.
- The associated Resource ID.
- The associated RecoveryRun ID, when applicable.

Recovery operations must use explicit action IDs with parameterized inputs. Piping arbitrary user input directly into Shell commands is forbidden.

## 4.4 Engineering requirements for stdio

On Windows, subprocess stdio pipes can block when their buffers fill up. The implementation must:

- Read stdout and stderr continuously and independently.
- Use stdout only for protocol messages.
- Use stderr for plugin diagnostic logs.
- Limit single-message size.
- Limit total buffer size.
- Support chunked reading.
- Handle incomplete messages.
- Handle plugins exiting early.
- Handle unresponsive plugins and timeouts.
- Support request cancellation.
- Return clear errors for malformed JSON and unknown methods.
- Never parse stderr as a protocol response.

For the first version, it is recommended to cap a single protocol message at 1 MiB, treat the exact value as a configurable safety limit, and handle large evidence via digest, truncation or chunking.

## 4.5 Plugin lifecycle

Long-lived plugin processes are recommended:

1. Custos starts.
2. Configured plugins are started.
3. Protocol handshake completes.
4. Capability declarations are obtained.
5. Protocol compatibility is verified.
6. Requests are accepted.
7. Plugin health is monitored.
8. On plugin crash, restart according to policy.
9. If a plugin cannot be recovered, mark related resources as not effectively probeable or operable.
10. Record the plugin failure and notify.

Plugins should not pay excessive overhead by re-creating a process for every probe.

## 4.6 Capability permissions

Every capability should declare:

- Read-only or state-modifying.
- Required permissions.
- The scope of resources affected.
- Whether it can interrupt the network.
- Whether it can interrupt remote management.
- Whether cancellation is supported.
- Whether it is idempotent.
- Whether verification is supported.
- Possible failure states.
- Whether compensation or rollback exists.

The Core Engine decides whether to execute based on configuration and policy.

A plugin declaring an action does not automatically make it allowed.

---

## Plugin model ruling (user decision, 2026-10-29)

- **One plugin = the complete monitoring and handling logic for one resource type**: probing, diagnosis, recovery actions and the verification loop live inside the same plugin, with no half-finished splits.
- **Plugins are composed through the resource dependency graph**: dependencies link resources, not plugins directly (`Dependency` carries a `ResourceId`, `crates/custos-model/src/dependency.rs:56-69`); a plugin rides along with its resource into the graph, and the Core Engine resolves it uniformly.
- **Consumers enable a plugin with simple configuration only**: declare the resource type and parameters, no glue code; configuration loading follows the configuration specification in §10. Implementation status: the schema has no `plugin` key yet and runtime assembly builds builtin probes only (`crates/custos-core/src/assemble.rs:43-56`); the shipped plugin binaries (mihomo, cloudflared, openclash) speak the protocol but are reachable only through their protocol tests for now.

## Builtin capabilities and Python script extension (user decision, 2026-10-29)

- **Builtin capabilities**: the core engine ships built-in basic adapters for common resources (services, processes, disks, network connectivity, etc.) that work out of the box — basic scenarios require no plugin development at all.
- **Python script extension (bidirectional)**:
  - Outbound: core functionality can be called from Python scripts via PyO3 bindings;
  - Inbound: custom probes and recovery actions can be written as Python scripts, loaded and executed by the core according to the plugin contract (timeouts, permissions and budgets follow the same rules as plugins).

## 5. Configuration wiring (implemented 2026-11-15)

The architecture sections above describe the contract; this section documents the configuration keys that wire plugins into a deployment — all implemented and tested.

### 5.1 Declaring plugin processes (`[[plugins]]`)

```toml
[[plugins]]
name = "openclash"                      # registry name, referenced below
binary = "/usr/local/bin/openclash-plugin"
request_timeout_ms = 10000              # per-RPC deadline (default 30000)

[plugins.env]                           # clean env + exactly these entries
OPENCLASH_API = "http://127.0.0.1:9090"
OPENCLASH_SECRET = "CHANGE_ME"
OPENCLASH_GROUP = "CHANGE_ME"
```

Validation (config load is the gate): names are unique and non-empty, the binary path is non-empty, `request_timeout_ms` >= 1, unknown fields are errors (§10.1 of dashboard-cli-config).

### 5.2 Plugin-served probes (`plugin` + `probe_operation`)

A resource that declares `plugin` is probed by that plugin's `probe_provider` capability instead of a builtin probe:

```toml
[[resources]]
id = "openclash"
type = "openclash"
plugin = "openclash"                    # must name a [[plugins]] entry
probe_operation = "openclash.probe"     # default: "probe"
```

`probe_operation` without `plugin` is a validation error. At runtime a degraded supervisor (crashed or unspawnable plugin) surfaces as `Unreachable`, which `diagnose()` maps to `ProbeTimeout` — a management-plane fault — while an unhealthy verdict from a healthy plugin is the plugin's judgment about its backend (`UnknownFailure`).

### 5.3 Recovery actions (`plugin` + `operation_id`)

An action is exactly one of two forms — argv XOR plugin operation; mixing is a validation error:

```toml
[resources.recovery_policy.plans.action]
plugin = "openclash"
operation_id = "openclash.switch_best_node"
```

```toml
[resources.recovery_policy.plans.action]
program = "/etc/init.d/openclash"
args = ["restart"]
```

The plugin name and operation id are validated at load (unknown plugin, empty operation id -> startup error, never a runtime surprise).

### 5.4 Process model: clean environment, budgets, shutdown

- **Lazy spawn**: a plugin process starts on its first probe or action call, not at configuration load.
- **Clean environment**: the child never inherits the host environment; it gets the declared `[plugins.env]` entries only. Credentials reach plugins explicitly (§5.1) instead of through inheritance.
- **Per-request budget**: `request_timeout_ms` bounds every RPC; the watchdog kills a wedged plugin and the supervisor restarts it per its restart policy (§4.5).
- **Shutdown**: the runtime assembly (`Assembled`, `crates/custos-core/src/assemble.rs`) holds every supervisor and shuts plugins down on drop — no orphaned children.

### 5.5 Dispatch: routing and outcome mapping

`PluginRoutingExecutor` (`crates/custos-engine/src/executor.rs`) routes by action form: plugin operations go to the named supervisor (§4.3 protocol), argv actions to the builtin exec executor. RPC results map to action outcomes:

| Plugin reply | Action outcome | Recovery semantics |
|---|---|---|
| `succeeded: true` | `Succeeded` | plan continues to verification |
| `succeeded: false` | `Failed` | plugin's verdict about the backend — retryable in the ladder |
| supervisor `Unavailable` | `OutcomeUnknown` | unknowable whether it ran — never blindly repeated (§6.6) |
| RPC timeout (`-32001`) | `TimedOut` | counts as a spent attempt |
| `-32601` unknown operation / malformed reply | `NotStarted` | wiring bug — permanent for the plan (C21) |

### 5.6 The shared clash/mihomo client (`plugins/clash-api`)

OpenClash runs a clash core on OpenWrt; its external controller is the same REST surface as mihomo's. Both shipped proxy plugins build on one client crate that owns:

- the REST calls (`/version`, `/proxies/{group}`, `/group/{group}/delay`, `PUT /proxies/{group}`) with bearer auth and a 5 s client budget;
- percent-encoding of group names (`♻️ 自动选择`, `三毛机场`);
- **node-selection policy**: `is_selectable_node` / `real_candidates` / `best_by_delay`. Subscription providers inject info members — `剩余流量：533.08 GB`, `套餐到期：2027-01-16`, `官网 …` — that carry healthy delay numbers but are not proxies; selecting one breaks all traffic (2026-11-15 real-scenario fix, verified live: the info members measured 150 ms while real nodes measured 51–106 ms). Every picker in every plugin goes through this filter, so `DIRECT`/`REJECT` and info nodes are never switch targets.

### 5.7 Worked example

`examples/openclash-failover.toml` (validates with `custos validate`): `wan` (detection only — a dead line must not churn proxy nodes), `openclash` (plugin-served probe + high-risk restart plan behind `allow_high_risk`), `google` / `github` (builtin proxy probes through the mixed port, plans dispatching `openclash.switch_best_node`; proxy authentication failures deliberately have no plan — switching nodes cannot fix wrong credentials).

---
title: Dashboard, CLI and Configuration
---

# 9. Web Dashboard and CLI

## 9.1 Dashboard disabled by default

Default configuration:

```toml
[dashboard]
enabled = false
listen = \

rules:
    - event: recovery.exhausted
      severity: critical
      channels:
        - primary-webhook
```

Note that:

1. Proxy health checks should distinguish proxy authentication failures, connection failures and target service failures wherever possible.
2. If the proxy is not a required dependency of the Tunnel, it must not be declared Required.
3. HTTP probing and the specific plugins above are capabilities for later phases.
4. The first version's configuration validator may define the complete configuration model up front, but unimplemented resource types must produce a clear capability-missing error instead of pretending to run.
5. Recovery actions must match the plugin's declared capabilities.

## 10.3 Configuration loading process

1. Parse the configuration.
2. Validate the version.
3. Validate the structure.
4. Validate resource IDs.
5. Validate dependency references.
6. Validate the dependency graph.
7. Validate recovery action references.
8. Validate plugin capabilities.
9. Validate permission and risk policies.
10. Validate notification channel configuration.
11. Validate database migration compatibility.
12. Create or update the runtime resource registry.

When configuration validation fails, the specific field and reason must be reported.

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
- Supportive of global budgets and notification rules.

TOML or YAML is suggested for the first version, but the first version picks exactly one primary configuration format. Avoid maintaining two formats at once.

## 10.2 Example: a Cloudflare Tunnel depending on a proxy

The following is the target configuration shape, shown to illustrate architecture semantics; it does not mean the first version already has the corresponding plugins.

```yaml
version: 1

resources:
  - id: proxy.egress
    type: proxy
    plugin: mihomo
    config:
      endpoint: \

>You are responsible for implementing Custos, a Rust-based cross-platform local self-healing engine.

Please first read the architecture design documents in the project root in full, and inspect the actual state of the repository.

Phase one implements P0 only: the core domain model, the Resource/Dependency Graph, configuration validation, Process Probe, Exec Probe, the RecoveryRun state machine, ordered recovery candidate plans, post-action health verification, recovery budgets, per-resource mutexes, SQLite persistence, basic CLI commands and integration tests.

Strictly follow these requirements:

1. Do not implement every planned feature in one go.
2. Do not implement HTTP Probe, Dashboard, OpenClash or complex network recovery in P0.
3. Analyze the existing code before proposing an implementation plan; do not assume the repository is empty.
4. Dependency recovery is coordinated by the Core Engine; plugins must not bypass the core engine and run recovery flows directly.
5. When a dependency is unsatisfied, upstream recovery actions that depend on it must not execute.
6. A recovery action succeeding does not mean the resource is healthy; independent verification is mandatory.
7. After an action fails, diagnose again and re-evaluate dependencies and candidate plans.
8. Every recovery task must have explicit timeout, budget and termination conditions.
9. Persist state before executing critical actions; after a process crash, do not unconditionally repeat actions with unknown outcomes.
10. One resource must not run multiple recovery tasks concurrently.
11. All state transitions and action attempts must be auditable.
12. Fake tests, hardcoded success results or unimplemented capability claims must never be used to pass acceptance.

Execute in the following steps:

A. Output the current repository structure, existing modules, dependency relationships and the implementations to be preserved.

B. Break P0 into independently acceptable small tasks, listing each task's files, interfaces, tests and dependencies.

C. First implement the minimal complete vertical loop: controlled process probe failure → fault verdict → create RecoveryRun → execute recovery action → independent health verification → persist the final result.

D. On that foundation, add the dependency graph, dependency recovery, multiple candidate plans, recovery budgets and crash recovery.

E. Complete SQLite persistence and the CLI.

F. Add tests for plugin timeouts, unknown action outcomes, dependency cycles, concurrent recovery, database failures and post-action verification failures.

G. Run formatting, static checks, unit tests and integration tests, and report the actual results.

After every task, you must provide the list of modified files, design notes, test commands, actual test results and unresolved issues. When the architecture documents conflict with the actual code, state the conflict and its impact first; never silently overwrite.

Do not move on to P1 or P2 early. Only after every P0 acceptance criterion is met should you submit the next phase's implementation plan.

**One important suggestion:** put these two documents into the repository as `docs/architecture.md` and `docs/implementation/p0.md`. When different Code Agents later implement the core engine, the plugin runtime and the notification module separately, all of them should work from the same domain model and state machine, to avoid designing mutually incompatible interfaces.

---

If you want, I can:

- Describe the Recovery Engine state machine and recovery flow in more detail
- Elaborate on RecoveryRun management and auditing implementation details
- Expand on plugin permissions and security policy design in the plugin architecture

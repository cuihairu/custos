---
title: Dashboard, CLI and Configuration
---

# 9. Web Dashboard and CLI

## 9.1 Dashboard disabled by default

The Dashboard is an optional component and is disabled by default; when not enabled there are zero listening ports and zero HTTP code paths.

Target configuration shape (not implemented before P4):

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
5. Transport security (auth, bind address, CSRF) is hardened centrally with the toolbox (ADR-0015).

## 9.2 CLI commands

The CLI is the P0/P1 operating surface (implemented):

```text
custos validate <CONFIG>           Parse and validate the configuration (rejects duplicate IDs, dangling references, cycles)
custos start <CONFIG> [OPTIONS]    Run the engine loop (--db PATH, --interval MS, --once)
custos status [--db PATH]          Store counters and recent audit events
custos runs [--db PATH]            Recent recovery runs (newest first)
custos incidents [--db PATH]       Incident history
custos notify status [--db PATH]   Notification queue counters (five states) and recent entries
custos notify retry <ID> [--db]    Manually requeue a dead-lettered notification
custos resources <CONFIG>          List configured resources
custos graph <CONFIG>              Dependency graph in topological order
```

`start` performs crash recovery first (unfinished runs are marked outcome-unknown and never blindly retried), then enters the probe/recover tick loop; the notification engine is wired into the same process (event enqueue + background delivery, offline drain).

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

The first version uses TOML (one primary format, `version = 1`).

## 10.2 Example: a Cloudflare Tunnel depending on a proxy

The following is the target configuration shape, shown to illustrate architecture semantics; it does not mean the first version already has the corresponding plugins.

```yaml
version: 1

resources:
  - id: proxy.egress
    type: proxy
    plugin: mihomo
    config:
      endpoint: http://127.0.0.1:7890

  - id: tunnel.egress
    type: tunnel
    plugin: cloudflared
    config:
      entry: https://api.example.com
    dependencies:
      - resource: proxy.egress
        requirement: required

notification:
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

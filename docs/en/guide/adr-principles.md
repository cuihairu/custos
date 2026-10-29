---
title: ADRs and Final Implementation Principles
---

# 17. ADRs: Architecture Decisions That Must Be Recorded

The following decisions need to be archived as ADRs:

1. Why Rust was chosen.
2. Why the first version uses external-process plugins.
3. Why JSON-RPC over stdio was chosen.
4. Why Resource + Dependency Graph was chosen.
5. Why the dependency graph is coordinated by the Core Engine.
6. Why action results must go through independent verification.
7. Why SQLite was chosen.
8. Why critical state must be persisted before recovery actions.
9. Why the notification system is decoupled from the recovery engine.
10. Why the Dashboard is disabled by default.
11. Why P0 implements only the Process and Exec Probes.
12. Why unauthorized high-risk network actions are not allowed.
13. Why unknown execution outcomes must not be retried directly.
14. Why ordinary dependency graph cycles are forbidden.

An ADR must record the background, the decision, the alternatives, the pros and cons, and the follow-up impact.

---

# 18. Final Implementation Principles

Custos's success criterion is not how many kinds of services it can monitor, but whether, when a real failure strikes, it can correctly judge dependencies, execute appropriate recovery actions, verify the results, and leave a complete record.

The whole system should follow:

**Resources describe objects, dependencies describe preconditions, plugins provide capabilities, policies decide actions, the core engine coordinates execution, verification judges results, persistence preserves facts, notification delivers events, and the Dashboard provides optional display.**

Phase one must first get the core recovery loop, dependency semantics, persistence and failure handling right. Windows network recovery, proxies, Cloudflare Tunnel, OpenClash and other platform capabilities are added gradually afterwards.

Do not implement every plugin at once, and do not build a complex Dashboard or plugin marketplace ahead of time.

Prove the architecture with the minimal but complete recovery loop first, then expand based on the failures that real deployments cannot yet handle.



## First execution instruction for the Code Agent

The documents make the overall goal clear, but when implementation actually starts, I suggest handing the following passage alone to the main agent first, letting it inspect the repository and execute P0, instead of implementing every phase immediately.



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

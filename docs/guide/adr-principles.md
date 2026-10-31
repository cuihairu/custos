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

15. Why the core maintains an internal integration toolbox (HTTP/RPC/SSH/probing primitives) and plugins are only the rule layer.

**Background**: as probing and recovery actions come to cover HTTP, RPC, SSH, service control and other transports/protocols, if every plugin implements its own transport, the security hardening (TLS verification, SSRF protection, SSH host-key verification, timeout/retry/cancellation, credential-safe reads) is diluted across N implementations and cannot be uniformly enforced.

**Decision**: the core converges a set of "internal integration tools" as capability executors (HTTP client, RPC client, SSH client, process/service/disk/network-connectivity probes, controlled command execution, reboot control), declared via Capability + OperationParams. Plugins and built-in capabilities share one dispatch table: built-in capabilities execute in-process in the core, plugin-declared capabilities execute in the plugin subprocess. Plugins no longer implement transports themselves; they provide capability declarations and rule logic (targets, parameters, decisions, verification).

**Alternatives**: keep plugins self-contained, each implementing its own transport. Drawbacks: duplicated transports, hardening easily bypassed, larger plugin footprint and attack surface.

**Pros and cons**: hardening happens once, in one place; unified semantics (timeout/cancel/budget/audit); thinner plugins. The cost is that the core's tool surface must be maintained, and plugin freedom shrinks (no arbitrary custom protocols).

**Follow-up impact**: P2 platform adapters (systemd/launchd/SCM, network interface/route/DNS) register as tool executors in the same registry; the Webhook notification channel reuses the toolbox HTTP client (with SSRF protection); the Python outbound bindings expose tool calls, not arbitrary execution.

---

# 18. Final Implementation Principles

Custos's success criterion is not how many kinds of services it can monitor, but whether, when a real failure strikes, it can correctly judge dependencies, execute appropriate recovery actions, verify the results, and leave a complete record.

The whole system should follow:

**Resources describe objects, dependencies describe preconditions, plugins provide capabilities, policies decide actions, the core engine coordinates execution, verification judges results, persistence preserves facts, notification delivers events, and the Dashboard provides optional display.**

Phase one must first get the core recovery loop, dependency semantics, persistence and failure handling right. Windows network recovery, proxies, Cloudflare Tunnel, OpenClash and other platform capabilities are added gradually afterwards.

Do not implement every plugin at once, and do not build a complex Dashboard or plugin marketplace ahead of time.

Prove the architecture with the minimal but complete recovery loop first, then expand based on the failures that real deployments cannot yet handle.

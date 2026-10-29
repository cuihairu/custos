> This document is compiled from a ChatGPT shared conversation (source: https://chatgpt.com/s/t_6ae30fee51e081918c88645c26817ff7, retrieved 2026-10-29). It is external advice, not a statement of fact about this repository; verify every item against the actual code before putting it into practice.

---
title: Overview and Architecture Decisions
---

# 1. Project Goals and Boundaries

## 1.1 The problem Custos solves

Custos addresses the situation where a device or service has failed and needs human intervention to recover — especially scenarios where the management channel itself depends on the thing that failed.

Typical scenarios:

1. A Windows PC loses Wi-Fi, so Sunlogin goes offline, and the network needs to be restored automatically.
2. A proxy node misbehaves, external access fails, and the node needs to be switched or the proxy service restarted.
3. A Cloudflare Tunnel drops while the proxy is fine — only cloudflared needs restarting.
4. A Cloudflare Tunnel drops while the proxy is also broken — the proxy must be recovered first, then the Tunnel.
5. A Linux service misbehaves, and the root cause may be a dependency service, the network, or local resources.
6. An OpenClash node misbehaves, and connection state must be diagnosed and the node switched in an orderly way.
7. Automatic recovery fails repeatedly, so complete evidence must be recorded and the user notified.

Common traits:

- When the failure happens, the remote management channel may already be unusable.
- The device must complete detection and recovery locally, on its own.
- A single service being alive is not enough to prove it is actually working.
- Repair actions have preconditions and dependencies between them.
- A repair action may execute successfully while the service remains unusable.
- When the network is down, notification services may be temporarily unavailable.
- The user needs to review the cause, the handling process and the final outcome afterwards.

## 1.2 Product goals

Custos should provide:

- Local continuous monitoring.
- Resource and dependency management.
- Failure detection and root-cause diagnosis.
- Dependency-aware recovery orchestration.
- Ordered candidate plans and re-diagnosis after failures.
- Health verification after recovery.
- Crash-recoverable persistence of task state.
- Complete structured audit records.
- An extensible plugin runtime.
- Built-in implementations of common notification channels.
- Extension points for third-party notification channels.
- An optional Web Dashboard, disabled by default.
- Platform adaptation for Windows, Linux and macOS, with BSD and OpenWrt to follow.

## 1.3 What is explicitly out of scope

Phase one does not implement:

- A mandatory cloud control plane.
- A multi-tenant SaaS operations platform.
- A general-purpose remote desktop and remote command execution platform.
- A software package management and auto-upgrade platform.
- Unrestricted automatic execution of arbitrary scripts.
- A management API listening on the public internet by default.
- Monitoring and recovery flows that only work over the network.
- A plugin marketplace, plugin auto-download or auto-install.
- Forcing every platform's behavior into a completely identical interface.

Custos is positioned as a locally autonomous self-healing engine, not yet another large operations platform.

---

# 2. Core Architecture Decisions

## 2.1 Unified domain model

Custos uses the following core concepts:

| Concept | Responsibility |
|---|---|
| Resource | The managed resource, e.g. Wi-Fi, proxy, Cloudflare Tunnel, a process or a business entry point |
| Dependency | The dependency relationship between resources and its satisfaction condition |
| Probe | The capability that collects resource health state |
| Evidence | Probe results, errors, timestamps and related evidence |
| Diagnosis | The fault judgment derived from evidence |
| RecoveryPolicy | The policy deciding how failures are handled |
| RecoveryAction | A recovery action that can be executed |
| Verification | Judging whether recovery truly succeeded |
| RecoveryRun | One complete failure-handling process |
| Plugin | An extension module encapsulating specific resources, probing, recovery or notification capabilities |
| Notification | An event that must be reported to the user or external systems |
| AuditEvent | A traceable system event |
| RecoveryStore | The persistence component holding state, evidence, actions and audit data |

The following distinctions must be preserved:

**A Resource is an object, a Dependency is a relationship, a Plugin is a capability implementation, a RecoveryPolicy is a policy, and a RecoveryRun is one execution instance.**

These concepts must not be conflated.

## 2.2 Component architecture

```mermaid
flowchart TB
    CLI[Custos CLI]
    Dashboard[Optional Web Dashboard]
    Config[Configuration Loader]

    subgraph Core[\

Host[Plugin Host]
        Probe[Probe Capabilities]
        Diagnose[Diagnosis Capabilities]
        Actions[Recovery Actions]
        Channels[Notification Channels]
    end

    Store[(SQLite Recovery Store)]
    Notify[Notification Engine]
    OS[Platform Adapters]
    External[External Systems]

    CLI --> Core
    Dashboard --> Core
    Config --> Registry
    Registry --> Graph
    Scheduler --> Engine
    Graph --> Engine
    Engine --> Policy
    Policy --> Host
    Host --> Probe
    Host --> Diagnose
    Host --> Actions
    Host --> OS
    Engine --> Verify
    Verify --> Events
    Engine --> Events
    Events --> Store
    Events --> Notify
    Notify --> Channels
    Channels --> External
    Core --> Store
```

The Dashboard is just an optional client. Even if the Dashboard is not compiled at all, the core engine must retain the full self-healing capability.

The CLI is the primary management interface.

## 2.3 Core operating principles

1. The Core Engine controls the entire recovery lifecycle.
2. Plugins provide capabilities; they do not own global recovery decisions.
3. Every recovery action must pass policy, permission and budget checks.
4. After any action completes, resource health must be re-evaluated.
5. Once dependency recovery completes, dependencies must be re-probed, not assumed healthy.
6. Every task and critical state transition must be persisted.
7. Notification failure must not block recovery.
8. Dashboard failure must not affect recovery.
9. Actions with unknown outcomes must not be blindly repeated.
10. Recovery tasks must have explicit termination conditions.

---

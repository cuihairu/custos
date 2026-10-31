---
title: Phased Implementation Plan
---

# 14. Phased Implementation Plan

## P0: Core recovery engine

Goal: implement a complete, testable recovery loop first, without wiring in complex network capabilities.

Implementation items:

1. Cargo Workspace.
2. Domain models such as Resource, Dependency, Evidence, Diagnosis, RecoveryPolicy and RecoveryRun.
3. Configuration parsing and validation.
4. Dependency graph construction and cycle detection.
5. Process Probe.
6. Exec Probe.
7. Consecutive failure thresholds and health state transitions.
8. The RecoveryRun state machine.
9. Ordered candidate actions.
10. Re-verification after a recovery action executes.
11. Re-diagnosis after failure.
12. Timeouts, budgets and cooldowns.
13. Per-resource recovery mutexes.
14. SQLite persistence.
15. Crash recovery and unknown-outcome handling.
16. Basic CLI commands.
17. Structured logging and audit events.
18. Integration tests and fault injection.

P0 does not include:

- HTTP Probe.
- Web Dashboard.
- Remote management API.
- Plugin marketplace.
- OpenClash.
- Complex Wi-Fi recovery.
- Enterprise notification platforms.

Acceptance criteria:

- A process exiting can be detected.
- A controlled command failing can trigger recovery.
- When the first recovery plan fails, the second plan can be selected.
- When a command returns success but the resource is still unhealthy, recovery must not be reported as successful.
- When a dependency is unhealthy, upstream actions do not execute early.
- The same resource never runs multiple recovery tasks concurrently.
- After Custos crashes and restarts, unfinished tasks can be resumed.
- Actions with unknown outcomes are not unconditionally repeated.
- All state transitions are traceable through logs and the database.
- Every task that exhausts its budget terminates explicitly.

## P1: Plugin runtime and notification infrastructure

Implementation items:

1. The JSON-RPC 2.0 stdio plugin protocol.
2. Plugin capability declarations.
3. Plugin start, shutdown and restart management.
4. Timeouts, cancellation and output limits.
5. Plugin crash isolation.
6. NotificationEvent.
7. Local log channel.
8. Built-in Webhook channel.
9. Built-in SMTP channel.
10. Persisted notification queue.
11. Notification retry, deduplication and dead-letter states.
12. CLI notification status and retry commands.
13. Plugin permission and capability checks.

Acceptance criteria:

- A plugin crash does not exit the core process.
- Heavy stderr output does not block protocol communication.
- An unresponsive plugin times out within the configured limit.
- While the network is down, notifications stay in the local queue.
- Once the network recovers, queued notifications continue sending.
- Notification delivery failure does not affect recovery tasks.
- Sensitive fields never reach ordinary logs.

## P2: Platform base capabilities

Prioritize Linux and Windows.

Implementation items:

- Process management.
- systemd service control.
- Windows Service control.
- Platform capability detection.
- Platform permission checks.
- System reboot action.
- Basic local network probing.
- Platform adapter integration tests.

Acceptance criteria:

- On Linux without systemd, capability-unsupported is reported correctly instead of misexecuting.
- Windows service operations have timeouts and result verification.
- High-risk operations without explicit authorization are rejected.
- Platform-specific errors convert into unified fault results.

## P3: Network, proxy and real self-healing scenarios

Implementation items:

- Network interface probing.
- Gateway and route probing.
- DNS probing.
- HTTP Probe.
- Proxy connection and authentication diagnosis.
- Mihomo plugin.
- cloudflared plugin.
- Windows Wi-Fi recovery.
- OpenWrt network adapter.
- OpenClash plugin.
- End-to-end verification after network recovery.
- Mutexes and risk control for network operations.

Acceptance scenarios:

1. Sunlogin offline but the network fine: only Sunlogin-related services are recovered.
2. Wi-Fi disconnected: restore the network first, then verify remote management capability.
3. Proxy healthy, Tunnel abnormal: restart the Tunnel.
4. Proxy broken, Tunnel abnormal: recover the proxy first, then handle the Tunnel.
5. Proxy returns 407: identify the authentication problem; do not declare the service down.
6. Proxy recovered but the Tunnel is still abnormal: diagnose the Tunnel again.
7. Every recovery plan failed: stop automatic handling and notify.
8. The network drops again during recovery: the local engine keeps running and persists results.

## P4: Optional management interface and extensions

Implementation items:

- Web Dashboard, disabled by default.
- Resource list and health status.
- Dependency graph display.
- RecoveryRun timeline.
- Notification status.
- Configuration checks.
- Richer notification plugins.
- macOS adaptation.
- BSD adaptation.
- Full OpenWrt capability.
- Plugin signing and distribution policy.
- A more complete operation authorization interface.

Acceptance criteria:

- With the Dashboard disabled by default, no port is listened on.
- When the Dashboard is unavailable, core functionality is unaffected.
- The Dashboard never directly executes actions that bypass the Core Engine.
- High-risk operations still go through the unified permission and audit process.

---

## Implementation tracking

Check off each phase with its commit hash once it lands; for sources and acceptance criteria see the corresponding phase above.

- [ ] P0 core recovery engine (commit: __)
- [ ] P1 plugin runtime and notification infrastructure (commit: __)
- [ ] P2 platform base capabilities (commit: __)
- [ ] P3 network, proxy and real self-healing scenarios (commit: __)
- [ ] P4 optional management interface and extensions (commit: __)

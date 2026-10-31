# ADR-0012: Why Unauthorized High-risk Network Actions Are Forbidden

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/security.md §12.2, platforms.md

## Background

A self-healing engine holds privileges to restart services, reset network interfaces, and change routes. Network actions differ fundamentally from ordinary service restarts: **they can cut the management channel** — Wi-Fi reconnect fails, the default route is cleared, the firewall is reset — the engine loses contact with the operator entirely, and this is exactly when the system most needs a human. A self-healing system losing its own connection is the worst failure mode.

## Decision

High-risk actions (`RiskLevel::High`: interface reset, system reboot, firewall clearing) are denied by default; they require explicit authorization (`permissions.default_allow_high_risk` or per-resource `recovery_policy.allow_high_risk`) and every execution enters the authorization audit; interface resets must be explicitly flagged "may interrupt management channel"; network operations have a global mutex (P3), with blanket firewall clears / full route resets forbidden.

## Alternatives

- **Allow by default, audit after**: once contact is lost, the audit is meaningless.
- **Per-resource default allow**: one configuration mistake exposes everything.

## Pros and Cons

Pros: the blast radius of destructive actions is explicitly controlled by configuration; the audit chain answers "who authorized it, when it ran."
Cons: capability requires configuration (documentation and examples must be clear); the "worth enabling high-risk" judgment falls on the user.

## Follow-up Impact

- Every P2 platform adapter's dangerous primitive must have a permission check first (todo P2 §permission checks).
- The P4 authorization UI shares the audit source, no custom permission model.

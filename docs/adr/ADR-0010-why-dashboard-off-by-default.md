# ADR-0010: Why Dashboard is Off by Default

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/dashboard-cli-config.md §9/§10, roadmap P4

## Background

The Dashboard is a management plane: it can see all resource health and trigger recovery operations. A self-healing engine with a default listening port is a persistent management entry on every target machine — exactly the attack surface a self-healing system should not have. P0/P1 goals (core recovery loop) require no UI.

## Decision

The Dashboard is a **disabled by default** optional component: when not enabled there are zero listening ports and zero HTTP code paths (acceptance: closed = no port, unavailability does not affect core, does not bypass the Core Engine, high-risk still goes through unified authorization audit). Before enabling, HTTP transport security (auth, CSRF, bind address, read-only semantics) must be centrally hardened (with the toolbox transport security, ADR-0015).

## Alternatives

- **On by default**: every machine has a persistent management plane; home/router deployments (OpenWrt) are especially dangerous.
- **CLI only, no Dashboard**: the CLI already covers status/runs/incidents/notify/graph; deferring the UI loses no core capability.

## Pros and Cons

Pros: zero new attack surface by default; the core stays headless and embeddable; UI iteration never touches recovery semantics.
Cons: early visibility relies on the CLI; operators need documentation.

## Follow-up Impact

- All P4 Dashboard entries are accepted under the "disabled by default" prerequisite.
- The P4 authorization UI reuses the core's authorization audit, not a custom permission model.

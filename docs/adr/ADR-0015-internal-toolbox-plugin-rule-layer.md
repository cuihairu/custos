# ADR-0015: Why the Core Maintains an Internal Toolbox, Plugins Are the Rule Layer

- Status: Accepted
- Date: 2026-10-31
- Related: adr-principles.md §17.15, ADR-0002, ADR-0012

## Background

Probes and recovery actions cover HTTP, RPC, SSH, service control and other transports/protocols. If every plugin implements its own transport, security hardening (TLS verification, SSRF protection, SSH host-key verification, timeout/retry/cancellation, credential-safe reads) is diluted across N implementations and cannot be uniformly enforced.

## Decision

The core converges a set of "internal integration tools" as capability executors (HTTP client, RPC client, SSH client, process/service/disk/network-connectivity probes, controlled command execution, reboot control), declared via Capability + OperationParams. Plugins and built-in capabilities share one dispatch table: built-in capabilities execute in-process in the core; plugin-declared capabilities execute in the plugin subprocess. Plugins no longer implement transports themselves — they provide capability declarations and rule logic (targets, parameters, decisions, verification).

## Alternatives

- **Self-contained plugins, each implementing its own transport**: duplicated implementations; hardening easily bypassed (one plugin disabling TLS verification); larger plugin footprint and attack surface.
- **Built-in only (no plugins)**: scenario-specific logic (mihomo/OpenClash domain decisions) cannot be contributed by the community.

## Pros and Cons

Pros: transport hardening happens once in one place (the webhook's SSRF protection is the first instance); timeout/cancel/budget/audit semantics are unified; plugins are thinner and auditable.
Cons: the core's tool surface must be continuously maintained; plugin freedom shrinks (no arbitrary custom protocols) — new protocol needs mean adding a core tool first, then writing plugin-layer rules.

## Follow-up Impact

- P2 platform adapters (systemd/launchd/SCM, network interface/route/DNS) register as tool executors in the same registry.
- The Webhook notification channel reuses the toolbox's HTTP client semantics (including SSRF policy, implemented).
- Python outbound bindings expose tool calls, not arbitrary code execution.

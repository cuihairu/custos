# ADR-0001: Why Rust

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/overview.md, workspace.md

## Background

Custos is a persistent local self-healing engine: long-running, latency-sensitive, must be cross-platform (Linux/Windows first), single-binary deployment, with strict state machines in the domain model (recovery runs, health transitions, budgets, mutexes). The language choice determines memory safety, deployment shape, and long-term maintenance cost.

## Decision

Rust as the only implementation language: domain models type-encoded (`HealthStatus` seven states, `RecoveryRunStatus` state machine, strongly-typed IDs), workspace layered by crate (model/engine/probe/notify/store/config/cli), async only inside the store (sync facade + current-thread runtime), TLS via rustls for pure-Rust cross-compilation.

## Alternatives

- **Go**: GC pauses and higher persistent memory footprint; error handling by convention (err != nil) with no compile-time constraint; cross-compilation to a single binary works but state-machine correctness relies on humans.
- **C/C++**: Memory safety is unacceptable — a self-healing engine holds system-level privileges; UAF/overflow is privilege escalation.
- **Python/Scripts**: Performance and deployment (interpreter, dependency tree) unsuitable for a persistent engine; weak type constraints.

## Pros and Cons

Pros: predictable latency without GC; `enum + match` exhaustiveness makes missing state-machine branches a compile-time error; mature ecosystem (rustls/sqlx/lettre/ureq) aligns with the "core uses mature crates" ruling; single binary, mingw cross-compilation verified.
Cons: longer compile times; slower iteration than scripting languages; some ecosystem complexity (PyO3 outbound bindings).

## Follow-up Impact

- The 11-crate workspace layering is carried by Rust's module system (one-way dependencies).
- All "core uses mature crates, plugin-side is lightweight" rulings build on this.
- rust-toolchain.toml must declare the stable channel (todo.md engineering cleanup).

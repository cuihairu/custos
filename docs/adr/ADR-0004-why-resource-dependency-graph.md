# ADR-0004: Why Resource + Dependency Graph

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/resources.md §3, ADR-0005

## Background

Correct handling of real failures is determined by topology: when the database dies, restarting the application is meaningless; when the proxy breaks, the Tunnel's "recovery" just spins. Resources are not a flat list of monitoring items — they form a directed dependency graph.

## Decision

`Resource` describes the object (probe, health policy, recovery policy); `Dependency` describes preconditions (required/optional/conditional + `HealthCondition`). The configuration loader builds both into a `DependencyGraph`: dangling references, duplicate IDs, impossible conditions, and cycles are all reported with concrete field paths at load time — startup is rejected instead of deadlocking at runtime.

## Alternatives

- **Flat resource list**: recovery has no ordering semantics; "fix db before app" cannot be expressed.
- **Implicit probe order (by config order)**: order is coincidental, not semantic; changing the config order changes the behavior.
- **Runtime dynamic dependency discovery**: no validation timing; the later an error is found, the costlier it is.

## Pros and Cons

Pros: recovery order is derivable (topological sort); all structural errors rejected at load time in one pass; conditional dependencies (`conditional_on`) and alternative groups (`alternative_groups`) express redundancy and degradation semantics.
Cons: modeling learning curve; configuration errors surface at startup (a feature, not a defect).

## Follow-up Impact

- graph.rs cycle detection returns the concrete cycle path (ADR-0014).
- The engine's waiter deduplication (§3.6) builds on graph semantics: all waiters on the same dependency share one recovery result.

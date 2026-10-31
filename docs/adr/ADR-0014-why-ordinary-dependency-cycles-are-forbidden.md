# ADR-0014: Why Ordinary Dependency Cycles Are Forbidden

- Status: Accepted
- Date: 2026-10-31
- Related: resources.md §3.5, ADR-0004

## Background

When A depends on B and B depends on A, the recovery order is logically unsolvable: both sides "wait for the other to become healthy first," forming mutual waiting. A cycle discovered at runtime manifests as deadlock or arbitrary-order random repairs; configuration errors silently fester.

## Decision

Ordinary dependency graphs must be acyclic: cycle detection at configuration load time (`DependencyGraph::build`); a cycle rejects startup and prints the concrete cycle path (`a -> b -> a`); users cannot "break" cycles with runtime fallbacks. Expressing "mutual backup" legitimately uses `alternative_groups` (redundancy groups, required_count semantics) instead of cycles.

## Alternatives

- **Runtime cycle breaking (timeout/arbitration)**: unpredictable behavior; recovery order depends on which side wins the race.
- **Allow cycles + tolerant topological sort**: the sort algorithm's output for cycles is arbitrary — equivalent to random recovery.

## Pros and Cons

Pros: unrecoverable modeling errors rejected once at startup; recovery order is always well-defined (topological sort).
Cons: mutual-dependency modeling needs alternative_groups (learning curve).

## Follow-up Impact

- Conditional dependencies (`conditional_on`) are special activation edges inside a DAG, also constrained by cycle detection.
- Configuration validation errors must include the full cycle path (tests: rejects_dependency_cycle_at_startup).

# ADR-0005: Why the Core Engine Coordinates the Graph

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/recovery-engine.md §6, ADR-0004

## Background

The graph is built — who drives it? If resources or plugins each judge "is my dependency healthy" and self-recover, concurrent execution inevitably produces two resources simultaneously repairing the same dependency, recovery-order races, and unknowing duplicate restarts.

## Decision

Recovery orchestration belongs exclusively to the Core Engine: each tick probes in topological order; dependency health is re-checked before recovery execution (§3.6); one recovery mutex per resource (the same resource never runs two recovery tasks concurrently); when a dependency is unhealthy, upstream resources wait (Waiting + waiter dedup) and the dependency's recovery result is shared to all waiters; budgets/cooldowns/high-risk counters are centralized state.

## Alternatives

- **Resource/plugin autonomous recovery**: races and ordering errors; duplicated side effects; no global budget control.
- **External orchestrator (systemd dependencies)**: systemd only manages service start ordering — it does not understand health probes or verification semantics; it moves core logic out of the core.

## Pros and Cons

Pros: single source of truth; wait/wake/dedup semantics centralized in one testable place; per-resource mutex naturally debounces.
Cons: the central component is complex (large state machine); requires high implementation quality — hence vertical integration tests and crash-recovery tests guard the recovery path.

## Follow-up Impact

- engine.rs drive/waiters/redrive/finish_* are the heaviest core paths — changes require tests.
- Plugins provide capabilities (actions) only; they must not make recovery decisions themselves (ADR-0015 toolbox model).

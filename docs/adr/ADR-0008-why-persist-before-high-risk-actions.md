# ADR-0008: Why Persist Critical State Before High-risk Actions

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/persistence.md §7.3, recovery-engine.md §6.6

## Background

The engine crashing while executing a recovery action is an expected scenario (power loss, OOM, upgrade). If the action already ran but the record was not persisted, after restart the core does not know the action executed — retrying means a second service restart / second network change, doubling side effects and making it unauditable.

## Decision

Write-ordering guarantee: the attempt is persisted with `outcome = NULL` (Executing state) **before** the external action runs; the outcome is written back after execution completes. On crash recovery: NULL outcome means "result may be unknown" — never treated as "not executed" (blind retry) nor as "success" — always re-diagnose (ADR-0013).

## Alternatives

- **Write after execution**: the crash window between execution and write loses facts, doubling retry side effects.
- **Write everything in memory, flush periodically**: larger window; incomplete audit trail.

## Pros and Cons

Pros: system state is derivable at any crash point; the audit chain is complete (what ran, when).
Cons: one extra synchronous write per action (cheap under SQLite WAL); code paths must strictly maintain write ordering.

## Follow-up Impact

- `resume_unfinished` marks non-terminal runs OutcomeUnknown and stops automatic retry.
- High-risk actions (RiskLevel::High) additionally go through budget and authorization (ADR-0012); persistence is the prerequisite.

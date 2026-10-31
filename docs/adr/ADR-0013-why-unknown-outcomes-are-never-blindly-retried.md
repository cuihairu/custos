# ADR-0013: Why Unknown Outcomes Are Never Blindly Retried

- Status: Accepted
- Date: 2026-10-31
- Related: recovery-engine.md §6.6, ADR-0008

## Background

The engine crashes (or is OOM-killed / times out) mid-recovery-action; the attempt's outcome stays NULL. The action **may have taken effect** (service restarted), may not have, or may be half-done. Retrying directly doubles side effects — the service is restarted twice, network changes are issued twice, state is corrupted when idempotency declarations are wrong.

## Decision

`OutcomeUnknown` is a first-class state: crash recovery marks non-terminal runs outcome-unknown and **stops automatic handling**, re-entering the "probe → diagnose" path with the resource's current state as the sole truth; unknown-outcome runs never re-execute using old diagnoses (P0 acceptance: unknown-outcome runs must not blindly retry). A new action is only possible when a new diagnosis finds the fault again with a new plan.

## Alternatives

- **Retry once directly**: side effects double (see background); it also masks "what actually happened in the last round."
- **Mark failed and give up**: pushes a self-healable fault to a human, violating the self-healing goal.

## Pros and Cons

Pros: side effects are bounded; the fact chain (what ran, result unknown, what the new diagnosis found) is complete and auditable.
Cons: the recovery path has one extra diagnosis cycle; probe quality matters more (diagnosis must be based on current health).

## Follow-up Impact

- `resume_unfinished` + `RecoveryRunStatus::OutcomeUnknown` + the engine event `recovery.outcome_unknown` (same-name notification event) implement this together.
- Idempotency declaration (`idempotent = true`) is an optimization, not an exemption: unknown results always re-diagnose.

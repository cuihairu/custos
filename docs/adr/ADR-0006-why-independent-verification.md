# ADR-0006: Why Independent Verification

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/recovery-engine.md §6, probe-diagnosis.md §5

## Background

The most convenient "success" signal for a recovery action is exit code 0 — but that only means the command ran: the process may have started and immediately died, the service may be up but not listening, the config may have been written but not applied. Trusting exit codes writes a false success back into the system, masks the real failure, triggers cooldown, and the failure is only passively discovered on the next probe.

## Decision

Each recovery plan's verification field (default `status_is = "healthy"`) judges the result via the **resource health probe**: after the action runs, the run enters Verifying; only when the probe meets the condition does it transition to Recovered. Verification failure re-enters diagnosis (the fault may have changed) and proceeds to the next plan or attempt. P0 acceptance is explicit: when the command succeeds but the resource is still abnormal, recovery must not be reported as successful.

## Alternatives

- **Trust exit code**: false success (see background).
- **Trust the action's own self-check output**: action and verification share a source — it cannot detect errors in the action's own implementation.

## Pros and Cons

Pros: recovery success = observable health, not a command's echo; verification failure naturally links into re-diagnosis.
Cons: verification waits for the next probe cycle (configurable convergence); probe quality becomes the floor for recovery correctness.

## Follow-up Impact

- VerificationResult is persisted — the audit chain answers "why was recovery judged successful."
- Both the engine event `recovery.verification` and the notification event `recovery.succeeded` are built on verification passing.

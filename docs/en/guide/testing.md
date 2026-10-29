---
title: Testing and Acceptance System
---

# 16. Testing and Acceptance System

## 16.1 Unit tests

Must cover:

- State transitions.
- Dependency verdicts.
- Dependency graph cycle detection.
- AlternativeGroup verdicts.
- Failure thresholds.
- Recovery budgets.
- Candidate plan ordering.
- Notification rules.
- Event deduplication.
- Sensitive data redaction.

## 16.2 Integration tests

Must cover:

- Process Probe.
- Exec Probe.
- Plugin startup failure.
- Plugin output blocking.
- Plugin timeout.
- Plugin crash.
- Resource dependency recovery.
- Multiple upstream resources depending on one resource.
- Concurrent recovery of the same resource.
- Notification queue persistence.
- Database restart recovery.
- Dashboard disabled by default.

## 16.3 Fault injection tests

Must simulate:

1. A process crash before a recovery action executes.
2. A process crash while a recovery action is executing.
3. A crash after the action completed but before it was recorded.
4. An action succeeding but health verification failing.
5. A dependency failing again during recovery.
6. A plugin exiting mid-request.
7. The network dropping while a notification is being sent.
8. A SQLite write failure.
9. Multiple tasks requesting recovery of the same resource simultaneously.
10. The recovery budget limit being reached.

Every fault must have explicit expected states and verification assertions.

## 16.4 Definition of done

A phase is only complete when:

- The code compiles.
- Formatting checks pass.
- Static checks pass.
- Unit tests pass.
- Integration tests pass.
- Critical failure paths have tests.
- Documentation matches the actual implementation.
- There are no unexplained TODOs or fake capability claims.
- Failures never cause infinite loops.
- Every recovery action has an explicit result.
- Critical state changes are auditable.

---

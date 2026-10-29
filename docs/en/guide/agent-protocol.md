---
title: Code Agent Execution Protocol
---

# 15. Execution Protocol for Code Agents

## 15.1 Generating the whole project from documents in one shot is not allowed

Implementation must be phased.

For each phase, first inspect the current repository state, then create the task list and implementation plan.

If the repository already contains code:

- Do not overwrite existing working implementations.
- Do not assume the previous architecture is obsolete.
- Existing call paths and module dependencies must be analyzed.
- Identify compatibility impact first, then decide the refactoring scope.
- Keep existing tests and add regression tests.

## 15.2 Every task must output

1. The list of modified files.
2. The reason for the change.
3. Interface changes.
4. State machine impact.
5. Database migration impact.
6. Platform compatibility impact.
7. Security impact.
8. New tests.
9. The verification commands executed.
10. The acceptance results.
11. Unresolved issues.

Submitting code without stating the verification results is not acceptable.

## 15.3 The agent's working order

Every phase follows:

```text
Inspect Repository
    ↓
Create Implementation Plan
    ↓
Define Interfaces
    ↓
Implement Minimal Vertical Slice
    ↓
Add Unit Tests
    ↓
Add Integration Tests
    ↓
Run Formatting and Linting
    ↓
Run Test Suite
    ↓
Review Security and Failure Paths
    ↓
Update Documentation
    ↓
Report Results
```

Each phase must only move to the next one after its tests pass.

## 15.4 Suggested first tasks

### Task 001: Domain model

Implement Resource, Dependency, HealthStatus, Evidence, Diagnosis, RecoveryPolicy, RecoveryRun, RecoveryAction, VerificationResult and AuditEvent.

Deliverables:

- Type definitions.
- State transition rules.
- Serialization and deserialization tests.
- Invalid state transition tests.
- Domain model documentation.

### Task 002: Configuration and dependency graph

Implement configuration parsing, reference validation, dependency graph construction, cycle detection and dependency condition validation.

Deliverables:

- A configuration schema or equivalent validation rules.
- The dependency graph algorithm.
- Error path location.
- Tests for valid configurations, missing dependencies, duplicate IDs and dependency cycles.

### Task 003: Recovery state machine

Implement RecoveryRun state transitions, recovery budgets, per-resource mutexes, action execution and the verification loop.

Deliverables:

- The complete state machine.
- Re-diagnosis after failure.
- A second candidate plan.
- Unknown action outcome handling.
- Concurrent recovery tests.

### Task 004: SQLite persistence

Implement database migrations, RecoveryRun persistence, action records and recovery task resumption.

Deliverables:

- The database schema.
- Transaction boundary documentation.
- Process crash tests.
- Database migration tests.
- Unfinished task recovery tests.

### Task 005: Plugin Host

Implement JSON-RPC stdio, the plugin lifecycle, message size limits, timeouts and cancellation.

Deliverables:

- Plugin protocol documentation.
- A sample plugin.
- Protocol tests.
- Heavy-output tests.
- Plugin crash and timeout tests.

### Task 006: Notification Engine

Implement event rules, the persisted queue, Webhook, SMTP, local logs and delivery retries.

Deliverables:

- A unified notification interface.
- The event model.
- The persisted queue.
- Channel implementations.
- Network-outage retry tests.
- Credential redaction tests.

### Task 007: CLI

Implement commands for status, resources, the dependency graph, fault history, recovery history and notification status.

Deliverables:

- CLI commands.
- Error output conventions.
- Help documentation.
- Integration tests.

### Task 008: First real recovery scenario

Prefer a simple scenario that can be verified automatically, for example automatic recovery after a controlled process exits.

Then implement the proxy and Cloudflare Tunnel scenario to verify the dependency recovery order.

Deliverables:

- Example configuration.
- Fault injection tests.
- A complete RecoveryRun record.
- Notification results for both successful and failed recovery.

---

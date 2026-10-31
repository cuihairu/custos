---
title: Persistence and Crash Consistency
---

# 7. Persistence and Crash Consistency

## 7.1 Database

The first version uses SQLite.

Enabling WAL is recommended, with an appropriate `synchronous` strategy chosen according to reliability requirements. Do not assume WAL automatically guarantees every record is durable, and there is no need to unconditionally fsync after every log write.

For critical recovery operations, transaction boundaries should be explicit.

## 7.2 Core tables

At least the following are recommended:

```text
resources
resource_dependencies
resource_health
incidents
recovery_runs
recovery_steps
probe_results
diagnoses
action_attempts
verification_results
audit_events
notification_queue
notification_attempts
plugin_registry
configuration_versions
```

Where:

- `resources` stores resource definitions.
- `resource_dependencies` stores dependency relationships.
- `resource_health` stores the current health snapshot.
- `incidents` stores fault lifecycles.
- `recovery_runs` stores recovery tasks.
- `recovery_steps` stores step order and status.
- `probe_results` stores probe evidence.
- `diagnoses` stores diagnosis results.
- `action_attempts` stores every action attempt.
- `verification_results` stores verification results.
- `audit_events` stores event auditing.
- `notification_queue` stores pending notifications.
- `notification_attempts` stores delivery history.
- `plugin_registry` stores loaded plugins and their capabilities.
- `configuration_versions` stores configuration versions or change summaries.

## 7.3 Persistence order for high-risk actions

Before executing a high-risk action:

1. Validate the resource and dependency conditions.
2. Validate action permissions.
3. Validate budget and mutexes.
4. Create the ActionAttempt.
5. Mark the status as `Executing`.
6. Execute the external action only after the transaction commits successfully.
7. Record the action's return result.
8. Re-verify resource health.
9. Update the RecoveryRun status.

If Custos crashes during step 6, then after recovery it must recognize that the execution outcome may be unknown.

"Database status written as Executing" must not be mistaken for "the external action definitely ran", and "the external action returned 0" must not be mistaken for "the service definitely recovered".

## 7.4 Data retention

Configuration:

- Audit record retention period.
- Probe detail retention period.
- Recovery task retention period.
- Notification delivery record retention period.
- Database size limit.

By default, do not delete Incidents, RecoveryRuns that are still active, or notifications not yet delivered.

Log compaction or cleanup must be decoupled from active task state.

---

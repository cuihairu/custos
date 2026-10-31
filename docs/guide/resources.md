---
title: Resource and Dependency Model
---

# 3. Resource and Dependency Model

## 3.1 Resource definition

A Resource is the basic object that Custos monitors and recovers.

Example resource types:

- `process`
- `system-service`
- `network-interface`
- `network-connectivity`
- `proxy`
- `tunnel`
- `http-endpoint`
- `custom-command`
- `database`
- `application`

Every resource should have:

```text
Resource
├── id
├── type
├── display_name
├── plugin_ref
├── config
├── health_policy
├── recovery_policy
├── dependencies[]
├── permissions
└── enabled
```

Field semantics:

- `id`: a stable, unique identifier within configuration scope.
- `type`: the resource type.
- `plugin_ref`: the plugin implementing the resource capabilities.
- `config`: configuration specific to the resource type.
- `health_policy`: health criteria, consecutive failure thresholds and recovery confirmation requirements.
- `recovery_policy`: recovery candidate plans and recovery limits.
- `dependencies`: the resource's upstream dependencies.
- `permissions`: the scope of operations allowed for this resource.
- `enabled`: whether monitoring and automatic recovery are enabled.

Resource IDs must not change across process restarts, plugin restarts or machine reboots.

## 3.2 Resource health status

The following health states are recommended:

```rust
enum HealthStatus {
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
    Recovering,
    Suppressed,
    Unreachable,
}
```

State semantics:

- `Unknown`: not enough evidence has been collected yet.
- `Healthy`: health criteria are satisfied.
- `Degraded`: some capability is impaired, but the service still works.
- `Unhealthy`: health criteria are clearly not satisfied.
- `Recovering`: a recovery process is being executed.
- `Suppressed`: automatic handling is paused by policy.
- `Unreachable`: the probe itself cannot obtain a valid result, so the root cause cannot yet be confirmed.

`Unreachable` must not be treated as equivalent to `Unhealthy`. For example, a probe timeout against a remote service could mean the remote end failed, or that the local network failed.

Resource state should also record:

- When the state was last updated.
- Consecutive success and failure counts.
- The time of the last valid probe.
- The currently active fault identifier.
- The RecoveryRun currently being executed.
- A summary of the current evidence.

## 3.3 Dependency definition

A dependency describes the conditions a resource needs satisfied in order to run normally.

Suggested definition:

```rust
struct Dependency {
    target_resource_id: ResourceId,
    requirement: DependencyRequirement,
    condition: HealthCondition,
    resolution_policy: DependencyResolutionPolicy,
    timeout: Duration,
}
```

where:

```rust
enum DependencyRequirement {
    Required,
    Optional,
    Conditional,
    AlternativeGroup,
}
```

Concrete semantics:

### Required

A dependency that must be satisfied.

If it is not, recovery actions that depend on it cannot be executed — unless the action is explicitly marked as being for recovering that dependency itself.

### Optional

A non-mandatory dependency.

Recovery may be attempted, but whether it blocks upstream operations is decided by policy.

### Conditional

Only required under specific conditions.

For example, the proxy must be healthy in proxy mode, but not in direct-connection mode.

### AlternativeGroup

A set of candidate resources where the specified number must be satisfied.

For example:

```text
ProxyAccess
├── PrimaryProxy
└── BackupProxy

Requirement: at least one available
```

Backup resources must not be naively modeled as "every dependency must be healthy".

## 3.4 Health conditions

A dependency must declare which health condition it depends on, not merely which resource it depends on.

Example:

```yaml
dependencies:
  - resource: proxy.egress
    requirement: required
    condition:
      status: healthy
```

More complex conditions may require:

- The resource status to be Healthy.
- A specific capability to be available.
- The last successful probe to be no older than a specified time.
- Consecutive successes to reach a specified count.
- A specific business check to pass.

The first version should implement only a small set of clear, testable conditions — no arbitrary expression language.

## 3.5 Dependency graph and cycle detection

Dependencies form a directed graph.

If resource A depends on B, the dependency edge is:

```text
A -> B
```

When recovering A, B is evaluated first.

At configuration load time, Custos must:

1. Verify resource IDs are unique.
2. Verify all dependency references exist.
3. Detect hard dependency cycles.
4. Verify that AlternativeGroup configurations are valid.
5. Check for dependency conditions that can never be satisfied.
6. Report the exact configuration error path.

For:

```text
A -> B -> C -> A
```

startup must be rejected at configuration time; the system must not wait until recovery and recurse forever.

The first version may forbid all ordinary dependency cycles. If cycles or mutual-backup relationships are ever needed later, they should be modeled through explicit redundancy constructs, not by allowing arbitrary cycles.

## 3.6 Dependency recovery algorithm

When recovering resource A:

1. Collect A's dependency set.
2. Check whether each dependency's current state is fresh enough.
3. Re-probe when state is unknown or stale.
4. Dependencies whose conditions are satisfied pass directly.
5. Unsatisfied dependencies enter recovery according to their own policy.
6. Wait for dependency recovery to complete.
7. Re-verify dependency health.
8. Only after all required dependencies are satisfied may A's recovery actions execute.
9. If dependency recovery fails, A's policy decides whether to wait, skip non-critical dependencies, terminate or escalate.

Two kinds of actions must be distinguished:

- Recovery actions executed in order to fix the dependency itself.
- Upstream actions that must wait for the dependency to be healthy before executing.

For example, when the proxy is unhealthy, "switch proxy node" is allowed; but restarting the Cloudflare Tunnel — which depends on the proxy — should not be executed while expecting it to succeed.

## 3.7 Concurrency and dependency deduplication

The same resource may be depended on by multiple upstream resources.

For example:

```text
Cloudflare Tunnel ──> Proxy
Sunlogin ────────────> Network
Other Service ───────> Proxy
```

If multiple recovery tasks discover at the same time that the Proxy is unhealthy, multiple proxy restart tasks must not be started simultaneously.

Requirements:

- Each resource has its own recovery mutex.
- At most one active RecoveryRun per resource at a time.
- Other tasks may wait on the same recovery result.
- When resource health changes, waiting tasks re-evaluate their dependencies.
- Recovery tasks must not repeatedly start the same high-risk action.
- Global and per-resource recovery budgets must be defined.

---

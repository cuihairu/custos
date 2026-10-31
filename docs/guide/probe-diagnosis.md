---
title: Probing, Diagnosis and Failure Verdicts
---

# 5. Probing, Diagnosis and Failure Verdicts

## 5.1 Separation of Probe and Diagnosis

The Probe collects facts.

The Diagnosis judges the cause of failure from multiple facts.

For example:

```text
Probe A: Wi-Fi connected
Probe B: IP address obtained
Probe C: default gateway reachable
Probe D: public IP unreachable
Probe E: proxy local port connectable
Probe F: proxy authentication failed
```

This evidence must not be naively merged into "network failure".

A diagnosis result should include:

```text
Diagnosis
├── fault_code
├── summary
├── confidence (optional)
├── evidence_refs[]
├── affected_resources[]
├── recommended_actions[]
└── limitations[]
```

`confidence` is an optional rule-based confidence level and must not pretend to be a statistical probability.

## 5.2 Failure types

The first version should distinguish at least:

- `process_exited`
- `service_stopped`
- `probe_timeout`
- `network_disconnected`
- `gateway_unreachable`
- `dns_failure`
- `proxy_connection_failed`
- `proxy_authentication_failed`
- `remote_endpoint_unreachable`
- `plugin_unavailable`
- `dependency_unsatisfied`
- `recovery_action_failed`
- `verification_failed`
- `unknown_failure`

Unknown failures must not directly trigger every possible recovery action.

## 5.3 Consecutive failures and flapping suppression

A single timeout must not immediately restart a service.

Health policies need to support:

- Consecutive failure threshold.
- Consecutive success threshold.
- Probe timeout.
- Failure window.
- Recovery confirmation window.
- Fault cooldown.

For example:

```yaml
health_policy:
  interval: 10s
  timeout: 3s
  failure_threshold: 3
  success_threshold: 2
```

These are example values; actual values should be set per resource type.

Thresholds only decide whether a fault state is entered; they must not be used to paper over persistent anomalies.

## 5.4 P0 probe scope

P0 implements only:

- Process Probe.
- Exec Probe.

P0 does not implement an HTTP Probe yet.

The Process Probe checks whether a process exists plus any required process attributes.

The Exec Probe executes a controlled check command and parses:

- The exit code.
- Standard output.
- Standard error.
- Timeout.
- Startup failure.
- Whether output was truncated.

The Exec Probe must limit runtime and output size, and clean up the process tree on timeout. On Windows it must not kill only the parent process and leave orphaned children behind.

Network, HTTP, service management and proxy-specific probes come in later versions.

---

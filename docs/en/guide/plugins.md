---
title: Plugin Architecture
---

# 4. Plugin Architecture

## 4.1 Plugin responsibilities

Plugins encapsulate specific resources or capabilities; they do not implement an independent self-healing system of their own.

A plugin may provide:

- Resource Provider.
- Probe Provider.
- Diagnosis Provider.
- Recovery Action Provider.
- Verification Provider.
- Notification Channel Provider.

One plugin may provide multiple capabilities.

For example, the OpenClash plugin could provide:

- The OpenClash service resource.
- Proxy port probing.
- Proxy authentication verification.
- Node state diagnosis.
- Node switching actions.
- Service restart actions.
- Node recovery verification.

The Core Engine decides when these capabilities are invoked and whether execution is allowed.

## 4.2 Plugin execution model

The first version uses:

**Rust Core + external executable plugins + JSON-RPC 2.0 over stdio.**

Advantages:

- A plugin crash does not directly crash the core process.
- Plugins can be implemented in different languages.
- No need to design a stable Rust dynamic library ABI right away.
- The plugin protocol can be versioned independently.
- Easier to isolate plugin lifecycles and enforce timeouts.

But be clear about this: an external process does not automatically constitute a security sandbox. Plugins still require least privilege, controlled working directories, controlled environment variables and explicit operation authorization.

## 4.3 Plugin protocol

The first version needs at least:

```text
initialize
describe_capabilities
probe
diagnose
list_recovery_actions
execute_recovery_action
verify
shutdown
```

These are logical operation names; they should ultimately map to versioned JSON-RPC methods.

A request must carry:

- A JSON-RPC ID.
- The protocol version.
- The method name.
- Parameters.
- A request timeout.
- The associated Resource ID.
- The associated RecoveryRun ID, when applicable.

Recovery operations must use explicit action IDs with parameterized inputs. Piping arbitrary user input directly into Shell commands is forbidden.

## 4.4 Engineering requirements for stdio

On Windows, subprocess stdio pipes can block when their buffers fill up. The implementation must:

- Read stdout and stderr continuously and independently.
- Use stdout only for protocol messages.
- Use stderr for plugin diagnostic logs.
- Limit single-message size.
- Limit total buffer size.
- Support chunked reading.
- Handle incomplete messages.
- Handle plugins exiting early.
- Handle unresponsive plugins and timeouts.
- Support request cancellation.
- Return clear errors for malformed JSON and unknown methods.
- Never parse stderr as a protocol response.

For the first version, it is recommended to cap a single protocol message at 1 MiB, treat the exact value as a configurable safety limit, and handle large evidence via digest, truncation or chunking.

## 4.5 Plugin lifecycle

Long-lived plugin processes are recommended:

1. Custos starts.
2. Configured plugins are started.
3. Protocol handshake completes.
4. Capability declarations are obtained.
5. Protocol compatibility is verified.
6. Requests are accepted.
7. Plugin health is monitored.
8. On plugin crash, restart according to policy.
9. If a plugin cannot be recovered, mark related resources as not effectively probeable or operable.
10. Record the plugin failure and notify.

Plugins should not pay excessive overhead by re-creating a process for every probe.

## 4.6 Capability permissions

Every capability should declare:

- Read-only or state-modifying.
- Required permissions.
- The scope of resources affected.
- Whether it can interrupt the network.
- Whether it can interrupt remote management.
- Whether cancellation is supported.
- Whether it is idempotent.
- Whether verification is supported.
- Possible failure states.
- Whether compensation or rollback exists.

The Core Engine decides whether to execute based on configuration and policy.

A plugin declaring an action does not automatically make it allowed.

---

## Plugin model ruling (user decision, 2026-10-29)

- **One plugin = the complete monitoring and handling logic for one resource type**: probing, diagnosis, recovery actions and the verification loop live inside the same plugin, with no half-finished splits.
- **Plugins can be depended on by other plugins**: dependency direction matches the resource dependency graph, resolved uniformly by the Core Engine.
- **Consumers enable a plugin with simple configuration only**: declare the resource type and parameters, no glue code; configuration loading follows the configuration specification in §10.

## Builtin capabilities and Python script extension (user decision, 2026-10-29)

- **Builtin capabilities**: the core engine ships built-in basic adapters for common resources (services, processes, disks, network connectivity, etc.) that work out of the box — basic scenarios require no plugin development at all.
- **Python script extension (bidirectional)**:
  - Outbound: core functionality can be called from Python scripts via PyO3 bindings;
  - Inbound: custom probes and recovery actions can be written as Python scripts, loaded and executed by the core according to the plugin contract (timeouts, permissions and budgets follow the same rules as plugins).

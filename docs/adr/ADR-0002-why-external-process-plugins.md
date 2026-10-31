# ADR-0002: Why External-process Plugins in v1

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/plugins.md §4, ADR-0003

## Background

Plugins carry arbitrary probe and recovery logic (mihomo, cloudflared, OpenClash, custom scripts); author languages are unpredictable and plugin code quality is uncontrolled: they will crash, hang, or write malformed output.

## Decision

Plugins are independent child processes talking to the core via stdio (custos-plugin-host): `PluginSupervisor` handles spawn/handshake/capability validation/crash restart (rolling window + give-up marker); the core process never exits because a plugin died; after a crash the plugin's resources are marked unprobeable per §4.5 and a notification is raised.

## Alternatives

- **In-process dynamic library (dlopen/cdylib)**: one crash kills the core — isolation is zero; fragile ABI; directly conflicts with the P1 gate "plugin crash must not exit core."
- **Built-in only (no plugins)**: extensions require a core release; the community cannot contribute scenario-specific capabilities.
- **WASM**: good sandbox but host interfaces (file/process/network) all need a custom ABI — too costly for v1.

## Pros and Cons

Pros: physical fault isolation; any language that writes to stdout works; crash-restart policy evolves independently; clean permission boundary (plugins hold no core memory or credentials).
Cons: IPC serialization overhead (JSON line protocol); process lifecycle management is complex (zombies, stderr backpressure); handshake/version/capability validation requires protocol discipline.

## Follow-up Impact

- ADR-0003 (JSON-RPC over stdio) implements the protocol layer.
- Continuous stderr reader thread prevents pipe-full blocking (tests: chatty_stderr_is_drained_and_capped).
- The Python inbound script loader (todo P1) reuses the same contract: timeout/permission/budget identical.

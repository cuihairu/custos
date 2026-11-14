---
title: Cross-Platform Architecture
---

# 11. Cross-Platform Architecture

## 11.1 Unified platform interfaces

The following abstractions are suggested:

```text
ProcessController
ServiceController
NetworkInterfaceController
NetworkConnectivityProbe
CredentialProvider
RebootController
```

Each interface only defines cross-platform common semantics. Platform-specific capabilities are described through Capabilities; platform support must never be faked for the sake of a unified interface.

## 11.2 Windows

Capabilities to support progressively include:

- Process detection.
- Windows Service control.
- Wi-Fi state detection.
- Wi-Fi reconnection (built, not yet wired — see Capabilities for the
  current `unsupported` declaration).
- Network interface reset.
- DNS flush.
- IP configuration checks.
- Default gateway checks.
- Network connectivity checks.
- Sunlogin process or service checks.
- Authorized system reboot.

Wi-Fi reconnection involves credentials, network adapters and system privileges; use system-supported APIs or controlled commands.

If resetting a network adapter could interrupt the management connection, the risk must be explicitly marked in policy.

## 11.3 Linux

Needs to support:

- Process detection.
- systemd service control.
- OpenRC adaptation later.
- Network interface state.
- IP and route checks.
- DNS checks.
- Controlled command execution.
- System reboot.

Do not assume every Linux system uses systemd.

## 11.4 macOS

Later support:

- Process detection.
- launchd service management.
- Network interface state.
- Wi-Fi reconnection.
- Network connectivity checks.
- System reboot.

## 11.5 BSD

Later support:

- Process detection.
- rc.d service management.
- Network interface and route state.
- System reboot.

## 11.6 OpenWrt

Support (implemented in `crates/custos-platform/src/openwrt.rs`):

- procd service management.
- UCI configuration reading (`uci_get`); controlled modification is not yet exposed.
- ubus calls.
- Network interface and route checks.
- OpenClash state detection.
- Proxy node switching.
- Service restart.

Every network and firewall change must have a clear scope of effect, preconditions, risk level and verification steps.

Clearing firewall rules by default, resetting all routes, or unconditionally overwriting the existing network configuration is forbidden.

### LuCI interface (the luci-app-custos package)

The UI entry point lives under LuCI "Services → Custos". UI text is Chinese, hardcoded (no .po files):

- **Settings page** (CBI): service enable switch, database path, monitoring config file path (TOML), probe interval. Notification settings (Telegram / Herald / local log) are handed to the engine as `CUSTOS_NOTIFY_*` environment variables by the init script: when any channel selector is present the engine replaces the whole `[notification]` section of the TOML with that overlay and never writes the TOML back.
- **Live status**: five cards (service state, resource health, recent audit events, incidents, recovery runs) that auto-refresh every 10 s. The data comes from the `custos --json` CLI, sanitized before exposure, and rendered through DOM `textContent`.
- **Monitoring resources** (editor):
  - Resource list (id / type / dependencies / live health summary / plan count) with create, edit and delete.
  - Structured forms cover five types in the first batch — `network-interface` / `default-gateway` / `custom-command` / `icmp` / `service`: probe fields switch by type (interface name, command array, target host, service unit), plus health_policy (failure/success thresholds), the dependency list, and the recovery plan list (plan_id, applicable faults, risk level, idempotence, timeout, retries, cooldown, action program and args, verification condition).
  - Every other type goes through the "advanced raw TOML" textarea escape hatch (edit one `[[resources]]` block as a whole); format errors are rejected by the server-side `custos validate` and echoed back with line numbers.
  - The TOML file is the engine's authoritative configuration (single source, no UCI relay). Server side uses a minimal TOML subset reader/writer (`luci.custos.tomledit`, Lua 5.1): unknown keys and tables are preserved verbatim, untouched resource blocks round-trip byte-identically (round-trip diff on the real device's custos.toml is empty); field-level validation runs server side, semantic validation is delegated to `custos validate`.
  - Write-back safety: every write first backs up `custos.toml.bak` (one rolling copy), then the new text goes to a temp file and runs `custos validate`; only a pass replaces the live file atomically and restarts the service. A failure rolls back (the live file stays untouched) and shows the validator output, which carries the offending line numbers.
  - High-risk actions: a plan with `risk = high` needs the confirmation checkbox in the form (front-end hint plus server-side hard rejection), and that confirmation doubles as writing `allow_high_risk = true` on the resource (engine-side requirement). The editor ships no "clear the firewall / reset all routes" preset actions — action content is entirely user-entered.

---

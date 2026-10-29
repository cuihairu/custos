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
SystemRebootController
```

Each interface only defines cross-platform common semantics. Platform-specific capabilities are described through Capabilities; platform support must never be faked for the sake of a unified interface.

## 11.2 Windows

Capabilities to support progressively include:

- Process detection.
- Windows Service control.
- Wi-Fi state detection.
- Wi-Fi reconnection.
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

Later support:

- procd service management.
- UCI configuration reading and controlled modification.
- ubus calls.
- Network interface and route checks.
- OpenClash state detection.
- Proxy node switching.
- Service restart.

Every network and firewall change must have a clear scope of effect, preconditions, risk level and verification steps.

Clearing firewall rules by default, resetting all routes, or unconditionally overwriting the existing network configuration is forbidden.

---

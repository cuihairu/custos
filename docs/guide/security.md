---
title: Security and Permissions
---

# 12. Security and Permissions

## 12.1 Least privilege

Custos may need to manage services, the network and system processes, so the following must be clearly distinguished:

- Read-only probe permissions.
- Ordinary recovery permissions.
- Network configuration permissions.
- Service management permissions.
- System reboot permissions.
- Notification sending permissions.

It is never acceptable to hand every plugin full system permissions by default just for convenience.

## 12.2 High-risk actions

High-risk actions include:

- Resetting network interfaces.
- Modifying the routing table.
- Modifying the firewall.
- Switching proxy configuration.
- Rebooting the entire device.
- Modifying authentication information.

Every action must have:

- A risk declaration.
- Permission requirements.
- A scope of effect.
- A timeout.
- Concurrency control.
- Result verification.
- Failure handling.
- Optional rollback or compensation.

In the first version, the administrator may explicitly allow high-risk actions in configuration.

## 12.3 Safety when recovering from network interruption

Recovering the network requires special caution:

- Do not conclude the underlying network failed just because the proxy failed.
- Do not conclude the proxy is down from an HTTP 407.
- Do not attribute every connection timeout to DNS.
- Do not blindly reset the network while dependency state is unknown.
- Do not repeatedly restart network interfaces back to back.
- Do not treat a broken remote management channel as justification for unlimited retries.

When evidence is insufficient, record the reason and escalate the alert instead of trying every recovery action.

---

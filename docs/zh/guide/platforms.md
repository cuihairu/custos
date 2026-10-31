---
title: 跨平台架构
---

# 11. 跨平台架构

## 11.1 统一平台接口

建议抽象：

```text
ProcessController
ServiceController
NetworkInterfaceController
NetworkConnectivityProbe
CredentialProvider
SystemRebootController
```

每个接口只定义跨平台共同语义。平台专属能力通过 Capability 描述，不得为了统一接口而伪造平台支持。

## 11.2 Windows

需要支持的能力逐步包括：

- 进程检测。
- Windows Service 控制。
- Wi-Fi 状态检测。
- Wi-Fi 重连。
- 网络接口重置。
- DNS 刷新。
- IP 配置检查。
- 默认网关检查。
- 网络连通性检查。
- 向日葵进程或服务检查。
- 有授权的系统重启。

Wi-Fi 重连涉及凭据、网卡和系统权限，应使用系统支持的 API 或受控命令。

如果重置网卡可能导致管理连接中断，必须在策略中明确标记风险。

## 11.3 Linux

需要支持：

- 进程检测。
- systemd 服务控制。
- OpenRC 后续适配。
- 网络接口状态。
- IP 和路由检查。
- DNS 检查。
- 受控命令执行。
- 系统重启。

不能假设所有 Linux 系统都使用 systemd。

## 11.4 macOS

后续支持：

- 进程检测。
- launchd 服务管理。
- 网络接口状态。
- Wi-Fi 重连。
- 网络连通性检查。
- 系统重启。

## 11.5 BSD

后续支持：

- 进程检测。
- rc.d 服务管理。
- 网络接口与路由状态。
- 系统重启。

## 11.6 OpenWrt

后续支持：

- procd 服务管理。
- UCI 配置读取和受控修改。
- ubus 调用。
- 网络接口和路由检查。
- OpenClash 状态检测。
- 代理节点切换。
- 服务重启。

所有网络和防火墙变更必须有明确作用范围、前置条件、风险等级和验证步骤。

禁止默认清空防火墙规则、重置所有路由或无条件覆盖现有网络配置。

---

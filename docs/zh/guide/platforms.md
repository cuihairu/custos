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
RebootController
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

支持（实现在 `crates/custos-platform/src/openwrt.rs`）：

- procd 服务管理。
- UCI 配置读取（`uci_get`）；受控修改暂未开放。
- ubus 调用。
- 网络接口和路由检查。
- OpenClash 状态检测。
- 代理节点切换。
- 服务重启。

所有网络和防火墙变更必须有明确作用范围、前置条件、风险等级和验证步骤。

禁止默认清空防火墙规则、重置所有路由或无条件覆盖现有网络配置。

### LuCI 界面（luci-app-custos 包）

界面入口在 LuCI「服务 → Custos」，界面文案为中文硬编码（未做 .po）：

- **设置页**（CBI）：服务开关、数据库路径、监控配置文件路径（TOML）、探测间隔；通知设置（Telegram / Herald / 本地日志）经 init.d 以 `CUSTOS_NOTIFY_*` 环境变量透传给引擎，任何通道选择变量存在时引擎用该覆盖整体替换 TOML 的 `[notification]` 段，不回写 TOML。
- **运行状态**：服务状态、资源健康、最近审计事件、事故、恢复运行五张卡，10 秒自动刷新；数据来自 `custos --json` CLI，脱敏后透出，全部经 DOM textContent 渲染。
- **监控资源**（编辑器）：
  - 资源列表（id / 类型 / 依赖 / 实时健康摘要 / 计划数），支持新建、编辑、删除。
  - 结构化表单首批覆盖 `network-interface` / `default-gateway` / `custom-command` / `icmp` / `service` 五种类型：probe 字段按类型动态展示（接口名、命令数组、目标主机、服务单元），health_policy（失败/成功阈值），依赖列表，恢复计划列表（plan_id、适用故障、风险等级、幂等、超时、重试、冷却、动作程序与参数、验证条件）。
  - 其余类型走「高级 TOML 直编」textarea 逃生口（单个 `[[resources]]` 块整体编辑），格式错误由服务端 `custos validate` 拒绝并回显行号。
  - TOML 是引擎权威配置（单源，不经 UCI 中转）。服务端用极小 TOML 子集读写器（`luci.custos.tomledit`，Lua 5.1）：未识别的键/表原样保留，未改动的资源块字节级保真往返（真机 custos.toml round-trip diff 为空）；字段级校验在服务端，语义校验统一交给 `custos validate`。
  - 写回安全：写前自动备份 `custos.toml.bak`（滚动一个）；先写临时文件并跑 `custos validate`，通过才原子替换并重启服务；失败则回滚（线上文件不动）并显示校验错误（含行号）。
  - 高危动作：`risk = high` 的恢复计划必须在表单勾选确认（前端提示 + 服务端强制拒绝），确认同时作为该资源的 `allow_high_risk = true` 写入（引擎侧要求）。编辑器不提供任何「清空防火墙/重置路由」类预设动作，动作内容完全由使用者输入。

---

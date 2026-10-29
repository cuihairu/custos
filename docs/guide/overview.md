> 本文整理自 ChatGPT 分享文档（源：https://chatgpt.com/s/t_6ae30fee51e081918c88645c26817ff7，取回日期 2026-10-29）。外部建议口径，非本仓事实陈述；落地前须对照代码逐项核验。

---
title: 概述与架构决策
---

# 1. 项目目标与边界

## 1.1 项目要解决的问题

Custos 解决的是设备或服务发生故障后，需要人工介入才能恢复的问题，尤其是管理通道与故障本身存在依赖关系的场景。

典型场景：

1. Windows 电脑 Wi-Fi 断开，导致向日葵离线，需要自动恢复网络。
2. 代理节点异常，导致外网访问失败，需要切换节点或重启代理服务。
3. Cloudflare Tunnel 断线，但代理正常，只需要重启 cloudflared。
4. Cloudflare Tunnel 断线，同时代理异常，必须先恢复代理，再恢复 Tunnel。
5. Linux 服务异常，根因可能是依赖服务、网络或本机资源。
6. OpenClash 节点异常，需要诊断连接状态并有序切换节点。
7. 自动恢复连续失败，必须记录完整证据并通知用户。

共同特点：

- 故障发生时，远程管理通道可能已经不可用。
- 设备必须在本地独立完成检测与恢复。
- 单个服务的存活状态不足以证明其正常。
- 修复动作之间存在前置条件和依赖关系。
- 修复动作可能执行成功，但业务依然不可用。
- 网络中断时，通知服务可能暂时无法使用。
- 用户需要在事后查看故障原因、处理过程和最终结果。

## 1.2 产品目标

Custos 应当提供：

- 本地持续监控。
- 资源和依赖关系管理。
- 故障检测与根因诊断。
- 依赖感知的恢复编排。
- 有序候选方案和失败后的重新诊断。
- 恢复后的健康验证。
- 崩溃可恢复的任务状态持久化。
- 完整的结构化审计记录。
- 可扩展的插件运行时。
- 常见通知渠道的内置实现。
- 第三方通知渠道扩展。
- 默认关闭的可选 Web Dashboard。
- Windows、Linux、macOS，以及后续 BSD、OpenWrt 的平台适配能力。

## 1.3 明确不做的事情

第一阶段不实现：

- 云端必需的控制平面。
- 多租户 SaaS 运维平台。
- 通用远程桌面和远程命令执行平台。
- 软件包管理与自动升级平台。
- 任意脚本的无限制自动执行。
- 默认监听公网的管理 API。
- 必须通过网络才能工作的监控和恢复流程。
- 插件市场、插件自动下载和自动安装。
- 将所有平台行为强行抽象成完全一致的接口。

Custos 的定位是本地自治的自愈引擎，而不是另一套大型运维平台。

---

# 2. 核心架构决策

## 2.1 统一领域模型

Custos 使用以下核心概念：

| 概念 | 职责 |
|---|---|
| Resource | 被管理的资源，例如 Wi-Fi、代理、Cloudflare Tunnel、进程或业务入口 |
| Dependency | 资源之间的依赖关系和满足条件 |
| Probe | 采集资源健康状态的能力 |
| Evidence | 探测结果、错误、时间戳及相关证据 |
| Diagnosis | 根据证据得出的故障判断 |
| RecoveryPolicy | 决定如何处理故障的策略 |
| RecoveryAction | 可以执行的恢复动作 |
| Verification | 判断恢复是否真正成功 |
| RecoveryRun | 一次完整的故障处理过程 |
| Plugin | 封装具体资源、探测、恢复或通知能力的扩展模块 |
| Notification | 需要通知用户或外部系统的事件 |
| AuditEvent | 可追溯的系统事件 |
| RecoveryStore | 保存状态、证据、动作和审计数据的持久化组件 |

必须保持以下区别：

**Resource 是对象，Dependency 是关系，Plugin 是能力实现，RecoveryPolicy 是策略，RecoveryRun 是一次执行实例。**

不能将这些概念混为一谈。

## 2.2 组件架构

```mermaid
flowchart TB
    CLI[Custos CLI]
    Dashboard[Optional Web Dashboard]
    Config[Configuration Loader]

    subgraph Core[\

Host[Plugin Host]
        Probe[Probe Capabilities]
        Diagnose[Diagnosis Capabilities]
        Actions[Recovery Actions]
        Channels[Notification Channels]
    end

    Store[(SQLite Recovery Store)]
    Notify[Notification Engine]
    OS[Platform Adapters]
    External[External Systems]

    CLI --> Core
    Dashboard --> Core
    Config --> Registry
    Registry --> Graph
    Scheduler --> Engine
    Graph --> Engine
    Engine --> Policy
    Policy --> Host
    Host --> Probe
    Host --> Diagnose
    Host --> Actions
    Host --> OS
    Engine --> Verify
    Verify --> Events
    Engine --> Events
    Events --> Store
    Events --> Notify
    Notify --> Channels
    Channels --> External
    Core --> Store
```

Dashboard 只是一个可选客户端。即使完全不编译 Dashboard，核心引擎也必须具备全部自愈能力。

CLI 是首要的管理接口。

## 2.3 核心运行原则

1. Core Engine 控制整个恢复生命周期。
2. 插件提供能力，不拥有全局恢复决策权。
3. 所有恢复动作必须经过策略、权限和预算检查。
4. 任何动作完成后都需要重新评估资源健康状态。
5. 依赖恢复完成后，必须重新检测依赖，而不是假设恢复成功。
6. 每个任务和关键状态转换必须持久化。
7. 通知失败不得阻塞恢复。
8. Dashboard 故障不得影响恢复。
9. 结果未知的动作不得盲目重复执行。
10. 恢复任务必须具备明确的终止条件。

---

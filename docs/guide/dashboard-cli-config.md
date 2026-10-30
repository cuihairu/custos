---
title: Dashboard、CLI 与配置
---

# 9. Web Dashboard 与 CLI

## 9.1 Dashboard 默认关闭

Dashboard 是可选组件，默认关闭；未启用时零监听端口、零 HTTP 代码路径。

目标配置形态（P4 之前不会实现）：

```toml
[dashboard]
enabled = false
listen = "127.0.0.1:8080"
```

启用前提与验收约束：

1. 关闭时不得监听任何端口。
2. Dashboard 不可用不得影响核心引擎运行。
3. Dashboard 不得绕过 Core Engine 直接执行恢复。
4. 高风险操作仍必须走统一的授权与审计。
5. 传输安全（认证、绑定地址、CSRF）随工具箱集中硬化（ADR-0015）。

## 9.2 CLI 命令

CLI 是 P0/P1 的操作面（已实现）：

```text
custos validate <CONFIG>           解析并校验配置（拒绝重复 ID、悬空引用、依赖环）
custos start <CONFIG> [OPTIONS]    运行引擎循环（--db PATH、--interval MS、--once）
custos status [--db PATH]          存储计数与最近审计事件
custos runs [--db PATH]            最近恢复任务（新→旧）
custos incidents [--db PATH]       事件（incident）历史
custos notify status [--db PATH]   通知队列五状态计数与最近条目
custos notify retry <ID> [--db]    手动重排一条死信通知
custos resources <CONFIG>          列出配置的资源
custos graph <CONFIG>              依赖图（拓扑序）
```

`start` 启动时先做崩溃恢复（未终态运行标记 outcome-unknown，绝不盲目重试），
再进入探测/恢复 tick 循环；通知引擎随进程接线（事件入队 + 后台投递，断网补发）。

---

# 10. 配置规范

## 10.1 基本原则

配置必须：

- 可验证。
- 可版本化。
- 能提供明确错误路径。
- 不允许未知字段被静默解释成有效配置。
- 支持安全的凭据引用。
- 支持资源之间的显式依赖。
- 支持资源级恢复策略。
- 支持全局预算和通知规则。

首版采用 TOML（单一主格式，`version = 1`）。

## 10.2 示例：Cloudflare Tunnel 依赖代理

以下为目标配置形态，展示架构语义；不代表第一版已有对应插件实现。

```yaml
version: 1

resources:
  - id: proxy.egress
    type: proxy
    plugin: mihomo
    config:
      endpoint: http://127.0.0.1:7890

  - id: tunnel.egress
    type: tunnel
    plugin: cloudflared
    config:
      entry: https://api.example.com
    dependencies:
      - resource: proxy.egress
        requirement: required

notification:
  rules:
    - event: recovery.exhausted
      severity: critical
      channels:
        - primary-webhook
```

需要注意：

1. 代理健康检查应尽量区分代理认证失败、连接失败和目标服务异常。
2. 如果代理并非 Tunnel 的必要依赖，则不应声明 Required。
3. HTTP 探测和上述特定插件属于后续阶段能力。
4. 第一版的配置验证器可以先定义完整配置模型，但未实现的资源类型必须报出清楚的能力缺失错误，不能假装已经运行。
5. 恢复动作必须与插件能力声明匹配。

## 10.3 配置加载过程

1. 解析配置。
2. 验证版本。
3. 验证结构。
4. 验证资源 ID。
5. 验证依赖引用。
6. 验证依赖图。
7. 验证恢复动作引用。
8. 验证插件能力。
9. 验证权限与风险策略。
10. 验证通知渠道配置。
11. 验证数据库迁移兼容性。
12. 创建或更新运行时资源注册表。

配置验证失败时必须指出具体字段和原因。

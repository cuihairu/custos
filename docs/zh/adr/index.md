---
title: 架构决策记录（ADR）
---

# 架构决策记录

每条 ADR 记录一个已接受的架构决策：背景、决策、替代方案、优缺点与后续影响。
模板与决策清单见 [ADR 与最终实施原则](/guide/adr-principles) §17。

| 编号 | 决策 |
|---|---|
| [ADR-0001](/adr/ADR-0001-why-rust) | 为什么选择 Rust |
| [ADR-0002](/adr/ADR-0002-why-external-process-plugins) | 为什么第一版采用外部进程插件 |
| [ADR-0003](/adr/ADR-0003-why-jsonrpc-over-stdio) | 为什么选择 JSON-RPC over stdio |
| [ADR-0004](/adr/ADR-0004-why-resource-dependency-graph) | 为什么采用 Resource + Dependency Graph |
| [ADR-0005](/adr/ADR-0005-why-core-coordinates-the-graph) | 为什么依赖图由 Core Engine 协调 |
| [ADR-0006](/adr/ADR-0006-why-independent-verification) | 为什么动作结果必须经过独立验证 |
| [ADR-0007](/adr/ADR-0007-why-sqlite) | 为什么使用 SQLite |
| [ADR-0008](/adr/ADR-0008-why-persist-before-high-risk-actions) | 为什么高风险动作前必须持久化 |
| [ADR-0009](/adr/ADR-0009-why-notification-decoupled) | 为什么通知系统与恢复引擎解耦 |
| [ADR-0010](/adr/ADR-0010-why-dashboard-off-by-default) | 为什么 Dashboard 默认关闭 |
| [ADR-0011](/adr/ADR-0011-why-p0-only-process-exec-probes) | 为什么 P0 只实现 Process 和 Exec Probe |
| [ADR-0012](/adr/ADR-0012-why-unauthorized-high-risk-network-actions-are-forbidden) | 为什么不允许未经授权的高风险网络动作 |
| [ADR-0013](/adr/ADR-0013-why-unknown-outcomes-are-never-blindly-retried) | 为什么未知执行结果不能直接重试 |
| [ADR-0014](/adr/ADR-0014-why-ordinary-dependency-cycles-are-forbidden) | 为什么禁止普通依赖图循环 |
| [ADR-0015](/adr/ADR-0015-internal-toolbox-plugin-rule-layer) | 为什么核心维护内部工具箱，插件只是规则层 |

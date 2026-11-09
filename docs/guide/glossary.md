---
title: Source-Layer Glossary
---

# Source-layer glossary

This table records concepts abstracted from the implementation, not module names. Each row points to the representative declaration; the source line is the best current anchor and may move during later refactors.

| 术语 | English | 定义 | file:line |
|---|---|---|---|
| 配置边界 | Validated configuration | 启动前解析并校验的 TOML 输入：禁止未知字段、重复 ID、悬空依赖和依赖环，输出资源、替代组、图、权限、通知与仪表盘配置 | `crates/custos-config/src/lib.rs:320` |
| 探测规则 | Probe schema | 每类资源探测参数的严格输入契约，原样进入 `Resource.config`，运行期由探测工厂解释 | `crates/custos-config/src/lib.rs:56` |
| 通知规则 | Notification rule | 按事件、最低严重级、资源、通道与去重窗口筛选通知的声明式规则 | `crates/custos-config/src/notification/mod.rs:33` |
| 通知通道 | Notification channel | local-log、webhook、SMTP、Herald 或 Telegram 的类型化外发通道，以及超时、重试、退避和密钥策略 | `crates/custos-config/src/notification/mod.rs:54` |
| 资源 | Resource | 被管理的可观测对象，携带类型、健康策略、恢复策略和探测配置 | `crates/custos-model/src/resource.rs:45` |
| 依赖 | Dependency | 一个资源对另一个资源的关系声明，包含满足条件、等待策略和激活模式 | `crates/custos-model/src/dependency.rs:56` |
| 替代组 | Alternative group | 一组可互换资源的最小健康数量约束，例如任一可用代理或两个可用出口 | `crates/custos-model/src/dependency.rs:102` |
| 依赖图 | Dependency graph | 有向依赖关系图；配置期校验引用、依赖环并生成依赖优先的拓扑序 | `crates/custos-model/src/graph.rs:11` |
| 健康状态 | Health status | Healthy、Degraded、Unhealthy、Unreachable 与 Unknown 的统一探测判定词汇 | `crates/custos-model/src/health.rs:10` |
| 证据 | Evidence | 一次探测产生的资源、探测名、结果、观察值与时间戳事实 | `crates/custos-model/src/evidence.rs:9` |
| 诊断 | Diagnosis | 从证据归因出的故障码、摘要、受影响资源和证据引用 | `crates/custos-model/src/diagnosis.rs:29` |
| 事件 | Incident | 从确认故障到恢复、抑制或升级的故障生命周期 | `crates/custos-model/src/incident.rs:11` |
| 恢复计划 | Recovery plan | 适用于特定故障码的动作、风险、幂等性、验证条件与失败策略组合 | `crates/custos-model/src/recovery.rs:54` |
| 恢复运行 | Recovery run | 一次故障处理实例和显式状态机；终态之外的所有关键转换必须持久化 | `crates/custos-model/src/recovery.rs:181` |
| 恢复预算 | Recovery budget | 运行并发、尝试数、运行时长、重启率与高风险次数的上限和记账状态 | `crates/custos-engine/src/budget.rs:59` |
| 协调循环 | Coordination loop | 依赖优先探测、失败诊断、依赖前置、计划选择、执行、健康验证、等待唤醒和崩溃续跑的主循环 | `crates/custos-engine/src/engine.rs:196` |
| 恢复执行器 | Action executor | 将动作翻译为实际调用的边界；内置执行器只能执行本机命令，验证必须来自探测而非退出码 | `crates/custos-engine/src/executor.rs:10` |
| 运行仓储 | Run store | 引擎写运行、尝试、验证、探测、诊断、事件、步骤和未完成运行的抽象持久化端口 | `crates/custos-engine/src/events.rs:130` |
| 运行时装配 | Runtime assembly | 唯一将配置、探测、引擎、SQLite 仓储、通知引擎和系统时钟连接起来的装配点，CLI 与 Python 共用 | `crates/custos-core/src/assemble.rs:58` |
| 原生探测 | Builtin probe | 在主进程内运行的资源观测器，返回归一化探测报告但不决定恢复动作 | `crates/custos-probe/src/lib.rs:70` |
| 探测调度 | Probe due tracker | 每资源一次到期时间簿记，使单个 tick 只探测到期资源 | `crates/custos-probe/src/lib.rs:86` |
| 能力声明 | Capability | 插件握手后发布的操作 ID、类型与所需权限，宿主据此建立可调用面 | `crates/custos-plugin-api/src/capability.rs:38` |
| 插件协议 | JSON-RPC protocol | 插件进程和宿主之间换行分帧的 JSON-RPC 请求/响应、握手与探测/诊断/恢复/验证方法 | `crates/custos-plugin-api/src/rpc.rs:23` |
| 插件会话 | Plugin session | 子进程的 stdin/stdout、并发请求、输出上限、超时、stderr 尾巴和协议错误边界 | `crates/custos-plugin-host/src/session.rs:101` |
| 插件监督 | Plugin supervisor | 插件进程生命周期：启动、握手、崩溃预算、重启、降级与诊断保留 | `crates/custos-plugin-host/src/supervisor.rs:66` |
| 能力表 | Tool registry | 内置工具与插件操作的统一派发表，执行前按所需权限拒绝未授权操作 | `crates/custos-toolbox/src/registry.rs:87` |
| 操作权限 | Operation permission | 每次 dispatch 请求的能力级权限断言；拒绝必须发生在任何执行前 | `crates/custos-toolbox/src/registry.rs:181` |
| 平台控制器 | Platform controller | 进程、服务、网络接口、连通性、凭据和重启的跨平台语义接口，由具体 OS 适配器实现 | `crates/custos-platform/src/controller.rs:16` |
| 平台能力 | Platform capability set | OS 适配器显式声明可支持的操作；不支持必须报告 Unsupported，不得猜测执行 | `crates/custos-platform/src/capability.rs:19` |
| 通知入队 | Notification dispatch | 恢复事件进入通知引擎的路径：规则匹配、去重并写入队列，不阻塞恢复 | `crates/custos-notify/src/engine.rs:137` |
| 通知队列 | Notification queue | SQLite 中的 at-least-once 投递队列，记录 pending、sending、delivered、retry 和 dead 状态 | `crates/custos-store/src/store.rs:609` |
| 通知投递 | Notification delivery | 后台批量 claim 到期条目，通过通道发送并按结果标记完成、重试或死信 | `crates/custos-notify/src/engine.rs:225` |
| 持久化存储 | SQLite store | 运行、尝试、验证、审计、健康、事件、步骤、探测、诊断、通知、插件注册与配置版本的唯一磁盘状态库 | `crates/custos-store/src/store.rs:34` |
| 只读仪表盘 | Dashboard | 默认关闭、仅回环、读取 SQLite 并拒绝远程绑定的 HTTP 观察面 | `crates/custos-dashboard/src/lib.rs:26` |
| 托盘观察 | Tray observer | 独立进程读取同一 SQLite 文件并显示健康/事件菜单，不参与恢复决策 | `crates/custos-tray/src/main.rs:137` |
| Python 门面 | Python binding | 面向嵌入方的 `run_once` 等函数，经同一运行时装配执行并返回 JSON 摘要 | `crates/custos-python/src/lib.rs:262` |

Related pages:

- [Source architecture](./source-architecture.md)
- [Resource and Dependency Model](./resources.md)
- [Recovery Engine](./recovery-engine.md)
- [Persistence and Crash Consistency](./persistence.md)
- [Plugin Architecture](./plugins.md)
- [Notification Engine](./notification.md)

---
title: 源码架构
---

# 源码架构

这是 Custos 的实现地图。层表示职责：一个层可能横跨多个 crate，一个 crate 也可能暴露多个层。

## 1. 配置与领域契约层

**职责：** 在任何运行期副作用之前，把操作者输入变成已校验的领域对象和共享词汇。

- **实现：** `crates/custos-config/src/lib.rs` 解析严格 TOML，校验引用、替代组、恢复计划和权限，然后构造 `ValidatedConfig`、`Resource`、`Dependency` 与 `DependencyGraph`（`crates/custos-config/src/build.rs:42`）。
- **领域模型：** 资源/依赖/恢复/事件类型在 `crates/custos-model`；依赖图负责环检测和拓扑序（`crates/custos-model/src/graph.rs:35`）。
- **配置 schema：** 顶层未知字段边界是 `RawConfig`（`crates/custos-config/src/schema.rs:254`）；所有探测字段声明在 `RawProbe`（`crates/custos-config/src/schema.rs:17`）。
- **专属协议文件：** 无；该层使用 TOML 和可序列化领域结构。
- **文案挂载：** `docs/zh/guide/resources.md`、`docs/zh/guide/dashboard-cli-config.md` 与 `docs/zh/guide/overview.md` §2。

## 2. 协调与恢复状态层

**职责：** 拥有观测调度、依赖排序、故障诊断、恢复状态转换、预算和最终健康判定。

- **实现：** `Engine` 持有托管资源、拓扑探测序、活动运行、等待项和事件 sink（`crates/custos-engine/src/engine/mod.rs:49`）；`tick` 按依赖序探测到期资源（`crates/custos-engine/src/engine/orchestrate.rs:9`）；恢复从 `try_recover` 开始（`crates/custos-engine/src/engine/recover.rs:8`）。
- **状态：** 进程内 `ManagedResource`、快照、活动运行集合和等待项；持久运行状态通过 `RunStore` 委托（`crates/custos-engine/src/events.rs:130`）。
- **预算：** 并发、尝试数、运行时长、重启率与高风险次数在执行前检查并记账（`crates/custos-engine/src/budget.rs:59`）。
- **配置 schema：** 该层消费校验后的图和每资源恢复策略；没有单独协议文件。
- **文案挂载：** `docs/zh/guide/recovery-engine.md` 与 `docs/zh/guide/security.md`。

## 3. 能力发现层

**职责：** 归一化观测但不决定恢复，并为原生与外部能力提供同一个可审计权限边界。

- **原生探测：** `Probe::probe` 返回归一化报告，绝不选择动作（`crates/custos-probe/src/lib.rs:70`）。`custos-core::build_probes` 在启动时把每个配置资源变成内置探测（`crates/custos-core/src/assemble.rs:42`）。
- **协议：** 原生探测没有 wire 协议；每个探测读取 `Resource.config` 的类型化切片。
- **插件协议：** 换行分帧 JSON-RPC 方法和能力声明定义在 `crates/custos-plugin-api/src/rpc.rs:23` 与 `crates/custos-plugin-api/src/capability.rs:38`。宿主会话对双向管道做输出上限和超时（`crates/custos-plugin-host/src/session.rs:101`）；示例插件调用 `crates/custos-plugin-api/src/server.rs:32` 的 `serve`。
- **权限/派发：** 内置工具与插件操作在 `ToolRegistry` 汇合；权限先于内置工具或插件执行（`crates/custos-toolbox/src/registry.rs:181`）。
- **配置 schema：** 插件可执行文件和资源探测参数通过主 TOML 契约输入；外部控制器配置由示例插件自行读取（`plugins/mihomo/src/main.rs:28`）。
- **文案挂载：** `docs/zh/guide/plugins.md`、`docs/zh/guide/probe-survey.md`、`docs/zh/guide/security.md` 与 `docs/zh/adr/ADR-0003-why-jsonrpc-over-stdio.md`。

## 4. 执行与平台层

**职责：** 通过受控 OS 原语执行被选中的本机动作，同时保留能力元数据和验证边界。

- **恢复边界：** `ActionExecutor` 暴露 execute 和 verify；内置执行器运行配置 argv，验证必须是探测而不是退出码（`crates/custos-engine/src/executor.rs:10`）。
- **OS 接口：** 进程、服务、网络接口、连通性、凭据和重启契约声明在 `crates/custos-platform/src/controller.rs:16` 至 `crates/custos-platform/src/controller.rs:104`；适配器实现 systemd/Windows SCM/netsh/reboot 路径（`crates/custos-platform/src/linux.rs:142`、`crates/custos-platform/src/windows.rs:165`）。
- **能力门：** 每个适配器显式报告可支持的操作；不支持必须是显式错误（`crates/custos-platform/src/capability.rs:19`）。
- **配置 schema：** 平台操作没有自己的 TOML schema；操作 ID 与风险/权限语义由工具和恢复计划提供。
- **文案挂载：** `docs/zh/guide/platforms.md` 与 `docs/zh/adr/ADR-0012-why-unauthorized-high-risk-network-actions-are-forbidden.md`。

## 5. 持久化与事件层

**职责：** 在恢复前、中、后保存证据和运行状态，并把同一个持久队列暴露给观察者。

- **引擎端口：** `RunStore` 持久化运行、尝试、验证、证据、诊断、事件、步骤和未完成运行（`crates/custos-engine/src/events.rs:130`）。
- **SQLite 实现：** `Store::open` 建立共享数据库（`crates/custos-store/src/store/lifecycle.rs:8`），同时实现 `RunStore` 与 `NotificationQueue`（`crates/custos-store/src/store/runs.rs:5`、`crates/custos-store/src/migrations.rs:180`）。
- **Schema：** 编号迁移创建持久表（`crates/custos-store/src/migrations.rs:61`）；迁移本身记录在 `crates/custos-store/src/migrations.rs:249`。
- **状态：** 仅文件型 SQLite。仓库中没有 Redis、MySQL 或守护进程级缓存状态。
- **文案挂载：** `docs/zh/guide/persistence.md` 与 `docs/zh/adr/ADR-0007-why-sqlite.md`。

## 6. 通知投递层

**职责：** 把用户可见报告从恢复中解耦，避免外发故障阻塞自愈，并让重试可审计。

- **实现：** 通知引擎匹配规则、抑制重复并入队（`crates/custos-notify/src/engine.rs:203`）；后台 worker 认领到期条目并发送（`crates/custos-notify/src/engine.rs:293`）。
- **通道：** webhook、SMTP、Herald、Telegram 和本地日志实现 `Channel` 端口（`crates/custos-notify/src/channel.rs:49`）；webhook 出站另有 SSRF 策略边界（`crates/custos-config/src/notification/mod.rs:114`）。
- **状态：** 通道配置是内存中的已校验配置；队列与尝试记录是 SQLite 状态。
- **配置 schema：** 通道判别表和规则在 `crates/custos-config/src/notification/mod.rs:54` 与 `crates/custos-config/src/notification/mod.rs:33`；密钥使用脱敏包装（`crates/custos-config/src/notification/mod.rs:236`）。
- **文案挂载：** `docs/zh/guide/notification.md` 与 `docs/zh/adr/ADR-0009-why-notification-decoupled.md`。

## 7. 运行时装配与进程生命周期层

**职责：** 只组合一次全部层，然后以崩溃安全语义运行/监督进程。

- **装配：** `assemble` 是探测、SQLite、通知、引擎、内置执行器和时钟的唯一构造点（`crates/custos-core/src/assemble.rs:59`）；CLI 启动先续跑未完成运行，再启动通知投递 worker（`crates/custos-cli/src/main.rs:221`、`crates/custos-cli/src/main.rs:243`）。
- **监督：** 插件重启预算、握手和降级状态隔离在 `PluginSupervisor`（`crates/custos-plugin-host/src/supervisor.rs:66`）。
- **状态：** 引擎和监督器的进程内状态有界且进程本地；崩溃持久状态在 SQLite。进程同时拥有两个 worker；没有 Redis 租约或 MySQL 协调器。
- **配置 schema：** CLI 选项在 `crates/custos-cli/src/main.rs:128` 解析；存储策略类型定义在 `crates/custos-store/src/config.rs:21`。
- **文案挂载：** `docs/zh/guide/workspace.md` 与 `docs/zh/adr/ADR-0013-why-unknown-outcomes-are-never-blindly-retried.md`。

## 8. 接口与观察者层

**职责：** 暴露读取、内省、控制面，但不允许客户端绕过协调、权限或持久状态。

- **CLI：** 命令面负责校验、内省、启动、查看运行/事件/资源/图（`crates/custos-cli/src/main.rs:50`）；主循环拥有 tick 节奏和通知 worker 生命周期（`crates/custos-cli/src/main.rs:273`）。
- **Dashboard：** 可选、默认关闭、仅回环且只读（`crates/custos-dashboard/src/lib.rs:26`；路由在 `crates/custos-dashboard/src/lib.rs:180`）。
- **托盘：** 独立进程读取同一 SQLite 文件并渲染系统菜单状态（`crates/custos-tray/src/main.rs:25`）。
- **Python 绑定：** 嵌入方调用 `run_once`，它走同一个 `assemble` 路径（`crates/custos-python/src/lib.rs:259`）。
- **配置 schema：** dashboard 字段在 `crates/custos-config/src/dashboard.rs`；dashboard 自身没有写 API。
- **文案挂载：** `docs/zh/guide/dashboard-cli-config.md`、`docs/zh/adr/ADR-0010-why-dashboard-off-by-default.md`，以及首页架构图。

## 拆分理由

1. **先拒绝非法拓扑再执行。** 配置解析在启动时构造并校验依赖图，避免未知资源和依赖环变成恢复期递归（`crates/custos-model/src/graph.rs:59`；`crates/custos-config/src/build.rs:42`）。
2. **能力提供者不进入控制流。** 探测只返回观测（`crates/custos-probe/src/lib.rs:70`），而引擎拥有依赖解析、计划选择、执行和最终健康判定（`crates/custos-engine/src/engine/orchestrate.rs:9`；`crates/custos-engine/src/engine/recover.rs:8`）。
3. **原生与外部能力在同一个权限边界归一。** 内置工具和插件操作注册进一张表，拒绝发生在任一实现运行之前（`crates/custos-toolbox/src/registry.rs:181`），插件子进程无法获得未审查控制路径。
4. **OS 执行可替换但不承载策略。** 恢复通过 `ActionExecutor` 委托执行（`crates/custos-engine/src/executor.rs:10`），具体 OS 能力声明并实现在平台契约后（`crates/custos-platform/src/controller.rs:41`；`crates/custos-platform/src/capability.rs:19`）。
5. **风险转换前后都持久化。** 引擎不直接依赖 SQL，而是通过 `RunStore` 写入（`crates/custos-engine/src/events.rs:130`）；SQLite 在同一个文件中实现持久运行与通知队列（`crates/custos-store/src/store/runs.rs:5`；`crates/custos-store/src/migrations.rs:180`）。
6. **报告失败不进入恢复路径。** 恢复只向非阻塞通知 sink 发事件（`crates/custos-core/src/assemble.rs:30`）；通知规则入队（`crates/custos-notify/src/engine.rs:203`），后续由独立 worker 投递（`crates/custos-notify/src/engine.rs:293`）。
7. **隔离崩溃影响范围。** 插件进程死亡由有界重启/降级监督器吸收（`crates/custos-plugin-host/src/supervisor.rs:66`）；被打断的引擎运行标记为未知，只通过显式 resume 逻辑重驱（`crates/custos-engine/src/engine/orchestrate.rs:224`）。
8. **管理面失败不影响可观测性。** 引擎循环独立于可选 CLI/dashboard/tray/Python 调用者；dashboard 只在启用时绑定（`crates/custos-cli/src/main.rs:199`），托盘读文件而不拥有引擎状态（`crates/custos-tray/src/main.rs:25`）。

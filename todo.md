# Custos 实施待办（原子清单）

> 依据 `docs/guide/`（roadmap §14、agent-protocol §15.4、adr-principles §17、plugins.md 用户裁定 2026-10-29）拆解。
> 规则：一项 = 一个可独立验收的交付物；完成后打勾并在行尾记 commit hash；阶段验收门全绿才勾 roadmap.md「落地跟踪」对应行。
> 骨架基线（commit `de3aa0d`）：workspace 11 crate、领域模型主体、状态机转换表、依赖图环检测、配置校验 v1、CLI `validate`。

## 0. 基线（已落地）

- [x] Cargo workspace 11 crate 骨架（custos-{model,core,engine,probe,plugin-api,plugin-host,platform,config,store,notify,cli}）`de3aa0d`
- [x] ID newtype：ResourceId / IncidentId / RecoveryRunId / ActionId（serde + Display）`de3aa0d`
- [x] HealthStatus 七态（Unknown…Unreachable，Unreachable ≠ Unhealthy）`de3aa0d`
- [x] RecoveryRun 状态机合法转换表 + `transition_to` 校验（非法转换报错）`de3aa0d`
- [x] DependencyGraph：悬空引用检测 + DFS 环检测（报错带完整路径）+ 拓扑序 `de3aa0d`
- [x] 配置加载 v1：版本/唯一 ID/依赖引用/环/未知字段拒绝（deny_unknown_fields）`de3aa0d`
- [x] CLI `custos validate <config>`：启动即查环，有环 exit 1 `de3aa0d`

## P0 核心恢复引擎

### 领域模型补全（custos-model）

- [x] VerificationResult 独立类型（与 ActionOutcome 分离，含判定依据 evidence_refs） `1c5609f`
- [x] RecoveryPolicy 候选方案完整建模：唯一 ID/适用故障/前置条件/风险等级/超时/最大尝试/冷却/幂等声明/验证条件/失败处理/可选补偿（§6.3 十一项） `1c5609f`
- [x] 候选方案 serde 反序列化测试（十一个字段全覆盖） `1c5609f`
- [x] HealthCondition 扩展：能力可用 / 最近成功探测时限 / 连续成功次数 / 业务检查（当前仅 StatusIs、ProbedWithin） `1c5609f`
- [x] AlternativeGroup 判定：N 选 M 语义 + 单元测试 `1c5609f`
- [x] DependencyRequirement::Conditional 条件求值语义 + 单元测试 `1c5609f`
- [x] Evidence↔Diagnosis 关联字段（evidence_refs 引用完整性） `1c5609f`
- [x] HealthStatus 抖动抑制所需状态字段：连续成功/失败计数、最近有效探测时间、活动故障标识 `1c5609f`
- [x] Waiting 状态语义：挂起原因 + 恢复去向，转换测试 `2c809f6`

### 配置（custos-config）

- [x] 配置 Schema 完整化：health_policy / recovery_policy / permissions 节点 `20984cd`
- [x] 候选方案引用验证：recovery_policy 中动作 ID 存在且与资源类型匹配 `20984cd`
- [x] 不可能满足依赖条件检测（§3.5 第 5 条：条件永假报配置错误） `1c5609f`
- [x] AlternativeGroup 配置合法性验证（§3.5 第 4 条） `1c5609f`
- [x] 配置错误输出：具体字段路径 + 原因（§10.3，全错误类型统一） `1c5609f`
- [x] 示例配置三份落盘：examples/basic-process.toml、dependency-chain.toml、notifications.toml `1c5609f`

### 探测（custos-probe）

- [x] Probe trait + 探测结果归一化（Evidence 产出） `278e747`
- [x] Process Probe：进程存在 + 必要属性检查 `278e747`
- [x] Exec Probe：受控命令执行，解析退出码/stdout/stderr/超时/启动失败/截断六种结果 `278e747`
- [x] Exec Probe：运行时间限制 + 输出大小限制 `278e747`
- [x] Exec Probe：超时清理进程树（Windows 不留孤儿子进程） `278e747`
- [x] 内置 process / custom-command 资源适配（用户裁定「集成基础能力」P0 部分） `278e747`
- [x] 探测调度：按 interval 周期执行 + 手动触发 `278e747`

### 引擎（custos-engine）

- [x] RecoveryRun 执行器骨架：驱动状态机从 Observing 走到终态 `278e747`
- [x] 连续失败/成功阈值 → 健康状态转换（failure_threshold/success_threshold 生效） `278e747`
- [x] 故障确认 → Incident 创建（同一故障多次尝试归属同一 Incident） `278e747`
- [x] 诊断管道：Evidence 聚合 → Diagnosis(fault_code + confidence + affected) `278e747`
- [x] 依赖解析：恢复 A 前先评估依赖新鲜度，过期重新探测（ResolvingDependencies） `278e747`
- [x] 依赖恢复后重新探测验证，不假设成功（§2.3 第 5 条） `278e747`
- [x] 有序候选方案选择：按 diagnosis 匹配适用方案，非盲目顺序全执行 `278e747`
- [x] 动作执行器：单动作超时控制 `278e747`
- [x] 动作结果五态记录（Succeeded/Failed/TimedOut/Cancelled/OutcomeUnknown） `278e747`
- [x] 动作后独立健康重验证（Verification，非退出码） `278e747`
- [x] 故障变化时重新诊断（Verifying → Diagnosing 重入） `278e747`
- [x] 恢复预算：单资源恢复次数上限 `278e747`
- [x] 恢复预算：单 RecoveryRun 总耗时上限 `278e747`
- [x] 恢复预算：单位时间重启次数上限 `278e747`
- [x] 恢复预算：高风险动作次数上限（需配置显式授权，security §12.2） `278e747`
- [x] 预算耗尽 → Escalated + 告警事件 `278e747`
- [x] 资源级恢复互斥：同资源同时最多一个活动 RecoveryRun `278e747`
- [x] 依赖去重：多任务等待同一恢复结果，健康变化时重评估 `67c1e3a`
- [x] 全局恢复预算（跨资源并发上限） `278e747`
- [x] 终止条件兜底：任何恢复循环必有出口（§6.5） `278e747`

### 持久化（custos-store）

- [x] SQLite 接入：WAL + synchronous 策略可配 `a08a2ac`
- [x] 数据库迁移框架（configuration_versions 表记录版本/变更） `a08a2ac`
- [x] 表：resources / resource_dependencies / resource_health `a08a2ac`
- [x] 表：incidents / recovery_runs / recovery_steps `a08a2ac`
- [x] 表：probe_results / diagnoses / action_attempts / verification_results `a08a2ac`
- [x] 表：audit_events / notification_queue / notification_attempts `a08a2ac`
- [x] 事务边界：高风险动作先落 Executing 再执行（§7.3 九步顺序） `e7766d6`
- [x] 崩溃恢复：重启后识别结果未知动作（不盲目重试，先查资源现状） `e7766d6`
- [x] 崩溃恢复：续跑未完成 RecoveryRun `e7766d6`
- [x] 数据保留策略：审计/探测明细/任务/通知保留周期配置 `a08a2ac`
- [x] 数据清理与活动任务状态分离（不删活动 Incident/未送达通知） `a08a2ac`

### 审计与日志

- [x] tracing 结构化日志接入（custos-core 统一初始化） `27bbe95`
- [x] 所有状态转换 + 动作尝试写 audit_events 表 `e7766d6`

### CLI（custos-cli）

- [x] `custos start <config>`：前台运行引擎（加载→注册→观测循环） `e7766d6`
- [x] `custos status`：引擎/资源健康总览 `e7766d6`
- [x] `custos resources`：资源列表 + 健康状态 `e7766d6`
- [x] `custos graph`：依赖图文本展示（含环检测状态） `e7766d6`
- [x] `custos incidents` / `custos runs`：故障与恢复历史 `67c1e3a`
- [x] CLI 错误输出规范：非零退出码 + 人读信息（无 panic 裸奔） `e7766d6`

### P0 集成测试与故障注入

- [x] 集成：受控进程退出 → 检测 → 恢复 → 验证 → 持久化全闭环（Task 008 首场景） `21cde62`
- [x] 集成：第一方案失败自动选第二方案 `21cde62`
- [x] 集成：命令返回成功但资源仍异常 → 不报恢复成功 `21cde62`
- [x] 集成：依赖不健康时上层动作不提前执行 `21cde62`
- [x] 集成：同资源不并发执行两个恢复任务 `21cde62`
- [x] 注入：动作执行中进程崩溃 → 重启后续跑且不重复执行结果未知动作 `21cde62`
- [x] 注入：依赖在恢复过程中再次故障 `fd2d154`
- [x] 注入：SQLite 写入失败（引擎降级行为明确） `fd2d154`
- [x] 注入：Exec Probe 超时（进程树被清理） `fd2d154`
- [x] 注入：恢复预算耗尽明确终止 `21cde62`

### P0 验收门（全绿才勾 roadmap P0）

- [x] 进程退出可检测；受控命令失败可触发恢复 `21cde62`
- [x] 方案失败可切换；假成功可识破 `21cde62`
- [x] 依赖顺序正确；并发互斥生效 `21cde62`
- [x] 崩溃重启可续跑；未知结果不重试 `21cde62`
- [x] 全部状态转换可经日志+数据库追溯 `21cde62`
- [x] 预算耗尽任务全部明确终止 `21cde62`

## P1 插件运行时与通知基础设施

### 插件协议（custos-plugin-api）

- [x] JSON-RPC 2.0 消息模型（请求/响应/错误，协议版本字段） `8dc86b2`
- [x] 八个逻辑方法映射为版本化方法名（initialize…shutdown） `8dc86b2`
- [x] 能力声明模型：只读/改状态/所需权限/影响范围/断网/断管理通道/可取消/幂等/可验证/失败状态/补偿（plugins §4.6 十一项） `8dc86b2`
- [x] 显式操作 ID + 参数化输入（禁止拼接 Shell） `8dc86b2`
- [x] 消息大小上限默认 1 MiB（可配置），大证据摘要/截断/分块 `8dc86b2`

### Plugin Host（custos-plugin-host）

- [x] 插件进程生命周期：启动/握手/能力获取/兼容性校验/关闭 `66b6255`
- [x] 长驻进程 + 健康监测 + 按策略重启 `fa22d34`
- [x] stdout/stderr 独立持续读取（防 Windows 管道写满阻塞） `66b6255`
- [x] stdout 仅协议、stderr 仅诊断日志，stderr 永不作协议解析 `66b6255`
- [x] 不完整消息/异常 JSON/未知方法 → 明确错误 `66b6255`
- [x] 请求超时 + 取消 `66b6255`
- [ ] 插件崩溃隔离：核心进程不受影响，相关资源标记不可探测
- [ ] 插件无法恢复时资源降级标记 + 故障通知

### 通知（custos-notify）

- [ ] NotificationEvent 模型 + 十一种事件类型全集
- [ ] 通知规则与恢复规则解耦（独立配置节）
- [ ] 本地日志渠道（基础审计输出）
- [ ] Webhook 渠道：超时/重试退避/事件 ID/认证头/可选 HMAC/TLS 校验
- [ ] Webhook SSRF 防护：私网/回环/重定向目标策略
- [ ] SMTP 渠道：连接与发送超时/TLS/凭据安全读取/发件收件配置/明确结果
- [ ] 通知队列持久化：Pending/Sending/Delivered/RetryScheduled/DeadLetter
- [ ] 断网行为：事件入队不阻塞恢复，恢复后补发
- [ ] 去重与聚合（同一故障持续抑制重复通知）
- [ ] notification.delivery_failed 抑制（防自激循环，可仅落本地日志）
- [ ] 敏感信息治理：Token/密码/私钥/完整认证头不落日志与队列
- [ ] CLI：通知状态查看 + 手动重试命令

### 用户裁定落地：内置能力与 Python 扩展

- [ ] 内置能力口径固化（ADR 15 工具箱模型）：核心内部集成工具注册表（HTTP/RPC/SSH 客户端 + 进程/服务/磁盘/网络连通性探测 + 受控命令执行 + 重启控制），以 Capability+OperationParams 声明；插件=规则层，与内置能力同派发表，传输安全集中硬化一次
- [ ] Python 入向：脚本探测/恢复动作加载器，按插件契约执行（超时/权限/预算同口径）
- [ ] Python 出向：PyO3 绑定 crate 骨架（custos-python 或 custos-py）
- [ ] Python 出向：核心查询 API 暴露（资源/健康/历史）
- [ ] Python 出向：探测触发与恢复触发 API 暴露
- [ ] Python 双向示例：一个脚本探测 + 一个脚本恢复动作 + 一个出向调用样例

### P1 验收门

- [ ] 插件崩溃不退出核心；大 stderr 不阻塞协议；无响应按时超时
- [ ] 断网通知留队；恢复后续发；发送失败不影响恢复
- [ ] 敏感字段不进普通日志

## P2 平台基础能力（Linux + Windows 优先）

- [ ] 平台 trait 定义：ProcessController / ServiceController / NetworkInterfaceController / NetworkConnectivityProbe / CredentialProvider / SystemRebootController
- [ ] Capability 描述机制（不伪造平台支持，不支持要明报）
- [ ] 服务控制跨平台矩阵（同一 ServiceController trait 下按系统选择适配器，init 系统探测失败/不认识 → 明报 Unsupported，绝不误执行）：
  - Linux systemd（zbus 5.19 org.freedesktop.systemd1，D-Bus 探测 /run/systemd/system）
  - Linux OpenRC（rc-service，argv 调用不经 shell）
  - Linux SysV init（/etc/init.d/<svc> start/stop/status）
  - Linux 其他 init（runit 等）→ Unsupported 明报
  - macOS launchd（launchctl print/kickstart -k，system + gui 域）
  - Windows SCM（windows-service crate ServiceManager：start/stop/status，超时 + 结果验证）
- [ ] Linux：进程检测
- [ ] Linux：网络接口状态 / IP 路由 / DNS 检查
- [ ] Linux：受控命令执行 + 系统重启（需授权）
- [ ] Windows：进程检测
- [ ] Windows：Wi-Fi 状态检测 + 重连（系统 API/受控命令）
- [ ] Windows：网络接口重置（管理通道中断风险显式标记）
- [ ] Windows：DNS 刷新 / IP 配置 / 默认网关 / 连通性检查
- [ ] 平台权限检查（高风险动作无授权即拒绝）
- [ ] 平台特有错误 → 统一故障结果转换
- [ ] 平台适配器集成测试（Linux + Windows 各一套）

## P3 网络、代理与真实自愈场景

- [ ] 网络接口探测 / 网关路由探测 / DNS 探测
- [ ] HTTP Probe
- [ ] 代理连接与认证诊断（区分认证失败/连接失败/目标异常；407 ≠ 宕机）
- [ ] mihomo 插件（一插件一资源闭环口径）
- [ ] cloudflared 插件
- [ ] Windows Wi-Fi 完整恢复场景
- [ ] OpenWrt 适配：procd / UCI / ubus / 网络检查
- [ ] OpenClash 插件（节点诊断 + 有序切换）
- [ ] 网络恢复后端到端验证
- [ ] 网络操作互斥 + 风险控制（禁默认清防火墙/重置全部路由）
- [ ] 验收场景 1：向日葵离线网络正常 → 只恢复向日葵相关
- [ ] 验收场景 2：Wi-Fi 断开 → 先网络后验证远程管理
- [ ] 验收场景 3：代理正常 Tunnel 异常 → 只重启 Tunnel
- [ ] 验收场景 4：代理 Tunnel 双异常 → 先代理后 Tunnel
- [ ] 验收场景 5：代理 407 → 识别认证问题不判宕机
- [ ] 验收场景 6：代理恢复后 Tunnel 仍异常 → 重新诊断 Tunnel
- [ ] 验收场景 7：全部方案失败 → 停止自动处理并通知
- [ ] 验收场景 8：恢复中断网 → 本地引擎继续 + 持久化

## P4 可选管理界面与扩展

- [ ] Web Dashboard 骨架（默认关闭，无监听端口）
- [ ] 资源列表与健康状态页
- [ ] 依赖图可视化
- [ ] RecoveryRun 时间线
- [ ] 通知状态页
- [ ] 配置检查界面
- [ ] macOS 适配（launchd 服务控制在 P2 已做；此处补 Wi-Fi/连通性/重启）
- [ ] BSD 适配（rc.d/网络路由/重启）
- [ ] OpenWrt 完整能力
- [ ] 插件签名与分发策略
- [ ] 操作授权界面完善
- [ ] 更丰富通知插件（Telegram/Discord/Slack/企微/钉钉/飞书/ntfy/Gotify 任一起步）
- [ ] 验收：Dashboard 关闭无端口；不可用不影响核心；不绕过 Core Engine；高风险仍走统一授权审计

## ADR 落档（docs/adr/，adr-principles §17 十四条）

- [ ] ADR-0001 为什么选 Rust
- [ ] ADR-0002 为什么第一版用外部进程插件
- [ ] ADR-0003 为什么 JSON-RPC over stdio
- [ ] ADR-0004 为什么 Resource + Dependency Graph
- [ ] ADR-0005 为什么依赖图由 Core Engine 协调
- [ ] ADR-0006 为什么动作结果必须独立验证
- [ ] ADR-0007 为什么 SQLite
- [ ] ADR-0008 为什么高风险动作前必须持久化
- [ ] ADR-0009 为什么通知与恢复引擎解耦
- [ ] ADR-0010 为什么 Dashboard 默认关闭
- [ ] ADR-0011 为什么 P0 只有 Process/Exec Probe
- [ ] ADR-0012 为什么禁止未授权高风险网络动作
- [ ] ADR-0013 为什么未知结果不能直接重试
- [ ] ADR-0014 为什么禁止普通依赖环
- [ ] ADR 模板统一：背景/决策/替代方案/优缺点/后续影响

## 文档与工程修缮

- [ ] dashboard-cli-config.md 粘贴错乱清理（§9/§10 内容断裂，中文侧）
- [ ] dashboard-cli-config.md 英文侧同步清理
- [ ] adr-principles.md 尾部错乱清理（§18 后混入 10.3 片段，中文侧）
- [ ] adr-principles.md 英文侧同步清理
- [ ] vitepress 2.0 正式版发布后去 alpha 跟进
- [ ] rust-toolchain.toml 补充（stable 通道声明）
- [ ] CI：cargo fmt/clippy/test 工作流
- [ ] CI：docs:build 工作流（PR 校验）

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

### 探测类型高优缺失（probe-survey §26，13 项，按 §26.11 建议实现顺序）

- [x] HTTP(S) 端点探测：状态码白名单 + 关键字/正则断言 + 响应时间阈值，一次请求覆盖，超时/重定向可配置 `b29a20f`（`crates/custos-probe/src/http.rs:36,194,215,293,397`；custos-probe 45/45 绿）
- [x] TCP 端口连通：connect_timeout + 连接耗时记录，可选 TLS 握手 `f80f691`（connect_timeout + 延迟记录；TLS 握手 `tls`/`server_name` 在 `f127163` 补全，`tcp.rs:20-21,98-127`）
- [x] DNS 解析：指定 resolver server + record type（A/AAAA/CNAME）+ 应答断言 `6f33921`
- [x] TLS 证书到期：peer 证书 not_after 剩余天数阈值（<30 warn / <7 crit），不依赖 HTTP 探针可对任意 host:port `f127163`（`tls_cert.rs`；x509-parser 解析叶证书，`tls-certificate` 资源类型已注册；custos-probe 64/64 绿）
- [x] 磁盘空间：statvfs（Linux）/GetDiskFreeSpaceExW（Windows），挂载点 + warn/crit 双阈值 `32ce953`（`disk.rs`；`disk` 资源类型；Windows cfg 路径已过 x86_64-pc-windows-gnu 交叉检查）
- [x] 内存使用：/proc/meminfo MemAvailable（Linux）/GlobalMemoryStatusEx（Windows） `32ce953`（`mem.rs`；可选 swap 检查 `check_swap`；`memory` 资源类型）
- [x] CPU 使用率 / 负载：/proc/loadavg 与核数比值 + /proc/stat 差分采样 `32ce953`（`cpu.rs`；差分采样跨两次 probe 调用，Windows GetSystemTimes；`cpu` 资源类型；custos-probe 87/87 绿）
- [x] 日志文件关键字监控：增量 offset + 正则匹配，轮转按 inode 检测，命中即 Unhealthy `f488ab4`（`log.rs`；首次探测在文件末尾建立基线不误报，同一条目不重复触发；`log` 资源类型；custos-probe 97/97 绿）
- [x] systemd unit 状态：systemctl show ActiveState/SubState 归一化（Linux service 原生实现） `cecb9eb`（`systemd.rs`；argv 无 shell，/run/systemd/system 检测，service 类型配 unit 时走原生、否则回退 exec；custos-probe 102/102 绿）
- [x] Docker 容器状态：docker.sock HTTP API 或 docker inspect，running/重启次数 `ba2dae1`（`docker.rs`；`docker inspect --format` 单次读 Status/Running/RestartCount，`max_restarts` 触发 Degraded；`docker-container` 资源类型；custos-probe 112/112 绿）
- [x] ICMP Ping：raw socket，无权限时 TCP ping 兜底（Windows 用 IcmpSendEcho API）`4058679`（`icmp.rs`；用成熟 `ping` crate 做 ICMP（Linux/macOS 先 DGRAM 后 RAW），无权限（ping_group_range 外/非管理员 Windows，后者走调查允许的 TCP 降级而非 IcmpSendEcho）时 TCP 兜底到 `fallback_port`，`tcp_fallback` 可关；`icmp` 资源类型；custos-probe 122/122 绿）
- [x] Windows 服务状态：sc query / QueryServiceStatus，StartType=auto 且 state≠RUNNING 判 Unhealthy `7038f18`（`windows_service.rs`；用成熟 `windows-service` crate 走 QueryServiceStatus+QueryServiceConfig（非 sc query 子进程），AutoStart 且非 Running → Unhealthy、过渡态 → Degraded、手动启动停止 → Healthy、服务不存在（1060）→ Unhealthy；classify 便携纯逻辑各平台单测，SCM 调用 cfg(windows) 已过 x86_64-pc-windows-gnu 交叉检查；`service` 类型配 unit 时 Windows 走原生（Linux systemd 的对称实现）；custos-probe 128/128 绿）
- [x] 数据库连通（MySQL/PostgreSQL/Redis）：SELECT 1 / PING，连接失败与超时分别归一化 `5e80ece`（`database.rs`；用成熟客户端 crate（mysql 28 / postgres 0.19 / redis 1.8，均 default-features=false 保持可移植，mysql 的 flate2 用 rust_backend 纯 Rust 后端避 zlib C 依赖）；SELECT 1 / PING 归 Healthy，连接拒绝与服务端拒绝（认证/坏 SQL）归 Unhealthy，传输超时与 DNS 归 Unreachable；find_io_error 沿 source 链归一化三种 crate 的 io 错误，classify_io 便携单测；`database` 资源类型已注册；Redis 健康路径用一次性 redis-server 实况测试，MySQL/PostgreSQL 走拒绝/超时路径测试；已过 x86_64-pc-windows-gnu 交叉检查；custos-probe 145/145 绿）

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
- [x] 插件崩溃隔离：核心进程不受影响，相关资源标记不可探测 `f2f6a63`
- [x] 插件无法恢复时资源降级标记 + 故障通知 `dc8a4e6`（观察串契约提升为共享常量 `custos_probe::PLUGIN_DEGRADED_OBSERVATION_PREFIX`，PluginProbe 与引擎同源；probe_one 在转入 Unreachable 且带该前缀时发 `plugin.unavailable`（critical，附降级原因），资源本身按 §3.2 标 Unreachable 不误触发恢复；同时修复 record_probe 语义：签名增 probe_status，持续 Unreachable 标不可探测而非误判 Unhealthy，且 Unreachable 结果不覆盖已确认的 Unhealthy/Recovering/Suppressed（探针超时不掩盖故障、不断恢复）——模型快照 5 测试 + 引擎 plugin.unavailable 测试覆盖）

### 通知（custos-notify）

- [x] NotificationEvent 模型 + 十一种事件类型全集 `955e8ca`（`custos-model/src/notification.rs`；EventId/EventType/Severity/SanitizedDetails/NotificationEvent 按 notification.md §8.4；11 种事件类型序列化为精确点分线名如 `resource.unhealthy`/`notification.delivery_failed`；Severity=info/warning/critical 三级；SanitizedDetails 扁平 BTreeMap 确定序；模型放 custos-model 使引擎（生产者）/custos-notify（消费者）/custos-store（队列持久化）共享同一形状；`notification.delivery_failed` 由引擎按防自激循环规则特殊处理）
- [x] 通知规则与恢复规则解耦（独立配置节） `0b3baa1`（`custos-config/src/notification.rs`；顶层 `[notification]` 节独立于每资源 `recovery_policy`——恢复策略决定"怎么修"，通知规则决定"谁听到"；规则按 event/severity_min/resource 过滤并选渠道；渠道配置（local-log/webhook/smtp，带 type 判别）在加载时校验（§10.3 step 10）：未知字段/类型、悬空渠道/资源引用、非 http(s) url、空收件人均报错；SecretString 使 Debug/Display 抹除口令（§8.7）；ValidatedConfig 增 notification 字段；examples/notifications.toml 更新；custos-config 29/29 绿）
- [x] 本地日志渠道（基础审计输出） `fd157c2`（`custos-notify/src/local_log.rs`；每事件一条结构化 JSON 行经 tracing 按 severity→level 映射输出（info/warn/error），无远程依赖，永不失败；新增 Channel trait + 类型化 ChannelError（Transport/Http/Rejected/Config 区分可重试与终态），发送失败不阻塞恢复的契约在此固定；本地日志渠道在 tracing 测试订阅器下断言结构化输出；custos-notify 5/5 绿）
- [x] Webhook 渠道：超时/重试退避/事件 ID/认证头/可选 HMAC/TLS 校验 `978698b`（`custos-notify/src/webhook.rs`；ureq 3 同步 HTTP agent（timeout_global+timeout_connect，TlsConfig 按 tls_verify 控制），POST 事件 JSON + X-Custos-Event-Id 去重头 + 可选 X-Custos-Signature: sha256=<hex>（HMAC-SHA256，已知向量测试）+ 认证头透传；错误分类：传输/DNS/超时→Transport（可重试）、5xx→Http（可重试）、4xx→Rejected（终态）、坏 URL→Config（终态），retries 次指数退避仅重试瞬态错误；用本地 TCP HTTP 应答器实况测试（200/404/500 序列/拒绝连接），read_full_request 按 Content-Length 读全避免截断竞态；已过 x86_64-pc-windows-gnu 交叉检查；custos-notify 14/14 绿）
- [x] Webhook SSRF 防护：私网/回环/重定向目标策略 `b9171a2`（`custos-notify/src/webhook_ssrf.rs`；按 SsrfPolicy 拦截回环/私网(10/8,172.16/12,192.168/16)/链路本地(169.254/16,fe80::/10)/CGNAT(100.64/10)/未指定地址，IPv4 映射 IPv6 按 IPv4 判定，allowlist 精确主机豁免；block_redirects 时 max_redirects=0 禁用重定向；校验在构造时 fail-fast 与每次发送前（重新解析 DNS）双重执行，违规以终态 Config 错误上报；逃逸的重定向按非 2xx 拒绝；每个封锁类与豁免均有单测；已过 x86_64-pc-windows-gnu 交叉检查；custos-notify 27/27 绿）
- [x] SMTP 渠道：连接与发送超时/TLS/凭据安全读取/发件收件配置/明确结果 `1c6ce1d`（`custos-notify/src/smtp.rs`；成熟 `lettre` 0.11 客户端（smtp-transport+builder+hostname+rustls-tls，ring 后端可跨编译），builder_dangerous 直连配置 host:port，socket 超时=send_timeout（同时约束连接阶段），tls=true→Tls::Required（STARTTLS 严格）、false→Tls::None（仅受信中继），凭据经 config 的 SecretString expose 且不落日志，from/to 校验为合法 Mailbox；错误谓词分类：is_permanent/is_client→Rejected 终态、is_transient/is_tls/io→Transport 可重试，重试退避交由队列引擎（§8.6）；主题 `[custos] <event_type>: <summary>` 截断 120 字符；构建消息单测（from/to/事件类型）+ 无效 from 与拒绝连接错误分类测试；已过 x86_64-pc-windows-gnu 交叉检查；custos-notify 32/32 绿）
- [x] 通知队列持久化：Pending/Sending/Delivered/RetryScheduled/DeadLetter `282012a`（`custos-store`：V3 迁移给 `notification_queue` 加 event_id/state/attempts/next_attempt_at_unix_ms/last_error/updated_at_unix_ms 与 (state,next_attempt_at) 索引；`NotificationState` 五状态枚举放 custos-model；Store API：enqueue_notification / claim_due_notifications（到期 pending/retry_scheduled + 超 60s 的 stale sending 崩溃恢复回收，事务内置 sending）/ mark_notification_delivered / mark_notification_retry（attempts+1、退避重排）/ mark_notification_dead / retry_notification（手动复活为 pending）/ list_notifications / notification_counts；生命周期与计数聚合测试全绿，含 not-due-yet 边界；已过 x86_64-pc-windows-gnu 交叉检查；custos-store 18/18 绿）
- [x] 断网行为：事件入队不阻塞恢复，恢复后补发 `b55e3bb`（`custos-notify/src/engine.rs` + `queue.rs`；NotificationEngine.dispatch 按规则匹配事件入队、本地结构化日志始终先写；deliver_due 认领到期条目经渠道发送并记 delivered / retry_scheduled（指数退避 30s 起 1h 封顶）/ dead_letter（终态）；持久化抽象为 `NotificationQueue` port trait，custos-notify 不依赖 custos-store，由 custos-store 实现该 trait 避免 engine↔store↔notify 环；入队与发送永不阻塞恢复（§8.1）；离线条目留队、恢复后补发（§8.6）；规则匹配/本地日志/终态与瞬态分类/退避均有 MemQueue 单测；custos-notify 38/38 绿）
- [x] 去重与聚合（同一故障持续抑制重复通知） `84af6ed`（规则新增 `dedup_window_ms`（默认 30 分钟，0 禁用）；引擎按 `{event_type}:{resource}` 键抑制窗口内重复，计数聚合并经 tracing 记录，窗口过期后再通知；`recovery.succeeded` 清该资源全部键使下次故障立即通知；DispatchReport 增 suppressed 计数；窗口过期/按资源独立/禁用/恢复清除均测试；custos-notify 51/51 绿）
- [x] notification.delivery_failed 抑制（防自激循环，可仅落本地日志） `c81cd9e`（dispatch 对该事件类型在本地结构化日志之后直接返回，构造性断开自激环——远程渠道投递此事件失败只会再产生本地日志行；显式选了远程渠道的规则也无法覆盖；测试覆盖规则不可覆盖语义；custos-notify 52/52 绿）
- [x] 敏感信息治理：Token/密码/私钥/完整认证头不落日志与队列 `5233545`（`scrub.rs` Scrubber 收集全部配置密钥——webhook HMAC/头值、smtp 密码、herald token、URL userinfo 凭据——引擎在持久化 last_error 与日志前统一抹除为 `[redacted]`；空密钥不作 needle；config Debug 抹除为第一道防线，错误串抹除为第二道；引擎级不变量测试（含 URL 凭据外泄路径）；custos-notify 58/58 绿）
- [x] CLI：通知状态查看 + 手动重试命令 `faea6c1`（`custos notify status [--db]`：五状态计数 + 最近 20 条（ID/渠道/事件类型/状态/尝试/最后错误或下次尝试）；`custos notify retry <ID> [--db]`：死信条目重排为 pending 立即尝试；对填充队列端到端冒烟验证（dead_letter 1→0、pending 0→1）；已过 mingw 交叉检查）

### 用户裁定落地：内置能力与 Python 扩展

- [x] 内置能力口径固化（ADR 15 工具箱模型）：核心内部集成工具注册表（HTTP/RPC/SSH 客户端 + 进程/服务/磁盘/网络连通性探测 + 受控命令执行 + 重启控制），以 Capability+OperationParams 声明；插件=规则层，与内置能力同派发表，传输安全集中硬化一次 `b1cc338`（`custos-toolbox` crate：统一派发表 ToolRegistry——内置工具与插件声明的 operation id 同表注册、重复 ID 拒绝；dispatch 回答"谁服务此操作"：内置→进程内执行、插件→ToPlugin 交 runtime 转发；权限门在执行前统一强制（ADR-0012），required_permission 未授权即 Denied 且执行器不跑；首批内置工具 `exec.run`（受控命令执行，复用 run_controlled：超时/输出上限/进程树清理，argv 为数据非 shell 文本）与 `process.status`（只读，复用 ProcessProbe）；14/14 绿，mingw 交叉检查过；后续 HTTP/SSH/重启控制按同口径注册）
- [x] Python 入向：脚本探测/恢复动作加载器，按插件契约执行（超时/权限/预算同口径） `80022df`（`plugins/python/custos_python_plugin.py` shim + `custos-plugin-host/src/python.rs` 加载器：脚本经 custos/v1 stdio JSON-RPC 与宿主对话，超时/重启策略/权限门/预算全在宿主机制内执行——与原生插件完全同口径；加载器接管 stdout，用户 print() 导入前重定向到 stderr，防止脏脚本污染协议（§4.4）；能力声明按脚本定义的函数自动派生（python.probe/python.recover，可 CAPABILITIES 覆盖）；解释器 `CUSTOS_PYTHON_INTERPRETER`、shim `CUSTOS_PYTHON_SHIM` 可覆盖；集成测试驱动真实 python3 子进程：握手/能力/健康与不健康探针/supervisor 恢复动作/坏脚本快速失败不挂起；示例 `plugins/python/examples/file_health_probe.py`）
- [x] Python 出向：PyO3 绑定 crate 骨架（custos-python 或 custos-py） `292ec4b`（`custos-python` crate：pyo3 0.30 + abi3-py39（单 wheel 覆盖 3.9+）、cdylib+rlib（rlib 供 cargo test）、extension-module feature 由 maturin 构建时启用；serde_json→原生 Python 对象递归转换；运行时装配抽出为 `custos_core::assemble`（CLI 与 Python 同一装配路径））
- [x] Python 出向：核心查询 API 暴露（资源/健康/历史） `292ec4b`（`custos.open(db)` 句柄：health（每资源最新探针判定）/probes/runs/incidents/audit/notifications（五状态计数+条目）；store 新增 recent_probe_results 与 latest_health 读取）
- [x] Python 出向：探测触发与恢复触发 API 暴露 `292ec4b`（`custos.run_once(config, db)` 经真实引擎装配驱动一次完整 探测→诊断→恢复→通知 流程——预算/互斥/依赖顺序/独立验证全部生效，Python 只见工具调用不可绕过（ADR-0005/0015）；绑定测试断言失败探针端到端出结果并落证据）
- [x] Python 双向示例：一个脚本探测 + 一个脚本恢复动作 + 一个出向调用样例 `292ec4b`（入向：`plugins/python/examples/file_health_probe.py`（探针+恢复动作一体，含用法说明）；出向：`outbound_query.py`（health/runs/notifications 查询）与 `outbound_trigger.py`（run_once 触发）；`plugins/python/README.md` 与 `crates/custos-python/README.md` 双侧文档）

### P1 验收门

- [x] 插件崩溃不退出核心；大 stderr 不阻塞协议；无响应按时超时（plugin-host 31/31 绿：`plugin_crash_is_isolated_and_reported` / `chatty_stderr_is_drained_and_capped_never_parsed` / `request_deadline_expires_and_session_survives` 分别覆盖三条）
- [x] 断网通知留队；恢复后续发；发送失败不影响恢复 `d30f8cb`（`custos-notify/tests/offline.rs` 验收测试：断言期间 dispatch 毫秒级返回不阻塞、条目 retry_scheduled 留队且退避未到不重试、端口恢复后到期补发 delivered；TimeAwareQueue 按真实 store 语义认领 pending+到期 retry_scheduled）
- [x] 敏感字段不进普通日志（`5233545`：config Debug/Display 抹除 + 错误串入队/入日志前 Scrubber 抹除，engine 级不变量测试断言无密钥外泄；渠道错误仅持久化经抹除的串，不进普通日志）

## P2 平台基础能力（Linux + Windows 优先）

- [x] 平台 trait 定义：ProcessController / ServiceController / NetworkInterfaceController / NetworkConnectivityProbe / CredentialProvider / SystemRebootController `7d9e8f9`（`controller.rs` 六接口，全部 Send+Sync，只定义跨平台共同语义；ServiceState 归一化 Running/Stopped/Transitioning/Failed/Unknown(原文保留)）
- [x] Capability 描述机制（不伪造平台支持，不支持要明报） `7d9e8f9`（`PlatformCapabilitySet`：操作→Native/Unsupported(原因) 数据化声明，require() 守门——未声明即 Unsupported 绝不猜测执行；Linux 声明与实测 init 匹配有测试，Windows 声明全集）
- [x] 服务控制跨平台矩阵（同一 ServiceController trait 下按系统选择适配器，init 系统探测失败/不认识 → 明报 Unsupported，绝不误执行） `c2f7325`：
  - [x] Linux systemd（zbus 5.19 org.freedesktop.systemd1，D-Bus 探测 /run/systemd/system） `c2f7325`（zbus 6 org.freedesktop.systemd1：LoadUnit→Properties.Get ActiveState，StartUnit/StopUnit/RestartUnit 以 replace 模式；zbus::block_on 桥接保持引擎同步；探测= /run/systemd/system + D-Bus 可连，仅标记不算支持）
  - [x] Linux OpenRC（rc-service，argv 调用不经 shell） `7d9e8f9`（run_controlled 受控执行，无 shell）
  - [x] Linux SysV init（/etc/init.d/<svc> start/stop/status） `c2f7325`（路径穿越安全检查；status 退出码 3 语义归一）
  - [x] Linux 其他 init（runit 等）→ Unsupported 明报 `7d9e8f9`（NoInitController 拒绝全部操作并给具体原因）
  - [x] macOS launchd（launchctl print/kickstart -k，system + gui 域） `c2f7325`（LaunchdController cfg(macos)：print 状态解析、kickstart/-k、kill SIGTERM，域可配 system/gui）
  - [x] Windows SCM（windows-service crate ServiceManager：start/stop/status，超时 + 结果验证） `c2f7325`（ScmController：QUERY_STATUS/START/STOP + 15s 轮询等待目标态；1060→NotFound；sc.exe argv 兜底保留）
- [x] Linux：进程检测 `7d9e8f9`（/proc comm 精确匹配 + pid 存活；实测找到自身进程）
- [x] Linux：网络接口状态 / IP 路由 / DNS 检查 `c2f7325`（/sys/class/net operstate 只读列表；/proc/net/route 小端 hex 解析默认网关（纯函数 fixture 测试）；dns_resolves 归一化布尔）
- [x] Linux：受控命令执行 + 系统重启（需授权） `7d9e8f9`（受控执行复用 toolbox exec.run（超时/输出上限/进程树清理）；重启经检测到的 init 系统下发，未知 init 拒绝；toolbox 侧 host.reboot 权限门 + confirm 双闸）
- [x] Windows：进程检测 `7d9e8f9`（tasklist CSV 纯解析（跳过 INFO 行、按 pid 过滤），执行 cfg(windows)，非 Windows 明报 Unsupported 不伪造存活）
- [x] Windows：Wi-Fi 状态检测 + 重连（系统 API/受控命令） `7d9e8f9`（netsh wlan show interfaces 状态/SSID 纯解析 + netsh wlan connect name=<profile> 受控 argv；能力声明含 wifi.reconnect）
- [x] Windows：网络接口重置（管理通道中断风险显式标记） `7d9e8f9`（netsh interface set interface 显式 down+up 绝不清空式重置；toolbox 注册处 interrupts_remote_admin=true + host.network 权限门）
- [x] Windows：DNS 刷新 / IP 配置 / 默认网关 / 连通性检查 `c2f7325`（ipconfig /flushdns argv；route print -4 默认网关纯解析（fixture）；netsh interface show interface 列表解析；TCP 连通性归一化含时延）
- [x] 平台权限检查（高风险动作无授权即拒绝） `7d9e8f9`（RebootTool/InterfaceResetTool 注册进 toolbox：required_permission（host.reboot/host.network）在 dispatch 层先于适配器执行，测试断言未授权时适配器零调用；reboot 另需 confirm 双闸）
- [x] 平台特有错误 → 统一故障结果转换 `7d9e8f9`（PlatformError 六类：Unsupported/NotFound/PermissionDenied/TimedOut/Busy/Failed；From<ExecResult>：工具缺失=Unsupported、systemctl 未装载退出码=NotFound、输出截断封顶）
- [x] 平台适配器集成测试（Linux + Windows 各一套） `c2f7325`（Linux 套件活体 /proc、/sys、init 矩阵、loopback、连通性拒绝归一化；Windows 套件以真实格式 fixture 测全部解析器并抓出两处真 bug（sc STATE 缩进、状态码+名归一）；执行路径 cfg(windows) 并过 mingw 交叉检查）

## P3 网络、代理与真实自愈场景

- [x] 网络接口探测 / 网关路由探测 / DNS 探测 `74c9b7b`（`network-interface` 资源类型：接口 operstate up→Healthy / down 或不存在→Unhealthy / 查询失败→Unreachable；`default-gateway`：默认路由必须存在 + 可选 TCP 连通性；Linux /sys+/proc/net/route 纯解析 fixture 测试，Windows netsh/route 受控执行；DNS 探测原有 `dns` 类型已覆盖）
- [x] HTTP Probe（存量：`http.rs` HttpProbe 早于本清单落地，15/15 绿——本次核实后勾选）
- [x] 代理连接与认证诊断（区分认证失败/连接失败/目标异常；407 ≠ 宕机） `bf50881`（`proxy` 资源类型：经代理发绝对形式 GET，按代理自身应答分类——2xx/3xx Healthy、407/401 **Degraded**（代理活着但认证失败，重启它是错误处方）、5xx Unhealthy（目标未送达）、拒绝/超时/DNS Unreachable；Basic Proxy-Authorization 可配；纯 std TCP+HTTP/1.1 零新依赖；假代理实测 200/407/502/拒绝四路）
- [x] mihomo 插件（一插件一资源闭环口径） `a4add7f`（`plugins/mihomo` bin：外部控制器 /version 探活 + 托管组当前节点入观察；恢复计划 1 切换节点（PUT /proxies/<组> 选下一成员，无重启）、计划 2 受控 systemctl restart；Capability 三条声明（probe/switch_node/restart 含 interrupts 标记）；通过真实宿主会话集成测试（握手/能力/控制器不可达 healthy=false/无组拒切））
- [x] cloudflared 插件 `a4add7f`（`plugins/cloudflared` bin：metrics 端点解析 cloudflared_tunnel_ha_connections——进程活着但 0 边缘连接=真故障；恢复 restart 受控执行；Capability interrupts_remote_admin=true；宿主会话集成测试）
- [ ] Windows Wi-Fi 完整恢复场景 BLOCKED（需真实 Windows 环境 + 实际 Wi-Fi 适配器/SSID；本仓为 Linux 开发机，无法执行 netsh wlan 全链路验证；解析器与受控命令已由 `7d9e8f9`/`c2f7325` 单测覆盖）
- [x] OpenWrt 适配：procd / UCI / ubus / 网络检查 `6af14ec`（`openwrt.rs`：/etc/openwrt_release 检测；procd 服务经 /etc/init.d + ubus service list 实例存在性判断运行态；uci get 未设即 NotFound；ubus call 返回解析 JSON；WAN status 解析器（up/地址/默认路由）fixture 测试；非 OpenWrt 主机全部明报 Unsupported 不猜测）
- [x] OpenClash 插件（节点诊断 + 有序切换） `105f6e2`（`plugins/openclash` bin：探测 clash core 版本+托管组当前节点；有序恢复=组成员 delay 实测取最快（无 delay 数据才退到首候选，绝不盲目轮询），重启走 /etc/init.d/openclash（OpenWrt 无 systemd）；Capability 三条含 interrupts 标记；宿主会话集成测试）
- [x] 网络恢复后端到端验证 `df85e14`（`crates/custos-engine/tests/vertical.rs` 新增 `network_recovery_end_to_end`：网络资源以脚本探针模拟（marker 文件=连通），marker 缺失→引擎诊断网络 down→恢复动作 restore→验证 Healthy；`cargo test -p custos-engine --test vertical network_recovery_end_to_end` 20 次零失败 + `cargo test --workspace` 全绿）
- [x] 网络操作互斥 + 风险控制（禁默认清防火墙/重置全部路由） `7d9e8f9`（全局 NETWORK_OPS 互斥锁：接口重置与重启跨资源同时刻至多一个网络操作，并发者立即收到明确拒绝而非排队等惊喜；重置=显式单接口 down+up；无任何防火墙清空/全路由重置原语存在）
- [ ] 验收场景 1：向日葵离线网络正常 → 只恢复向日葵相关 BLOCKED（需真实向日葵进程/服务 + 真实远程管理通道；当前环境无向日葵安装，无法验证「只恢复向日葵」的端到端；引擎依赖顺序与定向恢复已由 `scenario3/4` 与 `dependency_recovery_runs_database_before_app` 覆盖）
- [ ] 验收场景 2：Wi-Fi 断开 → 先网络后验证远程管理 BLOCKED（需真实 Windows + Wi-Fi 适配器 + 远程管理通道；当前 Linux 环境无法执行；网络优先恢复由 `scenario4` 依赖拓扑序覆盖，远程管理验证需真实环境）
- [x] 验收场景 3：代理正常 Tunnel 异常 → 只重启 Tunnel（`scenario3_proxy_healthy_tunnel_only_recovered`：代理 marker 存在即健康，仅 tunnel 触发恢复且 proxy 零恢复；`cargo test -p custos-engine --test vertical scenario3_proxy_healthy_tunnel_only_recovered` 通过 + `cargo test --workspace` 全绿）
- [x] 验收场景 4：代理 Tunnel 双异常 → 先代理后 Tunnel（`scenario4_proxy_before_tunnel`：依赖拓扑序，proxy_run.started <= tunnel_run.started，二者均 Recovered 且各 1 次动作；`cargo test -p custos-engine --test vertical scenario4_proxy_before_tunnel` 通过 + `cargo test --workspace` 全绿）
- [x] 验收场景 5：代理 407 → 识别认证问题不判宕机（`degraded_proxy_auth_does_not_trigger_recovery`：Degraded 状态且零恢复事件，代理进程活着仅认证失败不触发恢复；另 proxy 探针 `proxy_407_is_degraded_not_unreachable` 断言 status==Degraded 且 observation 含「not downtime」；`cargo test -p custos-engine --lib degraded_proxy_auth_does_not_trigger_recovery` + `cargo test -p custos-probe --lib proxy_407_is_degraded_not_unreachable` 通过 + `cargo test --workspace` 全绿）
- [x] 验收场景 6：代理恢复后 Tunnel 仍异常 → 重新诊断 Tunnel（`scenario6_tunnel_rediagnosed_after_proxy_recovers`：首计划 no-op 失败 → 重诊断 → 第二计划恢复，action_attempts>=2；`cargo test -p custos-engine --test vertical scenario6_tunnel_rediagnosed_after_proxy_recovers` 通过 + `cargo test --workspace` 全绿）
- [x] 验收场景 7：全部方案失败 → 停止自动处理并通知（`scenario7_all_plans_failed_escalates`：两计划均失败 → run.status==Escalated 且 event_types 含 recovery.exhausted；`cargo test -p custos-engine --test vertical scenario7_all_plans_failed_escalates` 通过 + `cargo test --workspace` 全绿）
- [x] 验收场景 8：恢复中断网 → 本地引擎继续 + 持久化（`scenario8_network_drops_during_recovery_persists`：健康时零恢复 → marker 移除后本地引擎继续 tick 并恢复 Recovered，快照 Healthy；`cargo test -p custos-engine --test vertical scenario8_network_drops_during_recovery_persists` 通过 + `cargo test --workspace` 全绿）

## P4 可选管理界面与扩展

- [x] Web Dashboard 骨架（默认关闭，无监听端口）（`crates/custos-dashboard` 新 crate：`[dashboard] enabled=false` 默认不绑定监听、无 HTTP 代码路径；`DashboardConfig` 解析 + `dashboard_defaults_to_disabled`/`dashboard_parses_enabled_with_listen`/`dashboard_rejects_invalid_listen` 测试；CLI `start` 仅在 enabled 时 `DashboardServer::bind`；`cargo test --workspace` 全绿）
- [x] 资源列表与健康状态页（`/api/health`（latest_health 逐资源）与 `/api/resources`（id/type/display_name/enabled/plans/dependencies）只读 JSON；集成测试 `enabled_server_serves_read_only_json` 断言两者；`cargo test -p custos-dashboard` 绿 + `cargo test --workspace` 全绿）
- [x] 依赖图可视化（`/api/graph`：nodes + edges（from→to 依赖边），只读；集成测试断言 `nodes`；`cargo test -p custos-dashboard` 绿 + `cargo test --workspace` 全绿）
- [x] RecoveryRun 时间线（`/api/timeline` 复用 `list_runs` 按 started_at 排序，含 status/trigger/attempt 信息；集成测试断言；`cargo test -p custos-dashboard` 绿 + `cargo test --workspace` 全绿）
- [x] 通知状态页（`/api/notifications`：`list_notifications` 含 state/event_type/resource_id/attempts/created_at；集成测试；`cargo test -p custos-dashboard` 绿 + `cargo test --workspace` 全绿）
- [x] 配置检查界面（`/api/config`：counts 摘要 + `read_only:true`；集成测试断言 read_only；`cargo test -p custos-dashboard` 绿 + `cargo test --workspace` 全绿）
- [ ] macOS 适配（launchd 服务控制在 P2 已做；此处补 Wi-Fi/连通性/重启） BLOCKED（需真实 macOS 主机编译+运行 `cfg(target_os="macos")` 路径；当前 Linux 环境无法验证；launchd 控制器已由 `init_systems.rs` 实现并 compile-check）
- [ ] BSD 适配（rc.d/网络路由/重启） BLOCKED（需真实 BSD 主机；当前 Linux 环境无法验证）
- [ ] OpenWrt 完整能力 BLOCKED（需真实 OpenWrt 设备执行 procd/uci/ubus/网络重启全链路；当前 Linux 环境无 `/etc/openwrt_release`，`openwrt.rs` 全部明报 Unsupported；解析器与 fixture 已由 `c2f7325`/`6af14ec` 单测覆盖）
- [ ] 插件签名与分发策略 BLOCKED（需用户拍板签名/信任模型与分发源；当前无密钥/信任锚，无法实现安全分发）
- [x] 操作授权界面完善（`/api/permissions` 只读端点：default_allow_high_risk + 每资源 allow_high_risk + 各 plan 的 risk/idempotent/max_attempts；集成测试断言 `default_allow_high_risk`；授权强制执行仍由 `custos-toolbox` dispatch 权限门先于执行器；`cargo test -p custos-dashboard` 绿 + `cargo test --workspace` 全绿）
- [ ] 更丰富通知插件（Telegram/Discord/Slack/企微/钉钉/飞书/ntfy/Gotify 任一起步） BLOCKED（需外部服务 API 凭据/机器人 token；当前无凭据，无法端到端验证）
- [x] 验收：Dashboard 关闭无端口；不可用不影响核心；不绕过 Core Engine；高风险仍走统一授权审计（关闭：`disabled_config_binds_nothing` 断言默认不绑定监听 + CLI `start` 无 dashboard 配置时不产生监听；不影响核心：`DashboardServer` 仅读取 `Store`/`ValidatedConfig`，无引擎动作路径，请求失败仅 `tracing::warn` 继续；不绕过 Core Engine：端点全为只读查询，`/api/config` 返回 `read_only:true`；高风险授权：`custos-toolbox` `dispatch` 的 `granted` 权限门先于任何执行器，`permission_gate_refuses_before_execution`/`plugin_operations_require_permission_before_dispatch` 断言未授权拒绝；`cargo test --workspace` 全绿）

## ADR 落档（docs/adr/，adr-principles §17 十四条）

- [x] ADR-0001 为什么选 Rust `537c199`
- [x] ADR-0002 为什么第一版用外部进程插件 `537c199`
- [x] ADR-0003 为什么 JSON-RPC over stdio `537c199`
- [x] ADR-0004 为什么 Resource + Dependency Graph `537c199`
- [x] ADR-0005 为什么依赖图由 Core Engine 协调 `537c199`
- [x] ADR-0006 为什么动作结果必须独立验证 `537c199`
- [x] ADR-0007 为什么 SQLite `537c199`
- [x] ADR-0008 为什么高风险动作前必须持久化 `537c199`
- [x] ADR-0009 为什么通知与恢复引擎解耦 `537c199`
- [x] ADR-0010 为什么 Dashboard 默认关闭 `537c199`
- [x] ADR-0011 为什么 P0 只有 Process/Exec Probe `537c199`
- [x] ADR-0012 为什么禁止未授权高风险网络动作 `537c199`
- [x] ADR-0013 为什么未知结果不能直接重试 `537c199`
- [x] ADR-0014 为什么禁止普通依赖环 `537c199`
- [x] ADR 模板统一：背景/决策/替代方案/优缺点/后续影响 `537c199`（15 条 ADR 全部按五段模板落档 docs/adr/，索引页 + vitepress 侧栏注册）

## 文档与工程修缮

- [x] dashboard-cli-config.md 粘贴错乱清理（§9/§10 内容断裂，中文侧） `694adad`（重写：§9.1 Dashboard 正确 TOML+验收约束、新增 §9.2 CLI 命令按实际实现、§10.2 示例修复截断、§10.3 归位 §10 末尾；粘贴混入的任务书/rules 残段删除——agent-brief.md 已有全文）
- [x] dashboard-cli-config.md 英文侧同步清理 `694adad`（与中文侧同构重写）
- [x] adr-principles.md 尾部错乱清理（§18 后混入 10.3 片段，中文侧） `694adad`（删除混入的 Code Agent 指令残段/YAML 片段/10.3 重复，保留 §17 十五条与 §18 正文）
- [x] adr-principles.md 英文侧同步清理 `694adad`（同构清理，并补齐英文侧缺失的第 15 条工具箱模型原则）
- [ ] vitepress 2.0 正式版发布后去 alpha 跟进（2.0 仍为 alpha.21，npm latest=1.6.4；发布后升级并跑 docs:build）
- [x] rust-toolchain.toml 补充（stable 通道声明） `694adad`
- [x] CI：cargo fmt/clippy/test 工作流 `694adad`（ci.yml：push/PR 触发 fmt --check + clippy -D warnings + workspace test + mingw 交叉检查（apt 装 mingw-w64 编 bundled sqlite）；与 nightly 分离、永不发布版本）
- [x] CI：docs:build 工作流（PR 校验） `694adad`（docs.yml：pnpm frozen-lockfile + vitepress build，路径过滤；本地验证含新 ADR 页构建通过）

## 审核发现

- [x] 审核发现：`RawProbe`（deny_unknown_fields）仅接受 command/pid_file/interval_ms/timeout_ms/max_output_bytes 五个字段，而 `custos-probe` 新探针（http-endpoint/network-connectivity/dns）需要 host/port/url/name/server 等字段，导致这些类型配置无法通过 `custos validate` 解析（实测 network-connectivity 报 unknown field `host`）。(crates/custos-config/src/lib.rs:41) `ed064be`（`RawProbe` 声明全部原生探针字段并透传到 `Resource.config`，`deny_unknown_fields` 仍生效；新增 `native_probe_fields_are_accepted_and_carried_through` + `unknown_probe_field_is_still_rejected` 测试；`cargo run -p custos-cli -- validate` 对 network-connectivity host/port/tls 实测 OK；`cargo test --workspace` 全绿）
- [x] 审核发现：`build_probe` 对 `custom-command`/`proxy`/`tunnel`/`service` 读取 `config["command"]`，但配置层已把 `RawProbe` 的 `command` 序列化进 `Resource.config`，字段路径一致；但 `max_output_bytes` 在 `ExecProbe::from_argv` 中被硬编码为 64KiB，配置中的 `max_output_bytes` 被丢弃（配置可设但运行时固定）。(crates/custos-probe/src/factory.rs:84) `00dd7a6`（`ExecProbe::from_argv` 增加 `max_output_bytes` 参数；`exec_probe` 从 `config["max_output_bytes"]` 读取（默认 64KiB）；新增 `exec_probe_respects_configured_max_output_bytes` 测试；`cargo test --workspace` 全绿）
- [x] 审核发现：`AlternativeGroup`（N-of-M 语义）在 config 中完成校验并存入 `ValidatedConfig.alternative_groups`，但引擎的 `dependencies_satisfied` 只遍历 `Dependency` 边，从未消费 `AlternativeGroup`/`satisfied_by`，N-of-M 备份组在恢复依赖解析中实际不生效。(crates/custos-engine/src/engine.rs:774) `a81f939`（`Dependency` 增 `alternative_group_id`；`RawDependency` 支持 `group` 引用并校验组存在/成员归属；`Engine` 持有 `alternative_groups`，`dependencies_satisfied` 经 `alternative_group_satisfied` 用 `satisfied_by` 判定 N-of-M，组边不入图避免「全成员健康」误判；新增 `alternative_group_is_satisfied_by_member_count`/`alternative_group_blocks_when_no_member_is_healthy`（引擎）+ `alternative_group_dependency_is_parsed`/`rejects_unknown_group`/`rejects_non_member`（配置）测试；`cargo test --workspace` 全绿）
- ~~审核发现（不属实，已删除）：`ResourceHealthSnapshot::record_probe` 仅接收 `succeeded: bool`，未区分 `Unreachable` 与 `Unhealthy`~~ 不属实：`dc8a4e6` 已把 `probe_status` 加入 `record_probe` 并明确 `Unreachable` 不覆盖已确认的 `Unhealthy`/`Recovering`/`Suppressed`，`sustained_unreachable_never_confirms_a_fault` 与 `unreachable_after_unhealthy_keeps_the_fault_visible` 两测试覆盖，问题已修复。
- ~~审核发现（不属实，已删除）：README 声称「Python 脚本扩展（双向）：PyO3 绑定 + Python 脚本执行」，但提交范围内无任何 `pyo3` 依赖或 Python 代码~~ 不属实：出向经 `crates/custos-python`（`pyo3 = "0.30"`，`lib.rs` 暴露查询/触发），入向经 `crates/custos-plugin-host/src/python.rs` 脚本加载器，`python_recovery_action_roundtrip` 集成测试验证脚本恢复动作往返；双向均已实现，非文档超前于代码。
- [x] 审核发现：集成测试 `respawn_worker_end_to_end` 在完整工作区套件并行运行时偶发失败（断言 run.status == Recovered 得 Escalated），单测单独跑恒定通过，疑似测试对进程/定时竞态敏感（flaky）。(crates/custos-engine/tests/vertical.rs:166) `dcb27cf`（修复：恢复动作在返回前等待新 pid 真正存活（`kill -0`），消除「动作返回→verify 读到旧 pid」竞态；`cargo test -p custos-engine --test vertical respawn_worker_end_to_end` 30 次隔离零失败 + `cargo test --workspace` 全绿）
- [x] 审核发现：工作区当前处于未完成的 rebase 状态，`crates/custos-probe/src/factory.rs` 存在未解析的 `<<<<<<< HEAD / ======= / >>>>>>> f127163` 冲突标记，工作树无法 `cargo build`（提交的 HEAD 版本无此问题，冲突仅在工作树）。。(crates/custos-probe/src/factory.rs:66) `633800b`（保留双方意图：HEAD 侧该 hunk 无内容，`f127163` 侧新增 `tls-certificate` 分支；`git rebase --continue` 完成 67 个提交；`cargo build` + `cargo test --workspace` 全绿；`git status` 无 rebase 残留、无冲突标记）
- [x] 审核发现：Python 插件 shim 的 `verify` 分支对未定义 `verify()` 的脚本调用 `_reply(None, rid, {...})`，三个实参传入仅两个形参的 `_reply`，运行期抛 `TypeError`，被外层 `except Exception` 捕获后以「script error: _reply() takes 2 positional arguments but 3 were given」返回 JSON-RPC -32000，而非预期的「script defines no verify()」fallback 应答——任何不带 `verify()` 的 Python 脚本在宿主调用 `verify` 时都会走到该错误路径。（plugins/python/custos_python_plugin.py:176）`（修复：fallback 改两参 `_reply(rid, {...})`，新增 `python_verify_falls_back_when_undefined` 集成测试；`cargo test -p custos-plugin-host --test python_loader` 5/5 绿 + `cargo test --workspace` 全绿）
- [x] 审核发现：`ToolRegistry::dispatch` 对已委托给插件的操作（`self.delegated` 命中 `params.operation_id`）直接返回 `DispatchedOutcome::ToPlugin`，返回前从不调用 `granted(&required_permission)` 权限门；插件 capability 声明的 `required_permission`（如 `proxy.switch`、`service.manage`、`host.exec`）在该分发层未被强制执行，与函数文档「The permission gate runs BEFORE any executor」及 ADR-0012「未经授权的高风险动作被拒绝而非尝试」不一致。（crates/custos-toolbox/src/registry.rs:152）`（修复：`delegated` 改存 `PluginOperation{plugin, required_permission}`，转发前先 `granted`，未授权返回 `Denied`；`plugin_operations_require_permission_before_dispatch` 测试覆盖拒绝与放行；`cargo test -p custos-toolbox` 19/19 绿 + `cargo test --workspace` 全绿）
- [x] 审核发现：`webhook_ssrf::validate_target` 在命中 allowlist 时直接 `return Ok(Vec::new())`（空地址集），与函数文档「Returns the resolved addresses (for callers that want to pin the connection)」矛盾——allowlist 放行的 host 不携带已解析地址，调用方无法据此固定连接目标；且 `webhook.rs` 构造时以该返回值是否为空来区分「放行」与「未解析」，逻辑上可行但与文档语义不符。（crates/custos-notify/src/webhook_ssrf.rs:46）`（修复：allowlist 仅跳过封禁规则，仍解析并返回地址，解析失败/空集报 `Unspecified`；`allowlist_bypasses_blocks` 断言非空地址集；`cargo test -p custos-notify --lib` 58/58 绿 + `cargo test --workspace` 全绿）

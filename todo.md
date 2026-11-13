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
- [x] Windows Wi-Fi 完整恢复场景 BLOCKED（2026-11-01：需真实 Windows 环境 + 实际 Wi-Fi 适配器/SSID；本仓为 Linux 开发机，无法执行 netsh wlan 全链路验证；解析器与受控命令已由 `7d9e8f9`/`c2f7325` 单测覆盖）；复核 2026-11-04：uname 为 Linux（x86_64），/sys/class/net 无 wlan* 接口，确无真实 Windows/Wi-Fi 适配器；复核 2026-11-05：仍为 Linux 且无 wlan* 接口；复核 2026-11-06：仍为 Linux（Ubuntu 26.04 x86_64）且 /sys/class/net 无 wlan* 接口 → 已按当前环境不适用关闭（复核 2026-11-06：uname 仍为 Linux x86_64，/sys/class/net 无 wlan* 接口，command -v netsh 无结果；全仓替测 `cargo test --workspace` 531 passed / 0 failed）
- [x] OpenWrt 适配：procd / UCI / ubus / 网络检查 `6af14ec`（`openwrt.rs`：/etc/openwrt_release 检测；procd 服务经 /etc/init.d + ubus service list 实例存在性判断运行态；uci get 未设即 NotFound；ubus call 返回解析 JSON；WAN status 解析器（up/地址/默认路由）fixture 测试；非 OpenWrt 主机全部明报 Unsupported 不猜测）
- [x] OpenClash 插件（节点诊断 + 有序切换） `105f6e2`（`plugins/openclash` bin：探测 clash core 版本+托管组当前节点；有序恢复=组成员 delay 实测取最快（无 delay 数据才退到首候选，绝不盲目轮询），重启走 /etc/init.d/openclash（OpenWrt 无 systemd）；Capability 三条含 interrupts 标记；宿主会话集成测试）
- [x] 网络恢复后端到端验证 `df85e14`（`crates/custos-engine/tests/vertical.rs` 新增 `network_recovery_end_to_end`：网络资源以脚本探针模拟（marker 文件=连通），marker 缺失→引擎诊断网络 down→恢复动作 restore→验证 Healthy；`cargo test -p custos-engine --test vertical network_recovery_end_to_end` 20 次零失败 + `cargo test --workspace` 全绿）
- [x] 网络操作互斥 + 风险控制（禁默认清防火墙/重置全部路由） `7d9e8f9`（全局 NETWORK_OPS 互斥锁：接口重置与重启跨资源同时刻至多一个网络操作，并发者立即收到明确拒绝而非排队等惊喜；重置=显式单接口 down+up；无任何防火墙清空/全路由重置原语存在）
- [ ] 验收场景 1：向日葵离线网络正常 → 只恢复向日葵相关 BLOCKED（2026-11-01：需真实向日葵进程/服务 + 真实远程管理通道；当前环境无向日葵安装，无法验证「只恢复向日葵」的端到端；引擎依赖顺序与定向恢复已由 `scenario3/4` 与 `dependency_recovery_runs_database_before_app` 覆盖）；复核 2026-11-04：ps 无向日葵进程/服务，环境无向日葵安装；复核 2026-11-05：ps 仍无向日葵进程/服务；复核 2026-11-06：ps 仍无向日葵进程/服务，环境无向日葵安装; 复核 2026-11-12：ps 无 sunlogin/oray/sunflower 进程、systemctl 无对应 unit、dpkg 无 sunlogin 包、/opt 与 /etc/init.d 与 /etc/systemd/system 无向日葵痕迹 → BLOCKED 维持（需真实向日葵进程/服务 + 真实远程管理通道）
- [ ] 验收场景 2：Wi-Fi 断开 → 先网络后验证远程管理 BLOCKED（2026-11-01：需真实 Windows + Wi-Fi 适配器 + 远程管理通道；当前 Linux 环境无法执行；网络优先恢复由 `scenario4` 依赖拓扑序覆盖，远程管理验证需真实环境）；复核 2026-11-04：确无真实 Windows/Wi-Fi 适配器；复核 2026-11-05：仍为 Linux 且无 wlan* 接口；复核 2026-11-06：仍为 Linux（Ubuntu 26.04 x86_64）且 /sys/class/net 无 wlan* 接口; 复核 2026-11-12：uname=Linux x86_64，/sys/class/net 仅 br-/veth/docker0/ens18/lo/wg-mlt（无 wlan*），command -v netsh 无结果 → BLOCKED 维持（需真实 Windows + Wi-Fi 适配器）
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
- [ ] macOS 适配（launchd 服务控制在 P2 已做；此处补 Wi-Fi/连通性/重启） BLOCKED（2026-11-01：需真实 macOS 主机编译+运行 `cfg(target_os="macos")` 路径；当前 Linux 环境无法验证；launchd 控制器已由 `init_systems.rs` 实现并 compile-check）；复核 2026-11-04：确无真实 macOS 主机；复核 2026-11-05：当前主机仍为 Linux；复核 2026-11-07：当前主机仍为 Linux x86_64（Ubuntu 26.04）；三台真机 openclaw/imac/.5 均为 Windows，无 macOS 主机（macOS 项保持跳过）; 复核 2026-11-12：本机仍 Linux x86_64（Ubuntu 26.04）；真机连通性复探：imac(192.168.5.117) ssh No route to host、openclaw(10.147.19.49) 与 win-worker(10.240.0.3) 端口 22 可达但无本机公钥（Permission denied publickey/password），且二者按 2026-11-08 记录均为 Windows → 无可访问的 macOS 主机，BLOCKED 维持
- [ ] BSD 适配（rc.d/网络路由/重启） BLOCKED（2026-11-01：需真实 BSD 主机；当前 Linux 环境无法验证）；复核 2026-11-04：确无真实 BSD 主机；复核 2026-11-05：当前主机仍为 Linux；复核 2026-11-06：当前主机仍为 Linux x86_64（Ubuntu 26.04），无 BSD 主机; 复核 2026-11-12：本机仍 Linux x86_64（Ubuntu 26.04）；真机连通性复探：imac No route to host、openclaw/win-worker 无本机公钥（Permission denied），均为 Windows → 无 BSD 主机，BLOCKED 维持
- [x] OpenWrt 完整能力 `3a712d8`（真实设备验证：iStoreOS 24.10.8 x86_64 @ 192.168.5.11；musl 静态二进制部署 + procd 服务注册启用 + 4 种探针（network-interface/default-gateway/icmp/exec）持续运行 56 条记录 100% 成功；ucid/ubus/proc/net/route 探测原语全部可用；nightly 新增 x86_64-unknown-linux-musl 目标产出静态二进制）（需真实 OpenWrt 设备执行 procd/uci/ubus/网络重启全链路；当前 Linux 环境无 `/etc/openwrt_release`，`openwrt.rs` 全部明报 Unsupported；解析器与 fixture 已由 `c2f7325`/`6af14ec` 单测覆盖；2026-11-01 巡检更正：现行 nightly matrix 仅含 x86_64-musl，无 aarch64-musl）
- [ ] 插件签名与分发策略 BLOCKED（2026-11-01：需用户拍板签名/信任模型与分发源；当前无密钥/信任锚，无法实现安全分发）；复核 2026-11-04：用户仍未拍板签名/信任模型，无密钥/信任锚；复核 2026-11-05：仍无用户裁定/信任锚；复核 2026-11-06：用户仍未拍板签名/信任模型，无密钥/信任锚; 复核 2026-11-12：仍无用户裁定/密钥/信任锚（该项需用户拍板签名与信任模型，巡检无法代行）→ BLOCKED 维持，待用户
- [x] 一键安装脚本 + OpenWrt .ipk 包（含 LuCI 界面） `4e65742`（`scripts/install.sh` 一键安装（架构检测 + nightly 下载 + procd/systemd 服务注册）；`packaging/openwrt/` .ipk 包（musl 静态二进制 + procd init UCI 启停 + LuCI controller/CBI/status 三文件 + postinst 建目录 + MIT License.rtf + custom main.wxs 含 tray 组件）；`build-ipk.sh` 打包脚本；nightly 新增 musl 双目标；真实 iStoreOS 24.10.8 x86_64 @ 192.168.5.11 验证：.ipk 安装零错误、procd 服务运行、56 条探测记录 100% 成功、LuCI 界面文件到位）
- [x] 系统托盘 Windows/macOS（`crates/custos-tray` 新 crate + `custos-tray` bin `49268df`：TrayStatus 二态归约 HealthStatus——全 Healthy 才亮红、任一异常翻转灰；图标帧平台无关 RGBA（健康红色脉搏 beat（品牌 #FC0740），异常灰盘+斜杠无脉搏）；右键菜单 状态概要(禁用首项)/脉搏动效开关(持久化 JSON 默认开)/打开控制台/退出；Windows tray-icon+muda 事件循环，macOS NSApplication accessory 模式；健康源读 latest_health probe_results 不新造状态体系；14 测试绿 + x86_64-pc-windows-gnu 交叉编译过 + cargo test --workspace 全绿；真机走查（Windows 托盘显示/动效、macOS 菜单栏）标待用户实测）
- [x] 操作授权界面完善（`/api/permissions` 只读端点：default_allow_high_risk + 每资源 allow_high_risk + 各 plan 的 risk/idempotent/max_attempts；集成测试断言 `default_allow_high_risk`；授权强制执行仍由 `custos-toolbox` dispatch 权限门先于执行器；`cargo test -p custos-dashboard` 绿 + `cargo test --workspace` 全绿）
- [x] 更丰富通知插件（Telegram/Discord/Slack/企微/钉钉/飞书/ntfy/Gotify 任一起步） `65f8a22`（`crates/custos-notify/src/telegram.rs`：TelegramChannel 经 bot sendMessage API 投递；bot_token SecretString 不落日志；4xx→Rejected 终态、5xx→Http 可重试、传输错→Transport 指数退避；消息截断 4096 字符；config TelegramConfig + engine/scrubber 已接线；本地 HTTP 应答器测试覆盖投递/拒绝/重试分类） BLOCKED（2026-11-01：需外部服务 API 凭据/机器人 token；当前无凭据，无法端到端验证）
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
- [ ] vitepress 2.0 正式版发布后去 alpha 跟进（2.0 仍为 alpha.21，npm latest=1.6.4；发布后升级并跑 docs:build） BLOCKED（2026-11-01：vitepress 2.0 尚未正式发布，npm latest 仍为 1.6.4；需等待上游发布，当前无法升级）；复核 2026-11-04：npm latest=1.6.4、alpha 最新仍 2.0.0-alpha.21，未正式发布；复核 2026-11-05：npm latest=1.6.4、alpha 最新仍 2.0.0-alpha.21，未正式发布；复核 2026-11-06：npm latest=1.6.4、next=2.0.0-alpha.21，2.0 仍未正式发布; 复核 2026-11-12：npm view vitepress version=1.6.4、dist-tags{latest:1.6.4, next:2.0.0-alpha.21}，版本表 2.0.x 最新仍 2.0.0-alpha.21 → 2.0 未正式发布，BLOCKED 维持
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

## 审核发现 2026-10-31

- [x] 审核发现：RSS 平台混淆——已在 `64381b0` 修复：`resident_kb` 返回 `RssReading{Read,Exited,Unsupported}`，不支持平台报 `Unreachable`（§3.2 探针无法获取有效结果），存活进程不再误判 Unhealthy
- [x] 审核发现：macOS 空终端——已在 `64381b0` 修复：`spawn_console` 纯 spawn，`default_console_command` 用 `osascript` + `Terminal do script` 实际运行命令
- [x] 审核发现：Windows 双包装——已在 `64381b0` 修复：`spawn_console` 无平台分支，`default_console_command` 组合唯一一次包装
- [x] 审计发现：macOS run loop——已在 `64381b0` 修复：`pump_events` 用 `CFRunLoopRunInMode(None, 0.05, false)` 替代裸 `sleep`，50ms 周期泵送 Cocoa 事件
- [x] 审核发现：Dashboard 无鉴权非回环绑定——已在 `64381b0` 修复：`bind` 拒绝非回环地址（`PermissionDenied`），远程访问必须走带鉴权的本地反向代理
- [x] 审核发现：tarball 缺 tray——已在 `64381b0` 修复：Linux/macOS tar 命令追加 `custos-tray`
- [x] 审核发现：RPM 资产路径不随 target——已在 `64381b0` 修复：构建前把 `target/<matrix-triple>/release/custos` 复制到固定路径，资产路径不再碰巧依赖宿主三元组
- [x] 审核发现：`shell_quote` dead code——已在 `64381b0` 修复：迁入 `main.rs` 并被 macOS osascript 组合使用
- [x] 审核发现：MSI 版本硬编码——已在 `64381b0` 修复：nightly 打包前从 Cargo.toml 读取版本号并替换
- [x] 审核发现：超时断言弱化——已在 `64381b0` 修复：收紧到 `< 10s`（捕获 25s 级回归，同时给 CI 负载留余量）
- [x] 审核发现：RSS 平台混淆修复不完整——`resident_kb` 仍以 `#[cfg(unix)]` 编译（macOS 属于 unix），macOS 上读取 `/proc/<pid>/statm` 必然失败并返回 `Exited`，存活进程仍会误报 `Unhealthy "exited before RSS could be read"`；`Unsupported` 分支仅在非 unix（Windows）可达，与 commit message「macOS/Windows 不再误判」不符。`52de067`（`crates/custos-probe/src/process.rs:95-118`：`resident_kb`/`page_size_kb` 改 `#[cfg(target_os = "linux")]`，其余 unix 返回 `Unsupported`；custos-probe 174/174 绿）
- [x] 审核发现：macOS 同样走 `#[cfg(unix)]` 的 `is_alive`，用 `/proc/<pid>` 存在性判断进程存活，macOS 无 `/proc` 时恒为 `false`，进程探针在 macOS 上会把所有进程判为已退出（RSS 阈值与否均误报 Unhealthy）。`52de067`（`crates/custos-probe/src/process.rs:136-146`：`is_alive` 非 Linux unix 用 `kill(pid, 0)`——返回 0 或 EPERM 均判存活；custos-probe 174/174 绿）
- [x] 审核发现：install.sh 的 asset 匹配模式 `[^"]*custos-${asset_arch}[^"]*\.tar\.gz` 过于宽松且不含 libc 区分——x86_64 会同时命中 `custos-linux-x86_64` 与 `custos-linux-musl-x86_64`（取 head -1 顺序不确定），OpenWrt/iStoreOS 部署可能下载到 glibc 版而非 musl 静态版；且 aarch64 的 `custos-linux-aarch64` 与 `custos-linux-musl-aarch64` 也无法区分。`9df2af2`（`scripts/install.sh:61-74`：改精确匹配完整资产名 `custos-linux-<arch>`，OpenWrt 优先 musl；机制描述修正见下条）
- [x] 审核发现：install.sh 服务注册在 OpenWrt 分支写死的 `PROG=/usr/bin/custos` 与 `--db /etc/custos/custos.db`，但 `register_service` 仅在 `$PLATFORM=openwrt` 时把 `PREFIX` 设为 `/usr/bin`，且该初始化脚本不读取 UCI `custos` 配置的 `enabled`/`db`/`interval`，与 packaging/openwrt/etc/init.d/custos 的 UCI 驱动行为不一致（安装脚本版本无条件启动且不检查 config 是否存在）。`9df2af2`（`scripts/install.sh:126-151`：生成的 init.d 脚本改为 `config_load custos` + `config_get enabled`/`db` 校验，`enabled=0` 或 config 缺失不启动，并补 `stop_service`/`reload_service`，与 `packaging/openwrt/etc/init.d/custos` 一致）
- [x] 审核发现：build-ipk.sh 硬编码 `ARCH=x86_64`，且 nightly 的 ipk 重命名用 `sed 's/unknown-linux-musl//;s/-//g'` 从 target 推导 ARCH，二者都写死 x86_64；`db5d5db` 已删除 aarch64-musl 目标但 `todo.md:243` 仍宣称「nightly 新增 x86_64-unknown-linux-musl + aarch64-unknown-linux-musl 目标产出静态二进制」，文档与实现不符。`4f05216`（`packaging/openwrt/build-ipk.sh:14-20`：`ARCH` 改由 `file -b "$BINARY"` 从 ELF 机器推导（x86-64/aarch64），不再硬编码；用真实 musl 二进制构建出 `custos_0.1.0-1_x86_64.ipk` 验证。注：`todo.md:243` 原声明「nightly 新增 x86_64-unknown-linux-musl + aarch64-unknown-linux-musl 目标」与实际 workflow 不符——`nightly.yml` matrix 现仅含 `x86_64-unknown-linux-musl`（`static:true`，无 aarch64-musl），该行表述已在本次改为仅 x86_64 静态目标）
- [x] 审核发现：LuCI 状态页 `status.htm` fetch `/cgi-bin/luci/admin/services/custos/api/status`，但 controller `custos.lua` 只注册了 `cbi("custos")` 与 `template("custos/status")` 两个 entry，没有 `/api/status` 路由，`fetch` 恒 404，页面状态列永远显示 `(unavailable)`；且 CBI 模型没有提交/应用后的 procd 重启逻辑，`enabled`/`db`/`interval` 改动不会生效。`44a29e5`（`controller/custos.lua:18-33`：新增 `api_status` entry 返回 JSON（procd status + config 存在性）；`model/cbi/custos.lua:7-10`：`Map.on_commit` 触发 `/etc/init.d/custos restart`；两个 Lua 文件 `luac -p` 语法检查通过）
- [x] 审核发现：nightly `package msi` 步骤把 `crates/custos-cli/wix/main.wxs` 的 `Version="0.1.0"` 用字符串替换写回，若 wxs 版本与 Cargo.toml 不一致（或以后改用其他版本串）该替换静默不生效；同时 `main.wxs` 的 `Source` 路径为 `target\release\...`，而 cargo wix 在 `--package custos-cli` 下相对 cwd 解析，`cargo wix` 从 crates/custos-cli 执行时 `target\release` 相对于该目录可能不存在——路径与实际 staging 目录关系未验证。`5efea13`（`.github/workflows/nightly.yml:150-163`：替换改为正则 `(<Product\b[^>]*\bVersion=")[^"]*(")` 匹配任意当前版本；`cargo wix` 的 staging 目录改为 `crates/custos-cli/target/release`（与 wxs 相对包根解析一致）；正则已对实际 `main.wxs` 验证 `Version="0.1.0"`→`9.9.9` 命中）
- [x] 审核发现：ipk 的 `CONTROL/control` `Depends: libc` 与 musl 静态二进制（无 libc 依赖）矛盾，opkg 会要求 libc 包且对 musl 系统不必要；`Installed-Size: 15728640` 为固定值，与 `data.tar.gz` 实际体积不一致。`4f05216`（`packaging/openwrt/CONTROL/control:3`：`Depends:` 置空；`build-ipk.sh:45-47`：`Installed-Size` 改为 `du -sk "$PKGDIR"` 实测写入；构建产物 control 内 `Depends:` 空、`Installed-Size: 16168`）
- [x] 审核发现：`install.sh` 解包用 `tar -xzf ... custos` 只提取 `custos`，但 nightly tar 包含 `custos-tray`（64381b0 起）；桌面托盘二进制未随一键安装部署，且 systemd 单实例不启动 tray，文档「一键安装」与实际交付内容不完整。`9df2af2`（`scripts/install.sh:92-105`：解包同时提取 `custos` 与 `custos-tray` 并安装，tray 缺失不致命）
- [x] 审核发现：install.sh 的 asset 匹配正则 `[^"]*custos-${asset_arch}[^"]*\.tar\.gz` 实测（对真实 release URL `.../custos-linux-x86_64.tar.gz`、`.../custos-linux-musl-x86_64.tar.gz`、`.../custos-linux-aarch64.tar.gz`）命中**零个**——文件名中 `custos-` 后紧跟 `linux-`/`linux-musl-`，不含子串 `custos-x86_64`/`custos-aarch64`，`asset` 为空后必然 `die "no nightly ... tarball found"`，一键安装脚本在 GitHub 资产上不可用；todo.md 既有条目「x86_64 会同时命中 custos-linux-x86_64 与 custos-linux-musl-x86_64」的机制描述与实测不符（实际一个都不命中）。`9df2af2`（`scripts/install.sh:61-74`：改为按完整资产名精确匹配，`sh -n` 语法检查通过）
- [x] 审核发现：`default_console_command` 的 macOS 分支用 `shell_quote(&status_args.join(" "))` 把整条 `custos status --db <path>` 包成单个单引号词，再经 `osascript ... do script "'custos status --db /path'"` 交给 Terminal 的 shell——shell 把带引号整串解析为**一个**程序名（含空格/路径），找不到该可执行文件，「打开控制台」在 macOS 上仍失败（`64381b0` 只是改用 osascript，未修复引号粒度；应对每个参数分别 quote 后再 join）。`9df2af2`（`crates/custos-tray/src/main.rs:141-162`：`macos_console_script` 对每个参数独立 `shell_quote` 后 join；测试 `macos_console_command_quotes_each_argument_independently` 断言含空格路径为独立 shell 词；custos-tray 15/15 绿）
- [x] 审核发现：build-ipk.sh 的 `file -b` ARCH 提取用 `s/.*, \(x86-64\|aarch64\).*/\1/p`，但 GNU `file` 对 aarch64 ELF 输出 `ARM aarch64`（`aarch64` 前是空格而非 `, `），实测该 sed 不命中、ARCH 为空，随后 `die "unsupported binary architecture"`，aarch64 ipk 打包实际不可用（packaging/openwrt/build-ipk.sh:17） `e778772`（`packaging/openwrt/build-ipk.sh:17`：sed 正则放宽为 `[, ]` 匹配 aarch64 前空格；aarch64/x86-64 ELF 实测命中）
- [x] 审核发现：install.sh 的 `find_asset` 管道在 grep 无匹配时以退出码 1 结束，而 `set -eu` 下 `asset=$(find_asset ...)`（scripts/install.sh:76,79）命令替换失败会使整个脚本立即退出（dash 实测），OpenWrt 分支的 musl→glibc 回退 `[ -z "$asset" ] && asset=$(...)`（scripts/install.sh:77）永远执行不到；资产不存在时也直接死在 grep 而非给出友好 `die` 信息（scripts/install.sh:69-80） `e778772`（`scripts/install.sh:69-85`：release JSON 先 fetch 一次、`find_asset` 管道加 `|| true` 避免 `set -eu` 命令替换失败退出，OpenWrt musl→glibc 回退可执行）
- [x] 审核发现：双语重构后英文根 locale 首页 `docs/index.md` 三个 hero 链接仍为 `/en/guide/...`，但 `docs/en/` 已删除、英文页移入 `docs/guide/`，这些链接将 404（docs/index.md:11,14,17） `e778772`（`docs/index.md:11,14,17`：hero 链接改 `/guide/...`，与 `docs/guide/` 实际位置一致）
- [x] 审核发现：中文首页 `docs/zh/index.md` 三个 hero 链接为 `/guide/...`，缺少 `/zh/` locale 前缀，会解析到英文根 locale 页而非中文页（docs/zh/index.md:11,14,17） `e778772`（`docs/zh/index.md:11,14,17`：hero 链接改 `/zh/guide/...`）
- [x] 审核发现：install.sh 解包 `tar -xzf ... custos custos-tray || die`（scripts/install.sh:93-94）在 tar 不含 `custos-tray` 时退出码 2（实测），会触发 `die` 使整脚本失败，与注释「tray 缺失不致命」（scripts/install.sh:106-108）及「engine 为主产物」的意图不符；旧/缺 tray 的 tarball 会导致一键安装完全中止 `e778772`（`scripts/install.sh:94-102`：先解 `custos`，`custos-tray` 用 `if` 判定可选，缺失不致命）
- [x] 审核发现：LuCI CBI 暴露 `db`/`interval` 选项（packaging/openwrt/usr/lib/lua/luci/model/cbi/custos.lua:19-25），但 `/etc/init.d/custos` 仅 `config_get enabled`（packaging/openwrt/etc/init.d/custos:12），`CONF_DB`/`CONF_TOML` 为硬编码（init.d/custos:6-7），新增 `on_commit` 重启（model/cbi/custos.lua:9-11）声称「db/interval 改动立即生效」实际不生效——`db` UCI 值被忽略、`interval` 根本不参与 `custos start`，CBI 中这两项为误导性死选项 `e778772`（`packaging/openwrt/etc/init.d/custos:8,12-16,23` 与 `scripts/install.sh:133-149`：init 脚本读取 UCI `db`/`interval` 并传入 `custos start`，CBI 选项生效）
- [x] 审核发现：LuCI controller 将 `api_status` 注册为 `entry(..., call("api_status"), _("状态 API"), 90).leaf = true`（packaging/openwrt/usr/lib/lua/luci/controller/custos.lua:20-22），会在 LuCI 菜单出现「状态 API」用户可见项，点击直接展示原始 JSON，而非作为内部路由（低严重度） `e778772`（`packaging/openwrt/usr/lib/lua/luci/controller/custos.lua:19-20`：移除菜单标题与优先级，`api/status` 改为纯内部路由）

- [x] **DOC-GIF · P2 · 文档 · 配置开关效果动态图(每个开关 off→on 对比+GIF)** `d756d03`（`scripts/generate-config-effects.sh` 驱动真实 custos 生成 4 组 off/on PNG+2 帧 GIF（dashboard.enabled / resources[].enabled / notification 通道 local-log / recovery_policy.allow_high_risk），双语 `docs/{guide,zh/guide}/config-effects.md` 嵌入，`pnpm docs:build` 绿、`bash -n` 通过、GIF 均 2 帧有效）
  - 交付:枚举本仓全部「开启后有效果」的开关/配置项;自动流水线生成证据——GUI 类用 @vscode/test-electron 或应用截图(off 一张/on 一张+切换过程录屏),CLI 类用终端录屏(asciinema→GIF 或 ffmpeg);ffmpeg 缝合动态 GIF。
  - 文档:docs「配置效果」页(双语)逐开关嵌入:说明+off 图+on 图+GIF。
  - 验收:清单内每个开关都有 off/on+GIF;流水线可重跑(配置变更随时再生成);兼作配置效果回归测试。
  - 参考:kode todo C-24 同款流水线模式(2026-11-01 用户令全舰队推广)。
- [x] 审核发现：`docs.yml` 的 deploy 环境 URL 写成 `url: \${{ steps.deployment.outputs.page_url }}`（.github/workflows/docs.yml:59），YAML 纯量中 `\$` 是字面反斜杠+`$`，GitHub Actions 展开 `${{ }}` 后 URL 值带有前导 `\`（如 `\https://...`），deploy 环境的页面 URL 链接会失效；正确写法应为不加转义反斜杠的 `${{ steps.deployment.outputs.page_url }}`。`ac70f31`（`.github/workflows/docs.yml:59`：`url: \${{ ... }}`→`url: ${{ ... }}`，去转义反斜杠；`pnpm docs:build` 绿）
- [x] 审核发现：`c00d174` 折叠 README 徽章时把 docs 徽章 `<img>` 从 `<a>` 中移出并遗留一个空 `<a href="https://cuihairu.github.io/custos//"></a>`（README.md:6、README.zh.md:6），docs 徽章失去可点击链接，且该空锚点 URL 含双斜杠 `custos//`（README.md:6）。`a094953`（`README.md:6`：docs 徽章 `<img>` 重新包回 `<a href="https://cuihairu.github.io/custos/">` 内、删除空锚点与双斜杠；`README.zh.md:6` 同（`.../custos/zh/`）；`pnpm docs:build` 绿）

## 审核批次：c00d1742d6e7f6e5e123e4fc2aeb64da8b2b3bdb..HEAD（只审不改）

覆盖范围：`crates/custos-tray/src/main.rs`、`crates/custos-tray/build.rs`、`crates/custos-tray/Cargo.toml`、`crates/custos-cli/build.rs`、`crates/custos-cli/Cargo.toml`、`crates/custos-cli/wix/main.wxs`、`.github/workflows/nightly.yml`、`.github/workflows/docs.yml`、`docs/.vitepress/theme/*`、`docs/.vitepress/theme/components/*`、`scripts/make-windows-icon.py`、`package.json`、`pnpm-lock.yaml`、README/README.zh、docs guide 与 index 文档。已本地 `cargo check -p custos-tray` + `cargo test -p custos-tray`（15/15 绿）验证。

- [x] 审核发现：MSI 的 `main.wxs` 以相对路径引用仓库根文件 `LICENSE`/`README.md`/`README.zh.md`/`docs\*.md`/`examples\*.toml` 与 `Icon SourceFile="assets\icons\custos.ico"`（crates/custos-cli/wix/main.wxs:10,69,72,75,83-89,98-102…），但这些文件只在 workspace 根存在（实测 `crates/custos-cli/` 下无 LICENSE/README/docs/assets）；而 cargo-wix 官方说明相对 Source 路径以「项目根 = 该包 Cargo.toml 目录」解析（`crates/custos-cli`），此前 `5efea13` 正是因该解析基准把 exe staging 到 `crates/custos-cli/target/release`。新 wxs 的 docs/license/icon/example 源在包根下均不存在，`cargo wix --package custos-cli` 的 candle/light 将报找不到文件——MSI 打包步骤（.github/workflows/nightly.yml:172-195）很可能失败或产出缺 docs/LICENSE/图标的安装包，需实机验证解析基准（crates/custos-cli/wix/main.wxs:10）。`e0fa28c`（`crates/custos-cli/wix/main.wxs`：全部非 `$(var.CargoTargetBinDir)` 的 `Source`/`SourceFile` 加 `..\..\` 前缀，从包根解析到 workspace 根（`LICENSE`/`README.md`/`README.zh.md`/`examples\*.toml`/`docs\*.md`/`assets\icons\custos.ico` 均存在）；42 个相对路径逐一对包根解析全部命中；XML well-formed；`cargo test --workspace` 全绿）
- [x] 审核发现：`setupMermaid()` 每次调用都 `new MutationObserver(...).observe(...)` 且从不 disconnect（docs/.vitepress/theme/mermaidClient.ts:66-74），而 `index.ts` 的 `watch(route.path)` 在每次 SPA 路由切换都调用 `setupMermaid()`（docs/.vitepress/theme/index.ts:17-24）——观察器随会话无界累积；每次 `<html>` class 变化（明/暗主题切换）全部累积观察器各触发一次 `enqueue()`，造成重复的 mermaid 全页重扫与内存泄漏（mermaidClient.ts:70）。`e0fa28c`（`docs/.vitepress/theme/mermaidClient.ts`：`setupMermaid` 改为模块级单例 `themeObserver`，仅在首次调用时创建并 observe，SPA 路由切换不再累积观察器；`pnpm docs:build` 绿）
- [x] 审核发现：托盘日志在配置目录不可写时回退 `std::io::sink()` 静默丢弃全部 tracing，且该分支的 `eprintln!` 警告在无控制台的 windowed Windows 发行版中不可见——只读安装目录（如 Program Files）下托盘运行但零可诊断日志、无任何用户可见信号（crates/custos-tray/src/main.rs:232-238）。`e0fa28c`（`crates/custos-tray/src/main.rs`：`init_tracing_file` 在配置目录不可写时先尝试系统临时目录，成功则把日志写到 `$TEMP` 并在订阅器安装后 `tracing::warn!` 记录重定位（窗口版可见），仅当临时目录也不可写时才回退 `io::sink()`；新增 `init_tracing_file_falls_back_to_temp_dir_on_readonly` 测试；`cargo test -p custos-tray` 14+10 全绿 + `cargo test --workspace` 全绿）
- [x] 审核发现：`make-windows-icon.py` 工具探测硬编码 `/usr/bin/{rsvg-convert,magick}`（scripts/make-windows-icon.py:38），macOS（Homebrew `/opt/homebrew/bin`）或 `/usr/local/bin` 安装时工具存在却被误报 not-found 退出 1；且 `subprocess.run(..., check=True)` 失败抛出未处理 `CalledProcessError`，无友好提示（scripts/make-windows-icon.py:47-54）。`e0fa28c`（`scripts/make-windows-icon.py`：改用 `shutil.which` 沿 PATH 解析 `rsvg-convert`/`magick`（覆盖 Homebrew/`/usr/local/bin`）；`subprocess.run` 包 `try/except CalledProcessError`，输出工具名/退出码/捕获的 stderr+stdout 后返回 1；`python3 -m py_compile` 通过）
- [x] 审核发现：新增的延迟参数错误路径（`--db`/`--config` 缺值、未知参数、`--help`）与 `init_tracing_file`（日志目录推导、不可写目录 sink 回退）均无单元测试覆盖，`cargo test` 仅覆盖 `macos_console_script`——参数解析与文件日志器回归无法被 CI 捕获（crates/custos-tray/src/main.rs:60-88,211-249）。`e0fa28c`（`crates/custos-tray/src/main.rs`：`parse_args` 提取为纯函数，新增 `parse_args_db_requires_value`/`parse_args_config_requires_value`/`parse_args_unknown_argument_is_an_error`/`parse_args_help_and_short_help`/`parse_args_defaults_without_arguments`/`parse_args_assigns_db_and_config`/`parse_args_console_consumes_the_rest` 七测试 + `init_tracing_file_writes_a_log_file`（日志目录推导）与 `init_tracing_file_falls_back_to_temp_dir_on_readonly`（不可写回退）两测试，共 9 个新测试；`cargo test -p custos-tray` 14+10 全绿 + `cargo test --workspace` 全绿）

## 审核发现 2026-11-03（ac70f315..HEAD，只审不改）

- [x] 审核发现：`make-windows-icon.py` 新增的 `except CalledProcessError` 处理器调用 `exc.stderr.strip()`，但对未设 `capture_output` 的 `rsvg-convert`/`magick` 转换调用失败时 `exc.stderr`/`exc.stdout` 均为 `None`，`None.strip()` 抛 `AttributeError`，脚本以未处理 traceback 崩溃而非提交声称的「带退出码与输出的友好提示」（该友好路径只在设了 `capture_output` 的 `identify` 调用时可用）。（scripts/make-windows-icon.py:73）`b7783d7`（`scripts/make-windows-icon.py`：错误处理器改为 `(exc.stderr or "").strip() or (exc.stdout or "").strip()`，`None` 时回退到整条 `CalledProcessError` 的友好字符串；`python3 -m py_compile` 通过；None 输出与捕获输出两条失败路径模拟均正常）
- [x] 审核发现：`init_tracing_file_falls_back_to_temp_dir_on_readonly` 整体包在 `#[cfg(unix)]` 内、在 Windows/macOS 上是空 no-op，而临时目录回退正是为窗口版 Windows（只读安装目录）新增的行为，目标平台 Windows CI 上「只读目录触发 + 回退」路径不被执行；「双目录皆不可写 → io::sink」分支与重定位告警分支亦无测试。（crates/custos-tray/src/main.rs:439）`ad5885a`（`crates/custos-tray/src/main.rs`：`select_log_writer` 抽为可注入 temp 目录的纯函数，测试用「普通文件顶替目录」在任意平台（含 Windows/macOS）确定性触发只读回退与双目录皆不可写→io::sink 丢弃；新增 `init_tracing_file_falls_back_to_temp_dir_on_readonly`（跨平台回退）与 `init_tracing_file_discards_when_both_dirs_unwritable` 两测试；`cargo test -p custos-tray` 25 全绿 + `cargo test --workspace` 全绿）
- [x] 审核发现：`init_tracing_file_writes_a_log_file` 仅断言日志文件存在（该文件由 appender `build()` 立即创建），未断言日志行确实被路由写入；`cargo test` 并行执行时全局 `try_init` 订阅器可能归属另一个 `init_tracing_file` 测试，故「实际写入日志文件」这一断言较弱。（crates/custos-tray/src/main.rs:412）`ad5885a`（`crates/custos-tray/src/main.rs`：`init_tracing_file_writes_a_log_file` 改用 `select_log_writer` + 线程局部 `tracing::subscriber::with_default` 建立订阅器，`assert_log_contains` 断言日志行 `log-file smoke test` 确实被写入文件；`cargo test -p custos-tray` 25 全绿 + `cargo test --workspace` 全绿）
- [x] 审核发现：`unique_dir` 的注释声称「The counter keeps the name unique within a process; the timestamp across processes」，但实现仅用进程内 `AtomicU64` 计数器、格式串 `custos-tray-test-log-{tag}-{}` 不含时间戳——跨进程唯一性实际不存在；两个并发测试进程（并行 CI 任务、或用户与 CI 同时跑 `cargo test`）会生成相同临时目录名 `custos-tray-test-log-<tag>-<n>`，可能互相删除对方目录或同写一个 `custos-tray.log`，测试存在低概率跨进程碰撞（注释与实现不符，属误导性文档 + 潜在 flaky）。（crates/custos-tray/src/main.rs:455-462）`3433235`（`crates/custos-tray/src/main.rs`：`unique_dir` 改含进程 ID + 纳秒时间戳 + 进程内计数器三要素，注释与实现一致，跨进程目录名碰撞消除；`cargo test -p custos-tray` 25 全绿 + `cargo test --workspace` 全绿 + `cargo check --target x86_64-pc-windows-gnu` 通过）
- [x] 审核发现：日志「双目录皆不可写 → io::sink 丢弃」分支（config 目录与临时目录均不可写）的唯一诊断是 `eprintln!` 警告（main.rs:303-308），其在 `#![cfg_attr(not(debug_assertions), windows_subsystem="windows")]` 的窗口版 Windows 发行版中不可见；且该分支不像 `relocated` 那样发 `tracing::warn!`，也没有日志文件可写——窗口版用户在该分支仍得到「零可诊断日志 + 零用户可见信号」，正是本批次声称消除的不可诊断状态（虽属极端边角：临时目录对用户会话几乎总是可写，但与批目标同类）。（crates/custos-tray/src/main.rs:302-311）`3433235`（`crates/custos-tray/src/main.rs`：双目录不可写分支改发 `tracing::warn!` + 新增 `warn_log_discarded`：窗口版 Windows 弹原生 `MessageBoxW`（`MB_OK|MB_ICONWARNING`）作为用户可见信号，非 Windows 仍落 stderr；`cargo test -p custos-tray` 25 全绿 + `cargo test --workspace` 全绿 + `cargo check --target x86_64-pc-windows-gnu` 通过）

## 审核发现 2026-11-03（8fe915f0..HEAD，只审不改）

- [x] 审核发现：日志「双目录皆不可写」分支的 `tracing::warn!` 在订阅器安装前执行、事件被丢弃 → `fe1261b`（`LogOutcome` 新增 `warning` 字段，`select_log_writer` 延迟收集、`init_tracing_file` 在 `try_init()` 后统一发出；`cargo test -p custos-tray` 25 全绿 + `cargo test --workspace` 全绿 + `cargo check --target x86_64-pc-windows-gnu` 通过）
- [x] 审核发现：跨平台测试在 Windows 触发阻塞式 `MessageBoxW` → `4f36889`（`warn_log_discarded` 在 `#[cfg(test)]` 下改写 stderr、生产构建保持 `MessageBoxW`；`cargo test -p custos-tray` 25 全绿 + `cargo check --workspace --tests --target x86_64-pc-windows-gnu` 编译通过）
- [x] 审核发现：`windows-sys` 未使用的 `Win32_System_Threading` feature → `628f0bb`（移除该 feature，仅保留 `Win32_Foundation` + `Win32_UI_WindowsAndMessaging`；`cargo check --workspace --target x86_64-pc-windows-gnu` 通过）
- [x] 审核发现：config-effects 文档「所有」过度声明 → `2b561fa`（en+zh 改为「四项用户可见配置开关」/「the four user-facing switches」精确口径，与生成器实际覆盖一致）
- [x] 审核发现：`generate-config-effects.sh` 缺少 `convert` 存在性检查 → `b761d12`（`set -euo pipefail` 前加 `command -v convert` 检查 + `bash -n` 通过）

## 审核发现 2026-11-04（8fe915f0..0587d4d，只审不改）

- [x] 审核发现：本批次新增的「警告延迟到订阅器安装后发出」（`init_tracing_file` 内 `if let Some(msg) = &outcome.warning { tracing::warn!(...) }`）无任何测试执行——三个 `init_tracing_file_*` 测试全部直接调用 `select_log_writer`、从不调用 `init_tracing_file`，故 relocated 分支的延迟告警行是否真正落入日志文件未被断言；删除该发射的回归不会被测试捕获（crates/custos-tray/src/main.rs:396） → 属实（crates/custos-tray/src/main.rs:378,407-409,432-437：抽出 `emit_deferred_warnings` 并由 `init_tracing_file_in` 调用；新增 `init_tracing_file_emits_deferred_relocation_warning` 测试经真实 `init_tracing_file_in` 驱动 relocation 分支并断言 `cannot open log file in`/`falling back to` 落入 relocated 日志文件；`cargo test -p custos-tray` 27 全绿 + `cargo test --workspace` 全绿 + `cargo check --target x86_64-pc-windows-gnu` 通过）
- [x] 审核发现：`frame` 把抓取到的原始终端文本作为内联 `caption:` 实参传给 ImageMagick，未转义 `%`——ImageMagick 对 `caption:` 原地展开 `%` 格式转义（已验证 `caption:abc%wdef` 与 `caption:abc%%wdef` 渲染结果不同且 `convert` 仍退出 0），故捕获文本一旦含 `%` 后随格式字母（如未来 dashboard HTML/URL/百分比、日志行内 URL），证据 PNG 静默渲染错误文本而脚本不报错；当前 html_shell 前 200 字节恰好无 `%`，属潜在正确性隐患（scripts/generate-config-effects.sh:32） → 属实（scripts/generate-config-effects.sh:30-31,35-47：新增 `im_escape_percent` 用 `sed 's/%/%%/g'` 转义，`frame` 在传给 `caption:` 前调用；实测 `disk 100%% full`/`https://x/a%%20b` 经转义后不再被 ImageMagick 展开；`bash scripts/generate-config-effects.sh` 4/4 断言绿、证据 PNG/GIF 重新生成）
- [x] 审核发现：dashboard 证据用硬编码 `PORT=18095` 抓 off 态，未先验证端口空闲——若上次运行被中断遗留 dashboard 监听（脚本自身恰在该端口起服务再 kill），或任意无关进程占用 18095，off 帧 `curl` 将拿到 HTTP 200，证据显示「off 态也有监听」与文档「enabled=false 无监听」矛盾，且 off/on 帧相同致 GIF 无切换（scripts/generate-config-effects.sh:47,75） → 属实（scripts/generate-config-effects.sh:88-108,113：新增 `pick_dashboard_port` 从候选端口（默认 18095 + 18096-18100）中挑一个当前无监听者；实测 18095 被无关进程占用时脚本不再产出错误 off 帧；`bash scripts/generate-config-effects.sh` 4/4 断言绿）
- [x] 审核发现：relocated 分支告警消息保留字面 `"warning: "` 前缀（`format!("warning: cannot open log file in {} ...")`），而 discard 分支消息无此前缀（`format!("cannot open log file in {} ...")`），两者均经 `tracing::warn!`（:396）发出——relocated 日志行读作 `WARN ...: warning: cannot open...`，前缀在 WARN 级别冗余且两分支不一致（crates/custos-tray/src/main.rs:296,307） → 属实（crates/custos-tray/src/main.rs:296：relocated 消息去掉 `"warning: "` 字面前缀，与 discard 分支一致；新增 `deferred_warning_messages_have_no_redundant_prefix` 测试断言 relocated 消息不以 `warning:` 开头且以 `cannot open log file` 开头；`cargo test -p custos-tray` 27 全绿）
- [x] 审核发现：文档宣称该脚本「兼作配置效果回归测试」（config-effects.md:3），但脚本对每次 `custos` 调用都以 `|| true`/`|| echo` 容忍失败、且从不断言 on 态确实产生预期效果（res-on 记录到 run、nl-on 产生审计行、hr-off 确实被拒）——开关语义一旦失效，脚本仍生成空/误导证据并以退出码 0 结束，与「回归测试」宣称不符（scripts/generate-config-effects.sh:73,145,187,233,237 与 docs/guide/config-effects.md:3） → 属实（scripts/generate-config-effects.sh:60-84：新增 `assert_grep`/`assert_not_grep` 断言助手；每组 off/on 后断言实际效果——dash off 无监听（curl 退出码非0）/on HTTP 200、res off `no recovery runs recorded`/on 出现 `run-.*demo`、nl off `no structured audit line`/on 出现 `event_type`、hr off `validate exit=1`/on `validate exit=0`；任何失效以非零退出终止；`bash scripts/generate-config-effects.sh` 4/4 断言绿）

## 审核发现 2026-11-04（0587d4d888e2..ddd2267，只审不改）

- [x] 审核发现：`assert_not_grep` 定义后从未被调用——dashboard OFF 检查改用内联 `if grep -q "curl exit=0"`（:153）而非该助手，`assert_not_grep` 成为死代码（scripts/generate-config-effects.sh:77） → 属实（scripts/generate-config-effects.sh:159：dashboard OFF 检查改为调用 `assert_not_grep "curl exit=0"`，原内联 `if grep -q` 删除，助手不再死代码；`bash scripts/generate-config-effects.sh` 9/9 断言绿） `297866f`
- [x] 审核发现：新测试注释声称「A prior global subscriber ... would make this a no-op ... written through the relocated writer regardless」不成立——若 `try_init` 因已存在全局订阅器而 no-op，`emit_deferred_warnings` 的 `tracing::warn!` 会派发到既有订阅器而非 relocated 文件，`assert_log_contains` 将失败；当前仅因该测试是唯一调用 `try_init` 者才通过，Rust 测试并行无顺序保证，未来任一安装全局订阅器的测试都会使其 flaky（crates/custos-tray/src/main.rs:680-683） → 属实（crates/custos-tray/src/main.rs:425-447：`emit_deferred_warnings` 改为显式接收 writer 并经 `tracing::subscriber::with_default` 在所选 writer 上发出，即使已有全局订阅器也落 relocated 文件；新增 `deferred_warnings_reach_selected_writer_despite_existing_global_subscriber` 预装全局订阅器后断言三个消息落入 relocated 日志；`cargo test -p custos-tray` 28 全绿） `297866f`
- [x] 审核发现：`curl` 被依赖但未在 requirements 检查中声明——缺 curl 时 `pick_dashboard_port` 把退出码 127 当「端口空闲」错选默认端口，OFF 帧以 `curl exit=127` 误导性通过、ON 帧以困惑的 HTTP 200 断言失败；新增 `pick_dashboard_port` 加深依赖却未加 `command -v curl` 守卫（scripts/generate-config-effects.sh:29-30,97,143,166） → 属实（scripts/generate-config-effects.sh:29：脚本顶部新增 `command -v curl >/dev/null || { echo "curl required"; exit 1; }` 守卫，缺 curl 立即退出而非把退出码 127 当「端口空闲」；`bash scripts/generate-config-effects.sh` 9/9 断言绿） `297866f`
- [x] 审核发现：结尾 `regression assertions: 4/4 passed` 为硬编码且计数不符——实为 8 个 `assert_grep` + 1 个内联 curl 检查共 9 项断言，「4/4」按 4 个开关类别计而非断言数，新增类别不会自动更新（scripts/generate-config-effects.sh:360） → 属实（scripts/generate-config-effects.sh:65,362：新增 `ASSERT_COUNT` 计数器于 `assert_grep`/`assert_not_grep` 内自增，结尾输出 `$ASSERT_COUNT/$ASSERT_COUNT passed` 动态计数，不再硬编码；实测 `9/9 passed` 对应 9 项断言；`bash scripts/generate-config-effects.sh` 9/9 断言绿） `297866f`

## 审核发现 2026-11-04（ddd2267b..HEAD，只审不改）

- [x] 审核发现：本轮 diff 未见问题（ddd2267b..HEAD）→ 属实（独立复核 `ddd2267b..HEAD`（`297866f`+`6ce4237`）：deferred-warning 经 `with_default` 路由到所选 writer、curl 守卫、`assert_not_grep` 调用、`ASSERT_COUNT` 动态计数均与描述一致，无新增问题；`cargo test -p custos-tray` 28 全绿 + `cargo test --workspace` 508 全绿 + `bash scripts/generate-config-effects.sh` 9/9 断言绿）
- [x] 审核发现：新增的内容宽度规则被 VitePress 默认主题带 scoped 属性的同名选择器覆盖，含侧栏目录的页面仍受 43rem 限制，未实现预期的 960px 内容宽度（docs/.vitepress/theme/custom.css:12） → 属实但已修复（3b70052b 时点属实：custom.css 的 `.VPDoc.has-aside .content-container`（特异度 0-2-0）无 `!important`，被 VPDoc.vue scoped 后的 `.VPDoc.has-aside .content-container[data-v-…]`（0-3-0）`max-width: 43rem` 覆盖；`1e68d34` 已为三条宽度规则补 `!important`。HEAD 实测：`pnpm docs:build` 后 preview 实测 has-aside 页 `/custos/guide/overview.html`，`.VPDoc.has-aside .content-container` computed max-width = `960px`，43rem 规则不再生效） `1e68d34`
- [x] 审核发现：本轮 diff 未见问题（3b70052b..HEAD）→ 属实（独立复核 `3b70052b..1e68d34`（本轮审核时点的 HEAD）：纯 docs 改动——markdown 长行解包 + custom.css 三条 960px 宽度规则补 `!important`，无代码路径变更；docs 站 `pnpm docs:build` 绿）

## 审核发现 2026-11-09（80ec03609f..HEAD 复核，逐项修复）

- [x] AUD-A [P2] `platform/controller.rs:94-102` `resolve_target`（R16/`0719cab`）元组 `(host, port).to_socket_addrs()` 拒绝括号 IPv6 字面量——`("[2001:db8::1]", 443)` 走 getaddrinfo 报 Err → `PlatformError::Failed("invalid target")`，而修复前的 `"[2001:db8::1]:443".parse::<SocketAddr>()` 恰好接受该规范写法：修好裸 IPv6 的同时把括号写法弄坏（回归）。✅ commit 0ec953f，`resolve_target` 重写三段式：剥 `[`/`]` 后先 `parse::<IpAddr>()` 直解析（IP 字面量——裸/括号两种写法——永不再碰 DNS），字面量路径外的主机名才回退元组 `to_socket_addrs`；查找失败改报 `target host unresolved: <host> (<OS 细节>)`，与非法输入分型；地址族选择保留 getaddrinfo 的 RFC 6724 排序（系统首选目的地，Linux/Windows 皆然），DNS 解析无超时为已知限制（引擎侧看门狗属 C20 范畴，不在本项修）。红先测试 `resolve_target_accepts_bracketed_ipv6_literal`（运行红：`("[2001:db8::1]", 443)` 报 invalid target）+ `resolve_target_labels_dns_failure_distinctly`（运行红；1100 字符超长名在 getaddrinfo 内部确定性失败，不依赖网络——本机 fake-IP 解析器对真实域名照单全收）；测试 562 通过/0 失败
- [x] AUD-B [P2] `platform/windows.rs:483-486` `reset()`（R13/`c9cc5f7`）`admin=disabled` 成功后 `admin=enabled` 失败即 `?` 直接返回，无重试/状态校验——网卡停留在管理性 down，恢复动作反向永久切断管理通道（正是 windows.rs:435-438 文档警告的 HIGH RISK）。✅ commit b3328f6，新增跨平台纯函数 `run_enable_with_retry`（4 次重试、500ms 间隔；穷尽后错误明示 `interface left administratively DOWN` 并带最后 netsh 细节，不再裸报），`reset()` 的 re-enable 步骤接入；红先测试 `reset_reenable_retries_then_names_the_nic_left_down`（新函数：前 2 次瞬时失败第 3 次恢复、5 次全败错误含接口名/DOWN 状态/原因；编译红→绿）；`cargo check --target x86_64-pc-windows-gnu` 通过；测试 562 通过/0 失败
- [x] AUD-C [P3] `plugins/mihomo/src/main.rs:108,121`（R15/`3552065`）新文档注释声称跳过含「自動/自动」的选择器成员，代码只 `contains("自动")`（简体）——繁体「自動」选择器仍可能被选为切换目标，恢复把组指到另一个选择器而非真实节点。✅ commit 1fad8cc，`next_plain_member` 过滤加 `contains("自動")`，代码对齐文档承诺；红先测试 `next_plain_member_skips_traditional_selectors_too`（运行红：`[X, 自動選擇, Y]` 从 X 轮转旧代码选中「自動選擇」）；测试 562 通过/0 失败
- ~~审核发现（不属实，已删除）：cloudflared `strip_prefix` 把 `# HELP/TYPE` 注释行误判为 gauge~~ 不属实：实证 `strip_prefix("cloudflared_tunnel_ha_connections")` 对 `#` 开头行恒返回 None（`"# TYPE ..."` 以 `#` 起始，前缀必然失配），注释行从不进入 gauge 解析；复核 agent 给出的触发构造无法复现。
- [x] AUD-E [P3] 文档源码锚点过期：`ef1226b`/`b73bb3c` 把 `custos-config/src/notification.rs` 拆为 `notification/mod.rs`+`raw.rs` 后，docs/guide 与 docs/zh 的 glossary.md:13-14、source-architecture.md:65,67 共 8 处仍指向已删除路径。✅ commit 347b538（4 文件 8 锚点改指 `notification/mod.rs` 对应行：规则 34→33、通道判别 55→54、SsrfPolicy 115→114、SecretString 234→236）

## 审核发现 2026-11-05（openclaw 只读侦察，逐项修复）

- [x] 网关解析空行 bug：`parse_route_print_default_gateway` 对每行用 `fields.next()?`，真实 `route print -4` 输出 Interface List 段后的空行导致整体返回 None，Windows 默认网关探针恒 Unhealthy → 属实（`8b2469a`：改 let-else `continue` 跳过无字段行/表头，镜像 windows.rs `parse_default_gateway`；fixture 换真实格式——Interface List + 空行 + 双默认路由（含 On-link 行）+ Persistent Routes，`cargo test -p custos-probe netif` 7 绿）
- [x] CP936 解码 bug：`read_capped` 用 `String::from_utf8_lossy`，zh-CN Windows 控制台 GBK（OEM 代码页 CP936）输出的非 ASCII 字节全变 U+FFFD → 属实（`e6c1944`：新增 `decode_console_output`——有效 UTF-8 原样通过，否则 encoding_rs GBK 解码（关 BOM 嗅探，防 UTF-16LE BOM 吃掉首字节）；GBK fixture 测试 `output_decoding_prefers_utf8_then_gbk` 覆盖 UTF-8 直通/GBK 解码/混合行/非法字节降级）
- [x] wifi.reconnect 虚假声明：`capabilities()` 无条件 `.native("wifi.reconnect")`，但 `netsh_wlan_connect_argv` 全仓无执行路径、无适配器探测 → 属实（`df76651`：改 `.unsupported("wifi.reconnect", "netsh wlan connect is built but no Wi-Fi adapter probe or execution path is wired yet")`，argv builder 文档标注 UNWIRED，新增测试 `capabilities_do_not_claim_unwired_wifi_reconnect` 断言门拒并带具体理由）
- [x] wifi.reconnect 接线缺口（后续工作）：补齐需 ① 适配器探测（如 `netsh wlan show interfaces`）守卫能力声明 + ② toolbox/dispatch 执行路径真正运行 `netsh wlan connect name=<profile>`，二者齐备后方可改回 `.native` → 已完成（① `crates/custos-platform/src/windows.rs:341-355`：新增 `has_wifi_adapter` 经 `netsh wlan show interfaces` 探测，绝不猜执行；`capabilities()`（:515-533）在 Windows 上按 `has_wifi_adapter()` 动态声明 `wifi.reconnect` native/unsupported——探测到适配器即 `.native`，否则 unsupported 带具体原因；`NetshNetworkController::connect_wifi`（:408-428）先探测适配器再运行 `netsh wlan connect name=<profile>`，非 Windows 明报 Unsupported；② `crates/custos-toolbox/src/platform_tools.rs:150-228`：新增 `WifiReconnectTool`（`wifi.reconnect`，`host.network` 权限门先于执行 + `NETWORK_OPS` 互斥 + 适配器 Unsupported 上抛），`NetworkInterfaceController` 增 `connect_wifi` 默认 Unsupported；`cargo test -p custos-platform` 18 全绿 + `cargo test -p custos-toolbox` 22 全绿 + `cargo check --target x86_64-pc-windows-gnu` 通过）

## 审核发现 2026-11-05（1e68d34774..bba0dd0a70，只审不改）

- [x] 审核发现：`parse_route_print_default_gateway` 由 `?` 改 let-else `continue` 后改为全量扫描整份 `route print -4` 输出，会扫过 Active Routes 段进入 Persistent Routes 段——当活动路由表无 0.0.0.0/0 网关行（如网卡断开、活动默认路由丢失）而 Persistent Routes 仍保留配置的默认网关时，解析器返回该**非活动**网关，探针误报 Healthy"default gateway present"（旧代码在首个空行即返回 None，扫不到 Persistent 段，故该泄漏系本 diff 引入）；fixture 只覆盖了 Active 段同时含网关行的路径，未覆盖"仅 Persistent 有默认路由"的情形。建议：扫描遇到 `Persistent Routes:` 段首行即停（只认 Active Routes 段），并补一条"仅 Persistent 有默认路由 → None"的用例（crates/custos-probe/src/netif.rs:303）。 → 属实（`crates/custos-probe/src/netif.rs:304-310`：扫描在 `Persistent Routes` 段首行 `break`，只认 Active Routes 段；新增 `route_print_parser_ignores_persistent_only_default_gateway` 断言"仅 Persistent 有默认路由 → None"；`cargo test -p custos-probe` 176 全绿）
- [x] 审核发现：`decode_console_output` 先做 UTF-8 直通，部分 GBK 字节序列恰好构成合法 UTF-8（如 默=0xC4AC→U+012C）会在整行碰巧合法 UTF-8 时被误解码为 UTF-8 而非 GBK（非回归，旧 `from_utf8_lossy` 同样 UTF-8 直通；后果仅展示性乱码，不影响 ASCII 路由解析）——属启发式固有歧义，可接受但宜在注释中言明（crates/custos-probe/src/exec.rs:172）。 → 属实但可接受（`crates/custos-probe/src/exec.rs:173-180`：注释已言明 UTF-8 直通的固有歧义——仅影响非 ASCII 展示、不影响 ASCII 路由解析，且与旧 `from_utf8_lossy` 同偏好；`cargo test -p custos-probe` 176 全绿）
- [x] 审核发现：解码硬编码 GBK（CP936）而未取真实 OEM 代码页（GetOEMCP/chcp），注释声明"仅支持 zh-CN Windows 部署"但代码/配置未强制该范围——若在非 zh-CN OEM 页（如 CP437 英文）Windows 上运行，非 UTF-8 输出会被错误 GBK 解码为乱码（crates/custos-probe/src/exec.rs:166）。 → 属实（`crates/custos-probe/src/exec.rs:187-218`：新增 `oem_code_page_encoding`，经 `GetOEMCP` 取真实 OEM 代码页并映射到 encoding_rs 编码（932/936/949/950/866/874/1250-1258），非 zh-CN 页正确解码，未知页回退 GBK；`Win32_Globalization` feature 启用；`cargo check -p custos-probe --target x86_64-pc-windows-gnu` 通过）
- [x] 审核发现：新测试 `capabilities_do_not_claim_unwired_wifi_reconnect` 仅抽查 `network.interface.reset`/`dns.flush` 仍为 native，未断言其余既有 native 操作（如 `process.detect`/`service.control`/`system.reboot`）未被误删，回归保护不足（crates/custos-platform/tests/windows_parsers.rs:175）。 → 属实（`crates/custos-platform/tests/windows_parsers.rs:178-191`：改为遍历全部既有 native 操作（process.detect/service.status/start/stop/restart/network.interface.list/reset/network.connectivity.tcp/dns.flush/system.reboot）断言仍为 native；`cargo test -p custos-platform` 10 全绿）
- [x] 审核发现：`capabilities()` 已将 `wifi.reconnect` 改声明为 unsupported，但 docs/guide/platforms.md:29 仍把"Wi-Fi reconnection"列为 Windows 能力项（该列表为"to support progressively"路线图，非当前支持声明，故非硬伤，但与本 diff 的能力集矛盾，易误导读者认为当前已支持）（docs/guide/platforms.md:29）。 → 属实（`docs/guide/platforms.md:29`：Wi-Fi reconnection 项追加"built, not yet wired — see Capabilities for the current `unsupported` declaration"标注，与 `capabilities()` 的 unsupported 声明一致，不再误示当前已支持）


## 审核发现 2026-11-05（bba0dd0a..HEAD，只审不改）

- [x] 审核发现：process 模板 `applicable_faults = ["process_exited", "probe_timeout"]` 含 `probe_timeout`，但引擎 `diagnose()` 对 process 探针恒诊断 `process_exited`、从不产生 `probe_timeout`（engine.rs:890），与本模板自身注释「a failing `process` probe diagnoses `process_exited`」（init.rs:192-195）矛盾（多余故障条目，无运行期危害，但注释与列表不符）。（crates/custos-cli/src/init.rs:205） → 属实（`crates/custos-cli/src/init.rs:205`：`applicable_faults` 删 `probe_timeout`，仅留 `process_exited`，与 `diagnose()` 恒诊断 `process_exited` 一致；`process_template_lists_process_exited` 测试增补 `!contains("probe_timeout")` 断言；`cargo test -p custos-cli` 10 全绿）
- [x] 审核发现：custom-command 模板 `applicable_faults = ["remote_endpoint_unreachable", "probe_timeout", "unknown_failure"]` 含 `unknown_failure`，但引擎 `diagnose()` 对 exec 探针仅产生 `remote_endpoint_unreachable`（Unhealthy）或 `probe_timeout`（Unreachable）、从不产生 `unknown_failure`（engine.rs:891-894），与本模板自身注释枚举的两个故障（init.rs:221-222）不符（多余故障条目，无运行期危害，但注释与列表不符）。（crates/custos-cli/src/init.rs:237） → 属实（`crates/custos-cli/src/init.rs:234-236`：`applicable_faults` 删 `unknown_failure`，仅留 `remote_endpoint_unreachable`/`probe_timeout`，与 `diagnose()` 对 exec 探针的映射一致；`exec_template_lists_the_exec_fault_mapping` 测试更新断言并增补 `!contains("unknown_failure")`；`cargo test -p custos-cli` 10 全绿）
- [x] 审核发现：新增的 WifiReconnectTool 未在任何生产路径注册到 ToolRegistry，默认注册表只有 exec 和 process 工具，因而 wifi.reconnect 即使声明为 native 也无法通过实际调度执行（crates/custos-toolbox/src/platform_tools.rs:150）。 → 属实（`cee3d05`：`ToolRegistry::with_builtin_tools` 注册 RebootTool/InterfaceResetTool/WifiReconnectTool，控制器经 `custos_platform::default_network_controller`/`default_reboot_controller` 工厂接线（Windows=Netsh/WindowsReboot，Unix=Sysnet/LinuxReboot，macOS=Unsupported stub）；新增 `default_registry_wires_the_wifi_reconnect_tool` 断言 wifi.reconnect/network.interface.reset/system.reboot 均 knows 且 wifi.reconnect capability 带 host.network 门；`cargo test --workspace` 全绿）
- [x] 审核发现：has_wifi_adapter 只检查 netsh 命令退出状态而不检查接口列表，命令成功报告没有无线接口时仍会将 wifi.reconnect 声明为 native（crates/custos-platform/src/windows.rs:345）。 → 属实（`6f76e62`：新增 `netsh_wlan_reports_adapter` 纯解析器，识别本地化「no wireless interface / 没有无线接口」否证与 `Name : <value>` 接口块证成；`has_wifi_adapter` 改为在 `Succeeded` 时解析接口列表而非只看退出码；新增 `wifi_adapter_probe_requires_an_interface_list_not_just_exit_status` 覆盖空输出/en/zh-CN 无接口通知/含适配器输出/空 Name 值；`cargo test --workspace` 全绿）
- [x] 审核发现：持久路由的截断仅识别英文 Persistent Routes 标题，中文等本地化 Windows 输出仍会把持久默认路由当作活动网关并误报健康（crates/custos-probe/src/netif.rs:311）。 → 属实（`a096814`：新增 `is_persistent_routes_header` 纯判定，识别带尾冒号的英文「Persistent Routes」及 zh-CN「持久路由」/ja「永続ルート」/ko「영구 경로」/de「Persistente Routen」/fr「Itinéraires persistants」/es「Rutas persistentes」/ru「Постоянные маршруты」本地化标题，`parse_route_print_default_gateway` 改用它截断；新增 `route_print_parser_stops_at_localized_persistent_headers` 遍历八种标题断言停止且活动网关仍正确报告；`cargo test --workspace` 全绿 + clippy 无警告）
- [x] 审核发现：OEM 代码页映射缺少常见的 CP437 和 CP850，英文及西欧 Windows 上的非 UTF-8 输出仍回退到 GBK 而被错误解码（crates/custos-probe/src/exec.rs:217）。 → 属实（`crates/custos-probe/src/exec.rs:190,212,231`：`oem_code_page()` 改返回 u16 码页号；新增 `decode_with_oem_code_page` 先经 `known_codepages::CodePage::from_number` 表驱动解码 CP437/CP850（`0x82,0xE1`→`éß` 而非 GBK 的 `傖`），其余页走 `encoding_for_oem_code_page` 的 `encoding_rs` 映射，未知页回退 GBK；`known-codepages = { version="0.0.4", default-features=false, features=["cp850","std"] }`；新增 `oem_cp437_and_cp850_are_not_misdecoded_as_gbk` 测试（`crates/custos-probe/src/exec.rs:447`）；`cargo test -p custos-probe` 178/178 绿 + `cargo test --workspace` 全绿 + `cargo check --workspace --target x86_64-pc-windows-gnu` 通过）


## 审核发现（a6de07cc..9b5e3f8 只审不改）

区间提交（`git log --oneline a6de07cc..9b5e3f8`，6 笔）：`cee3d05` 注册 WifiReconnectTool 到默认 registry、`8bae656` docs 关闭 registry finding、`6f76e62` wifi 适配器探测改接口列表、`44f1246` docs 关闭 wifi probe finding、`a096814` route 扫描在本地化 persistent 标题处停止、`9b5e3f8` docs 关闭 persistent routes finding。检查方式：逐文件人工审核 + `git diff a6de07cc..9b5e3f8` + `cargo test -p custos-probe --lib netif`（9 绿）/`cargo test -p custos-toolbox --lib`（23 绿）/`cargo test -p custos-platform`（12 绿，含新增 `wifi_adapter_probe_requires_an_interface_list_not_just_exit_status`、`route_print_parser_stops_at_localized_persistent_headers`、`default_registry_wires_the_wifi_reconnect_tool`）。

- [x] 审核发现：`default_reboot_controller()` 的 `#[cfg(not(windows))]` 分支引用 `linux::LinuxRebootController::detect()`，但 `pub mod linux` 是 `#[cfg(not(target_os = "macos"))]`——macOS 上 `not(windows)` 为真而 `linux` 模块被 cfg 排除，`linux::LinuxRebootController` 未定义 → macOS 编译失败（crates/custos-platform/src/lib.rs:45）。 → 属实（`cee3d05` 新增 `default_reboot_controller`：`#[cfg(not(windows))]` 分支取 `linux::LinuxRebootController::detect()`（lib.rs:43-45），`pub mod linux` 却仅 `#[cfg(not(target_os = "macos"))]` 导出（lib.rs:21-22）；对比同文件 `default_network_controller` 正确用 `#[cfg(all(not(windows), not(target_os="macos")))]` + `#[cfg(target_os="macos")]` 处理 macOS（lib.rs:60-67）；`.github/workflows/nightly.yml:78-79` 有 macos-14/aarch64-apple-darwin 构建，此错误会破坏 macOS CI；本机（Linux）`cargo check --target aarch64-apple-darwin` 因 `ring` 的 cc 在本机（Linux）无 macOS SDK 可用（macOS 打包由 GitHub macOS runner 承担；三台真机 openclaw/imac/.5 均为 Windows）未到达 custos-platform 而未直接实证，但 cfg 逻辑推理成立） → 已修复（crates/custos-platform/src/lib.rs:38-61：macOS 单独 MacosRebootController stub，Linux 分支改为 all(not(windows), not(target_os="macos"))，与 network 工厂同形；`rustc --edition 2021 --target aarch64-apple-darwin --crate-type lib --emit=metadata` 对同 cfg 复现 0；`cargo test --workspace` 531 passed / 0 failed）
- [x] 审核发现：`is_persistent_routes_header` 的白名单用「持久路由」，但真实 zh-CN `route print` 输出区段标题为「永久路由」——zh-CN 下不匹配，扫描仍越过 persistent 区段，过期默认网关仍误报 Healthy，`a096814` 的修复在 zh-CN 上未生效（crates/custos-probe/src/netif.rs:359）。 → 属实（netif.rs:359 白名单含「持久路由」；多个真实 zh-CN 解析实现均用「永久路由」——MushGrowth/dual-nic-route-tool `dual_nic_route_tool.py:331`：`if in_active and ("永久路由" in s or "Persistent Routes" in s)`；cnblogs suv789 同用「永久路由」；新增测试 `route_print_parser_stops_at_localized_persistent_headers` 仅遍历代码自身声明的字符串，属自证循环，未用真实 zh-CN 输出验证） → 已修复（crates/custos-probe/src/netif.rs:344-373 新增真实 zh-CN 标题「永久路由」并保留旧「持久路由」；测试 route_print_parser_stops_at_localized_persistent_headers 使用「永久路由:」真实形态且活动网关仍解析；`cargo test -p custos-probe --lib` 179/179 绿）
- [x] 审核发现：`netsh_wlan_reports_adapter` 仅识别英文「no wireless interface」与 zh-CN「没有无线接口」两个否定标记，及 `Name`/`名称` 两个肯定标签——非 en/zh-CN 系统（如 ja/fr/de/ko）有适配器时字段标签非 `Name`/`名称`（ja「名前」、fr「Nom」、ko「이름」）→ 误判无适配器（假阴性），wifi.reconnect 静默不触发；无适配器时否定提示不匹配但默认返回 false 仍保守正确（crates/custos-platform/src/windows.rs:343-362）。 → 属实（windows.rs:347 仅匹配 "no wireless interface"/"没有无线接口"，windows.rs:354 仅匹配 `Name`/`名称`；此缺口不影响安全性（不会误报 Healthy），但使 wifi 探测在那些本地化系统上不可用） → 已修复（crates/custos-platform/src/windows.rs:349-391 新增 ja/fr/de/ko 等无适配器标记与名前/Nom/이름/Имя/Nome 等本地化 Name 标签；测试覆盖 4 个无适配器标记与 9 个本地化 Name 标签；`cargo test -p custos-platform` 13/13 绿）
- [x] 审核发现：`custos-platform/src/windows.rs::parse_default_gateway` 未同步 `a096814` 的 persistent 区段截断逻辑，仍全量扫描可能返回 persistent 区段过期默认网关，与 netif.rs 版行为分叉；netif.rs:320 注释「Mirrors the platform-side parse_default_gateway」已不准（crates/custos-platform/src/windows.rs:277-299）。 → 属实（windows.rs:277-299 无 `is_persistent_routes_header` 截断，netif.rs:311-320 有；该函数当前仅被 `crates/custos-platform/tests/windows_parsers.rs` 使用，生产路径未引用，故无运行期影响，但注释误导） → 已修复（crates/custos-platform/src/windows.rs:277-293 复用 custos_probe::netif::is_persistent_routes_header 截断 persistent 区段；新增 route_print_default_gateway_ignores_persistent_only_default_gateway；`cargo test -p custos-platform` 13/13 绿）

## 审核发现（a6de07cc..d17e699 只审不改）

> 实际范围 7 笔提交（用户所述「共 9 笔」与事实不符，以 `git rev-list --count a6de07cc..d17e699`=7 为准）：
> `cee3d05` 注册 WifiReconnectTool/InterfaceResetTool/RebootTool 于默认注册表；`8bae656` docs；`6f76e62` has_wifi_adapter 改查接口列表；`44f1246` docs；`a096814` 持久路由按本地化标题截断；`9b5e3f8` docs；`d17e699` OEM CP437/CP850 解码。
> 检查方式：全量 `git diff a6de07cc..d17e699` 逐文件人工复核 + 源码 cfg 分析 + 定向测试复跑（`cargo test -p custos-probe oem_cp437_and_cp850` 1 绿、`cargo test -p custos-platform --test windows_parsers` 12 绿、`cargo test -p custos-toolbox registry` 均绿）+ `rustc --cfg target_os="macos"` cfg 复现。

- [x] 审核发现：`default_reboot_controller()` 的 `#[cfg(not(windows))]` 分支引用 `linux::LinuxRebootController::detect()`（crates/custos-platform/src/lib.rs:45），但 `pub mod linux` 以 `#[cfg(not(target_os = "macos"))]` 门控（lib.rs:21-22）在 macOS 上不编译 → macOS 目标编译报 E0433（unresolved module `linux`）。macOS 是仓库发布目标（.github/workflows/nightly.yml matrix：`macos-14`/`aarch64-apple-darwin`，产物 `custos-macos-aarch64`），本区间（cee3d05）将破坏 macOS 发布构建。对照 `default_network_controller` 已对 macOS 单独 stub（lib.rs:64-67），本函数漏了该分支。（正确性/编译断，高）。证据：`rustc --edition 2021 --cfg target_os="macos"` 对相同 cfg 形态复现 E0433；lib.rs:21-22,43-45；nightly.yml matrix 含 macos-14；ci.yml 仅 Linux+Windows 故未被 CI 类型检查捕获。 → 已修复（crates/custos-platform/src/lib.rs:38-61：macOS 分支返回 MacosRebootController Unsupported stub，Linux cfg 收窄为 all(not(windows), not(target_os="macos"))；`rustc --target aarch64-apple-darwin --emit=metadata` 验证 0；`cargo test --workspace` 531 passed / 0 failed）
- [x] 审核发现：`is_persistent_routes_header` 仅覆盖英文 + 7 种本地化标题（crates/custos-probe/src/netif.rs:352-366），其他本地化 Windows（如 it「Route persistenti」/pt「Rotas persistentes」/nl「Permanente routes」/pl「Trasy trwałe」/tr「Kalıcı rotalar」）仍不截断，持久默认路由会被当作活动网关报告为 Healthy——与本提交声称修复的缺陷同类，方向是高估健康（危险），仅收窄未根除。（正确性残留，低-中）。证据：netif.rs:352-366 枚举仅 8 项；`a096814` 仅新增该表。 → 已修复（crates/custos-probe/src/netif.rs:352-373 追加 it/pt/nl/pl/tr 标题，并修正 zh-CN 为真实「永久路由」；测试遍历 13 个标题；`cargo test -p custos-probe --lib` 179/179 绿）
- [x] 审核发现：`netsh_wlan_reports_adapter` 仅识别 en/zh-CN 无接口标记与 `Name`/`名称` 标签（crates/custos-platform/src/windows.rs:347,354），ja（`名前`）/fr（`Nom`）等其他本地化 netsh 输出中，有适配器的系统会因标签不匹配返回 false → `has_wifi_adapter` false → `wifi.reconnect` 不声明 native（功能降级；方向保守——低估而非误执行，故无安全危害）。（功能覆盖缺口，低）。证据：windows.rs:347,354 匹配集合有限；测试仅覆盖 en/zh-CN 与空 Name。 → 已修复（crates/custos-platform/src/windows.rs:349-391：is_no_wlan_marker 扩展 ja/fr/de/ko/ru 等，is_wlan_name_label 覆盖 11 种标签；测试含 ja/fr/de/ko 否证和 9 种本地化证成；`cargo test -p custos-platform` 13/13 绿）
- [x] 审核发现：`decode_with_oem_code_page` 仅由 known-codepages 处理 CP437/CP850，其余 DOS 单字节页（CP775/852/855/857/858/860/861/862/863/864/865/869 等）仍落入 `_ => GBK`（crates/custos-probe/src/exec.rs:248），这些系统上非 UTF-8 控制台输出仍被错误解码为 GBK（CP866/874 已由 encoding_rs IBM866/WINDOWS_874 覆盖）。known-codepages 已内置对应 feature，属「修复未做全」的残留缺口（非回归）。（正确性残留，低）。证据：exec.rs:212-248 分支表；Cargo.toml 仅启用 cp850 feature；`encoding_for_oem_code_page` 无 775/852/855/857/858/860-865/869 分支。 → 已修复（crates/custos-probe/Cargo.toml known-codepages features=["all","std"]，CP775/852/855/857/858/860-865/869 全部走表解码；新增 remaining_dos_code_pages_are_not_misdecoded_as_gbk 定向断言；`cargo test -p custos-probe --lib` 179/179 绿）
- [x] 审核发现：本轮 diff 未见问题（d17e6999..HEAD）。 → 已核实（`git diff --name-only d17e699..HEAD` 仅 crates/custos-cli/src/init.rs + todo.md；代码 diff 仅为 rustfmt 数组单行化，`cargo test --workspace` 531 passed / 0 failed）
- [x] 审核发现：本轮 diff 未见问题（3c2c0d57..HEAD）。 → 已核实（`git diff 3c2c0d57..HEAD` 覆盖平台解析/探测修复与 todo，未见新增问题；`cargo test --workspace` 531 passed / 0 failed）
- [x] 审核发现：本轮 diff（ae19ee8..HEAD）未见问题。 → 已核实（`git diff --name-only ae19ee8..HEAD` 仅 todo.md；`git show b9482d2` 为纯文档改动（关闭 Windows Wi-Fi 场景 + 追加审核行），无代码变更；环境声明属实（uname 为 Linux x86_64、/sys/class/net 无 wlan* 接口、command -v netsh 无结果）；`cargo test --workspace` 531 passed / 0 failed）
- [x] 审核发现：本轮 diff 未见问题（b9482d2e..HEAD）。 → 已核实（`git diff --stat b9482d2e..HEAD` 仅 todo.md 1 行新增；逐行复核 `7071e1a` 仅上一条审核结论文档记录，无代码改动；当日 HEAD=`7071e1a`）
- [x] 审核发现：本轮 diff 未见问题（7071e1af..HEAD）。 → 已核实（`git diff --name-only 7071e1af..HEAD` 仅 todo.md；`git show --stat 7dc8c0b` 仅 todo.md 1 行新增，内容为上一条审核结论文档记录，无代码改动；HEAD=7dc8c0b，父提交=7071e1a；`cargo test --workspace` exit 0，531 passed / 0 failed）
- [x] 审核发现：本轮 diff 未见问题（7dc8c0bd..HEAD）。 → 已核实（`git diff --name-only 7dc8c0bd..HEAD` 仅 todo.md；`git show --stat 7c8c0b2` 仅 todo.md 1 行新增，内容为上一条审核结论文档记录，无代码改动；HEAD=7c8c0b2，父提交=7dc8c0b；`cargo test --workspace` exit 0，全部测试目标 0 failed）

## 整机 MSI（用户反复提出未落账，2026-11-08 补记并排期）

- [x] CUSTOS-MSI-001 [P1] custos 整机安装包：现 Nightly 产物名 `custos-cli-*.msi`，实际虽含 custos+custos-tray 两 exe 但命名与定位是「CLI 的 MSI」——用户要的是 custos 整机安装包。改法：wxs Product 更名 Custos（开始菜单两组快捷方式：Custos CLI / Custos Tray），产物更名 `custos-<ver>-x64.msi`（版本仍从 Cargo.toml 动态读，禁手写），tray 可选开机自启（注册表 Run 键，安装参数开关），升级路径同 ProductCode 覆盖安装，卸载干净（含自启键清理）；nightly.yml 上传更名后产物；Windows 真机（imac）msiexec 实装/卸载双向验证 + 截图证据。 → 已完成（2026-11-08）。修复：`048b53f`（MajorUpgrade 组合）+ `1999489`（DowngradeErrorMessage，CNDL0010）+ `1d6ce42`（快捷方式独立组件，ICE43/ICE57）；产物 `custos-0.1.0-x64.msi` 由 nightly 42397054170 产出；imac（Win11 专业版 10.0.26300）实装验证：基线旧 nightly→新 MSI 同 UpgradeCode 覆盖升级（product code ED899C34→CF1414FC），两快捷方式+CustosIcon+HKCU Run 自启（AUTOTRAY=1 默认，=0 关闭）全部到位，卸载后零残留（文件/快捷方式/Run 键/PATH/产品记录全清）；证据归档 `docs/evidence/msi/`（README+4 份 msiexec /l*v 日志+2 张截图+2 个 .lnk）。

## 默认示例补向日葵监控场景（用户反复提出未落账，2026-11-08 补记并排期）

- [x] CUSTOS-EXAM-001 [P1] 默认示例集补向日葵监控：新增 `examples/sunlogin.toml`（type="service"，Windows unit="SunloginService" 实测于 imac 存在，Linux 对应 systemd unit 注释说明；依赖链 sunlogin→network 照 roadmap 场景1 口径）；`examples/` 现仅 3 个（basic-process/dependency-chain/notifications），README 未列 examples 入口——补 README 示例节 + nightly.yml 打包照旧 cp examples/*.toml；`custos validate examples/sunlogin.toml` 必过 + `cargo test --workspace` 全绿。 → 已完成（2026-11-09，并行提交 `1ce8e58`）：`examples/sunlogin.toml` 已建（type="service"、unit="SunloginService"、Linux `oray-sunlogin.service` 注释、依赖链 sunlogin→network）；README.md:49-58 + README.zh.md 已补 Examples 节；nightly.yml:110 照旧 `cp examples/*.toml`；`custos validate examples/sunlogin.toml` = OK（2 resources, acyclic）；SunloginService 于 imac 实测存在（Status=Stopped, StartType=Manual，2026-11-09 复核）；无 Rust 改动（仅 docs/toml），`cargo test --workspace` 537 passed / 0 failed。

## 源码分层分析 2026-11-07

### 抽象层清单

- 配置与领域契约层：启动前把 TOML 变成严格校验的资源、依赖、图、权限与集成配置。—— `crates/custos-config/src/lib.rs:347`
- 协调与恢复状态层：按依赖图调度观测，驱动诊断、恢复状态机、预算、等待唤醒和最终健康判定。—— `crates/custos-engine/src/engine.rs:196`
- 能力发现层：归一化原生探测，并以 JSON-RPC/能力声明接入外部插件，统一登记可调用操作。—— `crates/custos-core/src/assemble.rs:42`
- 执行与平台层：在权限和能力检查后，用可替换的执行器与 OS 适配器执行本机动作。—— `crates/custos-engine/src/executor.rs:10`
- 持久化与事件层：用 SQLite 保存运行、证据、诊断、审计、健康、步骤、插件注册与通知队列。—— `crates/custos-store/src/store.rs:810`
- 通知投递层：规则化筛选、去重入队并由后台 worker 重试外发，故障不阻塞恢复。—— `crates/custos-notify/src/engine.rs:225`
- 运行时装配与进程生命周期层：唯一组装完整运行时，恢复崩溃现场并监督 CLI、插件与后台 worker。—— `crates/custos-core/src/assemble.rs:58`
- 接口与观察者层：提供 CLI、只读 dashboard、托盘和 Python 嵌入门面，但不绕过核心协调。—— `crates/custos-cli/src/main.rs:44`

### 超大文件候选

- [x] LAY-001 [P1] 拆分 crates/custos-engine/src/engine.rs（2148 行）——观测、诊断、依赖等待、恢复计划、执行验证、预算升级和崩溃续跑都在一个类型路径中。——（crates/custos-engine/src/engine.rs:49）✅ commit 70e1c72，engine.rs→engine/{mod,orchestrate,observation,recover,diagnose,dependency}.rs，测试 537 通过/0 失败
- [x] LAY-002 [P1] 拆分 crates/custos-store/src/store.rs（1566 行）——连接生命周期、读取 API、运行仓储、通知队列、保留清理与多套集成测试耦合在一个实现文件。——（crates/custos-store/src/store.rs:34）✅ commit 8d0dd6c，store.rs→store/{mod,lifecycle,read,retention,notify,runs,rows,tests}.rs，测试 537 通过/0 失败
- [x] LAY-003 [P1] 拆分 crates/custos-config/src/lib.rs（1376 行）——主 schema、探测字段联合体、依赖/计划校验和运行结构构造都写在一个文件。——（crates/custos-config/src/lib.rs:56）✅ commit e809fb9，lib.rs→lib.rs+{schema,build,tests}.rs，测试 537 通过/0 失败
- [x] LAY-004 [P1] 拆分 crates/custos-probe/src/http.rs（1177 行）——HTTP 状态、内容、时延、重定向、TLS 与多协议出站探测混在一个适配器。——（crates/custos-probe/src/http.rs:26）✅ commit e2e911b，http.rs→http/{mod,json,net,tests}.rs，测试 537 通过/0 失败
- [x] LAY-005 [P1] 拆分 crates/custos-config/src/notification.rs（1047 行）——五种通道配置、原始表解包、逐通道校验与测试集中在同一模块。——（crates/custos-config/src/notification.rs:55）✅ commit ef1226b，notification.rs→notification/{mod,raw,tests}.rs，测试 537 通过/0 失败

### 目录/分类问题

- `crates/custos-probe/src/http.rs:26` 将通用 HTTP 探测、TLS 证书路径检查与代理链路探测放在一个 1177 行文件；文件名 `http.rs` 不能表达这三类观测职责。
- `crates/custos-config/src/lib.rs:56` 的 `RawProbe` 是所有探测类型的字段联合体，靠可选项表达每类约束；新增探测会继续扩大主配置文件而非各自形成类型化子模块。
- `crates/custos-config/src/notification.rs:55` 的 `ChannelConfig` 与每个通道的配置、校验逻辑同文件；通道数量增加时配置契约和通道校验没有天然边界。

## 代码深审 2026-11-01（已修 16，余 29 记档）

报告：`docs/review/2026-11-01-code-review.md`。已修项见该报告的 fix commits 一节，以下为本轮记录未修的 P2/P3。

### P2

- [x] R9 [P2] `netif.rs:143` 路由解析遇到首个缺少双空格分隔符的行即 `?` 中断，本地化或畸形行使整表报 NotFound 而非跳过该行。✅ commit 904434a，`parse_netsh_interface_state` 改为跳过无分隔符行（对齐平台侧 parse_netsh_interfaces），红先测试 `netsh_interface_state_parser_skips_separatorless_rows` 验证旧行为整表 None；另 commit b73bb3c 补提 ef1226b 漏掉的旧 notification.rs 删除；测试 538 通过/0 失败
- [x] R10 [P2] `docker.rs:121` `restarts.parse::<u64>().unwrap_or(0)` 把解析失败当成 0，掩盖崩溃循环。✅ commit 88a1220，不可解析的 RestartCount 改为 Unreachable（与 status/running 一致），红先测试 `unparsable_restart_count_is_unreachable_not_zero` 验证旧行为返回 Healthy；测试 539 通过/0 失败
- [x] R11 [P2] `plugins/cloudflared/src/main.rs:54,59-69` `cloudflared_tunnel_ha_connections` 解析失败时 `connections = None`，匹配分支判 HEALTHY——想探测的故障在解析失败后反被当成健康。✅ commit 6c8edf3，gauge 行存在但值不可解析 → 判 Unhealthy（新增 `unparsable` 追踪，区分「无 gauge」兜底），补齐缺失的单测 `unparsable_gauge_is_not_healthy`/`gauge_counts_are_interpreted`，红先验证旧行为返回 Healthy；测试 541 通过/0 失败
- [x] R12 [P2] `windows.rs:523-524` `parse_netsh_interfaces` 只识别英文/zh-CN 词元，ja/fr/de 等系统语言下所有接口判 down。✅ commit cc04908，按列 cell（≥2 空格分列、保留单元格内单空格）逐列匹配 12 语言 enabled/disabled/connected/disconnected 词元，无法识别行跳过不猜（§11.1），消除整行 substring 跨列碰撞；custos-probe 探测侧 `parse_netsh_interface_state` 同步修；红先测试 `netsh_interface_list_recognizes_localized_states`/`netsh_interface_state_recognizes_localized_states` 验证旧行为全判 down；测试 543 通过/0 失败
- [x] R13 [P2] `windows.rs:435-444` `netsh_interface_reset_argv` 把 `admin=disabled` 与 `admin=enabled` 塞进同一条 `netsh interface set interface`，单次调用只接受一个 `admin=`。✅ commit c9cc5f7，改为 `netsh_interface_set_admin_argv(name, enabled)` 每次一个 `admin=`，`reset` 顺序执行 down→up 两次调用；红先测试改为断言两个单 admin argv（编译红）验证旧形状不成立；测试 543 通过/0 失败
- [x] R14 [P2] `openwrt.rs:68-73` 非 JSON 的 `ubus call service list` 经 `unwrap_or(false)` 映射为 "Stopped"，应报 Unknown/错误。✅ commit a7d42be，提取纯函数 `service_list_has_instances`，非 JSON 回复 → `PlatformError::Failed`（对齐 `ubus_call`），不再猜 Stopped；红先测试 `non_json_service_list_reply_is_an_error_not_stopped`（编译红）验证旧 `unwrap_or(false)` 行为；测试 545 通过/0 失败
- [x] R15 [P2] `plugins/mihomo/src/main.rs:129-135` `plain.first()` 取首个非当前成员而非当前之后（含回绕）的下一个，反复恢复不推进轮转。✅ commit 3552065，提取纯函数 `next_plain_member`：从当前节点位置之后（含回绕）取下一个普通成员，跳过选择器与当前自身，反复恢复 A→B→C→A 推进而非 A↔B 弹跳；红先测试 `next_plain_member_walks_rotation`/`next_plain_member_skips_selectors_and_self`（编译红）；测试 547 通过/0 失败
- [x] R16 [P2] `linux.rs:331` / `windows.rs:542` `probe_tcp` 用 `format!("{}:{}", host, port).parse()`，裸 IPv6 字面量永远不是合法 `SocketAddr` 串，IPv6 目标结构性不可达。✅ commit 0719cab，新增 `controller::resolve_target`（元组形式 `(host,port).to_socket_addrs()`，与 `dns_resolves` 一致），linux/windows 两处 `probe_tcp` 改用它；红先验证：`format!("{}:{}","2001:db8::1",443).parse::<SocketAddr>()` 报 Err，元组形式得 `[2001:db8::1]:443`；测试 `resolve_target_handles_bare_ipv6_literal` 绿；测试 548 通过/0 失败
- [x] S2 [P2] `herald.rs:44` `endpoint()` 把 `config.base_url` 直接拼进 POST URL 且无 SSRF 校验（webhook 侧有 `webhook_ssrf.rs`）。✅ commit 4ea5b35，`HeraldConfig` 增加 `ssrf: SsrfPolicy`（raw 侧 `#[serde(default)]` 缺省 `default_blocking`，与 webhook 同形），`HeraldChannel::new` 构造期经 `webhook_ssrf::validate_target` 快速失败、`send_once` 每次发送前重检（新 DNS 解析），`block_redirects` 时 `max_redirects(0)`，违例为 `Config` 终态错误不重试；红先测试 `construction_rejects_ssrf_blocked_target`（运行红：旧 `new()` 放行 loopback）+ `allowlisted_herald_host_bypasses_ssrf`；examples/notifications.toml 补 herald ssrf 段；测试 550 通过/0 失败
- [x] C2 [P2] `store.rs:257,1060` 损坏的 `outcome` 串解析为 `None`，被误判为「执行中途崩溃」，原始 outcome 文本丢失。✅ commit 9e5947d，新增 `parse_outcome`（mod.rs）：NULL=未记录（§7.3 崩溃语义）与「存在但不可解析=损坏行」分离——`run()`/`attempts_of` 把损坏 outcome 以 `StoreError::Corrupt` 上报（与损坏 run payload 同待遇）并记 error 日志含原始文本；`unfinished_runs`（fire-and-forget 契约）记 error 日志含 run/action/原始文本后保守取 None（下游 OutcomeUnknown，不再无声）；`save_attempt` 序列化失败记日志并存 NULL 而非伪造空串行（不可注入失败，无红测试，防御性）；红先测试 `corrupt_outcome_text_is_an_error_not_silent_none`（运行红：旧读取把 `'not-json{"'` 静默变 None）；测试 551 通过/0 失败
- [x] C4 [P2] `herald.rs:94-99` 2xx 但 body 不可读 → `code=None` → 判 `Http`/可重试，产生多余重试（方向安全，优先级低）。✅ commit dfafce6，`classify_response` 区分三态：2xx+body 读失败 → `Transport`（可重试，传输层问题）；2xx+可读但非 `{"code":0}` 信封 → `Rejected` 终态（契约失配，重试同类请求无意义，不再产生多余重试）；非 2xx 维持状态码判定不变；红先测试 `two_hundred_without_envelope_is_rejected_not_retried`（运行红：200+HTML 信封被旧逻辑判 Http，重试 3 次打满后第 4 次连接拒绝转 Transport）；测试 552 通过/0 失败
- [x] C5 [P2] `engine.rs:289,1089` `let _ = self.try_recover(...)` 丢弃依赖/备用组恢复错误，等待中的 run 停在 Waiting 且无错误上报。✅ commit cc2b6ff，三处拒绝不再无声：`single_dependency_satisfied` 捕获 `try_recover` 错误 → `tracing::warn` + 升级 step detail 变为「dependency `x` unsatisfied (recovery refused: <原因>)」（区分「恢复跑过但失败」与「从未尝试」）；`alternative_group_satisfied` 逐成员记录拒绝并汇总进组不满足 step detail（`; recovery refused for m (原因)`）；`wake_ready_waiters` 依赖重试拒绝记 warn（等待者按预算继续等待/升级）；红先测试 `dependency_recovery_refusal_is_surfaced_in_escalation`/`group_member_recovery_refusal_is_surfaced_in_escalation`（运行红：旧 detail 均无拒绝原因）；测试 554 通过/0 失败
- [x] C7 [P2] `store.rs:437-446` + `webhook.rs:134-158` 过期 `sending` 回收（60s）与合法长投递（retries=6、退避 30000 翻倍超过 60s）竞态，导致并发重复投递；需租约/回收设计。✅ commit efe39b2，claim 显式租约：V4 migration 增 `lease_until_unix_ms` 列，`claim_due_notifications(now, limit, lease_ms)` 认领时把行置 `sending` 并写租约 `now+lease_ms`；租约大小由引擎按最长通道投递上界计算（remote 通道 = `backoff*(2^retries−1)+(retries+1)*timeout`，SMTP = connect+send 超时，LocalLog=0），clamp [60s, 1h]——活投递必在租约内，绝不被并发回收；回收只取租约已过期的 `sending` 行，NULL 租约（迁移前旧行）沿用旧 60s `updated_at` 回退；红先测试 `live_lease_blocks_reclaim_but_lapsed_lease_reclaims`（运行红：Stage A 前 70s 回收仍取走 120s 租约内的活行）+ 引擎侧单测 `claim_lease_outlives_channel_retry_ladder`（6 retries/30s 退避/10s 超时 → 1_960_000ms、local-log-only → 60s 下限、31 retries 巨型退避 → 1h 上限）；测试 556 通过/0 失败
- [x] C8 [P2] `supervisor.rs:96,185` 插件状态锁跨进程 spawn + 两次 RPC 往返持有（约 20s 阻塞 I/O），卡住引擎 tick 与 `status()`。✅ commit 2307567，新增 `spawn_gate`（respawn 序列化门，锁序恒 gate→state）：`start()`/`healthy_session()` 快路径只在短临界区持 state 锁（活性检查 + `enter_gate`），进程 spawn + `initialize`/`capabilities` 两次 RPC 全程无锁进行，结果经短临界区落账；崩溃会话回收的 `dying_words`（≤100ms stderr 轮询）同样移出锁外；并发调用者在门上排队后复检并复用他人 spawn（不重复拉起）；`shutdown` 先取门保持在途 spawn 先完成的原语义。红先测试 `status_stays_responsive_while_start_spawns_a_slow_plugin`/`status_stays_responsive_while_a_call_respawns_the_plugin`（运行红：旧代码 `status()` 在 spawn 在途时阻塞 2.51s；mock-plugin 增 `MOCK_PLUGIN_SLOW_START_MS`/`MOCK_PLUGIN_READY_FILE` 慢启动开关，CHILD 写就绪文件后才进入 stall，测试据此确定 spawn 在途）；开发中自测发现并修复引入的 self-deadlock（retire 段守卫未释放即重锁 state，`degraded_plugin_marks_resource_unprobeable` futex 永久挂起）；测试 558 通过/0 失败
- [x] C13 [P2] `budget.rs:54,134-138` attempt 历史无界增长，从不裁剪或重置（与 C29 叠加）。✅ commit 78f8cc1，`restarts` 时间戳日志按滑动窗口驱逐：新增 `prune_restarts`（`retain` 窗口内条目），`record_attempt`（签名增 `&RecoveryBudget`）与 `check_restart_rate`（`&self`→`&mut self`，调用方 `budget_gate` 本就 `&mut self`）两处先驱逐后计数/入队——活跃抖动资源的日志被窗口 + 速率上限（每窗口 ≤max_restarts_per_window 条）双重封顶，不再随宿主运行无限膨胀；诊断入口 `restart_history_len` 露出保留条数；红先测试 `restart_history_is_pruned_to_the_window`（签名变更编译红；无驱逐时 51 条断言 1 条必败，绿后断言成立）；长期静默资源残留的最后几条历史不主动清扫（无增长，仅存量），C29 的 attempts 计数重置另案；测试 563 通过/0 失败
- [x] C15 [P2] `engine.rs:263-311` 等待者的依赖每 tick 重探，绕过 `ProbeDueTracker`，每次调用落一行 `probe_results`。 `99807cd`（`wake_ready_waiters` 不再无条件重探依赖：tick 顶部观察遍已经经 `ProbeDueTracker` 探过所有到期资源，唤醒遍改读该遍写入的快照，仅在真实重试后再探一次；新增红测试 `waiter_dependency_probes_respect_the_probe_interval`（dep interval 60s、app 停靠 dep，tick + 5×100ms 子 tick，断言 dep 证据行数不变——修复前 4→9 逐 tick +1，修复后保持基线）；既有 `dependents_wait_on_one_dependency_recovery_and_resume` 的 db 探针序列删去两个唤醒遍探针（10→8 布尔）；`cargo test --workspace` 564 通过 0 失败）
- [x] C16 [P2] `notify engine.rs:255-258` 瞬时失败一律 `mark_retry`，无尝试上限，行与计数无界增长（关联 C17）。 `41b2f9e`（`deliver_due` 增 `MAX_QUEUE_ATTEMPTS=8`（与退避梯次饱和点对齐，约 2h 后到终态）：瞬时失败在 `row.attempts >= 上限` 时改走 `mark_dead`，attempts 计数与重试排期有界；报告计数改为按记录的迁移类型（`OutcomeTransition`）统计，封顶死信不再误报为 retried；红测试 `transient_failures_dead_letter_at_the_attempt_cap`（封顶处断言 dead=1、封顶-1 处断言仍 retry——修复前 dead=0）；`cargo test --workspace` 565 通过 0 失败）
- [x] C17 [P2] `notify engine.rs:244-259` 瞬时失败无限重试，无最大尝试死信；长期宕机的通道永不升级给人。 `41b2f9e`（与 C16 同一机制：封顶后转 `dead_letter` 终态，`custos notify status` 计数可见（store `notification_counts` 按状态分组）；双语 notification.md §8.6 注明封顶语义）
- [x] C18 [P2] `notify engine.rs:137-221` 同步 fsync 的 SQLite 插入在引擎 tick 线程内联执行，突发会拖住唯一 tick。 `d834a59`（新增 `custos-notify/src/spool.rs`：`SpooledDispatch::spawn` 起专用 worker 线程跑 `dispatch`，emitter 经有界 `sync_channel`（容量 256）交接事件，tick 线程只付 handoff；spool 满时 `try_send` 失败转一次内联 dispatch（背压不丢事件，§8.1）；`assemble.rs::attach_notifications` 改走 spool，CLI 与 Python 两个入口同路径；测试 `spool_hands_events_to_the_worker_and_drains` + `saturated_spool_falls_back_without_losing_events`（rendezvous 通道混用两种路径仍恰好 20 条全入队）；shape 变更为 compile-red（新类型），行为由新测试覆盖；`cargo test --workspace` 567 通过 0 失败）
- [x] C19 [P2] `notify engine.rs:225-281` 单通道重试睡眠链阻塞整条串行投递队列（webhook 通道内 `thread::sleep` 翻倍）。 `3e05de9`（webhook/herald/telegram 三通道删 `deliver_with_retry` 睡眠梯次，`deliver` 一次投递=一次请求，重试/退避由队列引擎端到端负责（§8.6 口径与 SMTP 一致）；C7 租金预算相应收缩为各通道自身 timeout（`remote_retry_budget` 删除，`channel_delivery_budget_ms` 每通道预算=发送超时）；红测试重构自旧梯次测试：`transient_5xx_is_one_attempt_the_queue_retries`（对旧代码实测 RED：梯次在第 2 次请求成功，新测试断言 Err + 恰 1 请求）、`server_5xx_is_one_attempt_the_queue_retries`（herald 同口径）；`cargo test --workspace` 566 通过 0 失败）
- [x] C20 [P2] `engine.rs:571-574,745-747` 恢复动作在 tick 线程上跑，只有 plan `timeout_ms` 无看门狗，动作挂死会无限拖住 tick。 `308d489`（`execute_with_watchdog`：executor 以 `Arc<dyn ActionExecutor>` 共享到 worker 线程执行（`Engine::new` 签名保持 `Box` 参数、内部 `Arc::from`，6 处调用点零改动），tick 线程 `recv_timeout(plan.timeout_ms + 5s 宽限)`；超期即记 `OutcomeUnknown`（§6.6：结果不可知绝不盲目重试——verify + re-diagnose 收敛，run 以 Escalated 终态收束），迟到的结果仅 warn 日志后丢弃；顺带修了连带暴露的潜伏缺陷：`run.action_attempts` 原在执行前 push `outcome: None` 骨架且从不回填，内存态 attempt outcome 恒空——改为结果落定后再 push。红测试 `hung_action_cannot_stall_the_tick_past_the_watchdog`（对未修复代码实测 RED：闭包 executor 停在 gate 上，tick 永不返回，`timeout 100` 杀掉 exit 143；修复后 tick 5.1s 返回、attempt=OutcomeUnknown、下一 tick <1s 存活）+ executor.rs 两个看门狗单测；`cargo test --workspace` 569 通过 0 失败）
- [x] C21 [P2] `executor.rs:41` `StartFailed → Failed` 可重试，永久缺失的二进制会在整个 plan 内反复重试。 `e99934c`（新增 `ActionOutcome::NotStarted`（§6.6 语义：动作从未运行，与可重试的瞬态 `Failed` 分型；serde snake_case 兼容，旧库行照常反序列化）；`BuiltinExecExecutor` 的 `StartFailed → NotStarted`；recover.rs 尝试循环捕获 outcome，`NotStarted` 即断本 plan 梯次（Verifying→Diagnosing 合法态路径 + `start_failed` 步骤记录），交给 `failure_handling`——TryNextPlan 换下一 plan，Escalate/Stop 照常升级；`did not start` 也发 `recovery.action_failed` 通知（坏 plan 必须可见）。红测试 `missing_binary_does_not_burn_the_whole_attempt_ladder`（对未修复代码实测 RED：3 层梯次全烧完 left:3 right:1）+ `start_failure_hands_over_to_the_next_plan`（broken plan 1 次 NotStarted → fix plan 成功验证 Recovered；新 outcome 断言随变体属 compile-red 形状变更）；`cargo test --workspace` 571 通过 0 失败）
- [x] C22 [P2] `webhook.rs:134-158` 退避重复计算：通道内重试与队列重试对同一错误各自排期。 `3e05de9`（与 C19 同根：通道内梯次整体删除——对同一瞬时错误不再有「通道排一次 + 队列再排一次」的双重排期，重试唯一归属队列引擎，退避梯次只走一份（BASE_RETRY_MS 倍增至 MAX_RETRY_MS，8 次封顶转死信）；验证同 C19：新单次投递测试对 HEAD 旧梯次代码实测 RED，`delivery_lease_floors_and_caps_per_attempt` 租金测试按单次超时预算重写，566 通过 0 失败）
- [x] C23 [P2] `dashboard/src/lib.rs:92-105` 每次读有 2s 超时但无整请求期限，慢速滴流客户端（< 64 KiB）可长期占住单线程 accept 循环。 `a679d37`（`REQUEST_DEADLINE=10s` 整请求期限：读循环内每片段后查 `Instant::elapsed`，超期即 `ErrorKind::TimedOut` 返回——静默对端仍由既有 2s read timeout 经 `?` 自然退出，滴流型客户端由总期限截断，单线程 accept 循环不再被扣住。红测试 `slow_drip_client_cannot_hold_the_request_open_past_the_deadline`（对未修复代码实测 RED：300ms 滴 1 字节后台线程 + 20s 轮询 `is_finished`，handle 永不返回→断言「still blocked」失败；修复后 ~10s 以 TimedOut 收束）；`cargo test --workspace` 572 通过 0 失败）
- [x] C24 [P2] `session.rs:245-309` 写确认等待用 `default_timeout()` 而非调用方 `timeout`，静默截断较长请求。 `a0d9fb2`（写确认等待改用调用方预算：新增 `spawn_call_with_deadline(method, params, deadline)`（原 `spawn_call` 语义不变，变为默认期限的薄封装），`call_with_timeout` 改走 `spawn_call_with_deadline(timeout)?.wait(timeout)`——同一份 caller 预算覆盖写确认与响应两阶段，排在慢写（管道阻塞的串行 writer）后面的请求不再被默认期限悄悄截断。红测试 `write_confirm_uses_the_callers_deadline_not_the_default`（对未修复代码实测 RED 0.84s：mock-plugin `MOCK_PLUGIN_SLOW_START_MS=1500` 在读循环前停摆 + 400ms 默认期限 + 10s 调用方期限，200KiB 前导请求堵住 64KiB 管道，r2 写确认在 400ms 默认处报 "could not be written" 而非活到调用方期限；修复后同一场景返回 probe Ok）；`cargo test --workspace` 573 通过 0 失败）
- [x] C27 [P2] `supervisor.rs:128-144` `HostError::Unavailable` 被当作崩溃，但存活插件也会经声明式 RPC 错误返回该值，白耗崩溃预算。 `1d47d72`（比巡检单描述还深一层：`absorb_crash` 对任何被 take 的 session 一律记崩溃（`dead.is_some()`），存活 session 也算；修法两道——`call()` 在 `Err(Unavailable)` 时先查 `session.is_running()`，活进程的声明式 `PLUGIN_UNAVAILABLE`（-32003，`finish()` 映射自 RPC error）原样穿透不记预算；`absorb_crash` 内再以 `!is_running()` 收紧为 `really_dead` 才 push crash。mock-plugin fixture 新增 probe `unavailable: true` 开关（回 -32003 但进程存活）。红测试 `declarative_unavailable_does_not_burn_the_crash_budget`（对未修复代码实测 RED：声明式失败一次后 max_restarts=1 的 supervisor 即判 unrecoverable，下一次正常 probe `expect("a live plugin must keep serving")` panic；修复后同场景 probe Ok + `Running{restarts:0}`）；`cargo test --workspace` 574 通过 0 失败）
- [x] C29 [P2] `budget.rs` attempt 预算文档称「每 incident」，但成功恢复后从不重置，抖动后转稳的资源可能被误判耗尽。 `c4ae550`（同批修掉同型的孪生缺陷：`max_high_risk_actions` 文档称「per RecoveryRun」但 `high_risk_used` 也是终身累计，一次高危动作后所有后续 run 永久被拒。修法：`BudgetState` 新增 `reset_attempts(resource_id)`（verify 成功 → `Recovered` 分支 `close_incident_of` 旁调用——incident 关闭即预算归零，§6.5 "per incident" 口径；Escalated 不重置，失败循环仍受控）与 `reset_high_risk()`（`try_recover` 在 `active_runs.insert` 后、每 run 开始时归零）。红测试 `attempt_budget_resets_after_a_verified_recovery`（对未修复代码实测 RED：`second.len()` 0 vs 1——refault 在 `check_start` 直接被拒连 run 都没建）+ `high_risk_budget_resets_per_run`（RED：Escalated vs Recovered，budget_gate 拒绝高危动作；两测试初版 advance 2s 因 probe interval 10s 未到期而红错原因，改 advance 11s 过 cooldown+interval 后红因正确）；`cargo test --workspace` 576 通过 0 失败）
- [x] C30 [P2] `dashboard/src/lib.rs:70-83` `for stream in self.listener.incoming() { Err(_) => continue }` 无退避，持续 accept 错误（EMFILE）会空转死循环。 `533598c`（accept 循环抽成 `serve_streams(incoming, store, config)` 自由函数（`serve` 解构 self 喂入 `listener.incoming()`），连续错误指数退避 `accept_backoff(n)`=10ms 起倍增、1s 封顶、任一成功归零，warn 日志带 backoff_ms——持续 accept 失败（EMFILE）从满速空转变为有界怠速。红测试 `persistent_accept_errors_back_off_instead_of_spinning`（合成错误流 `[Err×3]` 驱动 serve_streams，线程止于 `unreachable!` panic 即 join；对旧无退避语义重构实测 RED：3 错误仅 3.7ms（ Busy-spin），修复后 ≥70ms（10+20+40 调度退避））+ `accept_backoff_doubles_and_caps` 数值表单测；`cargo test --workspace` 578 通过 0 失败）

### P3

- [x] R17 [P3] `platform/error.rs:63-64` `From<ExecResult> for PlatformError` 内对 `Succeeded` 变体 `unreachable!()`。 `6184053`（Succeeded 臂改为 `PlatformError::Failed("internal: a successful execution reached the error conversion")`——当前生产路径（exec_tool.rs 自行 match，从不转换 Succeeded）确实不可达，但未来调用方接线错误会让看门狗 panic 而非暴露失败；恒不 panic 原则下归入 Failed 并注明 internal。红测试 `success_reaching_the_conversion_never_panics`（对旧代码实测 RED：panic at error.rs:64 `unreachable!`；修复后返回 Failed 且消息含 internal）；`cargo test --workspace` 579 通过 0 失败）
- [x] R18 [P3] `http.rs:120,127` `JsonOp::Exists` 分支 `unreachable!()`（当前结构上不可达，一次重构后即可达）。 `6d6c1af`（json.rs 两处平行 match 的 `JsonOp::Exists => unreachable!()` 合并为单一全操算符 match：`Exists => ("exists", true)`（路径缺失由统一 `let ... else` 报 missing），删除两条 landmine 臂，行为对所有操算符逐一等价。红测试 `exists_assertion_survives_the_shared_evaluation_path`（对旧代码重构模拟——把 Exists 折入共享求值路径——实测 RED：panic at json.rs:75 `internal error: entered unreachable code`，证实「一次重构后即可达」；修复后 GREEN）；`cargo test --workspace` 580 通过 0 失败）
- [x] S1 [P3] `plugins/python/examples/file_health_probe.py:20,31` `target` 取自 params 直接用于 `os.path.getmtime` / `open(..., "a")`，无规范化和 `..`、绝对路径拒绝。 `2fdcaec`（新增 `resolve_target()`：参数 `target` 必须是裸相对文件名，normpath 后拒绝 `..` 逃逸/绝对路径/空值，解析到默认目标的父目录内；env 默认值仍归操作者。probe/recover 捕获 ValueError 返回 refused 结果而非崩插件。新建 stdlib unittest 套件 `plugins/python/tests/test_file_health_probe.py`（5 测试，零依赖）。对旧代码实测 RED 4/5：绝对路径 probe 返回 healthy=True、绝对路径 recover 触碰外部文件、`../escaped.txt` 真实写到上级目录、相对 target 依赖进程 CWD；修复后 5/5 GREEN）；rust 门禁复跑 580 通过 0 失败）
- [x] C6 [P3] `notify engine.rs:274-277` 投递后队列写失败只记日志，无对账（行停在 `sending` 直至过期回收）。 `b78cdd1`（外部投递副作用已发生，outcome 写丢失＝无法与未认领行区分→过期回收后重发（重复 webhook/邮件）。三类 mark_* 写改写为 `OutcomeWrite` 数据描述 + `record_outcome()` 有界重试（3 次、25/50/75ms 退避，warn 日志带 attempt）；全部失败仍回落 §8.6 stale-lease at-least-once 兜底。红测试 `outcome_write_failures_are_retried_instead_of_stranding_the_row`（新增 `FlakyMarkQueue` 包装前 2 次 mark_delivered 返回 store busy；对旧代码实测 RED：report.delivered=0（行滞留 Sending）断言失败；修复后 delivered=1/failed=0/状态 Delivered）；`cargo test --workspace` 581 通过 0 失败）
- [x] C9 [P3] `platform_tools.rs` `std::sync::MutexGuard` 跨慢速平台 I/O 持有；`try_lock` 把竞争表现为静默跳过的操作而非等待。 `8369814`（三处 try_lock 即时拒绝改为 `lock_network_ops(wait_ms)` 有界等待：20ms 轮询 try_lock，上限 `NETWORK_OPS_WAIT_MS=30s`，超时才报 "still running after 30000 ms" 拒绝；Mutex 中毒时 `into_inner()` 恢复（panic 的操作已死、unit guard 无状态）。跨慢速 I/O 持锁是互斥语义本身要求（同时只许一个网络操作），修复的是竞争面：等待而非烧掉 attempt ladder。红测试 `contending_network_operation_waits_for_the_in_flight_one`（250ms SlowReset 在飞 + 80ms 后竞争者 → 旧代码实测 RED：`Err("another network operation is already running")` 即时拒绝；修复后等待 ~200ms 成功且 elapsed≥60ms）；既有 `concurrent_network_operations_never_race` 改走 `lock_network_ops(0)` 零等待超时路径避免 30s 阻塞）；`cargo test --workspace` 582 通过 0 失败）
- ~~审核发现（不属实）：本轮 diff 未见问题（80ec03609f..HEAD）~~ 不属实：逐提交复核（`git show` + 独立复审 agent，含测试执行与 Rust 地址解析语义实证）发现 4 个代码问题——R16 回归（`resolve_target` 拒绝括号 IPv6）、R13 enable 失败半程态（网卡停管理性 down）、mihomo 文档/代码缺口（繁体选择器）、cloudflared 注释行误判，另加 8 处过期文档锚点（已修 `347b538`），记档于「审核发现 2026-11-09」待修。
- [x] 审核发现：Windows 路径可绕过 `resolve_target` 的 base 目录约束，`..\evil` 未被拦截且 `C:evil` 会脱离 base（plugins/python/examples/file_health_probe.py:36）。（属实：以真实 `ntpath` 语义实测——`..\evil` 归一化后 checks 全过、`ntpath.join(base, ..\evil)` 得 `base\..\evil` 逃逸；`C:evil` 为 drive-relative，join 直接丢弃 base 按驱动器 CWD 解析；`../evil` 在 Windows 下归一化为 `..\evil` 漏过 `../` 检查。修复：`resolve_target` 先整体拒绝反斜杠分隔符与 `[A-Za-z]:` 驱动器前缀，再归一化到 `/` 后拒绝任意存活 `..` 段，双平台（posixpath/ntpath）一致。红测试 `test_windows_style_paths_are_refused_under_windows_semantics`（patch `os.path`→`ntpath` 实测 RED：`ValueError not raised: ..\evil`；修复后 6/6 绿，`..\evil`/`C:evil`/`C:\evil`/`../evil`/`..` 全拒绝，合法 `inner.txt` 仍解析））
- [x] 审核发现：架构文档源码锚点已过期，`engine.rs:137`/`engine.rs:225` 与 `assemble.rs:23` 不再指向对应实现（docs/guide/source-architecture.md:64、docs/zh/guide/source-architecture.md:98）。（属实：实际符号位置为 `engine.rs:203`（dispatch）、`engine.rs:293`（deliver_due）、`assemble.rs:30`（attach_notifications）；文档锚点均已漂移。修复：中英两份文档第 64/98 行锚点更新为 203/293/30，均对应当前实现）

## 审核发现 2026-11-10（62429521..HEAD）

- [x] 审核发现：词法校验后的目标路径未解析符号链接，base 内符号链接可让 probe/recover 读取或触碰 base 外文件（plugins/python/examples/file_health_probe.py:52）。（属实：实测 base 内 `link → ../victim.txt`，probe 返回 `healthy: True`（经链接 stat 到 base 外文件）、recover 触碰外部文件。修复：`resolve_target` 对最终路径做 `realpath` 符号链接解析，`os.path.commonpath([base, resolved]) == base` 判含（base 本身也 realpath；commonpath 对异盘/不可比较对抛 ValueError＝判外部），返回解析后真实路径；base 内互链仍合法。红测试 `test_symlink_inside_base_is_resolved_and_contained`（实测 RED：`healthy: True` 经链接逃逸；修复后 GREEN：外部链接 probe/recover 均 refused 且 victim mtime 不变，内部 `alias → inner.txt` 仍 healthy））
- [x] 审核发现：包含 NUL 的目标参数通过词法校验后使 probe 在 stat 阶段抛出未捕获 ValueError（plugins/python/examples/file_health_probe.py:61）。（属实：实测 `bad\x00name` 通过词法校验后 `getmtime` 抛 `ValueError: stat: embedded null character in path`，`except OSError` 不接 → 未捕获逃出 probe()。修复：`resolve_target` 显式拒绝 NUL（`target must not contain NUL bytes`），probe/recover 的 I/O 层 `except` 扩为 `(OSError, ValueError)` 双保险——OS 层未来任何值错误降级为 unhealthy/failed 而非崩插件。红测试 `test_target_with_nul_byte_is_refused_not_raised`（实测 RED：`ValueError: stat: embedded null character in path` 未捕获；修复后 GREEN：两种 NUL 形态均返回 refused 结果））

## 覆盖率基线推进 2026-11-11（巡检单 2026-10-20）

- [x] 建立全 workspace 覆盖率基线并补最低模块测试（基线 cargo llvm-cov --workspace（default features；--all-features 因 custos-python `extension-module` 打包 feature 使 pyo3 不链 libpython、测试二进制无法链接，该 feature 为全仓唯一非默认 feature 且不含可测代码，故跳过）：region 82.21% / function 78.20% / line 81.47%，605 测试 0 失败 → 补测后 region 83.64% / function 79.88% / line 83.00%。最低三模块补行为测试 23 个（不改生产代码、不动既有测试）：plugins/openclash/src/main.rs line 0%→62.78%（`31b2ce8` 之后的 `f1c8e4b`：env 访问器默认/覆盖/空值、get/put 传输失败映射、probe 核不可达、switch_best_node 无组/核不可达、capabilities 契约、非 OpenWrt restart 失败路径，9 测试）、crates/custos-cli/src/main.rs 0%→37.45%（`2e6d48b`：parse_start_options 默认/全旗标/四条错误路径、db_arg、load_config 缺文件与坏 TOML、validate 过 init 模板、notify retry 参数拒绝，8 测试）、crates/custos-config/src/notification/mod.rs 41.77%→100%（`31b2ce8`：channel() 命中/未命中、SecretString 三重 redact+expose、smtp/telegram/herald/local-log 四通道 Debug 红action 契约，6 测试）。clippy --workspace --all-targets -D warnings 0 错。98% 上限说明：剩余缺口由外部 I/O 与事件循环主导——cli/openclash 的 main()/serve() stdin-RPC 循环、status/runs/incidents/notify 的真实 sqlite 打开、mihomo/cloudflared 插件 main 循环（0-31%）、store/runs.rs 与 notify/queue.rs 的真实投递路径，均需活的外部系统，单测无法触达且不改生产行为即不可达。注意：custos-toolbox `concurrent_network_operations_never_race` 在全量并行下偶发 "mutex free at test start: WouldBlock"（复跑 3 次单测 + 整 crate + 全 workspace 均 605/0 通过），既有 flaky，与本轮无关）

## 审核修复批 2026-11-12（巡检单 cov2 同批，报告 audit-batch3-6repos-2026-11-11.md custos 节）

- [x] P0-1 `run_controlled` 超时后 `join_reader` 可能永久阻塞（exec.rs:129-150，成功分支 :107-108 同款）——daemonize 子进程把 pipe 写端交给逃逸孙进程时 reader `read()` 永不 EOF，join 永不返回，探针线程/engine tick 挂死。修复=排空截止时间内置 reader（`BoundedPipe` trait：unix `poll(2)` 按 deadline 等就绪 / windows `PeekNamedPipe` 查可用量，非阻塞 peek+阻塞读管道原语，截止=spawn+timeout+2s `DRAIN_GRACE`；正常路径行为不变），全部三处 join 有界返回；windows-sys 补 `Win32_System_Pipes` feature。红测试×2：`escaped_pipe_writer_does_not_block_return`（`sleep 60 & echo done`，旧行为 join 60s）/`timeout_returns_despite_setsid_grandchild_holding_pipe`（setsid 逃逸+SIGKILL 不可达）。custos-probe 189 绿，`9a3334a`
- [x] P0-2 `execute_with_watchdog` 每 action 泄漏一个 worker 线程（executor.rs:61-69）——线程无法强杀，按审核建议落泄漏观测面：`LEAKED_WATCHDOG_WORKERS` AtomicUsize（超时 +1、迟到结果 −1）+ `leaked_watchdog_workers()` 公开读数 + warn 日志带 leaked 字段；builtin 执行器自 P0-1 后恒有界（生产接线仅 BuiltinExecExecutor，assemble.rs:69），插件执行器挂死即在此计数可见，进程外隔离记为后续架构项。测试×2：C20 测试扩断言（挂起时恰 +1、放行后清零，.Mutex 串行化防并行互扰）+ 按期执行零泄漏。custos-engine 35+10 绿，`754d7bd`
- [x] P0-3 `PluginSession::start` 传输线程 spawn 失败泄漏子进程（session.rs:193-208）——修复=`reap_failed_start` 帮助函数（mark_dead + writer Close 排空 + `wait_child_exit(3s)` 有界收尸再返回 Err），三个 spawn 失败路径统一走它。fixture 补 `MOCK_PLUGIN_LEAK_FD_LATCH`（子进程泄漏一个进程级 fd 模拟 daemonize 写端继承，unix-only）。测试×4：集成两例（shutdown/drop 在泄漏写端存在下 <8s 收尸、latch 内容=leaked fd 4 证明管道确被持住）+ 单元两例（`reap_failed_start` 直接驱动：sleep 30 被 SIGKILL 后 `kill -0` 报不存在 + 快退子进程有界返回）。plugin-host 16 单元+31 集成绿，`be44e56`
- [x] P0-4 `redrive`/`wake_ready_waiters`/`try_recover` 对 `managed` 直索引可 panic（orchestrate.rs:46/:94、recover.rs:12）——恢复路径 replay 的 run 可引用已删资源（配置变更+崩溃恢复）。修复=三处守卫：redrive 缺失即 `finish_cancelled` + warn；wake_ready_waiters 缺失即取消该 dep 全部 parked waiter；try_recover 缺失即 Err(Stopped) + warn（resume_unfinished 非 Waiting 非终态分支同样经 resume_queue→redrive 守卫覆盖；新配置世界的依赖目标/组员在 config build.rs:91-97/:148-166 已校验，索引安全）。测试×3（redrive/wake/try_recover 各一，ghost 资源 run 必 Cancelled 不 panic）。custos-engine 38+10 绿，`f721fd3`
- [x] P1-5 `spawn_call_with_deadline` Disconnected 分支泄漏 pending 条目——**核实不成立**：`Disconnected` 分支的 `pending.remove(&id_key)` 自首个 commit（66b6255:298-301）即存在，现行 HEAD（session.rs:319-322）三分支（write 错误/Timeout/Disconnected）齐全，审核报告与代码不符，零改动
- [x] P1-7 `SpooledDispatch::spawn` 的 `.expect()` 可崩装配线程（spool.rs:45）——修复=spawn 失败降级纯 inline 模式（`tx: Option<SyncSender>`，None 时 dispatch 直走 `inline`，warn 日志），C18「通知不拖垮恢复」贯彻到线程创建失败。测试×1（tx: None 降级 spool 仍逐事件入队）。custos-notify 71+2 绿，`d763a3a`
- [x] P1-8 LogProbe 基线竞态——审核报告自行撤回（核实 `read_new(path, offset, max)` 从 offset 读、append 字节纳入下次读取，不构成 bug），零改动
- [x] P1-9 `build_request` Proxy-Authorization 恒为字面 `***`——**核实不成立**：该文件历史仅 0c5d5ef 一笔且首版实现即 `if let Some(auth) = auth { push_str("Proxy-Authorization: Basic {auth}") }`，全史无 `***` 字面（`git grep '\*\*\*' 0c5d5ef` 空），测试 `request_is_absolute_form_with_auth` 锁真实凭据头，审核报告与代码不符，零改动
- [x] P1-6 `suspend_on` 与 `burn_wait_cycles` 双重计费——**核实不成立（逐 tick 推演）**：两者计费不同事件（suspension vs retry-failure）且交替不重叠——`suspend_on` 落位的全部三条路径（active/cooldown、恢复失败后 apply_cooldown、refusal 走 escalate 不 park）都意味着 dep 进入 cooldown，同 tick 的 `wake_ready_waiters` 必因 `cooling_down` 短路跳过 burn；后续 tick 每次 burn 只对应一次真实失败的依赖观测，cooldown tick 不计费，无同一观测双计费面，MAX_WAIT_CYCLES=5 语义不变。不凑数修复，零改动
- [x] 收口验证：cargo test --workspace 616 绿 0 失败（含 P1-7 新增降级测试）；cargo clippy --workspace --all-targets -D warnings 0 错。四修 + 五核不成立/撤回，生产代码改动限四处最小面，无凑数测试

## 续推复核 2026-11-12（巡检单 custos-next：6 项未勾，前 3-5 项分批交付）

- [x] 复核结论：未勾项共 6 项（第 224/225/241/242/244/275 行），**6 项全部标注 BLOCKED**，按巡检单纪律「跳过标注 BLOCKED 的项」全部跳过；可交付项 0 项，本批无代码改动、无 commit（不凑数）
  - 验收场景 1（向日葵端到端）/ 验收场景 2（Wi-Fi 优先恢复）/ macOS 适配 / BSD 适配：均标注 BLOCKED 且环境性阻塞未解除（本机 Linux x86_64、无向日葵安装、无 wlan* 接口、无可访问的 macOS/BSD 主机；三台真机 imac/openclaw/win-worker 按 2026-11-08 记录均为 Windows，且 openclaw/win-worker 对本机无公钥授权、imac 无路由）
  - 插件签名与分发策略：BLOCKED 待用户拍板（签名与信任模型、分发源、密钥/信任锚），巡检不代行用户裁定
  - vitepress 2.0 去 alpha：BLOCKED 待上游发布（npm latest 仍 1.6.4，next 仍 2.0.0-alpha.21）
- [x] 六项均按 复核 2026-11-12 追加当日证据（逐条实测命令与读数，见各条目行内），沿用 2026-11-04/05/06/07 的复核口径
- [x] 无未勾项可推进时不新造任务：本批仅落账复核证据，不新增 `- [ ]` 项、不动生产代码
- [x] 顺带修复既有 flaky：`concurrent_network_operations_never_race` 在全量并行下偶发 `mutex free at test start: WouldBlock`（todo.md:527 已记档）——根因是它与 `contending_network_operation_waits_for_the_in_flight_one` 并行运行时后者先执行完释放锁，前者 try_lock 成功后再无锁可断言竞态拒绝路径。修复：新增 `NETWORK_OPS_TEST_LOCK` 静态互斥锁串行化两测试（crates/custos-toolbox/src/platform_tools.rs:254-258，同 executor.rs:185 LEAK_COUNTER_TEST_LOCK 范式）。验证：`cargo test -p custos-toolbox --lib` 连续 5 轮 24/24 绿；`cargo test --workspace` 616 passed / 0 failed；clippy -D warnings 0 错
## Codex 深审发现（2026-11-12，luna profile 近5 commit 审核，0C/1M/3m，待修）

- [ ] [M] 看门狗泄漏计数修超时/迟到结果竞态（crates/custos-engine/src/executor.rs:79-99）——worker 仅在 send 失败时减计数，主线程 Err 分支提前减导致双重扣减
- [ ] [m] inline 降级日志文案按实际路径区分（crates/custos-notify/src/spool.rs:83-92）——缺 tx 的降级被记成 spool saved
- [ ] [m] 刷新 todo.md:531-536 修复锚点行号（P0-1/P0-3/P0-4/P1-7 引用已漂移）
- [ ] [m] todo.md:224 环境复核证据补可复现命令输出
- [x] LUCI-ISTORE-001 [P1] LuCI 界面完善 + iStore 上架包（2026-11-13 用户令「写界面并出包，计划上 iStore」，2026-11-14 交付，单 commit）
  - ① 状态面板：controller 新路由 `api/overview`（packaging/openwrt/usr/lib/lua/luci/controller/custos.lua:49-150，procd 状态/UCI db/`custos status|runs|incidents --json` 三次 CLI 汇聚，sanitize 截断+%c 清洗、shell_quote 单引号、缺库/无输出/解析失败逐段错误如实透出）；status.htm 五卡（服务状态+计数、资源健康、最近事件、事故、恢复运行，packaging/openwrt/usr/lib/lua/luci/view/custos/status.htm:6-44），全部经 DOM textContent 渲染（无 innerHTML，XSS 不可达）、10s 自动刷新带 inflight 防重（:186-187）、空态/错误横幅如实显示
  - ② 设置页：CBI 补「监控配置文件路径」conf（model/cbi/custos.lua:25-28）+「通知设置」NamedSection（:37-84，telegram/herald/local-log 最小字段，no_space 校验防空白破坏 procd env 分词）；接线走 init.d：notify_pairs() 只对完整通道对发 `CUSTOS_NOTIFY_*` KEY=VALUE（etc/init.d/custos，半配不致命），procd_set_param env 透传；引擎侧 custos-config notification::env_overlay_from(get, fallback) 纯 getter 设计（notification/mod.rs，进程不读真实 env，并行测试零污染）整体替换 TOML [notification] 段，未重写配置系统；build.rs 接线 + notification/tests.rs 10 例
  - ③ CLI 支撑：custos-cli status/runs/incidents 加 `--json`（serde_json 构造，文本路径不变），history_query_args 统一解析；测试 3 例（--json 标志、status_json 计数+健康+审计、runs/incidents 种子库往返）
  - ④ iStore 规格：docs/analysis/istore-publishing-2026-11-14.md——GitHub 本机不可达（raw HTTP:000/gh EOF/clone 连接关闭）如实记录，规格改由真机 192.168.5.11 已装 app-meta 包逐字段一手核实（/usr/lib/opkg/meta/<name>.json + /www/luci-static/resources/app-icons/<name>.png + istorea/istoree 钩子，lan-wake.json 全字段引证，tags 词表 net/tool/multimedia/download/ai）；正式进商店需联系 Linkease（admin@linkease.com），文档留待办
  - ⑤ 出包：build-ipk.sh 重写三包 custos/luci-app-custos/app-meta-custos（CONTROL 12 字段齐全、中英双语 Description、conffiles 标记 etc/config/custos+etc/custos/custos.toml+etc/init.d/custos、postinst/prerm 幂等且 0755——非 755 会 configure exit 126 实机踩过）、`--arch x86_64|aarch64`/`--rel N` 参数化；图标 assets/icons/custos.png（128×128）；nightly 矩阵补 aarch64-musl 原生腿（ubuntu-24.04-arm），ipk 步骤按 matrix.target 传 --arch
  - ⑥ 真机验收（192.168.5.11，iStoreOS）：三包 0.1.0-2 在位（opkg list-installed）；升级路径实测 v1→v2：opkg 报 "Existing conffile /etc/custos/custos.toml is different…placed at custos.toml-opkg"，改动版保留（哨兵行数 1、md5 5be46cf3fd9a39515614c418fd834ed7 升级前后不变）✓；`uci set custos.main.enabled=1` 后 `/etc/init.d/custos status`=running，ps 实测 `/usr/bin/custos start /etc/custos/custos.toml --db /etc/custos/custos.db --interval 5000`（conf UCI 接线生效），health `at` 1794590266518→1794590286609 持续推进（间隔 5s 档）✓ procd 重启后探测继续；控制器端到端：设备上 lua 驱动（require luci.util/jsonc/model.uci + 桩 luci.http，dofile controller 后调 luci.controller.custos.api_overview()）→ ct=application/json、status=running、db=/etc/custos/custos.db、health_rows=4（dnsmasq ok=true at=2026-11-14 01:19:27）、audit/runs/incidents 0 行=新库真实空态、error=nil ✓；浏览器页面实测未做（LuCI 登录凭据未知，如实记录）；旧单包→三包迁移需 `--force-reinstall`（会替换 conffile）已在 README 注明
  - ⑦ 门禁：cargo test --workspace 629 passed / 0 failed（47 个测试二进制）；clippy --workspace --all-targets -D warnings 0 错；cargo fmt --all --check 过；luac -p 两 lua 文件过；README.md/README.zh.md 双语补 OpenWrt/iStoreOS 节（三包、安装顺序、conffile 升级语义、迁移口径）
- [ ] LUCI-RES-002 [P1] LuCI 资源/恢复策略编辑面（2026-11-14 用户令「监控啥、什么事件做什么操作」）：TOML 直接编辑路线（UCI 中转会造第二套 schema 翻译层易漂移）——①资源列表卡+新建/编辑/删除 ②编辑表单按 type 分域（首批四种常用+高级 TOML 直编逃生口，解析拒绝回显行号）③恢复计划编辑（plan_id/faults/risk/timeout/attempts/cooldown/action/verification，risk=high 强制二次确认）④极小 TOML 子集读写器（round-trip 保真=真机现有 toml diff 为空，未知结构原样保留）⑤写回安全（.bak 备份+custos validate 过才 restart，失败回滚显错）⑥真机 192.168.5.11 验收（改阈值生效/新建无害资源探测出现/确认框/回滚路径）。验收：round-trip 单测+真机四项全过+luac -p。

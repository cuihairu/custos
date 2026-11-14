---
title: Notification Engine
---

# 8. Notification Engine

## 8.1 设计目标

Notification 是独立子系统。

Core Engine 只产生结构化事件，不直接调用 Telegram、SMTP 或其他平台的 API。

通知系统负责：

- 事件筛选。
- 规则匹配。
- 通知构建。
- 通知队列持久化。
- 渠道选择。
- 重试与退避。
- 去重和聚合。
- 送达状态。
- 恢复后补发。

通知发送失败不得阻塞 RecoveryRun。

## 8.2 内置渠道

已配置并分发的通道有五个（`crates/custos-config/src/notification/raw.rs:277-300`）：

### Local Log

本地结构化日志，作为基础审计输出。每个事件都会先写入本地日志，再发其他通道，因此即使所有远程通道都不可用，本地也一定留有一条审计记录。

### Webhook

向用户配置的 HTTP 或 HTTPS 接口发送 JSON 事件。

已实现的行为：

- 请求超时。
- 重试与退避。
- 事件 ID。
- 可配置认证头。
- 可选 HMAC 签名。
- TLS 验证。
- 受控的目标地址策略。
- 对私网地址、回环地址和重定向的 SSRF 防护策略。
- 不记录密钥。
- 明确区分 HTTP 请求失败和对方业务确认失败。

### SMTP Email

常规 SMTP 邮件告警。

已实现的行为：

- 连接与发送超时。
- TLS 支持。
- 凭据安全读取。
- 可配置发件人和收件人。
- 发送失败重试。
- 明确的发送结果。

### Telegram

按 bot token 投递到配置的会话，OpenWrt init 脚本的覆盖通道用它（`CUSTOS_NOTIFY_TELEGRAM_TOKEN` / `CUSTOS_NOTIFY_TELEGRAM_CHAT`，见 §8.5）。

### Herald

投递到 Herald 实例，支持 API-key 或 bearer 认证，SSRF 姿态与 webhook 通道一致。

Console 不在其中：配置里没有 console 通道，`crates/custos-notify` 里也没有 console 通道实现。本地观察靠本地日志通道，加上 CLI 自身的 `status`、`runs`、`incidents` 输出。

## 8.3 第三方通知插件

尚未内置：

- Discord。
- Slack。
- 企业微信。
- 钉钉。
- 飞书。
- ntfy。
- Gotify。
- 用户自定义通知渠道。

这些通道通过统一 NotificationChannel 接口接入，不需要在 Core Engine 中加入各平台专属逻辑。

## 8.4 事件模型

```rust
struct NotificationEvent {
    event_id: EventId,
    event_type: EventType,
    severity: Severity,
    resource_id: Option<ResourceId>,
    incident_id: Option<IncidentId>,
    recovery_run_id: Option<RecoveryRunId>,
    occurred_at_unix_ms: u64,
    summary: String,
    details: SanitizedDetails,
}
```

主要事件类型：

```text
resource.unhealthy
resource.degraded
recovery.started
recovery.action_started
recovery.action_failed
recovery.succeeded
recovery.exhausted
recovery.outcome_unknown
plugin.unavailable
notification.delivery_failed
configuration.invalid
```

`notification.delivery_failed` 应受到特殊限制，避免通知系统自身故障产生无限通知循环。至少应允许仅写入本地日志。

## 8.5 通知策略

通知规则与恢复规则必须独立。

示例：

- 故障首次出现：记录日志。
- 故障持续超过阈值：发送告警。
- 自动恢复成功：可配置是否通知。
- 恢复耗尽：发送高优先级告警。
- 插件不可用：根据受影响资源的重要性决定是否通知。
- 同一故障持续发生：聚合并抑制重复通知。
- 故障恢复：发送恢复通知。

### 环境变量覆盖（`CUSTOS_NOTIFY_*`）

除了 TOML 的 `[notification]` 段，引擎启动时还接受环境变量覆盖（OpenWrt 的 init 脚本用它把 LuCI 设置页的通道配置透传给引擎）：

- 通道选择变量：`CUSTOS_NOTIFY_LOCAL_LOG`；`CUSTOS_NOTIFY_TELEGRAM_TOKEN` + `CUSTOS_NOTIFY_TELEGRAM_CHAT`；`CUSTOS_NOTIFY_HERALD_URL`（另加可选的 `CUSTOS_NOTIFY_HERALD_APP` / `_TOKEN`）。
- `CUSTOS_NOTIFY_MIN_SEVERITY` 只降低严重度下限，本身不选通道。
- 语义：只要**任一**通道选择变量存在，覆盖就**整体替换** `[notification]` 段（绝不回写 TOML）；只设 `CUSTOS_NOTIFY_MIN_SEVERITY` 而无通道选择变量时为无操作。

## 8.6 断网时的行为

网络中断时：

1. 生成 NotificationEvent。
2. 持久化到通知队列。
3. 标记为 Pending。
4. 不阻塞恢复任务。
5. 后台发送器根据退避策略尝试发送。
6. 网络恢复后继续处理。
7. 成功后记录 Delivered。
8. 结果不确定时，依据渠道策略决定是否重试。

不能保证所有外部渠道实现严格的端到端 exactly-once 送达。Custos 应采用稳定事件 ID、尽力去重和可观测的发送状态。

建议状态：

```text
Pending
Sending
Delivered
RetryScheduled
DeadLetter
```

`Delivered` 只代表渠道返回了其定义的成功结果；除非对方提供确认机制，否则不能声称最终用户一定看到了通知。

队列级重试梯次有上限：累计尝试达到 `MAX_QUEUE_ATTEMPTS` 后，瞬时失败转为死信（DeadLetter）而不是无限重试——渠道以可见的终态暴露给人，而不是静默空转；尝试计数与重试排期保持有界。

重试与退避由队列端到端负责：每个渠道每次投递只发一次请求——渠道内没有睡眠重试链——慢渠道不会拖住串行投递，重试也只排期一次。渠道配置中的 `retries`/`backoff_ms` 仍被接受（兼容既有配置），但不再驱动渠道内重试梯次。

## 8.7 安全要求

- 不在日志中记录 Token、密码、私钥和完整认证头。
- 配置文件中支持安全的凭据引用。
- Webhook 地址必须经过校验。
- 对插件的网络访问权限进行限制。
- 不允许第三方通知插件直接修改 RecoveryRun。
- 通知插件只能读取必要的事件数据。
- 发送队列不得保存不必要的敏感上下文。

---

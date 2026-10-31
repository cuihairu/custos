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

第一版内置：

### Local Log

本地结构化日志，作为基础审计输出。

### Webhook

向用户配置的 HTTP 或 HTTPS 接口发送 JSON 事件。

要求：

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

Webhook 应当是优先实现的通用外部渠道。

### SMTP Email

支持常规 SMTP 邮件告警。

要求：

- 连接与发送超时。
- TLS 支持。
- 凭据安全读取。
- 可配置发件人和收件人。
- 发送失败重试。
- 明确的发送结果。

### Console

CLI 实时展示事件，用于本地调试和运行观察。

Console 不是远程通知渠道。

## 8.3 第三方通知插件

支持：

- Telegram。
- Discord。
- Slack。
- 企业微信。
- 钉钉。
- 飞书。
- ntfy。
- Gotify。
- 用户自定义通知渠道。

这些渠道通过统一 NotificationChannel 接口接入，不需要在 Core Engine 中加入各平台专属逻辑。

第一版不需要全部内置。优先保证 Webhook 和 SMTP 可用。

## 8.4 事件模型

```rust
struct NotificationEvent {
    event_id: EventId,
    event_type: EventType,
    severity: Severity,
    resource_id: Option<ResourceId>,
    incident_id: Option<IncidentId>,
    recovery_run_id: Option<RecoveryRunId>,
    occurred_at: DateTime,
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

## 8.7 安全要求

- 不在日志中记录 Token、密码、私钥和完整认证头。
- 配置文件中支持安全的凭据引用。
- Webhook 地址必须经过校验。
- 对插件的网络访问权限进行限制。
- 不允许第三方通知插件直接修改 RecoveryRun。
- 通知插件只能读取必要的事件数据。
- 发送队列不得保存不必要的敏感上下文。

---

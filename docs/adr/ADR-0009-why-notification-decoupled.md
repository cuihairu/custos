# ADR-0009: 为什么通知系统与恢复引擎解耦

- 状态：已接受
- 日期：2026-10-31
- 关联：docs/guide/notification.md §8

## 背景

通知渠道（Webhook/SMTP/herald）依赖网络与第三方可用性；恢复引擎依赖本地资源。
耦合两者意味着：断网时恢复被发送卡住；渠道故障放大成系统故障；
`delivery_failed` 再触发通知会无限自激。

## 决策

Core Engine 只产生结构化 `NotificationEvent`（11 种类型，custos-model），
经 EventEmitter 回调交给 custos-notify；通知引擎负责规则匹配、入队
（SQLite 五状态队列）、后台投递、指数退避、死信、去重聚合；
入队与发送**永不阻塞**恢复（§8.1 验收）；`notification.delivery_failed`
只落本地日志，构造性断开自激环。持久化经 `NotificationQueue` port trait，
notify 不反向依赖 store/engine，依赖图无环。

## 替代方案

- **恢复引擎内联发送**：渠道慢即恢复慢；渠道崩即恢复崩；无法离线补发。
- **共享线程池直发**：无持久化，断网丢事件；无重试语义。

## 优缺点

优点：断网期间事件全部留队、恢复后补发（验收测试 offline.rs）；
渠道可插拔（local-log/webhook/smtp/herald 同一 Channel trait）；
敏感信息治理（scrub）在一处集中执行。
缺点：多一层队列状态机（Pending/Sending/Delivered/RetryScheduled/DeadLetter）；
端到端只有 at-least-once，依赖稳定事件 id + 接收方去重。

## 后续影响

- Webhook/SMTP/herald 渠道按此 trait 增补；新渠道不触碰核心。
- CLI `notify status/retry` 读队列；恢复与通知的验收门分离。

---
title: 持久化与崩溃一致性
---

# 7. 持久化与崩溃一致性

## 7.1 数据库

第一版使用 SQLite。

建议启用 WAL，并根据可靠性要求选择适当的 `synchronous` 策略。不能假设 WAL 自动保证每条记录都已经持久化，也不需要无条件在每个日志写入后手动执行 fsync。

对关键恢复操作，应明确事务边界。

## 7.2 核心表

建议至少包括：

```text
resources
resource_dependencies
resource_health
incidents
recovery_runs
recovery_steps
probe_results
diagnoses
action_attempts
verification_results
audit_events
notification_queue
notification_attempts
plugin_registry
configuration_versions
```

其中：

- `resources` 保存资源定义。
- `resource_dependencies` 保存依赖关系。
- `resource_health` 保存当前健康快照。
- `incidents` 保存故障生命周期。
- `recovery_runs` 保存恢复任务。
- `recovery_steps` 保存步骤顺序和状态。
- `probe_results` 保存探测证据。
- `diagnoses` 保存诊断结果。
- `action_attempts` 保存每次动作尝试。
- `verification_results` 保存验证结果。
- `audit_events` 保存事件审计。
- `notification_queue` 保存待发送通知。
- `notification_attempts` 保存发送历史。
- `plugin_registry` 保存已加载插件及能力信息。
- `configuration_versions` 保存配置版本或变更摘要。

## 7.3 高风险动作的持久化顺序

执行高风险动作前：

1. 校验资源和依赖条件。
2. 校验动作权限。
3. 校验预算和互斥锁。
4. 创建 ActionAttempt。
5. 将状态标记为 `Executing`。
6. 在事务提交成功后执行外部动作。
7. 记录动作返回结果。
8. 重新验证资源健康。
9. 更新 RecoveryRun 状态。

如果 Custos 在第 6 步期间崩溃，则恢复后必须识别执行结果可能未知。

不能把“数据库状态写成 Executing”误认为“外部动作一定执行了”，也不能把“外部动作返回 0”误认为“服务一定恢复”。

## 7.4 数据保留

配置：

- 审计记录保留周期。
- 探测明细保留周期。
- 恢复任务保留周期。
- 通知发送记录保留周期。
- 数据库大小限制。

默认不删除仍处于活动状态的 Incident、RecoveryRun 或尚未送达的通知。

日志压缩或清理必须与活动任务状态分离。

---

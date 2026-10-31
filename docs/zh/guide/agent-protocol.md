---
title: Code Agent 执行规范
---

# 15. Code Agent 的执行规范

## 15.1 不允许直接根据文档一次性生成全部项目

必须分阶段实现。

每个阶段先检查当前仓库状态，再创建任务清单和实施计划。

如果仓库已经存在代码：

- 不得覆盖已有有效实现。
- 不得假设之前的架构已经过时。
- 必须分析现有调用路径和模块依赖。
- 先识别兼容性影响，再决定重构范围。
- 保留已有测试并补充回归测试。

## 15.2 每个任务必须输出

1. 修改文件清单。
2. 修改原因。
3. 接口变更。
4. 状态机影响。
5. 数据库迁移影响。
6. 平台兼容性影响。
7. 安全影响。
8. 新增测试。
9. 执行的验证命令。
10. 验收结果。
11. 未解决的问题。

不能只提交代码而没有说明验证结果。

## 15.3 Agent 的工作顺序

每个阶段遵循：

```text
Inspect Repository
    ↓
Create Implementation Plan
    ↓
Define Interfaces
    ↓
Implement Minimal Vertical Slice
    ↓
Add Unit Tests
    ↓
Add Integration Tests
    ↓
Run Formatting and Linting
    ↓
Run Test Suite
    ↓
Review Security and Failure Paths
    ↓
Update Documentation
    ↓
Report Results
```

每个阶段必须在测试通过后再进入下一阶段。

## 15.4 建议的首批任务

### Task 001：领域模型

实现 Resource、Dependency、HealthStatus、Evidence、Diagnosis、RecoveryPolicy、RecoveryRun、RecoveryAction、VerificationResult 和 AuditEvent。

交付：

- 类型定义。
- 状态转换规则。
- 序列化和反序列化测试。
- 不合法状态转换测试。
- 领域模型文档。

### Task 002：配置与依赖图

实现配置解析、引用验证、依赖图构建、循环检测和依赖条件验证。

交付：

- 配置 Schema 或等效验证规则。
- 依赖图算法。
- 错误路径定位。
- 正常、缺失依赖、重复 ID 和循环依赖测试。

### Task 003：恢复状态机

实现 RecoveryRun 状态转换、恢复预算、资源级互斥、动作执行和验证闭环。

交付：

- 完整状态机。
- 失败后的重新诊断。
- 第二候选方案。
- 动作结果未知处理。
- 并发恢复测试。

### Task 004：SQLite 持久化

实现数据库迁移、RecoveryRun 持久化、动作记录和恢复任务重启。

交付：

- 数据库 Schema。
- 事务边界说明。
- 进程崩溃测试。
- 数据库迁移测试。
- 未完成任务恢复测试。

### Task 005：Plugin Host

实现 JSON-RPC stdio、插件生命周期、消息大小限制、超时和取消。

交付：

- 插件协议文档。
- 示例插件。
- 协议测试。
- 大量输出测试。
- 插件崩溃和超时测试。

### Task 006：Notification Engine

实现事件规则、持久化队列、Webhook、SMTP、本地日志和发送重试。

交付：

- 统一通知接口。
- 事件模型。
- 持久化队列。
- 渠道实现。
- 断网重试测试。
- 凭据脱敏测试。

### Task 007：CLI

实现状态、资源、依赖图、故障历史、恢复历史和通知状态命令。

交付：

- CLI 命令。
- 错误输出规范。
- 帮助文档。
- 集成测试。

### Task 008：首个真实恢复场景

优先选择一个简单且可以自动验证的场景，例如受控进程退出后自动恢复。

随后实现代理与 Cloudflare Tunnel 场景，验证依赖恢复顺序。

交付：

- 示例配置。
- 故障注入测试。
- 完整 RecoveryRun 记录。
- 恢复成功与失败的通知结果。

---

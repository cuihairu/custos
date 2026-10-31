---
title: Resource 与 Dependency 模型
---

# 3. Resource 与 Dependency 模型

## 3.1 Resource 定义

Resource 是 Custos 监控和恢复的基本对象。

资源类型示例：

- `process`
- `system-service`
- `network-interface`
- `network-connectivity`
- `proxy`
- `tunnel`
- `http-endpoint`
- `custom-command`
- `database`
- `application`

每个资源应具有：

```text
Resource
├── id
├── type
├── display_name
├── plugin_ref
├── config
├── health_policy
├── recovery_policy
├── dependencies[]
├── permissions
└── enabled
```

字段语义：

- `id`：配置范围内稳定且唯一的标识。
- `type`：资源类型。
- `plugin_ref`：实现资源能力的插件。
- `config`：资源类型专属配置。
- `health_policy`：健康判定、连续失败阈值和恢复确认要求。
- `recovery_policy`：恢复候选方案和恢复限制。
- `dependencies`：资源的前置依赖。
- `permissions`：该资源允许的操作范围。
- `enabled`：是否启用监控和自动恢复。

资源 ID 不应因进程重启、插件重启或机器重启而变化。

## 3.2 Resource 健康状态

建议使用以下健康状态：

```rust
enum HealthStatus {
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
    Recovering,
    Suppressed,
    Unreachable,
}
```

状态语义：

- `Unknown`：尚未获得足够证据。
- `Healthy`：满足健康条件。
- `Degraded`：部分能力异常，但仍能提供服务。
- `Unhealthy`：明确不满足健康条件。
- `Recovering`：正在执行恢复流程。
- `Suppressed`：依据策略暂停自动处理。
- `Unreachable`：探测手段本身无法取得有效结果，尚不能确认根因。

`Unreachable` 不能直接等同于 `Unhealthy`。例如，探测远端服务超时，可能是远端故障，也可能是本机网络故障。

资源状态还应记录：

- 状态更新时间。
- 连续成功与失败次数。
- 最近一次有效探测时间。
- 当前生效的故障标识。
- 正在执行的 RecoveryRun。
- 当前证据摘要。

## 3.3 Dependency 定义

依赖关系描述资源正常运行所需要满足的条件。

建议定义：

```rust
struct Dependency {
    target_resource_id: ResourceId,
    requirement: DependencyRequirement,
    condition: HealthCondition,
    resolution_policy: DependencyResolutionPolicy,
    timeout: Duration,
}
```

其中：

```rust
enum DependencyRequirement {
    Required,
    Optional,
    Conditional,
    AlternativeGroup,
}
```

具体语义：

### Required

必须满足的依赖。

如果不满足，则不能执行依赖该条件的恢复动作，除非该动作被明确标记为用于恢复该依赖本身。

### Optional

非强制依赖。

可以尝试恢复，但是否阻塞上层操作由策略决定。

### Conditional

仅在特定条件下需要满足。

例如，使用代理模式时需要代理健康；直连模式下则不需要。

### AlternativeGroup

多个候选资源中满足指定数量即可。

例如：

```text
ProxyAccess
├── PrimaryProxy
└── BackupProxy

要求：至少一个可用
```

不能把备用资源简单地建模成所有依赖都必须正常。

## 3.4 健康条件

依赖必须声明依赖的是哪种健康条件，而不是只声明依赖某个资源。

示例：

```yaml
dependencies:
  - resource: proxy.egress
    requirement: required
    condition:
      status: healthy
```

更复杂的条件可以要求：

- 资源状态为 Healthy。
- 特定能力可用。
- 最近一次成功探测不超过指定时间。
- 连续成功达到指定次数。
- 特定业务检查通过。

第一版应只实现少量明确、可测试的条件，不要设计任意表达式语言。

## 3.5 依赖图和循环检测

依赖关系构成有向图。

如果资源 A 依赖 B，则依赖边为：

```text
A -> B
```

恢复 A 时，先评估 B。

配置加载时必须：

1. 验证资源 ID 唯一。
2. 验证所有依赖引用存在。
3. 检测硬依赖循环。
4. 验证 AlternativeGroup 的配置合法。
5. 检查是否存在不可能满足的依赖条件。
6. 输出具体的配置错误路径。

对于：

```text
A -> B -> C -> A
```

必须在配置阶段拒绝启动，不能等到恢复时才无限递归。

第一版可以禁止所有普通依赖环；未来如果需要循环或互为备份关系，应通过显式的冗余组建模，而不是允许任意循环。

## 3.6 依赖恢复算法

恢复资源 A 时：

1. 获取 A 的依赖集合。
2. 检查每个依赖的当前状态是否足够新。
3. 状态未知或已过期时重新探测。
4. 满足条件的依赖直接通过。
5. 未满足的依赖按照自身策略进入恢复流程。
6. 等待依赖恢复完成。
7. 重新验证依赖健康状态。
8. 全部必需依赖满足后，才允许执行 A 的恢复动作。
9. 如果依赖恢复失败，则根据 A 的策略决定等待、跳过非关键依赖、终止或升级告警。

必须区分两种动作：

- 为了修复依赖本身而执行的恢复动作。
- 必须等待依赖健康后才能执行的上层动作。

例如，代理不健康时，允许执行“切换代理节点”；但不应该执行依赖代理才能完成的 Cloudflare Tunnel 重启，然后期待它成功。

## 3.7 并发与依赖去重

同一资源可能被多个上层资源依赖。

例如：

```text
Cloudflare Tunnel ──> Proxy
Sunlogin ────────────> Network
Other Service ───────> Proxy
```

如果多个恢复任务同时发现 Proxy 不健康，不能同时启动多个代理重启任务。

要求：

- 每个资源有独立的恢复互斥锁。
- 同一资源同时最多有一个活动 RecoveryRun。
- 其他任务可以等待同一个恢复结果。
- 资源健康状态变化时，等待中的任务重新评估依赖。
- 恢复任务不能重复启动相同的高风险动作。
- 必须定义全局与资源级恢复预算。

---

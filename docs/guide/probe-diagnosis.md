---
title: 探测、诊断与故障判定
---

# 5. 探测、诊断与故障判定

## 5.1 Probe 与 Diagnosis 分离

Probe 负责收集事实。

Diagnosis 根据多个事实判断故障原因。

例如：

```text
Probe A: Wi-Fi 已连接
Probe B: 已取得 IP 地址
Probe C: 默认网关可达
Probe D: 公网 IP 不可达
Probe E: 代理本地端口可连接
Probe F: 代理认证失败
```

这些证据不能简单合并成“网络故障”。

诊断结果应包括：

```text
Diagnosis
├── fault_code
├── summary
├── confidence (optional)
├── evidence_refs[]
├── affected_resources[]
├── recommended_actions[]
└── limitations[]
```

`confidence` 是可选的规则置信度，不应伪装成统计学概率。

## 5.2 故障类型

第一版至少区分：

- `process_exited`
- `service_stopped`
- `probe_timeout`
- `network_disconnected`
- `gateway_unreachable`
- `dns_failure`
- `proxy_connection_failed`
- `proxy_authentication_failed`
- `remote_endpoint_unreachable`
- `plugin_unavailable`
- `dependency_unsatisfied`
- `recovery_action_failed`
- `verification_failed`
- `unknown_failure`

未知故障不得直接触发所有可能的恢复动作。

## 5.3 连续失败与抖动抑制

不能因为一次超时就立即重启服务。

健康策略需要支持：

- 连续失败阈值。
- 连续成功阈值。
- 探测超时。
- 失败窗口。
- 恢复确认窗口。
- 故障冷却时间。

例如：

```yaml
health_policy:
  interval: 10s
  timeout: 3s
  failure_threshold: 3
  success_threshold: 2
```

此处为示例值，实际值应按资源类型设置。

阈值只负责决定是否进入故障状态，不应被用来掩盖持续异常。

## 5.4 P0 的探测范围

P0 只实现：

- Process Probe。
- Exec Probe。

P0 暂不实现 HTTP Probe。

Process Probe 用于检查进程是否存在以及必要的进程属性。

Exec Probe 用于执行受控的检查命令，并解析：

- 退出码。
- 标准输出。
- 标准错误。
- 超时。
- 启动失败。
- 输出是否被截断。

Exec Probe 必须限制运行时间和输出大小，并在超时时清理进程树。Windows 上不能只终止父进程而留下孤儿子进程。

后续版本再加入网络、HTTP、服务管理和代理专用探测器。

---

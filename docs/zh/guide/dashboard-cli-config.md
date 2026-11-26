---
title: Dashboard、CLI 与配置
---

# 9. Web Dashboard 与 CLI

## 9.1 Dashboard 默认关闭

Dashboard 是可选组件，默认关闭；未启用时零监听端口、零 HTTP 代码路径。

已实现、默认关闭（`enabled` 默认 false，`crates/custos-config/src/dashboard.rs:12-19`）；监听器仅在 enabled 时于启动阶段绑定（`crates/custos-cli/src/main.rs:199-200`）：

```toml
[dashboard]
enabled = false
listen = "127.0.0.1:8080"
```

启用前提与验收约束：

1. 关闭时不得监听任何端口。
2. Dashboard 不可用不得影响核心引擎运行。
3. Dashboard 不得绕过 Core Engine 直接执行恢复。
4. 高风险操作仍必须走统一的授权与审计。
5. 传输安全是尚未完成的一端：端点只读且没有认证、没有 CSRF token，目前唯一的保护是回环绑定拒绝——非回环的 `listen` 地址会被直接拒掉（`crates/custos-dashboard/src/lib.rs:39-53`）。远程访问需经本地反向代理提供认证。认证与 CSRF 随工具箱集中硬化（ADR-0015）仍待做。

## 9.2 CLI 命令

CLI 是 P0/P1 的操作面（已实现）：

```text
custos validate <CONFIG>           解析并校验配置（拒绝重复 ID、悬空引用、依赖环）
custos init <TYPE> --id <ID> [--out PATH] [--force]
                                   为某资源类型生成完整配置模板
                                   （占位值；使用前先编辑，再 validate）
custos start <CONFIG> [OPTIONS]    运行引擎循环（--db PATH、--interval MS、--once、--strict-config）
custos status [--db PATH] [--json]     存储计数与最近审计事件
custos runs [--db PATH] [--json]       最近恢复任务（新→旧）
custos incidents [--db PATH] [--json]  事件（incident）历史
custos notify status [--db PATH]   通知队列五状态计数与最近条目
custos notify retry <ID> [--db PATH]  手动重排一条死信通知
custos resources <CONFIG>          列出配置的资源
custos graph <CONFIG>              依赖图（拓扑序）
```

`start` 启动时先做崩溃恢复（未终态运行标记 outcome-unknown，绝不盲目重试），
再进入探测/恢复 tick 循环；通知引擎随进程接线（事件入队 + 后台投递，断网补发）。

**配置 last-known-good 回退**（2026-11-22 用户裁定：验证不过不许切换重启）：每次成功 `start` 会把通过校验的配置文本原子写入同目录 `<CONFIG>.last-good` 快照。运行时配置校验失败而快照可用时，引擎**在旧配置下继续运行并大声告警**，而不是退出——坏编辑绝不能让受监督机器从「有人看着」变成「无人值守、没信号」。快照只在干净加载后写入，坏配置不可能污染它。`--strict-config` 恢复 fail-fast（不回退）。`custos validate` 仍是纯检查器，从不触碰快照。

三个只读历史命令（`status` / `runs` / `incidents`）的 `--json` 把同样的行输出为单个机器可读 JSON 文档——它就是 LuCI 状态面板的数据源。

---

# 10. 配置规范

## 10.1 基本原则

配置必须：

- 可验证。
- 可版本化。
- 能提供明确错误路径。
- 不允许未知字段被静默解释成有效配置。
- 支持安全的凭据引用。
- 支持资源之间的显式依赖。
- 支持资源级恢复策略。
- 支持通知规则。全局恢复预算在 v1 里是引擎内置默认值，不是配置键（`crates/custos-engine/src/budget.rs:9-25`）。

首版采用 TOML（单一主格式，`version = 1`）。

## 10.2 示例：一个接到插件上的资源

下文的形态即已实现的 TOML schema。插件进程声明在 `[[plugins]]` 下；资源用 `plugin`（可选再加 `probe_operation`）引用其中一个，恢复计划在计划的 action 里用 `plugin` + `operation_id` 分发插件操作。下文 YAML 形态仅作示意——正式、经过校验的形态是 TOML；见 [docs/guide/plugins.md](plugins.md) 与仓库根目录的 `examples/openclash-failover.toml`。

```toml
version = 1

[[plugins]]
name = "mihomo"
binary = "/usr/local/bin/mihomo-plugin"
request_timeout_ms = 10000

[plugins.env]
MIHOMO_API = "http://127.0.0.1:9090"
MIHOMO_GROUP = "CHANGE_ME"

[[resources]]
id = "proxy.egress"
type = "mihomo"
plugin = "mihomo"
probe_operation = "mihomo.probe"
```

注意：

1. 代理健康检查区分代理认证失败（407 → `proxy_authentication_failed`）、连接失败（`proxy_connection_failed`）与目标服务异常（`remote_endpoint_unreachable`）——内置代理探测负责分类，计划阶梯按这些码选择。
2. 如果代理不是 Tunnel 的必要依赖，就不得声明为 Required。
3. 插件接线在加载时校验：未知插件名、无 `plugin` 的 `probe_operation`、以及 argv 与插件操作混用的动作形式，都是启动错误，绝不留到运行期才炸。
4. 恢复动作必须与插件声明的能力匹配。
5. 节点切换上，插件共用一个客户端（`plugins/clash-api`），其选择策略会过滤订阅信息节点——剩余流量/套餐到期这类成员延迟数字健康，但它们不是代理。

## 10.3 配置加载过程

1. 解析配置。
2. 验证版本。
3. 验证结构。
4. 验证资源 ID。
5. 验证依赖引用。
6. 验证依赖图。
7. 验证恢复动作引用。
8. 验证插件能力。（已交付：加载器校验 `plugin` 指向已声明插件、探测动作的操作 id 非空、argv 与插件操作不混用，`crates/custos-config/src/build.rs`；运行期能力发现由插件的 `describe_capabilities` 完成。）
9. 验证权限与风险策略。
10. 验证通知渠道配置。
11. 验证数据库迁移兼容性。（计划中：迁移在 store 打开时执行，`crates/custos-store/src/store/lifecycle.rs:39`；配置加载器不做迁移检查。）
12. 创建或更新运行时资源注册表。

配置验证失败时必须指出具体字段和原因。

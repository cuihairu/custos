---
title: 插件架构
---

# 4. 插件架构

## 4.1 插件的职责

插件负责封装具体资源或能力，而不是实现一套独立的自愈系统。

插件可以提供：

- Resource Provider。
- Probe Provider。
- Diagnosis Provider。
- Recovery Action Provider。
- Verification Provider。
- Notification Channel Provider。

一个插件可以提供多种能力。

例如 OpenClash 插件可以提供：

- OpenClash 服务资源。
- 代理端口探测。
- 代理认证验证。
- 节点状态诊断。
- 节点切换动作。
- 服务重启动作。
- 节点恢复验证。

Core Engine 负责判断何时调用这些能力以及是否允许执行。

## 4.2 插件执行模型

第一版使用：

**Rust Core + 外部可执行插件 + JSON-RPC 2.0 over stdio。**

优势：

- 插件崩溃不会直接导致核心进程崩溃。
- 插件可以使用不同语言实现。
- 不需要立即设计稳定的 Rust 动态库 ABI。
- 插件协议可以独立版本化。
- 有利于隔离插件生命周期和超时。

但需要明确：外部进程并不自动构成安全沙箱。插件仍然需要最小权限、受控执行目录、受控环境变量和明确的操作授权。

## 4.3 插件协议

第一版至少需要：

```text
initialize
describe_capabilities
probe
diagnose
list_recovery_actions
execute_recovery_action
verify
shutdown
```

这些是逻辑操作名称，最终应映射为有版本约束的 JSON-RPC 方法。

请求必须具有：

- JSON-RPC ID。
- 协议版本。
- 方法名称。
- 参数。
- 请求超时。
- 关联的 Resource ID。
- 必要时关联的 RecoveryRun ID。

恢复操作必须使用显式操作 ID，并携带参数化输入。禁止把任意用户输入直接拼接到 Shell 命令中。

## 4.4 stdio 的工程要求

Windows 子进程 stdio 管道可能因为缓冲区被写满而阻塞。实现时必须：

- 独立、持续读取 stdout 和 stderr。
- stdout 只用于协议消息。
- stderr 用于插件诊断日志。
- 限制单条消息大小。
- 限制总缓冲区大小。
- 支持分块读取。
- 处理不完整的消息。
- 处理插件提前退出。
- 处理无响应和超时。
- 支持请求取消。
- 对异常 JSON 和未知方法返回明确错误。
- 禁止将 stderr 原样作为协议响应解析。

建议第一版将单条协议消息限制在 1 MiB 以内，具体值作为可配置的安全上限，并对大型证据采用摘要、截断或分块方式处理。

## 4.5 插件生命周期

建议采用长生命周期插件进程：

1. Custos 启动。
2. 启动已配置的插件。
3. 完成协议握手。
4. 获取能力声明。
5. 验证协议兼容性。
6. 接受请求。
7. 监测插件健康状态。
8. 插件崩溃时根据策略重启。
9. 插件无法恢复时，将相关资源标记为无法有效探测或操作。
10. 记录插件故障并通知。

插件不应因为每次探测都重新创建进程而产生过高开销。

## 4.6 能力权限

每项能力应声明：

- 只读或修改状态。
- 所需权限。
- 影响的资源范围。
- 是否会中断网络。
- 是否会中断远程管理。
- 是否支持取消。
- 是否幂等。
- 是否支持验证。
- 可能的失败状态。
- 是否有补偿或回滚方法。

Core Engine 根据配置和策略决定是否执行。

不能因为插件声明了某个动作，就自动允许它执行。

---

## 插件口径补充（用户裁定 2026-10-29）

- **一个插件 = 一种资源的完整监控与处理逻辑**：探测、诊断、恢复动作、验证闭环在同一个插件内完成，不留半截。
- **插件经资源依赖图组合**：依赖边连接的是资源，不是插件（`Dependency` 携带 `ResourceId`，`crates/custos-model/src/dependency.rs:56-69`）；插件随其资源进入依赖图，由 Core Engine 统一解析。
- **使用方只需简单配置即可启用**：声明资源类型与参数，不写胶水代码；配置加载遵循 §10 的配置规范。实现现状（2026-11-26 更正）：schema 已交付 `[[plugins]]` 声明（`custos-config/src/build.rs:39`）与资源级 `plugin` / `probe_operation` 键（`schema.rs:217`），装配时会拉起插件 supervisor（`crates/custos-core/src/assemble.rs:75`）；已交付的插件二进制（mihomo、cloudflared、openclash、sunlogin、network-adapter）均可经配置接线启用（§5.1–5.9）。

## 集成基础能力与 Python 脚本扩展（用户裁定 2026-10-29）

- **集成基础能力**：核心引擎内置常见资源（服务、进程、磁盘、网络连通性等）的基础适配，开箱即用——基础场景不需要任何插件开发。
- **Python 脚本扩展（双向）**：
  - 出向：核心功能经 PyO3 绑定可被 Python 脚本调用；
  - 入向：自定义探测与恢复动作可以用 Python 脚本编写，由核心按插件契约加载执行（超时、权限、预算同插件口径）。

---

## 5. 配置接线（2026-11-15 交付）

上文各节描述契约；本节记录把插件接进部署的配置键——全部已实现并有测试。

### 5.1 声明插件进程（`[[plugins]]`）

```toml
[[plugins]]
name = "openclash"                      # 注册名，下文按名引用
binary = "/usr/local/bin/openclash-plugin"
request_timeout_ms = 10000              # 单次 RPC 截止（默认 30000）

[plugins.env]                           # 干净环境 + 恰好这些条目
OPENCLASH_API = "http://127.0.0.1:9090"
OPENCLASH_SECRET = "CHANGE_ME"
OPENCLASH_GROUP = "CHANGE_ME"
```

校验（配置加载即门禁）：name 唯一且非空、binary 路径非空、`request_timeout_ms` >= 1、未知字段报错（dashboard-cli-config §10.1）。

### 5.2 插件承载的探测（`plugin` + `probe_operation`）

声明了 `plugin` 的资源由该插件的 `probe_provider` 能力探测，替代内置探测：

```toml
[[resources]]
id = "openclash"
type = "openclash"
plugin = "openclash"                    # 必须指向一个 [[plugins]] 条目
probe_operation = "openclash.probe"     # 默认 "probe"
```

只写 `probe_operation` 而无 `plugin` 是校验错误。运行期，降级的 supervisor（插件崩溃或无法拉起）表现为 `Unreachable`，`diagnose()` 将其映射为 `ProbeTimeout`——管理面故障；而健康插件给出的 unhealthy 判定是插件对其后端的判断（`UnknownFailure`）。

### 5.3 恢复动作（`plugin` + `operation_id`）

动作恰为两种形式之一——argv 或插件操作，混用是校验错误：

```toml
[resources.recovery_policy.plans.action]
plugin = "openclash"
operation_id = "openclash.switch_best_node"
```

```toml
[resources.recovery_policy.plans.action]
program = "/etc/init.d/openclash"
args = ["restart"]
```

插件名与操作 id 在加载时校验（未知插件、空操作 id → 启动报错，绝不留给运行期惊吓）。

### 5.4 进程模型：干净环境、预算、停机

- **惰性拉起**：插件进程在首次探测或动作调用时启动，而非配置加载时。
- **干净环境**：子进程绝不继承宿主环境；只拿到声明的 `[plugins.env]` 条目加一份固定的 OS 基线（`PATH`，Windows 上另有 `SystemRoot`/`SystemDrive`/`ComSpec`/`TEMP`/`TMP`——缺了它们 `tasklist` 这类 Windows 系统工具加载不了自己的 DLL；由向日葵真机实测发现）。基线值存在时取宿主环境，否则取标准默认；声明条目覆盖基线。凭据经 §5.1 显式传给插件，而非靠继承泄漏。
- **按请求预算**：`request_timeout_ms` 约束每个 RPC；watchdog 击杀卡死的插件，supervisor 按其重启策略重启（§4.5）。
- **停机**：运行时装配体（`Assembled`，`crates/custos-core/src/assemble.rs`）持有全部 supervisor，drop 时统一停机——不留孤儿进程。

### 5.5 分发：路由与结果映射

`PluginRoutingExecutor`（`crates/custos-engine/src/executor.rs`）按动作形式路由：插件操作送往具名 supervisor（§4.3 协议），argv 动作送往内置 exec 执行器。RPC 结果映射为动作结果：

| 插件应答 | 动作结果 | 恢复语义 |
|---|---|---|
| `succeeded: true` | `Succeeded` | 计划进入验证 |
| `succeeded: false` | `Failed` | 插件对其后端的判定——阶梯内可重试 |
| supervisor `Unavailable` | `OutcomeUnknown` | 是否执行过不可知——绝不盲目重放（§6.6） |
| RPC 超时（`-32001`） | `TimedOut` | 计一次已耗尝试 |
| `-32601` 未知操作 / 应答畸形 | `NotStarted` | 接线错误——对该计划永久（C21） |

### 5.6 共享的 clash/mihomo 客户端（`plugins/clash-api`）

OpenClash 在 OpenWrt 上跑 clash 内核；其 external controller 与 mihomo 是同一套 REST 面。两个出厂代理插件都建在一个客户端 crate 上，它负责：

- REST 调用（`/version`、`/proxies/{group}`、`/group/{group}/delay`、`PUT /proxies/{group}`），bearer 认证，5 s 客户端预算；
- 组名的百分号编码（`♻️ 自动选择`、`三毛机场`）；
- **节点选择策略**：`is_selectable_node` / `real_candidates` / `best_by_delay`。订阅提供方会注入信息成员——`剩余流量：533.08 GB`、`套餐到期：2027-01-16`、`官网 …`——它们带着健康的延迟数字但不是代理；选上即断全部流量（2026-11-15 真实场景修复，实测：信息成员 150 ms 而真节点 51–106 ms）。每个插件的每个 picker 都过这道过滤，`DIRECT`/`REJECT` 与信息节点永远不会成为切换目标。

### 5.7 完整示例

`examples/openclash-failover.toml`（`custos validate` 通过）：`wan`（仅探测——线路死了不许翻动代理节点）、`openclash`（插件探测 + `allow_high_risk` 后的高危重启计划）、`google` / `github`（经 mixed port 的内置代理探测，计划分发 `openclash.switch_best_node`；代理认证失败故意无计划——切节点治不了错凭据）。

### 5.8 封装闭源客户端：向日葵插件

向日葵（Sunlogin）没有官方 API。封装原则沿用 clash-api 已验证的那条：**事实 API 就是产品自家 UI 在用的那个面**——且该面随版本变化，所以插件按野外存在的代际建模：

- **AweSun 16.x（现行，2026-11-16 真机实测）**：客户端回环监听（127.0.0.1:49679/49680）是 GUI↔服务*私有 IPC*——收 TCP 但不说 HTTP（curl 应答 code 000）。可观测健康模型分层，探测逐层点名：
  1. 服务 RUNNING（`sc query AweSunService`；Linux 为 systemd unit），
  2. 客户端进程存活（`tasklist`：`AweSun.exe`、`awesun_guard.exe`），
  3. **云会话 ESTABLISHED**（`netstat -ano`：客户端 pid → :443）——这才是健康信号：意味着这台机器*此刻*可从外部远程控制，
  4. `config.ini` 事实（`full_version`、`account` = 绑定），尽力而为。
- **旧 11.x 安装**：回环 HTTP 状态面，随机高端口上的 `/cgi-bin/rpc?action=login-type`（版本/绑定/平台），端口写进客户端日志。保留为可选增强：把 `SUNLOGIN_API` / `SUNLOGIN_PORT` 指过去；失败非致命。

`plugins/sunlogin` 在这些层之上实现只读观察加两个恢复动作：

- `sunlogin.probe`——上述分层阶梯；`healthy` = 服务 ∧ 进程 ∧ 云会话。观察点名失败层（绝不裸报 false），上报的版本兼任过期安装审计。
- `sunlogin.restart`——服务管理器重启（Windows `sc stop`/`start`，Linux `systemctl restart`）；声明 `interrupts_remote_admin`，因为重启远程访问客户端会杀掉在线会话。`sc start` 在 SCM 受理请求的瞬间就返回，所以 Windows 路径随后轮询 `sc query` 直到服务真正 RUNNING（45 s 预算）——真机实测曾见 START_PENDING 的"成功"抢在计划验证前造成假 Escalation。`SUNLOGIN_SERVICE` **钉死恰好一个服务**（无回退——错误回退绝不能被重启）；未设则尝试平台默认加备选。
- `sunlogin.relogin`——UI 自动化重登：为能力清单完整而声明，未实现；仅 Windows、脆弱（对话框/更新会打断选择器），必须作为末位计划留在 `allow_high_risk` 之后。

安全契约：只读观察面、仅回环；插件绝不触碰历史命令端点（`/check`——CNVD-2022-10270 RCE 面，上游已修）。见 `examples/sunlogin-plugin.toml`；解析器按真机 fixture 单测，`#[ignore]` 的实机测试（`cargo test -p custos-sunlogin-plugin -- --ignored`）探测真实机器。

**真机部署（iMac，Windows）**：supervisor 以 SYSTEM 计划任务运行（重启动作需要服务控制权；开机触发；`RestartCount 999` @ 1 min 崩溃自拉；经 `powershell.exe -Command "… *>> log"` 包装记日志）。两个 Windows 坑现场抓到：`-ExecutionTimeLimit ([TimeSpan]::Zero)`（"无限制"，PT0S）会让任务计划程序一起动就杀任务（exit 1，日志永远写不出）——要设有限的长时间；`-Force` 重注册任务不会杀旧实例，重启链会短暂新旧并行——重装前先拆（disable、kill、unregister）。端到端验收：注入 `sc stop` 故障，守护进程无人值守 ~4 s 检出并治愈（`recovery.started` → `action_finished` → `recovery.succeeded`，审计轨迹完整）。2026-11-21 复验（验收场景 1）：SSH 新发的 `sc stop` 在下一个 5 s tick 检出、~12 s 治愈（`run-18`：`recovery run started` → attempt `Succeeded` → `recovery succeeded and verified` → `Recovered`）；期间 runs 表始终只有 sunlogin 行——线上配置里没有其他资源，被触碰的只有向日葵，SSH 管理通路全程未断。

### 5.9 network-adapter 插件：自锁死类

启用/禁用网络适配器是 custos 封装的最危险恢复类：承载管理连接的适配器正是 disable 动作会拿走的东西。`plugins/network-adapter` 因此把控制 API 与硬编码守卫成对交付：

- **API 选型**：`netsh interface set interface "<name>" admin=enable|disable`——Windows 设置 UI 驱动的那个面（Linux：`ip link set dev <if> up|down`）。PowerShell 的 `Enable-NetAdapter`/`Disable-NetAdapter` 在 iMac 上实测**静默无效**（它们走 CIM/`MSFT_NetAdapter` 路；netsh 走 SetupDi/NDIS）——再次提醒：事实 API 是机器上真能用的那个，不是文档宣传的那个。非 ASCII 接口名（`以太网 2`）经插件 env（`[plugins.env]`，UTF-8 → UTF-16 round-trip）传输无碍；优先用名而非 ifIndex。
- **寻址**：优先稳定的接口名。Windows ifIndex（`if:19`、`args.if_index`）可接受但**跨重启会变**（2026-11-22 iMac 实测：Ethernet 13→16、闲置 10G 19→22——钉死的 `if:13` 重启后无声指向空处）。`if:N` → 别名解析本身走 `netsh int ipv4 show interfaces` 的 Idx 表，而非 PowerShell `Get-NetAdapter`：SYSTEM 上下文里 CIM 查询**整个漏查**（与上面静默无效的开关同源）；netsh 在探测能跑的任何地方都能跑。无 IPv4 绑定的接口不在这张表里。
- `network.probe`——适配器清单含 admin/连接态（`netsh interface show interface`，zh-CN 与 en-US 表头；Linux `ip -o link show`）。监督目标经 `NETADAPTER_INTERFACE`（默认：ANY 已连接接口）。
- `network.enable`——对治不了自身的故障的恢复动作：被禁的适配器远程同样不可达，所以 enable 需要接口冗余（以太网 down 时 Wi-Fi up）或带外通道。声明 `interrupts_network` + `interrupts_remote_admin`。
- `network.disable`——有序守卫：(1) `NETADAPTER_PROTECTED` 上的名字无条件拒绝，(2) CONNECTED 接口除非 `NETADAPTER_ALLOW_ACTIVE=1` 否则拒绝——禁掉承载适配器会同时切断 SSH/向日葵。`compensation_operation: network.enable`。不接进任何示例计划；仅限有意使用。

iMac 实机验收：闲置 10G 适配器（当时 ifIndex 19）disable → `AdminStatus Down/Disabled` → enable → `Up/Disconnected`，全程经 netsh，SSH 通路（当时 ifIndex 13）未受扰动；见 `examples/network-adapter.toml`。

真机注记（2026-11-21，iMac）：WLAN 适配器可能 admin-enabled 而 radio-off。两块 Wi-Fi 适配器（Broadcom 802.11ac `WLAN`、Intel AX201 `WLAN 2`）均报硬件开/软件关；软无线电关着时 `netsh wlan show networks` 列出 0 个 SSID、`netsh wlan connect` 报错 2150899714（"无线局域网接口电源关闭"）。admin disable/enable bounce **不会**把无线电开回来——软无线电开关在 netsh 接口面之外（OS Wi-Fi 设置/厂商工具），所以任何 Wi-Fi 重连恢复都必须假设无线电已开。对验收场景 2（Wi-Fi 断开→先恢复网络）的影响：主机有适配器和已存 profile，但无线电刻意关着——引擎没有（也不该猜）拨动软无线电开关的原语；阻塞点移到"无线电开或有活动 Wi-Fi 的主机"。

后续（同日）：随后实测了每一条程序化开无线电面，无一能及。Radio Management API——飞行模式那个面——在本 Windows 安装上缺席（`srmapi.dll` 缺失，`DllNotFoundException` HRESULT 0x800701C0）；设备节点电源循环（`pnputil /disable-device` + `/enable-device`，Broadcom `VEN_14E4&DEV_43A0` 与 Intel `VEN_8086&DEV_06F0` 双卡）在 PnP 层面成功——设备状态 OK——软件无线电依然关。软无线电状态凌驾于这些面之上（WLAN AutoConfig 按适配器跟踪它），只有 OS Wi-Fi 设置 UI 或厂商工具能够到它；network-adapter 插件钉定接口的健康判据是 `admin_enabled && connected`（`plugins/network-adapter/src/main.rs`），所以 radio 关的适配器永远过不了 `status_is = "healthy"` 验证，场景无法绕开无线电。

第五条面，同样负结果（2026-11-22，iMac，用户授权）：`WlanSvc` 服务重启（`net stop WlanSvc` + `net start WlanSvc`，确认 `STATE 4 RUNNING`）强制 WLAN AutoConfig 栈重枚举并重读无线电状态，之后两卡仍报硬件开/软件关，`netsh wlan show networks` 仍答"无线局域网接口电源关闭"。软无线电状态因此也活过 WlanSvc 重启——它不属于服务，而属于 OS 设置/厂商层。五条程序化面至此全部实测无效（netsh admin bounce、`Enable-NetAdapter` CIM、Radio Management API、`pnputil` 设备电源循环、WlanSvc 重启），验收场景 2 维持阻塞在人来拨无线电（OS Wi-Fi 设置 UI/厂商工具）；引擎仍然没有能拨的原语，也不该猜一个。

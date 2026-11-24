# C-SET-UX-06 只读面钉 · 验收记录（2026-11-25）

状态：**已验收，todo 已勾**（真机视觉项归 C-SET-UX-07）。本单范围：LuCI 状态页标明「运行与配置来源」；dashboard 只读红线为既有实现（本单未动 `crates/custos-dashboard`），本记录补其活体证据。

## 1. LuCI 状态页 = 运行与配置来源（本单改动）

- `api_overview`（`packaging/openwrt/usr/lib/lua/luci/controller/custos.lua`）新增 `out.conf`：TOML 配置源路径按 UCI `custos.main.conf` 下发（缺省回落 `/etc/custos/custos.toml`），不再由页面写死；`out.db` 既有。
- 状态模板（`packaging/openwrt/usr/lib/lua/luci/view/custos/status.htm`）：「TOML 配置」行从硬编码改为 `id="custos-conf"` 并由 `j.conf` 填充；「procd 状态」行升级为运行态徽标 `● 运行中`（绿）/ `■ 已停止`（红）。
- 页面 JS `node --check` 语法过；真机浏览器视觉（窄窗口/徽标准星）待 C-SET-UX-07 装机亲验。

## 2. Dashboard 只读红线（既有实现，本单验证）

代码门链（引用为证，未改动）：

| 红线 | 位置 |
|---|---|
| 默认关闭 | `crates/custos-config/src/dashboard.rs:19`（`Default: enabled: false`，默认监听 `127.0.0.1:8080`） |
| 启用门 | `crates/custos-cli/src/main.rs:277`（`config.dashboard.is_enabled()` 为假则 `DashboardServer::bind` 永不调用、无 HTTP 代码路径） |
| 仅回环 | `crates/custos-dashboard/src/lib.rs:44-49`（bind 前拒非回环地址：refusing to expose non-loopback） |
| 只读 | 端点全为只读查询，`/api/config` 返回 `read_only: true`（单测 `disabled_config_binds_nothing` 等，todo:241/255 已录） |

### 活体验证（本机 `target/debug/custos`，隔离配置 /tmp，版本随本 commit）

**默认关闭 = 零监听**（监听集合差分法；`ss -ltn` 前后对比）：

```
$ custos start off.toml --db a.db --interval 1000 &   # off.toml 无 [dashboard] 段
$ diff before.txt during.txt && echo "OFF: zero new listeners"
OFF: zero new listeners
```

**启用后 = 回环只读**（空闲端口 18123 正向对照）：

```
[dashboard]
enabled = true
listen = "127.0.0.1:18123"

$ curl -sS http://127.0.0.1:18123/        → <title>Custos 监控面板</title>
$ curl -sS http://127.0.0.1:18123/api/config
{"attempts":0,"audit_events":0,"read_only":true,"runs":0,"verifications":0}
```

**占用端口 = fail-fast**（本机 8080 被无关服务占用时）：

```
$ custos start (dashboard enabled, 8080 被占)
error: dashboard bind failed: Address already in use (os error 98)   # 进程退出，不静默降级
```

## 3. 环境备注（重要）

任务单给的判据「curl 127.0.0.1:8080 拒绝连接即通过」**在本开发机上不可用**：本机 `127.0.0.1:8080` 被无关的 meshlet console 长期占用（curl 返回的是 meshlet 页面，非 custos）。故本验收改用零差分法（OFF）+ 空闲端口正向对照（ON），两组证据都比原判据更强。**装机时（C-SET-UX-07）需在真机上重验 8080 空闲性**——OpenWrt 标准固件 LuCI 占 :80/:443，8080 通常空闲，但以 `netstat -ltn | grep 8080` 先证后测。

# C-SET-UX-03 实际投递验收记录（本地日志 / Herald）

> 日期：2026-11-27。承接 `setux-07-isolated-acceptance.md` §C-SET-UX-03 的「余项」——配置面已全验，本记录补齐「对照 UCI、实际投递和 TOML」中的实际投递对照（Telegram 投递除外，仍待用户 bot token）。
> 方法：host 与公网实例运行 `cargo build --release`（`917ead4` 后工作树）产物 `custos` 二进制，经 `CUSTOS_NOTIFY_*` 环境变量走引擎 `env_overlay` 路径——与设备上 init.d `notify_pairs()` 下发、LuCI 页面提交的是同一条 UCI→env→引擎契约（`7f273e3` 已在隔离实例核对 env 变量逐字一致）。

## 1. 有效本地日志（env 覆盖路径，host）

配置：无 `[notification]` 段的最小 TOML（process 资源 + 指向已退出 PID 的 `pid_file`）；env `CUSTOS_NOTIFY_LOCAL_LOG=1`；`--interval 500` 跑 8 s。

结果：首个 tick 即触发 `resource.unhealthy`（critical），`local-log` 通道真实写出行级审计 JSON（tracing ERROR 级）：

```json
{"event_id":"evt-1","event_type":"resource.unhealthy","severity":"critical","resource_id":"acceptance-probe","occurred_at_unix_ms":1795756302482,"summary":"resource is unhealthy: pid 42751 exited","details":{}}
```

队列 `notify status` 为 0 条——local-log 是同步直写通道，不入持久队列，与 `engine.rs` `has_local_log`/`ChannelConfig::LocalLog` 实现一致。TOML 侧对照：同配置去掉 env 变量后引擎按 TOML（无 `[notification]`）启动，无任何通知规则行（启动横幅 `0 notification rule(s)` vs 覆盖时 `1 notification rule(s)`）。

## 2. Herald 地址有效与错误（四路径全真实）

Herald env 覆盖硬编码 SSRF `default_blocking`（loopback/私网拒绝、无 allowlist），故有效路径须公网目标：接收端为公网实例上的临时 HTTP responder（端口 18081，跑完即清；实例仅限本验收，DNAT hairpin 规则与全部文件已清理）。

| 路径 | 输入 | 结果 |
|---|---|---|
| 有效+对端正确 | `CUSTOS_NOTIFY_HERALD_URL=http://106.14.216.177:18081`，对端回 `{"code":0}` | **delivered**。队列 `1 delivered`；接收端捕获 POST `/api/v1/apps/custos-ux03/dispatch` 完整 dispatch 信封（见下）；引擎 trace `notification delivery pass delivered=1` |
| 有效+对端违约 | 同 URL，对端回 HTTP 200 `{}` | **Rejected→dead_letter**（终态，不重试）：`herald answered HTTP 200 without an acceptance envelope`——2xx 无 `{"code":0}` 属契约错配（C4），与 `herald.rs classify_response` 注记一致 |
| 地址不可达 | 同 URL 指向无监听端口（安全组拦截） | **retry_scheduled**（1 次尝试后）：`Transport: herald transport error: timeout: global`；引擎继续运行，通道错误不致命（§8.1） |
| 非法 scheme | `CUSTOS_NOTIFY_HERALD_URL=ftp://…` | `custos validate` **rc=1**：`notification.env.CUSTOS_NOTIFY_HERALD_URL: herald \`base_url\` must be http or https`——env_overlay 整体拒绝，配置不装载，引擎拒绝启动 |

delivered 信封（severity→urgency、fault→state=down 映射真实生效）：

```json
{"audiences":["log"],"body":"resource is unhealthy: pid 42751 exited","category":"alerts","dedup_key":"resource.unhealthy:acceptance-probe","event_id":"evt-1","state":"down","title":"[custos] resource.unhealthy: resource is unhealthy: pid 42751 exited","urgency":"critical"}
```

## 3. 编辑器补强（随本验收落地）

本批发现并修复：LuCI 页 `herald_url` 此前仅 `no_space` 校验，非法 scheme 可入库；而引擎侧是**整体拒绝**（`build.rs:517` `?` 传播，且 last-known-good 快照同一进程内装载会被同一批 `CUSTOS_NOTIFY_*` 再度毒化，回退也失败）→ 保存即引擎起不来、页面仍显示「LuCI 在生效」。修复 `917ead4`：`settings.herald_url_error` 逐字镜像 env_overlay 规则（`^(.-)://` 取 scheme、大小写不敏感 http/https），编辑器层即拒（no_space+scheme 链），settings-test 55 断言 + `luac -p` 绿。上表「非法 scheme」路径即该修复的后端契约活体证据。

同批再补强：编辑器示例曾是 `http://127.0.0.1:8080`——env_overlay 的 SSRF 为硬编码 `default_blocking()`（loopback/私网拒绝、空 allowlist），该地址能入库却让通道构建失败（引擎只 log+跳过通道，事件 dead-letter），页面却仍称 LuCI 生效，比非法 scheme 更隐蔽。修复 `db5edc4`：`settings.herald_url_error` 现同时镜像**两条**目标规则：scheme 校验之外，解析 authority（剥 userinfo、剥端口、剥 `[...]`）后逐字镜像字面 IP 判定——0.0.0.0、10/8、172.16/12、192.168/16、169.254/16、CGNAT 100.64/10、127/8、localhost、IPv6 ::/::1、fc00::/7、fe80::/10、IPv4-mapped；**非字面主机名留给引擎 DNS 解析后判定**（解析权在 `webhook_ssrf::validate_target`）。settings-test 55→80 断言（新增 SSRF 字面判定 24 例） + `luac -p` 绿；CBI 描述改为公网示例并提示内网 Herald 需在 TOML 配 ssrf allowlist。

第三项补强 `6634737`：任务「怎么做」要求同时明确当前与**待应用**来源，此前只显示当前生效来源。通知页现有两行 DummyValue——「当前谁在生效」读已提交 UCI，「应用后谁生效」读同一次提交的表单字段（勾选框用 cbid 选中态与 cbe 存在标记区分「显式未勾」和「未提交」；文本字段直读），两者共用 `settings.effective_source` 判定，令牌值只参与判空不回显；未提交表单时第二行显示「无待应用改动」。settings-test 80→85 断言（form-over-stored 合并与显式空令牌覆盖存量令牌）全绿。

## 4. 余项

Telegram 真实投递对照：唯一未完成验收行，仍待用户 bot token + chat id（代理凭据已从路由器 UCI 取回并实测 `api.telegram.org` 经 7890 可达，302；验收命令见 todo.md C-SET-UX-03 注记，凭据不落仓库）。

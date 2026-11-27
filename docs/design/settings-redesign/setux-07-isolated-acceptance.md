# C-SET-UX-07 OpenWrt 隔离实例验收（2026-11-26）

状态：**已验收（C-SET-UX-01~04 闭项证据），07 保留两项用户侧余项**（Telegram 真投递对照需真实 bot token；「新用户独立完成」journey 属用户评估）。任务单标题允许「真机或隔离实例」，本记录在 qemu/KVM 隔离实例完成全部可自动化项。

## 1. 环境与版本

| 项 | 值 |
|---|---|
| 隔离实例 | qemu-system-x86_64（KVM），512 MB，e1000，hostfwd SSH/HTTP；OpenWrt **24.10.8** (r29233-443ec4032a) |
| LuCI | openwrt-24.10 branch (26.301.70438~aa4865e)，**Lua compatibility mode active**（luci-lua-runtime + luci-compat） |
| 包 | custos / luci-app-custos / app-meta-custos **0.1.0-1**（`build-ipk.sh` 从仓库状态构建，musl 静态二进制 x86_64） |
| 浏览器 | Playwright + Chromium，760 px（视觉）与 1280 px（流程）两种窗宽 |
| 接入 | LuCI :80 经 hostfwd（Chromium 拒绝 10080 不安全端口时用本机 TCP relay 绕行）；SSH hostfwd 做 CLI/procd 断言 |

**最终镜像复验**：修复后 ipk 重打 → imagebuilder 重烤 → **全新启动**（零手工步骤，仅按默认禁用策略 `uci set custos.main.enabled=1`）复核：loopback 存在、三个 Lua 运行时依赖随 Depends 装入、四条路由 200（`api/resources`/服务页/资源页/通知页）、写链 UCI 5000→2000→1500 一次提交生效、引擎 argv 跟随（`pidof` + `/proc/<pid>/cmdline`）。

## 2. 各任务单验收项

### C-SET-UX-01（服务页拆分 / UCI 毫秒差分）

- 表单填 `2` 秒 → Save & Apply 一次提交内：UCI `custos.main.interval` 2000，procd 实例 argv `--interval 2000`（`/proc/<pid>/cmdline` 实读）。恢复 1.5 秒 → UCI 1500。截屏 `shots-07/svc_01c_filled.png` / `svc_01c_applied.png`。
- 纯 Save（不 Apply）：web 会话 uci staging 仅含 `set main.interval 3500`，已提交配置与引擎不动（in-page `/ubus/` `uci changes` 断言）。
- 760 px 服务页：`02_service_760.png`。

### C-SET-UX-02（资源表 / 四态健康 / 草稿守卫）

四态全部真机呈现（截屏 760 px）：

| 状态 | 触发 | 截屏 |
|---|---|---|
| 健康（绿） | 默认全部 | `02_resources_healthy_760.png` |
| 检测失败（红） | 停 dnsmasq 越过 10 s 探测间隔 | `02_resources_failing_760.png`；恢复 dnsmasq 后回健康（活体恢复证据） |
| 无观测（灰） | 引擎停 + db 移除（全新装机场景，6 行全灰） | `02_resources_unknown_760.png` |
| 等待依赖（灰常规字重） | demo-dependent 资源 depends_on=dnsmasq（上游检测失败、自身无观测） | `02_resources_waiting_760.png` |

草稿守卫（`02_editor_dirty.png` → `02_draft_guard_kept.png` → `02_draft_guard_accepted.png`）：改字段后切换资源 → `window.confirm('编辑器有未保存的更改，切换将丢弃这些更改，确定？')`；**取消** = 编辑器保持、脏值 `DIRTY-DRAFT-VALUE` 原样；**确认** = 切到「编辑资源: gateway」。

未改动提交 = noop：绿色横幅「当前没有改动，未写入」，零写入（`04_noop_unchanged.png`）。TOML 回退：行内「TOML」按钮打开原样字节块、取消不落盘（`04_raw_toml_editor.png`）。字节级保真由 tomledit 44 例与上述 round-trip 覆盖。

### C-SET-UX-03（通知页揭示生效配置）

- 半配对拒绝：仅填 token 保存 → 「与接收会话 ID 需同时填写」，生效源仍 TOML（`03_notify_baseline.png` / `03a_pair_error.png`）。
- 合法配对 + 覆盖开关：保存后生效注记切为 LuCI；引擎 env 见 `CUSTOS_NOTIFY_TELEGRAM_*`（token 掩显）；页面/API 无令牌明文（`api/resources` 响应 grep `FAKE-TOKEN|111111` = 0；password 型输入框，`03b_filled.png` / `03b_applied.png`）。
- 覆盖关闭：恢复 TOML 生效注记，init.d 不再传任何 `CUSTOS_NOTIFY_*`（03 阶段 C 断言）。
- **余项**：真实 Telegram 投递对照（需用户 bot token，无法在隔离实例完成）。

### C-SET-UX-04 / 05（预览、备份、提交、四态、失败回滚）——活体全链

`api/write` 响应日志（真实抓取，两次请求）：

```
①写入:  {"ok":true,"state":"restart_failed","md5":"aa5629…","backup":"/etc/custos/custos.toml.bak",
         "message":"配置已写入并通过校验，但服务未确认运行，请检查服务状态","needs_attention":true}
②恢复:  {"ok":true,"message":"已保存并通过 custos validate，服务已确认运行","state":"confirmed"}
```

触发方式：raw TOML 追加 `[dashboard] listen="192.168.9.9:8080"`（通过 `custos validate`、引擎启动时拒非回环 bind 退出——同时复证 06 的 dashboard 红线在引擎侧生效）。端口占用变体亦验证（第二实例占 8080 → `dashboard bind failed: Address in use (os error 98)`，见 logread）。

- 预览（字段级 diff，仅列改动）：`04_review_preview.png`；
- restart_failed UI：黄条 + 红「用备份恢复 (.bak)」入口：`04_restart_failed.png`；
- 恢复后 confirmed 绿条、live/.bak 均回到 packaged md5 `b2b494…`、procd running rc=0：`04_restored.png`；
- settle 时序：restart 失败路径 346 ms 即回（不进轮询）；正常路径整窗 5298 ms（10×0.5 s，exec_rc logger 实测）；
- procd 状态诚实性：`status_service` 以 `ubus call service list` 实例 pid 判定——运行 rc=0「running」、未启动 rc=3「inactive」（procd 默认对从未启动的服务回 rc 0，修复前说谎）。
- 8080 空闲性先证后测（06 遗留）：装机实测 `netstat -ltn | grep 8080` 为空后测默认关、开、冲突三态，全过。

## 3. 验收发现并修复的实现缺陷（全部随本记录修复）

1. **init.d status 说谎**：procd 对从未启动的服务返回 rc 0，`status_service` 需以实例 pid 判定（`etc/init.d/custos`）。
2. **BusyBox sleep 无小数**：`sleep 0.5` 即时报错，settle 轮询窗塌缩到 ~350 ms；`real_fns().sleep` 改 nixio.nanosleep（整秒+纳秒），回退整秒向上取整。
3. **Save & Apply 重启先于提交**：compat cbi 默认委托 apply，`on_commit` 在 parse 期触发而真实 uci commit 在响应后才发生 → 引擎读到旧值。两页 cbi 模型加 `m.apply_on_parse = false` 内联提交。
4. **api/resources 500**：`file_md5` 为 local 函数却在其定义前被 `api_resources` 引用（Lua 词法作用域 → 全局 nil）。定义上移至首个使用点之前。
5. **ipk 缺 Lua 运行时依赖**：`luci-app-custos` 仅 `Depends: custos`，全新装机 LuCI 路由全 404（开发机手工装过 compat 层，掩盖了问题）。改为 `Depends: custos, luci-lua-runtime, luci-compat, luci-lib-nixio`，随重烤镜像全新启动复核零手工可用。
6. **restart_failed 后恢复被结构性拒绝**：失败写入返回不含新 md5，视图基线仍是写入前值 → 「用备份恢复」必然 stale 拒绝（该入口存在的唯一场景被自己的并发守卫挡死）。修复：失败写入响应携带 `md5`，视图在 restart_failed 分支收编为新基线；`apply-test.lua` 62→**69 断言**（新增失败写入 md5 随行/按报告 md5 恢复 confirmed/旧基线仍拒绝）。

## 4. 交付面复验（重烤镜像）

`build-ipk.sh` 重打 → imagebuilder `make image` → 全新 qcow2 启动：

- loopback 由 files overlay 修复（首次烤机镜像无 `lo` 地址，致 127.0.0.1 服务全部不可用）；
- 依赖三包由 Depends 链自动装入（opkg list-installed 实证）；
- 全部路由 200 + `api/resources` 200（含 md5）；
- 写链一次提交生效（UCI 5000→2000→1500，argv 跟随）；
- `enabled` 默认 0 = 引擎不自启（属既定策略），开启即 running。

## 5. 截屏索引（shots-07/）

| 文件 | 内容 |
|---|---|
| svc_01c_filled / svc_01c_applied | 01：interval 2 s 填写与应用后 |
| 02_service_760 / 02_status_760 / 02_notify_760 | 760 px 三页 |
| 02_resources_healthy / failing / unknown / waiting _760 | 四态健康标签 |
| 02_editor_dirty / 02_draft_guard_kept / 02_draft_guard_accepted | 草稿守卫取消/确认两路 |
| 03_notify_baseline / 03a_pair_error / 03b_filled / 03b_applied | 03：半配对拒绝、合法配对生效、token 掩显 |
| 04_raw_toml_editor | 02：TOML 回退编辑（原样字节） |
| 04_noop_unchanged | 04：无改动提交 noop |
| 04_review_preview | 04：应用前核对字段级 diff |
| 04_restart_failed / 04_restored | 05：失败态与恢复 confirmed |

## 6. 未闭余项（07 保持不勾的部分）

- Telegram **真实投递**对照：隔离实例无外网 bot token；配置面（配对校验、env 下发、掩显）已全部验证；实际投递对照（本地日志/Herald 路径）已由 `setux-03-delivery-acceptance.md`（2026-11-27）补齐。
- 「新用户独立调整探测周期、找到资源依赖、说清通知来源」journey：属用户评估，本记录提供全截屏链作为材料。

## 7. 补充验收（2026-11-27，全新重烤实例：键盘 / 长路径 / 应用后刷新）

任务单「怎么验证」的三个机械项在 2026-11-26 记录中未显式覆盖，本节补齐。实例为**从仓库状态全新重烤**（imagebuilder 24.10.8 + `build-ipk.sh` 0.1.0-1 + imagebuilder 装入 luci/luci-lua-runtime/luci-compat/luci-lib-nixio，files overlay uci-defaults 仅 `enabled=1` 与 lan proto=dhcp 适配 slirp hostfwd），镜像内容经 manifest 核对（custos 三包 + compat 层 + uhttpd/ubus），Playwright + Chromium 760 px（Chromium 拒 10080 不安全端口，本机 socat relay 绕行——与 §1 同一手法）。断言输出全 7 PASS：

| 项 | 方法 | 结果 |
|---|---|---|
| 键盘 | interval 字段 `.focus()` 断言 activeElement；字段内 Tab 前进；字段内 Enter 提交 | **PASS** ×3——focusable；Tab 到相邻表单控件 `cbid.custos.main.conf`（语义相邻，非跳链接即测试通过口径）；Enter 触发表单提交导航（`07_keyboard_focus.png`） |
| 长路径 | db 填 120 字符路径保存；状态页 760 px 检查 | **PASS** ×2——长路径在状态页可见；`scrollWidth-clientWidth=0` 零横向溢出（`07_longpath_service_applied.png` / `07_longpath_status_760.png`）。**且功能层活体**：procd 引擎 argv 逐字携带长 db 路径并 running（`/proc/<pid>/cmdline` 实读），非仅显示层 |
| 应用后刷新 | interval 5→1.5 一次 Save & Apply，页面 reload 后读字段 | **PASS**——刷新后字段显 `1.5`（`07_apply_refresh_1p5.png`）；UCI 同步验证后恢复 5 |

补充：本节实例为 2026-11-27 代码状态（含 `917ead4` herald scheme 校验、scubber 系列提交后的全新构建），写链 round-trip（表单→UCI→procd argv）在新烤镜像上复验一致——01 的 UCI 毫秒差分结论在全新镜像上仍然成立。无新增实现缺陷。

排障记录（非交付面问题）：烤机镜像首次启动 hang 的根因是 OpenWrt 默认 br-lan 静态 192.168.1.1 与 qemu slirp（10.0.2.15 + hostfwd）网段不匹配，SSH/HTTP 转发皆不可达——串口截图确认系统实际启动完成（Enter 激活 console 后 root shell 正常）。files overlay 加 lan proto=dhcp uci-defaults 后全新重烤解决。离线 debugfs 直写 ext4 已挂载过（非干净卸载）的镜像会因脏日志触发 ext4 journal abort 转 read-only（`deleted inode referenced`），本节改用重烤而非离线修补。

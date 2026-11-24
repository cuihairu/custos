# C-SET-UX-05 四态确认 · 验收记录（2026-11-25）

状态：**已验收，todo 已勾**。四态路径按 `apply-contract.md` §3/§4 逐条在隔离面（注入副作用的假世界 + Lua 5.4 宿主 + Docker Lua 5.1.5〔与设备 LuCI 运行时同大版本〕）验证。procd 真机时序（重启后实际 polling 节奏）仍归 C-SET-UX-07 装机亲验。

## 验收表（contract §4「重启」行逐句核对）

| # | 场景 | 触发 | 预期态 | 断言（apply-test.lua） | 5.4 宿主 | 5.1.5 容器 |
|---|---|---|---|---|---|---|
| 1 | 正常 | 重启 rc=0，poll 即 running 且复检仍 running | `confirmed` | happy path settles to confirmed；boolean restart rc counts as success | ✅ | ✅ |
| 2 | 重启命令非零 | `restart_rc=1`（不进轮询窗口） | `restart_failed`（live=新配置、`.bak`=旧配置保留） | restart rc /= 0 is restart_failed；failed restart skips the polling window | ✅ | ✅ |
| 3 | 启动后退出 | `run_seq={true,false}`：第 1 次 poll running、其后消亡 | `restart_failed` | start-then-exit is restart_failed | ✅ | ✅ |
| 4 | 用户主动停用 | 轮询超时 + UCI `enabled=0` | `stopped_by_user`（不算失败） | timeout with UCI disabled is stopped_by_user | ✅ | ✅ |
| 5 | 慢启动 | 第 3 次 poll 起 running | `confirmed` | late start within the window still confirms | ✅ | ✅ |
| 6 | 抖动后稳定 | `run_seq={true,false,true,true}` | `confirmed` | flap that settles running still confirms | ✅ | ✅ |
| 7 | 超时仍启用 | 轮询 10 次全失败 + `enabled=1` | `restart_failed` | timeout with UCI enabled is restart_failed | ✅ | ✅ |

配套不变量（同套件内，四态之外）：校验失败零副作用（live/`.bak` 字节不动、临时文件删除、无备份复制无重启）、基线 md5 不一致拒绝、备份哈希不一致拒绝、替换失败中止、restore 走同链且不覆盖 `.bak`。

## 判定规则的收紧（本批记录）

契约 §3 原文「出现 exit 0」即 confirmed，对**启动后退出**（起后几百 ms 内崩溃）会误报。本批把判定收紧为「**连续两次** exit 0」（仍在 10 次/~5s 轮询预算内，健康路径多花一个 500ms 间隔）；契约 §3 判定单元格已同步改写。场景 3 即该规则的回归用例。

## 命令与输出摘录

宿主 Lua 5.4：

```
$ lua5.4 packaging/openwrt/tests/apply-test.lua
… (62 行 ok)
62 assertions passed
```

隔离容器 Lua 5.1.5（debian:bookworm-slim + apt lua5.1，仓库只读挂载；5.1.5 与 OpenWrt LuCI 运行时同大版本）：

```
$ docker run --rm -v /home/cui/workspaces/custos:/work:ro debian:bookworm-slim \
    bash -c "apt-get update -qq && apt-get install -y -qq lua5.1 && \
    lua5.1 /work/packaging/openwrt/tests/apply-test.lua | tail -1"
Lua 5.1.5  Copyright (C) 1994-2012 Lua.org, PUC-Rio
62 assertions passed
```

同容器三套件合计：apply 62 / settings 45 / tomledit 44，全绿。语法门禁：`luac -p`（5.4）对 `custos/*.lua`、controller、tests 全过；设备侧以 Lua 5.1 解析同一批源文件（容器内执行即证）。

## 四态与 UI 的对应（实现位置）

- `luci.custos.apply` `settle()`：`packaging/openwrt/usr/lib/lua/luci/custos/apply.lua`（本文件同仓）——重启 rc≠0 直接 restart_failed；轮询 500ms×10；连续两次 running 才 confirmed；超时后按 UCI `custos.main.enabled` 分 stopped_by_user / restart_failed。
- 控制器映射 `write_response()`：confirmed/noop/stopped_by_user → ok；restart_failed → ok + `needs_attention` + `.bak` 路径；页面（resources.htm）按 state 渲染，restart_failed 保留错误与「用备份恢复」。

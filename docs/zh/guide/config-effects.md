---
title: 配置开关效果
---

# 配置开关效果

本页收录当前有「开启后有效果」的四项用户可见配置开关，附自动生成证据。证据由 `scripts/generate-config-effects.sh` 生成：该脚本驱动真实 `custos` 二进制，抓取 OFF 与 ON 两个状态的终端输出，并用 ffmpeg 缝合为循环 GIF。流水线可重跑——配置变更后随时再生成，兼作配置效果回归测试。

生成器覆盖以下四个开关。每个条目展示 OFF 帧、ON 帧以及 off→on 切换过程的循环 GIF。

## 1. `dashboard.enabled`

可选 Web 仪表盘默认关闭。`enabled = false` 时不监听任何端口、无 HTTP 代码路径；`enabled = true` 时按 `listen` 地址提供只读仪表盘。

```toml
[dashboard]
enabled = false        # off：无监听
listen = "127.0.0.1:8080"
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![仪表盘关闭](/effects/dashboard-enabled-off.png) | ![仪表盘开启](/effects/dashboard-enabled-on.png) | ![仪表盘切换](/effects/dashboard-enabled.gif) |

## 2. `resources[].enabled`

`enabled = false` 的资源完全跳过——不探测、不恢复。`enabled = true` 后探测执行，故障驱动恢复运行（及升级）。

```toml
[[resources]]
id = "demo"
type = "custom-command"
enabled = false       # off：不记录恢复运行
probe.command = ["false"]
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![资源关闭](/effects/resource-enabled-off.png) | ![资源开启](/effects/resource-enabled-on.png) | ![资源切换](/effects/resource-enabled.gif) |

## 3. `notification.channels.<id>.enabled`（local-log 示例）

通知渠道 `enabled = false` 时不出站；`enabled = true` 后本地结构化审计日志逐事件输出 JSON 行。

```toml
[notification.channels.audit]
type = "local-log"
enabled = false       # off：无审计行
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![本地日志关闭](/effects/local-log-enabled-off.png) | ![本地日志开启](/effects/local-log-enabled-on.png) | ![本地日志切换](/effects/local-log-enabled.gif) |

## 4. `recovery_policy.allow_high_risk`

高风险恢复方案在配置校验阶段即被拒绝，除非该资源 `allow_high_risk = true`（security.md §12.2）。

```toml
[[resources]]
id = "demo"
type = "custom-command"
[resources.recovery_policy]
allow_high_risk = false   # off：校验拒绝高风险方案
[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["unknown_failure"]
risk = "high"
action = { program = "/bin/true" }
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![高风险关闭](/effects/allow-high-risk-off.png) | ![高风险开启](/effects/allow-high-risk-on.png) | ![高风险切换](/effects/allow-high-risk.gif) |

## 再生成

```bash
cargo build -p custos-cli
bash scripts/generate-config-effects.sh
```

配置变更后随时运行；`docs/public/effects/` 中的证据原位再生成。脚本依赖 `ffmpeg` 与 ImageMagick（`convert`）。

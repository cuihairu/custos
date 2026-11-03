---
title: Configuration Switch Effects
---

# Configuration Switch Effects

This page documents the four user-facing configuration switches that currently have an observable off→on effect, with generated evidence. The evidence is produced by `scripts/generate-config-effects.sh`, which drives the real `custos` binary, captures terminal output of the OFF state and the ON state, and stitches a looping GIF. It is re-runnable: any config change re-generates the evidence in place, so it doubles as a configuration-effect regression check.

The generator covers the four switches below. Each entry shows the OFF frame, the ON frame, and a looping GIF of the off→on transition.

## 1. `dashboard.enabled`

The optional web dashboard is disabled by default. When `enabled = false` there is no listener and no HTTP code path runs; when `enabled = true` the dashboard serves on the configured `listen` address.

```toml
[dashboard]
enabled = false        # off: no listener
listen = "127.0.0.1:8080"
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![dashboard disabled](/effects/dashboard-enabled-off.png) | ![dashboard enabled](/effects/dashboard-enabled-on.png) | ![dashboard toggle](/effects/dashboard-enabled.gif) |

## 2. `resources[].enabled`

A resource with `enabled = false` is skipped entirely — no probe, no recovery run. With `enabled = true` the probe runs and failures drive recovery runs (and escalation).

```toml
[[resources]]
id = "demo"
type = "custom-command"
enabled = false       # off: no recovery run recorded
probe.command = ["false"]
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![resource disabled](/effects/resource-enabled-off.png) | ![resource enabled](/effects/resource-enabled-on.png) | ![resource toggle](/effects/resource-enabled.gif) |

## 3. `notification.channels.<id>.enabled` (local-log)

A notification channel with `enabled = false` emits nothing; with `enabled = true` the local structured audit log receives one JSON line per event.

```toml
[notification.channels.audit]
type = "local-log"
enabled = false       # off: no audit line
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![local-log disabled](/effects/local-log-enabled-off.png) | ![local-log enabled](/effects/local-log-enabled-on.png) | ![local-log toggle](/effects/local-log-enabled.gif) |

## 4. `recovery_policy.allow_high_risk`

High-risk recovery plans are rejected at validation time unless `allow_high_risk = true` on the resource (security.md §12.2).

```toml
[[resources]]
id = "demo"
type = "custom-command"
[resources.recovery_policy]
allow_high_risk = false   # off: validation rejects the high-risk plan
[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["unknown_failure"]
risk = "high"
action = { program = "/bin/true" }
```

**Off → On**

| OFF | ON | GIF |
|---|---|---|
| ![high-risk off](/effects/allow-high-risk-off.png) | ![high-risk on](/effects/allow-high-risk-on.png) | ![high-risk toggle](/effects/allow-high-risk.gif) |

## Regeneration

```bash
cargo build -p custos-cli
bash scripts/generate-config-effects.sh
```

Run it after any config change; the evidence in `docs/public/effects/` is regenerated in place. The script requires `ffmpeg` and ImageMagick (`convert`).

[English](README.md) | [中文](README.zh.md)

<div align="center">
    <img src="docs/public/logo.svg" alt="custos logo" width="64" height="64" />
  <h1>custos</h1>
</div>

<div align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-stable-dea584?logo=rust&logoColor=white" /> <img alt="Node.js" src="https://img.shields.io/badge/Node.js-24-339933?logo=nodedotjs&logoColor=white" /> <img alt="pnpm" src="https://img.shields.io/badge/pnpm-workspaces-F69220?logo=pnpm&logoColor=white" /> <img alt="Python" src="https://img.shields.io/badge/Python-PyO3%20bindings-3776AB?logo=python&logoColor=white" /> <img alt="License" src="https://img.shields.io/badge/License-Apache--2.0-blue" /> <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS%20%28partial%29%20%7C%20OpenWrt-lightgrey" /> <a href="https://cuihairu.github.io/custos/"><img src="https://img.shields.io/badge/docs-blue" alt="docs"></a>
</div>

Custos is a cross-platform, local-first, dependency-aware fault detection and automatic recovery engine (self-healing) written in Rust.

- Resources are modeled as a dependency graph; each resource can be probed, diagnosed, recovered and verified.
- Plugins encapsulate concrete capabilities; the core engine owns dependency coordination, execution permission, recovery budget, audit and final verdicts.
- Local autonomy first: detection, repair and persistence keep working while the network is down.
- Recovery success must pass a health verification, never just an exit code.

## Tech stack

| Layer | Choice |
| --- | --- |
| Core engine | Rust (stable, Cargo workspace) |
| Docs site | VitePress · Node.js 24 · pnpm |
| Scripting access | Core callable from Python (PyO3); extension logic via Python scripts |
| Persistence | SQLite |

## Plugin model (authoritative)

One plugin = the complete monitoring + handling logic for ONE resource kind (probe, diagnosis, recovery actions, verification). Resources and their plugins are composed through the resource dependency graph; consumers enable them with simple configuration only — no glue code.

## Builtin capabilities and Python script extension (authoritative)

- **Builtin capabilities**: the core engine ships builtin base adapters for common resources (services, processes, disks, network connectivity, etc.) that work out of the box — basic scenarios need no plugin development at all.
- **Python script extension (bidirectional)**: outbound — core functionality is callable from Python scripts via PyO3 bindings; inbound — custom probes and recovery actions can be written as Python scripts, loaded and executed by the core under the plugin contract (timeouts, permissions and budgets follow the same plugin rules).

## Documentation

The full architecture design and phased implementation plan live in the VitePress site under `docs/`:

```bash
pnpm install
pnpm run docs:dev    # local preview
pnpm run docs:build  # build
```

Or read the sources directly: [docs/guide/overview.md](docs/guide/overview.md).

## Examples

Ready-to-run monitoring configurations in `examples/` (each validates with `custos validate <file>`):

- [basic-process.toml](examples/basic-process.toml) — watch a PID file, restart via plan.
- [dependency-chain.toml](examples/dependency-chain.toml) — tunnel → proxy → network recovery order.
- [notifications.toml](examples/notifications.toml) — webhook/SMTP notification wiring.
- [sunlogin.toml](examples/sunlogin.toml) — 向日葵 (Sunlogin) remote-desktop supervision: PC offline detection and only-Sunlogin recovery, the canonical self-healing scenario (Windows `SunloginService`, Linux `oray-sunlogin.service`).
- [remote-desktop.toml](examples/remote-desktop.toml) — RustDesk + TeamViewer + 向日葵 behind one network dependency: if the network is down only network recovery runs; otherwise only the dead remote-desktop service is restarted.
- [network-outage.toml](examples/network-outage.toml) — 断网 detection with layered recovery: LAN gateway probe and public-DNS anchor as a dependency chain, so the LAN link is fixed before the WAN side.

中文文档：[README.zh.md](README.zh.md)

## OpenWrt / iStoreOS

OpenWrt packaging lives under `packaging/openwrt/` and builds three .ipk packages:

```bash
sh packaging/openwrt/build-ipk.sh <path-to-musl-custos-binary> dist/ --arch x86_64|aarch64
```

- `custos` — musl static binary, procd init script, UCI config (`/etc/config/custos`) and default TOML.
- `luci-app-custos` — LuCI app: settings page (service enable, database and TOML config paths, probe interval; Telegram/Herald/local-log notification channels passed to the engine as `CUSTOS_NOTIFY_*` environment variables by the init script), a live status dashboard (service state, resource health, recent audit events, incidents, recovery runs; auto-refresh every 10 s), and a monitoring-resource / recovery-policy editor — structured forms for the common probe types, a raw-TOML escape hatch for the rest, backups before every write, `custos validate` gating and automatic service restart (the TOML stays the single source of truth; unknown keys are preserved verbatim). UI text is Chinese, hardcoded in the templates — no .po files.
- `app-meta-custos` — iStore app metadata (icon + manifest), so the app shows up in the iStore panel like the other store apps.

Install order: `custos` → `luci-app-custos` → `app-meta-custos` (the Depends chain pulls them in this order), or simply `opkg install app-meta-custos_*.ipk`. Enable with `uci set custos.main.enabled=1 && uci commit custos && /etc/init.d/custos start`, then open LuCI → Services → Custos.

iStore publishing: the manifest format was verified against installed app-meta packages on a real iStoreOS device (see [docs/analysis/istore-publishing-2026-11-14.md](docs/analysis/istore-publishing-2026-11-14.md)); the official store listing itself still requires contacting the iStore (Linkease) team. The packages follow the generic opkg spec and install with plain `opkg` regardless.

Upgrade notes: `/etc/custos/custos.toml`, `/etc/config/custos` and `/etc/init.d/custos` are conffiles — a normal `opkg` upgrade keeps a modified config and drops the new default beside it as `custos.toml-opkg`. Migrating from an old single-package layout (everything in `custos`) needs `opkg install --force-reinstall custos_*.ipk` first — which does replace conffiles — before `luci-app-custos` can install.

## Status

P0–P3 are implemented (core recovery engine, plugin runtime and notifications, platform base capabilities, and the network/proxy/self-healing scenarios; acceptance scenarios 1 and 2 and the full Windows Wi-Fi path still await verification on real hardware). The optional management interface (P4) has the read-only dashboard and desktop tray. macOS has compile-checked base branches (launchd, network interface, reboot, menu-bar tray) but none of them is acceptance-tested on a real Mac yet; BSD has no implementation; plugin signing is pending. See the [phased implementation plan](docs/guide/roadmap.md).

## License

Apache-2.0 — see [LICENSE](LICENSE).

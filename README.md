[English](README.md) | [中文](README.zh.md)

<div align="center">
  <img src="docs/public/logo.svg" alt="custos logo" width="64" height="64" />
  <h1>custos</h1>
  <p><img alt="Rust" src="https://img.shields.io/badge/Rust-stable-dea584?logo=rust&logoColor=white" /> <img alt="Node.js" src="https://img.shields.io/badge/Node.js-24-339933?logo=nodedotjs&logoColor=white" /> <img alt="pnpm" src="https://img.shields.io/badge/pnpm-workspaces-F69220?logo=pnpm&logoColor=white" /> <img alt="Python" src="https://img.shields.io/badge/Python-PyO3%20bindings-3776AB?logo=python&logoColor=white" /> <img alt="License" src="https://img.shields.io/badge/License-Apache--2.0-blue" /> <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS%20%7C%20BSD%20%7C%20OpenWrt-lightgrey" /></p>
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

One plugin = the complete monitoring + handling logic for ONE resource kind (probe, diagnosis, recovery actions, verification). Plugins can depend on other plugins; consumers enable them with simple configuration only — no glue code.

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

中文文档：[README.zh.md](README.zh.md)

## Status

P0 in progress — the Cargo workspace skeleton has landed (domain model, dependency-graph cycle detection, configuration validation, the `custos validate` CLI command); the remaining P0 items follow the [phased implementation plan](docs/guide/roadmap.md).

## License

Apache-2.0 — see [LICENSE](LICENSE).

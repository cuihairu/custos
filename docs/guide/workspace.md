---
title: Cargo Workspace
---

# 13. Cargo Workspace

The workspace keeps the layout moderately modular; crates are split along the engine boundaries.

```text
custos/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── README.md
├── LICENSE
├── docs/                 VitePress site: guide/, adr/, zh/, public/
├── crates/
│   ├── custos-core/
│   ├── custos-model/
│   ├── custos-engine/
│   ├── custos-probe/
│   ├── custos-plugin-api/
│   ├── custos-plugin-host/
│   ├── custos-toolbox/
│   ├── custos-platform/
│   ├── custos-config/
│   ├── custos-store/
│   ├── custos-notify/
│   ├── custos-python/
│   ├── custos-tray/
│   ├── custos-dashboard/
│   └── custos-cli/
├── plugins/              external-process plugins
│   ├── mihomo/
│   ├── openclash/
│   ├── cloudflared/
│   └── python/           script-plugin shim and examples
└── examples/
    ├── basic-process.toml
    ├── dependency-chain.toml
    ├── network-outage.toml
    ├── notifications.toml
    ├── remote-desktop.toml
    └── sunlogin.toml
```

Integration tests live inside each crate under `crates/*/tests/` (for example `crates/custos-engine/tests/vertical.rs`, `crates/custos-notify/tests/offline.rs`, `crates/custos-platform/tests/`).

Responsibilities per module:

- `custos-model`: domain types, IDs, states and event models.
- `custos-core`: runtime assembly and resource registration.
- `custos-engine`: dependency scheduling, the recovery state machine, budgets and mutexes.
- `custos-probe`: probe scheduling and result normalization.
- `custos-plugin-api`: plugin protocol models and capability declarations.
- `custos-plugin-host`: plugin process management, JSON-RPC, timeouts and cancellation.
- `custos-toolbox`: internal built-in tool registry (exec, process, host operations) and the dispatch permission gate.
- `custos-platform`: cross-platform capability interfaces and their platform implementations.
- `custos-config`: configuration parsing, validation and version compatibility.
- `custos-store`: SQLite, migrations, transactions and recovery task persistence.
- `custos-notify`: event rules, the notification queue, retries and channel dispatch.
- `custos-python`: PyO3 bindings so Python can query and trigger the engine.
- `custos-tray`: desktop tray icon (Windows/macOS) driven by stored health state.
- `custos-dashboard`: read-only HTTP dashboard, disabled by default.
- `custos-cli`: the command-line entry point.

---

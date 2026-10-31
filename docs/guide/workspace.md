---
title: Cargo Workspace
---

# 13. Suggested Cargo Workspace

Keep the initial layout moderately modular; do not over-split into crates.

```text
custos/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── README.md
├── LICENSE
├── docs/
│   ├── architecture.md
│   ├── configuration.md
│   ├── plugin-protocol.md
│   ├── recovery-state-machine.md
│   ├── notification.md
│   ├── security.md
│   ├── testing.md
│   ├── adr/
│   └── implementation/
├── crates/
│   ├── custos-core/
│   ├── custos-model/
│   ├── custos-engine/
│   ├── custos-probe/
│   ├── custos-plugin-api/
│   ├── custos-plugin-host/
│   ├── custos-platform/
│   ├── custos-config/
│   ├── custos-store/
│   ├── custos-notify/
│   └── custos-cli/
├── plugins/
│   ├── process/
│   ├── exec/
│   ├── system-service/
│   ├── network/
│   ├── mihomo/
│   ├── cloudflared/
│   ├── openclash/
│   ├── notification-webhook/
│   └── notification-smtp/
├── tests/
│   ├── integration/
│   ├── recovery/
│   ├── persistence/
│   ├── plugin-protocol/
│   ├── notification/
│   └── fixtures/
└── examples/
    ├── basic-process.toml
    ├── dependency-chain.toml
    └── notifications.toml
```

This is the target structure; not every directory needs to exist on day one.

Initially it is fine to keep some plugins and platform adapters as modules inside the same crate, and split them out once the interfaces stabilize.

Suggested responsibilities per module:

- `custos-model`: domain types, IDs, states and event models.
- `custos-core`: runtime assembly and resource registration.
- `custos-engine`: dependency scheduling, the recovery state machine, budgets and mutexes.
- `custos-probe`: probe scheduling and result normalization.
- `custos-plugin-api`: plugin protocol models and capability declarations.
- `custos-plugin-host`: plugin process management, JSON-RPC, timeouts and cancellation.
- `custos-platform`: cross-platform capability interfaces and their platform implementations.
- `custos-config`: configuration parsing, validation and version compatibility.
- `custos-store`: SQLite, migrations, transactions and recovery task persistence.
- `custos-notify`: event rules, the notification queue, retries and channel dispatch.
- `custos-cli`: the command-line entry point.

---

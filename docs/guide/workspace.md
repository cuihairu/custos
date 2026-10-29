---
title: Cargo Workspace
---

# 13. 建议的 Cargo Workspace

建议初期保持适度模块化，不要过度拆分 crate。

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

这是目标结构，不要求所有目录在第一天都创建。

初期允许将部分插件和平台适配器放在同一 crate 的模块中，等接口稳定后再拆分。

建议各模块职责：

- `custos-model`：领域类型、ID、状态和事件模型。
- `custos-core`：运行时装配和资源注册。
- `custos-engine`：依赖调度、恢复状态机、预算与互斥。
- `custos-probe`：探测调度和结果归一化。
- `custos-plugin-api`：插件协议模型和能力声明。
- `custos-plugin-host`：插件进程管理、JSON-RPC、超时与取消。
- `custos-platform`：跨平台能力接口及平台实现。
- `custos-config`：配置解析、验证和版本兼容。
- `custos-store`：SQLite、迁移、事务和恢复任务持久化。
- `custos-notify`：事件规则、通知队列、重试和渠道调度。
- `custos-cli`：命令行入口。

---

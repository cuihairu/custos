---
title: Cargo Workspace
---

# 13. Cargo Workspace

工作区保持适度模块化；crate 按引擎边界拆分。

```text
custos/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── README.md
├── LICENSE
├── docs/                 VitePress 站点：guide/、adr/、zh/、public/
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
├── plugins/              外部进程插件
│   ├── mihomo/
│   ├── openclash/
│   ├── cloudflared/
│   └── python/           脚本插件 shim 与示例
└── examples/
    ├── basic-process.toml
    ├── dependency-chain.toml
    └── notifications.toml
```

集成测试在各 crate 内的 `crates/*/tests/` 下（如 `crates/custos-engine/tests/vertical.rs`、`crates/custos-notify/tests/offline.rs`、`crates/custos-platform/tests/`）。

建议各模块职责：

- `custos-model`：领域类型、ID、状态和事件模型。
- `custos-core`：运行时装配和资源注册。
- `custos-engine`：依赖调度、恢复状态机、预算与互斥。
- `custos-probe`：探测调度和结果归一化。
- `custos-plugin-api`：插件协议模型和能力声明。
- `custos-plugin-host`：插件进程管理、JSON-RPC、超时与取消。
- `custos-toolbox`：内置工具注册表（exec、process、host 操作）与分发权限门。
- `custos-platform`：跨平台能力接口及平台实现。
- `custos-config`：配置解析、验证和版本兼容。
- `custos-store`：SQLite、迁移、事务和恢复任务持久化。
- `custos-notify`：事件规则、通知队列、重试和渠道调度。
- `custos-python`：PyO3 绑定，供 Python 查询与触发引擎。
- `custos-tray`：桌面托盘（Windows/macOS），由存储的健康状态驱动。
- `custos-dashboard`：只读 HTTP 仪表盘，默认关闭。
- `custos-cli`：命令行入口。

---

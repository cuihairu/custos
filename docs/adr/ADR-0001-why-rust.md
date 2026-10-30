# ADR-0001: 为什么选择 Rust

- 状态：已接受
- 日期：2026-10-31
- 关联：docs/guide/overview.md，workspace.md

## 背景

Custos 是常驻本地的自愈引擎：长期运行、低抖动要求高、必须跨平台（Linux/Windows 优先）、
单二进制部署、领域模型里有严格的状态机（恢复运行、健康转换、预算与互斥）。选型决定了
内存安全、部署形态和长期维护成本。

## 决策

选择 Rust 作为唯一实现语言：领域模型用类型编码（`HealthStatus` 七态、`RecoveryRunStatus`
状态机、强类型 ID），workspace 按 crate 分层（model/engine/probe/notify/store/config/cli），
异步仅在 store 内部（sync 门面 + current-thread runtime），TLS 用 rustls 保持纯 Rust 可交叉编译。

## 替代方案

- **Go**：GC 停顿与常驻内存占用更高；错误处理靠约定（err != nil）无编译期约束；
  交叉编译单二进制可行但状态机正确性靠人。
- **C/C++**：内存安全不可接受——自愈引擎拥有系统级权限，UAF/溢出即权限提升。
- **Python/脚本**：性能与部署（解释器、依赖树）不适合常驻引擎；类型约束弱。

## 优缺点

优点：无 GC 的可预测延迟；`enum + match` 穷尽性检查让状态机漏分支编译期报错；
crates.io 成熟生态（rustls/sqlx/lettre/ureq）符合"核心用成熟库"的裁定；
单二进制、交叉编译到 mingw 验证过。
缺点：编译时间长；开发迭代慢于脚本语言；部分生态（PyO3 出向绑定）有额外复杂度。

## 后续影响

- workspace 11 个 crate 的分层由 Rust 模块系统承载（依赖方向单向）。
- 所有"核心用成熟 crate、插件侧轻依赖"的裁定以此为基础。
- rust-toolchain.toml 需声明 stable 通道（todo.md 工程修缮项）。

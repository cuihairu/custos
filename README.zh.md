[English](README.md) | [中文](README.zh.md)

<div align="center">
  <img src="docs/public/logo.svg" alt="custos logo" width="64" height="64" />
  <h1>custos</h1>
  <p><img alt="Rust" src="https://img.shields.io/badge/Rust-stable-dea584?logo=rust&logoColor=white" /> <img alt="Node.js" src="https://img.shields.io/badge/Node.js-24-339933?logo=nodedotjs&logoColor=white" /> <img alt="pnpm" src="https://img.shields.io/badge/pnpm-workspaces-F69220?logo=pnpm&logoColor=white" /> <img alt="Python" src="https://img.shields.io/badge/Python-PyO3%20bindings-3776AB?logo=python&logoColor=white" /> <img alt="License" src="https://img.shields.io/badge/License-Apache--2.0-blue" /> <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS%20%7C%20BSD%20%7C%20OpenWrt-lightgrey" /></p>
</div>

Custos 是一个用 Rust 编写的跨平台、本地自治、依赖感知的故障检测与自动恢复（自愈）引擎。

- 资源以依赖图建模；每个资源具备探测、诊断、恢复与验证能力。
- 插件封装具体能力；核心引擎统一负责依赖协调、执行权限、恢复预算、审计与最终判定。
- 本地自治优先：断网时检测、修复、持久化照常工作。
- 恢复成功必须通过健康验证，不能只看命令退出码。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 核心引擎 | Rust（stable，Cargo workspace） |
| 文档站点 | VitePress · Node.js 24 · pnpm |
| 脚本调用 | 核心经 PyO3 可被 Python 调用；扩展逻辑可用 Python 脚本编写 |
| 持久化 | SQLite |

## 插件口径（权威）

一个插件 = 一种资源的完整监控与处理逻辑（探测、诊断、恢复动作、验证）。插件之间可以相互依赖；使用方只需简单配置即可启用，无需编写胶水代码。

## 集成基础能力与 Python 脚本扩展（权威）

- **集成基础能力**：核心引擎内置常见资源（服务、进程、磁盘、网络连通性等）的基础适配，开箱即用——基础场景不需要任何插件开发。
- **Python 脚本扩展（双向）**：出向——核心功能经 PyO3 绑定可被 Python 脚本调用；入向——自定义探测与恢复动作可用 Python 脚本编写，由核心按插件契约加载执行（超时、权限、预算同插件口径）。

## 文档

完整架构设计与分阶段实施计划在 `docs/` 下的 VitePress 站点：

```bash
pnpm install
pnpm run docs:dev    # 本地预览
pnpm run docs:build  # 构建
```

也可直接读源文件：[docs/guide/overview.md](docs/guide/overview.md)。

English: [README.md](README.md)

## 状态

P0 实施中——Cargo workspace 骨架已落地（领域模型、依赖图环检测、配置校验、CLI validate 命令），其余 P0 项按 [分阶段实施计划](docs/guide/roadmap.md) 推进。

## 许可证

Apache-2.0，见 [LICENSE](LICENSE)。

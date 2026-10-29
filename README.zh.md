# custos

Custos 是一个用 Rust 编写的跨平台、本地自治、依赖感知的故障检测与自动恢复（自愈）引擎。

- 资源以依赖图建模；每个资源具备探测、诊断、恢复与验证能力。
- 插件封装具体能力；核心引擎统一负责依赖协调、执行权限、恢复预算、审计与最终判定。
- 本地自治优先：断网时检测、修复、持久化照常工作。
- 恢复成功必须通过健康验证，不能只看命令退出码。

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

设计阶段——架构文档与 P0–P4 分阶段计划是当前交付物，实现尚未开始。

## 许可证

见 [LICENSE](LICENSE)。

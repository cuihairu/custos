# custos

Custos is a cross-platform, local-first, dependency-aware fault detection and automatic recovery engine (self-healing) written in Rust.

- Resources are modeled as a dependency graph; each resource can be probed, diagnosed, recovered and verified.
- Plugins encapsulate concrete capabilities; the core engine owns dependency coordination, execution permission, recovery budget, audit and final verdicts.
- Local autonomy first: detection, repair and persistence keep working while the network is down.
- Recovery success must pass a health verification, never just an exit code.

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

Design phase — the architecture document and phased plan (P0–P4) are the current deliverable; implementation has not started.

## License

See [LICENSE](LICENSE).

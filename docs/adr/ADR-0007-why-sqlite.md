# ADR-0007: 为什么使用 SQLite

- 状态：已接受
- 日期：2026-10-31
- 关联：docs/guide/persistence.md §7

## 背景

Custos 是本地优先引擎：断网时必须继续工作（事件入队、状态持久化），
崩溃后必须能恢复未完成的恢复任务；用户不应被要求先装一个数据库服务。

## 决策

SQLite（WAL 模式，`libsqlite3-sys` bundled 保证各平台从源码构建）：
单一文件、零运维、事务满足崩溃一致性写序；custos-store 提供版本化迁移
（`schema_migrations`，迁移只增不改历史）与 sync 门面（引擎保持同步模型）。

## 替代方案

- **PostgreSQL/MySQL**：需要外部服务，违背本地优先；断网即失忆。
- **JSON 文件**：无事务，崩溃窗口内丢事实；无索引，审计查询退化。
- **嵌入式 KV（sled/redb）**：成熟度与查询能力不足，审计检索要自建。

## 优缺点

优点：部署即文件；事务让"执行前持久化 attempt"（ADR-0008）有原子性；
SQL 审计与 CLI 的 status/runs/incidents/notify 查询直达。
缺点：单写者（WAL + 单连接足够：一个引擎进程）；大表需保留期清理（§7.4 已实现）。

## 后续影响

- 所有持久化走 custos-store；迁移 V1→V3 已有，schema 变更必须新版本号。
- 通知队列（五状态）与恢复状态共用同一 SQLite 文件，断网补发天然成立。

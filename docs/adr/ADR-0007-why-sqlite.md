# ADR-0007: Why SQLite

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/persistence.md §7

## Background

Custos is a local-first engine: it must keep working when the network is down (events enqueued, state persisted), and it must recover unfinished recovery tasks after a crash; the user should not be required to install a database service first.

## Decision

SQLite (WAL mode, `libsqlite3-sys` bundled so all platforms build from source): single file, zero operations, transactions satisfy the crash-consistency write ordering; custos-store provides versioned migrations (`schema_migrations`, migrations only append) and a sync facade (the engine keeps a synchronous model).

## Alternatives

- **PostgreSQL/MySQL**: requires an external service — violates local-first; offline = amnesia.
- **JSON files**: no transactions, facts lost in the crash window; no indexes, audit queries degrade.
- **Embedded KV (sled/redb)**: insufficient maturity and query capability; audit retrieval would need custom building.

## Pros and Cons

Pros: deployment is a file; transactions give "persist attempt before execution" (ADR-0008) atomicity; SQL audit and the CLI's status/runs/incidents/notify queries reach directly.
Cons: single writer (WAL + one connection suffices: one engine process); large tables need retention purging (§7.4 implemented).

## Follow-up Impact

- All persistence goes through custos-store; migrations V1→V3 exist, schema changes must use a new version number.
- The notification queue (five states) and recovery state share the same SQLite file — offline drain works naturally.

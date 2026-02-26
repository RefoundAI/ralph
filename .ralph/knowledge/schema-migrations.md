---
title: Schema Migrations
tags: [sqlite, schema, migrations, dag, database]
created_at: "2026-02-18T00:00:00Z"
---

SQLite schema versioning via `user_version` pragma in `src/dag/db.rs`.

## Pattern

Migrations use version-range checks so fresh databases jump straight to the latest version:

```rust
if from_version < N && to_version >= N {
    conn.execute_batch("CREATE TABLE ...; ALTER TABLE ...;")?;
}
```

## Current Schema (v5)

- **v1**: `tasks`, `dependencies`, `task_logs` tables
- **v2**: `features` table; extends `tasks` with `feature_id`, `task_type`, `retry_count`, `max_retries`, `verification_status` (see [[Task Columns Mapping]])
- **v3**: `journal` table + FTS5 virtual table with auto-update triggers (see [[Journal System]])
- **v4**: Performance indexes on `tasks` (status/priority/created, parent_id, feature+status+priority+created), `dependencies` (blocked_id), and `task_logs` (task_id+timestamp)
- **v5**: `model_overrides` table (`iteration`, `strategy_choice`, `hint`, `created_at`) + index on `iteration`. Used by [[Model Strategy Selection]] to persist override history in SQLite instead of a flat file

## Gotchas

- FTS5 content-sync triggers must cover INSERT, UPDATE, and DELETE. Missing the UPDATE trigger causes stale search results.
- Version is stored in pragma `user_version`, not a table row.
- Migrations use `execute_batch()` for atomicity within a version step.
- WAL mode and foreign keys are set at connection time, not in schema.

See also: [[Task Columns Mapping]], [[Journal System]], [[Knowledge System]], [[Model Strategy Selection]]

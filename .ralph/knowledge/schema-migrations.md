---
title: "SQLite schema migration pattern"
tags: [sqlite, schema, migration, dag, database]
created_at: "2026-02-18T00:00:00Z"
---

Schema migrations in `dag/db.rs` use version-range checks against the SQLite `user_version` pragma. Current schema version is 3.

Pattern for adding a new migration:
```rust
if from_version < N && to_version >= N {
    conn.execute_batch("CREATE TABLE ...; ALTER TABLE ...;")?;
}
```

Migration history:
- **v1**: Base tables (`tasks`, `dependencies`, `task_logs`)
- **v2**: Adds `features` table, extends `tasks` with `feature_id`, `task_type`, `retry_count`, `max_retries`, `verification_status`
- **v3**: Adds `journal` table with FTS5 virtual table (`journal_fts`) and INSERT/UPDATE/DELETE triggers to maintain the FTS index

Gotchas:
- FTS5 triggers must mirror each other (INSERT/UPDATE/DELETE) to keep the content-synced virtual table consistent.
- Version is stored in pragma `user_version`, not a table row.
- Migrations use `execute_batch()` for atomicity within a version step.
- Always test the upgrade path from the previous version (e.g., `test_schema_v3_migration_from_v2`).

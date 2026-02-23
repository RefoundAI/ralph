---
title: Task Columns Mapping
tags: [dag, tasks, sqlite, schema, columns]
created_at: "2026-02-18T00:00:00Z"
---

Centralized SQL-to-Task mapping in `src/dag/mod.rs` via `TASK_COLUMNS` constant and `task_from_row()` helper.

## Column Order (14 columns, strict positional)

```
0: id, 1: title, 2: description, 3: status, 4: parent_id,
5: feature_id, 6: task_type, 7: priority, 8: retry_count,
9: max_retries, 10: verification_status, 11: created_at,
12: updated_at, 13: claimed_by
```

## Nullable Column Pattern

```rust
row.get::<_, Option<T>>(N)?.unwrap_or(default)
```

Nullable: `parent_id`, `claimed_by`, `task_type` (default "feature"), `feature_id`, `verification_status`, `priority` (default 0), `retry_count` (default 0), `max_retries` (default 3).

## Adding a New Column

1. Add migration in `src/dag/db.rs` (see [[Schema Migrations]])
2. Add field to `Task` struct in `src/dag/mod.rs`
3. Append to `TASK_COLUMNS` constant
4. Add extraction at the next index in `task_from_row()`
5. Update all test helpers that construct `Task` manually

Index mismatch causes silent wrong-value assignment — Rust won't catch this at compile time.

See also: [[Schema Migrations]], [[Auto-Transitions]]

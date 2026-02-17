---
title: "TASK_COLUMNS and task_from_row mapping"
tags: [task, sqlite, dag, columns, mapping]
created_at: "2026-02-18T00:00:00Z"
---

All Task queries must use `TASK_COLUMNS` (defined in `dag/mod.rs`) and `task_from_row()` for consistent SQL-to-struct mapping. There are 14 columns in exact positional order:

```
0: id, 1: title, 2: description, 3: status, 4: parent_id,
5: feature_id, 6: task_type, 7: priority, 8: retry_count,
9: max_retries, 10: verification_status, 11: created_at,
12: updated_at, 13: claimed_by
```

Nullable columns use `row.get::<_, Option<T>>(N)?.unwrap_or(default)`:
- description (pos 2): unwrap_or_default (empty string)
- task_type (pos 6): unwrap_or_else "feature"
- priority (pos 7): unwrap_or 0
- retry_count (pos 8): unwrap_or 0
- max_retries (pos 9): unwrap_or 3

When adding a new Task field:
1. Add column to CREATE TABLE in `db.rs` (or migration)
2. Add to end of `TASK_COLUMNS` string
3. Add field to `Task` struct
4. Add extraction at the next index in `task_from_row()`
5. Update all test helpers that construct Task manually

Index mismatch causes silent wrong-value assignment — Rust won't catch this at compile time.

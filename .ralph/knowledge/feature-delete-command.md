---
title: "Feature Delete Command"
tags: [feature, cli, delete, crud]
created_at: "2026-02-24T07:22:53.191412+00:00"
---

`ralph feature delete <name>` deletes a feature and all associated data:

## Cascade order (foreign key safe)
1. Dependencies (both blocker_id and blocked_id IN task_ids)
2. Task logs (task_id IN task_ids) 
3. Journal entries (WHERE feature_id = ?)
4. Tasks (WHERE feature_id = ?)
5. Feature directory on disk (`.ralph/features/<name>/`)
6. Feature row in DB

## Confirmation
- Shows feature status, task counts
- Warns if partially completed (some done, some not)
- Warns if tasks are in_progress
- `--yes`/`-y` flag skips confirmation

## Key function
`delete_tasks_for_feature()` in `dag/crud.rs` handles bulk task cleanup. Uses IN clauses with dynamic placeholders for deps/logs.

See also: [[Feature Lifecycle]]

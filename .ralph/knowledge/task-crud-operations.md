---
title: Task CRUD Operations
tags: [dag, crud, tasks, sqlite, ids, delete, tree]
created_at: "2026-02-23T06:37:00Z"
---

Task CRUD operations live in `src/dag/crud.rs`. ID generation in `src/dag/ids.rs`.

## Create

**`create_task(db, title, description, parent_id, priority)`** — defaults to `task_type = "feature"`, `max_retries = 3`. Delegates to `create_task_with_feature()`.

**`create_task_with_feature(..., feature_id, task_type, max_retries)`** — full creation function. Validates parent exists if `parent_id` specified. Uses `generate_and_insert_task_id()` with retry loop for collision handling. Sets `status = "pending"`, `retry_count = 0`.

## Read

- `get_task(db, id)` — single task via [[Task Columns Mapping]]
- `get_all_tasks(db)` — ordered by `priority ASC, created_at ASC`
- `get_all_tasks_for_feature(db, feature_id)` — feature-scoped, same ordering

## Update

`update_task(db, id, TaskUpdate { title, description, priority })` — dynamic SQL with only `Some` fields. Always updates `updated_at`. Returns task unchanged if no fields set.

## Delete

`delete_task(db, id)` has cascading behavior:

1. **Existence check** — error if task not found
2. **Blocker check** — error if task is a `blocker_id` in `dependencies` (must remove dependents first)
3. **Cascade children** — recursively deletes all child tasks (same safety checks each)
4. **Clean up edges** — deletes `dependencies` rows where task is `blocked_id`
5. **Clean up logs** — deletes `task_logs` entries
6. **Delete task** — removes the `tasks` row

## Logs

- `add_log(db, task_id, message)` — timestamped log entry (errors if task doesn't exist)
- `get_task_logs(db, task_id)` — all entries ordered by `timestamp ASC`
- `LogEntry` struct: `task_id`, `message`, `timestamp` (autoincrement `id` not surfaced)

## Dependency Queries

- `get_task_blockers(db, task_id)` — all tasks that block this one (prerequisites)
- `get_tasks_blocked_by(db, task_id)` — all tasks blocked by this one (dependents)

Both return full `Task` structs via JOIN on `dependencies` table.

## Task Trees

`get_task_tree(db, root_id)` — BFS traversal following `parent_id` edges (not dependency edges). Returns flat list of all descendants. Used by `ralph task tree` CLI for indented rendering.

## ID Generation

SHA-256 of `(timestamp_nanos_le_bytes || atomic_counter_le_bytes)`, take first 4 bytes (8 hex chars), prepend prefix:
- Tasks: `t-{8hex}` (e.g., `t-a3f1c924`)
- Features: `f-{8hex}` (e.g., `f-b7e20491`)
- Agent IDs use different mechanism: `DefaultHasher` over `(timestamp, PID)`, 8 hex chars

Expanded from 6 to 8 hex chars for collision resistance — see [[Task/Feature ID Entropy Expansion]].

**Collision handling:** `generate_and_insert_task_id()` wraps INSERT in retry loop (max 10). Atomic counter ensures successive calls in same nanosecond produce different hashes.

See also: [[Task Columns Mapping]], [[Auto-Transitions]], [[Dependency Cycle Detection]], [[Schema Migrations]], [[Task/Feature ID Entropy Expansion]]

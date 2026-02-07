# DAG Progress Tracker

Replace `progress.txt` with a SQLite-backed DAG of tasks in `.ralph/progress.db`. Single agent per iteration; parallelism deferred.

## Dependencies

Add `rusqlite` with `bundled` feature to `Cargo.toml`.

## Module Structure

New module: `src/dag/` with `mod.rs`, `db.rs`, `schema.rs`, `tasks.rs`, `dependencies.rs`, `ids.rs`.

## Schema

### R1: SQLite schema

Database: `.ralph/progress.db`. Enable WAL mode and foreign keys on every connection.

Tables:

```sql
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    parent_id TEXT REFERENCES tasks(id),
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','in_progress','done','blocked','failed')),
    priority INTEGER DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    claimed_by TEXT
);

CREATE TABLE dependencies (
    blocker_id TEXT NOT NULL REFERENCES tasks(id),
    blocked_id TEXT NOT NULL REFERENCES tasks(id),
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id != blocked_id)
);

CREATE TABLE task_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    message TEXT NOT NULL,
    timestamp TEXT NOT NULL
);
```

Track schema version via `PRAGMA user_version`.

**Verify:** Unit tests:
- Fresh DB creates all three tables with correct columns
- FK constraints enforced (insert dependency referencing nonexistent task fails)
- Duplicate dependency rejected (PK violation)
- Self-dependency rejected (CHECK constraint)
- `PRAGMA journal_mode` returns `wal`
- `PRAGMA foreign_keys` returns `1`

### R2: Task ID generation

Format: `t-` + 6 hex chars from SHA-256 of `(timestamp_nanos || counter)`.

On collision (insert fails with UNIQUE violation), increment counter and retry. Max 10 retries, then error.

IDs are immutable after creation.

**Verify:** Unit tests:
- Generated IDs match regex `^t-[0-9a-f]{6}$`
- 1000 sequential generates produce no duplicates
- Collision retry logic works (mock or force collision)

## Hierarchy & Dependencies

### R3: Task hierarchy

`parent_id` FK points to another task. Depth unbounded (practically: epic=0, task=1, subtask=2).

Parent status is derived from children:
- Any child `failed` -> parent `failed`
- All children `done` -> parent `done`
- Any child `in_progress` -> parent `in_progress`
- Otherwise -> `pending`

Parent status is recomputed on any child status change. Do not store derived status directly on parent rows that have children -- compute it on read, or update it transactionally on child mutation (implementer's choice, but must be consistent).

**Verify:** Unit tests:
- Parent with one `done` + one `pending` child -> `pending`
- Parent with all `done` children -> `done`
- Parent with one `failed` child -> `failed`
- Parent with one `in_progress` child -> `in_progress`
- 3-level deep hierarchy derives correctly

### R4: Dependencies (DAG edges)

A dependency `(blocker_id, blocked_id)` means `blocked_id` cannot start until `blocker_id` is `done`.

Cycle detection on insert: BFS/DFS from `blocked_id` through existing edges. If `blocker_id` is reachable from `blocked_id`, reject with error.

Removal is always allowed.

**Verify:** Unit tests:
- Valid dependency inserted
- Direct cycle rejected (A blocks B, B blocks A)
- Transitive cycle rejected (A->B->C, then C->A)
- Dependency removal succeeds
- Dependency on nonexistent task fails (FK)

### R5: Ready query

A task is "ready" when:
1. Status is `pending`
2. All blockers have status `done`
3. Parent (if any) is not `failed`
4. Task is a leaf node (no children)

Return ordered by: `priority ASC`, `created_at ASC`.

This is the query the run loop calls to pick the next task.

**Verify:** Unit tests:
- Task with no deps and no parent -> ready
- Task with pending blocker -> not ready
- Task with all blockers done -> ready
- Task with failed parent -> not ready
- Non-leaf task (has children) -> not ready
- Ordering: P0 before P1; same priority, earlier created_at first

### R6: Status transitions

Valid:
- `pending` -> `in_progress`
- `pending` -> `blocked`
- `in_progress` -> `done`
- `in_progress` -> `failed`
- `in_progress` -> `pending`
- `blocked` -> `pending`
- `failed` -> `pending`

All other transitions error.

Auto-transitions:
- Blocker marked `done` -> for each task it blocks, if all blockers now `done`, transition from `blocked` to `pending`
- Task marked `done` -> if parent exists and all siblings + self are `done`, mark parent `done`
- Task marked `failed` -> if parent exists, mark parent `failed`

All auto-transitions run in the same SQLite transaction as the triggering change.

**Verify:** Unit tests:
- Each valid transition succeeds
- Invalid transition (e.g. `done` -> `in_progress`) errors
- Auto-unblock fires: A blocks B (blocked), A completes, B becomes pending
- Auto-parent-complete fires: all children done, parent becomes done
- Auto-parent-fail fires: child fails, parent fails

## CRUD Operations

### R7: API surface

Implement in `src/dag/`:

```rust
pub fn init_db(path: &str) -> Result<Db>
pub fn create_task(db: &Db, title: &str, description: Option<&str>, parent_id: Option<&str>, priority: i32) -> Result<Task>
pub fn get_task(db: &Db, id: &str) -> Result<Task>
pub fn update_task(db: &Db, id: &str, fields: TaskUpdate) -> Result<Task>
pub fn delete_task(db: &Db, id: &str) -> Result<()>
pub fn add_dependency(db: &Db, blocker_id: &str, blocked_id: &str) -> Result<()>
pub fn remove_dependency(db: &Db, blocker_id: &str, blocked_id: &str) -> Result<()>
pub fn get_ready_tasks(db: &Db) -> Result<Vec<Task>>
pub fn get_task_tree(db: &Db, root_id: &str) -> Result<Vec<Task>>
pub fn claim_task(db: &Db, id: &str, agent_id: &str) -> Result<Task>
pub fn complete_task(db: &Db, id: &str) -> Result<()>
pub fn fail_task(db: &Db, id: &str, reason: &str) -> Result<()>
pub fn add_log(db: &Db, task_id: &str, message: &str) -> Result<()>
```

`Db` wraps `rusqlite::Connection`. `Task` is a struct mirroring the tasks table. `TaskUpdate` holds optional fields for partial update.

`delete_task`: reject if other tasks depend on it (blocker in dependencies table). Cascade delete children.

`claim_task`: atomic -- set `status=in_progress` + `claimed_by=agent_id` in one UPDATE. Fail if task is not `pending`.

`complete_task`: set `status=done`, clear `claimed_by`, run auto-transitions (R6).

`fail_task`: set `status=failed`, clear `claimed_by`, log reason to task_logs, run auto-transitions (R6).

**Verify:** Unit tests for each function. Integration test: create graph (A blocks B, B blocks C), claim A, complete A, verify B becomes ready, claim B, complete B, verify C becomes ready.

### R8: Database initialization

`init_db(path)`:
1. Create parent dirs if needed
2. Open or create SQLite DB at path
3. `PRAGMA journal_mode=WAL`
4. `PRAGMA foreign_keys=ON`
5. Read `PRAGMA user_version`
6. If `user_version < CURRENT_VERSION`, run migration SQL in order
7. Set `PRAGMA user_version = CURRENT_VERSION`
8. Return `Db`

Re-opening an existing DB with current version is a no-op (idempotent).

**Verify:** Unit tests:
- Fresh DB gets correct schema version
- Re-open existing DB is idempotent (no error, version unchanged)
- Tables exist after init

## Tasks

- [ ] [R1] Schema: create tables, WAL, FK pragma, user_version
- [ ] [R2] ID generation with collision retry
- [ ] [R3] Hierarchy: parent_id, derived parent status
- [ ] [R4] Dependency insert with cycle detection, removal
- [ ] [R5] Ready query implementation
- [ ] [R6] Status transition validation + auto-transitions
- [ ] [R7] Full CRUD API
- [ ] [R8] Database init with migration support

Checkpoint: after R1-R2, `cargo build && cargo test -- dag`. After R3-R6, test again. After R7-R8, full `cargo test`.

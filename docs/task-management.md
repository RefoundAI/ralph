# Task Management

Tasks are the atomic unit of work in Ralph. They live in a SQLite database
(`.ralph/progress.db`) organized as a directed acyclic graph (DAG) with two
orthogonal structuring mechanisms: a parent-child hierarchy for decomposition,
and cross-cutting dependency edges for execution ordering.

This document is a comprehensive reference for how the task management system
works internally, covering the data model, state machine, DAG operations, and
the mechanisms that keep the graph consistent.

## The Task Struct

Every task in Ralph is represented by a `Task` struct defined in
`src/dag/mod.rs` with 14 fields:

```rust
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub parent_id: Option<String>,
    pub feature_id: Option<String>,
    pub task_type: String,
    pub priority: i32,
    pub retry_count: i32,
    pub max_retries: i32,
    pub verification_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub claimed_by: Option<String>,
}
```

### Field Reference

| Field | Type | Description |
| --- | --- | --- |
| `id` | `String` | Unique identifier in the format `t-{6 hex chars}`. Generated from SHA-256 of `(timestamp_nanos \|\| atomic_counter)` in `dag/ids.rs`. |
| `title` | `String` | Short human-readable description of the task. |
| `description` | `String` | Full task details. Defaults to empty string if the database column is NULL. |
| `status` | `String` | Current lifecycle state. One of `pending`, `in_progress`, `done`, `blocked`, `failed`. Enforced by a SQL CHECK constraint. |
| `parent_id` | `Option<String>` | Points to the parent task via a self-referencing foreign key on the `tasks` table. `None` for root-level tasks. |
| `feature_id` | `Option<String>` | Links the task to a feature via a foreign key to the `features` table. `None` for standalone tasks without a feature association. |
| `task_type` | `String` | Either `"feature"` (belongs to a feature) or `"standalone"`. Enforced by a SQL CHECK constraint. Defaults to `"feature"` if the database column is NULL. |
| `priority` | `i32` | Integer priority where lower values indicate higher priority. Used for ordering ready tasks. Defaults to `0`. |
| `retry_count` | `i32` | Number of times this task has been retried after verification failure. Defaults to `0`. |
| `max_retries` | `i32` | Maximum allowed retry attempts. Defaults to `3`. Configurable per-task at creation time and globally via `--max-retries` or `.ralph.toml`. |
| `verification_status` | `Option<String>` | Tracks verification outcome. One of `pending`, `passed`, or `failed`. `None` when the task has not been through verification. |
| `created_at` | `String` | ISO 8601 / RFC 3339 timestamp set at creation time. |
| `updated_at` | `String` | ISO 8601 / RFC 3339 timestamp updated on every status transition. |
| `claimed_by` | `Option<String>` | The agent ID (format `agent-{8 hex chars}`) of the agent currently executing this task. Set when a task transitions to `in_progress`, cleared on completion, failure, or claim release. |

### Consistency Guarantees

All SQL queries that produce `Task` values go through two centralized helpers
in `src/dag/mod.rs`:

- **`TASK_COLUMNS`**: A constant string listing all 14 column names in the
  correct order for SELECT statements.
- **`task_from_row()`**: Maps a `rusqlite::Row` to a `Task` struct, handling
  nullable columns with the `row.get::<_, Option<T>>(N)?.unwrap_or(default)`
  pattern.

This ensures that adding or reordering columns only requires changes in one
place.

## Database Schema

The database uses schema version 2, stored in the SQLite `user_version` pragma.
It contains four tables. WAL journal mode and foreign key enforcement are
enabled at connection time in `dag/db.rs`.

### `tasks`

The core table. Stores all task data with CHECK constraints on `status`,
`task_type`, and `verification_status`.

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
    claimed_by TEXT,
    -- Added in schema v2:
    feature_id TEXT REFERENCES features(id),
    task_type TEXT DEFAULT 'feature'
        CHECK (task_type IN ('feature','standalone')),
    retry_count INTEGER DEFAULT 0,
    max_retries INTEGER DEFAULT 3,
    verification_status TEXT
        CHECK (verification_status IN ('pending','passed','failed'))
);
```

### `dependencies`

Stores directed edges between tasks. The composite primary key prevents
duplicate edges. The CHECK constraint prevents self-dependencies at the
database level.

```sql
CREATE TABLE dependencies (
    blocker_id TEXT NOT NULL REFERENCES tasks(id),
    blocked_id TEXT NOT NULL REFERENCES tasks(id),
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id != blocked_id)
);
```

A row `(A, B)` means "task A must complete before task B can start."

### `task_logs`

An append-only log of events for each task. Used to record failure reasons,
verification results, and other per-task history.

```sql
CREATE TABLE task_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    message TEXT NOT NULL,
    timestamp TEXT NOT NULL
);
```

The `LogEntry` struct exposed by the Rust API contains three fields: `task_id`,
`message`, and `timestamp`. The auto-increment `id` is not surfaced.

### `features`

Introduced in schema v2. Tracks high-level features that group related tasks.

```sql
CREATE TABLE features (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    spec_path TEXT,
    plan_path TEXT,
    status TEXT NOT NULL DEFAULT 'draft'
        CHECK (status IN ('draft','planned','ready','running','done','failed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### Schema Migrations

Migrations in `dag/db.rs` use version-range checks:

```rust
if from_version < 1 && to_version >= 1 { /* create v1 tables */ }
if from_version < 2 && to_version >= 2 { /* add v2 tables and columns */ }
```

This allows upgrading from any previous version to the current version in a
single pass. The `user_version` pragma is updated after all migrations succeed.

## Task Lifecycle

### Status State Machine

Valid transitions are defined in `src/dag/transitions.rs` via the
`is_valid_transition()` function:

```
pending -------> in_progress       claim_task()
pending -------> blocked           dependency added
in_progress ---> done              complete_task()
in_progress ---> failed            fail_task()
in_progress ---> pending           release_claim()
blocked -------> pending           auto-unblock (all blockers done)
failed --------> pending           retry_task()
```

Any transition not listed above is rejected with an error. The `done` status is
terminal under normal operations -- there is no valid transition out of `done`
through the state machine. (Force transitions bypass this restriction; see
[Force Transitions](#force-transitions) below.)

### Claiming and Releasing

The lifecycle of a single task execution follows this pattern:

1. **`claim_task(db, task_id, agent_id)`**: Transitions `pending` to
   `in_progress` and sets `claimed_by` to the agent's ID. This is an atomic
   operation -- the status transition and the `claimed_by` update happen in
   sequence on the same database connection.

2. The agent (Claude Code session) works on the task and emits a sigil.

3. One of three outcomes occurs:

   - **`complete_task(db, task_id)`**: Transitions `in_progress` to `done`,
     clears `claimed_by`, and triggers auto-transitions (unblocking dependents,
     completing parents).

   - **`fail_task(db, task_id, reason)`**: Transitions `in_progress` to
     `failed`, clears `claimed_by`, logs the failure reason to `task_logs`, and
     triggers auto-fail of the parent.

   - **`release_claim(db, task_id)`**: Called when Claude finishes an iteration
     without emitting a `<task-done>` or `<task-failed>` sigil. Checks that the
     task is currently `in_progress`, then transitions back to `pending` and
     clears `claimed_by`. The task becomes eligible for selection again in the
     next iteration.

### Auto-Transitions

Auto-transitions are side effects triggered by `set_task_status()` in
`src/dag/transitions.rs`. They maintain consistency between related tasks
without requiring explicit orchestration.

#### On Task Completion (`done`)

Two auto-transitions fire when a task is marked `done`:

1. **`auto_unblock_tasks()`**: Queries the `dependencies` table for all tasks
   that list the completed task as a blocker. For each such task that is
   currently in `blocked` status, it checks whether ALL of its blockers are now
   `done`. If so, the task transitions from `blocked` to `pending`, making it
   eligible for execution.

2. **`auto_complete_parent()`**: If the completed task has a `parent_id`, this
   function checks whether ALL children of that parent are in `done` status.
   If so, the parent is directly updated to `done` (bypassing
   `set_task_status()` to avoid infinite recursion). This then recursively
   checks the grandparent, and so on up the hierarchy.

#### On Task Failure (`failed`)

One auto-transition fires when a task is marked `failed`:

1. **`auto_fail_parent()`**: If the failed task has a `parent_id`, the parent
   is immediately set to `failed` (unless it is already `failed`). This
   cascades recursively upward -- a single leaf failure propagates all the way
   to the root of the hierarchy.

> [!IMPORTANT]
> Auto-complete and auto-fail of parents use direct SQL UPDATE statements
> rather than calling `set_task_status()` to avoid infinite recursion. The
> recursive call happens through `auto_complete_parent()` and
> `auto_fail_parent()` themselves, not through the general transition
> machinery.

### Force Transitions

Three functions in `src/dag/transitions.rs` allow manual intervention via the
CLI, bypassing the normal state machine by stepping through intermediate valid
states:

- **`force_complete_task(task_id)`**: Reaches `done` from any state.
  - `done` -- no-op
  - `in_progress` -- `in_progress` to `done`
  - `pending` -- `pending` to `in_progress` to `done`
  - `failed` -- `failed` to `pending` to `in_progress` to `done`
  - `blocked` -- `blocked` to `pending` to `in_progress` to `done`

- **`force_fail_task(task_id)`**: Reaches `failed` from any state.
  - `failed` -- no-op
  - `in_progress` -- `in_progress` to `failed`
  - `pending` -- `pending` to `in_progress` to `failed`
  - `blocked` -- `blocked` to `pending` to `in_progress` to `failed`
  - `done` -- Direct UPDATE (since `done` to `failed` is not a valid
    transition), then triggers `auto_fail_parent()`

- **`force_reset_task(task_id)`**: Resets to `pending` from any state, clearing
  `claimed_by`.
  - `pending` -- no-op
  - `in_progress`, `blocked`, `failed` -- Uses `set_task_status()` for the
    valid transition
  - `done` -- Direct UPDATE (since `done` to `pending` is not valid)

Because force transitions step through intermediate states using
`set_task_status()`, they trigger auto-transitions at each step. For example,
`force_complete_task()` on a `pending` task will trigger `auto_unblock_tasks()`
and `auto_complete_parent()` when it reaches `done`.

## Dependencies

### Two Structuring Mechanisms

Ralph's task graph uses two independent mechanisms for structuring work:

1. **Parent-child hierarchy** (via `parent_id`): Represents decomposition.
   A parent task is broken into smaller child tasks. Parent tasks are never
   directly executed -- only leaf nodes (tasks with no children) are eligible
   for the ready queue. Parent status is derived from children.

2. **Dependency edges** (via `dependencies` table): Represent execution
   ordering constraints. A dependency `(A, B)` means task A must reach `done`
   status before task B can be picked up. Dependencies can cross hierarchies --
   a child of one parent can depend on a child of another parent.

### Adding Dependencies

`add_dependency(db, blocker_id, blocked_id)` inserts a row into the
`dependencies` table after validating that the edge would not create a cycle.
Both task IDs must reference existing tasks (enforced by foreign keys).
Duplicate edges are rejected by the composite primary key. Self-dependencies
are rejected by both the application-level cycle check and the SQL CHECK
constraint.

### Cycle Detection

`would_create_cycle(blocker_id, blocked_id)` performs a breadth-first search
(BFS) starting from `blocked_id` and traversing existing dependency edges
forward (blocker to blocked). If the search reaches `blocker_id`, the proposed
edge would create a cycle and is rejected.

The algorithm:

1. Initialize a visited set and queue with `blocked_id`.
2. For each node in the queue, query `SELECT blocked_id FROM dependencies
   WHERE blocker_id = ?` to find all tasks that the current node blocks.
3. If any of those tasks is `blocker_id`, return `true` (cycle detected).
4. Otherwise, add unvisited nodes to the queue and continue.
5. If the queue is exhausted without finding `blocker_id`, return `false`.

This catches cycles of any length. For example, given the chain
`A -> B -> C -> D`, attempting to add `D -> A` triggers BFS from `A`, which
reaches `D` via `A -> B -> C -> D`, detecting the cycle.

> [!NOTE]
> Diamond-shaped dependencies are valid and explicitly supported. For example,
> `A -> B -> D` and `A -> C -> D` (where D depends on both B and C, which both
> depend on A) does not create a cycle.

### Removing Dependencies

`remove_dependency(db, blocker_id, blocked_id)` deletes the row from the
`dependencies` table. This operation does not trigger any auto-transitions --
if the blocked task is currently in `blocked` status, it remains blocked until
the ready-task query re-evaluates it.

## Ready Task Selection

A task is "ready" for execution when ALL of the following conditions hold:

1. **Status is `pending`**: Tasks in any other status are excluded.
2. **Leaf node**: The task has no children (no rows in `tasks` where
   `parent_id` equals this task's ID). Parent tasks are never directly
   executed.
3. **Parent not failed**: If the task has a `parent_id`, the parent's status
   must not be `failed`. This prevents executing children of a failed parent.
4. **All blockers done**: Every task listed as a blocker in the `dependencies`
   table must have status `done`. If any blocker is not done, the task is not
   ready.

The query is implemented as a single SQL statement in `get_ready_tasks()` in
`src/dag/mod.rs` using `NOT EXISTS` subqueries for conditions 2-4.

### Ordering

Ready tasks are ordered by:

1. `priority ASC` -- Lower values come first (higher priority).
2. `created_at ASC` -- Among equal-priority tasks, older tasks come first.

The run loop always picks the first task from this ordered list.

### Feature-Scoped Queries

`get_ready_tasks_for_feature(db, feature_id)` applies the same four conditions
with an additional `AND t.feature_id = ?` filter. This is used when
`ralph run` targets a specific feature.

### Standalone Task Queries

`get_standalone_tasks(db)` returns all tasks where `task_type = 'standalone'`,
ordered by priority and creation time. This query does not filter by status
and is used for listing, not for execution scheduling.

## Parent Status Derivation

Parent tasks in Ralph have a dual nature: they have a stored `status` column
in the database, but their effective status is derived from their children when
queried through `get_task_status()` in `src/dag/tasks.rs`.

### `get_task_status(conn, task_id)`

- If the task has no children (leaf node), returns the stored `status` value.
- If the task has children, delegates to `compute_parent_status()`.

### `compute_parent_status(conn, parent_id)`

Recursively evaluates children and applies these rules in order:

1. If any child's derived status is `"failed"`, the parent is `"failed"`.
2. If all children's derived statuses are `"done"`, the parent is `"done"`.
3. If any child's derived status is `"in_progress"`, the parent is
   `"in_progress"`.
4. Otherwise, the parent is `"pending"`.

The recursion means that a three-level hierarchy (grandparent -> parent ->
children) is correctly evaluated: if all leaf-level children are `done`, both
the parent and grandparent derive a `done` status.

> [!NOTE]
> `compute_parent_status()` is used for read-only status queries (e.g.,
> `ralph task show`). The auto-transition functions (`auto_complete_parent`,
> `auto_fail_parent`) handle the write-side by directly updating the database
> when children complete or fail.

## Retry Mechanism

The retry mechanism works in conjunction with the [verification agent] to
handle tasks that were completed but did not pass verification.

### Retry Flow

1. Claude emits `<task-done>{id}</task-done>`.
2. The run loop calls `handle_task_done()`, which spawns the verification
   agent.
3. The verification agent inspects the codebase, runs tests, and emits either
   `<verify-pass/>` or `<verify-fail>reason</verify-fail>`.
4. On `<verify-fail>`:
   - If `retry_count < max_retries`: Call `retry_task()`.
   - If `retry_count >= max_retries`: Call `fail_task()` with the verification
     failure reason. The task is permanently failed, which cascades to the
     parent via auto-fail.

### `retry_task(db, task_id)`

This function performs two operations:

1. Transitions the task from `failed` to `pending` (via `set_task_status()`).
2. Increments `retry_count`, sets `verification_status = 'failed'`, and
   clears `claimed_by`.

The task then re-enters the ready queue. On the next iteration, the run loop
detects `retry_count > 0` and builds a `RetryInfo` struct containing:

- `attempt`: The current attempt number (`retry_count + 1`).
- `max_retries`: The configured maximum.
- `previous_failure_reason`: The most recent message from `task_logs` for
  this task.

This retry context is included in the system prompt for the next Claude
session, so the agent can learn from the previous failure.

### Configuration

- **Per-task**: The `max_retries` field is set at task creation time (default
  `3`).
- **Global**: The `--max-retries` CLI flag or `execution.max_retries` in
  `.ralph.toml` sets the default for the run.
- **Disable verification**: `--no-verify` skips the verification agent
  entirely, so tasks complete immediately on `<task-done>` without retries.

[verification agent]: ../src/verification.rs

## Task Trees

`get_task_tree(db, root_id)` in `src/dag/crud.rs` performs a breadth-first
traversal starting from a root task and collecting all descendants into a flat
list. The traversal follows `parent_id` edges (not dependency edges).

The algorithm:

1. Fetch the root task and add it to the result list.
2. Initialize a queue with the root task's ID.
3. Pop a task ID from the queue, query all tasks where `parent_id` equals
   that ID.
4. Add each child to the result list and queue.
5. Repeat until the queue is empty.

The `ralph task tree` CLI command renders this flat list as an indented tree
with colored status indicators.

## CRUD Operations

All CRUD operations live in `src/dag/crud.rs`.

### Create

**`create_task(db, title, description, parent_id, priority)`**: Creates a task
with default `task_type = "feature"` and `max_retries = 3`. Delegates to
`create_task_with_feature()`.

**`create_task_with_feature(db, title, description, parent_id, priority,
feature_id, task_type, max_retries)`**: The full creation function. Validates
that the parent task exists if `parent_id` is specified (returns an error
otherwise). Generates a unique task ID using `generate_and_insert_task_id()`
with a retry loop for collision handling. Sets `status = "pending"`,
`retry_count = 0`, `verification_status = None`, and `claimed_by = None`.

### Read

**`get_task(db, id)`**: Fetches a single task by ID using `TASK_COLUMNS` and
`task_from_row()`.

**`get_all_tasks(db)`**: Returns all tasks ordered by `priority ASC,
created_at ASC`.

**`get_all_tasks_for_feature(db, feature_id)`**: Returns all tasks for a
specific feature, ordered by `priority ASC, created_at ASC`.

### Update

**`update_task(db, id, fields)`**: Accepts a `TaskUpdate` struct with optional
`title`, `description`, and `priority` fields. Builds a dynamic SQL UPDATE
statement including only the fields that are `Some`. Always updates
`updated_at`. Returns the updated task. If no fields are set, returns the task
unchanged without touching the database.

```rust
pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<i32>,
}
```

### Delete

**`delete_task(db, id)`**: Deletes a task with the following safety checks and
cascading behavior:

1. **Existence check**: Returns an error if the task does not exist.
2. **Blocker check**: Returns an error if the task is referenced as a
   `blocker_id` in the `dependencies` table. You must remove dependent tasks
   or their dependency edges before deleting a blocker.
3. **Cascade children**: Recursively deletes all child tasks (tasks where
   `parent_id` equals this task's ID). Each child deletion follows the same
   safety checks.
4. **Clean up edges**: Deletes all rows in `dependencies` where this task is
   the `blocked_id`.
5. **Clean up logs**: Deletes all rows in `task_logs` for this task.
6. **Delete the task**: Removes the row from `tasks`.

### Logs

**`add_log(db, task_id, message)`**: Inserts a timestamped log entry for a
task. Returns an error if the task does not exist.

**`get_task_logs(db, task_id)`**: Returns all log entries for a task, ordered
by `timestamp ASC`.

### Dependency Queries

**`get_task_blockers(db, task_id)`**: Returns all tasks that block the given
task (its prerequisites). Joins `dependencies` on `blocker_id` and returns
full `Task` structs.

**`get_tasks_blocked_by(db, task_id)`**: Returns all tasks that are blocked by
the given task (its dependents). Joins `dependencies` on `blocked_id` and
returns full `Task` structs.

## ID Generation

Task and feature IDs are generated in `src/dag/ids.rs` using SHA-256 hashing.

### Algorithm

1. Read the current system time as nanoseconds since the UNIX epoch.
2. Atomically increment a global `AtomicU64` counter (`COUNTER`).
3. Compute `SHA-256(timestamp_nanos_le_bytes || counter_le_bytes)`.
4. Take the first 3 bytes of the hash (6 hex characters).
5. Prepend the type prefix.

### ID Formats

| Type | Format | Example |
| --- | --- | --- |
| Task | `t-{6 hex}` | `t-a3f1c9` |
| Feature | `f-{6 hex}` | `f-b7e204` |
| Agent | `agent-{8 hex}` | `agent-1f3a7b9c` |

Agent IDs use a different mechanism: `DefaultHasher` over
`(timestamp_nanos, process_id)`, taking 8 hex characters from the hash.

### Collision Handling

With 6 hex characters (24 bits), the ID space is 16,777,216 values. Collisions
are theoretically possible, so `generate_and_insert_task_id()` wraps the
generation and INSERT in a retry loop:

1. Generate an ID.
2. Attempt the INSERT.
3. On `ConstraintViolation` (UNIQUE conflict), generate a new ID and retry.
4. After `max_retries` (default 10) failed attempts, return an error.

The atomic counter ensures that successive calls within the same nanosecond
produce different hashes, making collisions extremely unlikely in practice.

## Task Counts

The `TaskCounts` struct provides a summary view of the DAG:

```rust
pub struct TaskCounts {
    pub total: usize,
    pub ready: usize,
    pub done: usize,
    pub blocked: usize,
}
```

`get_task_counts(db)` computes these by running:

- `total`: `SELECT COUNT(*) FROM tasks`
- `ready`: Length of the result from `get_ready_tasks()` (the full ready-task
  query with all four conditions)
- `done`: `SELECT COUNT(*) FROM tasks WHERE status = 'done'`
- `blocked`: `SELECT COUNT(*) FROM tasks WHERE status = 'blocked'`

`get_feature_task_counts(db, feature_id)` computes the same counts scoped to
a specific feature.

## Resolution Check

`all_resolved(db)` returns `true` when every task in the database is in either
`done` or `failed` status:

```sql
SELECT COUNT(*) FROM tasks WHERE status NOT IN ('done', 'failed')
```

If this returns 0, the DAG is fully resolved and the run loop exits with
`Outcome::Complete`.

## Integration with the Run Loop

The run loop in `src/run_loop.rs` orchestrates task management through this
sequence on each iteration:

1. **`get_scoped_ready_tasks()`** fetches ready tasks filtered by the run
   target (feature, task ID, or global).
2. **`claim_task()`** takes the first ready task.
3. **`build_iteration_context()`** assembles parent context, completed
   blocker summaries, retry info, and skills for the system prompt.
4. Claude runs and emits sigils.
5. **Sigil handling**:
   - `<task-done>` triggers `handle_task_done()`, which optionally runs
     verification and then calls `complete_task()` or `retry_task()`.
   - `<task-failed>` calls `fail_task()`.
   - No sigil calls `release_claim()`.
6. **`all_resolved()`** checks if the loop should exit.

The completed-blockers context is particularly notable: when a task is picked
up for execution, the run loop queries all of its dependency predecessors that
are in `done` status and includes their title and most recent log entry (or
description) in the system prompt. This gives Claude context about work that
was done before its task became ready.

## CLI Commands

The following `ralph task` subcommands interact with the task management
system:

| Command | Function | Notes |
| --- | --- | --- |
| `ralph task add <TITLE>` | `create_task()` / `create_task_with_feature()` | Non-interactive, scriptable |
| `ralph task create` | Interactive Claude session | Produces `ralph task add` / `ralph task deps add` commands |
| `ralph task show <ID>` | `get_task()` | Supports `--json` output |
| `ralph task list` | `get_all_tasks()` / `get_all_tasks_for_feature()` | Filterable, supports `--json` |
| `ralph task update <ID>` | `update_task()` | Updates title, description, priority |
| `ralph task delete <ID>` | `delete_task()` | Rejects if task is a blocker |
| `ralph task done <ID>` | `force_complete_task()` | Steps through intermediate states |
| `ralph task fail <ID>` | `force_fail_task()` | Steps through intermediate states |
| `ralph task reset <ID>` | `force_reset_task()` | Resets to pending |
| `ralph task log <ID>` | `add_log()` / `get_task_logs()` | Add or view log entries |
| `ralph task deps add <A> <B>` | `add_dependency()` | A must complete before B |
| `ralph task deps rm <A> <B>` | `remove_dependency()` | Remove dependency edge |
| `ralph task deps list <ID>` | `get_task_blockers()` / `get_tasks_blocked_by()` | Show both directions |
| `ralph task tree <ID>` | `get_task_tree()` | Indented tree with status colors |

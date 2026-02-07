# DAG-Driven Loop

Replace the flat progress-file loop in `run_loop.rs` with a DAG-driven task loop using `.ralph/progress.db`.

## Outcome Enum

### R1: New Outcome variants

Update `Outcome` in `run_loop.rs`:

```rust
pub enum Outcome {
    Complete,      // all DAG tasks done
    Failure,       // <promise>FAILURE</promise> emitted
    LimitReached,  // iteration limit hit
    Blocked,       // no ready tasks, but incomplete tasks exist
    NoPlan,        // DAG is empty, user must run `ralph plan`
}
```

Exit code mapping in `main.rs`:
- `Complete` -> 0
- `Failure` -> 1
- `LimitReached` -> 0
- `Blocked` -> 2
- `NoPlan` -> 3

**Verify:** Unit test: each variant exists. Each maps to correct exit code. `Blocked` -> `ExitCode::from(2)`, `NoPlan` -> `ExitCode::from(3)`.

## Task Sigils

### R2: Parse `<task-done>` and `<task-failed>` sigils

New sigils in Claude's output:
- `<task-done>t-a1b2c3</task-done>` -- task completed
- `<task-failed>t-a1b2c3</task-failed>` -- task failed

Parse identically to `<next-model>` sigil parsing in `events.rs`. Extract the task ID string from inside the tags.

Add to `ResultEvent`:
- `task_done: Option<String>`
- `task_failed: Option<String>`

At most one of these is set per result. If both present, `task_done` wins (optimistic).

Validation: if emitted task ID does not match the assigned task ID (passed to Claude in context), log warning via `eprintln!` but still process the sigil.

**Verify:** Unit tests in `events.rs`:
- `<task-done>t-abc123</task-done>` -> `task_done = Some("t-abc123")`
- `<task-failed>t-abc123</task-failed>` -> `task_failed = Some("t-abc123")`
- No sigil -> both `None`
- Malformed (no closing tag, empty content) -> `None`
- Both present -> `task_done` wins
- Whitespace inside tags is trimmed

## Agent ID

### R3: Generate agent ID per run

Each `ralph run` invocation generates a unique agent ID: `agent-{8 hex chars}` (e.g., `agent-a1b2c3d4`). Use `rand` or hash of PID + timestamp -- implementer's choice.

Store as `agent_id: String` on `Config` (set in `Config::from_args` or at `run()` entry).

Passed to `claim_task()` when claiming a task from the DAG.

**Verify:** Unit test: agent ID matches `agent-[0-9a-f]{8}` regex. Non-empty. Two calls produce different IDs.

## Loop Restructure

### R4: DAG-driven loop

Replace `run()` in `run_loop.rs`. New flow:

```
1. Open .ralph/progress.db (init schema if needed)
2. Print DAG summary: "{total} tasks, {ready} ready, {done} done, {blocked} blocked"
3. Query get_ready_tasks(), pick first
4. If no ready tasks AND no tasks at all -> return Outcome::NoPlan
5. If no ready tasks BUT incomplete tasks exist -> return Outcome::Blocked
6. claim_task(task_id, agent_id)
7. Print: "[iter {n}] Working on: {task_id} -- {task_title}"
8. Build task context block for Claude (task ID, title, description, parent info, completed siblings)
9. Run Claude with task context injected into system prompt
10. Parse result for task-done / task-failed sigils + next-model hint
11. If task-done: complete_task(task_id)
12. If task-failed: fail_task(task_id, reason)
13. If neither sigil: warn, treat as incomplete (release claim)
14. Print task outcome
15. Check: all DAG tasks resolved? -> Outcome::Complete
16. Check: iteration limit reached? -> Outcome::LimitReached
17. Select model for next iteration (strategy + hint)
18. config = config.next_iteration()
19. Repeat from step 3
```

The `<promise>FAILURE</promise>` sigil still causes immediate `Outcome::Failure` (step 10, short-circuit before DAG update).

**Verify:** Integration test with mock DAG (in-memory or temp SQLite):
- 3 tasks: A (no deps), B (depends on A), C (depends on B)
- Loop processes A first, then B, then C
- Loop returns `Outcome::Complete` after all three
- If B fails, C is blocked; loop returns `Outcome::Blocked`
- Empty DAG returns `Outcome::NoPlan`

### R5: Remove old progress file logic

Remove from `run_loop.rs`:
- `touch_file(&config.progress_file)`
- `touch_file(&config.prompt_file)` (prompt file existence is checked elsewhere now)
- `has_specs()` function
- `run_interactive_specs()` function
- All references to `config.progress_file`
- All references to `config.specs_dir`

Model override logging (`strategy::log_model_override`) should write to a `task_logs` table in the DAG DB instead of the progress file. Update `log_model_override` signature to accept a DB handle instead of a file path.

**Verify:** `grep -r "progress_file" src/run_loop.rs` -> 0 hits. `grep -r "specs_dir" src/run_loop.rs` -> 0 hits. `grep -r "touch_file" src/run_loop.rs` -> 0 hits. `cargo build` clean.

## Display

### R6: Loop iteration display

At loop start (before first iteration):
```
DAG: 12 tasks, 3 ready, 0 done, 0 blocked
```

Each iteration:
```
[iter 1] Working on: t-a1b2c3 -- Implement CLI parser
```

After each iteration:
```
[iter 1] Done: t-a1b2c3
```
or
```
[iter 1] Failed: t-a1b2c3
```

Use `colored` crate for formatting (already a dependency). Task ID in cyan, status in green/red.

**Verify:** Output contains task ID and title. DAG summary printed at start. `cargo build` clean.

## DAG Interface

### R7: Expected DAG trait/API

The loop assumes these functions exist (implemented in a separate `dag` module, out of scope for this spec):

```rust
fn open_db(path: &str) -> Result<Db>
fn get_ready_tasks(db: &Db) -> Result<Vec<Task>>
fn get_task_counts(db: &Db) -> Result<TaskCounts>  // total, ready, done, blocked
fn claim_task(db: &Db, task_id: &str, agent_id: &str) -> Result<()>
fn complete_task(db: &Db, task_id: &str) -> Result<()>
fn fail_task(db: &Db, task_id: &str, reason: &str) -> Result<()>
fn all_resolved(db: &Db) -> Result<bool>
fn release_claim(db: &Db, task_id: &str) -> Result<()>
```

`Task` struct (minimum fields the loop needs):
```rust
struct Task {
    id: String,        // e.g. "t-a1b2c3"
    title: String,
    description: String,
    parent_id: Option<String>,
}
```

For this spec, stub these with `todo!()` or a trait so the loop compiles. Actual SQLite implementation is a separate spec.

**Verify:** `cargo build` clean with stubs. Loop code compiles against the API.

## Tasks

- [ ] [R1] Add `Blocked` and `NoPlan` variants to `Outcome`; update exit code mapping in `main.rs`
- [ ] [R2] Add `task_done`/`task_failed` sigil parsing to `events.rs`; add fields to `ResultEvent`
- [ ] [R3] Generate agent ID in config/run entry
- [ ] [R7] Stub DAG API (trait or module with `todo!()` bodies)
- [ ] [R4] Rewrite `run()` to DAG-driven loop
- [ ] [R5] Remove old progress file / specs dir / touch_file logic from `run_loop.rs`
- [ ] [R6] Add DAG summary and per-iteration task display

Checkpoint: after R1+R2+R3, `cargo build && cargo test`. After R7+R4+R5, `cargo build`. After R6, full `cargo test`.

# System Prompt Restructure

Replace `build_system_prompt()` and `build_claude_args()` in `src/claude/client.rs` to support DAG-based task assignment, multiple specs directories, and new task sigils. Remove all `progress.txt` / progress file references.

## Dependencies

Depends on: `dag-tracker.md` (R5 ready query, R7 `Task` struct), `config.md` (R1-R3 `RalphConfig` with `specs.dirs`).

## Specs Directory Awareness

### R1: Enumerate specs dirs in system prompt

`build_system_prompt` takes a `specs_dirs: &[String]` parameter (resolved paths from `RalphConfig`). The system prompt MUST list every directory path so Claude reads all spec files at iteration start.

Specs are read-only -- instruct Claude to never modify spec files.

Do NOT pass spec files as `@{file}` CLI args. Claude reads them itself using the listed directory paths.

**Verify:** Unit test: `build_system_prompt` with `specs_dirs = &[".ralph/specs".into()]` -- output contains `.ralph/specs`. Test with `specs_dirs = &["specs/api".into(), "specs/infra".into()]` -- output contains both paths. Output contains "do not modify" or equivalent for specs.

### R2: DAG task instructions replace progress file

Remove from system prompt:
- Any mention of `progress.txt`, `progress_file`, or "progress file"
- "Append to", "Update", or "Read" progress file instructions
- "Find the SINGLE highest-priority incomplete task" (ralph assigns tasks now)

Add to system prompt:
- Ralph assigns one task per iteration via a task context block
- Claude works on the assigned task only -- does not pick or reorder tasks
- Signal task done: `<task-done>{task_id}</task-done>`
- Signal task failed: `<task-failed>{task_id}</task-failed>` with reason in output text

Keep in system prompt:
- ONE TASK PER LOOP rule
- "Do not assume code exists" rule
- "Do not implement placeholders" rule
- "If tests fail, fix them" rule
- Commit changes instruction
- `AGENTS.md` update instruction

**Verify:** Unit test: system prompt contains `<task-done>` and `<task-failed>`. System prompt does NOT contain `progress.txt`, `progress_file`, or `append.*progress` (regex). System prompt still contains "ONE TASK PER LOOP".

## Task Context Block

### R3: `TaskInfo` struct

Define in `src/claude/client.rs` (or a new `src/claude/task_context.rs`):

```rust
pub struct TaskInfo {
    pub task_id: String,
    pub title: String,
    pub description: String,
    pub parent: Option<ParentContext>,
    pub completed_blockers: Vec<BlockerContext>,
    pub specs_dirs: Vec<String>,
}

pub struct ParentContext {
    pub title: String,
    pub description: String,
}

pub struct BlockerContext {
    pub task_id: String,
    pub title: String,
    pub summary: String,
}
```

**Verify:** Struct compiles. Fields match DAG tracker `Task` shape.

### R4: `build_task_context`

`pub fn build_task_context(task: &TaskInfo) -> String`

Output format:

```
## Assigned Task

**ID:** {task_id}
**Title:** {title}

### Description
{description}

### Parent Context
**Parent:** {parent.title}
{parent.description}

### Completed Prerequisites
- [{blocker.task_id}] {blocker.title}: {blocker.summary}

### Reference Specs
Read all files in: {specs_dirs comma-joined}
```

Omit "Parent Context" section when `parent` is `None`. Omit "Completed Prerequisites" section when `completed_blockers` is empty.

**Verify:** Unit tests:
- Task with all fields populated -- output contains all sections
- Task with no parent -- output omits "Parent Context" section entirely
- Task with no blockers -- output omits "Completed Prerequisites" section entirely
- Task with 2 blockers -- output contains both blocker lines
- Output contains task_id, title, description verbatim

### R5: Wire task context into `build_claude_args`

`build_claude_args` takes `Option<&TaskInfo>` in addition to `&Config`.

When `Some(task)`:
- Call `build_task_context(task)` and append result as a positional arg after `@{prompt_file}`
- Remove `format!("@{}", config.progress_file)` arg (progress file no longer passed)

When `None` (e.g., interactive/specs mode):
- Only pass `@{prompt_file}`, no task context, no progress file

**Verify:** Unit test: `build_claude_args` with `Some(task)` -- args contain the task context string, do NOT contain `@progress.txt` or any `@.*progress`. With `None` -- args contain `@{prompt_file}` only.

## New Sigils

### R6: Parse `<task-done>` and `<task-failed>`

Add to `src/claude/events.rs`:

```rust
pub fn parse_task_done(text: &str) -> Option<String>
pub fn parse_task_failed(text: &str) -> Option<String>
```

Same parsing pattern as `parse_next_model_hint`: find start tag, find end tag, extract content, trim whitespace. Accept any non-empty content as a valid task ID (no allowlist -- IDs are opaque strings).

Add fields to `ResultEvent`:
```rust
pub task_done: Option<String>,
pub task_failed: Option<String>,
```

Wire parsing into `stream_output` in `client.rs`: after extracting `next_model_hint`, also extract `task_done` and `task_failed` from result text. Populate `ResultEvent` fields.

**Verify:** Unit tests in `events.rs`:
- `parse_task_done("<task-done>t-abc123</task-done>")` -> `Some("t-abc123")`
- `parse_task_done("<task-done> t-abc123 </task-done>")` -> `Some("t-abc123")` (trim)
- `parse_task_done("no sigil")` -> `None`
- `parse_task_done("<task-done></task-done>")` -> `None` (empty)
- `parse_task_done("<task-done>t-abc123")` -> `None` (no closing tag)
- Same set for `parse_task_failed`
- `ResultEvent` default has `task_done: None, task_failed: None`

### R7: Document all sigils in system prompt

System prompt MUST contain documentation for all five sigils:
- `<promise>COMPLETE</promise>` -- entire DAG is done; ralph verifies against DB
- `<promise>FAILURE</promise>` -- unrecoverable failure
- `<task-done>{task_id}</task-done>` -- assigned task completed successfully
- `<task-failed>{task_id}</task-failed>` -- assigned task cannot be completed
- `<next-model>opus|sonnet|haiku</next-model>` -- model hint for next iteration

Instruct Claude: emit `<task-done>` or `<task-failed>` every iteration. Emit `<promise>COMPLETE</promise>` only when the entire project is done (ralph will verify). `<promise>FAILURE</promise>` only for unrecoverable situations.

**Verify:** Unit test: system prompt contains all five sigil strings (literal tag examples).

## Cleanup

### R8: Remove progress file from `build_claude_args`

Remove `format!("@{}", config.progress_file)` from the args vec.

If `Config` still has `progress_file` at this point (removal may be done by `config.md` R5), do not reference it in `build_claude_args` or `build_system_prompt`.

**Verify:** Unit test: `build_claude_args` output does not contain any arg matching `@.*progress`. `build_system_prompt` output does not contain "progress".

### R9: Update `run_loop.rs` sigil handling

In `run_loop.rs`, after Claude returns:
- Check `result.task_done` -- if `Some(id)`, call `dag::complete_task(db, &id)`
- Check `result.task_failed` -- if `Some(id)`, call `dag::fail_task(db, &id, reason)` (extract reason from output or use default)
- Check `result.is_complete()` -- verify against DAG (all tasks done?) before accepting
- Check `result.is_failure()` -- unchanged behavior

**Verify:** Integration test or unit test with mock: task_done sigil triggers `complete_task`. task_failed sigil triggers `fail_task`. COMPLETE sigil is verified against DAG state.

## Tasks

Implement in order. Run `cargo build && cargo test` after each.

- [ ] [R6] Add `parse_task_done`, `parse_task_failed` to `events.rs`; add fields to `ResultEvent`
- [ ] [R3] Define `TaskInfo`, `ParentContext`, `BlockerContext` structs
- [ ] [R4] Implement `build_task_context`
- [ ] [R2] Rewrite `build_system_prompt` -- remove progress refs, add DAG instructions
- [ ] [R1] Add `specs_dirs` param to `build_system_prompt`; enumerate dirs in output
- [ ] [R7] Add all five sigil docs to system prompt
- [ ] [R8] Remove progress file from `build_claude_args`
- [ ] [R5] Wire `TaskInfo` into `build_claude_args`; pass task context as positional arg
- [ ] [R9] Update `run_loop.rs` to handle `task_done`/`task_failed` sigils via DAG

Checkpoint: after R6, sigil parsing tests pass. After R3-R4, task context tests pass. After R2-R1-R7-R8, system prompt tests pass. After R5-R9, full `cargo build && cargo test`.

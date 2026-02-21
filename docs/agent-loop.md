# Agent Loop

Ralph's core is a synchronous iteration loop that picks ready tasks from a DAG,
spawns Claude Code sessions, and processes results. This document explains how
the loop works in detail.

The implementation lives in `src/run_loop.rs`. The public entry point is the
`run()` function, which takes a `Config` and returns an `Outcome`.

## Outcome

Every run terminates with one of five outcomes:

| Outcome        | Meaning                                                            |
| -------------- | ------------------------------------------------------------------ |
| `Complete`     | All DAG tasks resolved (done or failed, none pending/in-progress). |
| `Failure`      | Claude emitted `<promise>FAILURE</promise>`.                       |
| `LimitReached` | Hit the configured iteration limit before all tasks resolved.      |
| `Blocked`      | No ready tasks available but incomplete tasks remain.              |
| `NoPlan`       | The DAG is empty -- no tasks exist at all.                         |

`Blocked` typically indicates a dependency deadlock: remaining tasks depend on
failed blockers that will never become `done`, so nothing can proceed. It can
also occur when all remaining tasks are claimed by another agent.

`NoPlan` means the user has not yet run `ralph feature build` or `ralph task
add` to populate the DAG.

## Loop Initialization

Before the first iteration, `run()` performs two setup steps:

1. **Open the database.** Constructs the path
   `<project_root>/.ralph/progress.db` and calls `dag::open_db()`. This opens
   (or creates) the SQLite database with WAL mode and foreign keys enabled.

2. **Resolve feature context.** Calls `resolve_feature_context()` which
   dispatches on the `RunTarget`:
   - `Feature(name)` -- Looks up the feature in the database, then reads
     `.ralph/features/<name>/spec.md` and `.ralph/features/<name>/plan.md` from
     disk. Returns `(feature_id, spec_content, plan_content)`.
   - `Task(id)` -- Looks up the task. If it has a `feature_id`, loads that
     feature's spec and plan. Otherwise returns `(None, None, None)`.
   - `None` -- Returns `(None, None, None)`.

   The spec and plan content is loaded once and reused across all iterations.

## Iteration Lifecycle

Each iteration follows ten steps. The diagram below shows the high-level flow;
the sections that follow explain each step.

```
   +-------------------------------------------+
   |            ITERATION START                 |
   +-------------------------------------------+
              |
              v
   [1] Find ready tasks (scoped to run target)
              |
              v
   [2] Check terminal conditions
        |         |           |
        v         v           v
     NoPlan    Blocked     Complete
        |         |           |
       exit      exit        exit
                              |
              +---------------+
              |  (tasks remain)
              v
   [3] Claim task (pending -> in_progress)
              |
              v
   [4] Build iteration context
        (task, parent, blockers, retry, skills, spec, plan)
              |
              v
   [5] Select model (strategy + hint)
              |
              v
   [6] Spawn Claude Code session
        (direct or sandboxed, streaming NDJSON)
              |
              v
   [7] Parse result (sigils from output)
              |
              v
   [8] Handle outcome
        |              |              |
        v              v              v
     task-done      task-failed    no sigil
     -> verify?     -> fail_task   -> release_claim
     -> complete                    (back to pending)
     -> retry
              |
              v
   [9] Check loop exit conditions
        |                |
        v                v
     Complete        LimitReached
        |                |
       exit             exit
              |
              v
  [10] Advance to next iteration
        (increment counter, select model, continue)
```

### Step 1: Find Ready Tasks

`get_scoped_ready_tasks()` queries the database for tasks eligible to run. The
scope depends on the `RunTarget`:

- **`Feature(name)`** -- Calls `dag::get_ready_tasks_for_feature(feature_id)`.
  Only returns tasks whose `feature_id` matches.
- **`Task(id)`** -- Gets all ready tasks, then filters to the single task with
  the matching ID.
- **`None`** -- Calls `dag::get_ready_tasks()` with no scope filter.

A task is considered **ready** when all four conditions hold:

1. Status is `pending`.
2. It is a leaf node (no child tasks).
3. Its parent (if any) is not `failed`.
4. All dependency blockers have status `done`.

Ready tasks are ordered by `priority ASC, created_at ASC`. Lower priority
numbers run first; ties are broken by creation time (oldest first).

The SQL query (simplified) looks like:

```sql
SELECT *
FROM tasks t
WHERE t.status = 'pending'
  AND NOT EXISTS (SELECT 1 FROM tasks c WHERE c.parent_id = t.id)
  AND (t.parent_id IS NULL
       OR NOT EXISTS (SELECT 1 FROM tasks p
                      WHERE p.id = t.parent_id AND p.status = 'failed'))
  AND NOT EXISTS (SELECT 1 FROM dependencies d
                  JOIN tasks b ON d.blocker_id = b.id
                  WHERE d.blocked_id = t.id AND b.status != 'done')
ORDER BY t.priority ASC, t.created_at ASC
```

### Step 2: Check Terminal Conditions

The loop checks three conditions in order:

1. **Empty DAG.** If `counts.total == 0`, return `Outcome::NoPlan`.
2. **No ready tasks.** If `ready_tasks` is empty:
   - Check `dag::all_resolved()`. If all tasks are done or failed, return
     `Outcome::Complete`.
   - Otherwise, return `Outcome::Blocked`.
3. **All resolved.** If `all_resolved()` returns true even though ready tasks
   exist (edge case after concurrent completions), return `Outcome::Complete`.

### Step 3: Claim the Task

The loop picks `ready_tasks[0]` -- the highest-priority, oldest-created ready
task. It calls `dag::claim_task(task_id, agent_id)` which:

1. Transitions the task from `pending` to `in_progress` via
   `set_task_status()`.
2. Sets the `claimed_by` field to the agent's unique ID (format:
   `agent-{8 hex}`).

The agent ID is generated once at startup from a hash of the current timestamp
and process ID. It persists across all iterations within a single run.

### Step 4: Build Iteration Context

`build_iteration_context()` assembles an `IterationContext` struct that the
system prompt will be built from:

```
IterationContext
  +-- task: TaskInfo
  |     +-- task_id
  |     +-- title
  |     +-- description
  |     +-- parent: Option<ParentContext>
  |     |     +-- title
  |     |     +-- description
  |     +-- completed_blockers: Vec<BlockerContext>
  |     |     +-- task_id
  |     |     +-- title
  |     |     +-- summary (last log entry or description)
  +-- spec_content: Option<String>
  +-- plan_content: Option<String>
  +-- retry_info: Option<RetryInfo>
  |     +-- attempt (retry_count + 1)
  |     +-- max_retries
  |     +-- previous_failure_reason
  +-- skills_summary: Vec<(name, description)>
  +-- learn: bool
```

Key details:

- **Parent context.** If the task has a `parent_id`, loads the parent task and
  extracts its title and description. This gives Claude broader context about
  the work area.

- **Completed blockers.** Queries the `dependencies` table for tasks that block
  this one and have status `done`. For each, retrieves the title and the most
  recent log entry (falling back to the description). This tells Claude what
  prerequisite work was already completed.

- **Retry info.** If `retry_count > 0`, builds a `RetryInfo` with the current
  attempt number, the max retries allowed, and the last failure reason from
  `task_logs`. This is included in the system prompt so Claude knows what went
  wrong on the previous attempt.

- **Skills discovery.** `discover_skills()` scans
  `.ralph/skills/*/SKILL.md`. For each skill directory, it reads the YAML
  frontmatter and extracts the `description` field. Returns a list of
  `(name, description)` pairs.

- **Spec/plan content.** Passed through from the feature context resolved at
  loop initialization. Not re-read from disk each iteration.

### Step 5: Select Model

`strategy::select_model()` determines which Claude model to use. The selection
depends on the configured `ModelStrategy`:

| Strategy           | Behavior                                                |
| ------------------ | ------------------------------------------------------- |
| `Fixed`            | Always returns the `--model` value.                     |
| `CostOptimized`    | Default sonnet; escalates to opus on error signals in   |
|                    | progress DB; drops to haiku after 3+ clean completions. |
| `Escalate`         | Starts at haiku. Escalates to sonnet on moderate        |
|                    | distress, opus on severe. Never auto-de-escalates.      |
| `PlanThenExecute`  | Opus for iteration 1, sonnet for all subsequent.        |

Claude can override any strategy for the next iteration by emitting
`<next-model>opus|sonnet|haiku</next-model>` in its output. The hint is
extracted during result parsing (step 7) and passed to `select_model()` at the
start of the next iteration (step 10).

For the `Escalate` strategy, a Claude hint can also de-escalate the level
(e.g., hinting `haiku` when currently at opus). This is the only way to move
downward in the escalation ladder.

> [!NOTE]
> Model selection happens at two points. The **initial** model is determined by
> the strategy when `Config` is created. Subsequent iterations run
> `select_model()` after advancing the iteration counter in step 10.

### Step 6: Spawn Claude Code

`claude::client::run()` spawns a Claude Code CLI process. It first builds the
system prompt and CLI arguments, then dispatches to one of two execution modes.

**CLI arguments built by `build_claude_args()`:**

```
claude --print --verbose \
       --output-format stream-json \
       --no-session-persistence \
       --model <current_model> \
       --system-prompt <system_prompt> \
       @<prompt_file> \
       [--dangerously-skip-permissions | --allowed-tools <tools>]
```

When sandboxing is enabled, `--dangerously-skip-permissions` is used (the
sandbox itself restricts file access). When sandboxing is disabled, an explicit
`--allowed-tools` list is passed instead.

**Execution modes:**

- **Direct** (`run_direct()`). Spawns `claude` as a child process. stdout is
  piped for streaming; stderr is drained on a background thread to prevent pipe
  buffer deadlocks.

- **Sandboxed** (`run_sandboxed()`). Wraps the invocation in macOS
  `sandbox-exec`:

  ```
  sandbox-exec -f <profile> \
    -D PROJECT_DIR=<cwd> \
    -D HOME=<home> \
    -D ROOT_GIT_DIR=<git_root> \
    claude [args...]
  ```

  The sandbox profile is generated dynamically and written to a temp file. It
  denies all writes except the project directory, temp dirs, Claude state
  directories, and git worktree roots. The temp profile is cleaned up after the
  process exits.

**Output streaming:**

`stream_output()` reads stdout line by line using `BufReader::lines()`. No async
runtime is used -- the entire loop is synchronous. Each NDJSON line is:

1. Written as raw JSON to a log file under
   `$TMPDIR/ralph/logs/<project>/<timestamp>.log`.
2. Parsed via `parser::parse_line()` into a typed `Event`.
3. Formatted for terminal display by `formatter::format_event()`.

The last `ResultEvent` is captured and returned to the caller.

### Step 7: Parse Result

When the Claude session completes, the `ResultEvent` contains the final output
text. Sigils are parsed during NDJSON deserialization by the parser module:

| Sigil                | Parser function          | Result field       |
| -------------------- | ------------------------ | ------------------ |
| `<task-done>ID</>`   | `parse_task_done()`      | `result.task_done` |
| `<task-failed>ID</>` | `parse_task_failed()`    | `result.task_failed` |
| `<next-model>M</>`   | `parse_next_model_hint()`| `result.next_model_hint` |
| `<promise>*</>`      | `is_complete()` / `is_failure()` | checked via methods |

If both `<task-done>` and `<task-failed>` appear in the same output, the parser
resolves the conflict optimistically: `task-done` wins and `task-failed` is
discarded.

The `<next-model>` hint is validated against a whitelist of `["opus", "sonnet",
"haiku"]`. Invalid model names are silently ignored.

### Step 8: Handle Outcome

The loop checks sigils in priority order:

**1. Promise FAILURE (short-circuit).** If `result.is_failure()` returns true,
immediately return `Outcome::Failure`. This happens before any DAG updates.

**2. Task-done sigil.** If `result.task_done` matches the assigned task ID,
calls `handle_task_done()`:

```
handle_task_done()
  |
  +-- verify enabled?
  |     |
  |     yes --> spawn verification agent
  |     |         |
  |     |         +-- verify-pass --> complete_task()
  |     |         |                    set verification_status = 'passed'
  |     |         |
  |     |         +-- verify-fail + retries left --> retry_task()
  |     |         |                                   (failed -> pending,
  |     |         |                                    retry_count++)
  |     |         |
  |     |         +-- verify-fail + retries exhausted --> fail_task()
  |     |
  |     no --> complete_task()
```

> [!IMPORTANT]
> The task-done sigil ID is validated against the assigned task ID. If they do
> not match, a warning is printed to stderr and no state transition occurs.

**3. Task-failed sigil.** If `result.task_failed` matches the assigned task ID,
calls `dag::fail_task()` with a generic failure message. The failure reason is
logged to `task_logs`.

**4. No sigil.** If Claude produced no completion sigil, calls
`dag::release_claim()`. This transitions the task back from `in_progress` to
`pending` and clears `claimed_by`. The task becomes eligible for pickup on the
next iteration.

### Step 9: Check Loop Exit Conditions

After handling the task outcome, two exit conditions are checked:

1. **All resolved.** `dag::all_resolved()` queries for tasks with status not in
   `('done', 'failed')`. If the count is zero, return `Outcome::Complete`.

2. **Limit reached.** `config.limit_reached()` returns true when `limit > 0`
   and `iteration > limit`. A limit of 0 means unlimited. Return
   `Outcome::LimitReached`.

### Step 10: Advance to Next Iteration

If neither exit condition is met:

1. `config.next_iteration()` clones the config with `iteration + 1`.
2. `strategy::select_model()` runs with the `next_model_hint` from step 7
   (if any). The result updates `config.current_model`.
3. If the hint overrode the strategy's choice, the override is logged to the
   progress database.
4. The loop continues from step 1.

## System Prompt Construction

`build_system_prompt()` in `src/claude/client.rs` assembles a multi-section
markdown prompt. Sections are appended conditionally based on available context.

### Core Instructions (always present)

The base prompt establishes Ralph's operating rules:

- You are operating in a Ralph loop.
- **ONE TASK PER LOOP.** Do not work on multiple tasks.
- Do not assume code exists -- search the codebase first.
- Implement fully working code, not placeholders or stubs.
- Run tests and type checks.
- Commit changes with a descriptive message.
- Signal completion or failure with a sigil.

### Sigil Documentation (always present)

Documents all sigils Claude can emit:

- `<task-done>{task_id}</task-done>` -- task completed successfully.
- `<task-failed>{task_id}</task-failed>` -- task cannot be completed.
- `<promise>COMPLETE</promise>` -- entire DAG is done.
- `<promise>FAILURE</promise>` -- unrecoverable situation.
- `<next-model>opus|sonnet|haiku</next-model>` -- hint for next iteration.

### Task Assignment (when iteration context exists)

Built by `build_task_context()`:

- **Assigned Task** -- ID, title, and full description.
- **Parent Context** -- If the task has a parent: parent title and description.
- **Completed Prerequisites** -- For each done blocker: task ID, title, and
  summary (most recent log entry or original description).

### Feature Specification (when feature-scoped)

The raw content of `.ralph/features/<name>/spec.md`.

### Feature Plan (when feature-scoped)

The raw content of `.ralph/features/<name>/plan.md`.

### Retry Information (when retry_count > 0)

```
This is retry attempt N of M.

The previous attempt failed verification with the following reason:

> <failure reason from task_logs>

Fix the issues identified above before marking the task as done.
```

### Available Skills (when skills exist)

Lists discovered skills with their descriptions:

```
- **skill-name**: description from YAML frontmatter
```

### Learning Instructions (when learn is enabled)

Instructs Claude to create reusable skills in `.ralph/skills/` and update
`CLAUDE.md` with project-specific knowledge.

## Verification Integration

When `config.verify` is true, completing a task triggers a verification step
before the DAG is updated. The verification agent is a separate Claude session
with restricted capabilities.

### Verification Agent Configuration

- **Tools:** `Bash Read Glob Grep` only (read-only access).
- **Model:** Uses the same model as the current iteration.
- **Prompt:** Built by `build_verification_prompt()` with:
  - Task ID, title, and description.
  - Feature specification (if available).
  - Feature plan (if available).
  - Instructions to check implementation, run tests, and emit a sigil.

### Verification Sigils

- `<verify-pass/>` -- Implementation is correct.
- `<verify-fail>reason</verify-fail>` -- Implementation has issues.
- No sigil -- Treated as verification failure with a default reason.

### Retry Logic

When verification fails and retries are available:

1. `dag::retry_task()` transitions the task from `in_progress` to `pending`
   (via `failed -> pending`).
2. Increments `retry_count` and sets `verification_status` to `'failed'`.
3. Clears `claimed_by` so the task can be picked up again.

On the next iteration, the task appears as ready again. The retry info in the
system prompt tells Claude what went wrong.

When retries are exhausted, `dag::fail_task()` marks the task as failed
permanently. The failure message includes the retry count and the verification
reason.

## Status Transitions

The DAG enforces a finite state machine on task statuses. Understanding these
transitions is essential to understanding the loop's behavior.

### Valid Transitions

```
                  +----------+
                  |          |
                  v          |
  pending ---> in_progress --+---> done
     ^              |
     |              v
     +---------- failed
     |
     +---------- blocked
```

| From          | To            | Trigger                          |
| ------------- | ------------- | -------------------------------- |
| `pending`     | `in_progress` | `claim_task()`                   |
| `pending`     | `blocked`     | Explicit block                   |
| `in_progress` | `done`        | `complete_task()`                |
| `in_progress` | `failed`      | `fail_task()`                    |
| `in_progress` | `pending`     | `release_claim()` or `retry_task()` |
| `blocked`     | `pending`     | Auto-unblock when blockers done  |
| `failed`      | `pending`     | `retry_task()`                   |

### Auto-Transitions

When a task transitions to `done`:

1. **Unblock dependents.** Any task in `blocked` status whose blockers are now
   all `done` transitions to `pending`.
2. **Complete parent.** If all children of a parent task are `done`, the parent
   auto-transitions to `done`. This recurses up through grandparents.

When a task transitions to `failed`:

1. **Fail parent.** The parent task (if any) auto-transitions to `failed`. This
   recurses upward.

## Model Strategy Details

### CostOptimized (default)

Reads the progress database content and applies heuristics:

- **Error signals** (`error`, `failure`, `stuck`, `panic`, `crash`, `broken`,
  `regression`, etc.) trigger escalation to `opus`.
- **Steady completions** (3+ `done` markers with no error signals) trigger a
  drop to `haiku`.
- **Otherwise** (empty, ambiguous, early iterations) defaults to `sonnet`.

Error signals always take priority over completion signals.

### Escalate

Tracks an `escalation_level` (0=haiku, 1=sonnet, 2=opus) that only moves
upward:

- Level 0: No distress signals detected.
- Level 1: Moderate signals (`error`, `failure`, `failed`, `bug`).
- Level 2: Severe signals (`stuck`, `cannot`, `unable`, `panic`, `crash`,
  `broken`, `regression`).

The level is the max of the current level and the assessed need. It never
decreases automatically. Only a `<next-model>` hint from Claude can
de-escalate.

### PlanThenExecute

Simple two-phase approach:

- Iteration 1: `opus` (understand the full task, form a plan).
- Iteration 2+: `sonnet` (execute the plan).

## Streaming and Output

The loop uses `std::process::Command` with piped stdout and spawns no async
runtime. Stderr is drained on a background thread to prevent pipe buffer
deadlocks.

Each NDJSON line from Claude is parsed into one of these event types:

| Event Type     | Source                   | Terminal Rendering                 |
| -------------- | ------------------------ | ---------------------------------- |
| `StreamDelta`  | `content_block_delta`    | Thinking in dim, text in bright white |
| `Assistant`    | Complete assistant message | Model name in purple               |
| `ToolUse`      | Tool invocation block    | Tool name in cyan, input dimmed    |
| `ToolErrors`   | Error tool results       | Red, first 5 lines                 |
| `Result`       | Final result             | Duration + cost in green           |

Raw JSON is written to log files under `$TMPDIR/ralph/logs/<project>/`. Each
iteration gets its own timestamped log file. The log file path is printed to the
terminal as a clickable hyperlink (using terminal escape codes).

Audio notifications are triggered via macOS `say` on task completion and
failure.

## Error Handling

The loop is designed to be resilient to individual iteration failures:

- **Claude process crashes or returns no result.** The task is released via
  `release_claim()`, returning it to `pending`. It will be picked up on the
  next iteration.

- **Verification agent crashes.** Treated as verification failure. If retries
  remain, the task is retried. Otherwise it is failed.

- **Mismatched sigil IDs.** If Claude emits `<task-done>` or `<task-failed>`
  with an ID that does not match the assigned task, a warning is printed to
  stderr. No state transition occurs, and the task is not released (it remains
  `in_progress` and will need manual intervention).

- **Database errors.** Propagated as `anyhow::Error` and cause the loop to
  exit. The current task may remain in `in_progress` status.

> [!CAUTION]
> If the loop exits abnormally (panic, database error, kill signal), tasks may
> be left in `in_progress` with a stale `claimed_by`. Use `ralph task reset
> <ID>` to manually return them to `pending`.

## Key Source Files

| File                     | Role                                      |
| ------------------------ | ----------------------------------------- |
| `src/run_loop.rs`        | Main loop, context building, task handling |
| `src/claude/client.rs`   | System prompt, CLI args, process spawning  |
| `src/claude/events.rs`   | Event types, sigil parsing                 |
| `src/claude/parser.rs`   | NDJSON line deserialization                |
| `src/verification.rs`    | Verification agent                         |
| `src/strategy.rs`        | Model selection strategies                 |
| `src/dag/mod.rs`         | Ready queries, claim/complete/fail/retry   |
| `src/dag/transitions.rs` | Status state machine, auto-transitions     |
| `src/config.rs`          | Config struct, iteration advancement       |
| `src/output/formatter.rs`| Terminal formatting, ANSI colors           |
| `src/output/logger.rs`   | Log file path generation                   |

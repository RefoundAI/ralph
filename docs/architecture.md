# Architecture

Ralph is a Rust CLI that orchestrates Claude Code sessions through a
DAG-based task system. It decomposes work into a directed acyclic graph of
tasks stored in SQLite, picks one ready task per iteration, spawns a fresh
Claude Code session to execute it, processes sigils from Claude's output to
update task state, and loops until all tasks are resolved or a limit is hit.

## System Overview

The conceptual data flow through Ralph follows a pipeline from user intent to
autonomous execution:

```
User
  |
  v
CLI (ralph init / feature / task / run)
  |
  v
Feature Workflow
  spec --> plan --> build
  |                   |
  |                   v
  |            Task DAG (SQLite)
  |                   |
  v                   v
Agent Loop (run_loop.rs)
  |
  +---> Claim ready task
  |       |
  |       v
  |     Build iteration context
  |       (task info, parent, blockers, spec, plan, retry info, skills)
  |       |
  |       v
  |     Spawn Claude Code session
  |       (direct or sandboxed, streaming NDJSON)
  |       |
  |       v
  |     Parse sigils from output
  |       (<task-done>, <task-failed>, <promise>, <next-model>)
  |       |
  |       v
  |     Update task state in DAG
  |       |
  |       +---> Verification agent (optional)
  |       |       |
  |       |       v
  |       |     <verify-pass/> or <verify-fail>reason</verify-fail>
  |       |       |
  |       |       v
  |       |     Complete, retry, or fail task
  |       |
  |       v
  |     Select model for next iteration (strategy + hint)
  |       |
  +-------+  (loop)
  |
  v
Outcome: Complete | Failure | LimitReached | Blocked | NoPlan
```

The user defines work through the feature workflow (spec, plan, build) or by
creating standalone tasks. The agent loop then executes that work one task at a
time, with each Claude Code session isolated to a single task.

## Module Map

```
src/
  main.rs            Entry point, CLI dispatch, system prompt builders for
                     interactive sessions (spec, plan, build, task create),
                     project context gathering, task tree rendering

  cli.rs             Clap derive-based argument definitions: Args, Command,
                     FeatureAction, TaskAction, DepsAction enums.
                     Model/strategy validation via resolve_model_strategy()

  config.rs          Config struct (12-param from_run_args constructor),
                     ModelStrategy enum, RunTarget enum, agent ID generation
                     (agent-{8hex} from DefaultHasher of timestamp+PID)

  project.rs         .ralph.toml discovery (walks up from CWD),
                     RalphConfig/ProjectConfig/ExecutionConfig parsing,
                     ralph init (creates dirs, config, gitignore)

  run_loop.rs        Main iteration loop, Outcome enum, task claiming,
                     context assembly, sigil handling, verification dispatch,
                     retry logic, skill discovery

  strategy.rs        Model selection: Fixed, CostOptimized, Escalate,
                     PlanThenExecute. ModelSelection struct with override
                     detection. Progress-file heuristics for cost optimization

  feature.rs         Feature struct + CRUD (create, get, get_by_id, list,
                     update status/spec_path/plan_path, ensure_feature_dirs,
                     read_spec, read_plan, feature_exists)

  verification.rs    Read-only verification agent, VerificationResult struct,
                     verify-pass/verify-fail sigil parsing

  dag/
    mod.rs           Task struct (14 fields), TaskCounts, TASK_COLUMNS const,
                     task_from_row() helper, ready-task queries (global and
                     feature-scoped), claim/complete/fail/retry/release,
                     standalone and feature task queries

    db.rs            SQLite schema v2 (tasks, dependencies, task_logs,
                     features), WAL mode, foreign keys, version-range
                     migrations

    ids.rs           SHA-256 based ID generation with atomic counter:
                     t-{6hex} for tasks, f-{6hex} for features.
                     Collision retry via generate_and_insert_task_id()

    crud.rs          Task CRUD (create, create_with_feature, get, update,
                     delete), BFS tree traversal (get_task_tree), LogEntry
                     struct, blocker/blocked-by queries, get_all_tasks

    tasks.rs         Parent status derivation: compute_parent_status()
                     recursively from children. get_task_status() returns
                     derived status considering children

    transitions.rs   Status transition validation (state machine),
                     auto-transitions: unblock dependents on done,
                     auto-complete parent when all children done,
                     auto-fail parent when any child fails.
                     Force-transition functions for CLI commands

    dependencies.rs  Dependency edge management, BFS cycle detection via
                     would_create_cycle()

  claude/
    mod.rs           Module declarations

    client.rs        Claude CLI spawning (direct + sandboxed via
                     sandbox-exec), system prompt construction with task
                     context, streaming NDJSON output handling via
                     BufReader::lines(). Structs: TaskInfo, ParentContext,
                     BlockerContext, RetryInfo, IterationContext

    interactive.rs   run_interactive() for human-in-the-loop sessions
                     (inherited stdio). run_streaming() for autonomous
                     sessions (feature build) with --dangerously-skip-
                     permissions and stream-json output

    events.rs        Typed event structs (Event enum, Assistant,
                     ContentBlock, ToolResult, StreamDelta, ResultEvent).
                     Sigil parsing functions: parse_next_model_hint,
                     parse_task_done, parse_task_failed

    parser.rs        Raw JSON deserialization into typed Event values

  output/
    formatter.rs     ANSI terminal formatting via colored crate. Renders
                     streaming deltas (thinking in dim, text in bright
                     white), tool use (cyan name, dimmed input), tool
                     errors (red, first 5 lines), result summaries
                     (duration + cost). macOS audio notifications via say.
                     Clickable file hyperlinks via terminal escape codes

    logger.rs        Log file path generation under
                     $TMPDIR/ralph/logs/<project_name>/<timestamp>.log

  sandbox/
    profile.rs       macOS sandbox-exec profile generation. Reads base
                     profile from resources/sandbox.sb, appends
                     dynamically generated rules from --allow flags

    rules.rs         Allow-rule definitions mapping rule names to
                     directories and binaries. collect_dirs() expands
                     ~/ paths. collect_blocked_binaries() blocks
                     unlisted tools by resolving their paths via which
```

## Key Data Structures

### Task

The central unit of work. Defined in `src/dag/mod.rs`:

```rust
pub struct Task {
    pub id: String,                        // t-{6hex}
    pub title: String,
    pub description: String,
    pub status: String,                    // pending|in_progress|done|blocked|failed
    pub parent_id: Option<String>,         // tree structure
    pub feature_id: Option<String>,        // links to features table
    pub task_type: String,                 // "feature" or "standalone"
    pub priority: i32,                     // lower = higher priority
    pub retry_count: i32,
    pub max_retries: i32,
    pub verification_status: Option<String>, // pending|passed|failed
    pub created_at: String,
    pub updated_at: String,
    pub claimed_by: Option<String>,        // agent-{8hex}
}
```

All SQL queries that produce `Task` values use the `TASK_COLUMNS` constant and
`task_from_row()` function to ensure consistency. Nullable columns use the
`row.get::<_, Option<T>>(N)?.unwrap_or(default)` pattern.

### Feature

Represents a high-level unit of work that goes through the spec/plan/build
lifecycle. Defined in `src/feature.rs`:

```rust
pub struct Feature {
    pub id: String,                  // f-{6hex}
    pub name: String,                // unique
    pub spec_path: Option<String>,
    pub plan_path: Option<String>,
    pub status: String,              // draft|planned|ready|running|done|failed
}
```

### Config

Runtime configuration for a `ralph run` invocation. Built from CLI flags,
`.ralph.toml`, and environment variables via a 12-parameter constructor.
Defined in `src/config.rs`:

```rust
pub struct Config {
    pub prompt_file: String,
    pub limit: u32,
    pub iteration: u32,
    pub total: u32,
    pub use_sandbox: bool,
    pub allowed_tools: Vec<String>,
    pub allow_rules: Vec<String>,
    pub model_strategy: ModelStrategy,
    pub model: Option<String>,
    pub current_model: String,
    pub escalation_level: u8,
    pub project_root: PathBuf,
    pub ralph_config: RalphConfig,
    pub agent_id: String,            // agent-{8hex}
    pub max_retries: u32,
    pub verify: bool,
    pub learn: bool,
    pub run_target: Option<RunTarget>,
}
```

The `agent_id` is generated once per run from a `DefaultHasher` over the
current timestamp and process ID. It persists across iterations via
`next_iteration()` (which clones the config and increments `iteration`).

### Outcome

The result of the main loop. Defined in `src/run_loop.rs`:

```rust
pub enum Outcome {
    Complete,      // All DAG tasks are done
    Failure,       // <promise>FAILURE</promise> emitted
    LimitReached,  // Iteration limit hit
    Blocked,       // No ready tasks, but incomplete tasks remain
    NoPlan,        // DAG is empty
}
```

### RunTarget

Scoping for `ralph run`. Defined in `src/config.rs`:

```rust
pub enum RunTarget {
    Feature(String),  // Run all tasks for a feature (by name)
    Task(String),     // Run a single task (by ID)
}
```

### ModelStrategy

How to select which Claude model to use each iteration. Defined in
`src/config.rs`:

```rust
pub enum ModelStrategy {
    Fixed,             // Always use --model value
    CostOptimized,     // Heuristic: sonnet default, opus on errors, haiku on streaks
    Escalate,          // Start haiku, escalate on failure, never auto-de-escalate
    PlanThenExecute,   // opus for iteration 1, sonnet for 2+
}
```

### IterationContext

The full context assembled for each loop iteration and injected into the
system prompt. Defined in `src/claude/client.rs`:

```rust
pub struct IterationContext {
    pub task: TaskInfo,                          // assigned task details
    pub spec_content: Option<String>,            // feature spec markdown
    pub plan_content: Option<String>,            // feature plan markdown
    pub retry_info: Option<RetryInfo>,           // retry attempt + failure reason
    pub skills_summary: Vec<(String, String)>,   // (name, description) tuples
    pub learn: bool,                             // whether learning is enabled
}
```

## Communication: The Sigil Protocol

Claude communicates back to Ralph through XML-like sigils embedded in its text
output. The result text from each Claude session is scanned for these patterns:

| Sigil | Purpose | Emitted By |
| --- | --- | --- |
| `<task-done>{id}</task-done>` | Mark the assigned task as complete | Worker agent |
| `<task-failed>{id}</task-failed>` | Mark the assigned task as failed | Worker agent |
| `<promise>COMPLETE</promise>` | All work is done, exit 0 | Worker agent |
| `<promise>FAILURE</promise>` | Critical failure, exit 1 | Worker agent |
| `<next-model>opus\|sonnet\|haiku</next-model>` | Hint for next iteration's model | Worker agent |
| `<verify-pass/>` | Verification passed | Verification agent |
| `<verify-fail>reason</verify-fail>` | Verification failed with reason | Verification agent |

Parsing rules (implemented in `src/claude/events.rs`):

- Sigils are parsed with simple string `find()` on the result text. No XML
  parser is used.
- Whitespace inside tags is trimmed.
- If both `<task-done>` and `<task-failed>` appear in the same output,
  `<task-done>` takes priority (the run loop checks for `task_done` first).
- `<next-model>` only accepts the literal values `opus`, `sonnet`, or `haiku`.
  Invalid model names cause the hint to be silently ignored.
- The first occurrence of each sigil type wins when duplicates exist.
- If no task sigil is emitted, the task claim is released and the task returns
  to `pending` status.

## Database Schema

Schema version 2 with 4 tables. Managed in `src/dag/db.rs` with version-range
migration checks (`if from_version < N && to_version >= N`). WAL mode and
foreign keys are enabled at connection time.

```sql
-- Core task table
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
    -- Schema v2 additions:
    feature_id TEXT REFERENCES features(id),
    task_type TEXT DEFAULT 'feature'
        CHECK (task_type IN ('feature','standalone')),
    retry_count INTEGER DEFAULT 0,
    max_retries INTEGER DEFAULT 3,
    verification_status TEXT
        CHECK (verification_status IN ('pending','passed','failed'))
);

-- Dependency edges with cycle prevention
CREATE TABLE dependencies (
    blocker_id TEXT NOT NULL REFERENCES tasks(id),
    blocked_id TEXT NOT NULL REFERENCES tasks(id),
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id != blocked_id)
);

-- Append-only task log
CREATE TABLE task_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    message TEXT NOT NULL,
    timestamp TEXT NOT NULL
);

-- Feature registry (schema v2)
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

### Task Status State Machine

Valid transitions are enforced in `src/dag/transitions.rs`:

```
              +----------+
              |          |
              v          |
 pending --> in_progress --> done
   ^  |         |             |
   |  |         v             | (auto-complete parent)
   |  +----> blocked          |
   |  |                       |
   |  +----> failed           |
   |            |             |
   +------------+             |
   (retry: failed->pending)   |
                              v
                      (auto-unblock dependents)
```

Auto-transitions triggered by `set_task_status()`:

- **done**: Unblocks all tasks that were waiting on this one
  (`blocked -> pending`). If all siblings under the same parent are done, the
  parent is auto-completed. This cascades recursively to grandparents.
- **failed**: The parent is auto-failed. This cascades recursively upward.

### Ready Task Query

A task is ready to execute when all four conditions hold (see
`get_ready_tasks()` in `src/dag/mod.rs`):

1. Status is `pending`
2. Task is a leaf node (has no children in the `parent_id` tree)
3. Parent (if any) is not `failed`
4. All blockers in the `dependencies` table have status `done`

Ready tasks are ordered by `priority ASC, created_at ASC`. The loop always
picks the first one.

## Execution Modes

Claude Code is spawned in three distinct modes depending on the command:

### 1. Interactive (`run_interactive`)

**Source:** `src/claude/interactive.rs`

Spawns `claude` with inherited stdio so the user can interact directly.
No `--print`, no `--output-format`, no `--dangerously-skip-permissions`.

Used by: `ralph feature spec`, `ralph feature plan`, `ralph task create`

```
claude --system-prompt "..." [--model M] "initial message"
```

### 2. Streaming (`run_streaming`)

**Source:** `src/claude/interactive.rs`

Spawns `claude` with piped stdout for real-time NDJSON parsing. Runs
autonomously with `--dangerously-skip-permissions` so Claude can execute
tools without user approval. Output is formatted and displayed in real time.

Used by: `ralph feature build`

```
claude --print --verbose --output-format stream-json \
       --dangerously-skip-permissions \
       --system-prompt "..." [--model M] "initial message"
```

### 3. Loop Iteration (`client::run`)

**Source:** `src/claude/client.rs`

The main loop mode. Supports both direct and sandboxed execution. Builds a
rich system prompt with task assignment, spec/plan content, retry information,
skills summary, and learning instructions. Parses the final `ResultEvent`
for sigils.

Direct mode:

```
claude --print --verbose --output-format stream-json \
       --no-session-persistence --model MODEL \
       --system-prompt "..." \
       --allowed-tools "Bash Edit Write Read Glob Grep ..." \
       @prompt_file
```

Sandboxed mode wraps the same `claude` invocation inside `sandbox-exec`:

```
sandbox-exec -f /tmp/ralph-sandbox-XXXXX.sb \
    -D PROJECT_DIR="$PWD" -D HOME="$HOME" -D ROOT_GIT_DIR="..." \
    claude --print --verbose --output-format stream-json \
           --no-session-persistence --model MODEL \
           --system-prompt "..." \
           --dangerously-skip-permissions \
           @prompt_file
```

When sandboxed, `--dangerously-skip-permissions` is passed to Claude because
the sandbox itself provides the security boundary. Without a sandbox, the
explicit `--allowed-tools` list is used instead.

> [!IMPORTANT]
> Ralph does not use an async runtime. All process I/O uses synchronous
> `std::process::Command` with `BufReader::lines()` for streaming. Stderr is
> drained on a background thread to prevent pipe buffer deadlocks.

## Sandbox Model

**Source:** `src/sandbox/profile.rs`, `src/sandbox/rules.rs`,
`resources/sandbox.sb`

The sandbox uses macOS `sandbox-exec` with a deny-by-default write policy.
The base profile (`resources/sandbox.sb`) is embedded at compile time via
`include_str!` and extended dynamically with rules from `--allow` flags.

### Default Write Whitelist

| Path | Reason |
| --- | --- |
| `$PROJECT_DIR/**` | The project being worked on |
| `/tmp/**`, `/private/tmp/**` | Temp files for tools and nix |
| `/var/folders/**` | macOS per-user temp directories |
| `~/.claude/**`, `~/.config/claude/**` | Claude state, session data, logs |
| `~/.claude.json`, `~/.claude.json.backup` | Claude config files in home |
| `~/.cache/**` | General cache directory |
| `~/.local/state/**` | XDG state directory |
| `$ROOT_GIT_DIR/**` | Git worktree support (main repo's `.git`) |
| `/dev/null` | Standard output discard |

### Blocked IPC

The sandbox blocks `com.apple.systemevents` via `mach-lookup` denial. This
prevents UI automation (keystrokes, mouse clicks) while still allowing other
Apple Events like the `open` command.

### Allow Rules

The `--allow` flag extends the sandbox. Rule definitions live in
`src/sandbox/rules.rs`:

| Rule | Grants |
| --- | --- |
| `aws` | Write access to `~/.aws`, unblocks `aws` CLI binary |

Without `--allow=aws`, the `aws` binary is blocked via `process-exec*`
denial on its resolved path. New rules are added by extending the `HashMap`
definitions in `rules.rs`.

## Configuration Layers

Configuration is resolved from three sources, with later sources overriding
earlier ones:

### 1. `.ralph.toml` (Project Config)

Discovered by walking up the directory tree from the current working directory.
Parsed into `RalphConfig` in `src/project.rs`:

```toml
[specs]
dirs = [".ralph/specs"]      # Directories containing reference specs

[execution]
max_retries = 3              # Maximum retries for failed tasks
verify = true                # Enable autonomous verification
learn = true                 # Enable skill creation + CLAUDE.md updates
```

All sections and fields use `#[serde(default)]` so partial configs work.
Unknown keys are silently ignored for forward compatibility.

### 2. CLI Flags

Flags on the `ralph run` command override `.ralph.toml` values:

| Flag | Overrides |
| --- | --- |
| `--model MODEL` | Sets fixed strategy with the given model |
| `--model-strategy STRAT` | `execution` model selection strategy |
| `--max-retries N` | `execution.max_retries` |
| `--no-verify` | `execution.verify` (sets to false) |
| `--no-learn` | `execution.learn` (sets to false) |
| `--no-sandbox` | Disables macOS sandbox |
| `--allow RULE` | Adds sandbox allow rules |
| `--once` | Sets iteration limit to 1 |
| `--limit N` | Sets iteration limit (0 = unlimited) |

The `--model` flag alone implies `--model-strategy=fixed`. The
`--model-strategy=fixed` flag requires `--model` to be set. This is validated
in `cli::resolve_model_strategy()`.

### 3. Environment Variables

| Variable | Equivalent Flag |
| --- | --- |
| `RALPH_LIMIT` | `--limit` |
| `RALPH_MODEL` | `--model` |
| `RALPH_MODEL_STRATEGY` | `--model-strategy` |
| `RALPH_ITERATION` | (internal) Starting iteration number |
| `RALPH_TOTAL` | (internal) Total planned iterations |

## The Agent Loop in Detail

The main loop in `src/run_loop.rs` follows this sequence each iteration:

1. **Resolve feature context.** If the run target is a feature, load the
   spec and plan content from `.ralph/features/<name>/spec.md` and
   `plan.md`. If the target is a task with a `feature_id`, resolve the
   feature name and load its spec/plan.

2. **Get scoped ready tasks.** Filter the ready-task query by feature ID
   (for feature targets) or task ID (for task targets).

3. **Check termination conditions.** If the DAG is empty, return `NoPlan`.
   If no tasks are ready but unresolved tasks exist, return `Blocked`.
   If all tasks are resolved, return `Complete`.

4. **Claim the first ready task.** Transition it to `in_progress` and set
   `claimed_by` to the agent ID.

5. **Build iteration context.** Assemble the `IterationContext` with task
   info, parent context, completed blocker summaries, spec/plan content,
   retry info (if applicable), and discovered skills.

6. **Run Claude.** Spawn a Claude Code session with the assembled system
   prompt. Stream and format output in real time. Log raw NDJSON to a temp
   file.

7. **Parse sigils.** Extract `task_done`, `task_failed`, `next_model_hint`,
   and promise sigils from the result text.

8. **Handle task outcome.**
   - `<promise>FAILURE</promise>`: Return `Outcome::Failure` immediately.
   - `<task-done>`: If verification is enabled, spawn the verification
     agent. On `<verify-pass/>`, complete the task. On `<verify-fail>`,
     either retry (if under `max_retries`) or fail the task.
   - `<task-failed>`: Fail the task and log the reason.
   - No sigil: Release the claim (task returns to `pending`).

9. **Check completion.** If all tasks are resolved, return `Complete`.
   If the iteration limit is reached, return `LimitReached`.

10. **Advance to next iteration.** Increment the iteration counter, select
    the model for the next iteration based on strategy and hint, and loop.

## Model Strategy Details

Model selection happens at the boundary between iterations. The strategy
produces a candidate model, and Claude's `<next-model>` hint (if present)
can override it.

### Fixed

Always returns the `--model` value. Ignores iteration number and progress.

### CostOptimized (default)

Reads the progress database and applies heuristics:

- **Error signals detected** (error, failure, stuck, panic, etc.): Use `opus`
- **3+ completion signals with no errors**: Use `haiku`
- **Otherwise** (empty, ambiguous, early): Use `sonnet`

### Escalate

Monotonically escalates from cheap to expensive models:

- Starts at `haiku` (level 0)
- Moderate distress signals (error, failed, bug): Escalate to `sonnet` (level 1)
- Severe distress signals (stuck, cannot, panic, crash): Escalate to `opus` (level 2)
- Never auto-de-escalates. The `escalation_level` field tracks the floor.
- Claude hints can de-escalate by setting the level directly.

### PlanThenExecute

- Iteration 1: `opus` (for understanding and planning)
- Iteration 2+: `sonnet` (for execution)

### Hint Override

All strategies can be overridden by Claude's `<next-model>` hint. The
`ModelSelection` struct tracks whether the hint disagreed with the strategy's
choice (`was_overridden`), and overrides are logged to the progress database.

## Verification Agent

**Source:** `src/verification.rs`

When verification is enabled (`--no-verify` not set, `execution.verify` is
true), completed tasks are verified before being marked done.

The verification agent is a separate Claude Code session with restricted
tools:

```
--allowed-tools "Bash Read Glob Grep"
```

It receives a system prompt containing the task details, spec, and plan. It
inspects the codebase and runs tests, then emits either `<verify-pass/>`
or `<verify-fail>reason</verify-fail>`.

On verification failure:

- If `retry_count < max_retries`: Reset the task to `pending` with
  incremented `retry_count` and `verification_status = 'failed'`. The next
  iteration picks it up with retry context (attempt number and failure reason).
- If retries exhausted: Fail the task permanently.

## Key Files on Disk

| Path | Purpose |
| --- | --- |
| `.ralph.toml` | Project configuration (discovered by walking up) |
| `.ralph/progress.db` | SQLite DAG database (gitignored) |
| `.ralph/features/<name>/spec.md` | Feature specification document |
| `.ralph/features/<name>/plan.md` | Feature implementation plan |
| `.ralph/skills/<name>/SKILL.md` | Reusable agent skills with YAML frontmatter |
| `.ralph/specs/` | Reference specification documents |
| `$TMPDIR/ralph/logs/<project>/<timestamp>.log` | Raw NDJSON session logs |

[clap]: https://docs.rs/clap/latest/clap/
[rusqlite]: https://docs.rs/rusqlite/latest/rusqlite/

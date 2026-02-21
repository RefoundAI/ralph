# Architecture

Ralph is a Rust CLI that orchestrates ACP-compliant AI agent sessions through a
DAG-based task system. It decomposes work into a directed acyclic graph of
tasks stored in SQLite, picks one ready task per iteration, spawns a fresh ACP
agent session to execute it, processes sigils from the agent's output to update
task state, and loops until all tasks are resolved or a limit is hit.

Ralph communicates with agents via the [Agent Client Protocol](https://agentclientprotocol.com)
(ACP) — a JSON-RPC 2.0 standard over stdin/stdout. Ralph acts as the ACP
**client** and **tool provider**: it spawns any ACP-compliant agent binary and
fulfills `fs/read_text_file`, `fs/write_text_file`, and terminal tool requests
from the agent.

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
Agent Loop (run_loop.rs) [async, tokio]
  |
  +---> Claim ready task
  |       |
  |       v
  |     Build iteration context
  |       (task info, parent, blockers, spec, plan, retry info, journal, knowledge)
  |       |
  |       v
  |     run_iteration() — ACP connection lifecycle
  |       Spawn agent subprocess
  |       ACP handshake (initialize, new_session)
  |       Send prompt (system instructions + task context as TextContent block)
  |       Receive session_notification updates (text, thoughts, tool calls)
  |       Fulfill tool requests (fs read/write, terminal create/output/wait)
  |       Await PromptResponse.stop_reason
  |       |
  |       v
  |     Extract sigils from accumulated text
  |       (<task-done>, <task-failed>, <promise>, <next-model>, <journal>, <knowledge>)
  |       |
  |       v
  |     Update task state in DAG
  |       |
  |       +---> Verification agent (optional, read-only ACP session)
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
Outcome: Complete | Failure | LimitReached | Blocked | NoPlan | Interrupted
```

The user defines work through the feature workflow (spec, plan, build) or by
creating standalone tasks. The agent loop then executes that work one task at a
time, with each ACP agent session isolated to a single task.

## Module Map

```
src/
  main.rs            Entry point, CLI dispatch, system prompt builders for
                     interactive sessions (spec, plan, build, task create),
                     project context gathering, task tree rendering

  cli.rs             Clap derive-based argument definitions: Args, Command,
                     FeatureAction, TaskAction, DepsAction enums.
                     Model/strategy validation via resolve_model_strategy().
                     --agent flag on run, feature spec/plan/build, task create

  config.rs          Config struct, ModelStrategy enum, RunTarget enum,
                     agent ID generation (agent-{8hex} from DefaultHasher
                     of timestamp+PID). agent_command resolved from --agent
                     flag > RALPH_AGENT env > [agent].command > "claude".
                     Validated with shlex::split()

  project.rs         .ralph.toml discovery (walks up from CWD),
                     RalphConfig/ProjectConfig/ExecutionConfig/AgentConfig
                     parsing, ralph init (creates dirs, config, gitignore)

  run_loop.rs        Main async iteration loop, Outcome enum, task claiming,
                     context assembly, sigil handling, verification dispatch,
                     retry logic, stop reason mapping

  strategy.rs        Model selection: Fixed, CostOptimized, Escalate,
                     PlanThenExecute. ModelSelection struct with override
                     detection. Progress-file heuristics for cost optimization

  feature.rs         Feature struct + CRUD (create, get, get_by_id, list,
                     update status/spec_path/plan_path, ensure_feature_dirs,
                     read_spec, read_plan, feature_exists)

  verification.rs    Read-only ACP verification agent. Uses run_autonomous()
                     with read_only=true. VerificationResult struct,
                     verify-pass/verify-fail sigil parsing

  review.rs          Writable ACP review agent. Uses run_autonomous() with
                     model=Some("opus"). review-pass/review-changes sigils

  interrupt.rs       SIGINT handler (signal-hook AtomicBool). Double Ctrl+C
                     force-exits. prompt_for_feedback(), append_feedback_to_description()

  dag/
    mod.rs           Task struct (14 fields), TaskCounts, TASK_COLUMNS const,
                     task_from_row() helper, ready-task queries (global and
                     feature-scoped), claim/complete/fail/retry/release,
                     standalone and feature task queries

    db.rs            SQLite schema v3 (tasks, dependencies, task_logs,
                     features, journal + FTS5 index), WAL mode, foreign keys,
                     version-range migrations

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

  acp/
    mod.rs           Module root, re-exports

    connection.rs    Agent spawning (tokio::process::Command), ACP connection
                     lifecycle via ClientSideConnection, run_iteration()
                     (main loop entry point), run_autonomous() (verification,
                     review, feature build). Interrupt handling via select!,
                     stop reason mapping, process cleanup

    client_impl.rs   RalphClient: impl acp::Client trait. Handles
                     session_notification (text accumulation + rendering),
                     request_permission (auto-approve / read-only deny writes),
                     read_text_file, write_text_file (with files_modified
                     tracking), terminal tool delegation

    tools.rs         TerminalSession struct: tokio::process::Child with
                     stdout/stderr reader tasks (spawn_local). Handlers for
                     create_terminal (sh -c, piped IO), terminal_output
                     (drain buffers), wait_for_terminal_exit, kill_terminal,
                     release_terminal. Buffer cap: 1MB per stream

    prompt.rs        Prompt text construction: build_prompt_text() concatenates
                     system instructions + task context into a single string
                     (ACP has no separate system prompt channel).
                     build_system_instructions(), build_task_context()

    sigils.rs        Sigil extraction: extract_sigils() → SigilResult.
                     Individual parsers: parse_task_done, parse_task_failed,
                     parse_next_model_hint, parse_journal_sigil,
                     parse_knowledge_sigils, extract_attribute

    streaming.rs     Session update rendering: AgentMessageChunk → bright
                     white, AgentThoughtChunk → dim, ToolCall → cyan name +
                     dimmed input, tool errors → red (5 lines)

    interactive.rs   run_interactive(): ACP-mediated interactive sessions.
                     Ralph reads stdin, sends as PromptRequests, renders
                     streaming response. run_streaming(): single autonomous
                     prompt (feature build)

    types.rs         RunResult (Completed/Interrupted), StreamingResult
                     (full_text, files_modified, duration_ms, stop_reason),
                     SigilResult, IterationContext, TaskInfo, ParentContext,
                     BlockerContext, RetryInfo, KnowledgeSigil

  journal.rs         Persistent iteration records (SQLite + FTS5). JournalEntry
                     with run_id, outcome, model, duration_secs, cost_usd
                     (always 0.0 for ACP iterations), files_modified, notes.
                     Smart selection: recent entries + FTS matches. 3000-token
                     budget. Cost line omitted from rendering when cost_usd=0.0

  knowledge.rs       Tag-based project knowledge in .ralph/knowledge/ markdown
                     files. YAML frontmatter (title, tags). Tag-scored matching,
                     deduplication on write (exact title or >50% tag overlap).
                     2000-token budget

  output/
    formatter.rs     ANSI terminal formatting via colored crate. Renders
                     ACP session updates (text in bright white, thoughts in
                     dim, tool calls in cyan), result summaries (duration).
                     macOS audio notifications via say. Clickable file
                     hyperlinks via terminal escape codes

    logger.rs        Log file path generation under
                     $TMPDIR/ralph/logs/<project_name>/<timestamp>.log
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
`.ralph.toml`, and environment variables. Defined in `src/config.rs`:

```rust
pub struct Config {
    pub agent_command: String,       // Resolved: --agent > RALPH_AGENT > [agent].command > "claude"
    pub limit: u32,
    pub iteration: u32,
    pub total: u32,
    pub model_strategy: ModelStrategy,
    pub model: Option<String>,
    pub current_model: String,
    pub escalation_level: u8,
    pub project_root: PathBuf,
    pub ralph_config: RalphConfig,
    pub agent_id: String,            // agent-{8hex}
    pub max_retries: u32,
    pub verify: bool,
    pub run_id: String,              // run-{8hex}
    pub run_target: Option<RunTarget>,
}
```

The `agent_id` is generated once per run from a `DefaultHasher` over the
current timestamp and process ID. It persists across iterations via
`next_iteration()` (which clones the config and increments `iteration`).

### AgentConfig

ACP agent configuration from `.ralph.toml`. Defined in `src/project.rs`:

```rust
pub struct AgentConfig {
    pub command: String,   // Default: "claude"
}
```

Resolution order for `agent_command`: `--agent` CLI flag > `RALPH_AGENT` env
var > `[agent].command` in `.ralph.toml` > `"claude"`. Validated with
`shlex::split()` — error on malformed input (e.g. unclosed quotes).

### Outcome

The result of the main loop. Defined in `src/run_loop.rs`:

```rust
pub enum Outcome {
    Complete,      // All DAG tasks are done
    Failure,       // <promise>FAILURE</promise> emitted
    LimitReached,  // Iteration limit hit
    Blocked,       // No ready tasks, but incomplete tasks remain
    NoPlan,        // DAG is empty
    Interrupted,   // User pressed Ctrl+C and chose not to continue
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

How to select which model to use each iteration. Defined in
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
prompt. Defined in `src/acp/types.rs`:

```rust
pub struct IterationContext {
    pub task: TaskInfo,
    pub parent: Option<ParentContext>,
    pub blockers: Vec<BlockerContext>,
    pub spec_content: Option<String>,
    pub plan_content: Option<String>,
    pub retry_info: Option<RetryInfo>,
    pub run_id: String,
    pub journal_context: String,
    pub knowledge_context: String,
}
```

### RunResult / StreamingResult

The result of a single ACP iteration. Defined in `src/acp/types.rs`:

```rust
pub enum RunResult {
    Completed(StreamingResult),
    Interrupted,
}

pub struct StreamingResult {
    pub full_text: String,           // Accumulated agent message text
    pub files_modified: Vec<String>, // Paths written via fs/write_text_file
    pub duration_ms: u64,            // Wall-clock ms (tracked by Ralph)
    pub stop_reason: acp::StopReason,
}
```

### SigilResult

All sigils extracted from an ACP session's text output. Defined in
`src/acp/types.rs`:

```rust
pub struct SigilResult {
    pub task_done: Option<String>,
    pub task_failed: Option<String>,
    pub next_model_hint: Option<String>,
    pub journal_notes: Option<String>,
    pub knowledge_entries: Vec<KnowledgeSigil>,
    pub is_complete: bool,   // <promise>COMPLETE</promise>
    pub is_failure: bool,    // <promise>FAILURE</promise>
}
```

## Communication: The Sigil Protocol

The agent communicates back to Ralph through XML-like sigils embedded in its
text output. `acp::sigils::extract_sigils()` scans the accumulated
`StreamingResult.full_text` for these patterns:

| Sigil | Purpose | Emitted By |
| --- | --- | --- |
| `<task-done>{id}</task-done>` | Mark the assigned task as complete | Worker agent |
| `<task-failed>{id}</task-failed>` | Mark the assigned task as failed | Worker agent |
| `<promise>COMPLETE</promise>` | All work is done, exit 0 | Worker agent |
| `<promise>FAILURE</promise>` | Critical failure, exit 1 | Worker agent |
| `<next-model>opus\|sonnet\|haiku</next-model>` | Hint for next iteration's model | Worker agent |
| `<journal>notes</journal>` | Notes for the iteration journal | Worker agent |
| `<knowledge tags="..." title="...">body</knowledge>` | Project knowledge entry | Worker agent |
| `<verify-pass/>` | Verification passed | Verification agent |
| `<verify-fail>reason</verify-fail>` | Verification failed with reason | Verification agent |

Parsing rules (implemented in `src/acp/sigils.rs`):

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

## ACP Protocol

Ralph uses the [Agent Client Protocol](https://agentclientprotocol.com) crate
(`agent-client-protocol`) for JSON-RPC 2.0 communication with agent processes.

### Connection Lifecycle

Per-iteration lifecycle (one agent process per iteration):

1. **Spawn**: `tokio::process::Command::new(program).args(args).stdin(piped).stdout(piped).stderr(piped).current_dir(project_root)`
   with `RALPH_MODEL`, `RALPH_ITERATION`, `RALPH_TOTAL` env vars set.
2. **Wrap**: Agent stdio is wrapped with `tokio_util::compat` adapters for
   futures IO compatibility.
3. **Connect**: `ClientSideConnection::new(RalphClient, outgoing, incoming, spawn_local)` —
   the `io_future` is spawned via `spawn_local` inside a `LocalSet` to drive the
   JSON-RPC transport. ACP futures are `!Send`; `current_thread` runtime is used.
4. **Initialize**: `conn.initialize(InitializeRequest { protocol_version, capabilities: { fs: { read: true, write: !read_only }, terminal: true } })`
5. **Session**: `conn.new_session(NewSessionRequest { cwd: project_root })`
6. **Prompt**: `conn.prompt(PromptRequest { session_id, prompt: [TextContent { text: prompt_text }] })`
7. **Stream**: Agent sends `session_notification` updates (text chunks, thought chunks,
   tool call notifications). `RalphClient::session_notification()` renders and accumulates.
8. **Tool calls**: Agent requests `fs/read_text_file`, `fs/write_text_file`, `terminal/*`.
   `RalphClient` fulfills these by reading/writing disk and spawning subprocesses.
9. **Complete**: `conn.prompt()` returns `PromptResponse { stop_reason }`.
10. **Cleanup**: Terminal sessions killed, agent process killed/waited.

### Tool Provider

`RalphClient` implements the ACP `Client` trait and fulfills all tool requests
from the agent:

| ACP Method | Ralph Behavior |
| --- | --- |
| `fs/read_text_file` | Read file from disk, optional offset+limit parameters |
| `fs/write_text_file` | Write file, create parent dirs, track in `files_modified`; error in read-only mode |
| `terminal/create_terminal` | Spawn `sh -c <command>`, return terminal ID |
| `terminal/terminal_output` | Drain stdout+stderr buffers |
| `terminal/wait_for_terminal_exit` | Await child process completion |
| `terminal/kill_terminal_command` | Kill child process |
| `terminal/release_terminal` | Kill + cleanup + remove session |
| `request_permission` | Auto-approve (normal) / deny writes (read-only) |

Paths written to `files_modified` are normalized to project-relative paths via
`Path::strip_prefix(project_root)`.

### Interrupt Handling

```rust
tokio::select! {
    result = conn.prompt(prompt_request) => { /* normal completion */ }
    _ = poll_interrupt() => {
        conn.cancel(CancelNotification::new(session_id)).ok();
        return Ok(RunResult::Interrupted);
    }
}
```

`poll_interrupt()` checks `interrupt::is_interrupted()` every 100ms. The
existing feedback prompt, journal logging, and claim release logic in
`run_loop.rs` handle cleanup after `run_iteration()` returns `Interrupted`.

## Database Schema

Schema version 3 with 5 tables. Managed in `src/dag/db.rs` with version-range
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

-- Iteration journal with FTS5 full-text search (schema v3)
CREATE TABLE journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    iteration INTEGER NOT NULL,
    task_id TEXT,
    feature_id TEXT,
    outcome TEXT NOT NULL CHECK (outcome IN ('done','failed','retried','blocked')),
    model TEXT,
    duration_secs REAL,
    cost_usd REAL DEFAULT 0.0,   -- Always 0.0 for ACP iterations (ACP doesn't report cost)
    files_modified TEXT,
    notes TEXT,
    created_at TEXT NOT NULL
);
CREATE VIRTUAL TABLE journal_fts USING fts5(notes, content=journal, ...);
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

The ACP agent is spawned in three distinct modes depending on the command:

### 1. Interactive (`run_interactive`)

**Source:** `src/acp/interactive.rs`

Ralph mediates between the user (stdin) and the ACP agent via a
prompt/response cycle. The user types plain text; Ralph sends it as a
`PromptRequest`; the agent's response is rendered to the terminal.

Used by: `ralph feature spec`, `ralph feature plan`, `ralph task create`

### 2. Streaming (`run_streaming`)

**Source:** `src/acp/interactive.rs`

A single autonomous prompt. The agent runs to completion without user input.
Ralph renders the agent's streaming response via `session_notification`.

Used by: `ralph feature build`

### 3. Loop Iteration (`run_iteration`)

**Source:** `src/acp/connection.rs`

The main loop mode. One agent process per iteration. Builds a rich prompt from
`IterationContext` (task assignment, spec/plan, retry info, journal/knowledge
context). Parses `SigilResult` for loop control signals.

Used by: `ralph run`

> [!IMPORTANT]
> Ralph uses a tokio async runtime (`current_thread` flavor, required because
> ACP futures are `!Send`). The agent connection lifecycle runs inside a
> `LocalSet`. All process I/O uses `tokio::process::Command` with async IO.
> DAG operations (rusqlite) remain synchronous — called directly from async
> context.

## Configuration Layers

Configuration is resolved from three sources, with later sources overriding
earlier ones:

### 1. `.ralph.toml` (Project Config)

Discovered by walking up the directory tree from the current working directory.
Parsed into `RalphConfig` in `src/project.rs`:

```toml
[execution]
max_retries = 3              # Maximum retries for failed tasks
verify = true                # Enable autonomous verification

[agent]
command = "claude"           # ACP agent binary (default: claude)
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
| `--agent CMD` | `agent.command` (ACP agent binary) |
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
| `RALPH_AGENT` | `--agent` |
| `RALPH_ITERATION` | (internal) Starting iteration number |
| `RALPH_TOTAL` | (internal) Total planned iterations |

`RALPH_MODEL`, `RALPH_ITERATION`, and `RALPH_TOTAL` are also **passed through**
to the spawned agent subprocess, so the agent binary can read them.

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
   retry info (if applicable), journal context, and knowledge context.

6. **Run ACP agent session.** Call `acp::connection::run_iteration()`.
   Spawn agent subprocess, perform ACP handshake, send prompt (system
   instructions + task context as a single `TextContent` block), stream
   and render output, fulfill tool requests, await `PromptResponse`.

7. **Handle stop reason.** `EndTurn` → proceed to sigil extraction.
   `Cancelled` → treat as interrupted. `MaxTokens`/`MaxTurnRequests` →
   release claim, journal `"blocked"`. `Refusal` → fail task, journal
   `"failed"`.

8. **Extract sigils.** Call `acp::sigils::extract_sigils(&full_text)` on
   the accumulated agent text to produce a `SigilResult`.

9. **Handle task outcome.**
   - `<promise>FAILURE</promise>`: Return `Outcome::Failure` immediately.
   - `<task-done>`: If verification is enabled, spawn the verification
     agent (read-only ACP session). On `<verify-pass/>`, complete the task.
     On `<verify-fail>`, either retry (if under `max_retries`) or fail.
   - `<task-failed>`: Fail the task and log the reason.
   - No sigil: Release the claim (task returns to `pending`).

10. **Write journal entry.** Record outcome, model, duration, `cost_usd=0.0`,
    files modified, and any `<journal>` notes to the SQLite journal table.

11. **Write knowledge entries.** Upsert any `<knowledge>` sigils to
    `.ralph/knowledge/` markdown files.

12. **Check completion.** If all tasks are resolved, return `Complete`.
    If the iteration limit is reached, return `LimitReached`.

13. **Advance to next iteration.** Increment the iteration counter, select
    the model for the next iteration based on strategy and hint, and loop.

## Model Strategy Details

Model selection happens at the boundary between iterations. The strategy
produces a candidate model, and the agent's `<next-model>` hint (if present)
can override it. The selected model is communicated to the agent via the
`RALPH_MODEL` environment variable on the spawned process.

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
- Agent hints can de-escalate by setting the level directly.

### PlanThenExecute

- Iteration 1: `opus` (for understanding and planning)
- Iteration 2+: `sonnet` (for execution)

### Hint Override

All strategies can be overridden by the agent's `<next-model>` hint. The
`ModelSelection` struct tracks whether the hint disagreed with the strategy's
choice (`was_overridden`), and overrides are logged to the progress database.

## Verification Agent

**Source:** `src/verification.rs`

When verification is enabled (`--no-verify` not set, `execution.verify` is
true), completed tasks are verified before being marked done.

The verification agent uses `acp::connection::run_autonomous()` with
`read_only=true`. The `RalphClient` rejects `fs/write_text_file` requests
with an error response, while terminal operations remain available (for
running `cargo test`, inspecting build output, etc.).

The agent receives a system prompt containing the task details, spec, and plan.
It inspects the codebase and runs tests, then emits either `<verify-pass/>`
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
| `.ralph/knowledge/<name>.md` | Project knowledge entries with YAML frontmatter |
| `.claude/skills/<name>/SKILL.md` | Reusable agent skills with YAML frontmatter |
| `$TMPDIR/ralph/logs/<project>/<timestamp>.log` | Raw JSON-RPC session logs |

[clap]: https://docs.rs/clap/latest/clap/
[rusqlite]: https://docs.rs/rusqlite/latest/rusqlite/

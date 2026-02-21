# Interactive Flows

Ralph spawns ACP agent sessions in three distinct modes depending on the command.
Each mode controls whether the user can interact, whether the session is read-only,
and how the output is processed. This document explains each mode, when it is used,
and how it works.

All three modes communicate with the agent via the [Agent Client Protocol](https://agentclientprotocol.com)
(ACP) — a JSON-RPC 2.0 standard over stdin/stdout. The agent binary is configurable
via `--agent <CMD>`, `RALPH_AGENT` env var, or `[agent].command` in `.ralph.toml`
(default: `claude`).

## Overview

| Mode               | User Input | Key Function              | Commands                                     |
| ------------------ | ---------- | ------------------------- | -------------------------------------------- |
| Interactive        | Yes        | `run_interactive()`       | `feature spec`, `feature plan`, `task create` |
| Streaming          | No         | `run_streaming()`         | `feature build`                              |
| Loop Iteration     | No         | `run_iteration()`         | `ralph run`                                  |

All three modes live under `src/acp/`. Interactive and streaming modes are
defined in `interactive.rs`. Loop iteration mode is defined in `connection.rs`.

## Interactive Mode

### How It Works

`run_interactive()` in `src/acp/interactive.rs` spawns an ACP agent subprocess
and mediates all I/O between the user and the agent. Unlike the previous
stdio-inheritance approach, **Ralph is the intermediary**: it reads lines from
stdin, sends them to the agent as `PromptRequest`s, and renders the agent's
streaming response to the terminal.

> **UX change from previous versions**: Earlier versions of Ralph spawned Claude
> with inherited stdio, giving users Claude's native terminal UI. With ACP,
> Ralph mediates all I/O — the user types plain text, Ralph sends it as a prompt,
> and renders the agent's streaming response. This trades agent-specific UI
> features for agent-agnosticism.

The function signature:

```rust
pub async fn run_interactive(
    agent_command: &str,
    instructions: &str,
    initial_message: &str,
    project_root: &Path,
    model: Option<&str>,
) -> Result<()>
```

The `instructions` are prepended to `initial_message` in the first prompt
(ACP has no separate system prompt channel — everything is concatenated into
a single `TextContent` block). Subsequent user inputs are sent as standalone
`PromptRequest`s. The session loops until the user enters an empty line or
sends EOF (Ctrl+D).

### ACP Lifecycle

1. Spawn agent subprocess with `agent_command` (parsed via `shlex::split()`).
2. Initialize ACP connection: `conn.initialize()` with fs + terminal capabilities.
3. Create session: `conn.new_session(cwd = project_root)`.
4. Send first prompt: `instructions + initial_message` as a `TextContent` block.
5. Render agent's streaming response (text in bright white, thoughts in dim).
6. Read next user input from stdin.
7. Send as new `PromptRequest` with the user's text.
8. Repeat from step 5 until user exits.

### Commands That Use Interactive Mode

#### `ralph feature spec <name>`

Ralph mediates an interview session where the user describes requirements.
The agent writes `.ralph/features/<name>/spec.md`.

The system prompt (built by `build_feature_spec_system_prompt()` in `main.rs`)
instructs the agent to:

- Ask about functional requirements, technical constraints, edge cases, testing
  requirements, and dependencies.
- Produce a structured spec document with sections for Overview, Requirements,
  Architecture, API/Interface, Data Models, Testing, and Dependencies.
- Write the result to `.ralph/features/<name>/spec.md`.

The system prompt includes project context assembled by
`gather_project_context()` (see [Context Assembly](#context-assembly-for-interactive-sessions)).

#### `ralph feature plan <name>`

The agent reads the spec and creates `plan.md` through a mediated session.

The system prompt (`build_feature_plan_system_prompt()`) includes the full spec
content and instructs the agent to create a detailed implementation plan with
phases, per-phase details, verification criteria, and risk areas. The plan is
written to `.ralph/features/<name>/plan.md`.

Requires that `feature spec` has already been run (the feature must have a
`spec_path` in the database).

#### `ralph task create`

The agent interviews the user to understand what they want done, then creates
a standalone task in the Ralph database.

The system prompt (`build_task_new_system_prompt()`) includes project context
with the `include_tasks` flag set to `true`, so existing standalone tasks are
listed. The agent asks about the desired work, specific files, acceptance
criteria, and priority.

### Resume Support

Both `feature spec` and `feature plan` detect existing output files and support
resuming interrupted sessions:

1. If `spec.md` or `plan.md` already exists, its content is loaded, truncated
   to 10,000 characters if necessary, and appended to the system prompt context.
2. The initial message changes from `"Start the spec/plan interview..."` to
   `"Resume the spec/plan interview..."` with a note that the current draft is
   in the system prompt.

This lets users close a session and pick up where they left off without losing
prior work.

### Model Selection

All interactive commands accept `--model <model>` to choose which model to use.
The model name is passed to the spawned agent process via the `RALPH_MODEL`
environment variable. The agent binary reads this to select the appropriate model.

```bash
ralph feature spec my-feature --model opus
ralph feature plan my-feature --model sonnet
ralph task create --model haiku
```

## Streaming Mode

### How It Works

`run_streaming()` in `src/acp/interactive.rs` sends a single autonomous prompt
to the ACP agent and renders the streaming response in real time. The agent
runs to completion without user input.

```rust
pub async fn run_streaming(
    agent_command: &str,
    instructions: &str,
    message: &str,
    project_root: &Path,
    model: Option<&str>,
) -> Result<()>
```

The `instructions` and `message` are concatenated into a single `TextContent`
block (ACP has no separate system prompt channel).

Key differences from interactive mode:

- A single prompt is sent; no user input is read after that.
- The agent executes autonomously using the tools Ralph provides as tool
  provider: `fs/read_text_file`, `fs/write_text_file`, `terminal/create_terminal`, etc.
- `request_permission()` auto-approves all tool requests.
- The session ends when the agent's `PromptResponse` is received (stop reason:
  `EndTurn`).

### Output Processing

As the agent runs, `RalphClient::session_notification()` renders ACP session
updates to the terminal:

- **`AgentMessageChunk` text:** Bright white, flushed immediately.
- **`AgentThoughtChunk` text:** Dim (`bright_black`), flushed immediately.
- **`ToolCall`:** Tool name in cyan, input parameters dimmed.
- **Tool errors:** Red text, first 5 lines shown.

### Commands That Use Streaming Mode

#### `ralph feature build <name>`

This is the only command that uses streaming mode. The agent reads the spec and
plan and autonomously decomposes the plan into a task DAG.

The process:

1. Ralph validates that the feature has both `spec_path` and `plan_path` set.
2. Ralph creates a root task for the feature via `create_task_with_feature()`.
3. The system prompt (`build_feature_build_system_prompt()`) includes:
   - The full spec content.
   - The full plan content.
   - The root task ID.
   - The feature ID.
   - CLI command templates for `ralph task add` and `ralph task deps add`.
   - Decomposition rules (right-size tasks, reference spec/plan sections,
     include acceptance criteria, use parents for grouping, add dependencies
     for ordering).
4. The agent is launched with the initial message:
   `"Read the spec and plan, then create the task DAG. When done, stop."`
5. The agent reads the plan, identifies work items, and creates tasks by
   executing `ralph task add` and `ralph task deps add` CLI commands through
   Ralph's `terminal/create_terminal` tool.
6. After the agent finishes, Ralph reads back the task tree from the database
   and prints a summary with Unicode box-drawing characters.
7. Feature status is updated to `ready`.

> [!IMPORTANT]
> Ralph does **not** parse JSON from the agent's output to create tasks. Instead,
> the agent uses Ralph's own CLI as a tool, which means task creation goes through
> the same validation and ID generation as manual `ralph task add` commands.

Example of what the agent runs internally (via `terminal/create_terminal`):

```bash
# Create a parent task under the root
PARENT=$(ralph task add "Set up database schema" \
  -d "Create the SQLite schema for the new feature..." \
  --parent t-root123 \
  --feature f-abc456)

# Create a leaf task under the parent
CHILD=$(ralph task add "Add migration logic" \
  -d "Implement schema migration from v2 to v3..." \
  --parent $PARENT \
  --feature f-abc456)

# Add a dependency between tasks
ralph task deps add $PARENT $ANOTHER_TASK
```

## Loop Iteration Mode

### How It Works

`acp::connection::run_iteration()` is the execution mode used by the main agent
loop. It spawns an ACP agent, sends the full iteration context as a prompt,
streams output, and returns a `StreamingResult` for the loop to act on.

```rust
pub async fn run_iteration(
    config: &Config,
    context: &IterationContext,
) -> Result<RunResult>
```

### ACP Lifecycle

1. Parse `config.agent_command` with `shlex::split()`.
2. Spawn agent subprocess with `RALPH_MODEL`, `RALPH_ITERATION`, `RALPH_TOTAL` env vars set.
3. Wrap stdio with `tokio_util::compat` for futures IO compatibility.
4. Create `LocalSet`, run ACP session lifecycle inside it:
   - `ClientSideConnection::new(RalphClient, outgoing, incoming, spawn_local)`
   - Spawn io_future via `spawn_local` to drive JSON-RPC transport
   - `conn.initialize()` with fs + terminal capabilities
   - `conn.new_session(cwd = project_root)`
   - Build prompt text: `prompt::build_prompt_text(config, context)`
   - `tokio::select!` racing `conn.prompt(...)` against interrupt polling
5. Collect `StreamingResult` from `RalphClient` state.
6. Kill/wait agent process.
7. Return `RunResult::Completed(result)` or `RunResult::Interrupted`.

### Interrupt Handling

Interrupt polling runs concurrently with `conn.prompt()` via `tokio::select!`.
When the interrupt flag is set (Ctrl+C), `conn.cancel(CancelNotification)` is
sent to the agent and `RunResult::Interrupted` is returned. The existing
feedback prompt, journal logging, and claim release logic in `run_loop.rs`
then handles cleanup.

Double Ctrl+C still force-exits via `std::process::exit(130)`.

### Stop Reason Mapping

The `PromptResponse.stop_reason` is mapped in `run_loop.rs`:

| Stop Reason | Outcome |
| --- | --- |
| `EndTurn` | Normal completion — extract sigils, update DAG |
| `Cancelled` | `RunResult::Interrupted` (handled by interrupt select! path) |
| `MaxTokens` / `MaxTurnRequests` | Release claim, journal `"blocked"`, log warning |
| `Refusal` | Fail the task, journal `"failed"` |
| Unknown variants | Release claim, journal `"blocked"`, log warning |

### Sigil Extraction

After `EndTurn`, the run loop calls `acp::sigils::extract_sigils(&full_text)`
on the accumulated agent text, producing a `SigilResult`:

| Sigil | `SigilResult` Field |
| --- | --- |
| `<task-done>ID</task-done>` | `task_done: Option<String>` |
| `<task-failed>ID</task-failed>` | `task_failed: Option<String>` |
| `<next-model>M</next-model>` | `next_model_hint: Option<String>` |
| `<promise>COMPLETE</promise>` | `is_complete: bool` |
| `<promise>FAILURE</promise>` | `is_failure: bool` |
| `<journal>notes</journal>` | `journal_notes: Option<String>` |
| `<knowledge ...>body</knowledge>` | `knowledge_entries: Vec<KnowledgeSigil>` |

### Cost and Duration

ACP does not report API cost. Journal entries record `cost_usd = 0.0`.
Duration is tracked by Ralph's own `Instant::now()` timer and stored as
`duration_secs`. The cost line is omitted from journal rendering when
`cost_usd == 0.0`.

### Return Value

`run_iteration()` returns `RunResult`:

```rust
pub enum RunResult {
    Completed(StreamingResult),
    Interrupted,
}

pub struct StreamingResult {
    pub full_text: String,          // Accumulated agent text (for sigil extraction)
    pub files_modified: Vec<String>, // Paths written via fs/write_text_file
    pub duration_ms: u64,           // Wall-clock duration (tracked by Ralph)
    pub stop_reason: acp::StopReason,
}
```

The run loop uses these fields to update the DAG and decide what to do next.
See the [Agent Loop][agent-loop] documentation for the full iteration lifecycle.

### Logging

Every loop iteration writes raw JSON-RPC messages to a log file under
`$TMPDIR/ralph/logs/<project>/<timestamp>.log` via a tee adapter on the
agent's stdout. The log file path is printed to the terminal as a clickable
file hyperlink.

## Comparison Table

| Aspect               | Interactive                      | Streaming                           | Loop Iteration                        |
| -------------------- | -------------------------------- | ----------------------------------- | ------------------------------------- |
| User input           | Yes (Ralph mediates via stdin)   | No                                  | No                                    |
| System prompt        | Feature/task-specific            | Feature-specific + CLI instructions | Task-specific + full iteration context |
| Sigil parsing        | No                               | No                                  | Yes (via `extract_sigils()`)          |
| Agent tool provider  | Yes (Ralph: fs + terminal)       | Yes (Ralph: fs + terminal)          | Yes (Ralph: fs + terminal)            |
| Output handling      | ACP session updates to terminal  | ACP session updates to terminal     | ACP session updates to terminal       |
| Log file             | No                               | No                                  | Yes (raw JSON-RPC tee)                |
| Read-only mode       | No                               | No                                  | Verification only                     |
| Session persistence  | One agent per session            | One agent per session               | One agent per iteration               |
| Returns              | `Result<()>`                     | `Result<()>`                        | `Result<RunResult>`                   |
| Commands             | spec, plan, task create          | feature build                       | ralph run                             |

## Context Assembly

### For Interactive Sessions

`gather_project_context()` in `main.rs` assembles a project context string
embedded in the system prompt for all interactive commands:

1. **CLAUDE.md** -- Read from the project root. Truncated to 10,000 characters
   via `truncate_context()` if the file is large.
2. **.ralph.toml** -- Configuration file content, wrapped in a TOML code block.
3. **Features list** -- A markdown table of existing features with columns for
   Name, Status, Has Spec, and Has Plan. Queried from the database.
4. **Standalone tasks** -- (Optional, only for `task create`.) A bullet list
   of existing standalone tasks with their ID, status, and title.

If any source is unavailable, it degrades gracefully (e.g., `[Not found]` for
a missing CLAUDE.md).

### For Loop Iterations

`build_iteration_context()` in `run_loop.rs` assembles an `IterationContext`
struct with richer, task-specific information:

- **Task info** -- ID, title, description.
- **Parent context** -- Parent task's title and description (if the task has a
  parent).
- **Completed blockers** -- For each done dependency: task ID, title, and
  summary (most recent log entry, falling back to description).
- **Retry info** -- Attempt number, max retries, and the previous failure
  reason from `task_logs` (only present when `retry_count > 0`).
- **Spec and plan content** -- Loaded once at loop initialization and reused
  across all iterations.
- **Journal context** -- Recent iteration records (up to 3000-token budget).
- **Knowledge context** -- Relevant tagged knowledge entries (up to 2000-token budget).
- **run_id** -- Current run identifier, used to group journal entries.

`prompt::build_prompt_text(config, context)` concatenates the system
instructions and task context into a single string that is sent as a
`TextContent` block in the ACP `PromptRequest`.

## Output Formatting

All modes use `RalphClient::session_notification()` to handle ACP session
updates received during the prompt:

| Session Update Type  | Rendering                                                           |
| -------------------- | ------------------------------------------------------------------- |
| `AgentMessageChunk`  | Bright white text, flushed immediately for real-time display.       |
| `AgentThoughtChunk`  | Dim text (`bright_black`), flushed immediately.                     |
| `ToolCall`           | Tool name in cyan, input parameters dimmed.                         |
| Tool errors          | Red text, first 5 lines shown.                                      |

Raw JSON-RPC is also written to a log file (loop iteration mode only) via
the stdout tee adapter, under `$TMPDIR/ralph/logs/<project>/`.

Audio notifications are triggered via macOS `say` when the loop completes or
fails.

## Verification Agent

The verification agent is a specialized variant of loop iteration mode, invoked
by `handle_task_done()` when `config.verify` is true. It uses
`acp::connection::run_autonomous()` with `read_only=true`.

Key differences from a normal loop iteration:

- **Writes are rejected** by `RalphClient` in read-only mode (`fs/write_text_file`
  returns an error response to the agent). Terminal operations remain available
  for running tests (`cargo test`, etc.).
- **Does not parse task sigils.** Instead parses verification-specific sigils:
  `<verify-pass/>` and `<verify-fail>reason</verify-fail>`.
- **Prompt is verification-focused.** Built by `build_verification_prompt()`
  with the task details, spec, and plan. Instructs the agent to check the
  implementation, run tests, and emit a verification sigil.

If the verification agent does not emit any sigil, the result is treated as a
verification failure with the reason: "Verification agent did not emit a
verification sigil."

## Key Source Files

| File                        | Role                                                       |
| --------------------------- | ---------------------------------------------------------- |
| `src/acp/interactive.rs`    | `run_interactive()` and `run_streaming()`                  |
| `src/acp/connection.rs`     | `run_iteration()`, `run_autonomous()`, agent spawning      |
| `src/acp/client_impl.rs`    | `RalphClient`: ACP `Client` trait implementation           |
| `src/acp/tools.rs`          | Terminal session management                                |
| `src/acp/streaming.rs`      | Session update rendering to terminal                       |
| `src/acp/sigils.rs`         | Sigil extraction from accumulated text                     |
| `src/acp/prompt.rs`         | Prompt text construction: system instructions + context    |
| `src/acp/types.rs`          | `RunResult`, `StreamingResult`, `SigilResult`, context types |
| `src/main.rs`               | System prompt builders, context gathering                  |
| `src/run_loop.rs`           | Loop orchestration, iteration context building             |
| `src/verification.rs`       | Verification agent (read-only ACP session)                 |
| `src/output/formatter.rs`   | Terminal formatting with ANSI colors                       |
| `src/output/logger.rs`      | Log file path generation                                   |

[agent-loop]: ./agent-loop.md

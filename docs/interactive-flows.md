# Interactive Flows

Ralph spawns Claude Code in three distinct modes depending on the command.
Each mode controls how the process is configured, whether the user can interact,
and how the output is processed. This document explains each mode, when it is
used, and how it works.

## Overview

| Mode               | User Input | Key Function          | Commands                      |
| ------------------ | ---------- | --------------------- | ----------------------------- |
| Interactive        | Yes        | `run_interactive()`   | `feature spec`, `feature plan`, `task create` |
| Streaming          | No         | `run_streaming()`     | `feature build`               |
| Loop Iteration     | No         | `claude::client::run()` | `ralph run`                 |

All three modes live under `src/claude/`. Interactive and streaming modes are
defined in `interactive.rs`. Loop iteration mode is defined in `client.rs`.

## Interactive Mode

### How It Works

`run_interactive()` spawns the `claude` CLI with inherited stdin, stdout, and
stderr. The user sees Claude's output directly in the terminal and can type
responses. Ralph passes a system prompt and an initial message, then relinquishes
control to the user for the duration of the session.

The function signature:

```rust
pub fn run_interactive(
    system_prompt: &str,
    initial_message: &str,
    model: Option<&str>,
) -> Result<()>
```

The `initial_message` is passed as a positional argument to `claude`, causing
Claude to respond immediately when the session opens rather than waiting for user
input.

### CLI Arguments

```
claude --system-prompt <prompt> [--model <model>] <initial_message>
```

No `--print`, no `--output-format`, no `--dangerously-skip-permissions`. Claude
runs with its standard interactive permissions -- the user approves tool calls
as normal.

### Commands That Use Interactive Mode

#### `ralph feature spec <name>`

Claude interviews the user about requirements, then writes `spec.md`.

The system prompt (built by `build_feature_spec_system_prompt()` in `main.rs`)
instructs Claude to:

- Ask about functional requirements, technical constraints, edge cases, testing
  requirements, and dependencies.
- Produce a structured spec document with sections for Overview, Requirements,
  Architecture, API/Interface, Data Models, Testing, and Dependencies.
- Write the result to `.ralph/features/<name>/spec.md`.

The system prompt includes project context assembled by
`gather_project_context()` (see [Context Assembly](#context-assembly-for-interactive-sessions)).

#### `ralph feature plan <name>`

Claude reads the spec and interactively creates `plan.md`.

The system prompt (`build_feature_plan_system_prompt()`) includes the full spec
content and instructs Claude to create a detailed implementation plan with
phases, per-phase details, verification criteria, and risk areas. The plan is
written to `.ralph/features/<name>/plan.md`.

Requires that `feature spec` has already been run (the feature must have a
`spec_path` in the database).

#### `ralph task create`

Claude interviews the user to understand what they want done, then creates a
standalone task in the Ralph database.

The system prompt (`build_task_new_system_prompt()`) includes project context
with the `include_tasks` flag set to `true`, so existing standalone tasks are
listed. Claude asks about the desired work, specific files, acceptance criteria,
and priority.

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

All interactive commands accept `--model <model>` to choose which Claude model
to use. When omitted, the system default applies:

```bash
ralph feature spec my-feature --model opus
ralph feature plan my-feature --model sonnet
ralph task create --model haiku
```

## Streaming Mode

### How It Works

`run_streaming()` spawns Claude in a non-interactive, autonomous mode. Claude
runs to completion without user input. Output is piped and formatted in real
time.

The CLI arguments:

```
claude --print --verbose \
       --output-format stream-json \
       --dangerously-skip-permissions \
       --system-prompt <prompt> \
       [--model <model>] \
       <initial_message>
```

Key differences from interactive mode:

- `--print` makes Claude produce output to stdout rather than opening an
  interactive session.
- `--output-format stream-json` switches output to NDJSON for structured
  parsing.
- `--dangerously-skip-permissions` allows Claude to execute tool calls
  autonomously without user approval.
- stdout is piped (not inherited) and processed through `stream_output()`.
- stderr is drained on a background thread to prevent pipe buffer deadlocks.

### Output Processing

stdout is piped to `stream_output()` (defined in `client.rs`), which reads
NDJSON lines and formats them for terminal display:

- **StreamDelta (thinking):** Dim text (`bright_black`), flushed immediately.
- **StreamDelta (text):** Bright white, flushed immediately.
- **ToolUse:** Tool name in cyan, input parameters in dimmed text.
- **ToolErrors:** Red, first 5 lines shown, XML tags stripped.
- **Result:** Green checkmark with duration in seconds and cost in USD.

No log file is written in streaming mode (`log_file` is `None`).

### Commands That Use Streaming Mode

#### `ralph feature build <name>`

This is the only command that uses streaming mode. Claude reads the spec and plan
and autonomously decomposes the plan into a task DAG.

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
4. Claude is launched with the initial message:
   `"Read the spec and plan, then create the task DAG. When done, stop."`
5. Claude reads the plan, identifies work items, and creates tasks by executing
   `ralph task add` and `ralph task deps add` CLI commands through its Bash
   tool.
6. After Claude finishes, Ralph reads back the task tree from the database and
   prints a summary with Unicode box-drawing characters.
7. Feature status is updated to `ready`.

> [!IMPORTANT]
> Ralph does **not** parse JSON from Claude's output to create tasks. Instead,
> Claude uses Ralph's own CLI as a tool, which means task creation goes through
> the same validation and ID generation as manual `ralph task add` commands.

Example of what Claude runs internally:

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

`claude::client::run()` is the execution mode used by the main agent loop. It
spawns Claude as an autonomous process, streams output, and returns a parsed
`ResultEvent` for the loop to act on.

```rust
pub fn run(
    config: &Config,
    log_file: Option<&str>,
    context: Option<&IterationContext>,
) -> Result<Option<ResultEvent>>
```

### CLI Arguments

Built by `build_claude_args()`:

```
claude --print --verbose \
       --output-format stream-json \
       --no-session-persistence \
       --model <current_model> \
       --system-prompt <system_prompt> \
       @<prompt_file> \
       [--dangerously-skip-permissions | --allowed-tools <tools>]
```

Key differences from streaming mode:

- `--no-session-persistence` prevents Claude from resuming a previous
  conversation.
- `--model` is explicitly set by the model strategy for each iteration.
- `@<prompt_file>` reads the task description from a file.
- Tool permissions depend on sandboxing: when the sandbox is enabled,
  `--dangerously-skip-permissions` is used (the sandbox itself restricts file
  access); when disabled, an explicit `--allowed-tools` list is passed.

### Execution Paths

Two execution paths exist:

**Direct** (`run_direct()`). Spawns `claude` as a child process:

```
claude [args...]
```

stdout is piped for streaming. stderr is drained on a background thread.

**Sandboxed** (`run_sandboxed()`). Wraps the invocation in macOS
`sandbox-exec`:

```
sandbox-exec -f <profile> \
  -D PROJECT_DIR=<cwd> \
  -D HOME=<home> \
  -D ROOT_GIT_DIR=<git_root> \
  claude [args...]
```

The sandbox profile is generated dynamically by `sandbox::profile::generate()`,
written to a temp file, and cleaned up after the process exits. It denies all
writes except the project directory, temp dirs, Claude state directories
(`~/.claude`, `~/.config/claude`), `~/.cache`, `~/.local/state`, and git
worktree roots.

### Sigil Parsing

Loop iteration mode is the only mode that parses sigils from Claude's output.
The parser (in `parser.rs`) extracts sigils during NDJSON result event
deserialization:

| Sigil                                   | Parser Function           | ResultEvent Field    |
| --------------------------------------- | ------------------------- | -------------------- |
| `<task-done>ID</task-done>`             | `parse_task_done()`       | `task_done`          |
| `<task-failed>ID</task-failed>`         | `parse_task_failed()`     | `task_failed`        |
| `<next-model>M</next-model>`            | `parse_next_model_hint()` | `next_model_hint`    |
| `<promise>COMPLETE</promise>`           | `is_complete()`           | Checked via method   |
| `<promise>FAILURE</promise>`            | `is_failure()`            | Checked via method   |

If both `<task-done>` and `<task-failed>` appear in the same output, the parser
resolves the conflict optimistically: `task-done` wins and `task-failed` is
discarded.

### Return Value

Unlike the other modes (which return `Result<()>`), loop iteration mode returns
`Result<Option<ResultEvent>>`. The `ResultEvent` contains:

- `result` -- The full text output from Claude.
- `duration_ms` -- How long the session ran.
- `total_cost_usd` -- API cost for the session.
- `next_model_hint` -- Model hint for the next iteration, if any.
- `task_done` -- Task ID from a `<task-done>` sigil, if present.
- `task_failed` -- Task ID from a `<task-failed>` sigil, if present.

The run loop uses these fields to update the DAG and decide what to do next.
See the [Agent Loop][agent-loop] documentation for the full iteration lifecycle.

### Logging

Every loop iteration writes raw NDJSON to a log file under
`$TMPDIR/ralph/logs/<project>/<timestamp>.log`. The log file path is printed to
the terminal as a clickable file hyperlink. Streaming mode does not write log
files.

## Comparison Table

| Aspect               | Interactive                    | Streaming                           | Loop Iteration                       |
| -------------------- | ------------------------------ | ----------------------------------- | ------------------------------------ |
| User input           | Yes (inherited stdio)          | No                                  | No                                   |
| System prompt        | Feature/task-specific          | Feature-specific + CLI instructions | Task-specific + full iteration context |
| Sigil parsing        | No                             | No                                  | Yes                                  |
| Sandbox support      | No                             | No                                  | Yes (optional)                       |
| Output handling      | Direct to terminal             | `stream_output()` formatting        | `stream_output()` formatting         |
| Log file             | No                             | No                                  | Yes                                  |
| Permissions          | Standard Claude permissions    | `--dangerously-skip-permissions`    | `--dangerously-skip-permissions` or `--allowed-tools` |
| Session persistence  | Default (persisted)            | Default (persisted)                 | `--no-session-persistence`           |
| Returns              | `Result<()>` (exit code)       | `Result<()>` (exit code)            | `Result<Option<ResultEvent>>`        |
| Commands             | spec, plan, task create        | feature build                       | ralph run                            |

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
- **Skills summary** -- Discovered from `.ralph/skills/*/SKILL.md` by parsing
  YAML frontmatter for the `description` field.
- **Spec and plan content** -- Loaded once at loop initialization and reused
  across all iterations.
- **Learn flag** -- Whether skill creation and CLAUDE.md updates are enabled.

This context is passed to `build_system_prompt()` which constructs the full
markdown system prompt with conditional sections (retry info, skills, learning
instructions are only included when relevant).

## Output Formatting

Both streaming and loop iteration modes use `stream_output()` to process NDJSON
events from Claude's stdout:

| Event Type      | Rendering                                              |
| --------------- | ------------------------------------------------------ |
| `StreamDelta`   | Thinking text in dim (`bright_black`), regular text in bright white. Flushed immediately for real-time display. |
| `Assistant`     | Model name in purple. Text content blocks in bright white. Thinking blocks prefixed with dim `|` bar. |
| `ToolUse`       | Tool name in cyan (`-> ToolName`), input parameters dimmed and truncated to 80 chars per value. |
| `ToolErrors`    | Red text, first 5 lines shown, XML tags stripped.      |
| `Result`        | Green checkmark with duration (seconds) and cost (USD). |

Raw NDJSON is also written to a log file (loop iteration mode only) under
`$TMPDIR/ralph/logs/<project>/`.

Audio notifications are triggered via macOS `say` when the loop completes or
fails.

## Verification Agent

The verification agent is a specialized variant of loop iteration mode, invoked
by `handle_task_done()` when `config.verify` is true. It uses
`run_direct_with_args()` rather than the full `run()` path.

Key differences from a normal loop iteration:

- **Tools are restricted** to `Bash Read Glob Grep` (read-only access).
- **Uses `--allowed-tools`** instead of `--dangerously-skip-permissions`,
  regardless of sandbox settings.
- **Does not parse task sigils.** Instead parses verification-specific sigils:
  `<verify-pass/>` and `<verify-fail>reason</verify-fail>`.
- **Prompt is verification-focused.** Built by `build_verification_prompt()`
  with the task details, spec, and plan. Instructs Claude to check the
  implementation, run tests, and emit a verification sigil.

If the verification agent does not emit any sigil, the result is treated as a
verification failure with the reason: "Verification agent did not emit a
verification sigil."

## Key Source Files

| File                        | Role                                           |
| --------------------------- | ---------------------------------------------- |
| `src/claude/interactive.rs` | `run_interactive()` and `run_streaming()`       |
| `src/claude/client.rs`      | `run()`, `stream_output()`, system prompt build |
| `src/claude/events.rs`      | Event types, sigil parsers                      |
| `src/claude/parser.rs`      | NDJSON line deserialization                     |
| `src/main.rs`               | System prompt builders, context gathering       |
| `src/run_loop.rs`           | Loop orchestration, iteration context building  |
| `src/verification.rs`       | Verification agent                              |
| `src/output/formatter.rs`   | Terminal formatting with ANSI colors            |
| `src/output/logger.rs`      | Log file path generation                        |

[agent-loop]: ./agent-loop.md

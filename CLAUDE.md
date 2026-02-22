# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (stripped, LTO)
cargo test               # Run tests
cargo run -- --help      # Show CLI usage
cargo run -- run --once  # Run single iteration (for testing)
```

## Project Overview

Ralph is an autonomous agent loop harness that iteratively invokes Claude Code until tasks are complete. It decomposes work into a DAG of tasks stored in SQLite, picks ready tasks one at a time, and loops until all tasks are resolved or a limit is hit.

## Architecture

### Core Loop (`src/run_loop.rs`)
DAG-driven async loop: open `.ralph/progress.db`, get scoped ready tasks, claim one, build iteration context (parent, blockers, spec/plan, retry info, journal/knowledge context), run ACP agent session, handle sigils, verify (if enabled), retry on failure, repeat. Uses tokio async runtime with `run_iteration()` driving the ACP connection lifecycle.

**Outcome enum**: `Complete`, `Failure`, `LimitReached`, `Blocked` (no ready tasks but incomplete tasks remain), `NoPlan` (DAG is empty), `Interrupted` (user pressed Ctrl+C and chose not to continue).

**Key functions**: `resolve_feature_context()` loads spec/plan for feature targets. `get_scoped_ready_tasks()` filters by feature or task ID. `build_iteration_context()` assembles full context including journal + knowledge. `handle_task_done()` orchestrates verification + retry. `journal::select_journal_entries()` picks relevant past iteration records. `knowledge::discover_knowledge()` and `knowledge::match_knowledge_entries()` find and score relevant project knowledge.

### DAG Task System (`src/dag/`)
SQLite-based task management with WAL mode and foreign keys (schema v3):
- **mod.rs**: Defines `Task` (with `status`, `priority`, `created_at`, `updated_at`, `claimed_by`, `task_type`, `feature_id`, `retry_count`, `max_retries`, `verification_status`) and `TaskCounts` structs. Provides `task_from_row()` helper and `TASK_COLUMNS` constant for consistent SQL queries. Re-exports main task operations. Implements scoped queries: `get_ready_tasks_for_feature()`, `get_standalone_tasks()`, `get_feature_task_counts()`, `retry_task()`. Force-transition functions: `force_complete_task()`, `force_fail_task()`, `force_reset_task()`.
- **db.rs**: Opens/creates `.ralph/progress.db`, defines schema (`tasks`, `dependencies`, `task_logs`, `features`, `journal` tables). Schema v2 adds `features` table and extends `tasks` with `feature_id`, `task_type`, `retry_count`, `max_retries`, `verification_status`. Schema v3 adds `journal` table with FTS5 full-text search index for iteration history.
- **ids.rs**: Generates task IDs (`t-{6 hex}`) and feature IDs (`f-{6 hex}`) from SHA-256 of timestamp+counter.
- **tasks.rs**: `compute_parent_status()` derives parent status from children (any failed -> failed, all done -> done). `get_task_status()` returns derived status for a task considering its children.
- **transitions.rs**: Status transitions (`pending`→`in_progress`→`done`/`failed`) with auto-transitions: completing a task unblocks dependents; completing all children auto-completes parent; failing a child auto-fails parent
- **dependencies.rs**: Dependency management with BFS cycle detection
- **crud.rs**: Task CRUD operations (`create_task`, `create_task_with_feature`, `get_task`, `update_task`, `delete_task`, `add_log`, `get_task_logs`, `get_task_blockers`, `get_tasks_blocked_by`, `get_all_tasks_for_feature`, `get_all_tasks`). Defines `LogEntry` struct.

### Feature Management (`src/feature.rs`)
CRUD operations for features: `create_feature`, `get_feature` (by name), `get_feature_by_id`, `list_features`, `update_feature_status/spec_path/plan_path`, `ensure_feature_dirs`, `read_spec`, `read_plan`, `feature_exists`.

### Verification Agent (`src/verification.rs`)
Spawns a read-only ACP agent session to verify completed tasks. Uses `run_autonomous()` with `read_only=true` — the `RalphClient` rejects `fs/write_text_file` but permits terminal operations so the agent can run `cargo test`. Parses `<verify-pass/>` and `<verify-fail>reason</verify-fail>` sigils.

### Interrupt Handling (`src/interrupt.rs`)
Graceful Ctrl+C support using `signal-hook`. Registers a SIGINT handler that sets an `AtomicBool` flag on first press and force-exits (`exit(130)`) on second press. The stream reader checks `is_interrupted()` each line and returns `StreamResult::Interrupted` early, which propagates up as `RunResult::Interrupted`. On interrupt the run loop: prints an interrupted banner, prompts the user for task feedback (multi-line, empty to skip), appends feedback to the task description as a `**User Guidance**` section, logs a journal entry with outcome `"interrupted"`, releases the task claim via `release_claim()`, and asks whether to continue. Key functions: `register_signal_handler()`, `is_interrupted()`, `clear_interrupt()`, `prompt_for_feedback()`, `append_feedback_to_description()`, `should_continue()`.

### ACP Integration (`src/acp/`)
Ralph communicates with AI agents via the Agent Client Protocol (ACP) — a JSON-RPC 2.0 standard over stdin/stdout. Ralph is the ACP client and tool provider; any ACP-compliant agent binary can be used.

- **connection.rs**: Agent spawning, ACP connection lifecycle, `run_iteration()` (the main entry point for the agent loop), `run_autonomous()` (for verification, review, and feature create (build phase)). Handles interrupt cancellation via `tokio::select!`, stop reason mapping, and process cleanup.
- **client_impl.rs**: `RalphClient` implementing the ACP `Client` trait. Handles `session_notification` (accumulates text, renders output), `request_permission` (auto-approve normal / deny writes in read-only mode), and all tool calls delegated from the agent.
- **tools.rs**: Terminal session management — `TerminalSession` struct with stdout/stderr reader tasks, `create_terminal`, `terminal_output`, `wait_for_terminal_exit`, `kill_terminal_command`, `release_terminal` handlers.
- **prompt.rs**: Prompt text construction — `build_prompt_text()` concatenates system instructions and task context into a single `TextContent` block (ACP has no separate system prompt channel). Migrated from `claude/client.rs`.
- **sigils.rs**: Sigil extraction — `extract_sigils()` combines all parsers into a `SigilResult`. Individual parsers: `parse_task_done()`, `parse_task_failed()`, `parse_next_model_hint()`, `parse_journal_sigil()`, `parse_knowledge_sigils()`.
- **streaming.rs**: Session update rendering — maps ACP `SessionUpdate` variants to terminal output (text in bright white, thoughts in dim, tool calls in cyan/dimmed).
- **interactive.rs**: `run_interactive()` — ACP-mediated interactive sessions where Ralph reads user input and sends it as prompts; `run_streaming()` — single autonomous prompt for feature create (build phase).
- **types.rs**: `RunResult` (`Completed(StreamingResult)` / `Interrupted`), `StreamingResult` (full_text, files_modified, duration_ms, stop_reason), `SigilResult`, `IterationContext`, `TaskInfo`, `ParentContext`, `BlockerContext`, `RetryInfo`, `KnowledgeSigil`.

### Project Configuration (`src/project.rs`)
- Discovers `.ralph.toml` by walking up directory tree from CWD
- `ralph init` creates `.ralph.toml`, `.ralph/` directory structure (including `features/`, `knowledge/`), `.claude/skills/` directory, and empty `progress.db`. Includes backward-compat migration for legacy `.ralph/skills/` directories.
- Config sections: `[execution]` (max_retries, verify), `[agent]` (command, default: `"claude"`)

### Config (`src/config.rs`)
- Holds the `Config` struct: agent command, limits, iteration counters, model strategy, agent ID, run ID, project root, parsed `RalphConfig`, max_retries, verify, run_target
- Defines the `ModelStrategy` enum: `Fixed`, `CostOptimized`, `Escalate`, `PlanThenExecute`
- Defines the `RunTarget` enum: `Feature(String)`, `Task(String)`
- Generates agent IDs: `agent-{8 hex}` from `DefaultHasher` over timestamp + PID
- Generates run IDs: `run-{8 hex}` via `generate_run_id()` from SHA-256 of timestamp + counter
- `agent_command` resolved via: `--agent` flag > `RALPH_AGENT` env > `[agent].command` in `.ralph.toml` > `"claude"`. Validated with `shlex::split()` — error on malformed input (e.g. unclosed quotes).

### Output (`src/output/`)
- **formatter.rs**: Terminal output formatting with ANSI colors via `colored` crate. Renders streaming deltas (thinking in dim, text in bright white), tool use (cyan name, dimmed input), tool errors (red, first 5 lines), result summaries (duration + cost in green). Uses macOS `say` for audio notifications. Clickable file hyperlinks via terminal escape codes.
- **logger.rs**: Generates log file paths under `$TMPDIR/ralph/logs/<project_name>/<timestamp>.log`

### Model Strategy (`src/strategy.rs`)
Selects which Claude model to use each iteration based on `--model-strategy`:
- **Fixed**: Always returns the `--model` value
- **CostOptimized** (default): Defaults to `sonnet`; escalates to `opus` on error signals; drops to `haiku` on clean completions
- **Escalate**: Starts at `haiku`, escalates to `sonnet`/`opus` on failure signals. Monotonic — only de-escalates via Claude hint.
- **PlanThenExecute**: `opus` for iteration 1, `sonnet` for all subsequent iterations

Claude can override any strategy for the next iteration via `<next-model>opus|sonnet|haiku</next-model>`.

### Journal (`src/journal.rs`)
Persistent iteration records stored in SQLite with FTS5 full-text search. Each `JournalEntry` records `run_id`, `iteration`, `task_id`, `feature_id`, `outcome`, `model`, `duration_secs`, `cost_usd`, `files_modified`, and `notes` (from `<journal>` sigil). Smart selection combines recent entries from the current run with FTS-matched entries from prior runs. Rendered into the system prompt within a 3000-token budget.

### Knowledge Base (`src/knowledge.rs`)
Tag-based project knowledge stored as markdown files in `.ralph/knowledge/`. Each `KnowledgeEntry` has YAML frontmatter (`title`, `tags`, optional `feature`, `created_at`) and a body (max ~500 words). Discovery scans the directory; matching scores entries by tag relevance to the current task, feature, and recently modified files. Deduplication on write: exact title match replaces, >50% tag overlap merges, otherwise creates new. Rendered into the system prompt within a 2000-token budget.

### Sigils
Claude's output is scanned for:
- `<task-done>{task_id}</task-done>` - Mark assigned task as done
- `<task-failed>{task_id}</task-failed>` - Mark assigned task as failed
- `<promise>COMPLETE</promise>` - All tasks done, exit 0
- `<promise>FAILURE</promise>` - Critical failure, exit 1
- `<next-model>MODEL</next-model>` - Hint for next iteration's model
- `<verify-pass/>` - Verification passed (emitted by verification agent)
- `<verify-fail>reason</verify-fail>` - Verification failed (emitted by verification agent)
- `<journal>notes</journal>` - Iteration notes for run journal
- `<knowledge tags="..." title="...">body</knowledge>` - Reusable project knowledge entry

### Key Files
- `.ralph.toml` - Project configuration (discovered by walking up directory tree)
- `.ralph/progress.db` - SQLite DAG database (gitignored)
- `.ralph/features/<name>/spec.md` - Feature specifications
- `.ralph/features/<name>/plan.md` - Feature implementation plans
- `.claude/skills/<name>/SKILL.md` - Reusable agent skills with YAML frontmatter
- `.ralph/knowledge/<name>.md` - Project knowledge entries with YAML frontmatter

## CLI

Ralph uses a nested subcommand architecture:

```
ralph init                        # Initialize project (.ralph.toml, .ralph/ dirs)
ralph feature create <name>       # Unified: interview → spec → plan → task DAG
ralph feature list                # List features and status
ralph task add <TITLE> [flags]     # Non-interactive task creation (scriptable)
ralph task create [--model M]     # Interactive ACP-assisted task creation
ralph task show <ID> [--json]     # Full task details
ralph task list [filters] [--json] # List tasks (feature-scoped, filterable)
ralph task update <ID> [flags]    # Update task fields
ralph task delete <ID>            # Delete a task
ralph task done <ID>              # Mark done (auto-transitions)
ralph task fail <ID> [-r reason]  # Mark failed
ralph task reset <ID>             # Reset to pending
ralph task log <ID> [-m msg]      # Add or view log entries
ralph task deps add <A> <B>       # A must complete before B
ralph task deps rm <A> <B>        # Remove dependency
ralph task deps list <ID>         # Show blockers and dependents
ralph task tree <ID> [--json]     # Indented tree with status colors
ralph run <target>                # Execute scoped work (feature name or task ID)
  --once                          # Single iteration
  --limit=N                       # Max iterations (0=unlimited)
  --model=MODEL                   # opus, sonnet, haiku (implies fixed strategy)
  --model-strategy=STRAT          # fixed, cost-optimized, escalate, plan-then-execute
  --agent=CMD                     # ACP agent command (env: RALPH_AGENT, default: claude)
  --max-retries=N                 # Maximum retries for failed tasks
  --no-verify                     # Disable autonomous verification
```

Environment variables: `RALPH_LIMIT`, `RALPH_MODEL`, `RALPH_MODEL_STRATEGY`, `RALPH_ITERATION`, `RALPH_TOTAL`, `RALPH_AGENT`.

## Understanding Design Decisions

Before modifying code, check two resources that explain the **why** behind implementation choices:

### Documentation (`docs/`)

Prose documentation covering system design, data flows, and architectural rationale. **Read the relevant doc before changing a subsystem** — they contain design constraints and tradeoffs that aren't obvious from code alone.

- **architecture.md** — Full system design: module structure, data model, schema, execution modes, ACP protocol. Read when adding a new subsystem or understanding how pieces connect.
- **agent-loop.md** — The ten-step iteration lifecycle inside `run_loop.rs`: context assembly, model selection, sigil parsing, outcome handling. Read when debugging loop behavior or modifying prompt construction.
- **task-management.md** — Task data model, SQLite schema, status state machine, parent-child hierarchies, dependency edges, cycle detection, ready queries, retries. Read when changing task internals or query logic.
- **interactive-flows.md** — Three Claude spawning modes (interactive, streaming, loop iteration), their CLI arguments, and output handling. Read when working on `feature create` or output formatting.
- **specs-plans-tasks.md** — The four-phase feature workflow (spec → plan → build → run), decomposition rules, how context flows from spec through execution. Read when modifying the feature pipeline.
- **oneshot-vs-features.md** — Tradeoffs between one-shot tasks and the feature workflow. Read when changing how run targets are resolved or adding new execution modes.
- **memory-and-learning.md** — Journal/knowledge system, skills (SKILL.md), discovery, system prompt integration. Read when working on journal entries, knowledge persistence, or skill creation.
- **getting-started.md** — User-facing walkthrough of install, init, features, tasks, and running. Read when changing CLI UX or onboarding behavior.

### Project Knowledge (`.ralph/knowledge/`)

Targeted knowledge entries that capture specific gotchas, patterns, and decisions learned during development. Each file has YAML frontmatter with `title` and `tags`, followed by a focused explanation. **Scan these before working on a subsystem** — they record pitfalls and constraints that prevent repeated mistakes.

Examples of what's in here:
- Parameter contracts and call-site update requirements (`config-from-run-args.md`)
- Schema migration patterns and FTS5 trigger gotchas (`schema-migrations.md`)
- ACP connection lifecycle patterns (`acp-connection-lifecycle-pattern-with-localset-and-owned-data.md`)
- Run loop iteration sequence and outcome handling (`run-loop-lifecycle.md`)
- Interrupt flow and signal handling details (`interrupt-handling.md`)
- Testing patterns for ACP mock agents and LocalSet (`mock-acp-agent-binary-pattern-with-agentsideconnection.md`)

To find relevant entries, match the tags in frontmatter against the subsystem you're working on. File names are descriptive — browse the directory listing to find what's relevant.

## Nix Development

```bash
nix develop  # Enters shell with Rust toolchain via rust-overlay
```

## Releases

Uses `cargo-dist` v0.30.3. Config lives in `dist-workspace.toml` (not Cargo.toml).

### Key Files
- `dist-workspace.toml` - cargo-dist configuration (targets, installers, CI settings)
- `.github/workflows/release.yml` - Generated CI workflow, triggers on `v*` tags

### Commands

```bash
dist plan                # Preview what will be built
dist build               # Build for current platform locally
dist generate            # Regenerate CI workflow after config changes
```

### Cutting a Release
1. Bump version in `Cargo.toml`
2. Commit, tag `vX.Y.Z`, push tag
3. CI builds tarballs (.tar.xz), installer script, checksums, and source archive

### Targets
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`

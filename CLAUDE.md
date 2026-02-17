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
DAG-driven loop: open `.ralph/progress.db`, get scoped ready tasks, claim one, build iteration context (parent, blockers, spec/plan, retry info, skills), run Claude, handle sigils, verify (if enabled), retry on failure, repeat. No async runtime — uses synchronous `std::process` with `BufReader::lines()` for streaming.

**Outcome enum**: `Complete`, `Failure`, `LimitReached`, `Blocked` (no ready tasks but incomplete tasks remain), `NoPlan` (DAG is empty).

**Key functions**: `resolve_feature_context()` loads spec/plan for feature targets. `get_scoped_ready_tasks()` filters by feature or task ID. `build_iteration_context()` assembles full context. `handle_task_done()` orchestrates verification + retry. `discover_skills()` scans `.ralph/skills/*/SKILL.md`.

### DAG Task System (`src/dag/`)
SQLite-based task management with WAL mode and foreign keys (schema v2):
- **mod.rs**: Defines `Task` (with `status`, `priority`, `created_at`, `updated_at`, `claimed_by`, `task_type`, `feature_id`, `retry_count`, `max_retries`, `verification_status`) and `TaskCounts` structs. Provides `task_from_row()` helper and `TASK_COLUMNS` constant for consistent SQL queries. Re-exports main task operations. Implements scoped queries: `get_ready_tasks_for_feature()`, `get_standalone_tasks()`, `get_feature_task_counts()`, `retry_task()`. Force-transition functions: `force_complete_task()`, `force_fail_task()`, `force_reset_task()`.
- **db.rs**: Opens/creates `.ralph/progress.db`, defines schema (`tasks`, `dependencies`, `task_logs`, `features` tables). Schema v2 adds `features` table and extends `tasks` with `feature_id`, `task_type`, `retry_count`, `max_retries`, `verification_status`.
- **ids.rs**: Generates task IDs (`t-{6 hex}`) and feature IDs (`f-{6 hex}`) from SHA-256 of timestamp+counter.
- **tasks.rs**: `compute_parent_status()` derives parent status from children (any failed -> failed, all done -> done). `get_task_status()` returns derived status for a task considering its children.
- **transitions.rs**: Status transitions (`pending`→`in_progress`→`done`/`failed`) with auto-transitions: completing a task unblocks dependents; completing all children auto-completes parent; failing a child auto-fails parent
- **dependencies.rs**: Dependency management with BFS cycle detection
- **crud.rs**: Task CRUD operations (`create_task`, `create_task_with_feature`, `get_task`, `update_task`, `delete_task`, `add_log`, `get_task_logs`, `get_task_blockers`, `get_tasks_blocked_by`, `get_all_tasks_for_feature`, `get_all_tasks`). Defines `LogEntry` struct.

### Feature Management (`src/feature.rs`)
CRUD operations for features: `create_feature`, `get_feature` (by name), `get_feature_by_id`, `list_features`, `update_feature_status/spec_path/plan_path`, `ensure_feature_dirs`, `read_spec`, `read_plan`, `feature_exists`.

### Verification Agent (`src/verification.rs`)
Spawns a read-only Claude session to verify completed tasks. Uses restricted tools (`Bash Read Glob Grep`). Parses `<verify-pass/>` and `<verify-fail>reason</verify-fail>` sigils.

### Claude Integration (`src/claude/`)
- **client.rs**: Spawns `claude` CLI with `--output-format stream-json` and `--model <model>`, handles both direct and sandboxed execution. Builds the system prompt with DAG task assignment instructions, spec/plan content, retry info, skills summary, and learning instructions. Defines `TaskInfo`, `ParentContext`, `BlockerContext`, `RetryInfo`, `IterationContext` structs. `build_task_context()` renders task assignment markdown.
- **interactive.rs**: `run_interactive()` spawns Claude with inherited stdio for interactive sessions (used by spec, plan, build, and task create commands).
- **events.rs**: Typed event structs for NDJSON parsing. Parses sigils from result text: `<task-done>`, `<task-failed>`, `<next-model>`, `<promise>COMPLETE/FAILURE</promise>`
- **parser.rs**: Deserializes raw JSON into typed events

### Project Configuration (`src/project.rs`)
- Discovers `.ralph.toml` by walking up directory tree from CWD
- `ralph init` creates `.ralph.toml`, `.ralph/` directory structure (including `features/`, `skills/`), and empty `progress.db`
- Config sections: `[specs]` (dirs), `[prompts]` (dir), `[execution]` (max_retries, verify, learn)

### Config (`src/config.rs`)
- Holds the `Config` struct: prompt file, limits, iteration counters, sandbox settings, model strategy, agent ID, project root, parsed `RalphConfig`, max_retries, verify, learn, run_target
- Defines the `ModelStrategy` enum: `Fixed`, `CostOptimized`, `Escalate`, `PlanThenExecute`
- Defines the `RunTarget` enum: `Feature(String)`, `Task(String)`
- Generates agent IDs: `agent-{8 hex}` from `DefaultHasher` over timestamp + PID
- Default allowed tools list: `Bash`, `Edit`, `Write`, `Read`, `Glob`, `Grep`, `Task`, `TodoWrite`, `NotebookEdit`, `WebFetch`, `WebSearch`, `mcp__*`

### Sandbox (`src/sandbox/`)
macOS `sandbox-exec` integration for filesystem write restrictions:
- **profile.rs**: Generates sandbox.sb profiles dynamically
- **rules.rs**: Defines allow rules (e.g., `--allow=aws` grants `~/.aws` write access)

The sandbox denies all writes except: project directory, temp dirs, Claude state (`~/.claude`, `~/.config/claude`), `~/.cache`, `~/.local/state`, and git worktree roots. Also blocks `com.apple.systemevents` to prevent UI automation.

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

### Sigils
Claude's output is scanned for:
- `<task-done>{task_id}</task-done>` - Mark assigned task as done
- `<task-failed>{task_id}</task-failed>` - Mark assigned task as failed
- `<promise>COMPLETE</promise>` - All tasks done, exit 0
- `<promise>FAILURE</promise>` - Critical failure, exit 1
- `<next-model>MODEL</next-model>` - Hint for next iteration's model
- `<verify-pass/>` - Verification passed (emitted by verification agent)
- `<verify-fail>reason</verify-fail>` - Verification failed (emitted by verification agent)

### Key Files
- `.ralph.toml` - Project configuration (discovered by walking up directory tree)
- `.ralph/progress.db` - SQLite DAG database (gitignored)
- `.ralph/features/<name>/spec.md` - Feature specifications
- `.ralph/features/<name>/plan.md` - Feature implementation plans
- `.ralph/skills/<name>/SKILL.md` - Reusable agent skills with YAML frontmatter
- `.ralph/prompts/` - Prompt files
- `prompt` (default) - Task description file read by Claude each iteration

## CLI

Ralph uses a nested subcommand architecture:

```
ralph init                        # Initialize project (.ralph.toml, .ralph/ dirs)
ralph feature spec <name>         # Interactive: craft a specification
ralph feature plan <name>         # Interactive: create implementation plan from spec
ralph feature build <name>        # Decompose plan into task DAG (interactive CLI-based)
ralph feature list                # List features and status
ralph task add <TITLE> [flags]     # Non-interactive task creation (scriptable)
ralph task create [--model M]     # Interactive Claude-assisted task creation
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
  --no-sandbox                    # Disable macOS sandbox
  --allow=RULE                    # Enable sandbox rule (e.g., aws)
  --max-retries=N                 # Maximum retries for failed tasks
  --no-verify                     # Disable autonomous verification
```

Environment variables: `RALPH_LIMIT`, `RALPH_MODEL`, `RALPH_MODEL_STRATEGY`, `RALPH_ITERATION`, `RALPH_TOTAL`.

## Documentation (`docs/`)

The `docs/` directory contains detailed prose documentation. Consult these when working on or near the relevant subsystem — they have context beyond what this file covers.

- **architecture.md** — Full system design: module structure, data model, schema, execution modes, sandbox. Read when adding a new subsystem or understanding how pieces connect.
- **agent-loop.md** — The ten-step iteration lifecycle inside `run_loop.rs`: context assembly, model selection, sigil parsing, outcome handling. Read when debugging loop behavior or modifying prompt construction.
- **task-management.md** — Task data model, SQLite schema, status state machine, parent-child hierarchies, dependency edges, cycle detection, ready queries, retries. Read when changing task internals or query logic.
- **interactive-flows.md** — Three Claude spawning modes (interactive, streaming, loop iteration), their CLI arguments, and output handling. Read when working on `feature spec/plan/build` or output formatting.
- **specs-plans-tasks.md** — The four-phase feature workflow (spec → plan → build → run), decomposition rules, how context flows from spec through execution. Read when modifying the feature pipeline.
- **oneshot-vs-features.md** — Tradeoffs between one-shot tasks and the feature workflow. Read when changing how run targets are resolved or adding new execution modes.
- **memory-and-learning.md** — Skills (SKILL.md), CLAUDE.md updates, discovery, system prompt integration, `--no-learn`. Read when working on skill creation or context persistence.
- **getting-started.md** — User-facing walkthrough of install, init, features, tasks, and running. Read when changing CLI UX or onboarding behavior.

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

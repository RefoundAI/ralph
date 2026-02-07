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
DAG-driven loop: open `.ralph/progress.db`, get ready tasks, claim one, run Claude, handle sigils, repeat. No async runtime — uses synchronous `std::process` with `BufReader::lines()` for streaming.

**Outcome enum**: `Complete`, `Failure`, `LimitReached`, `Blocked` (no ready tasks but incomplete tasks remain), `NoPlan` (DAG is empty).

### DAG Task System (`src/dag/`)
SQLite-based task management with WAL mode and foreign keys:
- **db.rs**: Opens/creates `.ralph/progress.db`, defines schema (`tasks`, `dependencies`, `task_logs` tables)
- **ids.rs**: Generates task IDs (`t-{6 hex}`) from SHA-256 of timestamp+counter
- **tasks.rs**: `get_ready_tasks()`, `claim_task()`, `complete_task()`, `fail_task()`, `release_claim()`, `all_resolved()`
- **transitions.rs**: Status transitions (`pending`→`in_progress`→`done`/`failed`) with auto-transitions: completing a task unblocks dependents; completing all children auto-completes parent; failing a child auto-fails parent
- **dependencies.rs**: Dependency management with BFS cycle detection
- **crud.rs**: Task CRUD operations (`create_task`, `get_task`, `update_task`, `delete_task`, `add_log`)

### Claude Integration (`src/claude/`)
- **client.rs**: Spawns `claude` CLI with `--output-format stream-json` and `--model <model>`, handles both direct and sandboxed execution. Builds the system prompt with DAG task assignment instructions. Also defines `TaskInfo`, `ParentContext`, `BlockerContext` structs and `build_task_context()` for rendering task assignment markdown.
- **events.rs**: Typed event structs for NDJSON parsing. Parses sigils from result text: `<task-done>`, `<task-failed>`, `<next-model>`, `<promise>COMPLETE/FAILURE</promise>`
- **parser.rs**: Deserializes raw JSON into typed events

### Project Configuration (`src/project.rs`)
- Discovers `.ralph.toml` by walking up directory tree from CWD
- `ralph init` creates `.ralph.toml`, `.ralph/` directory structure, and empty `progress.db`
- Config sections: `[specs]` (dirs), `[prompts]` (dir)

### Sandbox (`src/sandbox/`)
macOS `sandbox-exec` integration for filesystem write restrictions:
- **profile.rs**: Generates sandbox.sb profiles dynamically
- **rules.rs**: Defines allow rules (e.g., `--allow=aws` grants `~/.aws` write access)

The sandbox denies all writes except: project directory, temp dirs, Claude state (`~/.claude`, `~/.config/claude`), `~/.cache`, `~/.local/state`, and git worktree roots. Also blocks `com.apple.systemevents` to prevent UI automation.

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

### Key Files
- `.ralph.toml` - Project configuration (discovered by walking up directory tree)
- `.ralph/progress.db` - SQLite DAG database (gitignored)
- `.ralph/specs/` - Specification documents
- `.ralph/prompts/` - Prompt files
- `prompt` (default) - Task description file read by Claude each iteration

## CLI

Ralph uses a subcommand architecture:

```
ralph init                    # Initialize project (.ralph.toml, .ralph/ dirs)
ralph run [PROMPT_FILE]       # Run agent loop (default prompt: "prompt")
  --once                      # Single iteration
  --limit=N                   # Max iterations (0=unlimited)
  --model=MODEL               # opus, sonnet, haiku (implies fixed strategy)
  --model-strategy=STRAT      # fixed, cost-optimized, escalate, plan-then-execute
  --no-sandbox                # Disable macOS sandbox
  --allow=RULE                # Enable sandbox rule (e.g., aws)
ralph plan [PROMPT_FILE]      # Decompose prompt into task DAG (not yet implemented)
ralph specs                   # Author specification documents (not yet implemented)
ralph prompt                  # Create a new prompt file (not yet implemented)
```

Environment variables: `RALPH_FILE`, `RALPH_LIMIT`, `RALPH_MODEL`, `RALPH_MODEL_STRATEGY`, `RALPH_ITERATION`, `RALPH_TOTAL`.

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

# Getting Started

This guide walks through installing Ralph, initializing a project, and using it
to plan and execute work autonomously through Claude Code.

## Prerequisites

- **Rust toolchain** -- install via [rustup][rustup], or use `nix develop` if
  you have Nix (the flake provides the full toolchain)
- **Claude Code CLI** -- installed and authenticated (`claude --help` should
  work)
- **macOS recommended** -- Ralph uses `sandbox-exec` to restrict Claude's
  filesystem access; this feature is macOS-only

## Installation

Build from source:

```bash
cargo build --release
# Binary at ./target/release/ralph
```

Or install directly:

```bash
cargo install --path .
```

Pre-built binaries are also available on the [releases page][releases]. Install
the latest with:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Studio-Sasquatch/ralph/releases/latest/download/ralph-installer.sh | sh
```

Verify the installation:

```bash
ralph --help
```

## Initializing a Project

Navigate to your project directory and run:

```bash
cd your-project
ralph init
```

This creates:

- `.ralph.toml` -- project configuration file
- `.ralph/` -- working directory with subdirectories:
  - `features/` -- feature specifications and plans
  - `skills/` -- reusable agent skills
  - `prompts/` -- prompt files
  - `specs/` -- specification documents (legacy)
- `.ralph/progress.db` -- SQLite task database (gitignored)

The `.ralph.toml` file has three configuration sections:

```toml
[specs]
# dirs = [".ralph/specs"]

[execution]
# max_retries = 3    # Maximum retries for failed tasks
# verify = true      # Enable autonomous verification
# learn = true       # Enable skill creation + CLAUDE.md updates
```

All fields have sensible defaults. You can leave the file as-is to start.

## Working with Features

Features are the primary way to organize work in Ralph. They progress through a
lifecycle: `draft` -> `planned` -> `ready` -> `running` -> `done`/`failed`.

### 1. Write a Specification

```bash
ralph feature spec my-feature
```

This opens an interactive Claude session that interviews you about requirements,
constraints, and acceptance criteria. Claude writes the result to
`.ralph/features/my-feature/spec.md`. The feature is created in the database
with `draft` status.

Use `--model` to pick a specific model for the session:

```bash
ralph feature spec my-feature --model sonnet
```

### 2. Create an Implementation Plan

```bash
ralph feature plan my-feature
```

Claude reads the spec and collaborates with you to create an implementation plan
at `.ralph/features/my-feature/plan.md`. The plan breaks the spec into concrete
steps, identifies risks, and sequences the work. Feature status becomes
`planned`.

### 3. Decompose into Tasks

```bash
ralph feature build my-feature
```

Claude autonomously reads the spec and plan, then creates a DAG of tasks using
`ralph task add` and `ralph task deps add` CLI calls. It creates a root task
with child tasks and dependency edges. Feature status becomes `ready`.

### 4. Execute

```bash
ralph run my-feature
```

The agent loop picks one ready task at a time, spawns a Claude session to work
on it, handles completion and failure, runs verification, and continues until
all tasks are done or a limit is hit.

> [!WARNING]
> Ralph can (and possibly WILL) destroy anything you have access to, according
> to the whims of the LLM. Use `ralph run my-feature --once` to test before
> unleashing unattended loops.

### 5. Check Progress

```bash
ralph feature list                   # Overview of all features with task counts
ralph task list --feature my-feature # Tasks for a specific feature
ralph task tree t-abc123             # Visual tree of task hierarchy
ralph task show t-abc123             # Full task details
```

## Working with Standalone Tasks

For simpler work that does not need the full feature workflow, create standalone
tasks directly.

### Add a Task

```bash
ralph task add "Fix the login bug" -d "Users report 500 errors on /login"
```

Create subtasks with `--parent`:

```bash
ralph task add "Refactor auth module" --parent t-abc123
```

The `add` command prints the new task ID to stdout, making it scriptable:

```bash
ROOT=$(ralph task add "Parent task" -d "Top-level task")
ralph task add "Child task" --parent "$ROOT" -d "A subtask"
```

### Run a Single Task

```bash
ralph run t-abc123
```

This runs only that task through the agent loop.

### Interactive Task Creation

```bash
ralph task create
```

Claude interviews you to create a well-defined task with a clear title,
description, and acceptance criteria.

## Managing Tasks

List tasks:

```bash
ralph task list                        # Standalone tasks (default view)
ralph task list --all                  # All tasks
ralph task list --ready                # Only ready-to-execute tasks
ralph task list --status pending       # Filter by status
ralph task list --feature my-feature   # Tasks for a feature
ralph task list --json                 # Machine-readable output
```

Inspect a task:

```bash
ralph task show t-abc123               # Full details
ralph task show t-abc123 --json        # JSON output
ralph task tree t-abc123               # Indented tree with status colors
ralph task tree t-abc123 --json        # Tree as JSON
```

Update a task:

```bash
ralph task update t-abc123 --title "New title"
ralph task update t-abc123 -d "Updated description"
ralph task update t-abc123 --priority 1
```

Change task status manually:

```bash
ralph task done t-abc123               # Mark complete (triggers auto-transitions)
ralph task fail t-abc123 -r "reason"   # Mark failed (propagates to parent)
ralph task reset t-abc123              # Reset to pending
```

Log entries:

```bash
ralph task log t-abc123 -m "Investigated the bug, root cause is in auth.rs"
ralph task log t-abc123                # View all log entries
```

Delete a task:

```bash
ralph task delete t-abc123
```

> [!NOTE]
> Deletion is rejected if the task is a blocker for other tasks. Remove the
> dependency first with `ralph task deps rm`.

## Managing Dependencies

Add a dependency (A must complete before B can start):

```bash
ralph task deps add t-abc123 t-def456
```

Remove a dependency:

```bash
ralph task deps rm t-abc123 t-def456
```

List dependencies for a task:

```bash
ralph task deps list t-abc123
```

Ralph performs BFS cycle detection when adding dependencies. Circular
dependencies are rejected.

## Run Options

Control how `ralph run` behaves:

```bash
# Iteration control
ralph run my-feature --once              # Single iteration only
ralph run my-feature --limit=5           # Max 5 iterations (0 = unlimited)

# Model selection
ralph run my-feature --model=opus        # Use specific model (implies fixed strategy)
ralph run my-feature --model-strategy=cost-optimized   # Let Ralph pick models

# Sandbox control (macOS)
ralph run my-feature --no-sandbox        # Disable macOS sandbox
ralph run my-feature --allow=aws         # Grant sandbox write access to ~/.aws

# Verification and learning
ralph run my-feature --no-verify         # Skip verification agent
ralph run my-feature --no-learn          # Don't create skills or update CLAUDE.md

# Retries
ralph run my-feature --max-retries=5     # Override retry count (default: 3)
```

### Exit Codes

| Exit Code | Outcome      | Meaning                                    |
| :-------- | :----------- | :----------------------------------------- |
| 0         | Complete     | All tasks resolved                         |
| 0         | LimitReached | Iteration limit hit (not an error)         |
| 1         | Failure      | Critical failure                           |
| 2         | Blocked      | No ready tasks but incomplete tasks remain |
| 3         | NoPlan       | DAG is empty -- run `ralph feature build`  |

## Model Strategies

Ralph swaps between Claude models (`opus`, `sonnet`, `haiku`) across iterations
to balance cost and capability. Set the strategy with `--model-strategy`:

- **`cost-optimized`** (default) -- Starts with `sonnet`. Escalates to `opus`
  on error signals. Drops to `haiku` when tasks complete cleanly.
- **`fixed`** -- Always uses the `--model` value. Requires `--model` to be set.
- **`escalate`** -- Starts at `haiku`, escalates through `sonnet` to `opus` on
  failure signals. Never auto-de-escalates; only Claude can hint to step back.
- **`plan-then-execute`** -- Uses `opus` for the first iteration, `sonnet` for
  all subsequent iterations.

Claude can override any strategy for the next iteration by emitting a model
hint in its output. Hints apply to the next iteration only.

## Environment Variables

| Variable               | Description                       |
| :--------------------- | :-------------------------------- |
| `RALPH_LIMIT`          | Default iteration limit           |
| `RALPH_MODEL`          | Default model (opus/sonnet/haiku) |
| `RALPH_MODEL_STRATEGY` | Default model strategy            |
| `RALPH_ITERATION`      | Current iteration (for resume)    |
| `RALPH_TOTAL`          | Total iterations (for display)    |

## Project Files

```
.ralph.toml                  Project configuration
.ralph/progress.db           SQLite task database (gitignored)
.ralph/features/<name>/
  spec.md                    Feature specification
  plan.md                    Implementation plan
.ralph/skills/<name>/
  SKILL.md                   Reusable agent skill (YAML frontmatter + instructions)
.ralph/specs/                Specification documents (legacy)
```

## Next Steps

- Read the [architecture documentation][architecture] for a deeper
  understanding of how Ralph works internally
- Explore the `--model-strategy` options to optimize cost for your workloads
- Create reusable skills in `.ralph/skills/` to teach Ralph patterns specific
  to your project

[rustup]: https://rustup.rs
[releases]: https://github.com/Studio-Sasquatch/ralph/releases
[architecture]: ./architecture.md

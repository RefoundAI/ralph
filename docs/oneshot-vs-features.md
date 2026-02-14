# One-Shot Tasks vs Feature Workflow

Ralph supports two modes of operation for getting work done: **one-shot tasks**
for quick, well-scoped work, and the **feature workflow** for complex,
multi-step changes that benefit from upfront planning. This guide compares the
two approaches, explains the tradeoffs, and helps you decide which to use.

## Overview

| Mode             | Commands                                        | Best For                            |
| ---------------- | ----------------------------------------------- | ----------------------------------- |
| One-shot tasks   | `ralph task add` + `ralph run t-...`            | Quick fixes, small changes, scripts |
| Feature workflow | `ralph feature spec/plan/build` + `ralph run`   | Multi-file features, complex work   |

The most important practical difference is **context during execution**: feature
tasks receive the full specification and implementation plan in every Claude
session, while one-shot tasks only receive their title, description, and
immediate graph context.

## One-Shot Tasks

### Creating Tasks

Tasks can be created in two ways.

**Non-interactive (scriptable):**

```bash
ralph task add "Fix the login bug" -d "Users report 500 errors on POST /login"
ralph task add "Add input validation" --parent t-abc123  # subtask
ralph task add "Update tests" --priority 1               # higher priority (lower number)
```

`ralph task add` creates the task and prints the task ID to stdout. It is fully
scriptable -- you can capture the ID and use it in subsequent commands:

```bash
ROOT=$(ralph task add "Refactor auth module" -d "Break auth into smaller pieces")
ralph task add "Extract token validation" --parent "$ROOT"
ralph task add "Add refresh token support" --parent "$ROOT"
```

**Interactive (Claude-assisted):**

```bash
ralph task create
```

Claude interviews you to create a well-defined task with a clear title,
description, and acceptance criteria. This uses interactive mode with inherited
stdio -- you converse with Claude directly.

### Running a One-Shot Task

```bash
ralph run t-abc123
```

This enters the agent loop scoped to a single task. The loop:

1. Checks if the task is ready (status is `pending`, no unmet dependencies).
2. Claims the task (transitions to `in_progress`, sets `claimed_by`).
3. Spawns Claude with the task assignment context.
4. Claude implements the work and emits `<task-done>` or `<task-failed>`.
5. If verification is enabled, a read-only Claude session checks the work.
6. Done (or retry on verification failure, up to `max_retries`).

For a single leaf task with no children, this is typically one iteration.

### Task Hierarchies Without Features

You can build parent-child hierarchies and dependency graphs with standalone
tasks:

```bash
# Create parent
ROOT=$(ralph task add "Refactor auth module")

# Create children
EXTRACT=$(ralph task add "Extract token validation" --parent "$ROOT")
REFRESH=$(ralph task add "Add refresh token support" --parent "$ROOT")

# Add dependencies: extract must complete before refresh
ralph task deps add "$EXTRACT" "$REFRESH"
```

Then run the parent:

```bash
ralph run "$ROOT"
```

The agent loop picks ready children and executes them in dependency order. The
parent auto-completes when all children are done.

### What Claude Receives (One-Shot Context)

When working on a standalone task, Claude's system prompt includes:

- **Task assignment** -- ID, title, description
- **Parent context** -- Parent task's title and description (if the task has a
  parent)
- **Completed prerequisites** -- Titles and summaries of done blocker tasks
- **Retry information** -- Attempt number and previous failure reason (if
  retrying)
- **Available skills** -- Names and descriptions from `.ralph/skills/`

Claude does **not** receive a spec or plan. The task description is the only
source of requirements.

### When to Use One-Shot Tasks

- Quick bug fixes with clear scope
- Small, well-understood changes where you know exactly what needs to happen
- Tasks where a single sentence of description is sufficient
- Scripted or automated task creation (CI pipelines, external tools)
- Exploratory work where committing to a full plan would be premature

## Feature Workflow

### The Pipeline

The feature workflow is a four-phase pipeline:

```bash
ralph feature spec my-feature    # 1. Interactive: define requirements
ralph feature plan my-feature    # 2. Interactive: design implementation
ralph feature build my-feature   # 3. Autonomous: decompose into task DAG
ralph run my-feature             # 4. Autonomous: execute all tasks
```

Each phase produces a persistent artifact that feeds into the next.

### Phase 1: Specification

```bash
ralph feature spec my-feature
```

Claude interviews you about requirements, constraints, edge cases, and
acceptance criteria. The result is written to
`.ralph/features/my-feature/spec.md`. The feature is created in the database
with `draft` status.

### Phase 2: Planning

```bash
ralph feature plan my-feature
```

Claude reads the spec and collaborates with you to create an implementation
plan at `.ralph/features/my-feature/plan.md`. The plan breaks the spec into
concrete phases, identifies risks, and sequences the work. Feature status
becomes `planned`.

### Phase 3: Decomposition

```bash
ralph feature build my-feature
```

Claude autonomously reads both the spec and plan, then creates a DAG of tasks
using `ralph task add` and `ralph task deps add` CLI calls. It creates a root
task with child tasks and dependency edges. Feature status becomes `ready`.

### Phase 4: Execution

```bash
ralph run my-feature
```

The agent loop picks one ready task at a time from the feature's task DAG,
spawns a Claude session to work on it, handles completion and failure, runs
verification, and continues until all tasks are done or a limit is hit.

### What Claude Receives (Feature Context)

When working on a feature task, Claude's system prompt includes everything a
one-shot task gets, **plus**:

- **Feature specification** -- Full content of `spec.md`
- **Feature plan** -- Full content of `plan.md`

This is the most important practical difference. The spec and plan give Claude
the broader picture of the entire feature, even when the immediate task is
narrow. A Claude session implementing "Add input validation to the user
endpoint" still has access to the full specification's acceptance criteria and
the full plan's architectural decisions.

### When to Use the Feature Workflow

- Multi-file changes that need coordination across components
- Work that benefits from upfront requirements gathering
- Complex features where the implementation approach is not obvious
- Projects where you want documentation of what was built and why
- Work where consistent architectural decisions matter across tasks

## Key Differences

### Context During Execution

| Context Section         | One-Shot Tasks | Feature Tasks |
| ----------------------- | :------------: | :-----------: |
| Task ID, title, description | Yes        | Yes           |
| Parent context          | Yes            | Yes           |
| Completed prerequisites | Yes            | Yes           |
| Retry information       | Yes            | Yes           |
| Available skills        | Yes            | Yes           |
| Feature specification   | No             | Yes           |
| Feature plan            | No             | Yes           |

The spec and plan context helps Claude:

- Understand how the current task fits into the larger picture
- Make consistent decisions across tasks
- Follow the architectural approach defined in the plan
- Know the acceptance criteria for the overall feature

### Task Scoping

- **One-shot**: `ralph run t-abc123` runs only that task (or its children if it
  is a parent task).
- **Feature**: `ralph run my-feature` runs all tasks belonging to the feature,
  in dependency order.

### Database Fields

| Field        | Standalone Task     | Feature Task         |
| ------------ | ------------------- | -------------------- |
| `task_type`  | `"standalone"`      | `"feature"`          |
| `feature_id` | `NULL`              | Set to feature's ID  |

### Listing and Queries

```bash
ralph task list                        # Standalone tasks (default view)
ralph task list --feature my-feature   # Tasks for a specific feature
ralph task list --all                  # Everything
ralph feature list                     # Features with task counts
```

### RunTarget Resolution

When you run `ralph run <target>`:

- If the target starts with `t-`, it is treated as a task ID
  (`RunTarget::Task`).
- Otherwise, it is treated as a feature name (`RunTarget::Feature`).

## Hybrid Approach

You can mix both modes within the same project:

- Start with a feature workflow for the main body of work.
- Add standalone tasks for quick fixes or side work that does not belong to any
  feature.
- Manually add tasks to a feature after the build phase:

```bash
ralph task add "Extra migration step" \
  --feature f-abc123 \
  --parent t-root123 \
  -d "Handle edge case discovered during implementation"
```

## Tradeoffs

### One-Shot Advantages

- **Faster startup** -- No spec or plan overhead. Create a task and run it.
- **Scriptable** -- `ralph task add` prints the task ID to stdout, making it
  composable with shell scripts and other tools.
- **No ceremony** -- A title and description are all you need.
- **Good for small scope** -- When you already know exactly what to do, a spec
  and plan would be wasted effort.

### One-Shot Disadvantages

- **No persistent spec/plan context** -- Claude only sees the task description
  during execution. For multi-step work, each task must be self-contained.
- **Manual graph management** -- You build the parent-child hierarchy and
  dependency edges yourself.
- **No documentation trail** -- There is no spec or plan on disk explaining what
  was built or why.

### Feature Advantages

- **Rich context throughout execution** -- Every Claude session receives the
  full spec and plan, leading to more informed decisions.
- **Structured documentation** -- The spec and plan persist in
  `.ralph/features/<name>/` as a record of requirements and approach.
- **Automated decomposition** -- Claude breaks the plan into a properly ordered
  task DAG during the build phase.
- **Feature-level progress tracking** -- `ralph feature list` shows feature
  status with task counts.
- **Consistency across tasks** -- The shared spec and plan context helps Claude
  make consistent architectural decisions.

### Feature Disadvantages

- **Upfront time investment** -- The spec and plan phases are interactive and
  take real time.
- **More ceremony** -- For a one-line fix, the four-phase pipeline is overkill.
- **Feature names must be unique** -- You cannot reuse a feature name once it
  exists in the database.

## Decision Guide

Use this as a rough guideline:

| Situation                                          | Recommendation             |
| -------------------------------------------------- | -------------------------- |
| 1--2 tasks, clear scope                            | One-shot                   |
| 3+ tasks, needs planning                           | Feature workflow           |
| Quick fix you can describe in one sentence         | One-shot                   |
| "I need to think about how to approach this"       | Feature workflow           |
| Automated or scripted task creation                | One-shot with `task add`   |
| Want documentation for posterity                   | Feature workflow           |
| Exploratory work, unclear scope                    | One-shot (iterate quickly) |
| Multi-component change with ordering constraints   | Feature workflow           |

When in doubt, start with a one-shot task. If you find yourself writing a long
description to capture all the requirements, that is a signal to switch to the
feature workflow where the spec and plan phases can capture that complexity
properly.

## Quick Reference

### One-Shot: Minimal Example

```bash
ralph init
ralph task add "Fix null pointer in auth handler" \
  -d "Handle the case where token is None in src/auth.rs:47"
# Prints: t-a1b2c3
ralph run t-a1b2c3 --once
```

### Feature: Minimal Example

```bash
ralph init
ralph feature spec user-auth        # Interactive: define requirements
ralph feature plan user-auth        # Interactive: design implementation
ralph feature build user-auth       # Autonomous: create task DAG
ralph run user-auth                 # Autonomous: execute all tasks
```

### One-Shot: Scripted Multi-Task Example

```bash
ROOT=$(ralph task add "Migrate database schema")
T1=$(ralph task add "Add new columns" --parent "$ROOT" -d "ALTER TABLE users ADD ...")
T2=$(ralph task add "Backfill data" --parent "$ROOT" -d "UPDATE users SET ...")
T3=$(ralph task add "Update queries" --parent "$ROOT" -d "Change SELECT statements ...")

ralph task deps add "$T1" "$T2"   # new columns before backfill
ralph task deps add "$T2" "$T3"   # backfill before query changes

ralph run "$ROOT"
```

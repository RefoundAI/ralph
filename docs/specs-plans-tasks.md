# Specifications, Plans, and Tasks

Ralph's feature workflow is a three-phase pipeline that transforms an idea into
autonomous execution. Each phase produces a concrete artifact that feeds into the
next, and those artifacts persist as context throughout the entire execution
lifecycle.

1. **Specification** -- Define what to build (interactive, human-guided)
2. **Planning** -- Design how to build it (interactive, human-guided)
3. **Decomposition** -- Break the plan into executable tasks (autonomous)

The feature progresses through statuses as it moves through the pipeline:
`draft` --> `planned` --> `ready` --> `running` --> `done` / `failed`.

## The Feature Entity

Features are stored in the `features` table in SQLite (added in schema v2). Each
feature tracks its identity, its artifacts, and its lifecycle status.

| Column       | Type   | Description                                          |
| ------------ | ------ | ---------------------------------------------------- |
| `id`         | `TEXT`  | Primary key, format `f-{6 hex}` (SHA-256 derived)   |
| `name`       | `TEXT`  | Unique human-readable name, used as directory name   |
| `spec_path`  | `TEXT`  | Absolute path to `spec.md` (set after spec phase)    |
| `plan_path`  | `TEXT`  | Absolute path to `plan.md` (set after plan phase)    |
| `status`     | `TEXT`  | One of: `draft`, `planned`, `ready`, `running`, `done`, `failed` |
| `created_at` | `TEXT`  | RFC 3339 timestamp                                   |
| `updated_at` | `TEXT`  | RFC 3339 timestamp                                   |

Feature CRUD operations live in `src/feature.rs`:

- `create_feature()` -- Insert a new feature with `draft` status
- `get_feature()` / `get_feature_by_id()` -- Look up by name or ID
- `list_features()` -- Return all features ordered by creation time
- `update_feature_status()` / `update_feature_spec_path()` /
  `update_feature_plan_path()` -- Modify individual fields
- `ensure_feature_dirs()` -- Create `.ralph/features/<name>/` on disk
- `read_spec()` / `read_plan()` -- Read the markdown files from disk
- `feature_exists()` -- Check if a name is taken

## Phase 1: Specification

**Command:** `ralph feature spec <name>`

The spec phase captures _what_ to build through an interactive interview between
the user and Claude.

### Step-by-Step Flow

1. **Create or retrieve the feature.** If the name does not exist in the
   database, `create_feature()` inserts a new row with `draft` status. If it
   already exists, the existing record is retrieved.

2. **Create the directory.** `ensure_feature_dirs()` creates
   `.ralph/features/<name>/` if it does not exist.

3. **Gather project context.** `gather_project_context()` assembles a markdown
   block containing:
   - CLAUDE.md content (truncated to 10,000 characters)
   - `.ralph.toml` configuration
   - A table of existing features (name, status, has spec, has plan)

4. **Check for an existing spec (resume support).** If `spec.md` already exists
   on disk, its content is appended to the context under an "Existing Spec
   (Resume)" heading. Claude is told to resume rather than start fresh. This
   allows incremental refinement across multiple sessions.

5. **Build the system prompt.** `build_feature_spec_system_prompt()` constructs
   a prompt that instructs Claude to:
   - Interview the user about requirements, constraints, edge cases
   - Ask about functional and non-functional requirements
   - Ask about testing and acceptance criteria
   - Write the result to `spec.md` at the feature's path

6. **Launch an interactive Claude session.** `run_interactive()` spawns the
   `claude` CLI with inherited stdio -- the user types responses, Claude
   responds, and the conversation continues until Claude writes the spec.

7. **Update the database.** After the session ends, Ralph sets `spec_path` on
   the feature record.

### What the Spec Contains

The spec is a markdown document produced by Claude based on the interview. The
system prompt requests these sections:

1. **Overview** -- What the feature does
2. **Requirements** -- Functional and non-functional
3. **Architecture** -- Components and data flow
4. **API / Interface** -- Function signatures and contracts
5. **Data Models** -- Types, schemas, validation
6. **Testing** -- Test cases and acceptance criteria
7. **Dependencies** -- Libraries and services

The exact content depends on the user's answers during the interview.

## Phase 2: Planning

**Command:** `ralph feature plan <name>`

The plan phase designs _how_ to build what the spec defines, again through an
interactive session.

### Prerequisites

- The feature must exist in the database.
- `spec_path` must be set (the spec is a required input to planning). If the
  feature has no spec, Ralph exits with an error directing the user to run
  `ralph feature spec <name>` first.

### Step-by-Step Flow

1. **Retrieve the feature and validate.** Ralph looks up the feature by name and
   checks that `spec_path` is set.

2. **Read the spec.** `read_spec()` loads the full spec content from disk.

3. **Gather project context.** Same as the spec phase -- CLAUDE.md,
   `.ralph.toml`, features list.

4. **Check for an existing plan (resume support).** If `plan.md` already exists,
   it is appended to the context for resumption, just like the spec phase.

5. **Build the system prompt.** `build_feature_plan_system_prompt()` constructs
   a prompt that:
   - Includes the full spec content
   - Instructs Claude to work with the user on a detailed implementation plan
   - Asks Claude to consider implementation order and dependencies
   - Directs the output to `plan.md`

6. **Launch an interactive Claude session.** Same mechanism as the spec phase.

7. **Update the database.** Ralph sets `plan_path` and transitions the feature
   status from `draft` to `planned`.

### What the Plan Contains

The plan bridges the spec to implementation. The system prompt requests:

1. **Implementation phases** -- Ordered list of work
2. **Per-phase details** -- What to implement and test
3. **Verification criteria** -- How to know each phase is done
4. **Risk areas** -- Things that might go wrong

## Phase 3: Decomposition

**Command:** `ralph feature build <name>`

The build phase converts the plan into a machine-executable task DAG. Unlike the
previous two phases, this runs autonomously -- Claude creates tasks without user
input.

### Prerequisites

- The feature must exist in the database.
- Both `spec_path` and `plan_path` must be set. Ralph checks both and exits with
  a specific error if either is missing.

### Step-by-Step Flow

1. **Retrieve the feature and read artifacts.** Ralph loads the feature record,
   then reads both `spec.md` and `plan.md` from disk.

2. **Create the root task.** `create_task_with_feature()` inserts a root task
   into the `tasks` table:
   - Title: `"Feature: <name>"`
   - `task_type`: `"feature"`
   - `feature_id`: the feature's ID
   - `parent_id`: `NULL` (it is the root)
   - This root task serves as the structural parent for all subtasks. It never
     executes directly -- it auto-completes when all its children complete.

3. **Build the system prompt.** `build_feature_build_system_prompt()` constructs
   a comprehensive prompt that includes:
   - The full spec content
   - The full plan content
   - The root task ID and feature ID
   - Instructions for using `ralph task add` and `ralph task deps add` via the
     Bash tool
   - Decomposition rules (right-size tasks, include acceptance criteria, use
     dependencies for ordering)

4. **Launch Claude in streaming mode.** `run_streaming()` spawns Claude with
   `--print --verbose --output-format stream-json --dangerously-skip-permissions`.
   Claude runs autonomously, reading the plan and creating tasks by calling
   Ralph's own CLI.

5. **Claude creates the task DAG.** Claude executes Bash commands like:

   ```bash
   ID=$(ralph task add "Implement user model" \
     -d "Create User struct with validation..." \
     --parent t-abc123 \
     --feature f-def456)
   ```

   And then adds dependency edges:

   ```bash
   ralph task deps add $SCHEMA_TASK_ID $HANDLER_TASK_ID
   ```

6. **Print a summary.** After Claude finishes, Ralph reads the task tree from
   the database using `get_task_tree()` and prints it as an indented tree with
   status colors.

7. **Update feature status.** Ralph transitions the feature status to `ready`.

### Why CLI Commands Instead of JSON

Claude creates tasks by calling `ralph task add` through its Bash tool rather
than outputting structured JSON for Ralph to parse. This design has several
advantages:

- Task creation goes through the same validation as manual `ralph task add`
  (parent existence checks, ID generation, feature_id association).
- Dependency edges go through BFS cycle detection in `add_dependency()`.
- Claude can inspect the output of its own commands and reference earlier task
  IDs in later dependency edges.
- The DAG is built incrementally and can be inspected at any point during
  creation.

### Decomposition Rules

The system prompt instructs Claude to follow these rules:

1. **Right-size tasks** -- One coherent unit of work per task, typically touching
   1-3 files.
2. **Reference spec/plan sections** -- Each task description must cite which
   section it implements.
3. **Include acceptance criteria** -- Each task must describe how to verify
   completion.
4. **Parent tasks for grouping** -- Parents organize related children; they never
   execute directly.
5. **Dependencies for ordering** -- Only add edges when task B genuinely needs
   artifacts from task A.
6. **Foundation first** -- Schemas and types before the code that uses them.

## Phase 4: Execution

**Command:** `ralph run <feature-name>`

Once a feature has status `ready`, the agent loop picks up its tasks and
executes them autonomously.

### How Specs and Plans Feed into Execution

Specs and plans do not stop being useful after task creation. They persist as
context in every iteration of the agent loop:

1. `resolve_feature_context()` in `run_loop.rs` loads the feature record and
   reads both `spec.md` and `plan.md` from disk.
2. `build_iteration_context()` packages the spec and plan content into an
   `IterationContext` struct.
3. The system prompt builder in `client.rs` appends them as "Feature
   Specification" and "Feature Plan" sections.
4. Every Claude session during `ralph run` receives the full spec and plan,
   giving it the broader picture even when working on a single narrow task.

### Task Selection

The agent loop picks one ready task per iteration. A task is ready when all four
conditions hold:

1. **Status is `pending`** -- Not yet started, not blocked, not done.
2. **It is a leaf node** -- No children in the `tasks` table (parent tasks
   auto-complete; they are never assigned to Claude).
3. **Parent is not `failed`** -- If the parent failed, children are not
   eligible.
4. **All blockers are `done`** -- Every task in the `dependencies` table that
   blocks this task must have status `done`.

Ready tasks are ordered by `priority ASC, created_at ASC` -- lower priority
numbers run first, and ties are broken by creation time.

### Iteration Context

Each Claude session receives a rich context assembled by
`build_iteration_context()`:

| Context Section          | Source                                              |
| ------------------------ | --------------------------------------------------- |
| Task assignment          | Task ID, title, description                         |
| Parent context           | Parent task's title and description (if any)         |
| Completed prerequisites  | Titles and summaries of done blocker tasks           |
| Feature specification    | Full `spec.md` content                              |
| Feature plan             | Full `plan.md` content                              |
| Retry information        | Attempt number, max retries, previous failure reason |
| Available skills         | Names and descriptions from `.ralph/skills/`         |
| Learning instructions    | Skill creation and CLAUDE.md update guidance         |

### Task Completion and Auto-Transitions

When Claude finishes working on a task, it emits a sigil:

- `<task-done>{task_id}</task-done>` -- Task completed successfully
- `<task-failed>{task_id}</task-failed>` -- Task cannot be completed

Ralph processes these sigils and triggers auto-transitions in the DAG:

**On completion (`done`):**

1. Any tasks that were `blocked` and had this task as their only remaining
   blocker transition to `pending` (they become ready).
2. If all siblings under the same parent are now `done`, the parent
   auto-completes. This cascades upward -- if the parent's completion causes
   all of _its_ siblings to be done, the grandparent auto-completes too.

**On failure (`failed`):**

1. The parent task is automatically marked `failed`. This cascades upward
   through the hierarchy.
2. Children of a failed parent are excluded from the ready query, preventing
   further work on a failed branch.

### Verification

When verification is enabled (the default), a separate read-only Claude session
checks each completed task before marking it done:

1. Ralph spawns Claude with restricted tools (`Bash Read Glob Grep` -- no write
   tools).
2. The verification prompt includes the task details, spec, and plan.
3. Claude inspects the implementation and runs tests.
4. Claude emits `<verify-pass/>` or `<verify-fail>reason</verify-fail>`.
5. On pass: the task is marked `done` and auto-transitions fire.
6. On fail: if retries remain, the task transitions back to `pending` with
   `retry_count` incremented and the failure reason logged. On the next
   iteration, Claude receives the retry information (attempt number and
   previous failure reason). If max retries are exhausted, the task is marked
   `failed`.

### Feature Completion

When all leaf tasks under the root complete, the root task auto-completes via
the cascading parent completion mechanism. The feature status can then be
updated to `done`.

## How the Artifacts Connect

```
ralph feature spec <name>
  |
  |  User interviews with Claude
  |  Claude writes spec.md
  v
spec.md  (what to build)
  |
  |  Fed into plan system prompt
  v
ralph feature plan <name>
  |
  |  User interviews with Claude
  |  Claude writes plan.md
  v
plan.md  (how to build it)
  |
  |  Both spec + plan fed into build prompt
  v
ralph feature build <name>
  |
  |  Claude autonomously creates tasks via CLI
  v
Task DAG  (in SQLite)
  |
  |  Tasks assigned one at a time
  v
ralph run <name>
  |
  |  Each iteration receives spec + plan + task context
  v
Implemented code
```

The key insight is that spec and plan are not consumed and discarded. They flow
through the entire pipeline:

- The **plan** reads the **spec** to know what to design.
- The **build** reads both to decompose work into tasks.
- Every **execution iteration** reads both to give Claude the full picture of
  what is being built and how, even when the immediate task is narrow.

This means a Claude session implementing "Add input validation to the user
endpoint" still has access to the full specification's acceptance criteria and
the full plan's architectural decisions.

## Standalone Tasks vs Feature Tasks

Ralph supports two categories of tasks that differ in how they are created and
scoped.

### Feature Tasks

Feature tasks are created during `ralph feature build` (or manually with
`ralph task add --feature <id>`). They share these characteristics:

- `task_type` is `"feature"` and `feature_id` is set.
- They are organized under a root task in a parent-child hierarchy.
- They are executed with `ralph run <feature-name>`, which scopes task selection
  to only tasks belonging to that feature via `get_ready_tasks_for_feature()`.
- The spec and plan are loaded and passed to each iteration.

### Standalone Tasks

Standalone tasks are created with `ralph task add` (no `--feature` flag) or
through the interactive `ralph task create` command.

- `task_type` is `"standalone"` and `feature_id` is `NULL`.
- They can still have parent-child and dependency relationships with other
  standalone tasks.
- They are executed with `ralph run <task-id>` (the ID starts with `t-`).
- No spec or plan context is loaded (there is no associated feature).
- They are queried with `get_standalone_tasks()`.

### When to Use Which

| Scenario                                  | Use                |
| ----------------------------------------- | ------------------ |
| Multi-step work with structured planning  | Feature workflow   |
| Quick fixes or one-off changes            | Standalone tasks   |
| Work that needs a spec and plan           | Feature workflow   |
| Simple tasks that do not need planning    | Standalone tasks   |

## File Layout

```
.ralph/
  features/
    my-feature/
      spec.md          # Written during "ralph feature spec"
      plan.md          # Written during "ralph feature plan"
    another-feature/
      spec.md
      plan.md
  skills/              # Reusable agent skills (optional)
    committing:git/
      SKILL.md
  progress.db          # SQLite database (features + tasks tables)

.ralph.toml            # Project configuration
```

## Status Lifecycle

### Feature Status

```
draft -----> planned -----> ready -----> running -----> done
                                    |               |
                                    +--------> failed
```

| Status    | Meaning                                              |
| --------- | ---------------------------------------------------- |
| `draft`   | Feature created; spec may or may not exist yet        |
| `planned` | Both spec and plan exist; set after `feature plan`    |
| `ready`   | Task DAG created; set after `feature build`           |
| `running` | Agent loop is executing tasks (set by `ralph run`)    |
| `done`    | All tasks completed successfully                     |
| `failed`  | One or more tasks failed and could not be recovered   |

### Task Status

```
pending -----> in_progress -----> done
    ^              |
    |              v
    +---------- failed
    ^
    |
 blocked ------+
```

| Status        | Meaning                                             |
| ------------- | --------------------------------------------------- |
| `pending`     | Waiting to be picked up; all blockers done           |
| `in_progress` | Claimed by an agent; Claude is working on it         |
| `done`        | Completed (possibly verified); triggers auto-transitions |
| `failed`      | Could not be completed; logged with reason            |
| `blocked`     | Waiting for a blocker task to complete                |

Valid transitions are enforced by `set_task_status()` in `transitions.rs`. The
force-transition functions (`force_complete_task`, `force_fail_task`,
`force_reset_task`) step through valid intermediate states to reach the target
status.

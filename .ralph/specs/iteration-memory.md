# Iteration Memory

Ralph currently treats each iteration as stateless: a fresh Claude Code session receives a task assignment but has no memory of what happened in previous iterations. This design is simple but leads to repeated failures, wasted compute on known-bad approaches, and inability to learn from experience within a run.

This spec designs three memory systems that give Ralph cross-iteration awareness while maintaining the existing synchronous, single-agent, SQLite-backed architecture.

**Three systems:**

1. **Error Recovery Memory** -- Structured failure reports that prevent repeating the same mistakes when retrying tasks
2. **Self-Improvement / Learning Extraction** -- Reusable insights captured from successful and failed iterations, injected as context into future tasks
3. **Strategic Intelligence** -- Data-driven model selection, difficulty estimation, and stuck-loop detection

## Architecture Overview

### Error Recovery Memory

**Purpose:** When a task fails (or completes without a sigil, indicating an incomplete attempt), capture a structured failure report so the next attempt has full context about what went wrong and what was already tried.

**Hook point — capture (after iteration):** In `run_loop::run()`, after the `// Handle task completion/failure sigils` block (lines 107–143), when a task-failed sigil is received or no sigil is emitted, invoke a new `memory::capture_failure()` function. This receives the `task_id`, the `ResultEvent` (which contains the result text, duration, cost), and the log file path. The function parses any `<failure-report>` sigil from the result text (via a new parser in `events.rs`) and stores the structured report in the `failure_reports` table. If no `<failure-report>` sigil is present, a minimal report is generated from the result text (truncated) and the outcome status.

**Hook point — injection (before iteration):** In `claude::client::build_task_context()`, after rendering the "Completed Prerequisites" section, query the `failure_reports` table for any previous attempts at this `task_id`. If found, append a `### Previous Attempts` section showing each attempt's error summary, approach taken, and what to avoid. This is the primary consumer of error recovery memory.

**Data flow:**

```
 Claude output (ResultEvent)
       │
       ▼
 parse <failure-report> sigil  ──→  failure_reports table
       │                                    │
       ▼                                    ▼
 task status updated            build_task_context() queries
 (fail_task / release_claim)    previous attempts for this task_id
                                           │
                                           ▼
                                 "### Previous Attempts" section
                                 injected into next iteration's
                                 system prompt
```

**Retry awareness:** The run loop currently calls `release_claim()` for no-sigil iterations, which transitions the task back to `pending`. This already enables retries. Error recovery memory enhances this by ensuring the retry has context about what failed, rather than starting blind.

### Self-Improvement / Learning Extraction

**Purpose:** Capture reusable insights — both from successful iterations ("this approach worked") and failures ("this dependency is tricky") — and inject relevant learnings into future task contexts to improve success rates over time.

**Hook point — capture (after iteration):** In `run_loop::run()`, after processing task sigils, invoke `memory::capture_learnings()`. This parses any `<learning>` sigils from `ResultEvent.result` text. Each learning has a category tag, a brief description, and optional relevance tags (e.g., file paths, error types, tool names). Learnings are stored in the `learnings` table, keyed by the originating `task_id` and tagged for relevance matching.

**Hook point — injection (before iteration):** In `claude::client::build_task_context()`, query the `learnings` table for entries whose relevance tags overlap with the current task's description, title, or parent context. Inject a `### Learnings from Previous Iterations` section containing the top-N most relevant learnings, ranked by recency and relevance score. A context budget (character limit) caps the total injection size.

**Relevance matching:** Learnings are tagged at capture time with keywords extracted from the task context (file paths mentioned, error types, tool names). At injection time, the current task's description and title are compared against these tags using simple substring/keyword matching (no ML needed). The matching is done in SQL with LIKE queries and scored by tag overlap count.

**Data flow:**

```
 Claude output (ResultEvent)
       │
       ▼
 parse <learning> sigils  ──→  learnings table
 (category, description,       (with relevance tags, task_id,
  relevance tags)                timestamp)
                                       │
                                       ▼
                              build_task_context() queries
                              learnings matching current task
                                       │
                                       ▼
                              "### Learnings" section injected
                              into system prompt (budget-capped)
```

### Strategic Intelligence

**Purpose:** Replace the current heuristic-based model selection in `strategy.rs` (which reads the progress.db file as raw text and searches for keywords like "error", "stuck", etc.) with data-driven metrics. Also detect stuck loops and provide difficulty estimates.

**Hook point — metrics capture (after iteration):** In `run_loop::run()`, after processing sigils and before the `all_resolved()` check, invoke `memory::record_iteration_metrics()`. This records: task_id, iteration number, model used, duration_ms, cost_usd, outcome (done/failed/no-sigil), and any `<difficulty-estimate>` sigil value. Stored in `strategy_metrics` table.

**Hook point — model selection (strategy replacement):** Modify `strategy::select_cost_optimized()` and `strategy::select_escalate()` to query `strategy_metrics` instead of reading the progress.db file as text. The new functions:
- Count recent consecutive failures for the same task → escalate model
- Check overall success rate across recent N iterations → choose model tier
- Detect stuck loops: if the same task has been attempted 3+ times with failures, signal a stuck condition

**Hook point — stuck-loop detection:** In `run_loop::run()`, before claiming a task, query `strategy_metrics` for the number of previous failed attempts on this task. If the count exceeds a threshold (configurable, default 3), inject a `### Stuck Loop Warning` section into the task context suggesting alternative approaches or task decomposition.

**Hook point — context injection:** In `claude::client::build_task_context()`, append a `### Loop Status` section showing: current iteration, total attempts on this task, recent success/failure rates, and the model selection rationale. This gives Claude situational awareness.

**Data flow:**

```
 Iteration outcome
 (task_id, model, duration,
  cost, outcome)
       │
       ▼
 strategy_metrics table  ──→  select_model() queries metrics
       │                       instead of reading raw text
       │
       ├──→ stuck-loop detection (attempt count per task)
       │          │
       │          ▼
       │    "### Stuck Loop Warning" in task context
       │
       └──→ "### Loop Status" section in task context
```

### Integration Points

The three memory systems are designed to be independent at the storage layer but interact through shared hooks in the run loop and context builder:

```
                         ┌─────────────────────────────┐
                         │       run_loop::run()        │
                         │                              │
                         │  ┌─── claim_task() ────────┐ │
                         │  │                          │ │
                         │  │  build_task_context()    │ │
                         │  │    ├─ Error Recovery:    │ │
                         │  │    │  previous attempts  │ │
                         │  │    ├─ Learnings:         │ │
                         │  │    │  relevant insights  │ │
                         │  │    └─ Strategy:          │ │
                         │  │       loop status        │ │
                         │  │                          │ │
                         │  │  claude::client::run()   │ │
                         │  │         │                │ │
                         │  │         ▼                │ │
                         │  │    ResultEvent           │ │
                         │  │         │                │ │
                         │  └─────────┼────────────────┘ │
                         │            │                   │
                         │     ┌──────┴──────┐           │
                         │     ▼      ▼      ▼           │
                         │  capture  capture  record     │
                         │  failure  learning metrics    │
                         │     │      │       │          │
                         │     ▼      ▼       ▼          │
                         │  ┌─────────────────────┐      │
                         │  │   progress.db        │      │
                         │  │   (new tables)       │      │
                         │  └─────────────────────┘      │
                         │            │                   │
                         │            ▼                   │
                         │     select_model()             │
                         │     (reads strategy_metrics)   │
                         └─────────────────────────────────┘
```

**Cross-system interactions:**

1. **Error patterns → Strategy:** When `strategy_metrics` shows repeated failures on a task, `select_model()` escalates to a more capable model. The failure count comes from `strategy_metrics`, not from `failure_reports`, keeping the systems decoupled.

2. **Learnings → Error recovery context:** When retrying a failed task, the `build_task_context()` function injects both the previous attempt details (from `failure_reports`) and any relevant learnings (from `learnings`). These are separate sections but complement each other: failure reports say "what went wrong", learnings say "what works in similar situations."

3. **Strategy → Context injection:** The `### Loop Status` section (from strategic intelligence) gives Claude meta-awareness of how many iterations have passed and how the run is going, which influences how aggressively it should approach the current task. This is independent of the error recovery and learning sections.

4. **Difficulty estimates → Model selection:** Claude's `<difficulty-estimate>` sigil (captured in `strategy_metrics`) informs future model selection for similar tasks. A task rated "hard" that was completed by sonnet suggests sonnet is sufficient; a task rated "hard" that failed might trigger opus escalation.

### Module Layout

**New modules:**

```
src/
├── memory/                    # NEW: Cross-iteration memory system
│   ├── mod.rs                 # Public API: capture_failure(), capture_learnings(),
│   │                          #   record_iteration_metrics(), get_failure_context(),
│   │                          #   get_relevant_learnings(), get_loop_status()
│   ├── errors.rs              # Failure report capture, storage, and retrieval
│   ├── learnings.rs           # Learning extraction, storage, relevance matching
│   ├── metrics.rs             # Iteration metrics recording and querying
│   └── context.rs             # Context rendering: builds the memory sections
│                              #   for injection into build_task_context()
```

**Modified existing modules:**

| Module | Change |
|---|---|
| `src/dag/db.rs` | Bump `SCHEMA_VERSION` to 2. Add `failure_reports`, `learnings`, `strategy_metrics` tables in `migrate()` (v1→v2 migration). |
| `src/claude/events.rs` | Add `parse_failure_report()`, `parse_learning()`, `parse_difficulty_estimate()` sigil parsers. Add corresponding fields to `ResultEvent`. |
| `src/claude/parser.rs` | Wire new sigil parsers into `ResultEvent` construction (in the Result event branch). |
| `src/claude/client.rs` | Modify `build_task_context()` to accept a `MemoryContext` struct and render memory sections (previous attempts, learnings, loop status). Modify `build_system_prompt()` to document new sigils. |
| `src/run_loop.rs` | After sigil processing, call `memory::capture_failure()`, `memory::capture_learnings()`, `memory::record_iteration_metrics()`. Before building task context, call `memory::get_failure_context()`, `memory::get_relevant_learnings()`, `memory::get_loop_status()`. |
| `src/strategy.rs` | Replace `analyze_progress()` file-reading heuristic with `strategy_metrics` table queries. Replace `assess_escalation_need()` similarly. |
| `src/config.rs` | No changes needed — existing `Config` struct is sufficient. |
| `src/dag/mod.rs` | Re-export memory-related DB initialization if needed (or keep memory module self-contained with its own DB access). |

**Design decision — memory module DB access:** The `memory` module receives a `&Db` reference from the run loop (same database handle used for tasks). All memory tables live in the same `progress.db` file, maintaining the single-database architecture. The memory module does not own the connection; it borrows it from the run loop, just as `dag::claim_task()` and `dag::complete_task()` do today.

## Data Model

All new tables share the same `progress.db` database file alongside the existing `tasks`, `dependencies`, and `task_logs` tables. They will be created in a v1→v2 schema migration in `src/dag/db.rs`.

### Error Recovery Memory

#### `iteration_outcomes` table

Tracks the outcome of each iteration attempt. One row per iteration, keyed by task_id + attempt_number. This provides a timeline of what happened across retries.

```sql
CREATE TABLE iteration_outcomes (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL,
    model TEXT NOT NULL,                    -- 'opus', 'sonnet', 'haiku'
    started_at TEXT NOT NULL,               -- ISO 8601 timestamp
    duration_ms INTEGER NOT NULL,           -- Wall-clock time for iteration
    tokens_input INTEGER,                   -- Input tokens (from ResultEvent cost data)
    tokens_output INTEGER,                  -- Output tokens
    outcome TEXT NOT NULL                   -- 'done', 'failed', 'no_sigil', 'error'
        CHECK (outcome IN ('done','failed','no_sigil','error')),
    error_type TEXT,                        -- Optional classification: 'timeout', 'tool_error', 'assertion_failure', 'unknown'
    PRIMARY KEY (task_id, attempt_number)
);

CREATE INDEX idx_iteration_outcomes_task_id ON iteration_outcomes(task_id);
CREATE INDEX idx_iteration_outcomes_started_at ON iteration_outcomes(started_at);
CREATE INDEX idx_iteration_outcomes_model_outcome ON iteration_outcomes(model, outcome);
```

**Column rationale:**

- `task_id`, `attempt_number`: Composite key. `attempt_number` increments from 1 on first attempt. Allows tracking retry history.
- `model`: Which model was used. Enables analysis of model performance across tasks.
- `started_at`, `duration_ms`: Timing data. `started_at` provides chronological ordering across tasks; `duration_ms` enables timeout/performance analysis.
- `tokens_input`, `tokens_output`: Cost tracking. Nullable because early iterations might not report token counts.
- `outcome`: Enum of result types. 'no_sigil' = Claude finished without emitting task-done or task-failed; 'error' = runtime error (timeout, crash).
- `error_type`: Optional classification for failures. Helps categorize recurring error patterns.

**Indexing strategy:**

- `task_id` index: Fast lookup of all attempts for a task (used when injecting previous-attempt context).
- `started_at` index: Chronological queries for "recent N iterations" (used in loop status and stuck detection).
- `model,outcome` composite index: Performance analysis queries like "success rate of sonnet in last 20 iterations."

#### `failure_reports` table

Structured failure details captured from `<failure-report>` sigils or auto-generated from result text. One row per failed attempt. Provides rich context for retries.

```sql
CREATE TABLE failure_reports (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL,
    what_was_tried TEXT NOT NULL,           -- Brief description of approach taken
    why_it_failed TEXT NOT NULL,            -- Root cause analysis
    error_category TEXT,                    -- 'test_failure', 'build_error', 'missing_file', 'logic_error', etc.
    relevant_files TEXT,                    -- JSON array of file paths mentioned in error
    stack_trace_snippet TEXT,               -- First N lines of stack trace or error output
    created_at TEXT NOT NULL,               -- ISO 8601 timestamp
    PRIMARY KEY (task_id, attempt_number),
    FOREIGN KEY (task_id, attempt_number)
        REFERENCES iteration_outcomes(task_id, attempt_number)
        ON DELETE CASCADE
);

CREATE INDEX idx_failure_reports_task_id ON failure_reports(task_id);
CREATE INDEX idx_failure_reports_error_category ON failure_reports(error_category);
```

**Column rationale:**

- `task_id`, `attempt_number`: Composite key matching `iteration_outcomes`. One failure report per failed attempt.
- `what_was_tried`: Claude's own description of the approach (from sigil), or extracted from result text ("Attempted to fix by...").
- `why_it_failed`: Root cause. Human/Claude-readable explanation.
- `error_category`: Machine-readable error type. Used for relevance matching and pattern detection.
- `relevant_files`: JSON array of strings like `["src/dag/tasks.rs", "src/run_loop.rs"]`. Enables file-based relevance matching for learnings.
- `stack_trace_snippet`: Truncated stack trace (first 500 chars). Useful for diagnosing repeated errors.
- `created_at`: When the failure was captured. Enables "freshest failure first" ordering.

**Indexing strategy:**

- `task_id` index: Retrieve all failure history for a task when building retry context.
- `error_category` index: Find failures of the same type across tasks (e.g., "show me all test_failure errors").

### Self-Improvement / Learning Extraction

#### `learnings` table

Reusable insights captured from `<learning>` sigils. Tagged for relevance matching. Can be pruned/merged over time.

```sql
CREATE TABLE learnings (
    id TEXT PRIMARY KEY,                    -- 'l-{6 hex}' format, same as task IDs
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,  -- Origin task (nullable for cross-task learnings)
    category TEXT NOT NULL,                 -- 'success_pattern', 'pitfall', 'tool_usage', 'code_structure', 'testing_strategy', etc.
    content TEXT NOT NULL,                  -- The learning itself (1-3 sentences)
    relevance_tags TEXT NOT NULL,           -- JSON array of strings: file paths, keywords, error types
    created_at TEXT NOT NULL,               -- ISO 8601 timestamp
    pruned_at TEXT,                         -- NULL if active, timestamp if pruned
    superseded_by TEXT REFERENCES learnings(id) ON DELETE SET NULL  -- Points to newer learning that replaces this one
);

CREATE INDEX idx_learnings_task_id ON learnings(task_id);
CREATE INDEX idx_learnings_created_at ON learnings(created_at);
CREATE INDEX idx_learnings_pruned_at ON learnings(pruned_at);
CREATE INDEX idx_learnings_category ON learnings(category);
```

**Column rationale:**

- `id`: Unique identifier for the learning. Uses same ID generation as tasks (`l-{6 hex}`).
- `task_id`: Which task produced this learning. Nullable because learnings can be manually added or merged from multiple tasks. ON DELETE SET NULL preserves the learning even if the task is deleted.
- `category`: Coarse-grained classification for learnings. Helps with organization and filtering.
- `content`: The actual insight, stored as plain text. Should be concise and actionable.
- `relevance_tags`: JSON array like `["src/dag/tasks.rs", "Rust borrow checker", "test_failure"]`. Used for matching against current task context.
- `created_at`: When the learning was captured. Newer learnings prioritized when budget-limited.
- `pruned_at`: NULL if learning is active. Non-NULL timestamp if it's been archived/removed from active use. Allows soft deletion.
- `superseded_by`: Points to a newer learning that replaces/generalizes this one. Enables chaining/evolution of learnings over time.

**Indexing strategy:**

- `task_id` index: Find all learnings from a specific task.
- `created_at` index: "Newest first" ordering for recency ranking.
- `pruned_at` index: Fast filtering of active learnings (`WHERE pruned_at IS NULL`).
- `category` index: Filter by learning type.

**Relevance tags design:**

Tags are stored as a JSON array for simplicity (no separate join table needed). Matching logic:

1. Extract current task's file paths (from description), keywords (from title/description), and error category (if retrying a failed task).
2. Query learnings where `relevance_tags` contains ANY of those keywords (using SQLite `json_each()` and `LIKE` or exact match).
3. Score by number of tag matches + recency bias.
4. Return top N learnings within context budget.

Example tags: `["src/dag/tasks.rs", "Rust", "foreign key constraint", "test_failure", "SQLite"]`.

### Strategic Intelligence

#### `strategy_metrics` table

Aggregated iteration metrics for model selection and stuck-loop detection. Deliberately denormalized from `iteration_outcomes` for fast querying.

```sql
CREATE TABLE strategy_metrics (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    total_attempts INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,  -- Reset on success
    last_attempt_at TEXT,                             -- ISO 8601 timestamp of most recent attempt
    last_success_at TEXT,                             -- ISO 8601 timestamp of last successful completion
    difficulty_estimate TEXT,                         -- 'trivial', 'easy', 'moderate', 'hard', 'blocked' (from <difficulty-estimate> sigil)
    suggested_model TEXT,                             -- Model that successfully completed this task, or NULL
    stuck_flag INTEGER NOT NULL DEFAULT 0,            -- 1 if flagged as stuck (3+ consecutive failures)
    PRIMARY KEY (task_id)
);

CREATE INDEX idx_strategy_metrics_stuck_flag ON strategy_metrics(stuck_flag);
CREATE INDEX idx_strategy_metrics_consecutive_failures ON strategy_metrics(consecutive_failures);
```

**Column rationale:**

- `task_id`: One row per task. Updated after each iteration.
- `total_attempts`: Lifetime attempt count for this task. Increments on every claim.
- `consecutive_failures`: How many failures in a row since last success. Reset to 0 on `done`. Used for escalation logic.
- `last_attempt_at`, `last_success_at`: Timestamps for recency checks and timeout detection.
- `difficulty_estimate`: Captured from Claude's `<difficulty-estimate>` sigil. Informs future model selection for similar tasks.
- `suggested_model`: The model that successfully completed this task. Used for learning "sonnet was enough for moderate difficulty."
- `stuck_flag`: Boolean (0/1). Set to 1 when `consecutive_failures >= 3`. Triggers special handling (warnings, decomposition suggestions, human escalation).

**Indexing strategy:**

- `stuck_flag` index: Fast query for all stuck tasks (`WHERE stuck_flag = 1`).
- `consecutive_failures` index: Find tasks approaching stuck threshold (`WHERE consecutive_failures >= 2`).

**Update triggers:**

This table is updated by `memory::record_iteration_metrics()` after each iteration:

- Increment `total_attempts`
- If outcome = 'done': reset `consecutive_failures` to 0, update `last_success_at`, set `suggested_model`
- If outcome = 'failed'/'no_sigil'/'error': increment `consecutive_failures`, set `stuck_flag = 1` if `consecutive_failures >= 3`
- Update `last_attempt_at` to current timestamp
- If `<difficulty-estimate>` sigil present, update `difficulty_estimate`

### Schema Relationships

All new tables reference the existing `tasks` table via foreign keys. This maintains referential integrity and enables cascading deletes.

```
┌─────────────────┐
│     tasks       │  (existing table)
│  id (PK)        │
│  parent_id      │
│  title          │
│  status         │
│  ...            │
└────────┬────────┘
         │
         │ (1:N)
         ├──────────────────────────────────────────┐
         │                                          │
         ▼                                          ▼
┌─────────────────────────┐            ┌──────────────────────────┐
│  iteration_outcomes     │            │   strategy_metrics       │
│  task_id (FK) ───┐      │            │   task_id (PK, FK)       │
│  attempt_number  │      │            │   total_attempts         │
│  model           │      │            │   consecutive_failures   │
│  outcome         │      │            │   stuck_flag             │
│  ...             │      │            │   ...                    │
└──────────────────┼──────┘            └──────────────────────────┘
                   │
                   │ (1:1)
                   ▼
         ┌──────────────────────┐
         │  failure_reports     │
         │  task_id (FK) ───────┤
         │  attempt_number (FK) │
         │  what_was_tried      │
         │  why_it_failed       │
         │  error_category      │
         │  ...                 │
         └──────────────────────┘

         ┌──────────────────────┐
         │     learnings        │
         │  id (PK)             │
         │  task_id (FK, NULL)  │─ ─ ─ ─ ─ ─ ─ ─ ─ ─ (optional link to task)
         │  superseded_by (FK)  │───┐
         │  relevance_tags      │   │ (self-referential)
         │  ...                 │   │
         └──────────────────────┘◄──┘
```

**Foreign key cascade behavior:**

- `iteration_outcomes.task_id` → `tasks.id` ON DELETE CASCADE: If a task is deleted, all its iteration outcomes are deleted.
- `failure_reports.task_id` → `tasks.id` ON DELETE CASCADE: Deleting a task deletes its failure reports.
- `failure_reports.(task_id, attempt_number)` → `iteration_outcomes.(task_id, attempt_number)` ON DELETE CASCADE: Deleting an iteration outcome deletes its failure report.
- `strategy_metrics.task_id` → `tasks.id` ON DELETE CASCADE: Deleting a task deletes its metrics.
- `learnings.task_id` → `tasks.id` ON DELETE SET NULL: Deleting a task does NOT delete learnings, just unlinks them. Learnings are general insights that outlive their origin task.
- `learnings.superseded_by` → `learnings.id` ON DELETE SET NULL: Deleting a learning that superseded another clears the link but preserves both learnings.

**Migration strategy (v1 → v2):**

In `src/dag/db.rs`, add this to the `migrate()` function:

```rust
if from_version < 2 && to_version >= 2 {
    conn.execute_batch(r#"
        -- [All CREATE TABLE statements from above]
        -- [All CREATE INDEX statements from above]
    "#).context("Failed to create schema v2 (memory tables)")?;
}
```

Bump `SCHEMA_VERSION` to `2`. Existing databases auto-migrate on next `init_db()` call.

## Sigil Design

### Error Recovery Memory

<!-- <failure-report> sigil: format, regex, examples, backward compatibility -->

### Self-Improvement / Learning Extraction

<!-- <learning> sigil: format, regex, examples, backward compatibility -->

### Strategic Intelligence

<!-- <difficulty-estimate> sigil: format, regex, examples -->
<!-- <retry-suggestion> sigil: format, regex, examples -->

### Backward Compatibility

<!-- All new sigils are optional; their absence changes nothing -->

## Context Injection

### Error Recovery Memory

<!-- How failure history is rendered when retrying a task: Markdown template, attempt history, error details -->

### Self-Improvement / Learning Extraction

<!-- How relevant learnings are selected and injected: relevance matching, budget system, truncation -->

### Strategic Intelligence

<!-- Loop status section: iteration count, success rate, recent failures, model rationale -->

### Context Budget Management

<!-- Total memory injection budget, priority ranking, truncation strategy -->

### Prompt Template Examples

<!-- Concrete examples of what Claude would actually see with memory context injected -->

## Lifecycle

### Memory Growth

<!-- How data accumulates during a run: capture triggers, storage timing -->

### Summarization

<!-- When learnings exceed threshold: summarization triggers, merge strategy -->

### Pruning

<!-- Superseded learnings, archived failure reports, aggregated metrics -->

### Cross-Run Persistence

<!-- How learnings carry forward across separate ralph run invocations, staleness checks -->

### Failure Escalation Lifecycle

<!-- First failure -> report, second -> model escalation, third -> decomposition suggestion, Nth -> human review -->
<!-- State diagram for retry escalation path -->

## Migration Path

### Phase 1: Foundation

<!-- New DB tables, new sigil parsing, basic iteration outcome capture -->
<!-- Files to modify, new files to create, testing strategy -->

### Phase 2: Error Recovery

<!-- Failure report capture and injection, retry awareness in build_task_context() -->
<!-- Files to modify, new files to create, testing strategy -->

### Phase 3: Learning System

<!-- Learning sigil capture, storage, relevance matching, context injection -->
<!-- Files to modify, new files to create, testing strategy -->

### Phase 4: Strategic Intelligence

<!-- Data-driven model strategy, difficulty estimation, stuck-loop detection -->
<!-- Files to modify, new files to create, testing strategy -->

### Schema Migration Strategy

<!-- Version tracking in SQLite, auto-migration of existing progress.db files, backward compatibility -->

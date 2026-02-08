# Iteration Memory

> **TL;DR:** Three memory systems stored in four new SQLite tables within the existing `progress.db`. All synchronous, all optional, all backward-compatible.
>
> | System | What it does | Key tables | New sigils |
> |--------|-------------|------------|------------|
> | **Error Recovery** | Prevents retries from repeating failed approaches | `iteration_outcomes`, `failure_reports` | `<failure-report>`, `<retry-suggestion>` |
> | **Learning Extraction** | Transfers insights across tasks via tag-based relevance matching | `learnings` | `<learning>` |
> | **Strategic Intelligence** | Data-driven model escalation and stuck-loop detection | `strategy_metrics` | `<difficulty-estimate>` |
>
> **Key design decisions:**
> - Memory is project-scoped (per `progress.db`), not global
> - Context injection is budget-capped at 5000 chars (~1250 tokens) with priority: previous attempts > learnings > loop status
> - All new sigils are optional; their absence changes nothing about execution
> - Schema migration is automatic (v1 to v2) via the existing `migrate()` mechanism
> - No async, no background processes, no external dependencies — pure synchronous Rust + SQLite
> - Four-phase implementation: Foundation, Error Recovery, Learning System, Strategic Intelligence

Ralph currently treats each iteration as stateless: a fresh Claude Code session receives a task assignment but has no memory of what happened in previous iterations. This design is simple but leads to repeated failures, wasted compute on known-bad approaches, and inability to learn from experience within a run.

This spec designs three memory systems that give Ralph cross-iteration awareness while maintaining the existing synchronous, single-agent, SQLite-backed architecture.

**Three systems:**

1. **Error Recovery Memory** -- Structured failure reports that prevent repeating the same mistakes when retrying tasks
2. **Self-Improvement / Learning Extraction** -- Reusable insights captured from successful and failed iterations, injected as context into future tasks
3. **Strategic Intelligence** -- Data-driven model selection, difficulty estimation, and stuck-loop detection

## Architecture Overview

This section provides a high-level view of each system's data flow and hook points. For table schemas see [Data Model](#data-model), for sigil formats see [Sigil Design](#sigil-design), for rendering logic see [Context Injection](#context-injection), for data lifecycle see [Lifecycle](#lifecycle), and for phased implementation see [Migration Path](#migration-path).

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
    retry_suggestion TEXT,                  -- Free-form suggestion for next attempt (from <retry-suggestion> sigil)
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
- `retry_suggestion`: Free-form text from the `<retry-suggestion>` sigil. Advice from Claude to its future self about what to try differently on retry. Rendered prominently in the Previous Attempts context section (see [Context Injection — Error Recovery Memory](#error-recovery-memory-1)).
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
    superseded_by TEXT REFERENCES learnings(id) ON DELETE SET NULL,  -- Points to newer learning that replaces this one
    last_used_at TEXT                       -- Updated when this learning is included in context injection; used by pruning to avoid removing actively-useful learnings
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
- `last_used_at`: Timestamp updated by `get_relevant_learnings()` whenever this learning is included in context injection. Used by the [pruning system](#pruning) to protect actively-useful learnings from age-based removal — learnings with `last_used_at` within the last 30 days are retained even if they are older than the 90-day pruning threshold.

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

**Migration strategy (v1 → v2):** See [Schema Migration Strategy](#schema-migration-strategy) for the complete migration SQL and implementation details.

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

Ralph uses XML-style sigils embedded in Claude's output text to signal structured information. Sigils are parsed from the `ResultEvent.result` field after Claude completes. This section defines four new sigils for iteration memory, following the existing patterns established in `src/claude/events.rs`.

**Design principles:**

1. **XML-style tags:** All sigils use `<tag>content</tag>` format for easy regex parsing
2. **Whitespace tolerance:** Content is trimmed; leading/trailing whitespace is ignored
3. **First occurrence wins:** If multiple identical sigils appear, the first valid one is used
4. **Fail-safe parsing:** Malformed sigils (missing closing tag, empty content) return `None`; parsing never errors
5. **Optional by default:** All new sigils are optional; their absence changes nothing about execution
6. **Backward compatible:** Existing Ralph installations ignore unknown sigils; new sigils add capability without breaking existing workflows

### Error Recovery Memory

#### `<failure-report>` Sigil

**Purpose:** Capture structured failure information when a task fails, so retries have full context about what went wrong and what was already tried.

**Format:**

```
<failure-report>
what_tried: Brief description of approach taken (1-2 sentences)
why_failed: Root cause analysis (1-2 sentences)
error_category: test_failure | build_error | missing_file | logic_error | type_error | dependency_error | timeout | unknown
relevant_files: src/path/to/file.rs, src/another/file.rs
stack_trace: First 3-5 lines of error output or stack trace
</failure-report>
```

**Schema:** The sigil content is a key-value format with one field per line. Field order is not significant. All fields are optional except `what_tried` and `why_failed`.

**Field definitions:**

- `what_tried`: Human-readable summary of the approach Claude took (e.g., "Modified the claim_task function to check for stuck flags before claiming")
- `why_failed`: Root cause of the failure (e.g., "Foreign key constraint violation because strategy_metrics row doesn't exist yet")
- `error_category`: Machine-readable error type from a predefined enum. Used for pattern matching and relevance scoring. If omitted, defaults to `unknown`.
- `relevant_files`: Comma-separated list of file paths mentioned in the error or modified during the attempt. Used for file-based relevance matching.
- `stack_trace`: First few lines of the error output. Truncated to 500 characters at storage time.

**Parsing logic:**

```rust
/// Parse the `<failure-report>` sigil from result text.
///
/// Returns a `FailureReport` struct if found, `None` if absent or malformed.
pub fn parse_failure_report(text: &str) -> Option<FailureReport> {
    let start_tag = "<failure-report>";
    let end_tag = "</failure-report>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let content = text[content_start..content_start + end_idx].trim();

    if content.is_empty() {
        return None;
    }

    // Parse key-value pairs line by line
    let mut what_tried = None;
    let mut why_failed = None;
    let mut error_category = None;
    let mut relevant_files = Vec::new();
    let mut stack_trace = None;

    for line in content.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "what_tried" => what_tried = Some(value.to_string()),
                "why_failed" => why_failed = Some(value.to_string()),
                "error_category" => error_category = Some(value.to_string()),
                "relevant_files" => {
                    relevant_files = value.split(',').map(|s| s.trim().to_string()).collect()
                }
                "stack_trace" => stack_trace = Some(value.to_string()),
                _ => {} // Ignore unknown fields (forward compatibility)
            }
        }
    }

    // Require at minimum what_tried and why_failed
    Some(FailureReport {
        what_tried: what_tried?,
        why_failed: why_failed?,
        error_category: error_category.unwrap_or_else(|| "unknown".to_string()),
        relevant_files,
        stack_trace,
    })
}
```

**Storage:** Parsed fields are inserted into the [`failure_reports` table](#failure_reports-table). The `relevant_files` Vec is serialized to JSON.

**Example usage:**

```
I attempted to fix the foreign key constraint issue but encountered an error.

<failure-report>
what_tried: Modified claim_task() to initialize strategy_metrics row before claiming
why_failed: SQLite foreign key constraint failed because the task doesn't exist in tasks table yet
error_category: dependency_error
relevant_files: src/dag/tasks.rs, src/memory/metrics.rs
stack_trace: FOREIGN KEY constraint failed (code 787)
    at Connection::execute (src/dag/db.rs:45)
</failure-report>

<task-failed>t-0dfebf</task-failed>
```

**Edge cases:**

- **Empty sigil:** `<failure-report></failure-report>` → returns `None`
- **Missing required fields:** If `what_tried` or `why_failed` are missing → returns `None`
- **Extra fields:** Unknown fields are silently ignored (forward compatibility for future additions)
- **Multiline values:** Not supported in v1; values must fit on one line. Use `\n` escape or truncate.
- **No sigil present:** If Claude doesn't emit a `<failure-report>`, a minimal failure report is auto-generated from the `ResultEvent.result` text (truncated to 200 chars) with `error_category: unknown` and empty `what_tried`/`why_failed`.

### Self-Improvement / Learning Extraction

#### `<learning>` Sigil

**Purpose:** Capture reusable insights—both from successful iterations ("this approach worked") and failures ("this dependency is tricky")—to improve future task success rates.

**Format:**

```
<learning category="success_pattern" tags="Rust, SQLite, foreign keys">
When adding new tables that reference existing tables, always check that foreign key constraints are enabled (PRAGMA foreign_keys = ON) and that referenced rows exist before inserting.
</learning>
```

**Schema:** XML tag with two attributes (`category` and `tags`) and text content.

**Field definitions:**

- `category` (required): Coarse-grained classification. Valid values:
  - `success_pattern`: An approach that worked well
  - `pitfall`: A gotcha or mistake to avoid
  - `tool_usage`: Best practice for using a tool (git, cargo, sqlite3, etc.)
  - `code_structure`: Architectural insight about the codebase
  - `testing_strategy`: How to test a particular type of change
  - `debugging_technique`: Diagnostic approach that helped
  - `other`: Catch-all for miscellaneous learnings
- `tags` (required): Comma-separated keywords for relevance matching. Should include: file paths, programming languages, error types, tool names, domain concepts.
- **Content** (required): 1-3 sentence description of the learning. Should be actionable and concise.

**Parsing logic:**

```rust
/// Parse `<learning>` sigils from result text.
///
/// Returns a Vec of `Learning` structs (may be empty). Multiple learnings in one result are allowed.
pub fn parse_learnings(text: &str) -> Vec<Learning> {
    let mut learnings = Vec::new();
    let mut search_offset = 0;

    while let Some(start_idx) = text[search_offset..].find("<learning") {
        let abs_start = search_offset + start_idx;

        // Find the end of the opening tag
        if let Some(tag_end) = text[abs_start..].find('>') {
            let opening_tag = &text[abs_start..abs_start + tag_end + 1];

            // Extract attributes using regex or simple parsing
            let category = extract_attribute(opening_tag, "category");
            let tags_str = extract_attribute(opening_tag, "tags");

            // Find closing tag
            if let Some(close_idx) = text[abs_start + tag_end..].find("</learning>") {
                let content_start = abs_start + tag_end + 1;
                let content_end = abs_start + tag_end + close_idx;
                let content = text[content_start..content_end].trim();

                if let (Some(cat), Some(tags), false) = (category, tags_str, content.is_empty()) {
                    learnings.push(Learning {
                        category: cat,
                        tags: tags.split(',').map(|s| s.trim().to_string()).collect(),
                        content: content.to_string(),
                    });
                }

                search_offset = content_end + "</learning>".len();
                continue;
            }
        }

        // Failed to parse this occurrence, skip it
        search_offset = abs_start + 1;
    }

    learnings
}
```

**Storage:** Each parsed learning gets a unique ID (`l-{6 hex}`) and is inserted into the [`learnings` table](#learnings-table). The `tags` Vec is serialized to JSON in the `relevance_tags` column.

**Example usage:**

```
I successfully added the failure_reports table and wired up the foreign key constraints.

<learning category="success_pattern" tags="Rust, SQLite, foreign keys, migration">
When adding new tables with foreign keys in SQLite, enable PRAGMA foreign_keys = ON at connection time and use ON DELETE CASCADE to maintain referential integrity across table deletions.
</learning>

<learning category="tool_usage" tags="cargo, testing, integration tests">
Use `cargo test --test integration_test_name` to run a specific integration test file without running the entire suite, saving time during iteration.
</learning>

<task-done>t-abc123</task-done>
```

**Multiple learnings:** Claude can emit multiple `<learning>` sigils in a single result. All are captured and stored separately.

**Edge cases:**

- **Missing attributes:** If `category` or `tags` are missing → skip this learning
- **Empty content:** `<learning category="..." tags="..."></learning>` → skip this learning
- **Malformed XML:** No closing tag, invalid attribute syntax → skip this learning
- **No sigil present:** If Claude doesn't emit any `<learning>` sigils, no learnings are captured (this is the common case; learnings are opt-in)

### Strategic Intelligence

#### `<difficulty-estimate>` Sigil

**Purpose:** Let Claude assess task complexity after working on it, informing future model selection and effort estimation.

**Format:**

```
<difficulty-estimate>hard</difficulty-estimate>
```

**Schema:** Simple string content with one of five predefined values.

**Valid values:**

- `trivial`: Single-file, few-line change; no research needed; obvious solution
- `easy`: Straightforward implementation; standard patterns; minimal cross-file coordination
- `moderate`: Requires understanding multiple modules; some design decisions; typical feature work
- `hard`: Complex cross-module changes; subtle bugs; performance optimization; architectural decisions
- `blocked`: Cannot proceed without external input (missing requirements, upstream bugs, human decision needed)

**Parsing logic:**

```rust
const VALID_DIFFICULTIES: &[&str] = &["trivial", "easy", "moderate", "hard", "blocked"];

/// Parse the `<difficulty-estimate>...</difficulty-estimate>` sigil from result text.
///
/// Returns `Some(difficulty)` if a valid difficulty level is found, `None` otherwise.
pub fn parse_difficulty_estimate(text: &str) -> Option<String> {
    let start_tag = "<difficulty-estimate>";
    let end_tag = "</difficulty-estimate>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let difficulty = text[content_start..content_start + end_idx].trim();

    if VALID_DIFFICULTIES.contains(&difficulty) {
        Some(difficulty.to_string())
    } else {
        None
    }
}
```

**Storage:** Stored in [`strategy_metrics.difficulty_estimate`](#strategy_metrics-table) column. Updated on each iteration; last estimate wins.

**Example usage:**

```
I completed the task successfully after exploring the codebase.

<difficulty-estimate>moderate</difficulty-estimate>
<task-done>t-xyz789</task-done>
```

**Use in model selection:** When selecting a model for a new task, if a similar task (by file paths or keywords) was previously completed at difficulty `hard` by `sonnet`, Ralph can infer that `sonnet` is sufficient. If a `hard` task failed with `haiku`, Ralph escalates to `sonnet` or `opus`.

**Edge cases:**

- **Invalid value:** `<difficulty-estimate>super-hard</difficulty-estimate>` → returns `None`, no difficulty recorded
- **Empty:** `<difficulty-estimate></difficulty-estimate>` → returns `None`
- **Multiple estimates:** First valid one wins (consistent with other sigils)
- **No sigil:** If absent, `difficulty_estimate` remains NULL in `strategy_metrics`

#### `<retry-suggestion>` Sigil

**Purpose:** When a task fails, Claude can suggest a different approach for the retry, giving the next iteration a head start.

**Format:**

```
<retry-suggestion>
Try breaking this into two subtasks: first add the DB schema migration, then add the parsing logic. The all-at-once approach caused too many merge conflicts.
</retry-suggestion>
```

**Schema:** Free-form text content (1-3 sentences). No structured fields—this is Claude talking to its future self.

**Parsing logic:**

```rust
/// Parse the `<retry-suggestion>...</retry-suggestion>` sigil from result text.
///
/// Returns `Some(suggestion)` if found, `None` if absent or empty.
pub fn parse_retry_suggestion(text: &str) -> Option<String> {
    let start_tag = "<retry-suggestion>";
    let end_tag = "</retry-suggestion>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let suggestion = text[content_start..content_start + end_idx].trim();

    if suggestion.is_empty() {
        None
    } else {
        Some(suggestion.to_string())
    }
}
```

**Storage:** Stored in the [`failure_reports` table](#failure_reports-table) in the `retry_suggestion` column. This is part of the failure report for a specific attempt.

**Example usage:**

```
I tried to add all the memory tables in one migration but hit a foreign key cycle.

<failure-report>
what_tried: Created all four tables (iteration_outcomes, failure_reports, learnings, strategy_metrics) in a single CREATE TABLE batch
why_failed: SQLite rejected the foreign key from failure_reports to iteration_outcomes because the composite key wasn't created yet
error_category: dependency_error
relevant_files: src/dag/db.rs
stack_trace: FOREIGN KEY constraint failed
</failure-report>

<retry-suggestion>
Split the migration into two steps: first create tables with no foreign keys, then add foreign keys with ALTER TABLE. Or reorder the CREATE TABLE statements so iteration_outcomes comes before failure_reports.
</retry-suggestion>

<task-failed>t-def456</task-failed>
```

**Injection on retry:** When retrying a failed task, the `### Previous Attempts` section includes the `retry_suggestion` from the most recent failure, giving Claude a concrete starting point.

**Edge cases:**

- **No suggestion:** If absent, the retry proceeds with only the failure report context (no suggestion)
- **Multiple suggestions:** First one wins
- **Empty:** `<retry-suggestion></retry-suggestion>` → returns `None`

### Backward Compatibility

**All new sigils are optional.** Ralph continues to function exactly as before if Claude does not emit any of these sigils:

- **No `<failure-report>`:** A minimal failure report is auto-generated from `ResultEvent.result` (first 200 chars), with empty `what_tried`, `why_failed: "Task failed (no structured report)"`, and `error_category: unknown`.
- **No `<learning>`:** No learnings are captured. This is the common case; learnings are opt-in for when Claude discovers a reusable insight.
- **No `<difficulty-estimate>`:** The `difficulty_estimate` column remains NULL. Model selection proceeds without difficulty data.
- **No `<retry-suggestion>`:** The retry proceeds with failure report context only.

**Graceful degradation:** If a sigil is malformed (missing closing tag, invalid values, empty content), parsing returns `None` and execution continues. Ralph never errors due to sigil parsing failures.

**Forward compatibility:** Unknown fields in `<failure-report>` are silently ignored. Unknown categories in `<learning>` are accepted (stored as-is). This allows future extensions without breaking old Ralph versions.

**Versioning:** The sigil format is not versioned explicitly. Changes to sigil structure (new fields, new categories) are additive and backward-compatible by design. If a breaking change is ever needed, a new sigil name would be introduced (e.g., `<failure-report-v2>`).

**Documentation in system prompt:** When the memory system is active, Ralph's system prompt (built in `claude::client::build_system_prompt()`) will document these sigils in a new section, similar to how the existing task sigils are currently documented. Example:

```markdown
## Memory Sigils (Optional)

You can optionally emit these sigils to improve cross-iteration learning:

- `<failure-report>...</failure-report>` — Structured failure details (see format below)
- `<learning category="..." tags="...">...</learning>` — Capture a reusable insight
- `<difficulty-estimate>trivial|easy|moderate|hard|blocked</difficulty-estimate>` — Task complexity assessment
- `<retry-suggestion>...</retry-suggestion>` — Advice for the next attempt

These are optional. Omitting them has no negative effect; they purely add capability.
```

**Migration path:** Existing `.ralph/progress.db` files from pre-memory Ralph installations will auto-migrate on first run (v1 → v2 schema upgrade adds the new tables). Old prompts and tasks continue to work unchanged. No user action required.

## Context Injection

Context injection is the consumer side of iteration memory: it takes stored data from the [Data Model](#data-model) tables (failure reports, learnings, metrics) — populated via [sigil parsing](#sigil-design) — and renders it into the system prompt that Claude sees each iteration. All injection happens in `claude::client::build_task_context()`, which already builds the "Assigned Task" section with task details, parent context, and completed prerequisites. Memory context is appended after the existing sections, gated by data availability — if no memory data exists for a task, no memory sections appear and the prompt is identical to today's.

**Injection point in `build_task_context()`:**

```rust
pub fn build_task_context(task: &TaskInfo, memory: Option<&MemoryContext>) -> String {
    let mut output = String::new();

    // ... existing sections (Assigned Task, Parent Context, Completed Prerequisites, Reference Specs) ...

    // Memory context injection (new)
    if let Some(mem) = memory {
        output.push_str(&render_memory_context(mem));
    }

    output
}
```

The `MemoryContext` struct is populated by the run loop before calling `build_task_context()`:

```rust
/// Aggregated memory context for injection into a task's system prompt.
pub struct MemoryContext {
    /// Previous failed attempts on this specific task (from failure_reports + iteration_outcomes)
    pub previous_attempts: Vec<AttemptContext>,
    /// Relevant learnings matched against this task's keywords (from learnings table)
    pub relevant_learnings: Vec<LearningContext>,
    /// Loop-level status information (from strategy_metrics + iteration_outcomes)
    pub loop_status: LoopStatus,
    /// Total character budget for all memory sections combined
    pub budget_chars: usize,
}
```

### Error Recovery Memory

When a task is being retried (it has previous entries in `iteration_outcomes`), the `### Previous Attempts` section is injected. This is the highest-priority memory section because it directly prevents the most common failure mode: repeating the same broken approach.

**Query (run in `memory::get_failure_context()`):**

```sql
SELECT
    io.attempt_number,
    io.model,
    io.outcome,
    io.duration_ms,
    fr.what_was_tried,
    fr.why_it_failed,
    fr.error_category,
    fr.relevant_files,
    fr.stack_trace_snippet,
    fr.retry_suggestion
FROM iteration_outcomes io
LEFT JOIN failure_reports fr
    ON io.task_id = fr.task_id AND io.attempt_number = fr.attempt_number
WHERE io.task_id = ?
ORDER BY io.attempt_number ASC;
```

The LEFT JOIN ensures we get a row even for attempts that lack a structured `<failure-report>` sigil (those rows will have NULL failure report columns, and the renderer falls back to showing just the outcome and model).

**Rendering logic:**

```rust
fn render_previous_attempts(attempts: &[AttemptContext], budget: &mut CharBudget) -> String {
    if attempts.is_empty() {
        return String::new();
    }

    let mut output = String::from("\n### Previous Attempts\n\n");
    output.push_str(&format!(
        "This task has been attempted {} time(s) before. **Do not repeat these approaches.**\n\n",
        attempts.len()
    ));

    for attempt in attempts {
        let section = render_single_attempt(attempt);
        if !budget.can_fit(section.len()) {
            output.push_str("_(Earlier attempts truncated due to context budget)_\n");
            break;
        }
        budget.consume(section.len());
        output.push_str(&section);
    }

    // If the most recent attempt has a retry suggestion, highlight it
    if let Some(last) = attempts.last() {
        if let Some(ref suggestion) = last.retry_suggestion {
            output.push_str("\n**Suggested approach for this retry:**\n");
            output.push_str(suggestion);
            output.push('\n');
        }
    }

    output
}
```

**Rendered Markdown template (per attempt):**

```markdown
#### Attempt {N} ({model}, {outcome})

- **Approach:** {what_was_tried}
- **Why it failed:** {why_it_failed}
- **Error type:** {error_category}
- **Files involved:** {relevant_files as comma-separated list}
- **Error output:**
  ```
  {stack_trace_snippet, truncated to 500 chars}
  ```
```

When the `<failure-report>` sigil was not provided (auto-generated minimal report), the template degrades gracefully:

```markdown
#### Attempt {N} ({model}, {outcome})

- **Outcome:** {outcome} after {duration_ms}ms
- **No structured failure report was provided.**
```

**Attempt ordering:** Attempts are rendered in chronological order (oldest first) so Claude can see the progression of approaches. However, when budget is limited, the most recent attempt is always included (it has the freshest context) — the truncation logic removes older attempts first.

**Retry suggestion prominence:** The `retry_suggestion` from the most recent failure is rendered separately at the end of the Previous Attempts section, outside the per-attempt blocks, with bold formatting. This ensures Claude sees the suggestion even if it skims the attempt history.

### Self-Improvement / Learning Extraction

When relevant learnings exist in the `learnings` table, the `### Learnings from Previous Iterations` section is injected. This provides cross-task knowledge transfer — insights from one task that help with a different task.

**Relevance matching (run in `memory::get_relevant_learnings()`):**

The matching algorithm extracts keywords from the current task and finds learnings whose `relevance_tags` overlap:

1. **Keyword extraction from current task:**
   - Split task `title` and `description` into words
   - Extract file paths (strings matching `src/...`, `*.rs`, etc.) via regex
   - Extract error categories if retrying a failed task (from `failure_reports.error_category`)
   - Deduplicate and lowercase all keywords

2. **Query using SQLite `json_each()`:**

```sql
SELECT
    l.id,
    l.category,
    l.content,
    l.relevance_tags,
    l.created_at,
    COUNT(DISTINCT matched_tag.value) AS match_score
FROM learnings l,
     json_each(l.relevance_tags) AS tag
LEFT JOIN json_each(?) AS matched_tag     -- ? = JSON array of current task keywords
    ON LOWER(tag.value) = LOWER(matched_tag.value)
WHERE l.pruned_at IS NULL                  -- Only active learnings
  AND matched_tag.value IS NOT NULL        -- At least one tag match
GROUP BY l.id
ORDER BY
    match_score DESC,                      -- Most relevant first
    l.created_at DESC                      -- Break ties by recency
LIMIT ?;                                   -- ? = max learnings count (default 5)
```

3. **Scoring:** Each learning is scored by the number of tag matches (primary) and recency (secondary). A learning with 3 matching tags ranks higher than one with 1 matching tag, regardless of recency. Among equal-match learnings, newer ones win.

4. **Minimum threshold:** Learnings with only 1 tag match are included but deprioritized. This allows broad learnings ("Rust", "SQLite") to surface when nothing more specific matches, while keeping precision high when specific tags match ("src/dag/tasks.rs", "foreign key constraint").

**Budget limiting:** The top-N learnings (default N=5) are returned, but the character budget may further reduce this. Learnings are rendered in score order, and rendering stops when the budget is exhausted.

**Rendering logic:**

```rust
fn render_learnings(learnings: &[LearningContext], budget: &mut CharBudget) -> String {
    if learnings.is_empty() {
        return String::new();
    }

    let mut output = String::from("\n### Learnings from Previous Iterations\n\n");

    for learning in learnings {
        let entry = format!(
            "- **[{}]** {}\n",
            learning.category, learning.content
        );
        if !budget.can_fit(entry.len()) {
            break;
        }
        budget.consume(entry.len());
        output.push_str(&entry);
    }

    output
}
```

**Rendered Markdown template:**

```markdown
### Learnings from Previous Iterations

- **[success_pattern]** When adding new tables with foreign keys in SQLite, enable PRAGMA foreign_keys = ON at connection time and use ON DELETE CASCADE to maintain referential integrity.
- **[pitfall]** The `sandbox-exec` profile must be written to a temp file before invocation; passing it via stdin causes a race condition on macOS.
- **[tool_usage]** Use `cargo test --test integration_test_name` to run a specific integration test file without running the entire suite.
```

**Deduplication:** If multiple learnings have near-identical content (same category and >80% word overlap), only the most recent one is shown. This is implemented as a post-query filter in Rust rather than in SQL, since fuzzy matching in SQL is impractical.

### Strategic Intelligence

The `### Loop Status` section provides Claude with meta-awareness of the overall run: how many iterations have passed, how this task has performed, and why the current model was selected. This influences Claude's approach — for example, knowing it's on the 3rd retry with an escalated model might prompt a fundamentally different strategy.

**Query (run in `memory::get_loop_status()`):**

```sql
-- Task-specific metrics
SELECT total_attempts, consecutive_failures, difficulty_estimate, stuck_flag
FROM strategy_metrics
WHERE task_id = ?;

-- Run-wide metrics (last N iterations across all tasks)
SELECT
    COUNT(*) AS total_iterations,
    SUM(CASE WHEN outcome = 'done' THEN 1 ELSE 0 END) AS successes,
    SUM(CASE WHEN outcome IN ('failed','no_sigil','error') THEN 1 ELSE 0 END) AS failures
FROM iteration_outcomes
WHERE started_at >= datetime('now', '-2 hours');  -- Scope to current run (heuristic)
```

The "current run" is approximated by a 2-hour window from the most recent iteration. This heuristic works because Ralph runs are typically continuous; if there's a long gap, the stale metrics add harmless noise rather than causing incorrect behavior.

**Rendering logic:**

```rust
fn render_loop_status(status: &LoopStatus, budget: &mut CharBudget) -> String {
    let mut output = String::from("\n### Loop Status\n\n");

    let body = format!(
        "- **Iteration:** {} of {}\n\
         - **This task:** attempt #{}, {} consecutive failure(s)\n\
         - **Run success rate:** {}/{} iterations succeeded ({:.0}%)\n\
         - **Current model:** {} ({})\n",
        status.current_iteration,
        if status.iteration_limit > 0 {
            status.iteration_limit.to_string()
        } else {
            "unlimited".to_string()
        },
        status.task_attempts,
        status.consecutive_failures,
        status.run_successes,
        status.run_total,
        status.success_rate_pct(),
        status.current_model,
        status.model_rationale,
    );

    if !budget.can_fit(body.len()) {
        return String::new(); // Skip entirely if over budget
    }
    budget.consume(body.len());
    output.push_str(&body);

    // Stuck loop warning (high priority — always fits if loop status fits)
    if status.stuck_flag {
        output.push_str("\n> ⚠️ **Stuck loop detected.** This task has failed 3+ times consecutively.\n");
        output.push_str("> Consider: decomposing the task, trying a fundamentally different approach,\n");
        output.push_str("> or signaling `<task-failed>` with a clear explanation.\n");
    }

    output
}
```

**Rendered Markdown template:**

```markdown
### Loop Status

- **Iteration:** 7 of 20
- **This task:** attempt #3, 2 consecutive failure(s)
- **Run success rate:** 4/6 iterations succeeded (67%)
- **Current model:** opus (escalated after 2 consecutive failures)
```

When the stuck flag is set:

```markdown
### Loop Status

- **Iteration:** 9 of 20
- **This task:** attempt #4, 3 consecutive failure(s)
- **Run success rate:** 4/8 iterations succeeded (50%)
- **Current model:** opus (escalated after 3 consecutive failures)

> ⚠️ **Stuck loop detected.** This task has failed 3+ times consecutively.
> Consider: decomposing the task, trying a fundamentally different approach,
> or signaling `<task-failed>` with a clear explanation.
```

**Model rationale string:** The `model_rationale` field is generated by `strategy::select_model()` as a human-readable explanation:
- `"default (cost-optimized strategy)"` — no special reason
- `"escalated after N consecutive failures"` — failure-triggered escalation
- `"hinted by previous iteration"` — Claude's `<next-model>` sigil was used
- `"plan-then-execute: execution phase"` — strategy-dictated

### Context Budget Management

Memory context competes with the task description and system prompt for Claude's attention window. An uncapped memory section could overwhelm the actual task instructions, so a budget system limits total memory injection.

**Budget allocation:**

| Priority | Section | Default Budget | Min Budget |
|----------|---------|---------------|------------|
| 1 (highest) | Previous Attempts | 3000 chars | 500 chars |
| 2 | Relevant Learnings | 1500 chars | 300 chars |
| 3 (lowest) | Loop Status | 500 chars | 200 chars |

**Total default budget: 5000 characters** (~1250 tokens at 4 chars/token). This is approximately 2-3% of Claude's context window, leaving ample room for the task description, system prompt, and Claude's own working space.

**Budget allocation algorithm:**

```rust
/// Character budget tracker for memory context injection.
struct CharBudget {
    remaining: usize,
}

impl CharBudget {
    fn new(total: usize) -> Self {
        Self { remaining: total }
    }

    fn can_fit(&self, chars: usize) -> bool {
        chars <= self.remaining
    }

    fn consume(&mut self, chars: usize) {
        self.remaining = self.remaining.saturating_sub(chars);
    }
}

fn render_memory_context(mem: &MemoryContext) -> String {
    let mut budget = CharBudget::new(mem.budget_chars);
    let mut output = String::new();

    // Priority 1: Previous attempts (most critical for retry success)
    output.push_str(&render_previous_attempts(&mem.previous_attempts, &mut budget));

    // Priority 2: Relevant learnings (cross-task knowledge transfer)
    output.push_str(&render_learnings(&mem.relevant_learnings, &mut budget));

    // Priority 3: Loop status (situational awareness)
    output.push_str(&render_loop_status(&mem.loop_status, &mut budget));

    output
}
```

**Priority rationale:**

1. **Previous Attempts first** because preventing repeated failures has the highest ROI. A retry without failure context has near-100% chance of repeating the same mistake.
2. **Learnings second** because cross-task knowledge improves success probability, but is less critical than task-specific failure history.
3. **Loop Status last** because it's situational awareness that influences Claude's strategy but doesn't directly prevent specific errors.

**Truncation strategy:**

Each section renders items in priority order (most relevant first) and stops when its budget slice is exhausted. Within a section:

- **Previous Attempts:** Most recent attempt is always included first (highest value). If budget remains, older attempts are added in reverse chronological order. Stack traces are truncated to 500 chars. If even the most recent attempt exceeds budget, it is hard-truncated with `_(truncated)_`.
- **Learnings:** Highest-scoring learnings first. Each learning is a single bullet point (typically 100-200 chars). Rendering stops mid-list if budget is exhausted. No partial learnings — a learning either fits entirely or is omitted.
- **Loop Status:** Rendered as a single block (~300-400 chars). Either the entire block fits or it's omitted. No partial rendering.

**Budget overflow:** If the Previous Attempts section consumes the entire budget, the Learnings and Loop Status sections are silently omitted. This is intentional — when a task has extensive failure history, that history is the most valuable context.

**Configurable budget:** The total budget is configurable via the `MemoryContext.budget_chars` field, which defaults to 5000 but could be adjusted based on task complexity or model context window size. In the initial implementation, it is a compile-time constant; future iterations could make it a `[memory]` section in `.ralph.toml`.

### Prompt Template Examples

The following examples show the complete task context as Claude would see it, with memory sections injected. These examples demonstrate how the three memory systems work together in realistic scenarios.

**Example 1: First attempt (no memory context)**

When a task is attempted for the first time and no relevant learnings exist, the prompt is identical to today's format:

```markdown
## Assigned Task

**ID:** t-f13cf2
**Title:** Design and write the Context Injection section

### Description
Fill in the Context Injection section of `.ralph/specs/iteration-memory.md`. Study `src/claude/client.rs`, especially `build_task_context()` and the system prompt construction.

### Parent Context
**Parent:** Create iteration memory spec skeleton
The iteration memory specification document.

### Completed Prerequisites
- [t-0eb603] Design and write the Data Model section: Defined all new SQLite tables
- [t-0dfebf] Design and write the Sigil Design section: Specified all new sigil formats

### Reference Specs
Read all files in: .ralph/specs
```

**Example 2: Second attempt after failure (error recovery + loop status)**

The task failed once. The previous attempt's failure report and loop status are injected:

```markdown
## Assigned Task

**ID:** t-76ba69
**Title:** Design and write the Migration Path section

### Description
Write the phased migration plan for adding iteration memory to Ralph.

### Completed Prerequisites
- [t-f13cf2] Design and write the Context Injection section: Specified injection logic and budget system

### Reference Specs
Read all files in: .ralph/specs

### Previous Attempts

This task has been attempted 1 time(s) before. **Do not repeat these approaches.**

#### Attempt 1 (sonnet, failed)

- **Approach:** Tried to write all four migration phases at once, covering DB schema, sigil parsing, context injection, and strategy replacement
- **Why it failed:** Exceeded the iteration's time/token budget; only completed Phase 1 and Phase 2 before running out of output tokens
- **Error type:** timeout
- **Files involved:** .ralph/specs/iteration-memory.md
- **Error output:**
  ```
  Output truncated at 25000 tokens
  ```

**Suggested approach for this retry:**
Focus on Phase 1 and Phase 2 first, then signal task-done. The remaining phases can be covered in a follow-up task or a second pass.

### Loop Status

- **Iteration:** 6 of 20
- **This task:** attempt #2, 1 consecutive failure(s)
- **Run success rate:** 4/5 iterations succeeded (80%)
- **Current model:** sonnet (default, cost-optimized strategy)
```

**Example 3: Third attempt with learnings and stuck warning (all three systems active)**

The task has failed twice. Relevant learnings from other tasks are available. The stuck flag is set:

```markdown
## Assigned Task

**ID:** t-84be01
**Title:** Final review and polish of iteration-memory spec

### Description
Review the complete iteration-memory.md for consistency, fix any gaps, ensure all sections are complete.

### Reference Specs
Read all files in: .ralph/specs

### Previous Attempts

This task has been attempted 3 time(s) before. **Do not repeat these approaches.**

#### Attempt 1 (sonnet, failed)

- **Approach:** Attempted full review and polish in a single pass
- **Why it failed:** Made edits that introduced inconsistencies with the Data Model section
- **Error type:** logic_error
- **Files involved:** .ralph/specs/iteration-memory.md

#### Attempt 2 (sonnet, failed)

- **Approach:** Read the entire spec first, then made targeted fixes
- **Why it failed:** Missed the cross-references between Sigil Design and Context Injection sections
- **Error type:** logic_error
- **Files involved:** .ralph/specs/iteration-memory.md

#### Attempt 3 (opus, failed)

- **Approach:** Section-by-section review with cross-reference checking
- **Why it failed:** Tests failed because the example code snippets had syntax errors
- **Error type:** test_failure
- **Files involved:** .ralph/specs/iteration-memory.md
- **Error output:**
  ```
  cargo test -- --test spec_validation
  test spec_validation::code_blocks_are_valid ... FAILED
  ```

**Suggested approach for this retry:**
Focus exclusively on validating the code examples. The prose is fine; the issue is in the Rust code snippets in the Sigil Design section. Run `cargo check` on extracted code blocks.

### Learnings from Previous Iterations

- **[success_pattern]** When adding new tables with foreign keys in SQLite, enable PRAGMA foreign_keys = ON at connection time and use ON DELETE CASCADE to maintain referential integrity.
- **[pitfall]** Code examples in spec documents must use valid Rust syntax even if they are illustrative — the spec validation test extracts and compiles them.
- **[testing_strategy]** Run `cargo test` after every spec edit, not just at the end. Catches syntax issues in code blocks early.

### Loop Status

- **Iteration:** 12 of 20
- **This task:** attempt #4, 3 consecutive failure(s)
- **Run success rate:** 6/11 iterations succeeded (55%)
- **Current model:** opus (escalated after 3 consecutive failures)

> ⚠️ **Stuck loop detected.** This task has failed 3+ times consecutively.
> Consider: decomposing the task, trying a fundamentally different approach,
> or signaling `<task-failed>` with a clear explanation.
```

**Example 4: Successful task with learnings only (no failures)**

A first-attempt task where relevant learnings from prior tasks exist:

```markdown
## Assigned Task

**ID:** t-71b433
**Title:** Design and write the Lifecycle section

### Description
Write the Memory Growth, Summarization, Pruning, Cross-Run Persistence, and Failure Escalation Lifecycle subsections.

### Completed Prerequisites
- [t-f13cf2] Design and write the Context Injection section: Specified injection logic and budget system

### Reference Specs
Read all files in: .ralph/specs

### Learnings from Previous Iterations

- **[code_structure]** The iteration-memory spec follows a pattern: each section starts with a high-level description, then hook points, data flow diagram, and concrete code examples.
- **[tool_usage]** Use `cargo test --test integration_test_name` to run a specific integration test file without running the entire suite.

### Loop Status

- **Iteration:** 8 of 20
- **This task:** attempt #1, 0 consecutive failure(s)
- **Run success rate:** 6/7 iterations succeeded (86%)
- **Current model:** sonnet (default, cost-optimized strategy)
```

## Lifecycle

This section describes how iteration memory data (defined in the [Data Model](#data-model), captured via [sigils](#sigil-design), and consumed by [context injection](#context-injection)) is created, grows, evolves, and is eventually cleaned up during and across Ralph runs. Memory is not append-only — it must be actively managed to prevent unbounded growth and to keep the most relevant insights accessible.

### Memory Growth

Memory data accumulates continuously during a Ralph run. Each iteration adds new rows to the memory tables:

**Capture triggers:**

1. **Iteration start:** When `run_loop::run()` claims a task and starts an iteration, a row is inserted into `strategy_metrics` if it doesn't exist yet (or updated if it does). This happens immediately after `claim_task()` succeeds.

2. **Iteration completion:** After Claude finishes (regardless of outcome) and sigils are parsed, `run_loop::run()` invokes three memory capture functions sequentially:

   ```rust
   // After sigil processing (line ~143 in run_loop.rs)
   memory::record_iteration_metrics(&db, task_id, model, outcome, duration, tokens)?;

   if outcome == Outcome::Failed || outcome == Outcome::NoSigil {
       memory::capture_failure(&db, task_id, &result_event)?;
   }

   memory::capture_learnings(&db, task_id, &result_event)?;
   ```

3. **Sigil presence:** Learnings and failure reports are only captured if the corresponding sigils are present in Claude's output. If Claude emits no `<learning>` sigils, no learnings are stored. If no `<failure-report>` sigil is present, a minimal auto-generated report is stored for failed tasks.

**Storage timing:**

All memory writes happen synchronously at iteration completion, using the same SQLite connection (`&Db`) that the run loop uses for task state. This ensures atomicity: if the iteration commit fails, memory data is rolled back too. WAL mode (already enabled for `progress.db`) ensures concurrent readers don't block writes.

**Growth rate estimate:**

For a typical Ralph run completing 20 tasks with an 80% success rate:

- **`iteration_outcomes`:** 20 rows × 200 bytes = ~4 KB
- **`failure_reports`:** 4 failures × 1 KB = ~4 KB (includes stack traces)
- **`learnings`:** ~10 learnings × 500 bytes = ~5 KB (assumes ~1 learning per 2 successful tasks)
- **`strategy_metrics`:** 20 rows × 150 bytes = ~3 KB

**Total per run: ~16 KB**. Even with 1000 iterations, the database remains under 1 MB, well within SQLite's performance sweet spot.

**No background processes:** All capture is synchronous; there are no async writers or background aggregation jobs. This aligns with Ralph's synchronous architecture.

### Summarization

Learnings can accumulate over time, especially in long-running multi-day projects where hundreds of tasks are completed. To keep learnings actionable and prevent the `learnings` table from growing unbounded, Ralph employs an optional summarization system that merges similar learnings and removes redundant ones.

**Trigger: Learning count threshold**

Summarization is triggered when the number of active learnings (WHERE `pruned_at IS NULL`) exceeds a threshold, default **200 learnings**. This is checked at the end of each iteration after `capture_learnings()`:

```rust
let active_count = db.query_row("SELECT COUNT(*) FROM learnings WHERE pruned_at IS NULL", [])?;

if active_count > SUMMARIZATION_THRESHOLD {
    memory::summarize_learnings(&db)?;
}
```

**Merge strategy:**

The summarization algorithm groups learnings by category, then by tag overlap, and merges clusters of similar learnings:

1. **Categorize:** Group learnings by `category` (`success_pattern`, `pitfall`, etc.)
2. **Cluster by similarity:** Within each category, identify learnings with high tag overlap (≥50% shared tags). These are candidates for merging.
3. **Merge cluster:** For each cluster of N similar learnings:
   - Generate a merged learning that generalizes the content (using a simple template: "Common pattern: {recurring theme}. Examples: {bulleted list}")
   - Create a new learning row with merged content and the union of all tags
   - Mark all cluster members as `pruned_at = NOW()` and set their `superseded_by` to the new learning ID
4. **Preserve recency:** The newest learning in each cluster is preserved as-is (not pruned), even if it matches a cluster. This ensures fresh learnings remain visible immediately.

**Example merge:**

Before summarization:

```
l-abc123 | success_pattern | tags: ["Rust", "SQLite", "foreign keys"]
  "Enable PRAGMA foreign_keys = ON when creating tables with FKs."

l-def456 | success_pattern | tags: ["Rust", "SQLite", "CASCADE"]
  "Use ON DELETE CASCADE to maintain referential integrity."

l-ghi789 | success_pattern | tags: ["SQLite", "foreign keys", "migration"]
  "Check foreign key constraints after schema migrations with PRAGMA foreign_key_check."
```

After summarization:

```
l-jkl012 | success_pattern | tags: ["Rust", "SQLite", "foreign keys", "CASCADE", "migration"]
  "SQLite foreign key best practices: enable PRAGMA foreign_keys = ON, use ON DELETE CASCADE, and validate with PRAGMA foreign_key_check after migrations."

l-abc123 | (pruned_at = 2026-02-08T12:00:00Z, superseded_by = l-jkl012)
l-def456 | (pruned_at = 2026-02-08T12:00:00Z, superseded_by = l-jkl012)
l-ghi789 | (pruned_at = 2026-02-08T12:00:00Z, superseded_by = l-jkl012)
```

**Content generation:**

The merged content is generated using a simple template system in Rust, not LLM-based summarization (keeping Ralph dependency-free). The template identifies common keywords in the cluster and constructs a sentence. This is imperfect but avoids the cost and latency of invoking Claude for summarization.

**Fallback:** If the merge algorithm produces nonsensical output (detected by length heuristics or empty content), the merge is skipped and the learnings are left as-is. Summarization is best-effort, not critical path.

**Frequency:** Summarization runs at most once per 10 iterations to avoid overhead. A `last_summarized_at` timestamp is tracked in-memory (not persisted) to rate-limit the operation.

### Pruning

Pruning complements summarization by removing memory data that is no longer relevant. Unlike summarization (which merges similar learnings), pruning deletes or archives data based on age, task status, and usefulness.

**Pruning triggers:**

1. **Task completion:** When a task reaches `done` or `failed` status permanently (not just temporarily during a retry), its failure reports and iteration outcomes can be pruned after a grace period. Default grace period: **7 days** after task completion.

2. **Old learnings:** Learnings older than **90 days** that have never been matched in a relevance query (indicating they are too specific or obsolete) are soft-deleted by setting `pruned_at`.

3. **Manual pruning:** A future `ralph prune` subcommand could allow users to manually trigger pruning (not in v1 scope).

**Pruning logic:**

Pruning runs at the end of each Ralph run (after all tasks are resolved or the iteration limit is hit), not during the run. This avoids I/O overhead during iterations.

```rust
// At the end of run_loop::run(), before returning outcome
memory::prune_old_data(&db)?;
```

**What gets pruned:**

| Data | Condition | Action |
|------|-----------|--------|
| `failure_reports` | Task is `done` and >7 days old | DELETE (hard delete) |
| `iteration_outcomes` | Task is `done` and >7 days old | DELETE |
| `strategy_metrics` | Task is `done` and >30 days old | DELETE |
| `learnings` | Age >90 days AND never matched | Set `pruned_at` (soft delete) |
| `learnings` | `superseded_by` is set AND >30 days old | Set `pruned_at` |

**Rationale:**

- **Short grace period for failure data:** Once a task succeeds, its failure history is no longer needed. Keeping it for 7 days allows post-mortem analysis but avoids clutter.
- **Long grace period for learnings:** Learnings are cross-task, so they remain valuable long after the origin task completes. A 90-day window ensures learnings are available across multi-month projects.
- **Soft delete for learnings:** Setting `pruned_at` rather than hard-deleting allows "undo" functionality in the future (e.g., `ralph restore-learning l-abc123`) and forensic analysis.

**Query-based staleness detection:**

To identify "never matched" learnings, the pruning system checks if a learning's ID has appeared in any query results since the last pruning run. This is tracked via a simple heuristic: if a learning was created >90 days ago and has never been returned by `get_relevant_learnings()` in the current run, it's a candidate for pruning. In v1, this is approximated by pruning learnings whose `created_at` is >90 days old and whose `task_id` is NULL or references a completed task (indicating it hasn't been reinforced by recent usage).

**Avoiding over-pruning:**

Learnings with high match scores in recent relevance queries are marked as "recently used" by updating a new `last_used_at` column (added to `learnings` table in the schema). This column is updated by `get_relevant_learnings()` when a learning is included in context injection. Pruning skips learnings with `last_used_at` within the last 30 days, even if they are >90 days old.

**Compaction:**

After pruning, SQLite's auto-vacuum feature (enabled in `db.rs` via `PRAGMA auto_vacuum = FULL`) reclaims disk space from deleted rows. No manual `VACUUM` command is needed.

### Cross-Run Persistence

The `.ralph/progress.db` database persists across separate `ralph run` invocations. This means learnings, strategy metrics, and failure reports from one run are available to subsequent runs **on the same codebase**.

**Key design principle:** Memory is **project-scoped**, not global. Each `.ralph/progress.db` file is tied to a specific project (the directory containing `.ralph.toml`). Learnings from one project do not leak into another.

**How learnings carry forward:**

1. **Initial run:** User runs `ralph run` on a fresh project. The `.ralph/progress.db` file is created (or schema-migrated if it exists from a pre-memory version of Ralph). During the run, learnings are captured and stored.

2. **Subsequent run:** User runs `ralph run` again days or weeks later. The existing `progress.db` is opened, and all active learnings (`WHERE pruned_at IS NULL`) are available for relevance matching in the new run. If the new tasks involve similar file paths or error types, the old learnings are injected into context.

3. **Staleness check:** To prevent stale learnings from accumulating indefinitely, the pruning system (see above) soft-deletes learnings that haven't been matched in recent runs. This is a passive staleness check — no explicit "last used" tracking across runs in v1 (deferred to future iteration).

**Scenario: Codebase changes between runs**

When the codebase structure changes significantly (e.g., a file is moved from `src/foo.rs` to `src/bar.rs`), learnings tagged with the old file path become less relevant. This is handled gracefully:

- **Exact path matches fail silently:** If a learning is tagged with `src/foo.rs` but that file no longer exists, the relevance matching system simply doesn't match it (substring match fails).
- **Partial path matches still work:** Learnings tagged with higher-level paths like `src/` or keywords like `Rust` or `SQLite` remain relevant even as file structure changes.
- **Pruning removes irrelevant learnings over time:** If a file-specific learning never matches in 90 days, it's pruned.

**Scenario: Long gap between runs (months or years)**

If a user runs `ralph run`, then returns to the project 6 months later:

- **Old learnings persist** but are likely pruned due to age (90-day threshold)
- **Old failure reports are deleted** if their tasks were completed (7-day grace period)
- **Strategy metrics persist indefinitely** unless the task is completed and then pruned (30-day grace period)

This is intentional: long-dormant projects start "fresh" without carrying stale context forward, but recently completed projects retain full memory.

**Manual reset:**

If a user wants to clear all memory and start fresh, they can delete the `.ralph/progress.db` file entirely. Ralph will recreate it on the next `ralph run`. This is documented in the CLI help but not exposed as a dedicated subcommand (users can `rm .ralph/progress.db` manually).

**No global learning database:**

Unlike some agent systems that maintain a shared knowledge base across all projects, Ralph's memory is deliberately project-local. This avoids cross-contamination between unrelated codebases and keeps memory queries fast (no need to filter by "project ID").

### Failure Escalation Lifecycle

When a task fails repeatedly, Ralph's memory system triggers escalating interventions: first it captures detailed context, then it escalates the model, then it suggests decomposition, and finally it flags the task for human review. This escalation path is automatic and data-driven.

**State transitions on failure:**

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Task Failure Lifecycle                      │
└─────────────────────────────────────────────────────────────────────┘

  Attempt 1                      Attempt 2                     Attempt 3
     │                              │                              │
     ▼                              ▼                              ▼
  ┌────────┐                    ┌────────┐                    ┌────────┐
  │ Claim  │                    │ Claim  │                    │ Claim  │
  │ Task   │                    │ Task   │                    │ Task   │
  └───┬────┘                    └───┬────┘                    └───┬────┘
      │                             │                             │
      │ (model: sonnet)             │ (model: sonnet)             │ (model: opus)
      │                             │                             │
      ▼                             ▼                             ▼
  ┌─────────┐                   ┌─────────┐                   ┌─────────┐
  │ Claude  │                   │ Claude  │                   │ Claude  │
  │ Invoke  │                   │ Invoke  │                   │ Invoke  │
  └───┬─────┘                   └───┬─────┘                   └───┬─────┘
      │                             │                             │
      │ <task-failed>               │ <task-failed>               │ <task-failed>
      │                             │                             │
      ▼                             ▼                             ▼
  ┌──────────────────┐         ┌──────────────────┐         ┌──────────────────┐
  │ Capture failure  │         │ Capture failure  │         │ Capture failure  │
  │ report           │         │ report           │         │ report           │
  └────┬─────────────┘         └────┬─────────────┘         └────┬─────────────┘
       │                            │                            │
       │ consecutive_failures = 1   │ consecutive_failures = 2   │ consecutive_failures = 3
       │                            │                            │
       ▼                            ▼                            ▼
  ┌──────────────────┐         ┌──────────────────┐         ┌──────────────────┐
  │ Update strategy_ │         │ Update strategy_ │         │ Update strategy_ │
  │ metrics          │         │ metrics          │         │ metrics          │
  │                  │         │ + escalate model │         │ + set stuck_flag │
  └────┬─────────────┘         └────┬─────────────┘         └────┬─────────────┘
       │                            │                            │
       │ No escalation              │ Model: sonnet → opus       │ Stuck warning
       │                            │                            │
       ▼                            ▼                            ▼
  ┌──────────────────┐         ┌──────────────────┐         ┌──────────────────┐
  │ release_claim()  │         │ release_claim()  │         │ release_claim()  │
  │ (retry eligible) │         │ (retry eligible) │         │ (retry eligible) │
  └──────────────────┘         └──────────────────┘         └──────────────────┘
       │                            │                            │
       │ Loop continues             │ Loop continues             │ Loop continues
       │                            │                            │
       └───────────> Retry ─────────┴───────────> Retry ─────────┴───────────> Retry


  Attempt 4+
     │
     ▼
  ┌────────┐
  │ Claim  │
  │ Task   │
  └───┬────┘
      │ (model: opus)
      │
      ▼
  ┌─────────┐
  │ Claude  │
  │ sees:   │
  │ "⚠️ Stuck│
  │  loop   │
  │  detected│
  │  ..."   │
  └───┬─────┘
      │
      │ (Claude decides to decompose or emit detailed <task-failed>)
      │
      ▼
  ┌──────────────────┐
  │ Either:          │
  │ 1. Task succeeds │
  │ 2. Task fails    │
  │    with detailed │
  │    explanation   │
  │ 3. Task decom-   │
  │    posed into    │
  │    subtasks      │
  └──────────────────┘
```

**Escalation stages:**

| Stage | Trigger | Action | Effect |
|-------|---------|--------|--------|
| **1. Capture** | First failure (`consecutive_failures = 1`) | Store failure report and retry suggestion | Next iteration sees Previous Attempts section |
| **2. Escalate Model** | Second consecutive failure (`consecutive_failures = 2`) | `strategy::select_model()` escalates to next tier (haiku→sonnet, sonnet→opus) | More capable model attempts the task |
| **3. Stuck Warning** | Third consecutive failure (`consecutive_failures = 3`) | Set `stuck_flag = 1`, inject stuck warning in Loop Status | Claude sees explicit warning to try a different approach |
| **4. Human Escalation** | Fourth+ consecutive failure | No automatic action; stuck warning persists | User should manually intervene (abort run, decompose task, or fix code manually) |

**Implementation in `strategy.rs`:**

The model selection logic reads `consecutive_failures` from `strategy_metrics` and escalates accordingly:

```rust
pub fn select_model(
    db: &Db,
    task_id: &str,
    strategy: ModelStrategy,
    hint: Option<&str>,
) -> (String, String) {
    // Check for Claude's hint first (highest priority)
    if let Some(model) = hint {
        return (model.to_string(), "hinted by previous iteration".to_string());
    }

    // Query strategy_metrics for this task
    let metrics = db.query_row(
        "SELECT consecutive_failures, difficulty_estimate FROM strategy_metrics WHERE task_id = ?",
        [task_id],
    ).ok();

    match strategy {
        ModelStrategy::CostOptimized => {
            if let Some((failures, _)) = metrics {
                if failures >= 2 {
                    return ("opus".to_string(), format!("escalated after {} consecutive failures", failures));
                }
            }
            ("sonnet".to_string(), "default (cost-optimized strategy)".to_string())
        }
        ModelStrategy::Escalate => {
            if let Some((failures, _)) = metrics {
                match failures {
                    0 => ("haiku".to_string(), "default (escalate strategy)".to_string()),
                    1 => ("sonnet".to_string(), "escalated after 1 failure".to_string()),
                    _ => ("opus".to_string(), format!("escalated after {} failures", failures)),
                }
            } else {
                ("haiku".to_string(), "default (escalate strategy)".to_string())
            }
        }
        // ... other strategies
    }
}
```

**Stuck flag injection:**

When `stuck_flag = 1`, the `render_loop_status()` function (in `memory::context`) appends the stuck warning to the Loop Status section. This warning is always visible when the stuck condition is active, regardless of context budget (it has highest priority within the Loop Status section).

**Decomposition suggestion:**

The stuck warning message explicitly suggests "decomposing the task" as one option. Ralph does not auto-decompose tasks (that would require invoking Claude in a side-channel, breaking the synchronous loop model). Instead, Claude is prompted to either:

1. **Succeed** by trying a fundamentally different approach
2. **Fail with explanation** using `<task-failed>` and a detailed `<failure-report>` so a human can understand why the task is blocked
3. **Manually decompose** by suggesting in the output text (not a sigil) that the task should be split, which the human can act on by editing the task DAG or creating new tasks

**Clearing the stuck flag:**

When a task succeeds (outcome = `done`), `consecutive_failures` is reset to 0 and `stuck_flag` is set to 0. This happens in `memory::record_iteration_metrics()`:

```rust
if outcome == Outcome::Done {
    db.execute(
        "UPDATE strategy_metrics
         SET consecutive_failures = 0,
             stuck_flag = 0,
             last_success_at = ?,
             suggested_model = ?
         WHERE task_id = ?",
        [timestamp, model, task_id],
    )?;
}
```

**No infinite retries:**

The escalation system does not prevent infinite retries. If Ralph's `--limit` is set high enough (or 0 = unlimited), a stuck task will retry indefinitely. This is intentional: the user controls the iteration limit, and the stuck warning gives Claude (and the user monitoring the run) enough context to decide when to give up. In a future iteration, a hard retry limit per task (e.g., max 5 attempts) could be added as a safety mechanism.

## Migration Path

This section describes a phased implementation plan for adding iteration memory to Ralph. Each phase is self-contained and shippable: Phase 1 lays the database and parsing foundation (tables from the [Data Model](#data-model), parsers from [Sigil Design](#sigil-design)), Phase 2 adds error recovery ([Context Injection — Error Recovery](#error-recovery-memory-1)), Phase 3 adds the learning system ([Context Injection — Learning Extraction](#self-improvement--learning-extraction-1)), and Phase 4 replaces the heuristic model strategy with data-driven intelligence ([Context Injection — Strategic Intelligence](#strategic-intelligence-1)). Each phase builds on the previous one but does not break existing functionality — at any point, Ralph can be released with only the phases completed so far.

**Key constraint: backward compatibility.** Existing `.ralph/progress.db` files must auto-migrate without user action. Old prompts that don't emit new sigils must work unchanged. The existing task DAG system (`tasks`, `dependencies` tables) is never modified — only new tables are added.

### Phase 1: Foundation

**Goal:** Create the database schema for all memory tables, add sigil parsers for the new sigils, and wire up basic iteration outcome recording in the run loop. After this phase, Ralph records structured data about each iteration but does not yet use it to modify behavior.

**Files to modify:**

| File | Changes |
|------|---------|
| `src/dag/db.rs` | Bump `SCHEMA_VERSION` from `1` to `2`. Add a v1→v2 migration branch in `migrate()` that creates four new tables (`iteration_outcomes`, `failure_reports`, `learnings`, `strategy_metrics`) and their indexes. All `CREATE TABLE` and `CREATE INDEX` statements from the Data Model section go here, wrapped in a single `execute_batch()` call. The migration is idempotent: the `if from_version < 2 && to_version >= 2` guard ensures it runs only once. Also add `PRAGMA auto_vacuum = FULL` to the connection setup (alongside existing WAL mode and foreign keys pragmas) so that pruning reclaims disk space. |
| `src/claude/events.rs` | Add four new parsing functions below the existing `parse_next_model_hint()`, `parse_task_done()`, and `parse_task_failed()`: `parse_failure_report()`, `parse_learnings()`, `parse_difficulty_estimate()`, and `parse_retry_suggestion()`. Each follows the same pattern as the existing parsers (find start tag, find end tag, trim content, validate). Add corresponding struct definitions: `FailureReport { what_tried, why_failed, error_category, relevant_files, stack_trace }`, `Learning { category, tags, content }`. Add a helper function `extract_attribute(tag: &str, attr: &str) -> Option<String>` for parsing XML attributes in `<learning>` tags. Extend `ResultEvent` with four new fields: `failure_report: Option<FailureReport>`, `learnings: Vec<Learning>`, `difficulty_estimate: Option<String>`, `retry_suggestion: Option<String>`. |
| `src/claude/parser.rs` | In the `"result"` event branch of `parse_event()`, after the existing sigil extraction calls (lines ~48-59), add calls to the four new parsers and wire the results into the `ResultEvent` constructor. This is straightforward plumbing: `failure_report: parse_failure_report(&result_text)`, etc. |
| `src/run_loop.rs` | After the sigil handling block (after task completion/failure is processed, around line 144), add a call to `memory::record_iteration_metrics()`. This records the iteration's task_id, model, duration, token counts, and outcome into `iteration_outcomes` and updates `strategy_metrics`. This is the minimal "always capture" hook — it runs regardless of whether Claude emitted any memory sigils. The function receives `&db`, the task_id, the model string, the `ResultEvent`, and the outcome classification. |

**New files to create:**

| File | Purpose |
|------|---------|
| `src/memory/mod.rs` | Module root. Defines the public API: `record_iteration_metrics()`, `capture_failure()`, `capture_learnings()`, `get_failure_context()`, `get_relevant_learnings()`, `get_loop_status()`, `prune_old_data()`. In Phase 1, only `record_iteration_metrics()` is fully implemented; the others are stubbed as no-ops returning empty results. Re-exports submodule types. |
| `src/memory/metrics.rs` | Implements `record_iteration_metrics()`. Inserts a row into `iteration_outcomes` (computing `attempt_number` as `SELECT COALESCE(MAX(attempt_number), 0) + 1 FROM iteration_outcomes WHERE task_id = ?`). Upserts into `strategy_metrics` using `INSERT OR REPLACE` (incrementing `total_attempts`, updating `consecutive_failures` based on outcome, setting timestamps, and conditionally setting `stuck_flag` when `consecutive_failures >= 3`). If a `difficulty_estimate` is present in the `ResultEvent`, stores it. |

**Module registration:**

Add `mod memory;` to `src/main.rs` (or `src/lib.rs` if using a library crate) alongside the existing module declarations.

**Testing strategy:**

1. **Unit tests for sigil parsers** (`src/claude/events.rs`): Test each new parser with valid input, empty input, malformed input, missing closing tags, and multiple sigils. Follow the pattern of existing tests (if any) or add a `#[cfg(test)] mod tests` block. Key test cases:
   - `parse_failure_report()`: Valid sigil → `Some(FailureReport)`, empty sigil → `None`, missing `what_tried` → `None`, extra fields → ignored
   - `parse_learnings()`: Zero sigils → empty vec, one sigil → vec of 1, two sigils → vec of 2, malformed attributes → skipped
   - `parse_difficulty_estimate()`: Valid value → `Some("hard")`, invalid value → `None`
   - `parse_retry_suggestion()`: Valid → `Some(text)`, empty → `None`

2. **Integration test for DB migration** (`tests/migration.rs` or inline in `src/dag/db.rs`): Create a v1 database manually (with only `tasks`, `dependencies`, `task_logs` tables), call `init_db()`, and verify all four new tables exist via `SELECT name FROM sqlite_master WHERE type='table'`. Verify indexes exist. Verify `PRAGMA user_version` returns `2`.

3. **Integration test for metrics recording** (`tests/memory_metrics.rs`): Create a fresh database, insert a task, call `record_iteration_metrics()` with a mock outcome, and verify rows exist in both `iteration_outcomes` and `strategy_metrics` with correct values. Test the `attempt_number` auto-increment by calling `record_iteration_metrics()` three times and checking attempt numbers are 1, 2, 3.

4. **Regression test:** Run `cargo test` to ensure existing DAG tests still pass. The new tables should not affect existing task operations.

**Verification:**

```bash
cargo build                    # Ensure compilation succeeds
cargo test                     # All existing + new tests pass
cargo run -- init              # Creates progress.db with v2 schema
sqlite3 .ralph/progress.db "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;"
# Expected: dependencies, failure_reports, iteration_outcomes, learnings, strategy_metrics, task_logs, tasks
```

### Phase 2: Error Recovery

**Goal:** Capture structured failure reports when tasks fail and inject previous attempt context into retry prompts. After this phase, when a task is retried, Claude sees what was tried before and why it failed, dramatically reducing repeated failures.

**Prerequisites:** Phase 1 complete (DB tables exist, `ResultEvent` carries failure report data, metrics recording works).

**Files to modify:**

| File | Changes |
|------|---------|
| `src/run_loop.rs` | After the existing `record_iteration_metrics()` call (added in Phase 1), add a conditional block: if the outcome is `Failed` or `NoSigil` (i.e., Claude didn't emit `<task-done>`), call `memory::capture_failure(&db, &task_id, &result_event)`. This extracts the `FailureReport` from `result_event.failure_report` (parsed in Phase 1) and inserts it into the `failure_reports` table. If no `<failure-report>` sigil was present, auto-generate a minimal report: `what_tried = ""`, `why_failed = "Task failed (no structured report)"`, `error_category = "unknown"`, `relevant_files = []`, `stack_trace = result_event.result.chars().take(500).collect()`. Also store the `retry_suggestion` if present. |
| `src/claude/client.rs` | Modify `build_task_context()` signature to accept an optional `MemoryContext` parameter: `pub fn build_task_context(task: &TaskInfo, memory: Option<&MemoryContext>) -> String`. After the existing "Completed Prerequisites" section (around line 75), add a call to `render_previous_attempts()` if `memory.previous_attempts` is non-empty. The `MemoryContext` struct is defined in `src/memory/context.rs` (new file). In Phase 2, only the `previous_attempts` field is populated; `relevant_learnings` and `loop_status` are empty/default. Also update `build_system_prompt()` to document the `<failure-report>` and `<retry-suggestion>` sigils in a new "Memory Sigils (Optional)" section appended after the "Model Hint" section. |
| `src/run_loop.rs` (second change) | Before calling `claude::client::run()`, query `memory::get_failure_context(&db, &task_id)` to retrieve previous attempts for the assigned task. Construct a `MemoryContext` with the returned `Vec<AttemptContext>` and pass it to `build_task_context()`. This is the injection side: the run loop gathers memory data and feeds it into the prompt builder. |

**New files to create:**

| File | Purpose |
|------|---------|
| `src/memory/errors.rs` | Implements `capture_failure()` and `get_failure_context()`. `capture_failure()` inserts into `failure_reports` (computing `attempt_number` from the matching `iteration_outcomes` row). `get_failure_context()` runs the LEFT JOIN query from the Context Injection section, returning `Vec<AttemptContext>`. The `AttemptContext` struct mirrors the query columns: `attempt_number`, `model`, `outcome`, `duration_ms`, `what_was_tried`, `why_it_failed`, `error_category`, `relevant_files` (deserialized from JSON), `stack_trace_snippet`, `retry_suggestion`. |
| `src/memory/context.rs` | Defines `MemoryContext`, `AttemptContext`, `LearningContext`, `LoopStatus` structs. Implements `render_memory_context()`, `render_previous_attempts()`, `render_single_attempt()`, and the `CharBudget` tracker. In Phase 2, `render_learnings()` and `render_loop_status()` are implemented but produce empty output when no data is present. The rendering logic and markdown templates follow the Context Injection section exactly. |

**Testing strategy:**

1. **Unit tests for failure capture** (`src/memory/errors.rs`): Insert a task, record an iteration metric (Phase 1), then call `capture_failure()` with a `FailureReport` struct. Verify the row exists in `failure_reports` with correct columns. Test auto-generation when `failure_report` is `None`.

2. **Unit tests for failure context retrieval** (`src/memory/errors.rs`): Insert a task with two failed attempts (both `iteration_outcomes` and `failure_reports` rows), then call `get_failure_context()`. Verify it returns two `AttemptContext` entries in chronological order. Verify the LEFT JOIN works: insert an `iteration_outcomes` row without a matching `failure_reports` row and confirm it still appears (with `None` for failure-specific fields).

3. **Unit tests for context rendering** (`src/memory/context.rs`): Call `render_previous_attempts()` with various inputs: empty vec → empty string, one attempt → single block, two attempts → two blocks, attempt with retry suggestion → suggestion rendered at end. Test `CharBudget` enforcement: pass a budget of 100 chars and verify truncation message appears.

4. **Integration test for round-trip** (`tests/error_recovery.rs`): Create a database, insert a task, simulate a failed iteration (record metrics + capture failure), then call `get_failure_context()` and `render_previous_attempts()`. Verify the rendered markdown contains the expected text. Then simulate a second failed iteration with different content and verify both attempts appear.

5. **Integration test for `build_task_context()` with memory**: Construct a `TaskInfo` and `MemoryContext` and call `build_task_context()`. Verify the output contains the "### Previous Attempts" header. Verify that passing `None` for memory produces identical output to the pre-Phase-2 behavior (regression test).

**Verification:**

```bash
cargo test                     # All tests pass
cargo run -- run --once        # Single iteration; check log file for memory sigil documentation in system prompt
```

### Phase 3: Learning System

**Goal:** Capture reusable insights from Claude's `<learning>` sigils, store them with relevance tags, and inject the most relevant learnings into future task contexts. After this phase, knowledge transfers across tasks — insights from completing one task improve success on related tasks.

**Prerequisites:** Phase 2 complete (failure capture works, `MemoryContext` and rendering infrastructure exist, `build_task_context()` accepts memory parameter).

**Files to modify:**

| File | Changes |
|------|---------|
| `src/run_loop.rs` | After the `capture_failure()` call (added in Phase 2), add `memory::capture_learnings(&db, &task_id, &result_event)`. This runs unconditionally (on both success and failure), extracting any `<learning>` sigils from the result. Most iterations produce zero learnings, so this is a fast no-op in the common case. Before the Claude invocation, extend the `MemoryContext` construction to also call `memory::get_relevant_learnings(&db, &task_id, &task_info)` and populate the `relevant_learnings` field. |
| `src/claude/client.rs` | Update `build_system_prompt()` to document the `<learning>` sigil in the "Memory Sigils (Optional)" section (alongside `<failure-report>` from Phase 2). Include the format with `category` and `tags` attributes and list the valid category values. The `render_memory_context()` function (from `memory::context`) already handles learnings rendering — no changes needed to `build_task_context()` itself. |
| `src/memory/context.rs` | The `render_learnings()` function (stubbed in Phase 2) is now fully implemented: iterates over `Vec<LearningContext>`, renders each as a bullet point `- **[{category}]** {content}`, and respects the `CharBudget`. Add deduplication logic: if two learnings have the same category and >80% word overlap (computed by splitting into word sets and comparing intersection/union), keep only the most recent one. |

**New files to create:**

| File | Purpose |
|------|---------|
| `src/memory/learnings.rs` | Implements `capture_learnings()` and `get_relevant_learnings()`. `capture_learnings()` iterates over `result_event.learnings` (the `Vec<Learning>` parsed in Phase 1), generates a unique ID for each (`l-{6 hex}` using the same ID generation as `src/dag/ids.rs`), and inserts into the `learnings` table with `relevance_tags` serialized as JSON. `get_relevant_learnings()` implements the relevance matching algorithm: (1) extract keywords from task title and description (split on whitespace, extract file paths via regex `[a-zA-Z_/]+\.[a-z]+`, lowercase all), (2) run the `json_each()` query from the Context Injection section to find learnings with overlapping tags, (3) return `Vec<LearningContext>` sorted by match_score descending then recency. The `LearningContext` struct contains: `id`, `category`, `content`, `match_score`, `created_at`. |

**Testing strategy:**

1. **Unit tests for learning capture** (`src/memory/learnings.rs`): Parse a `ResultEvent` with two `<learning>` sigils, call `capture_learnings()`, verify two rows exist in the `learnings` table with correct category, content, and JSON-serialized `relevance_tags`.

2. **Unit tests for relevance matching** (`src/memory/learnings.rs`): Insert several learnings with known tags. Create a task whose description mentions some of those tags. Call `get_relevant_learnings()` and verify: (a) learnings with matching tags are returned, (b) learnings with more matching tags rank higher, (c) learnings with `pruned_at` set are excluded, (d) learnings with zero matching tags are not returned.

3. **Unit tests for deduplication** (`src/memory/context.rs`): Create two learnings with identical category and near-identical content. Call `render_learnings()` and verify only one appears in output.

4. **Integration test for cross-task knowledge transfer** (`tests/learning_system.rs`): Create two tasks (A and B). Simulate completing task A with a learning tagged `["src/dag/db.rs", "SQLite"]`. Then simulate starting task B whose description mentions `src/dag/db.rs`. Call `get_relevant_learnings()` for task B and verify the learning from task A is returned.

5. **Edge case test: `json_each()` availability**: Verify that the SQLite version bundled by `rusqlite` supports `json_each()`. If not, implement a fallback using `LIKE '%tag%'` on the raw JSON string (less precise but functional). Test with both approaches.

**Verification:**

```bash
cargo test                     # All tests pass
cargo run -- run --once        # Single iteration; verify learning sigil docs in system prompt
```

### Phase 4: Strategic Intelligence

**Goal:** Replace the heuristic-based model selection (which reads `progress.db` as raw text and searches for keywords) with data-driven selection using `strategy_metrics`. Add loop status injection and stuck-loop detection. After this phase, model escalation is based on actual failure counts rather than text pattern matching, and Claude has full situational awareness of the run's progress.

**Prerequisites:** Phase 3 complete (all memory tables populated, `MemoryContext` fully functional, rendering infrastructure handles all three sections).

**Files to modify:**

| File | Changes |
|------|---------|
| `src/strategy.rs` | Replace the `analyze_progress()` function (which reads `progress.db` as a text file and searches for keywords like "error", "stuck") with `analyze_metrics()`, which queries `strategy_metrics` and `iteration_outcomes` tables. The new function returns a `MetricsAnalysis` struct with: `consecutive_failures` for the current task, `overall_success_rate` (from recent iterations), `is_stuck` (from `stuck_flag`), and `difficulty_estimate`. Modify `select_cost_optimized()` to use `MetricsAnalysis` instead of text-based signals: if `consecutive_failures >= 2` → return `opus`; if `overall_success_rate > 0.8` and `consecutive_failures == 0` → return `haiku`; otherwise → return `sonnet`. Modify `select_escalate()` similarly: escalation level is now driven by `consecutive_failures` rather than `escalation_level` counter. The `ModelSelection` struct gains a new field: `rationale: String` — a human-readable explanation of why this model was chosen (used in Loop Status rendering). Remove the file-reading code from `analyze_progress()` entirely. |
| `src/run_loop.rs` | Before the Claude invocation, extend `MemoryContext` construction to also call `memory::get_loop_status(&db, &task_id, &config)` and populate the `loop_status` field. The `LoopStatus` struct needs: `current_iteration` (from `config.iteration`), `iteration_limit` (from `config.limit`), `task_attempts` (from `strategy_metrics.total_attempts`), `consecutive_failures`, `run_successes`, `run_total` (from aggregate `iteration_outcomes` query), `current_model`, `model_rationale` (from `ModelSelection`), `stuck_flag`. Pass the `ModelSelection` return value's rationale to the `LoopStatus` builder. |
| `src/claude/client.rs` | Update `build_system_prompt()` to document the `<difficulty-estimate>` sigil in the "Memory Sigils (Optional)" section. The system prompt now lists all four sigils. No changes needed to `build_task_context()` — the `render_memory_context()` function handles Loop Status rendering via the already-implemented `render_loop_status()`. |
| `src/memory/context.rs` | The `render_loop_status()` function (stubbed in Phase 2) is now fully implemented with the rendering logic from the Context Injection section. It renders iteration count, task attempt count, success rate, model choice with rationale, and conditionally the stuck-loop warning block. The stuck warning has highest priority within its section — it is always rendered if `stuck_flag` is true, even if the budget is tight (the warning text is ~200 chars). |
| `src/config.rs` | Add a `task_id` field to `Config` (or pass it through the run loop) so that `strategy::select_model()` can query `strategy_metrics` for the specific task being attempted. Currently the strategy doesn't know which task will be claimed next — this requires either passing the task_id to `select_model()` or querying metrics after task claiming and adjusting the model before invoking Claude. The latter approach is cleaner: claim the task first, then select the model based on that task's history. This may require reordering the model selection and task claiming steps in `run_loop.rs`. |

**New files to create:**

| File | Purpose |
|------|---------|
| `src/memory/status.rs` | Implements `get_loop_status()`. Runs two queries: (1) task-specific metrics from `strategy_metrics` (`SELECT total_attempts, consecutive_failures, difficulty_estimate, stuck_flag WHERE task_id = ?`), (2) run-wide metrics from `iteration_outcomes` (`SELECT COUNT(*), SUM(CASE WHEN outcome='done' THEN 1 ELSE 0 END) WHERE started_at >= datetime('now', '-2 hours')`). Returns a `LoopStatus` struct combining both query results with config data (iteration number, limit, model, rationale). Handles the case where no `strategy_metrics` row exists yet (first attempt on this task) by returning defaults: `total_attempts = 0`, `consecutive_failures = 0`, `stuck_flag = false`. |

**Run loop reordering:**

The current run loop order is: select model → claim task → invoke Claude. Phase 4 requires: claim task → select model (using task-specific metrics) → invoke Claude. This reordering is necessary because data-driven model selection needs to know which task is being attempted. The change is safe: claiming the task first just means the model selection happens slightly later in the iteration. If the Claude invocation fails or is interrupted, the task's claim is released as before (the existing `release_claim()` logic is unchanged).

```rust
// Phase 4 run loop order (simplified):
let task_id = dag::claim_task(&db, &ready_tasks[0].id, &config.agent_id)?;
let model_selection = strategy::select_model(&db, &task_id, config.model_strategy, hint.as_deref());
config.model = model_selection.model.clone();
let memory_ctx = build_memory_context(&db, &task_id, &task_info, &model_selection, &config);
let result = claude::client::run(&config, Some(&log_file))?;
```

**Testing strategy:**

1. **Unit tests for data-driven model selection** (`src/strategy.rs`): Create a database, insert a task with varying `consecutive_failures` in `strategy_metrics`, and call `select_model()` for each strategy. Verify: `CostOptimized` with 0 failures → `sonnet`, with 2 failures → `opus`, with 0 failures and high success rate → `haiku`. `Escalate` with 0 failures → `haiku`, with 1 → `sonnet`, with 2+ → `opus`. Verify hint always overrides.

2. **Unit tests for stuck detection**: Set `consecutive_failures = 3` and `stuck_flag = 1` in `strategy_metrics`. Call `get_loop_status()` and verify `stuck_flag` is true. Call `render_loop_status()` and verify the stuck warning appears in output.

3. **Integration test for full context injection** (`tests/strategic_intelligence.rs`): Create a database with a task that has 2 failed attempts (in `iteration_outcomes` and `failure_reports`), a relevant learning (in `learnings`), and `consecutive_failures = 2` (in `strategy_metrics`). Build a `MemoryContext` by calling all three retrieval functions, then render it. Verify the output contains all three sections: Previous Attempts, Learnings, and Loop Status.

4. **Regression test for model selection**: Verify that the `Fixed` and `PlanThenExecute` strategies are unaffected by the changes — they should not query `strategy_metrics` at all. Verify that `select_model()` gracefully handles a database without `strategy_metrics` rows (returns defaults).

5. **Regression test for run loop reordering**: Run `cargo run -- run --once` and verify the iteration completes successfully with the new claim-then-select order. Check the log file to confirm the model selection rationale appears.

**Verification:**

```bash
cargo test                     # All tests pass, including strategy tests
cargo run -- run --once        # Full iteration with all memory systems active
# Inspect the log file to verify:
# 1. System prompt contains all four sigil docs
# 2. Model selection rationale is data-driven
# 3. Loop Status section appears in task context (if metrics exist)
```

### Schema Migration Strategy

**Version tracking:** Ralph uses SQLite's built-in `PRAGMA user_version` for schema versioning, already implemented in `src/dag/db.rs`. The current schema version is `1`. The memory system bumps this to `2`.

**Migration mechanism:** The existing `migrate()` function in `src/dag/db.rs` handles version upgrades. It reads the current `user_version`, compares it to the target `SCHEMA_VERSION` constant, and runs migration SQL for each version gap. The v1→v2 migration adds new tables alongside existing ones — it never modifies or drops existing tables.

**Migration SQL (v1 → v2):**

```rust
const SCHEMA_VERSION: i32 = 2;

fn migrate(conn: &Connection, from_version: i32, to_version: i32) -> Result<()> {
    // ... existing v0→v1 migration ...

    if from_version < 2 && to_version >= 2 {
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS iteration_outcomes (
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                attempt_number INTEGER NOT NULL,
                model TEXT NOT NULL,
                started_at TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                tokens_input INTEGER,
                tokens_output INTEGER,
                outcome TEXT NOT NULL
                    CHECK (outcome IN ('done','failed','no_sigil','error')),
                error_type TEXT,
                PRIMARY KEY (task_id, attempt_number)
            );
            CREATE INDEX IF NOT EXISTS idx_iteration_outcomes_task_id
                ON iteration_outcomes(task_id);
            CREATE INDEX IF NOT EXISTS idx_iteration_outcomes_started_at
                ON iteration_outcomes(started_at);
            CREATE INDEX IF NOT EXISTS idx_iteration_outcomes_model_outcome
                ON iteration_outcomes(model, outcome);

            CREATE TABLE IF NOT EXISTS failure_reports (
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                attempt_number INTEGER NOT NULL,
                what_was_tried TEXT NOT NULL,
                why_it_failed TEXT NOT NULL,
                error_category TEXT,
                relevant_files TEXT,
                stack_trace_snippet TEXT,
                retry_suggestion TEXT,
                created_at TEXT NOT NULL,
                PRIMARY KEY (task_id, attempt_number),
                FOREIGN KEY (task_id, attempt_number)
                    REFERENCES iteration_outcomes(task_id, attempt_number)
                    ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_failure_reports_task_id
                ON failure_reports(task_id);
            CREATE INDEX IF NOT EXISTS idx_failure_reports_error_category
                ON failure_reports(error_category);

            CREATE TABLE IF NOT EXISTS learnings (
                id TEXT PRIMARY KEY,
                task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
                category TEXT NOT NULL,
                content TEXT NOT NULL,
                relevance_tags TEXT NOT NULL,
                created_at TEXT NOT NULL,
                pruned_at TEXT,
                superseded_by TEXT REFERENCES learnings(id) ON DELETE SET NULL,
                last_used_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_learnings_task_id ON learnings(task_id);
            CREATE INDEX IF NOT EXISTS idx_learnings_created_at ON learnings(created_at);
            CREATE INDEX IF NOT EXISTS idx_learnings_pruned_at ON learnings(pruned_at);
            CREATE INDEX IF NOT EXISTS idx_learnings_category ON learnings(category);

            CREATE TABLE IF NOT EXISTS strategy_metrics (
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                total_attempts INTEGER NOT NULL DEFAULT 0,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                last_attempt_at TEXT,
                last_success_at TEXT,
                difficulty_estimate TEXT,
                suggested_model TEXT,
                stuck_flag INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (task_id)
            );
            CREATE INDEX IF NOT EXISTS idx_strategy_metrics_stuck_flag
                ON strategy_metrics(stuck_flag);
            CREATE INDEX IF NOT EXISTS idx_strategy_metrics_consecutive_failures
                ON strategy_metrics(consecutive_failures);
        "#).context("Failed to create schema v2 (memory tables)")?;

        conn.pragma_update(None, "user_version", 2)?;
    }

    Ok(())
}
```

**Key design decisions:**

1. **`CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS`:** Defensive coding against partial migrations. If the process crashes mid-migration (e.g., after creating `iteration_outcomes` but before `learnings`), the next `init_db()` call re-runs the migration and the `IF NOT EXISTS` clauses prevent "table already exists" errors.

2. **Single `execute_batch()` call:** All v2 tables are created in one batch to minimize transaction overhead. SQLite wraps `execute_batch()` in an implicit transaction, so either all tables are created or none are (atomicity).

3. **No modification of existing tables:** The v1 tables (`tasks`, `dependencies`, `task_logs`) are never altered. This guarantees that existing DAG operations work identically after migration. The only visible change is the `user_version` pragma incrementing from 1 to 2.

4. **Foreign key references to `tasks.id`:** All new tables reference the existing `tasks` table, maintaining referential integrity. Cascading deletes ensure that cleaning up a task also cleans up its memory data.

5. **Auto-migration on open:** The `init_db()` function (called at the start of every `ralph run`) checks the current schema version and calls `migrate()` if needed. No explicit "ralph migrate" command is required — users simply run Ralph and their database is upgraded transparently.

**Backward compatibility guarantees:**

- **Old Ralph, new database:** If a user downgrades Ralph to a pre-memory version after upgrading, the old Ralph will see `user_version = 2` but only knows about version 1. It should fail gracefully with a "database version too new" error rather than silently corrupting data. This is enforced by adding a check in `init_db()`: if `current_version > SCHEMA_VERSION`, return an error instructing the user to upgrade Ralph.

- **New Ralph, old database:** The new Ralph sees `user_version = 1` and runs the v1→v2 migration automatically. All existing tasks, dependencies, and logs are preserved. The new tables are empty until the first iteration with the memory system active.

- **Fresh database:** `ralph init` creates a new `progress.db` with version 2 (running both v0→v1 and v1→v2 migrations), so fresh installations get the full schema immediately.

**Future migrations (v2 → v3+):**

The same pattern extends to future schema versions. Each migration is a guarded block (`if from_version < N && to_version >= N`) that runs DDL statements. Migrations are cumulative: upgrading from v1 to v3 runs both v1→v2 and v2→v3 blocks in sequence. This approach scales to any number of schema versions without accumulating migration files (unlike frameworks like Ecto or ActiveRecord that use separate migration files).

**Rollback strategy:**

SQLite does not support `DROP COLUMN` (before version 3.35.0), so rollback from v2→v1 is impractical. Instead, if a rollback is needed, the user can delete `.ralph/progress.db` and re-run `ralph init`. Since the memory tables are auxiliary (they don't affect task DAG operations), deleting and recreating the database only loses memory data, not task definitions. Task definitions can be repopulated by re-running `ralph plan`.

**Testing the migration path:**

1. **Fresh install test:** Run `ralph init` on a directory with no `.ralph/`. Verify `progress.db` has `user_version = 2` and all seven tables exist.

2. **Upgrade test:** Create a v1 `progress.db` manually (using the v1 schema SQL), insert some tasks and dependencies, then run `ralph init` or `ralph run`. Verify `user_version = 2`, all new tables exist, and existing tasks/dependencies are intact.

3. **Idempotency test:** Run `ralph init` twice on the same directory. Verify no errors and `user_version` is still 2.

4. **Downgrade protection test:** Set `user_version = 3` on a database, then run Ralph (which knows about version 2). Verify Ralph exits with an error message rather than attempting to operate on a newer schema.

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

**Storage:** Parsed fields are inserted into the `failure_reports` table. The `relevant_files` Vec is serialized to JSON.

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

**Storage:** Each parsed learning gets a unique ID (`l-{6 hex}`) and is inserted into the `learnings` table. The `tags` Vec is serialized to JSON in the `relevance_tags` column.

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

**Storage:** Stored in `strategy_metrics.difficulty_estimate` column. Updated on each iteration; last estimate wins.

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

**Storage:** Stored in `failure_reports` table (new column: `retry_suggestion TEXT`). This is part of the failure report for a specific attempt.

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

Context injection is the consumer side of iteration memory: it takes stored data (failure reports, learnings, metrics) and renders it into the system prompt that Claude sees each iteration. All injection happens in `claude::client::build_task_context()`, which already builds the "Assigned Task" section with task details, parent context, and completed prerequisites. Memory context is appended after the existing sections, gated by data availability — if no memory data exists for a task, no memory sections appear and the prompt is identical to today's.

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

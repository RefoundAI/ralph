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

### Error Recovery Memory

<!-- iteration_outcomes table, failure_reports table: CREATE TABLE statements, column rationale, indexes -->

### Self-Improvement / Learning Extraction

<!-- learnings table: CREATE TABLE statements, column rationale, indexes, relevance tags design -->

### Strategic Intelligence

<!-- strategy_metrics table: CREATE TABLE statements, column rationale, indexes -->

### Schema Relationships

<!-- Foreign key relationships to existing tasks table, ER diagram -->

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

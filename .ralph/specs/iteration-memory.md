# Iteration Memory

Ralph currently treats each iteration as stateless: a fresh Claude Code session receives a task assignment but has no memory of what happened in previous iterations. This design is simple but leads to repeated failures, wasted compute on known-bad approaches, and inability to learn from experience within a run.

This spec designs three memory systems that give Ralph cross-iteration awareness while maintaining the existing synchronous, single-agent, SQLite-backed architecture.

**Three systems:**

1. **Error Recovery Memory** -- Structured failure reports that prevent repeating the same mistakes when retrying tasks
2. **Self-Improvement / Learning Extraction** -- Reusable insights captured from successful and failed iterations, injected as context into future tasks
3. **Strategic Intelligence** -- Data-driven model selection, difficulty estimation, and stuck-loop detection

## Architecture Overview

### Error Recovery Memory

<!-- How error recovery hooks into the run loop, where new modules live, data flow for failure capture and injection -->

### Self-Improvement / Learning Extraction

<!-- How learnings are captured during iteration, stored, matched for relevance, and injected into future tasks -->

### Strategic Intelligence

<!-- How iteration metrics feed into model selection, difficulty estimation, and stuck-loop detection -->

### Integration Points

<!-- Diagram/description of how the three systems interact: error patterns feeding strategy, learnings informing retry context, strategic data guiding model selection -->

### Module Layout

<!-- Where new code lives in the existing src/ tree, which existing modules are modified -->

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

---
title: "Interactive Flow Sigils (phase-complete, tasks-created)"
tags: [sigils, interactive, feature-create, task-create]
created_at: "2026-02-26T02:20:48.574958+00:00"
---

Two sigils control interactive session auto-exit:

- `<phase-complete>spec|plan|build</phase-complete>` — emitted by the agent after writing a spec/plan document or completing task DAG creation. Valid phases: `spec`, `plan`, `build`.
- `<tasks-created>` (or `<tasks-created/>`) — emitted after `ralph task add` in task create sessions.

**Parsing:** `parse_phase_complete()` and `parse_tasks_created()` in `src/acp/sigils.rs`. Combined via `extract_interactive_sigils()` → `InteractiveSigils` struct.

**Auto-exit:** `run_interactive()` in `src/acp/interactive.rs` checks `extract_interactive_sigils()` on accumulated text after each agent response. When a sigil is detected, the loop breaks and returns accumulated text.

**Return types:** Both `run_interactive()` and `run_streaming()` return `Result<String>` (accumulated agent text) for sigil extraction by callers.

**System prompts:** `src/feature_prompts.rs` instructs agents to emit the appropriate sigil at completion of each phase.

**Non-draining peek:** `RalphClient::peek_accumulated_text()` reads the accumulator without clearing it, enabling mid-session sigil checks.

See also: [[Execution Modes]], [[Feature Lifecycle]], [[Sigil Parsing]]

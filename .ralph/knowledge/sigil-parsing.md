---
title: "Sigil parsing from agent output"
tags: [sigils, parsing, acp, output]
created_at: "2026-02-18T00:00:00Z"
---

Ralph communicates with agents via sigils — XML-like tags embedded in the agent's text output. Parsing happens in `src/acp/sigils.rs` via `extract_sigils()`.

Sigils and their effects:
- `<task-done>{task_id}</task-done>` — Mark task done, trigger auto-transitions
- `<task-failed>{task_id}</task-failed>` — Mark task failed, trigger auto-fail parent
- `<promise>COMPLETE</promise>` — All tasks done, exit loop with code 0
- `<promise>FAILURE</promise>` — Critical failure, short-circuits BEFORE DAG update, exit code 1
- `<next-model>opus|sonnet|haiku</next-model>` — Override model strategy for next iteration only
- `<verify-pass/>` — Verification agent: task passed
- `<verify-fail>reason</verify-fail>` — Verification agent: task failed
- `<journal>notes</journal>` — Iteration notes written to journal table
- `<knowledge tags="..." title="...">body</knowledge>` — Creates/updates knowledge entry file

Parsing is string-based (indexOf + substring), not XML parsing. Sigils must be exact — no extra whitespace inside tags. The `<knowledge>` sigil attributes can appear in any order (tags before title or vice versa).

The `extract_sigils()` function returns a `SigilResult` struct with fields: `task_done`, `task_failed`, `promise`, `next_model`, `journal_notes`, `knowledge_entries`.

Important: FAILURE promise short-circuits the loop before any DAG state update. No task is marked done or failed.

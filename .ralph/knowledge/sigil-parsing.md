---
title: Sigil Parsing
tags: [sigils, parsing, acp, output, xml]
created_at: "2026-02-18T00:00:00Z"
---

Ralph communicates with agents via XML-like sigils in text output. Parsing in `src/acp/sigils.rs` via `extract_sigils()`.

## Sigils

| Sigil | Effect |
|-------|--------|
| `<task-done>{id}</task-done>` | Mark task done, trigger [[Auto-Transitions]] |
| `<task-failed>{id}</task-failed>` | Mark task failed, trigger auto-fail parent |
| `<promise>COMPLETE</promise>` | All tasks done, exit 0 |
| `<promise>FAILURE</promise>` | Critical failure, short-circuits before DAG update, exit 1 |
| `<next-model>opus\|sonnet\|haiku</next-model>` | Override [[Model Strategy Selection]] for next iteration |
| `<verify-pass/>` | [[Verification Agent]]: passed |
| `<verify-fail>reason</verify-fail>` | [[Verification Agent]]: failed |
| `<journal>notes</journal>` | Write to [[Journal System]] |
| `<knowledge tags="..." title="...">body</knowledge>` | Write to [[Knowledge System]] |
| `<phase-complete>spec\|plan\|build</phase-complete>` | Auto-exit interactive session (see [[Interactive Flow Sigils (phase-complete, tasks-created)]]) |
| `<tasks-created>` / `<tasks-created/>` | Signal DAG populated (see [[Interactive Flow Sigils (phase-complete, tasks-created)]]) |

## Implementation

String-based parsing (indexOf + substring), not XML. Whitespace trimmed inside tags. `<knowledge>` attributes can appear in any order. First `<next-model>` wins if duplicated.

## FAILURE Short-Circuit

`<promise>FAILURE</promise>` exits *before* any DAG state update. No task is marked done or failed. See [[Run Loop Lifecycle]] step 9.

See also: [[Run Loop Lifecycle]], [[Auto-Transitions]], [[Model Strategy Selection]], [[Verification Agent]], [[Journal System]], [[Knowledge System]], [[Interactive Flow Sigils (phase-complete, tasks-created)]]

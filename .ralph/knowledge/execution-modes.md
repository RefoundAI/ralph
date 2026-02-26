---
title: Execution Modes
tags: [acp, interactive, streaming, connection, run-loop, feature]
created_at: "2026-02-23T06:37:00Z"
---

Ralph spawns ACP agent sessions in three distinct modes. All live under `src/acp/`.

## Interactive (`run_interactive`)

**Source:** `src/acp/interactive.rs`

Ralph mediates between user (stdin) and ACP agent via a prompt/response cycle. User types plain text; Ralph sends it as a `PromptRequest`; agent's streaming response is rendered to terminal.

**Signature:** `run_interactive(agent_command, instructions, initial_message, project_root, model)`

**ACP lifecycle:** Spawn agent → initialize → new_session → send first prompt (instructions + initial_message concatenated as single `TextContent` block) → render response → read next user input → send as new prompt → repeat until empty line or EOF.

**Used by:** `feature create` (spec phase, plan phase), `task create`

When TUI is active, user input comes from UI modals instead of raw stdin. Semantics are unchanged.

**Resume support:** Both spec and plan phases detect existing output files. If `spec.md`/`plan.md` exists, content is loaded (truncated to 10K chars) and appended to context; initial message changes to "Resume the interview..."

## Streaming (`run_streaming`)

**Source:** `src/acp/interactive.rs`

Single autonomous prompt. Agent runs to completion without user input. Ralph renders streaming response in real time.

**Signature:** `run_streaming(agent_command, instructions, message, project_root, model)`

Key differences from interactive: single prompt only, no stdin reading, `request_permission()` auto-approves all tool requests, session ends on `PromptResponse`.

**Used by:** `feature create` (build phase only)

The agent creates the task DAG by executing `ralph task add` and `ralph task deps add` via `terminal/create_terminal` — task creation goes through the same validation as manual CLI usage. See [[Feature Lifecycle]].

## Loop Iteration (`run_iteration`)

**Source:** `src/acp/connection.rs`

Main loop mode. One agent process per iteration. Builds rich prompt from `IterationContext`. Returns `RunResult` for loop to act on. See [[Run Loop Lifecycle]] and [[ACP Connection Lifecycle]] for details.

**Used by:** `ralph run`

Also: `run_autonomous()` in the same file handles [[Verification Agent]] (read-only) and review sessions.

## Comparison

| Aspect | Interactive | Streaming | Loop Iteration |
|---|---|---|---|
| User input | Yes (stdin) | No | No |
| Sigil parsing | Auto-exit only ([[Interactive Flow Sigils (phase-complete, tasks-created)]]) | Auto-exit only | Yes (full) |
| Log file | No | No | Yes (JSON-RPC tee) |
| Read-only mode | No | No | Verification only |
| Returns | `Result<String>` | `Result<String>` | `Result<RunResult>` |

## Context Assembly

**Interactive sessions:** `gather_project_context()` in `main.rs` — CLAUDE.md (truncated 10K), `.ralph.toml`, features list table, standalone tasks (optional, only for `task create`).

**Loop iterations:** `build_iteration_context()` in `run_loop.rs` — task info, parent, blockers, spec/plan, retry, journal, knowledge. See [[System Prompt Construction]].

See also: [[ACP Connection Lifecycle]], [[Feature Lifecycle]], [[Run Loop Lifecycle]], [[UI Interactive Modals and Explorer Views]], [[Ratatui UI Runtime]]

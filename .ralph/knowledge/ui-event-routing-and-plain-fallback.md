---
title: UI Event Routing and Plain Fallback
tags: [ui, output, formatter, acp, streaming, fallback]
created_at: "2026-02-24T08:03:34Z"
---

Output now flows through a dual-mode rendering path.

## Formatter Contract

`src/output/formatter.rs` is the presentation boundary:

- If UI is active: emit `UiEvent` / log lines into the TUI channel.
- If UI is inactive: keep plain stdout/stderr behavior.

Run-loop messaging should go through formatter helpers (not direct `println!`/`eprintln!`) for consistency.

## ACP Stream Routing

`src/acp/streaming.rs` keeps existing parsing/formatting helpers but short-circuits rendering when UI is active:

- `AgentText` -> `UiEvent::AgentText`
- tool summaries/details -> `UiEvent::ToolActivity`
- tool errors -> `UiEvent::Log { level: Error, ... }`

When UI is not active, old terminal formatting remains.

## Connection Layer

`src/acp/connection.rs` now reports connection/status warnings through formatter, so warnings land in either dashboard logs (UI mode) or stderr (plain mode).

## Behavior Safety

- Sigil extraction and stop-reason handling are unchanged.
- UI changes are presentation-only for run-loop semantics.

See also: [[Run Loop Lifecycle]], [[Error Handling and Resilience]], [[Ratatui UI Runtime]], [[Execution Modes]]

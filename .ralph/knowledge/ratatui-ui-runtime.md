---
title: Ratatui UI Runtime
tags: [ui, ratatui, crossterm, tty, cli, runtime]
created_at: "2026-02-24T08:03:34Z"
---

Ralph now has a global TUI runtime in `src/ui/` powered by `ratatui` + `crossterm`.

## Activation Rules

- UI mode is resolved by `UiMode::resolve()` in `src/ui/mod.rs`:
  - `--no-ui` forces `Off`
  - `RALPH_UI=1|true|on` forces `On`
  - `RALPH_UI=0|false|off` forces `Off`
  - anything else is `Auto`
- `Auto`/`On` only enable UI when both stdout and stderr are TTYs.
- Non-TTY contexts auto-fallback to plain rendering.

## Runtime Lifecycle

- `ui::start()` spawns one UI thread and keeps a global session in `OnceLock<Mutex<Option<UiSession>>>`.
- Commands/events are sent over `std::sync::mpsc`.
- `UiGuard` is RAII and always calls `ui::stop()` on drop.
- App teardown restores terminal state (raw mode off, leave alternate screen, show cursor).

## Screens and Interactions

- **Dashboard:** run status + logs + agent stream + tool activity.
- **Explorer:** read-only full-screen list/detail view with keyboard scroll.
- **Modal:** multiline input + confirm dialogs for interactive flows.

## Wiring

- `ralph run` starts UI at command entry and tears down at outcome.
- `feature create`, `task create`, and non-JSON browse flows (`feature list`, `task list/show/tree/deps list`) start UI where needed for modals/explorer.

See also: [[Execution Modes]], [[Configuration Layers]], [[UI Event Routing and Plain Fallback]], [[UI Interactive Modals and Explorer Views]], [[TUI Input Model (v0.8.1+)]], [[Themeable TUI Colour Scheme]]

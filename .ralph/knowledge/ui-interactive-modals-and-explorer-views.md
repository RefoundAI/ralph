---
title: UI Interactive Modals and Explorer Views
tags: [ui, interactive, modal, explorer, feature, task, interrupt]
created_at: "2026-02-24T08:03:34Z"
---

Interactive and browse UX now has TUI-native paths.

## Interactive Input

`src/acp/interactive.rs`:

- Uses `ui::prompt_multiline()` when UI is active.
- Falls back to existing stdin multiline behavior when UI is inactive.
- Preserves first-prompt construction (`instructions + --- + initial_message`) and ACP lifecycle semantics.

## Interrupt Flow

`src/interrupt.rs`:

- `prompt_for_feedback()` uses UI multiline modal in UI mode.
- `should_continue()` uses UI confirm modal in UI mode.
- Plain TTY prompt behavior remains for non-UI runs.

## Explorer Views

Non-JSON command outputs can render as full-screen explorer:

- `feature list`
- `task list`
- `task show`
- `task tree`
- `task deps list`
- task DAG summary at end of `feature create`

Explorer keys:

- `Up/Down` (or `k/j`) scroll
- `PageUp/PageDown` faster scroll
- `q`, `Esc`, or `Enter` close

## Mutation Command UX Pattern

Destructive task/feature commands use a consistent confirmation pattern in `src/main.rs`:

- `confirm_if_ui_active(title, prompt, default_yes)` — shows confirm modal when TUI is active, returns `true` in non-UI/non-TTY mode
- `show_result_if_ui_active(title, lines)` — shows result panel in TUI, falls back to plain print

Commands using confirm modals: `task delete`, `task done`, `task fail`, `task reset`, `feature delete`

Commands with result display only (no confirm): `task update`, `task log -m`, `task deps add/rm`

All destructive commands support `--yes`/`-y` to bypass confirmation (preserves scriptability).

## Auth Path

`ralph auth` delegates to `claude auth login`. When TUI is active, UI is stopped before spawning the external auth CLI, then terminal state is restored. This prevents alternate-screen conflicts with the interactive auth process.

See also: [[Feature Lifecycle]], [[Execution Modes]], [[Interrupt Handling]], [[Ratatui UI Runtime]], [[Feature Delete Command]], [[TUI Input Model (v0.8.1+)]]

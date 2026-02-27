---
title: "Event Emission System"
tags: [events, output, formatter, ui, tui, stderr, run-loop]
created_at: "2026-02-27T00:00:00Z"
---

Structured event emissions for orchestration observability, added across the run loop, DAG transitions, verification, review, journal, and knowledge subsystems.

## Architecture

`emit_event(category, message, is_error)` and `emit_event_info(category, message)` in `src/output/formatter.rs` provide dual-mode output:

- **UI active:** emits `UiEvent::Event(EventLine { category, message, timestamp, is_error })` to the TUI Events panel
- **UI inactive:** emits timestamped, category-colored lines to **stderr** (not stdout)

## EventLine Struct

Defined in `src/ui/event.rs`:

```rust
pub struct EventLine {
    pub category: String,   // e.g. "task", "iter", "dag"
    pub message: String,    // pre-formatted with template variables substituted
    pub timestamp: String,  // "HH:MM:SS" local time
    pub is_error: bool,     // render in error style (red) when true
}
```

## Event Categories

| Category | Emitted From | Examples |
|----------|-------------|----------|
| `task` | run_loop | claimed, done, failed, released, incomplete (no sigil) |
| `iter` | run_loop | iteration start, model selection, limit reached |
| `feature` | run_loop, transitions | feature done/failed (via [[Auto-Transitions]]) |
| `verify` | run_loop | verification pass/fail |
| `review` | review.rs | review pass/fail |
| `journal` | run_loop | journal entry written, write failures |
| `knowledge` | run_loop | knowledge entry written/merged, write failures |
| `interrupt` | run_loop | interrupt detected, user chose to halt/continue |
| `dag` | run_loop, transitions | DAG summary, task unblocked, parent completed/failed |
| `config` | run_loop | strategy selection, model override |

## TUI Integration

Events panel in the Dashboard left column (below DAG summary). State managed in `AppState`:
- `events: VecDeque<EventLine>` (max 200 entries, oldest dropped)
- `events_scroll: Option<usize>` — `None` = auto-scroll to bottom, `Some(n)` = pinned scroll position
- Scroll methods: `events_scroll_up()`, `events_scroll_down()`, `events_scroll_to_bottom()`

## Theme Tokens

10 per-category color tokens in `Theme` struct: `event_task_fg` through `event_config_fg`. Each has a `[ui.colors]` override (e.g. `event_task = "#ff0000"`). `event_category_style(category)` maps category string to the correct `Style`.

## Plain Mode Colors

`color_category_plain()` in formatter maps categories to `colored` crate colors for terminal stderr output.

## AutoTransition Events

`set_task_status()` returns `Vec<AutoTransition>` — callers iterate and emit events for each: unblocked tasks, parent completions/failures, feature done/failed. See [[Auto-Transitions]].

## Non-TTY Smoke Testing

`tests/smoke/non_tty_smoke.sh` verifies that events are emitted on stderr (not stdout) when piped, ensuring scriptability.

See also: [[UI Event Routing and Plain Fallback]], [[Ratatui UI Runtime]], [[Auto-Transitions]], [[Run Loop Lifecycle]], [[Custom Color Palettes in .ralph.toml]]

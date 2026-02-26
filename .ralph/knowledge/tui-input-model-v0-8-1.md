---
title: "TUI Input Model (v0.8.1+)"
tags: [ui, input, cursor, keybindings]
created_at: "2026-02-26T01:35:58.590737+00:00"
---

The TUI multiline input uses a unified `input_text: String` + `input_cursor: usize` model in `AppState` (src/ui/state.rs).

## Key bindings (free-text mode)
- **Enter**: submits input (empty text → Exit signal)
- **Shift+Enter**: inserts newline character
- **←/→**: move cursor by one character
- **↑/↓**: move cursor between lines, preserving column
- **Home/End**: move to start/end of current line
- **Backspace/Delete**: delete before/after cursor
- **PageUp/PageDown**: scroll agent stream (not input)
- **Ctrl+C**: interrupt, **Esc**: exit

## Rendering
- `render_input_pane()` in `view.rs` walks logical lines, breaks at `inner_w` for wrapping
- Block cursor shown as inverted (black on white) `Span` at cursor position
- Terminal cursor positioned via `set_cursor_position()` for blinking
- `count_wrapped_lines()` helper estimates visual line count for dynamic height
- Input pane height capped at 50% of right column

## Helpers
- `input_cursor_up()` / `input_cursor_down()` in `app.rs` handle vertical cursor movement
- Both preserve column position when moving between lines of different lengths

See also: [[Ratatui UI Runtime]], [[UI Interactive Modals and Explorer Views]]

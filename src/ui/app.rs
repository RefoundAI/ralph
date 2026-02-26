//! UI runtime loop backed by ratatui + crossterm.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::ui::state::{AppState, FrameAreas};
use crate::ui::view;
use crate::ui::{UiCommand, UiPromptResult};

const DRAW_INTERVAL: Duration = Duration::from_millis(50);

enum Interaction {
    None,
    Multiline {
        reply: Sender<UiPromptResult>,
    },
    Confirm {
        reply: Sender<bool>,
        default_yes: bool,
    },
    Explorer {
        reply: Sender<()>,
    },
}

/// Execute the UI loop until a shutdown command is received.
pub(super) fn run(rx: Receiver<UiCommand>) -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide)?;
    enable_raw_mode()?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut state = AppState::default();
    let mut interaction = Interaction::None;
    let mut areas = FrameAreas::default();
    let mut should_exit = false;

    while !should_exit {
        match rx.recv_timeout(DRAW_INTERVAL) {
            Ok(cmd) => {
                should_exit = apply_command(&mut state, &mut interaction, cmd);
                while let Ok(next) = rx.try_recv() {
                    should_exit = should_exit || apply_command(&mut state, &mut interaction, next);
                    if should_exit {
                        break;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        handle_terminal_events(&mut state, &mut interaction, &areas);

        // Show terminal cursor when input pane is active in free-text mode,
        // hide it otherwise so it doesn't flicker over the dashboard.
        if state.input_active && state.input_choices.is_none() {
            let _ = execute!(terminal.backend_mut(), Show);
        } else {
            let _ = execute!(terminal.backend_mut(), Hide);
        }

        terminal.draw(|frame| view::render(frame, &state, &mut areas))?;
    }

    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    Ok(())
}

fn apply_command(state: &mut AppState, interaction: &mut Interaction, cmd: UiCommand) -> bool {
    match cmd {
        UiCommand::Event(evt) => {
            state.apply(evt);
            false
        }
        UiCommand::PromptMultiline {
            title,
            hint,
            choices,
            reply,
        } => {
            // Defensive: if a Multiline is already active, deactivate first.
            // The old reply channel is dropped, causing recv() Err on the caller.
            if matches!(interaction, Interaction::Multiline { .. }) {
                state.deactivate_input();
            }
            state.activate_input(title, hint, choices);
            *interaction = Interaction::Multiline { reply };
            false
        }
        UiCommand::Confirm {
            title,
            prompt,
            default_yes,
            reply,
        } => {
            // Defensive: if a Multiline is active, deactivate input first.
            // The old reply channel is dropped, causing recv() Err on the caller.
            if matches!(interaction, Interaction::Multiline { .. }) {
                state.deactivate_input();
            }
            state.modal = Some(crate::ui::state::UiModal::Confirm {
                title,
                prompt,
                default_yes,
            });
            *interaction = Interaction::Confirm { reply, default_yes };
            false
        }
        UiCommand::ShowExplorer {
            title,
            lines,
            reply,
        } => {
            state.show_explorer(title, lines);
            state.modal = None;
            *interaction = Interaction::Explorer { reply };
            false
        }
        UiCommand::Shutdown => true,
    }
}

fn handle_terminal_events(state: &mut AppState, interaction: &mut Interaction, areas: &FrameAreas) {
    // Drain ALL available events before returning, so paste, held-key
    // repeats, and scroll gestures are batched into a single redraw cycle.
    loop {
        let Ok(has_event) = event::poll(Duration::from_millis(0)) else {
            return;
        };
        if !has_event {
            return;
        }
        let Ok(ev) = event::read() else {
            return;
        };
        match ev {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                process_key(state, interaction, key);
            }
            Event::Mouse(mouse) => {
                process_mouse(state, interaction, mouse, areas);
            }
            _ => continue,
        }
    }
}

fn process_key(
    state: &mut AppState,
    interaction: &mut Interaction,
    key: crossterm::event::KeyEvent,
) {
    match interaction {
        Interaction::Multiline { .. } if state.input_choices.is_some() => {
            // Choice mode key handling.
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let old = std::mem::replace(interaction, Interaction::None);
                    if let Interaction::Multiline { reply } = old {
                        let _ = reply.send(UiPromptResult::Interrupted);
                    }
                    state.deactivate_input();
                }
                KeyCode::Esc => {
                    let old = std::mem::replace(interaction, Interaction::None);
                    if let Interaction::Multiline { reply } = old {
                        let _ = reply.send(UiPromptResult::Exit);
                    }
                    state.deactivate_input();
                }
                KeyCode::Up => {
                    state.input_choice_cursor = state.input_choice_cursor.saturating_sub(1);
                }
                KeyCode::Down => {
                    if let Some(choices) = &state.input_choices {
                        let max = choices.len().saturating_sub(1);
                        if state.input_choice_cursor < max {
                            state.input_choice_cursor += 1;
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(choices) = &state.input_choices {
                        let selected = choices[state.input_choice_cursor].clone();
                        let old = std::mem::replace(interaction, Interaction::None);
                        if let Interaction::Multiline { reply } = old {
                            let _ = reply.send(UiPromptResult::Input(selected));
                        }
                        state.deactivate_input();
                    }
                }
                KeyCode::Char(ch @ '1'..='9') => {
                    let digit = (ch as usize) - ('0' as usize);
                    if let Some(choices) = &state.input_choices {
                        if digit <= choices.len() {
                            let selected = choices[digit - 1].clone();
                            let old = std::mem::replace(interaction, Interaction::None);
                            if let Interaction::Multiline { reply } = old {
                                let _ = reply.send(UiPromptResult::Input(selected));
                            }
                            state.deactivate_input();
                        }
                        // Out of range digit: ignore
                    }
                }
                KeyCode::PageUp => {
                    state.agent_scroll_up(20);
                }
                KeyCode::PageDown => {
                    let line_count = state.agent_text.lines().count();
                    state.agent_scroll_down(20, line_count);
                }
                KeyCode::End => {
                    state.agent_scroll_to_bottom();
                }
                KeyCode::Backspace => {
                    // No-op in choice mode
                }
                KeyCode::Char(ch) => {
                    // Any other character switches to free-text mode
                    state.input_choices = None;
                    state.input_text.push(ch);
                    state.input_cursor = state.input_text.len();
                }
                _ => {}
            }
        }
        Interaction::Multiline { .. } => match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let old = std::mem::replace(interaction, Interaction::None);
                if let Interaction::Multiline { reply } = old {
                    let _ = reply.send(UiPromptResult::Interrupted);
                }
                state.deactivate_input();
            }
            KeyCode::Esc => {
                let old = std::mem::replace(interaction, Interaction::None);
                if let Interaction::Multiline { reply } = old {
                    let _ = reply.send(UiPromptResult::Exit);
                }
                state.deactivate_input();
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                // Shift+Enter inserts a newline.
                state.input_text.insert(state.input_cursor, '\n');
                state.input_cursor += 1;
            }
            KeyCode::Enter => {
                // Enter submits the input.
                let old = std::mem::replace(interaction, Interaction::None);
                if let Interaction::Multiline { reply } = old {
                    let text = state.input_text.trim_end().to_string();
                    if text.is_empty() {
                        let _ = reply.send(UiPromptResult::Exit);
                    } else {
                        let _ = reply.send(UiPromptResult::Input(text));
                    }
                }
                state.deactivate_input();
            }
            KeyCode::Backspace => {
                if state.input_cursor > 0 {
                    // Find the previous char boundary.
                    let prev = state.input_text[..state.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    state.input_text.drain(prev..state.input_cursor);
                    state.input_cursor = prev;
                }
            }
            KeyCode::Delete => {
                if state.input_cursor < state.input_text.len() {
                    let next = state.input_cursor
                        + state.input_text[state.input_cursor..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(0);
                    state.input_text.drain(state.input_cursor..next);
                }
            }
            KeyCode::Left => {
                if state.input_cursor > 0 {
                    state.input_cursor = state.input_text[..state.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if state.input_cursor < state.input_text.len() {
                    state.input_cursor += state.input_text[state.input_cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                }
            }
            KeyCode::Up => {
                // Move cursor to the same column on the previous line.
                input_cursor_up(state);
            }
            KeyCode::Down => {
                // Move cursor to the same column on the next line.
                input_cursor_down(state);
            }
            KeyCode::Home => {
                // Move cursor to start of current line.
                let before = &state.input_text[..state.input_cursor];
                state.input_cursor = match before.rfind('\n') {
                    Some(pos) => pos + 1,
                    None => 0,
                };
            }
            KeyCode::End => {
                // Move cursor to end of current line.
                let after = &state.input_text[state.input_cursor..];
                state.input_cursor += match after.find('\n') {
                    Some(pos) => pos,
                    None => after.len(),
                };
            }
            KeyCode::Char(ch) => {
                state.input_text.insert(state.input_cursor, ch);
                state.input_cursor += ch.len_utf8();
            }
            KeyCode::PageUp => {
                state.agent_scroll_up(20);
            }
            KeyCode::PageDown => {
                let line_count = state.agent_text.lines().count();
                state.agent_scroll_down(20, line_count);
            }
            _ => {}
        },
        Interaction::Confirm { reply, default_yes } => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let _ = reply.send(true);
                *interaction = Interaction::None;
                state.modal = None;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                let _ = reply.send(false);
                *interaction = Interaction::None;
                state.modal = None;
            }
            KeyCode::Enter => {
                let _ = reply.send(*default_yes);
                *interaction = Interaction::None;
                state.modal = None;
            }
            _ => {}
        },
        Interaction::Explorer { reply } => match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                state.hide_explorer();
                let _ = reply.send(());
                *interaction = Interaction::None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.explorer_scroll_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.explorer_scroll_down();
            }
            KeyCode::PageUp => {
                for _ in 0..10 {
                    state.explorer_scroll_up();
                }
            }
            KeyCode::PageDown => {
                for _ in 0..10 {
                    state.explorer_scroll_down();
                }
            }
            _ => {}
        },
        Interaction::None => {
            // Dashboard mode: arrow keys scroll the Agent Stream panel.
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::interrupt::request_interrupt();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    state.agent_scroll_up(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    // Use a large max so scroll_down clamps properly;
                    // exact max is computed at render time but we approximate here.
                    let line_count = state.agent_text.lines().count();
                    state.agent_scroll_down(1, line_count);
                }
                KeyCode::PageUp => {
                    state.agent_scroll_up(20);
                }
                KeyCode::PageDown => {
                    let line_count = state.agent_text.lines().count();
                    state.agent_scroll_down(20, line_count);
                }
                KeyCode::End => {
                    state.agent_scroll_to_bottom();
                }
                _ => {}
            }
        }
    }
}

fn process_mouse(
    state: &mut AppState,
    _interaction: &mut Interaction,
    mouse: crossterm::event::MouseEvent,
    areas: &FrameAreas,
) {
    let scroll_lines: i32 = match mouse.kind {
        MouseEventKind::ScrollUp => -3,
        MouseEventKind::ScrollDown => 3,
        _ => return,
    };

    let col = mouse.column;
    let row = mouse.row;

    // Determine which frame the cursor is over.
    let in_rect = |r: &ratatui::layout::Rect| -> bool {
        col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
    };

    if let Some(ref r) = areas.agent {
        if in_rect(r) {
            if scroll_lines < 0 {
                state.agent_scroll_up((-scroll_lines) as usize);
            } else {
                let line_count = state.agent_text.lines().count();
                state.agent_scroll_down(scroll_lines as usize, line_count);
            }
            return;
        }
    }

    if let Some(ref r) = areas.logs {
        if in_rect(r) {
            let inner_h = r.height.saturating_sub(2) as usize;
            let total = state.logs.len().min(120);
            let max_offset = total.saturating_sub(inner_h);
            if scroll_lines < 0 {
                state.logs_scroll_up((-scroll_lines) as usize);
            } else {
                state.logs_scroll_down(scroll_lines as usize, max_offset);
            }
            return;
        }
    }

    if let Some(ref r) = areas.tools {
        if in_rect(r) {
            let inner_h = r.height.saturating_sub(2) as usize;
            let total = state.tools.len().min(80);
            let max_offset = total.saturating_sub(inner_h);
            if scroll_lines < 0 {
                state.tools_scroll_up((-scroll_lines) as usize);
            } else {
                state.tools_scroll_down(scroll_lines as usize, max_offset);
            }
            return;
        }
    }

    if let Some(ref r) = areas.input {
        if in_rect(r) {
            // input_scroll is used as a hint by the renderer; auto-scroll
            // overrides it when the cursor would be off-screen.
            if scroll_lines < 0 {
                state.input_scroll_up((-scroll_lines) as usize);
            } else {
                // Use a generous max — the renderer will clamp.
                state.input_scroll_down(scroll_lines as usize, 10_000);
            }
        }
    }
}

/// Move the input cursor up one line, preserving the column position.
fn input_cursor_up(state: &mut AppState) {
    let text = &state.input_text;
    let before = &text[..state.input_cursor];

    // Find start of current line and the column offset.
    let cur_line_start = match before.rfind('\n') {
        Some(pos) => pos + 1,
        None => return, // Already on the first line.
    };
    let col = state.input_cursor - cur_line_start;

    // Find start of previous line.
    let prev_line_start = match before[..cur_line_start.saturating_sub(1)].rfind('\n') {
        Some(pos) => pos + 1,
        None => 0,
    };
    let prev_line_len = cur_line_start.saturating_sub(1) - prev_line_start;
    state.input_cursor = prev_line_start + col.min(prev_line_len);
}

/// Move the input cursor down one line, preserving the column position.
fn input_cursor_down(state: &mut AppState) {
    let text = &state.input_text;
    let before = &text[..state.input_cursor];
    let after = &text[state.input_cursor..];

    // Find start of current line and column offset.
    let cur_line_start = match before.rfind('\n') {
        Some(pos) => pos + 1,
        None => 0,
    };
    let col = state.input_cursor - cur_line_start;

    // Find the newline after the cursor (end of current line).
    let Some(nl_offset) = after.find('\n') else {
        return; // Already on the last line.
    };
    let next_line_start = state.input_cursor + nl_offset + 1;

    // Find the end of the next line.
    let next_line_end = match text[next_line_start..].find('\n') {
        Some(pos) => next_line_start + pos,
        None => text.len(),
    };
    let next_line_len = next_line_end - next_line_start;
    state.input_cursor = next_line_start + col.min(next_line_len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    #[test]
    fn freetext_typing_and_backspace() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Type "hi"
        process_key(&mut state, &mut interaction, key(KeyCode::Char('h')));
        process_key(&mut state, &mut interaction, key(KeyCode::Char('i')));
        assert_eq!(state.input_text, "hi");
        assert_eq!(state.input_cursor, 2);

        // Backspace deletes last char
        process_key(&mut state, &mut interaction, key(KeyCode::Backspace));
        assert_eq!(state.input_text, "h");
        assert_eq!(state.input_cursor, 1);

        // No modal should be involved at any point
        assert!(state.modal.is_none());
    }

    #[test]
    fn freetext_enter_submits() {
        let mut state = AppState::default();
        let (tx, rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Type some text
        process_key(&mut state, &mut interaction, key(KeyCode::Char('a')));
        process_key(&mut state, &mut interaction, key(KeyCode::Char('b')));

        // Enter submits
        process_key(&mut state, &mut interaction, key(KeyCode::Enter));
        let result = rx.try_recv().unwrap();
        assert!(matches!(result, UiPromptResult::Input(s) if s == "ab"));
        assert!(!state.input_active);
        assert!(matches!(interaction, Interaction::None));
    }

    #[test]
    fn freetext_empty_enter_exits() {
        let mut state = AppState::default();
        let (tx, rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Empty enter on empty buffer → Exit
        process_key(&mut state, &mut interaction, key(KeyCode::Enter));
        let result = rx.try_recv().unwrap();
        assert!(matches!(result, UiPromptResult::Exit));
        assert!(!state.input_active);
    }

    #[test]
    fn freetext_shift_enter_inserts_newline() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        process_key(&mut state, &mut interaction, key(KeyCode::Char('a')));
        process_key(
            &mut state,
            &mut interaction,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        );
        process_key(&mut state, &mut interaction, key(KeyCode::Char('b')));

        assert_eq!(state.input_text, "a\nb");
        assert_eq!(state.input_cursor, 3);
        // Should still be active (not submitted)
        assert!(matches!(interaction, Interaction::Multiline { .. }));
    }

    #[test]
    fn freetext_ctrl_c_interrupts() {
        let mut state = AppState::default();
        let (tx, rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        process_key(&mut state, &mut interaction, ctrl_c());
        let result = rx.try_recv().unwrap();
        assert!(matches!(result, UiPromptResult::Interrupted));
        assert!(!state.input_active);
        assert!(matches!(interaction, Interaction::None));
    }

    #[test]
    fn cursor_navigation_left_right() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Type "abc"
        for ch in ['a', 'b', 'c'] {
            process_key(&mut state, &mut interaction, key(KeyCode::Char(ch)));
        }
        assert_eq!(state.input_cursor, 3);

        // Left moves back one char
        process_key(&mut state, &mut interaction, key(KeyCode::Left));
        assert_eq!(state.input_cursor, 2);

        // Type 'x' at cursor position
        process_key(&mut state, &mut interaction, key(KeyCode::Char('x')));
        assert_eq!(state.input_text, "abxc");
        assert_eq!(state.input_cursor, 3);

        // Right moves forward
        process_key(&mut state, &mut interaction, key(KeyCode::Right));
        assert_eq!(state.input_cursor, 4);

        // Left at position 0 stays at 0
        state.input_cursor = 0;
        process_key(&mut state, &mut interaction, key(KeyCode::Left));
        assert_eq!(state.input_cursor, 0);
    }

    #[test]
    fn cursor_navigation_home_end() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Type "ab\ncd" (using Shift+Enter for newline)
        for ch in ['a', 'b'] {
            process_key(&mut state, &mut interaction, key(KeyCode::Char(ch)));
        }
        process_key(
            &mut state,
            &mut interaction,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        );
        for ch in ['c', 'd'] {
            process_key(&mut state, &mut interaction, key(KeyCode::Char(ch)));
        }
        assert_eq!(state.input_text, "ab\ncd");
        assert_eq!(state.input_cursor, 5); // end of "cd"

        // Home moves to start of current line
        process_key(&mut state, &mut interaction, key(KeyCode::Home));
        assert_eq!(state.input_cursor, 3); // start of "cd"

        // End moves to end of current line
        process_key(&mut state, &mut interaction, key(KeyCode::End));
        assert_eq!(state.input_cursor, 5); // end of "cd"
    }

    #[test]
    fn cursor_navigation_up_down() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Build "ab\ncd\nef"
        state.input_text = "ab\ncd\nef".to_string();
        state.input_cursor = 7; // at 'f' in "ef"

        // Up from "ef" line, col 1 → "cd" line, col 1
        process_key(&mut state, &mut interaction, key(KeyCode::Up));
        assert_eq!(state.input_cursor, 4); // 'd' in "cd"

        // Up from "cd" line, col 1 → "ab" line, col 1
        process_key(&mut state, &mut interaction, key(KeyCode::Up));
        assert_eq!(state.input_cursor, 1); // 'b' in "ab"

        // Up from first line → stays
        process_key(&mut state, &mut interaction, key(KeyCode::Up));
        assert_eq!(state.input_cursor, 1);

        // Down from "ab" line, col 1 → "cd" line, col 1
        process_key(&mut state, &mut interaction, key(KeyCode::Down));
        assert_eq!(state.input_cursor, 4);

        // Down from "cd" line, col 1 → "ef" line, col 1
        process_key(&mut state, &mut interaction, key(KeyCode::Down));
        assert_eq!(state.input_cursor, 7);

        // Down from last line → stays
        process_key(&mut state, &mut interaction, key(KeyCode::Down));
        assert_eq!(state.input_cursor, 7);
    }

    #[test]
    fn pageup_pagedown_scroll_agent_during_input() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.agent_text = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Initially auto-scrolling (None)
        assert!(state.agent_scroll.is_none());

        // PageUp scrolls agent stream
        process_key(&mut state, &mut interaction, key(KeyCode::PageUp));
        assert!(state.agent_scroll.is_some());

        // Interaction should still be Multiline
        assert!(matches!(interaction, Interaction::Multiline { .. }));
    }

    #[test]
    fn backspace_at_mid_cursor() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        state.input_text = "abc".to_string();
        state.input_cursor = 2; // before 'c'

        process_key(&mut state, &mut interaction, key(KeyCode::Backspace));
        assert_eq!(state.input_text, "ac");
        assert_eq!(state.input_cursor, 1);
    }

    #[test]
    fn delete_key() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        state.input_text = "abc".to_string();
        state.input_cursor = 1; // before 'b'

        process_key(&mut state, &mut interaction, key(KeyCode::Delete));
        assert_eq!(state.input_text, "ac");
        assert_eq!(state.input_cursor, 1);
    }

    #[test]
    fn choice_mode_cursor_navigation() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        let choices = vec![
            "Option A".to_string(),
            "Option B".to_string(),
            "Option C".to_string(),
        ];
        state.activate_input("Choose".to_string(), "Pick one".to_string(), Some(choices));
        let mut interaction = Interaction::Multiline { reply: tx };

        // Starts at 0
        assert_eq!(state.input_choice_cursor, 0);

        // Up at 0 clamps at 0
        process_key(&mut state, &mut interaction, key(KeyCode::Up));
        assert_eq!(state.input_choice_cursor, 0);

        // Down moves to 1
        process_key(&mut state, &mut interaction, key(KeyCode::Down));
        assert_eq!(state.input_choice_cursor, 1);

        // Down again moves to 2
        process_key(&mut state, &mut interaction, key(KeyCode::Down));
        assert_eq!(state.input_choice_cursor, 2);

        // Down at max (len-1 = 2) clamps
        process_key(&mut state, &mut interaction, key(KeyCode::Down));
        assert_eq!(state.input_choice_cursor, 2);

        // Up goes back to 1
        process_key(&mut state, &mut interaction, key(KeyCode::Up));
        assert_eq!(state.input_choice_cursor, 1);

        // Interaction still active
        assert!(matches!(interaction, Interaction::Multiline { .. }));
    }

    #[test]
    fn choice_mode_number_key_selects() {
        let mut state = AppState::default();
        let (tx, rx) = std::sync::mpsc::channel();

        let choices = vec![
            "Option A".to_string(),
            "Option B".to_string(),
            "Option C".to_string(),
        ];
        state.activate_input("Choose".to_string(), "Pick one".to_string(), Some(choices));
        let mut interaction = Interaction::Multiline { reply: tx };

        // Press '2' to select the second choice
        process_key(&mut state, &mut interaction, key(KeyCode::Char('2')));

        let result = rx.try_recv().unwrap();
        assert!(matches!(result, UiPromptResult::Input(s) if s == "Option B"));
        assert!(!state.input_active);
        assert!(matches!(interaction, Interaction::None));
    }

    #[test]
    fn choice_mode_typing_switches_to_freetext() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        let choices = vec![
            "Option A".to_string(),
            "Option B".to_string(),
            "Option C".to_string(),
        ];
        state.activate_input("Choose".to_string(), "Pick one".to_string(), Some(choices));
        let mut interaction = Interaction::Multiline { reply: tx };

        // Press 'a' — should switch to free-text mode
        process_key(&mut state, &mut interaction, key(KeyCode::Char('a')));

        assert!(state.input_choices.is_none());
        assert_eq!(state.input_text, "a");
        // Interaction should still be Multiline (not resolved)
        assert!(matches!(interaction, Interaction::Multiline { .. }));
        // Input should still be active
        assert!(state.input_active);
    }
}

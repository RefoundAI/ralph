//! UI runtime loop backed by ratatui + crossterm.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::ui::state::AppState;
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
    execute!(stdout, EnterAlternateScreen, Hide)?;
    enable_raw_mode()?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut state = AppState::default();
    let mut interaction = Interaction::None;
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

        handle_keys(&mut state, &mut interaction);

        // Show terminal cursor when input pane is active in free-text mode,
        // hide it otherwise so it doesn't flicker over the dashboard.
        if state.input_active && state.input_choices.is_none() {
            let _ = execute!(terminal.backend_mut(), Show);
        } else {
            let _ = execute!(terminal.backend_mut(), Hide);
        }

        terminal.draw(|frame| view::render(frame, &state))?;
    }

    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
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

fn handle_keys(state: &mut AppState, interaction: &mut Interaction) {
    // Drain ALL available key events before returning, so paste and held-key
    // repeats are batched into a single redraw cycle.
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
        let Event::Key(key) = ev else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        process_key(state, interaction, key);
    }
}

fn process_key(
    state: &mut AppState,
    interaction: &mut Interaction,
    key: crossterm::event::KeyEvent,
) {
    match interaction {
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
            KeyCode::Enter => {
                if state.input_current_line.is_empty() {
                    let old = std::mem::replace(interaction, Interaction::None);
                    if let Interaction::Multiline { reply } = old {
                        if state.input_lines.is_empty() {
                            let _ = reply.send(UiPromptResult::Exit);
                        } else {
                            let _ = reply.send(UiPromptResult::Input(state.input_lines.join("\n")));
                        }
                    }
                    state.deactivate_input();
                } else {
                    let line = std::mem::take(&mut state.input_current_line);
                    state.input_lines.push(line);
                }
            }
            KeyCode::Backspace => {
                state.input_current_line.pop();
            }
            KeyCode::Char(ch) => {
                state.input_current_line.push(ch);
            }
            KeyCode::Up => {
                state.agent_scroll_up(1);
            }
            KeyCode::PageUp => {
                state.agent_scroll_up(20);
            }
            KeyCode::Down => {
                let line_count = state.agent_text.lines().count();
                state.agent_scroll_down(1, line_count);
            }
            KeyCode::PageDown => {
                let line_count = state.agent_text.lines().count();
                state.agent_scroll_down(20, line_count);
            }
            KeyCode::End => {
                state.agent_scroll_to_bottom();
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
    fn freetext_keystroke_no_dual_state() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Type "hi"
        process_key(&mut state, &mut interaction, key(KeyCode::Char('h')));
        process_key(&mut state, &mut interaction, key(KeyCode::Char('i')));
        assert_eq!(state.input_current_line, "hi");
        assert!(state.input_lines.is_empty());

        // Enter commits the line
        process_key(&mut state, &mut interaction, key(KeyCode::Enter));
        assert!(state.input_current_line.is_empty());
        assert_eq!(state.input_lines, vec!["hi"]);

        // Type more and backspace
        process_key(&mut state, &mut interaction, key(KeyCode::Char('x')));
        process_key(&mut state, &mut interaction, key(KeyCode::Char('y')));
        assert_eq!(state.input_current_line, "xy");
        process_key(&mut state, &mut interaction, key(KeyCode::Backspace));
        assert_eq!(state.input_current_line, "x");

        // No modal should be involved at any point
        assert!(state.modal.is_none());
    }

    #[test]
    fn freetext_submit_on_empty_enter() {
        let mut state = AppState::default();
        let (tx, rx) = std::sync::mpsc::channel();

        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Type a line and commit it
        process_key(&mut state, &mut interaction, key(KeyCode::Char('a')));
        process_key(&mut state, &mut interaction, key(KeyCode::Enter));
        assert_eq!(state.input_lines, vec!["a"]);

        // Empty enter on non-empty buffer submits
        process_key(&mut state, &mut interaction, key(KeyCode::Enter));
        let result = rx.try_recv().unwrap();
        assert!(matches!(result, UiPromptResult::Input(s) if s == "a"));
        assert!(!state.input_active);
        assert!(matches!(interaction, Interaction::None));
    }

    #[test]
    fn freetext_empty_enter_on_empty_buffer_exits() {
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
    fn scroll_keys_work_during_active_input() {
        let mut state = AppState::default();
        let (tx, _rx) = std::sync::mpsc::channel();

        // Populate agent text with enough lines to scroll
        state.agent_text = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.activate_input("Test".to_string(), "hint".to_string(), None);
        let mut interaction = Interaction::Multiline { reply: tx };

        // Initially auto-scrolling (None)
        assert!(state.agent_scroll.is_none());

        // Up arrow should pin scroll
        process_key(&mut state, &mut interaction, key(KeyCode::Up));
        assert!(state.agent_scroll.is_some());

        // PageUp scrolls further up
        let before = state.agent_scroll.unwrap();
        process_key(&mut state, &mut interaction, key(KeyCode::PageUp));
        assert!(state.agent_scroll.unwrap() < before);

        // Down scrolls back
        let before = state.agent_scroll.unwrap();
        process_key(&mut state, &mut interaction, key(KeyCode::Down));
        assert!(state.agent_scroll.unwrap() > before);

        // End resumes auto-scroll
        process_key(&mut state, &mut interaction, key(KeyCode::End));
        assert!(state.agent_scroll.is_none());

        // Interaction should still be Multiline (scroll doesn't resolve it)
        assert!(matches!(interaction, Interaction::Multiline { .. }));
    }
}

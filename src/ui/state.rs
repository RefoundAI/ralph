//! Mutable app state for the TUI renderer.

use std::collections::VecDeque;

use crate::ui::event::{UiEvent, UiLevel};

const MAX_LOG_LINES: usize = 300;
const MAX_TOOL_LINES: usize = 200;
const MAX_AGENT_CHARS: usize = 60_000;

/// One line in the operator event log.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: UiLevel,
    pub message: String,
}

/// Optional modal rendered above the base screen.
#[derive(Debug, Clone)]
pub enum UiModal {
    Confirm {
        title: String,
        prompt: String,
        default_yes: bool,
    },
}

/// Top-level content screen.
#[derive(Debug, Clone)]
pub enum UiScreen {
    Dashboard,
    Explorer {
        title: String,
        lines: Vec<String>,
        scroll: usize,
    },
}

/// Render state for the TUI.
#[derive(Debug, Clone)]
pub struct AppState {
    pub status_line: String,
    pub dag_summary: String,
    pub current_task: String,
    pub logs: VecDeque<LogLine>,
    pub tools: VecDeque<String>,
    pub agent_text: String,
    /// When `None`, Agent Stream auto-scrolls to the bottom.
    /// When `Some(offset)`, the user has pinned the scroll position.
    pub agent_scroll: Option<usize>,
    pub screen: UiScreen,
    pub modal: Option<UiModal>,
    /// Whether the persistent Input pane is accepting keystrokes.
    pub input_active: bool,
    /// Title shown on the Input pane border.
    pub input_title: String,
    /// Hint text shown when inactive or as header when active.
    pub input_hint: String,
    /// Committed input lines (from Enter presses).
    pub input_lines: Vec<String>,
    /// Current line being typed.
    pub input_current_line: String,
    /// When `Some`, the Input pane renders as a choice selector.
    pub input_choices: Option<Vec<String>>,
    /// Which choice is highlighted in choice mode.
    pub input_choice_cursor: usize,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            status_line: "Starting".to_string(),
            dag_summary: "DAG: n/a".to_string(),
            current_task: "Task: idle".to_string(),
            logs: VecDeque::new(),
            tools: VecDeque::new(),
            agent_text: String::new(),
            agent_scroll: None,
            screen: UiScreen::Dashboard,
            modal: None,
            input_active: false,
            input_title: "Input".to_string(),
            input_hint: "Waiting for agent...".to_string(),
            input_lines: Vec::new(),
            input_current_line: String::new(),
            input_choices: None,
            input_choice_cursor: 0,
        }
    }
}

impl AppState {
    pub fn apply(&mut self, event: UiEvent) {
        match event {
            UiEvent::StatusLine(line) => {
                self.status_line = line;
            }
            UiEvent::DagSummary(line) => {
                self.dag_summary = line;
            }
            UiEvent::CurrentTask(line) => {
                self.current_task = line;
            }
            UiEvent::Log { level, message } => {
                self.logs.push_back(LogLine { level, message });
                while self.logs.len() > MAX_LOG_LINES {
                    self.logs.pop_front();
                }
            }
            UiEvent::AgentText(text) => {
                self.agent_text.push_str(&text);
                if self.agent_text.len() > MAX_AGENT_CHARS {
                    let mut split = self.agent_text.len() - MAX_AGENT_CHARS;
                    while split < self.agent_text.len() && !self.agent_text.is_char_boundary(split)
                    {
                        split += 1;
                    }
                    if split < self.agent_text.len() {
                        self.agent_text.drain(..split);
                    }
                }
            }
            UiEvent::ToolActivity(line) => {
                self.tools.push_back(line);
                while self.tools.len() > MAX_TOOL_LINES {
                    self.tools.pop_front();
                }
            }
        }
    }

    pub fn show_explorer(&mut self, title: String, lines: Vec<String>) {
        self.screen = UiScreen::Explorer {
            title,
            lines,
            scroll: 0,
        };
    }

    pub fn hide_explorer(&mut self) {
        self.screen = UiScreen::Dashboard;
    }

    /// Scroll the Agent Stream panel up by `n` lines. Activates pinned scroll
    /// mode, disabling auto-scroll. When auto-scrolling (None), we start from
    /// the approximate bottom so the first scroll-up moves up by `n` lines
    /// rather than jumping to the top.
    pub fn agent_scroll_up(&mut self, n: usize) {
        let current = self.agent_scroll.unwrap_or_else(|| {
            // Approximate current bottom offset from total line count.
            self.agent_text.lines().count()
        });
        self.agent_scroll = Some(current.saturating_sub(n));
    }

    /// Scroll the Agent Stream panel down by `n` lines, capped at the bottom.
    /// Passing the total line count allows clamping.
    pub fn agent_scroll_down(&mut self, n: usize, max_offset: usize) {
        let current = self.agent_scroll.unwrap_or(max_offset);
        let new = (current + n).min(max_offset);
        // If we've scrolled back to the bottom, resume auto-scroll.
        if new >= max_offset {
            self.agent_scroll = None;
        } else {
            self.agent_scroll = Some(new);
        }
    }

    /// Reset Agent Stream to auto-scroll (follow the tail).
    pub fn agent_scroll_to_bottom(&mut self) {
        self.agent_scroll = None;
    }

    /// Activate the input pane for a new prompt.
    pub fn activate_input(&mut self, title: String, hint: String, choices: Option<Vec<String>>) {
        self.input_active = true;
        self.input_title = title;
        self.input_hint = hint;
        self.input_lines.clear();
        self.input_current_line.clear();
        self.input_choices = choices;
        self.input_choice_cursor = 0;
    }

    /// Deactivate the input pane (return to idle).
    pub fn deactivate_input(&mut self) {
        self.input_active = false;
        self.input_title = "Input".to_string();
        self.input_hint = "Waiting for agent...".to_string();
        self.input_lines.clear();
        self.input_current_line.clear();
        self.input_choices = None;
        self.input_choice_cursor = 0;
    }

    pub fn explorer_scroll_up(&mut self) {
        if let UiScreen::Explorer { scroll, .. } = &mut self.screen {
            *scroll = scroll.saturating_sub(1);
        }
    }

    pub fn explorer_scroll_down(&mut self) {
        if let UiScreen::Explorer { lines, scroll, .. } = &mut self.screen {
            if *scroll + 1 < lines.len() {
                *scroll += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_updates_status_line() {
        let mut state = AppState::default();
        state.apply(UiEvent::StatusLine("iter 2".to_string()));
        assert_eq!(state.status_line, "iter 2");
    }

    #[test]
    fn log_ring_buffer_is_capped() {
        let mut state = AppState::default();
        for i in 0..400 {
            state.apply(UiEvent::Log {
                level: UiLevel::Info,
                message: format!("line {i}"),
            });
        }
        assert_eq!(state.logs.len(), 300);
        assert_eq!(
            state.logs.front().map(|l| l.message.as_str()),
            Some("line 100")
        );
    }

    #[test]
    fn agent_text_is_capped_and_utf8_safe() {
        let mut state = AppState::default();
        state.apply(UiEvent::AgentText("🎉".repeat(70_000)));
        assert!(state.agent_text.is_char_boundary(state.agent_text.len()));
        assert!(state.agent_text.len() <= 60_000 + 4);
    }

    #[test]
    fn input_activation_and_deactivation() {
        let mut state = AppState::default();

        // Verify defaults.
        assert!(!state.input_active);
        assert_eq!(state.input_title, "Input");
        assert_eq!(state.input_hint, "Waiting for agent...");
        assert!(state.input_lines.is_empty());
        assert!(state.input_current_line.is_empty());
        assert!(state.input_choices.is_none());
        assert_eq!(state.input_choice_cursor, 0);

        // Activate with free-text mode.
        state.activate_input(
            "Interactive Prompt".to_string(),
            "Type your response".to_string(),
            None,
        );
        assert!(state.input_active);
        assert_eq!(state.input_title, "Interactive Prompt");
        assert_eq!(state.input_hint, "Type your response");
        assert!(state.input_lines.is_empty());
        assert!(state.input_current_line.is_empty());
        assert!(state.input_choices.is_none());
        assert_eq!(state.input_choice_cursor, 0);

        // Simulate some typing.
        state.input_current_line = "hello".to_string();
        state.input_lines.push("first line".to_string());

        // Deactivate resets everything.
        state.deactivate_input();
        assert!(!state.input_active);
        assert_eq!(state.input_title, "Input");
        assert_eq!(state.input_hint, "Waiting for agent...");
        assert!(state.input_lines.is_empty());
        assert!(state.input_current_line.is_empty());
        assert!(state.input_choices.is_none());
        assert_eq!(state.input_choice_cursor, 0);

        // Activate with choice mode.
        let choices = vec!["Option A".to_string(), "Option B".to_string()];
        state.activate_input(
            "Choose".to_string(),
            "Pick one".to_string(),
            Some(choices.clone()),
        );
        assert!(state.input_active);
        assert_eq!(state.input_title, "Choose");
        assert_eq!(state.input_hint, "Pick one");
        assert_eq!(state.input_choices, Some(choices));
        assert_eq!(state.input_choice_cursor, 0);

        // Deactivate clears choices too.
        state.deactivate_input();
        assert!(state.input_choices.is_none());
    }

    #[test]
    fn explorer_scroll_bounds() {
        let mut state = AppState::default();
        state.show_explorer(
            "Tasks".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        state.explorer_scroll_up();
        if let UiScreen::Explorer { scroll, .. } = &state.screen {
            assert_eq!(*scroll, 0);
        } else {
            panic!("expected explorer");
        }
        state.explorer_scroll_down();
        state.explorer_scroll_down();
        state.explorer_scroll_down();
        if let UiScreen::Explorer { scroll, .. } = &state.screen {
            assert_eq!(*scroll, 2);
        } else {
            panic!("expected explorer");
        }
    }
}

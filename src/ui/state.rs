//! Mutable app state for the TUI renderer.

use std::collections::VecDeque;

use crate::ui::event::{EventLine, ToolLine, UiEvent};

const MAX_TOOL_LINES: usize = 200;
const MAX_EVENT_LINES: usize = 200;
const MAX_AGENT_CHARS: usize = 60_000;

/// Cached rectangle positions of dashboard frames from the last render pass.
/// Used by the event loop to route mouse scroll events to the correct panel.
#[derive(Debug, Clone, Default)]
pub struct FrameAreas {
    pub tools: Option<ratatui::layout::Rect>,
    pub agent: Option<ratatui::layout::Rect>,
    pub input: Option<ratatui::layout::Rect>,
    pub events: Option<ratatui::layout::Rect>,
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
    pub tools: VecDeque<ToolLine>,
    pub agent_text: String,
    /// Cached line count for `agent_text` to avoid repeated scans in hot paths.
    pub agent_line_count: usize,
    /// Monotonic revision for agent text updates; used by renderer caches.
    pub agent_revision: u64,
    /// When `None`, Agent Stream auto-scrolls to the bottom.
    /// When `Some(offset)`, the user has pinned the scroll position.
    pub agent_scroll: Option<usize>,
    /// When `None`, Tool Activity panel auto-scrolls to the bottom.
    /// When `Some(offset)`, the user has pinned the scroll position.
    pub tools_scroll: Option<usize>,
    /// Scroll offset for the Input pane content (when it overflows).
    pub input_scroll: usize,
    pub screen: UiScreen,
    pub modal: Option<UiModal>,
    /// Whether the persistent Input pane is accepting keystrokes.
    pub input_active: bool,
    /// Title shown on the Input pane border.
    pub input_title: String,
    /// Hint text shown when inactive or as header when active.
    pub input_hint: String,
    /// Full input buffer (may contain newlines from Shift+Enter).
    pub input_text: String,
    /// Byte offset of the cursor within `input_text`.
    pub input_cursor: usize,
    /// When `Some`, the Input pane renders as a choice selector.
    pub input_choices: Option<Vec<String>>,
    /// Which choice is highlighted in choice mode.
    pub input_choice_cursor: usize,
    /// Ring buffer of structured orchestration events for the Events panel.
    pub events: VecDeque<EventLine>,
    /// When `None`, Events panel auto-scrolls to the bottom.
    /// When `Some(offset)`, the user has pinned the scroll position.
    pub events_scroll: Option<usize>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            status_line: "Starting".to_string(),
            dag_summary: "DAG: n/a".to_string(),
            current_task: "Task: idle".to_string(),
            tools: VecDeque::new(),
            agent_text: String::new(),
            agent_line_count: 0,
            agent_revision: 0,
            agent_scroll: None,
            tools_scroll: None,
            input_scroll: 0,
            screen: UiScreen::Dashboard,
            modal: None,
            input_active: false,
            input_title: "Input".to_string(),
            input_hint: "Waiting for agent...".to_string(),
            input_text: String::new(),
            input_cursor: 0,
            input_choices: None,
            input_choice_cursor: 0,
            events: VecDeque::new(),
            events_scroll: None,
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
            UiEvent::AgentText(text) => {
                // Trim leading whitespace from the very first text chunk.
                let text = if self.agent_text.is_empty() {
                    text.trim_start().to_string()
                } else {
                    text
                };
                if text.is_empty() {
                    return;
                }

                append_collapsed_text(&mut self.agent_text, &text);
                self.cap_agent_text();
            }
            UiEvent::AgentThinking(text) => {
                // Indent each line of thinking text by 2 spaces and append.
                if text.is_empty() {
                    return;
                }
                // Ensure we start thinking on a new line.
                if !self.agent_text.is_empty() && !self.agent_text.ends_with('\n') {
                    self.agent_text.push('\n');
                }
                for line in text.lines() {
                    self.agent_text.push_str("  ");
                    self.agent_text.push_str(line);
                    self.agent_text.push('\n');
                }
                self.cap_agent_text();
            }
            UiEvent::IterationDivider { iteration } => {
                // Insert a visual divider line in the agent stream.
                if !self.agent_text.is_empty() && !self.agent_text.ends_with('\n') {
                    self.agent_text.push('\n');
                }
                self.agent_text
                    .push_str(&format!("\n───── iteration {iteration} ─────\n\n"));
                self.cap_agent_text();
            }
            UiEvent::ToolActivity(tool_line) => {
                self.tools.push_back(tool_line);
                while self.tools.len() > MAX_TOOL_LINES {
                    self.tools.pop_front();
                }
            }
            UiEvent::ToolDetail(detail) => {
                // Append as a detail line (no tool name, just indented text).
                self.tools.push_back(ToolLine {
                    name: String::new(),
                    summary: detail,
                });
                while self.tools.len() > MAX_TOOL_LINES {
                    self.tools.pop_front();
                }
            }
            UiEvent::Event(line) => {
                self.events.push_back(line);
                let mut dropped = 0usize;
                while self.events.len() > MAX_EVENT_LINES {
                    self.events.pop_front();
                    dropped += 1;
                }
                if dropped > 0 {
                    if let Some(offset) = self.events_scroll {
                        self.events_scroll = Some(offset.saturating_sub(dropped));
                    }
                }
            }
        }
    }

    /// Cap `agent_text` to `MAX_AGENT_CHARS`, update line count and revision.
    fn cap_agent_text(&mut self) {
        if self.agent_text.len() > MAX_AGENT_CHARS {
            let mut split = self.agent_text.len() - MAX_AGENT_CHARS;
            while split < self.agent_text.len() && !self.agent_text.is_char_boundary(split) {
                split += 1;
            }
            if split < self.agent_text.len() {
                self.agent_text.drain(..split);
            }
        }
        self.agent_line_count = if self.agent_text.is_empty() {
            0
        } else {
            self.agent_text.lines().count()
        };
        self.agent_revision = self.agent_revision.wrapping_add(1);
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
        // Approximate current bottom offset from total line count.
        let current = self.agent_scroll.unwrap_or(self.agent_line_count);
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

    /// Scroll the Events panel up by `n` lines. Activates pinned scroll mode.
    pub fn events_scroll_up(&mut self, n: usize) {
        let current = self.events_scroll.unwrap_or(self.events.len());
        self.events_scroll = Some(current.saturating_sub(n));
    }

    /// Scroll the Events panel down by `n` lines, capped at the bottom.
    /// Reaching the bottom resumes auto-scroll.
    pub fn events_scroll_down(&mut self, n: usize, max_offset: usize) {
        if let Some(offset) = self.events_scroll {
            let new = (offset + n).min(max_offset);
            if new >= max_offset {
                self.events_scroll = None;
            } else {
                self.events_scroll = Some(new);
            }
        }
        // If None (auto-scroll), down is a no-op — already at bottom.
    }

    /// Reset Events panel to auto-scroll (follow the tail).
    #[allow(dead_code)]
    pub fn events_scroll_to_bottom(&mut self) {
        self.events_scroll = None;
    }

    /// Scroll the Tool Activity panel up by `n` lines. Activates pinned scroll mode.
    pub fn tools_scroll_up(&mut self, n: usize) {
        let current = self.tools_scroll.unwrap_or(self.tools.len());
        self.tools_scroll = Some(current.saturating_sub(n));
    }

    /// Scroll the Tool Activity panel down by `n` lines, capped at the bottom.
    /// Reaching the bottom resumes auto-scroll.
    pub fn tools_scroll_down(&mut self, n: usize, max_offset: usize) {
        if let Some(offset) = self.tools_scroll {
            let new = (offset + n).min(max_offset);
            if new >= max_offset {
                self.tools_scroll = None;
            } else {
                self.tools_scroll = Some(new);
            }
        }
        // If None (auto-scroll), down is a no-op — already at bottom.
    }

    /// Reset Tool Activity panel to auto-scroll (follow the tail).
    #[allow(dead_code)]
    pub fn tools_scroll_to_bottom(&mut self) {
        self.tools_scroll = None;
    }

    /// Scroll the Input pane up by `n` lines.
    pub fn input_scroll_up(&mut self, n: usize) {
        self.input_scroll = self.input_scroll.saturating_sub(n);
    }

    /// Scroll the Input pane down by `n` lines.
    pub fn input_scroll_down(&mut self, n: usize, max_offset: usize) {
        self.input_scroll = (self.input_scroll + n).min(max_offset);
    }

    /// Activate the input pane for a new prompt.
    pub fn activate_input(&mut self, title: String, hint: String, choices: Option<Vec<String>>) {
        self.input_active = true;
        self.input_title = title;
        self.input_hint = hint;
        self.input_text.clear();
        self.input_cursor = 0;
        self.input_choices = choices;
        self.input_choice_cursor = 0;
        self.input_scroll = 0;
    }

    /// Deactivate the input pane (return to idle).
    pub fn deactivate_input(&mut self) {
        self.input_active = false;
        self.input_title = "Input".to_string();
        self.input_hint = "Waiting for agent...".to_string();
        self.input_text.clear();
        self.input_cursor = 0;
        self.input_choices = None;
        self.input_choice_cursor = 0;
        self.input_scroll = 0;
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

/// Append text while collapsing newline runs to at most two `\n` chars.
fn append_collapsed_text(dst: &mut String, src: &str) {
    let mut newline_run = dst.chars().rev().take_while(|ch| *ch == '\n').count();

    for ch in src.chars() {
        if ch == '\n' {
            if newline_run < 2 {
                dst.push('\n');
            }
            newline_run += 1;
        } else {
            dst.push(ch);
            newline_run = 0;
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
        assert!(state.input_text.is_empty());
        assert_eq!(state.input_cursor, 0);
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
        assert!(state.input_text.is_empty());
        assert_eq!(state.input_cursor, 0);
        assert!(state.input_choices.is_none());
        assert_eq!(state.input_choice_cursor, 0);

        // Simulate some typing.
        state.input_text = "hello\nfirst line".to_string();
        state.input_cursor = state.input_text.len();

        // Deactivate resets everything.
        state.deactivate_input();
        assert!(!state.input_active);
        assert_eq!(state.input_title, "Input");
        assert_eq!(state.input_hint, "Waiting for agent...");
        assert!(state.input_text.is_empty());
        assert_eq!(state.input_cursor, 0);
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

    fn make_event(category: &str, message: &str) -> EventLine {
        EventLine {
            category: category.to_string(),
            message: message.to_string(),
            timestamp: "12:00:00".to_string(),
            is_error: false,
        }
    }

    #[test]
    fn apply_handles_ui_event_event() {
        let mut state = AppState::default();
        assert!(state.events.is_empty());

        state.apply(UiEvent::Event(make_event("task", "t-abc12345 claimed")));
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].category, "task");
        assert_eq!(state.events[0].message, "t-abc12345 claimed");
    }

    #[test]
    fn events_ring_buffer_caps_at_max() {
        let mut state = AppState::default();
        for i in 0..201 {
            state.apply(UiEvent::Event(make_event("iter", &format!("event {i}"))));
        }
        assert_eq!(state.events.len(), 200);
        // First event (index 0) should have been dropped; front is now event 1.
        assert_eq!(state.events[0].message, "event 1");
        assert_eq!(state.events[199].message, "event 200");
    }

    #[test]
    fn events_ring_buffer_adjusts_scroll_offset() {
        let mut state = AppState::default();
        // Fill to capacity.
        for i in 0..200 {
            state.apply(UiEvent::Event(make_event("dag", &format!("event {i}"))));
        }
        // Pin scroll at offset 10.
        state.events_scroll = Some(10);

        // Push one more — drops one from front, offset should decrease by 1.
        state.apply(UiEvent::Event(make_event("dag", "event 200")));
        assert_eq!(state.events.len(), 200);
        assert_eq!(state.events_scroll, Some(9));

        // Pin scroll at 0, push another — offset stays at 0 (saturating_sub).
        state.events_scroll = Some(0);
        state.apply(UiEvent::Event(make_event("dag", "event 201")));
        assert_eq!(state.events_scroll, Some(0));
    }

    #[test]
    fn events_scroll_up_from_auto_scroll() {
        let mut state = AppState::default();
        for i in 0..50 {
            state.apply(UiEvent::Event(make_event("task", &format!("event {i}"))));
        }
        assert_eq!(state.events_scroll, None); // auto-scroll

        // Scroll up 5 from auto-scroll: should pin at len - 5 = 45.
        state.events_scroll_up(5);
        assert_eq!(state.events_scroll, Some(45));

        // Scroll up 10 more: 45 - 10 = 35.
        state.events_scroll_up(10);
        assert_eq!(state.events_scroll, Some(35));

        // Scroll up more than remaining: saturates to 0.
        state.events_scroll_up(100);
        assert_eq!(state.events_scroll, Some(0));
    }

    #[test]
    fn events_scroll_down_resumes_auto_scroll() {
        let mut state = AppState::default();
        for i in 0..50 {
            state.apply(UiEvent::Event(make_event("task", &format!("event {i}"))));
        }
        let max_offset = 40; // hypothetical panel height of 10

        // Pin at offset 30.
        state.events_scroll = Some(30);

        // Scroll down 5: 30 + 5 = 35.
        state.events_scroll_down(5, max_offset);
        assert_eq!(state.events_scroll, Some(35));

        // Scroll down 5 more: 35 + 5 = 40 >= max_offset, resume auto-scroll.
        state.events_scroll_down(5, max_offset);
        assert_eq!(state.events_scroll, None);

        // When already auto-scrolling, down is a no-op.
        state.events_scroll_down(10, max_offset);
        assert_eq!(state.events_scroll, None);
    }

    #[test]
    fn events_scroll_to_bottom() {
        let mut state = AppState::default();
        state.events_scroll = Some(15);
        state.events_scroll_to_bottom();
        assert_eq!(state.events_scroll, None);
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

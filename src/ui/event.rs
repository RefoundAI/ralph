//! Events emitted by core modules and consumed by the TUI runtime.

/// A structured tool activity entry for the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLine {
    /// Tool name (e.g. "Read", "Bash", "Edit").
    pub name: String,
    /// Concise summary of what the tool is doing.
    pub summary: String,
}

/// A structured event entry for the Events panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLine {
    /// Category label: "task", "iter", "feature", "verify", "review",
    /// "journal", "knowledge", "interrupt", "dag", "config".
    pub category: String,
    /// Pre-formatted message with template variables already substituted.
    pub message: String,
    /// Local timestamp formatted as "HH:MM:SS".
    pub timestamp: String,
    /// If true, render message in error style (red) instead of info style.
    pub is_error: bool,
}

/// Event payload rendered by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    StatusLine(String),
    DagSummary(String),
    CurrentTask(String),
    AgentText(String),
    /// Thinking text from the agent, rendered indented in the agent stream.
    AgentThinking(String),
    ToolActivity(ToolLine),
    /// Detail line for the most recent tool call (indented under it).
    ToolDetail(String),
    /// Visual divider between iterations in the agent stream.
    IterationDivider {
        iteration: u32,
    },
    /// Structured orchestration event for the Events panel.
    Event(EventLine),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_line_partial_eq() {
        let a = EventLine {
            category: "task".to_string(),
            message: "t-abcd1234 claimed".to_string(),
            timestamp: "14:32:05".to_string(),
            is_error: false,
        };
        let b = a.clone();
        assert_eq!(a, b);

        let c = EventLine {
            category: "task".to_string(),
            message: "t-abcd1234 claimed".to_string(),
            timestamp: "14:32:05".to_string(),
            is_error: true,
        };
        assert_ne!(a, c);
    }
}

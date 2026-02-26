//! Events emitted by core modules and consumed by the TUI runtime.

/// A structured tool activity entry for the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLine {
    /// Tool name (e.g. "Read", "Bash", "Edit").
    pub name: String,
    /// Concise summary of what the tool is doing.
    pub summary: String,
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
}

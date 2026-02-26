//! Events emitted by core modules and consumed by the TUI runtime.

/// Log severity for the event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLevel {
    Info,
    Warn,
    Error,
}

/// Event payload rendered by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    StatusLine(String),
    DagSummary(String),
    CurrentTask(String),
    Log { level: UiLevel, message: String },
    AgentText(String),
    ToolActivity(String),
}

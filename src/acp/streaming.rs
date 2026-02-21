//! Streaming display handler for ACP session updates.
//!
//! Maps `SessionUpdateMsg` variants to terminal output, mirroring the
//! rendering behavior of the old `src/output/formatter.rs::format_event()`
//! for ACP sessions.

use std::io::Write;

use colored::Colorize;

use crate::acp::tools::SessionUpdateMsg;

/// Render a single ACP session update to the terminal.
///
/// Output style per variant:
/// - `AgentText`      — bright white (matches text delta behavior)
/// - `AgentThought`   — bright black / dim (matches thinking delta behavior)
/// - `ToolCall`       — `name` in cyan + `input` dimmed
/// - `ToolCallError`  — red, capped at the first 5 lines
/// - `PlanUpdate`     — normal color
/// - `Finished`       — flushes stdout
pub fn render_session_update(update: &SessionUpdateMsg) {
    match update {
        SessionUpdateMsg::AgentText(text) => {
            print!("{}", text.bright_white());
        }
        SessionUpdateMsg::AgentThought(text) => {
            print!("{}", text.bright_black());
        }
        SessionUpdateMsg::ToolCall { name, input } => {
            println!("{} {}", name.cyan(), input.dimmed());
        }
        SessionUpdateMsg::ToolCallError { name, error } => {
            let lines: Vec<&str> = error.lines().take(5).collect();
            let truncated = lines.join("\n");
            eprintln!("{}: {}", name.red(), truncated.red());
        }
        SessionUpdateMsg::PlanUpdate(text) => {
            print!("{text}");
        }
        SessionUpdateMsg::Finished => {
            flush_stdout();
        }
    }
}

/// Flush stdout, ignoring any I/O errors.
pub fn flush_stdout() {
    std::io::stdout().flush().ok();
}

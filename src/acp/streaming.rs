//! Streaming display handler for ACP session updates.
//!
//! Maps `SessionUpdateMsg` variants to terminal output with visually distinct
//! formatting per output type:
//! - Tool calls: blue name `->` light gray metadata (single line)
//! - LLM responses: purple model name `->` default color text
//! - Errors: red name `->` bright red details
//! - Thinking: dim/gray, truncated to 100 chars
//! - Tool progress: suppressed (too noisy)

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use colored::Colorize;

use crate::acp::tools::SessionUpdateMsg;

/// State carried across render calls within a single session.
///
/// Tracks the model name (for the `model ->` prefix) and whether the current
/// text chunk is the first one after a tool call or session start (so we know
/// when to print the prefix).
pub struct RenderState {
    pub model_name: String,
    pub is_first_chunk: Rc<RefCell<bool>>,
}

/// Truncate a string to at most one line and `max_chars` characters.
///
/// Takes only the first line. If the result exceeds `max_chars`, truncates
/// and appends `...`.
pub fn truncate_to_line(s: &str, max_chars: usize) -> String {
    let first_line = s.lines().next().unwrap_or("");
    if first_line.len() > max_chars {
        format!("{}...", &first_line[..max_chars])
    } else {
        first_line.to_string()
    }
}

/// Render a single ACP session update to the terminal.
///
/// Output style per variant:
/// - `AgentText`         — `{model.purple()} -> ` prefix on first chunk, then default color
/// - `AgentThought`      — dim/gray, truncated to 100 chars
/// - `ToolCall`          — `{name.blue()} -> {input.bright_black()}` (single line)
/// - `ToolCallError`     — `ERROR.red() -> {error.bright_red()}`
/// - `ToolCallProgress`  — suppressed
/// - `Finished`          — newline + flush
pub fn render_session_update(update: &SessionUpdateMsg, state: &RenderState) {
    match update {
        SessionUpdateMsg::AgentText(text) => {
            let mut is_first = state.is_first_chunk.borrow_mut();
            if *is_first {
                print!("\n{} {} ", state.model_name.purple(), "->".dimmed());
                *is_first = false;
            }
            print!("{text}");
            flush_stdout();
        }
        SessionUpdateMsg::AgentThought(text) => {
            let truncated = truncate_to_line(text, 100);
            if !truncated.is_empty() {
                print!("{}", truncated.dimmed());
                flush_stdout();
            }
        }
        SessionUpdateMsg::ToolCall { name, input } => {
            // Reset first-chunk flag so the next text chunk gets a model prefix.
            *state.is_first_chunk.borrow_mut() = true;
            let truncated_input = truncate_to_line(input, 120);
            println!(
                "{} {} {}",
                name.blue(),
                "->".dimmed(),
                truncated_input.bright_black()
            );
        }
        SessionUpdateMsg::ToolCallError { name, error } => {
            eprintln!(
                "{} {} {}",
                "ERROR".red(),
                "->".dimmed(),
                format!("{name}: {error}").bright_red()
            );
        }
        SessionUpdateMsg::ToolCallProgress { .. } => {
            // Suppressed — file dumps and terminal output are noise in the new format.
        }
        SessionUpdateMsg::Finished => {
            println!();
            flush_stdout();
        }
    }
}

/// Flush stdout, ignoring any I/O errors.
pub fn flush_stdout() {
    std::io::stdout().flush().ok();
}

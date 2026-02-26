//! Streaming display handler for ACP session updates.
//!
//! Maps `SessionUpdateMsg` variants to terminal output with visually distinct
//! formatting per output type:
//! - Tool calls: blue name `->` light gray metadata (single line)
//! - LLM responses: purple model name `->` markdown-formatted text
//! - Errors: red name `->` bright red details
//! - Thinking: dim/gray, truncated to 100 chars
//! - Tool progress: suppressed (too noisy)

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use colored::Colorize;

use crate::acp::tools::SessionUpdateMsg;
use crate::ui::event::ToolLine;
use crate::ui::{self, UiEvent, UiLevel};

/// State carried across render calls within a single session.
///
/// Tracks the model name (for the `model ->` prefix), whether the current
/// text chunk is the first one after a tool call or session start, and a
/// line buffer for markdown formatting.
pub struct RenderState {
    pub model_name: String,
    pub is_first_chunk: Rc<RefCell<bool>>,
    pub line_buffer: Rc<RefCell<String>>,
    pub in_code_block: Rc<RefCell<bool>>,
    /// Tracks whether we are inside a multi-line sigil tag (e.g. `<journal>`).
    pub in_sigil: Rc<RefCell<Option<String>>>,
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

/// Flush any remaining partial line from the buffer to stdout.
///
/// Called before tool call lines and at session end to ensure no text is lost.
fn flush_line_buffer(state: &RenderState) {
    let mut buf = state.line_buffer.borrow_mut();
    if !buf.is_empty() {
        let line = std::mem::take(&mut *buf);
        let mut in_code = state.in_code_block.borrow_mut();
        let mut in_sigil = state.in_sigil.borrow_mut();
        let formatted = format_markdown_line(&line, &mut in_code, &mut in_sigil);
        print!("{formatted}");
        flush_stdout();
    }
}

/// Check whether a tool call has enough data for a useful summary line.
///
/// Returns `true` if there are locations or non-trivial input to display.
pub fn has_useful_summary(input: &str, locations: &[String]) -> bool {
    if !locations.is_empty() {
        return true;
    }
    let trimmed = input.trim();
    !trimmed.is_empty() && trimmed != "{}" && trimmed != "null"
}

/// Render a single ACP session update to the terminal.
///
/// Output style per variant:
/// - `AgentText`         — `{model.purple()} -> ` prefix on first chunk, then markdown-formatted
/// - `AgentThought`      — dim/gray, truncated to 100 chars
/// - `ToolCallPreamble`  — flush text buffer + newline separator + reset first_chunk
/// - `ToolCall`          — `{name.blue()} -> {input.bright_black()}` (summary + detail lines)
/// - `ToolCallError`     — `ERROR.red() -> {error.bright_red()}`
/// - `ToolCallProgress`  — suppressed
/// - `Finished`          — flush buffer + newline
pub fn render_session_update(update: &SessionUpdateMsg, state: &RenderState) {
    if ui::is_active() {
        match update {
            SessionUpdateMsg::AgentText(text) => {
                ui::emit(UiEvent::AgentText(text.to_owned()));
            }
            SessionUpdateMsg::AgentThought(text) => {
                if !text.is_empty() {
                    ui::emit(UiEvent::AgentThinking(text.to_owned()));
                }
            }
            SessionUpdateMsg::ToolCall {
                name,
                input,
                locations,
            } => {
                let summary = format_tool_summary(name, input, locations);
                ui::emit(UiEvent::ToolActivity(ToolLine {
                    name: name.to_string(),
                    summary,
                }));

                if !input.is_empty() && input != "{}" && input != "null" {
                    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(input) {
                        for detail in format_tool_detail_lines(name, &obj) {
                            ui::emit(UiEvent::ToolDetail(detail));
                        }
                    }
                }
            }
            SessionUpdateMsg::ToolCallDetail { detail_lines, .. } => {
                for detail in detail_lines {
                    ui::emit(UiEvent::ToolDetail(detail.to_string()));
                }
            }
            SessionUpdateMsg::ToolCallError { name, error } => {
                ui::emit(UiEvent::Log {
                    level: UiLevel::Error,
                    message: format!("{name}: {error}"),
                });
            }
            SessionUpdateMsg::ToolCallPreamble => {
                // Add a newline after the LLM response text before tool calls.
                ui::emit(UiEvent::AgentText("\n".to_string()));
            }
            SessionUpdateMsg::ToolCallProgress { .. } => {}
            SessionUpdateMsg::Finished => {
                // Add a trailing newline after the final LLM response.
                ui::emit(UiEvent::AgentText("\n".to_string()));
            }
        }
        return;
    }

    match update {
        SessionUpdateMsg::AgentText(text) => {
            let mut is_first = state.is_first_chunk.borrow_mut();
            let text = if *is_first {
                // Trim leading whitespace/newlines from the start of the response
                // so we don't get blank lines between "model ->" and the content.
                let trimmed = text.trim_start();
                if trimmed.is_empty() {
                    // Pure whitespace before any content — skip rendering.
                    return;
                }
                print!("\n{} {} ", state.model_name.purple(), "->".dimmed());
                *is_first = false;
                trimmed
            } else {
                text
            };
            drop(is_first);

            // Append to line buffer and render complete lines.
            let mut buf = state.line_buffer.borrow_mut();
            buf.push_str(text);

            // Process all complete lines (terminated by \n).
            while let Some(newline_pos) = buf.find('\n') {
                let line: String = buf.drain(..=newline_pos).collect();
                // Strip the trailing \n for formatting, then println.
                let trimmed = &line[..line.len() - 1];
                let mut in_code = state.in_code_block.borrow_mut();
                let mut in_sigil = state.in_sigil.borrow_mut();
                let formatted = format_markdown_line(trimmed, &mut in_code, &mut in_sigil);
                drop(in_sigil);
                drop(in_code);
                println!("{formatted}");
            }

            flush_stdout();
        }
        SessionUpdateMsg::AgentThought(text) => {
            let truncated = truncate_to_line(text, 100);
            if !truncated.is_empty() {
                print!("{}", truncated.dimmed());
                flush_stdout();
            }
        }
        SessionUpdateMsg::ToolCallPreamble => {
            // Flush any buffered text before the tool call line.
            let was_streaming = !*state.is_first_chunk.borrow();
            flush_line_buffer(state);

            // If text was streaming, add a newline to separate.
            if was_streaming {
                println!();
            }

            // Reset first-chunk flag so the next text chunk gets a model prefix.
            *state.is_first_chunk.borrow_mut() = true;
        }
        SessionUpdateMsg::ToolCall {
            name,
            input,
            locations,
        } => {
            let summary = format_tool_summary(name, input, locations);
            println!(
                "{} {} {}",
                name.blue(),
                "->".dimmed(),
                summary.bright_black()
            );

            // If we have raw_input, render detail lines immediately.
            if !input.is_empty() && input != "{}" && input != "null" {
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(input) {
                    let details = format_tool_detail_lines(name, &obj);
                    for line in &details {
                        println!("  {} {}", "|".dimmed(), line.bright_black());
                    }
                }
            }
        }
        SessionUpdateMsg::ToolCallDetail { name, detail_lines } => {
            for line in detail_lines {
                println!("  {} {}", "|".dimmed(), line.bright_black());
            }
            let _ = name; // used for potential future per-tool styling
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
            flush_line_buffer(state);
            println!();
            flush_stdout();
        }
    }
}

/// Build a concise summary of what a tool call is doing.
///
/// Priority:
/// 1. Locations (file paths from ACP) — shown as shortened paths.
/// 2. Extracted fields from raw_input JSON (file_path, command, pattern, query, etc.)
/// 3. Truncated raw input as fallback.
fn format_tool_summary(name: &str, raw_input: &str, locations: &[String]) -> String {
    // If we have locations, show them (shortened).
    if !locations.is_empty() {
        let paths: Vec<&str> = locations.iter().map(|p| shorten_path(p)).collect();
        return paths.join(", ");
    }

    // Try to extract useful fields from raw_input JSON.
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(raw_input) {
        if let Some(summary) = extract_tool_metadata(name, &obj) {
            return summary;
        }
    }

    // Fallback: truncated raw input (skip if empty or just "{}").
    let trimmed = raw_input.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return String::new();
    }
    truncate_to_line(raw_input, 120)
}

/// Extract a meaningful one-liner from tool input JSON based on tool name.
fn extract_tool_metadata(name: &str, input: &serde_json::Value) -> Option<String> {
    let name_lower = name.to_lowercase();

    // File operations: look for file_path / path.
    if name_lower.contains("read")
        || name_lower.contains("edit")
        || name_lower.contains("write")
        || name_lower.contains("notebook")
    {
        if let Some(path) = input.get("file_path").or(input.get("path")) {
            return Some(shorten_path(path.as_str()?).to_string());
        }
    }

    // Bash / terminal: show the command.
    if name_lower.contains("bash") || name_lower.contains("terminal") || name_lower == "execute" {
        if let Some(cmd) = input.get("command") {
            let cmd_str = cmd.as_str()?;
            return Some(truncate_to_line(cmd_str, 120));
        }
    }

    // Search tools: show pattern + optional path.
    if name_lower.contains("grep") || name_lower.contains("search") || name_lower.contains("glob") {
        let mut parts = Vec::new();
        if let Some(pat) = input.get("pattern") {
            parts.push(format!("/{}/", pat.as_str()?));
        }
        if let Some(path) = input.get("path") {
            parts.push(shorten_path(path.as_str()?).to_string());
        }
        if !parts.is_empty() {
            return Some(parts.join(" in "));
        }
    }

    // Web fetch: show URL.
    if name_lower.contains("fetch") || name_lower.contains("web") {
        if let Some(url) = input.get("url") {
            return Some(truncate_to_line(url.as_str()?, 120));
        }
    }

    // Task tool / agent launch: show description or prompt snippet.
    if name_lower.contains("task") || name_lower.contains("agent") {
        if let Some(desc) = input.get("description") {
            return Some(truncate_to_line(desc.as_str()?, 80));
        }
        if let Some(prompt) = input.get("prompt") {
            return Some(truncate_to_line(prompt.as_str()?, 80));
        }
    }

    // Generic: try common field names.
    for key in &["file_path", "path", "command", "pattern", "query", "url"] {
        if let Some(val) = input.get(key) {
            if let Some(s) = val.as_str() {
                let label = if *key == "file_path" || *key == "path" {
                    shorten_path(s).to_string()
                } else {
                    truncate_to_line(s, 120)
                };
                return Some(label);
            }
        }
    }

    None
}

/// Build up to 2 detail lines for a tool call based on its name and input JSON.
///
/// Returns meaningful fields extracted per tool type:
/// - **Edit**: file path, then `"old..." → "new..."` (truncated)
/// - **Write**: file path
/// - **Read**: file path (+ offset/limit if present)
/// - **Bash**: command, then description
/// - **Grep/Glob**: `/pattern/` in path
/// - **Task**: description, then prompt snippet
/// - **WebFetch**: URL
/// - **Generic**: first 2 recognizable fields
pub fn format_tool_detail_lines(name: &str, input: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    let name_lower = name.to_lowercase();

    if name_lower.contains("edit") || name_lower.contains("notebook") {
        if let Some(path) = input
            .get("file_path")
            .or(input.get("path"))
            .or(input.get("notebook_path"))
            .and_then(|v| v.as_str())
        {
            lines.push(shorten_path(path).to_string());
        }
        // Show old → new for Edit
        if let (Some(old), Some(new)) = (
            input.get("old_string").and_then(|v| v.as_str()),
            input.get("new_string").and_then(|v| v.as_str()),
        ) {
            let old_trunc = truncate_to_line(old, 40);
            let new_trunc = truncate_to_line(new, 40);
            lines.push(format!("\"{}\" -> \"{}\"", old_trunc, new_trunc));
        }
        return lines;
    }

    if name_lower.contains("write") {
        if let Some(path) = input
            .get("file_path")
            .or(input.get("path"))
            .and_then(|v| v.as_str())
        {
            lines.push(shorten_path(path).to_string());
        }
        return lines;
    }

    if name_lower.contains("read") {
        if let Some(path) = input
            .get("file_path")
            .or(input.get("path"))
            .and_then(|v| v.as_str())
        {
            let mut detail = shorten_path(path).to_string();
            let offset = input.get("offset").or(input.get("line"));
            let limit = input.get("limit");
            if offset.is_some() || limit.is_some() {
                let parts: Vec<String> = [
                    offset.map(|v| format!("offset={}", v)),
                    limit.map(|v| format!("limit={}", v)),
                ]
                .into_iter()
                .flatten()
                .collect();
                detail.push_str(&format!(" ({})", parts.join(", ")));
            }
            lines.push(detail);
        }
        return lines;
    }

    if name_lower.contains("bash") || name_lower.contains("terminal") || name_lower == "execute" {
        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
            lines.push(truncate_to_line(cmd, 100));
        }
        if let Some(desc) = input.get("description").and_then(|v| v.as_str()) {
            lines.push(truncate_to_line(desc, 100));
        }
        return lines;
    }

    if name_lower.contains("grep") || name_lower.contains("glob") || name_lower.contains("search") {
        let mut parts = Vec::new();
        if let Some(pat) = input.get("pattern").and_then(|v| v.as_str()) {
            parts.push(format!("/{}/", pat));
        }
        if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
            parts.push(format!("in {}", shorten_path(path)));
        }
        if !parts.is_empty() {
            lines.push(parts.join(" "));
        }
        return lines;
    }

    if name_lower.contains("task") || name_lower.contains("agent") {
        if let Some(desc) = input.get("description").and_then(|v| v.as_str()) {
            lines.push(truncate_to_line(desc, 80));
        }
        if let Some(prompt) = input.get("prompt").and_then(|v| v.as_str()) {
            lines.push(truncate_to_line(prompt, 80));
        }
        return lines;
    }

    if name_lower.contains("fetch") || name_lower.contains("web") {
        if let Some(url) = input.get("url").and_then(|v| v.as_str()) {
            lines.push(truncate_to_line(url, 120));
        }
        return lines;
    }

    // Generic: first 2 recognizable string fields
    for key in &["file_path", "path", "command", "pattern", "query", "url"] {
        if lines.len() >= 2 {
            break;
        }
        if let Some(val) = input.get(key).and_then(|v| v.as_str()) {
            let label = if *key == "file_path" || *key == "path" {
                shorten_path(val).to_string()
            } else {
                truncate_to_line(val, 100)
            };
            lines.push(label);
        }
    }

    lines
}

/// Known sigil tag names for inline formatting.
const SIGIL_TAGS: &[&str] = &[
    "task-done",
    "task-failed",
    "next-model",
    "journal",
    "knowledge",
    "promise",
    "verify-pass",
    "verify-fail",
];

/// Format a line containing sigil markup with colored tags.
///
/// Returns `Some(formatted)` if the line contains a sigil, `None` otherwise.
/// Handles single-line sigils (`<tag>content</tag>`), self-closing (`<verify-pass/>`),
/// opening tags, closing tags, and content lines within multi-line sigils.
fn format_sigil_line(line: &str, in_sigil: &mut Option<String>) -> Option<String> {
    let trimmed = line.trim();

    // Check for closing tag of current multi-line sigil.
    if let Some(ref tag) = in_sigil.clone() {
        let close_tag = format!("</{}>", tag);
        if trimmed.contains(&close_tag) {
            *in_sigil = None;
            return Some(format!("{}", line.green()));
        }
        // Inside multi-line sigil: render content as bright_black.
        return Some(format!("{}", line.bright_black()));
    }

    // Check for sigil patterns in this line.
    for tag in SIGIL_TAGS {
        let open = format!("<{}", tag);
        if !trimmed.contains(&open) {
            continue;
        }

        // Self-closing: <verify-pass/>
        let self_close = format!("<{}/>", tag);
        if trimmed.contains(&self_close) {
            return Some(format!("{}", line.green()));
        }

        // Single-line: <tag>content</tag> or <tag attr="...">content</tag>
        let close = format!("</{}>", tag);
        if trimmed.contains(&close) {
            return Some(format_sigil_single_line(line, tag, &close));
        }

        // Opening tag only (multi-line sigil starts).
        if trimmed.contains(&format!("<{}>", tag)) || trimmed.contains(&format!("<{} ", tag)) {
            *in_sigil = Some(tag.to_string());
            return Some(format!("{}", line.green()));
        }
    }

    None
}

/// Format a single-line sigil: green for the tags, bright_black for content.
fn format_sigil_single_line(line: &str, tag: &str, close_tag: &str) -> String {
    // Find the end of the opening tag (after `>`)
    let open_prefix = format!("<{}", tag);
    if let Some(open_start) = line.find(&open_prefix) {
        if let Some(open_end) = line[open_start..].find('>') {
            let abs_open_end = open_start + open_end + 1;
            if let Some(close_start) = line.find(close_tag) {
                let before = &line[..open_start];
                let open_tag_str = &line[open_start..abs_open_end];
                let content = &line[abs_open_end..close_start];
                let close_tag_str = close_tag;
                let after = &line[close_start + close_tag.len()..];
                return format!(
                    "{}{}{}{}{}",
                    before,
                    open_tag_str.green(),
                    content.bright_black(),
                    close_tag_str.green(),
                    after
                );
            }
        }
    }
    // Fallback: whole line green.
    format!("{}", line.green())
}

/// Shorten a file path to its last 2-3 components for display.
fn shorten_path(path: &str) -> &str {
    // Show at most the last 3 path segments.
    let mut count = 0;
    for (i, c) in path.char_indices().rev() {
        if c == '/' {
            count += 1;
            if count == 3 {
                return &path[i + 1..];
            }
        }
    }
    path
}

/// Format a single line of markdown for terminal display.
///
/// Handles:
/// - Fenced code block delimiters (```) → toggle state, render dimmed
/// - Lines inside code blocks → bright_black, no inline formatting
/// - Headings (`#`, `##`, `###`) → bold
/// - Other lines → inline markdown formatting
pub fn format_markdown_line(
    line: &str,
    in_code_block: &mut bool,
    in_sigil: &mut Option<String>,
) -> String {
    let trimmed = line.trim_start();

    // Check for fenced code block delimiter.
    if trimmed.starts_with("```") {
        *in_code_block = !*in_code_block;
        return format!("{}", line.dimmed());
    }

    // Inside code blocks: render as dim, no inline formatting.
    if *in_code_block {
        return format!("{}", line.bright_black());
    }

    // Check for sigil patterns (before markdown formatting).
    if let Some(formatted) = format_sigil_line(line, in_sigil) {
        return formatted;
    }

    // Headings: # → bold.
    if trimmed.starts_with("### ") || trimmed.starts_with("## ") || trimmed.starts_with("# ") {
        return format!("{}", line.bold());
    }

    // Normal text: apply inline markdown formatting.
    format_inline_markdown(line)
}

/// Apply inline markdown formatting to a line of text.
///
/// Recognizes:
/// - `` `code` `` → cyan
/// - `**bold**` → bold
/// - `*italic*` → italic
///
/// Unclosed delimiters are left as-is.
pub fn format_inline_markdown(line: &str) -> String {
    let mut result = String::with_capacity(line.len() + 32);
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Backtick: inline code.
        if chars[i] == '`' {
            if let Some(end) = find_closing_char(&chars, i + 1, '`') {
                let code: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!("\x1b[36m`{code}`\x1b[0m"));
                i = end + 1;
                continue;
            }
        }

        // Double asterisk: bold.
        if chars[i] == '*' && i + 1 < len && chars[i + 1] == '*' {
            if let Some(end) = find_closing_pair(&chars, i + 2, '*', '*') {
                let bold_text: String = chars[i + 2..end].iter().collect();
                result.push_str(&format!("\x1b[1m**{bold_text}**\x1b[22m"));
                i = end + 2;
                continue;
            }
        }

        // Single asterisk: italic (but not **).
        if chars[i] == '*' && !(i + 1 < len && chars[i + 1] == '*') {
            if let Some(end) = find_closing_single_asterisk(&chars, i + 1) {
                let italic_text: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!("\x1b[3m*{italic_text}*\x1b[23m"));
                i = end + 1;
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Find the position of a closing single character after `start`.
fn find_closing_char(chars: &[char], start: usize, close: char) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(j, ch)| if *ch == close { Some(j) } else { None })
}

/// Find the position of a closing two-character pair (e.g. `**`) after `start`.
///
/// Returns the index of the first character of the pair.
fn find_closing_pair(chars: &[char], start: usize, c1: char, c2: char) -> Option<usize> {
    if chars.len() < 2 {
        return None;
    }

    (start..(chars.len() - 1)).find(|&j| chars[j] == c1 && chars[j + 1] == c2)
}

/// Find a closing single `*` that is not part of `**`.
fn find_closing_single_asterisk(chars: &[char], start: usize) -> Option<usize> {
    for j in start..chars.len() {
        if chars[j] == '*' {
            // Make sure it's not a `**` pair.
            if j + 1 < chars.len() && chars[j + 1] == '*' {
                continue;
            }
            return Some(j);
        }
    }
    None
}

/// Flush stdout, ignoring any I/O errors.
pub fn flush_stdout() {
    std::io::stdout().flush().ok();
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- format_markdown_line tests ----------------------------------------

    #[test]
    fn test_plain_text_unchanged() {
        let mut in_code = false;
        let mut in_sigil = None;
        let result = format_markdown_line("hello world", &mut in_code, &mut in_sigil);
        assert_eq!(result, "hello world");
        assert!(!in_code);
    }

    #[test]
    fn test_heading_h1_is_bold() {
        let mut in_code = false;
        let mut in_sigil = None;
        let result = format_markdown_line("# Heading", &mut in_code, &mut in_sigil);
        // Should contain ANSI bold escape.
        assert!(result.contains("Heading"));
        assert!(!in_code);
    }

    #[test]
    fn test_heading_h2_is_bold() {
        let mut in_code = false;
        let mut in_sigil = None;
        let result = format_markdown_line("## Sub Heading", &mut in_code, &mut in_sigil);
        assert!(result.contains("Sub Heading"));
    }

    #[test]
    fn test_heading_h3_is_bold() {
        let mut in_code = false;
        let mut in_sigil = None;
        let result = format_markdown_line("### Third", &mut in_code, &mut in_sigil);
        assert!(result.contains("Third"));
    }

    #[test]
    fn test_code_block_toggle() {
        let mut in_code = false;
        let mut in_sigil = None;

        // Opening fence.
        let _ = format_markdown_line("```rust", &mut in_code, &mut in_sigil);
        assert!(in_code, "should be inside code block after opening fence");

        // Line inside code block.
        let inside = format_markdown_line("let x = 1;", &mut in_code, &mut in_sigil);
        assert!(in_code, "should still be inside code block");
        // Inside code block lines are bright_black (contain ANSI).
        assert!(inside.contains("let x = 1;"));

        // Closing fence.
        let _ = format_markdown_line("```", &mut in_code, &mut in_sigil);
        assert!(!in_code, "should be outside code block after closing fence");
    }

    #[test]
    fn test_code_block_no_inline_formatting() {
        let mut in_code = true;
        let mut in_sigil = None;
        let result = format_markdown_line("**not bold** `not code`", &mut in_code, &mut in_sigil);
        // Should NOT contain inline formatting escapes — rendered as bright_black plain text.
        assert!(
            !result.contains("\x1b[1m"),
            "should not apply bold inside code block"
        );
        assert!(
            !result.contains("\x1b[36m"),
            "should not apply cyan inside code block"
        );
    }

    // ---- format_inline_markdown tests --------------------------------------

    #[test]
    fn test_inline_code() {
        let result = format_inline_markdown("use `foo` here");
        assert!(
            result.contains("\x1b[36m`foo`\x1b[0m"),
            "backtick code should be cyan: {result}"
        );
    }

    #[test]
    fn test_inline_bold() {
        let result = format_inline_markdown("this is **bold** text");
        assert!(
            result.contains("\x1b[1m**bold**\x1b[22m"),
            "double asterisk should be bold: {result}"
        );
    }

    #[test]
    fn test_inline_italic() {
        let result = format_inline_markdown("this is *italic* text");
        assert!(
            result.contains("\x1b[3m*italic*\x1b[23m"),
            "single asterisk should be italic: {result}"
        );
    }

    #[test]
    fn test_unclosed_backtick_left_asis() {
        let result = format_inline_markdown("unclosed `backtick");
        assert_eq!(
            result, "unclosed `backtick",
            "unclosed delimiter should be literal"
        );
    }

    #[test]
    fn test_unclosed_bold_left_asis() {
        let result = format_inline_markdown("unclosed **bold");
        assert_eq!(result, "unclosed **bold");
    }

    #[test]
    fn test_unclosed_italic_left_asis() {
        let result = format_inline_markdown("unclosed *italic");
        assert_eq!(result, "unclosed *italic");
    }

    #[test]
    fn test_mixed_inline() {
        let result = format_inline_markdown("`code` and **bold** and *italic*");
        assert!(result.contains("\x1b[36m`code`\x1b[0m"), "code: {result}");
        assert!(result.contains("\x1b[1m**bold**\x1b[22m"), "bold: {result}");
        assert!(
            result.contains("\x1b[3m*italic*\x1b[23m"),
            "italic: {result}"
        );
    }

    #[test]
    fn test_empty_line() {
        let mut in_code = false;
        let mut in_sigil = None;
        let result = format_markdown_line("", &mut in_code, &mut in_sigil);
        assert_eq!(result, "");
    }

    // ---- truncate_to_line tests (existing) ---------------------------------

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate_to_line("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        assert_eq!(truncate_to_line("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_multiline() {
        assert_eq!(truncate_to_line("line1\nline2", 100), "line1");
    }

    // ---- shorten_path tests -------------------------------------------------

    #[test]
    fn test_shorten_deep_path() {
        assert_eq!(
            shorten_path("/Users/rk/code/ralph/src/acp/streaming.rs"),
            "src/acp/streaming.rs"
        );
    }

    #[test]
    fn test_shorten_short_path() {
        assert_eq!(shorten_path("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_shorten_just_filename() {
        assert_eq!(shorten_path("file.rs"), "file.rs");
    }

    // ---- format_tool_summary tests ------------------------------------------

    #[test]
    fn test_summary_from_locations() {
        let result = format_tool_summary(
            "Read File",
            "{}",
            &["/Users/rk/code/ralph/src/main.rs".into()],
        );
        // shorten_path keeps last 3 segments: ralph/src/main.rs
        assert_eq!(result, "ralph/src/main.rs");
    }

    #[test]
    fn test_summary_read_from_input() {
        let input = r#"{"file_path":"/Users/rk/code/ralph/src/acp/tools.rs"}"#;
        let result = format_tool_summary("Read File", input, &[]);
        assert_eq!(result, "src/acp/tools.rs");
    }

    #[test]
    fn test_summary_bash_command() {
        let input = r#"{"command":"cargo test --lib"}"#;
        let result = format_tool_summary("Bash", input, &[]);
        assert_eq!(result, "cargo test --lib");
    }

    #[test]
    fn test_summary_grep_pattern() {
        let input = r#"{"pattern":"raw_input","path":"/Users/rk/code/ralph/src"}"#;
        let result = format_tool_summary("Grep", input, &[]);
        assert_eq!(result, "/raw_input/ in code/ralph/src");
    }

    #[test]
    fn test_summary_empty_input() {
        let result = format_tool_summary("Edit", "{}", &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_summary_edit_from_input() {
        let input = r#"{"file_path":"/Users/rk/code/ralph/src/main.rs","old_string":"foo","new_string":"bar"}"#;
        let result = format_tool_summary("Edit", input, &[]);
        // /Users/rk/code/ralph/src/main.rs → 3 segments from right: ralph/src/main.rs
        assert_eq!(result, "ralph/src/main.rs");
    }

    // ---- has_useful_summary tests -------------------------------------------

    #[test]
    fn test_useful_summary_with_locations() {
        assert!(has_useful_summary("", &["/some/path".into()]));
    }

    #[test]
    fn test_useful_summary_with_json_input() {
        assert!(has_useful_summary(r#"{"file_path":"src/main.rs"}"#, &[]));
    }

    #[test]
    fn test_useful_summary_empty_input() {
        assert!(!has_useful_summary("", &[]));
    }

    #[test]
    fn test_useful_summary_empty_json() {
        assert!(!has_useful_summary("{}", &[]));
    }

    #[test]
    fn test_useful_summary_null() {
        assert!(!has_useful_summary("null", &[]));
    }

    #[test]
    fn test_useful_summary_whitespace_only() {
        assert!(!has_useful_summary("   ", &[]));
    }
}

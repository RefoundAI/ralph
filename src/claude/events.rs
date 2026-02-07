//! Event types for Claude's stream-json output.

use serde::Deserialize;
use std::collections::HashMap;

/// Sigils for completion detection.
pub const COMPLETE_SIGIL: &str = "<promise>COMPLETE</promise>";
pub const FAILURE_SIGIL: &str = "<promise>FAILURE</promise>";

/// Parsed event from Claude's NDJSON stream.
#[derive(Debug)]
pub enum Event {
    Assistant(Assistant),
    ToolErrors(Vec<ToolResult>),
    Result(ResultEvent),
    Unknown,
}

/// Assistant message with content blocks.
#[derive(Debug)]
pub struct Assistant {
    pub model: Option<String>,
    pub content: Vec<ContentBlock>,
}

/// Content block types.
#[derive(Debug)]
pub enum ContentBlock {
    Text { text: String },
    Thinking { thinking: String },
    ToolUse { id: String, name: String, input: HashMap<String, serde_json::Value> },
    Unknown,
}

/// Tool result from user message.
#[derive(Debug)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Final result event from Claude.
#[derive(Debug, Default)]
pub struct ResultEvent {
    pub result: Option<String>,
    pub duration_ms: u64,
    pub total_cost_usd: f64,
    /// Model hint extracted from `<next-model>...</next-model>` sigil.
    /// Applies to the next iteration only; `None` if absent or malformed.
    pub next_model_hint: Option<String>,
    /// Task ID from `<task-done>...</task-done>` sigil, if present.
    pub task_done: Option<String>,
    /// Task ID from `<task-failed>...</task-failed>` sigil, if present.
    pub task_failed: Option<String>,
}

impl ResultEvent {
    /// Check if result contains the COMPLETE sigil.
    pub fn is_complete(&self) -> bool {
        self.result
            .as_ref()
            .map(|r| r.contains(COMPLETE_SIGIL))
            .unwrap_or(false)
    }

    /// Check if result contains the FAILURE sigil.
    pub fn is_failure(&self) -> bool {
        self.result
            .as_ref()
            .map(|r| r.contains(FAILURE_SIGIL))
            .unwrap_or(false)
    }
}

/// Valid model names for the `<next-model>` sigil.
const VALID_MODELS: &[&str] = &["opus", "sonnet", "haiku"];

/// Parse the `<next-model>...</next-model>` sigil from result text.
///
/// Returns `Some(model)` if a valid model name is found between the tags,
/// `None` if the sigil is absent or contains an invalid model name.
pub fn parse_next_model_hint(text: &str) -> Option<String> {
    let start_tag = "<next-model>";
    let end_tag = "</next-model>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let model = text[content_start..content_start + end_idx].trim();

    if VALID_MODELS.contains(&model) {
        Some(model.to_string())
    } else {
        None
    }
}

/// Parse the `<task-done>...</task-done>` sigil from result text.
///
/// Returns `Some(task_id)` if a task ID is found between the tags,
/// `None` if the sigil is absent or malformed. Whitespace is trimmed.
pub fn parse_task_done(text: &str) -> Option<String> {
    let start_tag = "<task-done>";
    let end_tag = "</task-done>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let task_id = text[content_start..content_start + end_idx].trim();

    if task_id.is_empty() {
        None
    } else {
        Some(task_id.to_string())
    }
}

/// Parse the `<task-failed>...</task-failed>` sigil from result text.
///
/// Returns `Some(task_id)` if a task ID is found between the tags,
/// `None` if the sigil is absent or malformed. Whitespace is trimmed.
pub fn parse_task_failed(text: &str) -> Option<String> {
    let start_tag = "<task-failed>";
    let end_tag = "</task-failed>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let task_id = text[content_start..content_start + end_idx].trim();

    if task_id.is_empty() {
        None
    } else {
        Some(task_id.to_string())
    }
}

/// Raw JSON structures for deserialization.
#[derive(Deserialize)]
pub(crate) struct RawEvent {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub message: Option<RawMessage>,
    pub result: Option<String>,
    pub duration_ms: Option<u64>,
    pub total_cost_usd: Option<f64>,
}

#[derive(Deserialize)]
pub(crate) struct RawMessage {
    pub model: Option<String>,
    pub content: Option<Vec<RawContentBlock>>,
}

#[derive(Deserialize)]
pub(crate) struct RawContentBlock {
    #[serde(rename = "type")]
    pub block_type: Option<String>,
    pub text: Option<String>,
    pub thinking: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub input: Option<HashMap<String, serde_json::Value>>,
    pub tool_use_id: Option<String>,
    pub content: Option<String>,
    pub is_error: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_next_model_hint tests ---

    #[test]
    fn parse_hint_opus() {
        let text = "Some output here <next-model>opus</next-model> more text";
        assert_eq!(parse_next_model_hint(text), Some("opus".to_string()));
    }

    #[test]
    fn parse_hint_sonnet() {
        let text = "<next-model>sonnet</next-model>";
        assert_eq!(parse_next_model_hint(text), Some("sonnet".to_string()));
    }

    #[test]
    fn parse_hint_haiku() {
        let text = "Result text\n<next-model>haiku</next-model>\nDone.";
        assert_eq!(parse_next_model_hint(text), Some("haiku".to_string()));
    }

    #[test]
    fn parse_hint_with_whitespace_inside_tags() {
        let text = "<next-model> opus </next-model>";
        assert_eq!(parse_next_model_hint(text), Some("opus".to_string()));
    }

    #[test]
    fn parse_hint_absent_returns_none() {
        let text = "No sigil here, just regular output.";
        assert_eq!(parse_next_model_hint(text), None);
    }

    #[test]
    fn parse_hint_empty_text_returns_none() {
        assert_eq!(parse_next_model_hint(""), None);
    }

    #[test]
    fn parse_hint_invalid_model_returns_none() {
        let text = "<next-model>gpt-4</next-model>";
        assert_eq!(parse_next_model_hint(text), None);
    }

    #[test]
    fn parse_hint_empty_model_returns_none() {
        let text = "<next-model></next-model>";
        assert_eq!(parse_next_model_hint(text), None);
    }

    #[test]
    fn parse_hint_malformed_no_closing_tag_returns_none() {
        let text = "<next-model>opus";
        assert_eq!(parse_next_model_hint(text), None);
    }

    #[test]
    fn parse_hint_malformed_no_opening_tag_returns_none() {
        let text = "opus</next-model>";
        assert_eq!(parse_next_model_hint(text), None);
    }

    #[test]
    fn parse_hint_alongside_complete_sigil() {
        let text = "<promise>COMPLETE</promise>\n<next-model>haiku</next-model>";
        assert_eq!(parse_next_model_hint(text), Some("haiku".to_string()));
    }

    #[test]
    fn parse_hint_first_occurrence_wins() {
        // If multiple sigils, the first valid one is used
        let text = "<next-model>opus</next-model> later <next-model>haiku</next-model>";
        assert_eq!(parse_next_model_hint(text), Some("opus".to_string()));
    }

    // --- ResultEvent next_model_hint integration tests ---

    #[test]
    fn result_event_default_has_no_hint() {
        let event = ResultEvent::default();
        assert!(event.next_model_hint.is_none());
    }

    #[test]
    fn result_event_with_hint() {
        let event = ResultEvent {
            result: Some("done <next-model>opus</next-model>".to_string()),
            next_model_hint: Some("opus".to_string()),
            ..Default::default()
        };
        assert_eq!(event.next_model_hint, Some("opus".to_string()));
        assert!(!event.is_complete());
        assert!(!event.is_failure());
    }

    // --- parse_task_done tests ---

    #[test]
    fn parse_task_done_basic() {
        let text = "<task-done>t-abc123</task-done>";
        assert_eq!(parse_task_done(text), Some("t-abc123".to_string()));
    }

    #[test]
    fn parse_task_done_with_context() {
        let text = "Task completed: <task-done>t-xyz789</task-done> successfully.";
        assert_eq!(parse_task_done(text), Some("t-xyz789".to_string()));
    }

    #[test]
    fn parse_task_done_with_whitespace_inside_tags() {
        let text = "<task-done>  t-abc123  </task-done>";
        assert_eq!(parse_task_done(text), Some("t-abc123".to_string()));
    }

    #[test]
    fn parse_task_done_no_sigil() {
        let text = "No task sigil here";
        assert_eq!(parse_task_done(text), None);
    }

    #[test]
    fn parse_task_done_malformed_no_closing_tag() {
        let text = "<task-done>t-abc123";
        assert_eq!(parse_task_done(text), None);
    }

    #[test]
    fn parse_task_done_empty_content() {
        let text = "<task-done></task-done>";
        assert_eq!(parse_task_done(text), None);
    }

    // --- parse_task_failed tests ---

    #[test]
    fn parse_task_failed_basic() {
        let text = "<task-failed>t-def456</task-failed>";
        assert_eq!(parse_task_failed(text), Some("t-def456".to_string()));
    }

    #[test]
    fn parse_task_failed_with_context() {
        let text = "Task failed: <task-failed>t-ghi012</task-failed> with errors.";
        assert_eq!(parse_task_failed(text), Some("t-ghi012".to_string()));
    }

    #[test]
    fn parse_task_failed_with_whitespace_inside_tags() {
        let text = "<task-failed>  t-def456  </task-failed>";
        assert_eq!(parse_task_failed(text), Some("t-def456".to_string()));
    }

    #[test]
    fn parse_task_failed_no_sigil() {
        let text = "No task sigil here";
        assert_eq!(parse_task_failed(text), None);
    }

    #[test]
    fn parse_task_failed_malformed_no_closing_tag() {
        let text = "<task-failed>t-def456";
        assert_eq!(parse_task_failed(text), None);
    }

    #[test]
    fn parse_task_failed_empty_content() {
        let text = "<task-failed></task-failed>";
        assert_eq!(parse_task_failed(text), None);
    }

    // --- Multiple sigil tests ---

    #[test]
    fn both_task_sigils_task_done_wins() {
        let text = "<task-done>t-done</task-done> <task-failed>t-fail</task-failed>";
        assert_eq!(parse_task_done(text), Some("t-done".to_string()));
        assert_eq!(parse_task_failed(text), Some("t-fail".to_string()));
    }

    #[test]
    fn task_sigil_with_model_hint() {
        let text = "<task-done>t-task123</task-done>\n<next-model>opus</next-model>";
        assert_eq!(parse_task_done(text), Some("t-task123".to_string()));
        assert_eq!(parse_next_model_hint(text), Some("opus".to_string()));
    }
}

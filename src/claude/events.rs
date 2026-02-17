//! Event types for Claude's stream-json output.

use serde::Deserialize;
use std::collections::HashMap;

/// Sigils for completion detection.
#[allow(dead_code)]
pub const COMPLETE_SIGIL: &str = "<promise>COMPLETE</promise>";
pub const FAILURE_SIGIL: &str = "<promise>FAILURE</promise>";

/// Parsed event from Claude's NDJSON stream.
#[derive(Debug)]
pub enum Event {
    Assistant(Assistant),
    ToolErrors(Vec<ToolResult>),
    Result(ResultEvent),
    /// Streaming delta from --include-partial-messages.
    StreamDelta(StreamDelta),
    Unknown,
}

/// A streaming text/thinking delta from --include-partial-messages.
#[derive(Debug)]
pub struct StreamDelta {
    /// "text_delta" or "thinking_delta"
    pub delta_type: String,
    pub text: String,
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
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: HashMap<String, serde_json::Value>,
    },
    Unknown,
}

/// Tool result from user message.
#[derive(Debug)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Parsed from a `<knowledge>` sigil in Claude's output.
#[derive(Debug, Clone)]
pub struct KnowledgeSigil {
    pub title: String,
    pub tags: Vec<String>,
    pub body: String,
}

/// Final result event from Claude.
#[derive(Debug)]
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
    /// Notes from `<journal>...</journal>` sigil, if present.
    pub journal_notes: Option<String>,
    /// Knowledge entries from `<knowledge>` sigils, if any.
    pub knowledge_entries: Vec<KnowledgeSigil>,
    /// File paths modified during streaming (populated from tool use events).
    pub files_modified: Vec<String>,
}

impl Default for ResultEvent {
    fn default() -> Self {
        Self {
            result: None,
            duration_ms: 0,
            total_cost_usd: 0.0,
            next_model_hint: None,
            task_done: None,
            task_failed: None,
            journal_notes: None,
            knowledge_entries: Vec::new(),
            files_modified: Vec::new(),
        }
    }
}

impl ResultEvent {
    /// Check if result contains the COMPLETE sigil.
    #[allow(dead_code)]
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

/// Parse the `<journal>...</journal>` sigil from result text.
///
/// Returns `Some(notes)` if non-empty content is found between the tags,
/// `None` if the sigil is absent or the content is empty. Whitespace is trimmed.
pub fn parse_journal_sigil(text: &str) -> Option<String> {
    let start_tag = "<journal>";
    let end_tag = "</journal>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let notes = text[content_start..content_start + end_idx].trim();

    if notes.is_empty() {
        None
    } else {
        Some(notes.to_string())
    }
}

/// Parse all `<knowledge tags="..." title="...">...</knowledge>` sigils from result text.
///
/// Returns a `Vec` of `KnowledgeSigil` for each valid sigil found. Entries missing
/// required attributes (`tags`, `title`) or with an empty body are skipped.
pub fn parse_knowledge_sigils(text: &str) -> Vec<KnowledgeSigil> {
    let mut entries = Vec::new();
    let mut search_from = 0;

    while let Some(start_idx) = text[search_from..].find("<knowledge ") {
        let abs_start = search_from + start_idx;
        // Find closing > of the opening tag
        let tag_end = match text[abs_start..].find('>') {
            Some(idx) => abs_start + idx,
            None => break,
        };
        // Extract the attribute content (skip "<knowledge ")
        let tag_content = &text[abs_start + 11..tag_end];
        let title = extract_attribute(tag_content, "title");
        let tags_str = extract_attribute(tag_content, "tags");

        // Find </knowledge>
        let content_start = tag_end + 1;
        let end_tag = "</knowledge>";
        let end_idx = match text[content_start..].find(end_tag) {
            Some(idx) => content_start + idx,
            None => break,
        };
        let body = text[content_start..end_idx].trim().to_string();

        if let (Some(title), Some(tags_str)) = (title, tags_str) {
            let tags: Vec<String> = tags_str
                .split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect();
            if !tags.is_empty() && !body.is_empty() {
                entries.push(KnowledgeSigil { title, tags, body });
            }
        }
        search_from = end_idx + end_tag.len();
    }
    entries
}

/// Extract an attribute value from a tag's attribute string.
///
/// Looks for `attr_name="value"` in the tag content. Returns `None` if not found.
/// Handles attributes appearing in any order.
fn extract_attribute(tag_content: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr_name);
    let start = tag_content.find(&pattern)?;
    let value_start = start + pattern.len();
    let end = tag_content[value_start..].find('"')?;
    Some(tag_content[value_start..value_start + end].to_string())
}

/// Raw JSON structures for deserialization.
#[derive(Deserialize)]
pub(crate) struct RawEvent {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub message: Option<RawMessage>,
    pub event: Option<RawStreamEvent>,
    pub result: Option<String>,
    pub duration_ms: Option<u64>,
    pub total_cost_usd: Option<f64>,
}

/// Raw stream event from --include-partial-messages.
#[derive(Deserialize)]
pub(crate) struct RawStreamEvent {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub delta: Option<RawDelta>,
}

/// Delta payload inside a stream event.
#[derive(Deserialize)]
pub(crate) struct RawDelta {
    #[serde(rename = "type")]
    pub delta_type: Option<String>,
    pub text: Option<String>,
    pub thinking: Option<String>,
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

    // --- parse_journal_sigil tests ---

    #[test]
    fn test_parse_journal_sigil() {
        let text = "<journal>some notes</journal>";
        assert_eq!(parse_journal_sigil(text), Some("some notes".to_string()));
    }

    #[test]
    fn test_parse_journal_sigil_multiline() {
        let text = "<journal>\nLine one.\nLine two.\nLine three.\n</journal>";
        let result = parse_journal_sigil(text).unwrap();
        assert!(result.contains("Line one."));
        assert!(result.contains("Line two."));
        assert!(result.contains("Line three."));
    }

    #[test]
    fn test_parse_journal_sigil_absent() {
        let text = "No journal sigil here.";
        assert_eq!(parse_journal_sigil(text), None);
    }

    #[test]
    fn test_parse_journal_sigil_empty() {
        let text = "<journal></journal>";
        assert_eq!(parse_journal_sigil(text), None);
    }

    #[test]
    fn test_parse_journal_sigil_whitespace_only() {
        let text = "<journal>   \n   </journal>";
        assert_eq!(parse_journal_sigil(text), None);
    }

    #[test]
    fn test_parse_journal_sigil_with_context() {
        let text =
            "Task done.\n<journal>Chose nom for parsing.</journal>\n<task-done>t-abc</task-done>";
        assert_eq!(
            parse_journal_sigil(text),
            Some("Chose nom for parsing.".to_string())
        );
    }

    // --- parse_knowledge_sigils tests ---

    #[test]
    fn test_parse_knowledge_sigil() {
        let text = r#"<knowledge tags="testing,cargo" title="Cargo bench requires nightly">Run cargo bench with nightly toolchain.</knowledge>"#;
        let entries = parse_knowledge_sigils(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Cargo bench requires nightly");
        assert_eq!(entries[0].tags, vec!["testing", "cargo"]);
        assert_eq!(entries[0].body, "Run cargo bench with nightly toolchain.");
    }

    #[test]
    fn test_parse_knowledge_sigils_multiple() {
        let text = concat!(
            r#"<knowledge tags="rust,testing" title="First entry">Body of first entry.</knowledge>"#,
            "\n",
            r#"<knowledge tags="database,sqlite" title="Second entry">Body of second entry.</knowledge>"#
        );
        let entries = parse_knowledge_sigils(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "First entry");
        assert_eq!(entries[1].title, "Second entry");
        assert_eq!(entries[1].tags, vec!["database", "sqlite"]);
    }

    #[test]
    fn test_parse_knowledge_sigil_missing_tags() {
        let text = r#"<knowledge title="No tags entry">Some body content here.</knowledge>"#;
        let entries = parse_knowledge_sigils(text);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_parse_knowledge_sigil_missing_title() {
        let text = r#"<knowledge tags="rust,testing">Some body content here.</knowledge>"#;
        let entries = parse_knowledge_sigils(text);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_parse_knowledge_sigil_empty_body() {
        let text = r#"<knowledge tags="rust" title="Empty body entry"></knowledge>"#;
        let entries = parse_knowledge_sigils(text);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_parse_knowledge_sigil_tags_normalized() {
        let text = r#"<knowledge tags="Rust, Testing, CARGO" title="Tag normalization">Body content.</knowledge>"#;
        let entries = parse_knowledge_sigils(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tags, vec!["rust", "testing", "cargo"]);
    }

    #[test]
    fn test_parse_knowledge_sigil_attributes_reversed_order() {
        // Attributes in reversed order (title before tags)
        let text = r#"<knowledge title="Reversed attrs" tags="foo,bar">Some body.</knowledge>"#;
        let entries = parse_knowledge_sigils(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Reversed attrs");
        assert_eq!(entries[0].tags, vec!["foo", "bar"]);
    }

    // --- extract_attribute tests ---

    #[test]
    fn test_extract_attribute() {
        let tag_content = r#"tags="foo,bar" title="My Title""#;
        assert_eq!(
            extract_attribute(tag_content, "tags"),
            Some("foo,bar".to_string())
        );
        assert_eq!(
            extract_attribute(tag_content, "title"),
            Some("My Title".to_string())
        );
    }

    #[test]
    fn test_extract_attribute_missing() {
        let tag_content = r#"tags="foo,bar""#;
        assert_eq!(extract_attribute(tag_content, "title"), None);
    }

    #[test]
    fn test_extract_attribute_empty_value() {
        let tag_content = r#"tags="" title="Some title""#;
        assert_eq!(extract_attribute(tag_content, "tags"), Some("".to_string()));
    }
}

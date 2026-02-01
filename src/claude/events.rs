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

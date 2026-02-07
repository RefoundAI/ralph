//! NDJSON line parser for Claude's stream output.

use super::events::*;
use anyhow::Result;

/// Parse a single line of NDJSON into an event.
pub fn parse_line(line: &str) -> Result<Option<Event>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }

    let raw: RawEvent = serde_json::from_str(line)?;
    Ok(Some(parse_event(raw)))
}

fn parse_event(raw: RawEvent) -> Event {
    match raw.event_type.as_deref() {
        Some("assistant") => {
            let model = raw.message.as_ref().and_then(|m| m.model.clone());
            let content = raw
                .message
                .and_then(|m| m.content)
                .unwrap_or_default()
                .into_iter()
                .map(parse_content_block)
                .collect();
            Event::Assistant(Assistant { model, content })
        }
        Some("user") => {
            let tool_results: Vec<ToolResult> = raw
                .message
                .and_then(|m| m.content)
                .unwrap_or_default()
                .into_iter()
                .filter(|b| b.block_type.as_deref() == Some("tool_result"))
                .map(parse_tool_result)
                .filter(|r| r.is_error)
                .collect();

            if tool_results.is_empty() {
                Event::Unknown
            } else {
                Event::ToolErrors(tool_results)
            }
        }
        Some("result") => {
            let next_model_hint = raw
                .result
                .as_deref()
                .and_then(super::events::parse_next_model_hint);
            let task_done = raw
                .result
                .as_deref()
                .and_then(super::events::parse_task_done);
            let task_failed = raw
                .result
                .as_deref()
                .and_then(super::events::parse_task_failed);

            // If both task_done and task_failed are present, task_done wins (optimistic)
            let final_task_done = if task_done.is_some() {
                task_done
            } else {
                None
            };
            let final_task_failed = if final_task_done.is_none() {
                task_failed
            } else {
                None
            };

            Event::Result(ResultEvent {
                result: raw.result,
                duration_ms: raw.duration_ms.unwrap_or(0),
                total_cost_usd: raw.total_cost_usd.unwrap_or(0.0),
                next_model_hint,
                task_done: final_task_done,
                task_failed: final_task_failed,
            })
        }
        _ => Event::Unknown,
    }
}

fn parse_content_block(block: RawContentBlock) -> ContentBlock {
    match block.block_type.as_deref() {
        Some("text") => ContentBlock::Text {
            text: block.text.unwrap_or_default(),
        },
        Some("thinking") => ContentBlock::Thinking {
            thinking: block.thinking.unwrap_or_default(),
        },
        Some("tool_use") => ContentBlock::ToolUse {
            id: block.id.unwrap_or_default(),
            name: block.name.unwrap_or_default(),
            input: block.input.unwrap_or_default(),
        },
        _ => ContentBlock::Unknown,
    }
}

fn parse_tool_result(block: RawContentBlock) -> ToolResult {
    ToolResult {
        tool_use_id: block.tool_use_id.unwrap_or_default(),
        content: block.content.unwrap_or_default(),
        is_error: block.is_error.unwrap_or(false),
    }
}

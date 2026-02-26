//! Sigil extraction from accumulated agent output text.
//!
//! Migrated from `src/claude/events.rs` with a new `extract_sigils()` combinator
//! that calls all individual parsers and assembles a `SigilResult`.

use crate::acp::types::{KnowledgeSigil, SigilResult};

/// Sigil for the COMPLETE promise.
#[allow(dead_code)]
pub const COMPLETE_SIGIL: &str = "<promise>COMPLETE</promise>";

/// Sigil for the FAILURE promise.
pub const FAILURE_SIGIL: &str = "<promise>FAILURE</promise>";

/// Valid model names for the `<next-model>` sigil.
const VALID_MODELS: &[&str] = &["opus", "sonnet", "haiku"];

/// Valid phase names for the `<phase-complete>` sigil.
const VALID_PHASES: &[&str] = &["spec", "plan", "build"];

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

/// Parse the `<phase-complete>spec|plan|build</phase-complete>` sigil from result text.
///
/// Returns `Some(phase)` if a valid phase name is found between the tags,
/// `None` if the sigil is absent or contains an invalid phase name.
pub fn parse_phase_complete(text: &str) -> Option<String> {
    let start_tag = "<phase-complete>";
    let end_tag = "</phase-complete>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let phase = text[content_start..content_start + end_idx].trim();

    if VALID_PHASES.contains(&phase) {
        Some(phase.to_string())
    } else {
        None
    }
}

/// Check for `<tasks-created>` sigil in result text.
///
/// Returns `true` if the sigil is present (self-closing or with empty content).
pub fn parse_tasks_created(text: &str) -> bool {
    text.contains("<tasks-created>")
        || text.contains("<tasks-created/>")
        || text.contains("<tasks-created />")
}

/// Sigils extracted from interactive/streaming session output.
///
/// A lighter-weight sigil result for interactive flows (feature create, task create)
/// where only phase completion and task creation signals are relevant.
#[derive(Debug, Default)]
pub struct InteractiveSigils {
    /// Phase name from `<phase-complete>spec|plan|build</phase-complete>`.
    pub phase_complete: Option<String>,
    /// True if `<tasks-created>` was found.
    pub tasks_created: bool,
}

/// Extract interactive-flow sigils from accumulated agent output text.
pub fn extract_interactive_sigils(text: &str) -> InteractiveSigils {
    InteractiveSigils {
        phase_complete: parse_phase_complete(text),
        tasks_created: parse_tasks_created(text),
    }
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

/// Extract all sigils from accumulated agent output text.
///
/// Calls all individual sigil parsers and assembles the results into a `SigilResult`.
pub fn extract_sigils(text: &str) -> SigilResult {
    SigilResult {
        task_done: parse_task_done(text),
        task_failed: parse_task_failed(text),
        next_model_hint: parse_next_model_hint(text),
        journal_notes: parse_journal_sigil(text),
        knowledge_entries: parse_knowledge_sigils(text),
        is_complete: text.contains(COMPLETE_SIGIL),
        is_failure: text.contains(FAILURE_SIGIL),
    }
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

    // --- SigilResult next_model_hint integration tests (adapted from ResultEvent tests) ---

    #[test]
    fn result_event_default_has_no_hint() {
        // Equivalent: extract_sigils on empty text has no model hint
        let result = extract_sigils("");
        assert!(result.next_model_hint.is_none());
    }

    #[test]
    fn result_event_with_hint() {
        // Equivalent: extract_sigils with next-model sigil populates the hint
        let result = extract_sigils("done <next-model>opus</next-model>");
        assert_eq!(result.next_model_hint, Some("opus".to_string()));
        assert!(!result.is_complete);
        assert!(!result.is_failure);
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

    // --- extract_sigils() tests ---

    #[test]
    fn test_extract_sigils_all_present() {
        let text = concat!(
            "<task-done>t-abc123</task-done>\n",
            "<next-model>sonnet</next-model>\n",
            "<journal>Some iteration notes.</journal>\n",
            r#"<knowledge tags="rust,testing" title="A knowledge entry">Knowledge body here.</knowledge>"#,
            "\n",
            "<promise>COMPLETE</promise>",
        );
        let result = extract_sigils(text);
        assert_eq!(result.task_done, Some("t-abc123".to_string()));
        assert_eq!(result.task_failed, None);
        assert_eq!(result.next_model_hint, Some("sonnet".to_string()));
        assert_eq!(
            result.journal_notes,
            Some("Some iteration notes.".to_string())
        );
        assert_eq!(result.knowledge_entries.len(), 1);
        assert_eq!(result.knowledge_entries[0].title, "A knowledge entry");
        assert!(result.is_complete);
        assert!(!result.is_failure);
    }

    #[test]
    fn test_extract_sigils_none_present() {
        let text = "Just some plain agent output with no sigils at all.";
        let result = extract_sigils(text);
        assert_eq!(result.task_done, None);
        assert_eq!(result.task_failed, None);
        assert_eq!(result.next_model_hint, None);
        assert_eq!(result.journal_notes, None);
        assert!(result.knowledge_entries.is_empty());
        assert!(!result.is_complete);
        assert!(!result.is_failure);
    }

    #[test]
    fn test_extract_sigils_partial() {
        // Only task-done and journal sigils present
        let text = "Work done.\n<task-done>t-xyz789</task-done>\n<journal>Key decision: used HashMap.</journal>";
        let result = extract_sigils(text);
        assert_eq!(result.task_done, Some("t-xyz789".to_string()));
        assert_eq!(result.task_failed, None);
        assert_eq!(result.next_model_hint, None);
        assert_eq!(
            result.journal_notes,
            Some("Key decision: used HashMap.".to_string())
        );
        assert!(result.knowledge_entries.is_empty());
        assert!(!result.is_complete);
        assert!(!result.is_failure);
    }

    // --- parse_phase_complete tests ---

    #[test]
    fn parse_phase_complete_spec() {
        let text = "I've written the spec.\n<phase-complete>spec</phase-complete>";
        assert_eq!(parse_phase_complete(text), Some("spec".to_string()));
    }

    #[test]
    fn parse_phase_complete_plan() {
        let text = "<phase-complete>plan</phase-complete>";
        assert_eq!(parse_phase_complete(text), Some("plan".to_string()));
    }

    #[test]
    fn parse_phase_complete_build() {
        let text = "Tasks created.\n<phase-complete>build</phase-complete>\nDone.";
        assert_eq!(parse_phase_complete(text), Some("build".to_string()));
    }

    #[test]
    fn parse_phase_complete_with_whitespace() {
        let text = "<phase-complete> spec </phase-complete>";
        assert_eq!(parse_phase_complete(text), Some("spec".to_string()));
    }

    #[test]
    fn parse_phase_complete_invalid_phase() {
        let text = "<phase-complete>review</phase-complete>";
        assert_eq!(parse_phase_complete(text), None);
    }

    #[test]
    fn parse_phase_complete_absent() {
        let text = "No phase sigil here.";
        assert_eq!(parse_phase_complete(text), None);
    }

    #[test]
    fn parse_phase_complete_empty() {
        let text = "<phase-complete></phase-complete>";
        assert_eq!(parse_phase_complete(text), None);
    }

    #[test]
    fn parse_phase_complete_malformed_no_closing() {
        let text = "<phase-complete>spec";
        assert_eq!(parse_phase_complete(text), None);
    }

    // --- parse_tasks_created tests ---

    #[test]
    fn parse_tasks_created_with_closing_tag() {
        let text = "Done.\n<tasks-created></tasks-created>";
        assert!(parse_tasks_created(text));
    }

    #[test]
    fn parse_tasks_created_self_closing() {
        let text = "Task created: t-abc123\n<tasks-created/>";
        assert!(parse_tasks_created(text));
    }

    #[test]
    fn parse_tasks_created_self_closing_with_space() {
        let text = "<tasks-created />";
        assert!(parse_tasks_created(text));
    }

    #[test]
    fn parse_tasks_created_absent() {
        let text = "No sigils here.";
        assert!(!parse_tasks_created(text));
    }

    // --- extract_interactive_sigils tests ---

    #[test]
    fn test_extract_interactive_sigils_phase_complete() {
        let text = "Spec written.\n<phase-complete>spec</phase-complete>";
        let result = extract_interactive_sigils(text);
        assert_eq!(result.phase_complete, Some("spec".to_string()));
        assert!(!result.tasks_created);
    }

    #[test]
    fn test_extract_interactive_sigils_tasks_created() {
        let text = "Created task t-abc123.\n<tasks-created></tasks-created>";
        let result = extract_interactive_sigils(text);
        assert!(result.tasks_created);
        assert_eq!(result.phase_complete, None);
    }

    #[test]
    fn test_extract_interactive_sigils_both() {
        let text = "<phase-complete>build</phase-complete>\n<tasks-created/>";
        let result = extract_interactive_sigils(text);
        assert_eq!(result.phase_complete, Some("build".to_string()));
        assert!(result.tasks_created);
    }

    #[test]
    fn test_extract_interactive_sigils_none() {
        let text = "Just regular output.";
        let result = extract_interactive_sigils(text);
        assert_eq!(result.phase_complete, None);
        assert!(!result.tasks_created);
    }
}

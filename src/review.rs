//! Iterative review agent for spec and plan documents.

use anyhow::Result;

use crate::output::formatter;

/// Maximum number of review rounds before stopping.
const MAX_REVIEW_ROUNDS: u32 = 5;

/// What kind of document is being reviewed.
#[derive(Debug, Clone, Copy)]
pub enum DocumentKind {
    Spec,
    Plan,
}

impl DocumentKind {
    pub fn label(&self) -> &'static str {
        match self {
            DocumentKind::Spec => "spec",
            DocumentKind::Plan => "plan",
        }
    }
}

/// Run an iterative review loop on a spec or plan document.
///
/// Spawns autonomous Claude sessions that review and improve the document
/// until either a review agent finds no major issues or the maximum number
/// of rounds is reached.
pub fn review_document(
    document_path: &str,
    kind: DocumentKind,
    feature_name: &str,
    spec_content: Option<&str>,
    project_context: &str,
) -> Result<u32> {
    let label = kind.label();

    formatter::print_review_start(label, feature_name);

    for round in 1..=MAX_REVIEW_ROUNDS {
        formatter::print_review_round(round, MAX_REVIEW_ROUNDS, label);

        let result = run_review_agent(
            document_path,
            kind,
            feature_name,
            spec_content,
            project_context,
            round,
        )?;

        if result.passed {
            formatter::print_review_result(round, true, "", label);
            formatter::print_review_complete(label, feature_name, round);
            return Ok(round);
        }

        formatter::print_review_result(round, false, &result.changes_summary, label);
    }

    formatter::print_review_max_rounds(label, feature_name, MAX_REVIEW_ROUNDS);
    Ok(MAX_REVIEW_ROUNDS)
}

/// Result of a single review round.
struct ReviewResult {
    passed: bool,
    changes_summary: String,
}

/// Run a single review agent on the document.
fn run_review_agent(
    document_path: &str,
    kind: DocumentKind,
    feature_name: &str,
    spec_content: Option<&str>,
    project_context: &str,
    round: u32,
) -> Result<ReviewResult> {
    let system_prompt = build_review_prompt(
        document_path,
        kind,
        feature_name,
        spec_content,
        project_context,
        round,
    );

    let args = vec![
        "--print".to_string(),
        "--verbose".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--no-session-persistence".to_string(),
        "--model".to_string(),
        "opus".to_string(),
        "--allowed-tools".to_string(),
        "Bash Edit Write Read Glob Grep".to_string(),
        "--system-prompt".to_string(),
        system_prompt,
        format!(
            "Review the {} at {} and improve it.",
            kind.label(),
            document_path
        ),
    ];

    let result = crate::claude::client::run_direct_with_args(&args, None)?;

    if let Some(ref r) = result {
        if let Some(ref text) = r.result {
            if parse_review_pass(text) {
                return Ok(ReviewResult {
                    passed: true,
                    changes_summary: String::new(),
                });
            }
            if let Some(summary) = parse_review_changes(text) {
                return Ok(ReviewResult {
                    passed: false,
                    changes_summary: summary,
                });
            }
        }
    }

    // No sigil found — treat as pass to prevent infinite loops
    Ok(ReviewResult {
        passed: true,
        changes_summary: String::new(),
    })
}

fn build_review_prompt(
    document_path: &str,
    kind: DocumentKind,
    feature_name: &str,
    spec_content: Option<&str>,
    project_context: &str,
    round: u32,
) -> String {
    let label = kind.label();

    let kind_specific_criteria = match kind {
        DocumentKind::Spec => r#"## Spec-Specific Criteria

- **Completeness**: Does the spec cover all functional and non-functional requirements?
- **Testability**: Are acceptance criteria concrete and verifiable?
- **Precision**: Are data models, APIs, and schemas defined with enough detail for implementation?
- **Edge cases**: Are error handling, boundary conditions, and failure modes addressed?
- **Dependencies**: Are external dependencies, integrations, and assumptions documented?"#,
        DocumentKind::Plan => r#"## Plan-Specific Criteria

- **Completeness**: Does the plan cover all spec requirements?
- **Ordering**: Are implementation phases in a logical, dependency-respecting order?
- **Task granularity**: Are phases broken into right-sized, implementable chunks?
- **Verification**: Does each phase have clear verification/acceptance criteria?
- **Risk coverage**: Are risk areas, failure modes, and mitigation strategies identified?
- **Spec alignment**: Does the plan reference specific spec sections?"#,
    };

    let spec_section = match spec_content {
        Some(spec) => format!("## Feature Specification\n\n{}\n", spec),
        None => String::new(),
    };

    let round_note = if round > 1 {
        format!(
            "\n**This is review round {}.** Previous rounds made changes. \
             Focus on remaining issues only — do not re-apply changes that are already present.\n",
            round
        )
    } else {
        String::new()
    };

    format!(
        r#"You are a document review agent for Ralph. Your job is to review and improve a feature {label} document.

## Feature

**Name:** {feature_name}
**Document:** `{document_path}`
{round_note}
{spec_section}
{project_context}

## Your Task

1. Read the {label} document at `{document_path}`
2. Read relevant source code to understand the project's existing patterns, conventions, and architecture
3. Assess the document for completeness, robustness, and clarity
4. Make ALL recommended improvements directly to the file using Edit/Write tools
5. Emit a sigil indicating the outcome

{kind_specific_criteria}

## General Quality Criteria

- **Clarity**: Is the writing unambiguous? Could an AI agent implement from this without questions?
- **Structure**: Is it well-organized with clear sections and consistent formatting?
- **Consistency**: Does it align with the project's existing patterns and conventions (check CLAUDE.md, existing code)?
- **Feasibility**: Are the proposed approaches realistic given the codebase?

## Rules

- You MUST read the document first before making any assessment
- Explore the codebase (using Read, Glob, Grep) to validate that proposals are feasible
- Make changes directly — do not just list suggestions
- Preserve the author's intent — improve clarity and coverage, do not redesign
- If the document is already comprehensive and clear, do not make changes just for the sake of it
- Focus on substantive issues, not minor stylistic preferences

## Sigils

After your review, emit exactly one of these sigils:

- `<review-pass/>` — The document is comprehensive, clear, and ready for use. No major issues found.
- `<review-changes>summary of what you changed</review-changes>` — You made substantive improvements. Briefly describe what changed.

You MUST emit one of these sigils. If you made any changes to the file, use `<review-changes>`. If the document was already good, use `<review-pass/>`."#,
        label = label,
        feature_name = feature_name,
        document_path = document_path,
        round_note = round_note,
        spec_section = spec_section,
        project_context = project_context,
        kind_specific_criteria = kind_specific_criteria,
    )
}

/// Parse the `<review-pass/>` sigil from result text.
fn parse_review_pass(text: &str) -> bool {
    text.contains("<review-pass/>")
}

/// Parse the `<review-changes>...</review-changes>` sigil from result text.
fn parse_review_changes(text: &str) -> Option<String> {
    let start_tag = "<review-changes>";
    let end_tag = "</review-changes>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let summary = text[content_start..content_start + end_idx].trim();

    if summary.is_empty() {
        None
    } else {
        Some(summary.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_review_pass() {
        assert!(parse_review_pass("All good <review-pass/> done"));
        assert!(!parse_review_pass("No pass sigil here"));
    }

    #[test]
    fn test_parse_review_changes() {
        let text = "<review-changes>Added edge case handling section</review-changes>";
        assert_eq!(
            parse_review_changes(text),
            Some("Added edge case handling section".to_string())
        );
    }

    #[test]
    fn test_parse_review_changes_empty() {
        assert_eq!(
            parse_review_changes("<review-changes></review-changes>"),
            None
        );
    }

    #[test]
    fn test_parse_review_changes_absent() {
        assert_eq!(parse_review_changes("no sigil"), None);
    }

    #[test]
    fn test_parse_review_changes_multiline() {
        let text =
            "<review-changes>\nAdded error handling.\nImproved data model.\n</review-changes>";
        let result = parse_review_changes(text).unwrap();
        assert!(result.contains("Added error handling."));
        assert!(result.contains("Improved data model."));
    }

    #[test]
    fn test_document_kind_labels() {
        assert_eq!(DocumentKind::Spec.label(), "spec");
        assert_eq!(DocumentKind::Plan.label(), "plan");
    }

    #[test]
    fn test_review_prompt_spec_criteria() {
        let prompt = build_review_prompt("/tmp/spec.md", DocumentKind::Spec, "test", None, "", 1);
        assert!(prompt.contains("Completeness"));
        assert!(prompt.contains("Testability"));
        assert!(prompt.contains("<review-pass/>"));
        assert!(prompt.contains("<review-changes>"));
    }

    #[test]
    fn test_review_prompt_plan_criteria() {
        let prompt = build_review_prompt(
            "/tmp/plan.md",
            DocumentKind::Plan,
            "test",
            Some("Spec content here"),
            "",
            1,
        );
        assert!(prompt.contains("Ordering"));
        assert!(prompt.contains("Task granularity"));
        assert!(prompt.contains("Spec content here"));
    }

    #[test]
    fn test_review_prompt_round_note() {
        let r1 = build_review_prompt("/tmp/spec.md", DocumentKind::Spec, "test", None, "", 1);
        assert!(!r1.contains("review round"));

        let r2 = build_review_prompt("/tmp/spec.md", DocumentKind::Spec, "test", None, "", 2);
        assert!(r2.contains("review round 2"));
    }

    #[test]
    fn test_review_prompt_includes_context() {
        let ctx = "## Project Context\n\nTest content";
        let prompt =
            build_review_prompt("/tmp/spec.md", DocumentKind::Spec, "test", None, ctx, 1);
        assert!(prompt.contains("Test content"));
    }
}

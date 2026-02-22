//! Verification agent for autonomous task validation.

use anyhow::Result;

use crate::acp;
use crate::config::Config;
use crate::dag::Task;

/// Result of task verification.
#[derive(Debug)]
pub struct VerificationResult {
    pub passed: bool,
    pub reason: String,
}

/// Verify a completed task against its spec and plan.
///
/// Spawns a read-only ACP session that can run tests and inspect code
/// but cannot modify the codebase (write_text_file is rejected).
pub async fn verify_task(
    config: &Config,
    task: &Task,
    spec_content: Option<&str>,
    plan_content: Option<&str>,
    _log_file: &str,
) -> Result<VerificationResult> {
    let system_prompt = build_verification_prompt(task, spec_content, plan_content);

    let result = acp::connection::run_autonomous(
        &config.agent_command,
        &config.project_root,
        &system_prompt,
        "Verify the task.",
        true, // read_only = true
        Some(&config.current_model),
        acp::connection::SessionRestrictions {
            allow_terminal: true, // verification needs to run tests
            ..Default::default()
        },
    )
    .await?;

    let text = &result.full_text;

    // Parse verification sigils from the accumulated agent text
    if parse_verify_pass(text) {
        return Ok(VerificationResult {
            passed: true,
            reason: "Verification passed".to_string(),
        });
    }
    if let Some(reason) = parse_verify_fail(text) {
        return Ok(VerificationResult {
            passed: false,
            reason,
        });
    }

    // No sigil found — treat as failure
    Ok(VerificationResult {
        passed: false,
        reason: "Verification agent did not emit a verification sigil".to_string(),
    })
}

fn build_verification_prompt(
    task: &Task,
    spec_content: Option<&str>,
    plan_content: Option<&str>,
) -> String {
    let mut prompt = String::new();

    prompt.push_str("You are a verification agent for Ralph. Your job is to verify that a task was completed correctly.\n\n");
    prompt.push_str("## Task to Verify\n\n");
    prompt.push_str(&format!("**ID:** {}\n", task.id));
    prompt.push_str(&format!("**Title:** {}\n", task.title));
    prompt.push_str(&format!("**Description:** {}\n\n", task.description));

    if let Some(spec) = spec_content {
        prompt.push_str("## Specification\n\n");
        prompt.push_str(spec);
        prompt.push_str("\n\n");
    }

    if let Some(plan) = plan_content {
        prompt.push_str("## Plan\n\n");
        prompt.push_str(plan);
        prompt.push_str("\n\n");
    }

    prompt.push_str(
        r#"## Instructions

1. Read the relevant source files to check if the task was implemented correctly
2. Run any applicable tests (cargo test, etc.)
3. Check that acceptance criteria from the task description are met
4. Do NOT modify any files — you are read-only

## Sigils

After verification, emit exactly one of these sigils:

- `<verify-pass/>` — The task was implemented correctly
- `<verify-fail>reason</verify-fail>` — The task has issues (explain why)
"#,
    );

    prompt
}

/// Parse the `<verify-pass/>` sigil from result text.
pub fn parse_verify_pass(text: &str) -> bool {
    text.contains("<verify-pass/>")
}

/// Parse the `<verify-fail>...</verify-fail>` sigil from result text.
pub fn parse_verify_fail(text: &str) -> Option<String> {
    let start_tag = "<verify-fail>";
    let end_tag = "</verify-fail>";

    let start_idx = text.find(start_tag)?;
    let content_start = start_idx + start_tag.len();
    let end_idx = text[content_start..].find(end_tag)?;
    let reason = text[content_start..content_start + end_idx].trim();

    if reason.is_empty() {
        None
    } else {
        Some(reason.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_verify_pass() {
        assert!(parse_verify_pass("All looks good <verify-pass/> done"));
        assert!(!parse_verify_pass("No pass sigil here"));
    }

    #[test]
    fn test_parse_verify_fail() {
        let text = "<verify-fail>Tests are failing in module X</verify-fail>";
        assert_eq!(
            parse_verify_fail(text),
            Some("Tests are failing in module X".to_string())
        );
    }

    #[test]
    fn test_parse_verify_fail_empty() {
        assert_eq!(parse_verify_fail("<verify-fail></verify-fail>"), None);
    }

    #[test]
    fn test_parse_verify_fail_absent() {
        assert_eq!(parse_verify_fail("no sigil"), None);
    }
}

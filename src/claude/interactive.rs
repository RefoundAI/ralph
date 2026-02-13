//! Claude CLI invocation for prompt, specs, and plan subcommands.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::{Command, Stdio};

/// Launch Claude in interactive mode with a system prompt.
///
/// Spawns `claude` without `--print` or `--output-format`, so the user
/// gets a full interactive session. The process inherits stdin/stdout/stderr.
/// Returns when Claude exits.
pub fn run_interactive(system_prompt: &str) -> Result<()> {
    let status = Command::new("claude")
        .arg("--system-prompt")
        .arg(system_prompt)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to spawn claude process. Is `claude` installed and in PATH?")?;

    if !status.success() {
        anyhow::bail!("claude exited with status: {}", status);
    }

    Ok(())
}

/// Run Claude in print mode with streaming output and no tools.
///
/// Spawns `claude --print --output-format stream-json --tools ""` and streams
/// NDJSON events to the terminal in real time (thinking, text).
/// Tools are disabled so Claude outputs text only.
/// Returns the final result text for further processing.
pub fn run_streaming(system_prompt: &str, user_message: &str, model: &str) -> Result<String> {
    let mut child = Command::new("claude")
        .arg("--print")
        .arg("--verbose")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--include-partial-messages")
        .arg("--model")
        .arg(model)
        .arg("--tools")
        .arg("")
        .arg("--system-prompt")
        .arg(system_prompt)
        .arg(user_message)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn claude process. Is `claude` installed and in PATH?")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;
    let stderr_thread = super::client::drain_stderr(stderr);

    let result = super::client::stream_output(stdout, None, true)?;

    let status = child.wait().context("Failed to wait for claude process")?;
    let stderr_output = stderr_thread.join().unwrap_or_default();

    if !status.success() {
        if stderr_output.is_empty() {
            anyhow::bail!("claude exited with status: {}", status);
        } else {
            anyhow::bail!(
                "claude exited with status: {}\nstderr: {}",
                status,
                stderr_output
            );
        }
    } else if !stderr_output.is_empty() {
        eprintln!("{}", stderr_output);
    }

    result
        .and_then(|r| r.result)
        .ok_or_else(|| anyhow::anyhow!("Claude did not return a result"))
}

/// Structured output from the plan command.
#[derive(Debug, Deserialize)]
pub struct PlanOutput {
    pub tasks: Vec<PlanTask>,
}

/// A single task in the plan output.
#[derive(Debug, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub priority: i32,
}

/// Extract and parse JSON from Claude's plan output.
///
/// Handles JSON wrapped in ```json code blocks, plain ``` blocks,
/// or raw JSON objects.
pub fn extract_plan_json(output: &str) -> Result<PlanOutput> {
    let json_str = if let Some(start) = output.find("```json") {
        let content_start = start + "```json".len();
        let end = output[content_start..]
            .find("```")
            .ok_or_else(|| anyhow::anyhow!("Found ```json block but no closing ```"))?;
        &output[content_start..content_start + end]
    } else if let Some(start_pos) = output.find('{') {
        // Find the matching closing brace
        let mut depth = 0i32;
        let mut end = start_pos;
        for (i, ch) in output[start_pos..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start_pos + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth != 0 {
            anyhow::bail!("Unbalanced braces in Claude's output");
        }
        &output[start_pos..end]
    } else {
        anyhow::bail!("No JSON found in Claude's output. Raw output:\n{}", output);
    };

    serde_json::from_str(json_str.trim())
        .with_context(|| format!("Failed to parse plan JSON:\n{}", json_str.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_raw() {
        let output = r#"{"tasks": [{"id": "1", "title": "Test", "description": "Desc", "parent_id": null, "depends_on": [], "priority": 0}]}"#;
        let plan = extract_plan_json(output).unwrap();
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].title, "Test");
    }

    #[test]
    fn extract_json_from_code_block() {
        let output = "Here is the plan:\n```json\n{\"tasks\": [{\"id\": \"1\", \"title\": \"Test\", \"description\": \"Desc\", \"parent_id\": null}]}\n```\nDone.";
        let plan = extract_plan_json(output).unwrap();
        assert_eq!(plan.tasks.len(), 1);
    }

    #[test]
    fn extract_json_with_prose_around_it() {
        let output = "Let me think about this...\n{\"tasks\": [{\"id\": \"1\", \"title\": \"Do thing\", \"description\": \"Details\", \"parent_id\": null}]}\nHope that helps!";
        let plan = extract_plan_json(output).unwrap();
        assert_eq!(plan.tasks[0].title, "Do thing");
    }

    #[test]
    fn extract_json_no_json_fails() {
        let output = "This has no JSON at all.";
        assert!(extract_plan_json(output).is_err());
    }

    #[test]
    fn extract_json_with_dependencies() {
        let output = r#"{"tasks": [
            {"id": "1", "title": "First", "description": "D1", "parent_id": null},
            {"id": "2", "title": "Second", "description": "D2", "parent_id": null, "depends_on": ["1"]}
        ]}"#;
        let plan = extract_plan_json(output).unwrap();
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[1].depends_on, vec!["1"]);
    }

    #[test]
    fn extract_json_with_parent_hierarchy() {
        let output = r#"{"tasks": [
            {"id": "1", "title": "Parent", "description": "P", "parent_id": null},
            {"id": "2", "title": "Child", "description": "C", "parent_id": "1"}
        ]}"#;
        let plan = extract_plan_json(output).unwrap();
        assert_eq!(plan.tasks[1].parent_id, Some("1".to_string()));
    }
}

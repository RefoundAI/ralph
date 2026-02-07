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

/// Build the system prompt for `ralph prompt`.
///
/// Instructs Claude to co-author a prompt file with the user, writing the
/// result to the given prompts directory with `YYYY-MM-DD-<slug>.md` naming.
pub fn build_prompt_system_prompt(prompts_dir: &str) -> String {
    format!(
        r#"You are helping the user write a prompt file for Ralph, an autonomous AI agent loop.

## Your Role

Co-author a clear, actionable prompt file with the user. The prompt file will be used as instructions for an AI agent that works autonomously in a coding loop.

## Guidelines

- Ask the user what they want to accomplish
- Help refine the requirements into clear, specific instructions
- The prompt should describe WHAT to build, not HOW (the agent will figure out implementation)
- Include acceptance criteria or success conditions when possible
- Keep the prompt focused — one logical unit of work per file

## Output

When the prompt is ready, write it to a file in the prompts directory:

- **Directory:** `{prompts_dir}`
- **Naming format:** `YYYY-MM-DD-<slug>.md` (e.g., `2025-01-15-add-auth.md`)
- Use today's date and a short descriptive slug
- Write the file using the Write tool

## Prompt File Format

The prompt file should be markdown with:
- A clear title (H1)
- Brief description of the task
- Specific requirements or acceptance criteria
- Any constraints or notes the agent should know"#,
        prompts_dir = prompts_dir
    )
}

/// Build the system prompt for `ralph plan`.
///
/// Instructs Claude to decompose a prompt into a task DAG and output
/// structured JSON that can be parsed and inserted into the task database.
pub fn build_plan_system_prompt(specs_content: &str) -> String {
    format!(
        r#"You are a planning agent for Ralph, an autonomous AI agent loop that drives Claude Code.

Decompose the user's prompt into a task DAG. Each task runs in a separate, isolated Claude Code session — one task per iteration. Sessions share the filesystem but not conversation history.

## How Ralph Executes Tasks

- Picks ONE ready leaf task per iteration, assigns it to a fresh Claude Code session
- The session gets: task title, description, parent context, completed prerequisite summaries
- The session does NOT see other tasks, the full DAG, or output from non-prerequisite tasks
- Only leaf tasks execute — parent tasks auto-complete when all children complete

## Decomposition Rules

1. **Right-size tasks**: One coherent unit of work per task. Good tasks touch 1-3 files. Too small wastes iterations. Too large risks failure.

2. **Concise but sufficient descriptions**: 3-8 sentences. The agent has full codebase access and can read any file, so point it to relevant files by path rather than inlining content. Focus on WHAT to do and WHERE, not exhaustive HOW. Include: what to change, which files to read/modify, key constraints, and how to verify.

3. **Reference, don't inline**: Say "Read the schema in `src/dag/db.rs` and add a new `metrics` table following the same pattern" — NOT a paragraph reproducing the schema. The agent can read files. Point to them.

4. **Parent tasks for grouping**: Parents are never executed — they organize related children. Use for logical epics.

5. **depends_on for real data dependencies**: Only when task B needs artifacts from task A. Independent tasks should be parallelizable. Don't add ordering dependencies just for preference.

6. **Spec/doc tasks**: Decompose by section. First task creates the file skeleton, subsequent tasks fill in one section each and depend on the skeleton task.

7. **Foundation first**: Schemas and types before the code that uses them. Use depends_on to enforce.

## Available Specifications

{specs_content}

## Output Format

Output ONLY a JSON object. No prose, no markdown fences — just raw JSON:

{{
  "tasks": [
    {{
      "id": "1",
      "title": "Short imperative title",
      "description": "Concise description: what to do, which files, how to verify.",
      "parent_id": null,
      "depends_on": [],
      "priority": 0
    }}
  ]
}}

Fields:
- `id`: Sequential ("1", "2", ...) — replaced with real IDs on insert
- `title`: Imperative, brief (e.g., "Add sessions table to db.rs")
- `description`: 3-8 sentences. Reference files by path. What + where + verify.
- `parent_id`: Parent task ID for grouping, or null
- `depends_on`: Task IDs that must complete first (real dependencies only)
- `priority`: 0 = highest, higher = lower"#,
        specs_content = specs_content
    )
}

/// Build the system prompt for `ralph specs`.
///
/// Instructs Claude to co-author specification documents with the user, writing
/// them to the configured specs directories.
pub fn build_specs_system_prompt(specs_dirs: &[String]) -> String {
    let dirs_list = specs_dirs
        .iter()
        .map(|d| format!("- `{}`", d))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You are helping the user write specification documents for Ralph, an autonomous AI agent loop.

## Your Role

Co-author comprehensive specification documents with the user. These specs will guide the AI agent when implementing features, fixing bugs, or performing other coding tasks.

## Guidelines

- Interview the user to understand:
  - What feature, module, or system they want to specify
  - Technical requirements and constraints
  - Expected behavior and edge cases
  - Testing requirements
  - Dependencies and integration points

- Specifications should be:
  - **Detailed**: Include enough information for an agent to implement without ambiguity
  - **Structured**: Use markdown sections (Requirements, Architecture, API, Testing, etc.)
  - **Concrete**: Provide examples, schemas, and expected behaviors
  - **Testable**: Define clear acceptance criteria

## Output

When the spec is ready, write it to one of the specs directories:

**Available directories:**
{dirs_list}

- **Naming format:** `<feature-or-module>.md` (e.g., `authentication.md`, `api-endpoints.md`)
- Use descriptive names that match the feature or module
- Write the file using the Write tool

## Spec Document Format

A good specification should include:

1. **Overview** (H1) - Brief summary of what this spec covers
2. **Requirements** - Functional and non-functional requirements
3. **Architecture** - High-level design, components, data flow
4. **API / Interface** - Function signatures, endpoints, contracts
5. **Data Models** - Schemas, types, validation rules
6. **Testing** - Test cases, edge cases, acceptance criteria
7. **Dependencies** - External libraries, services, or modules
8. **Open Questions** - Any unresolved items (if applicable)"#,
        dirs_list = dirs_list
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_system_prompt_contains_date_format() {
        let prompt = build_prompt_system_prompt(".ralph/prompts");
        assert!(
            prompt.contains("YYYY-MM-DD"),
            "system prompt should instruct Claude about YYYY-MM-DD naming"
        );
    }

    #[test]
    fn prompt_system_prompt_contains_prompts_directory() {
        let prompt = build_prompt_system_prompt(".ralph/prompts");
        assert!(
            prompt.contains(".ralph/prompts"),
            "system prompt should contain the prompts directory path"
        );
    }

    #[test]
    fn prompt_system_prompt_contains_custom_directory() {
        let prompt = build_prompt_system_prompt("my/custom/prompts");
        assert!(
            prompt.contains("my/custom/prompts"),
            "system prompt should contain the custom prompts directory"
        );
    }

    #[test]
    fn prompt_system_prompt_mentions_slug() {
        let prompt = build_prompt_system_prompt(".ralph/prompts");
        assert!(
            prompt.contains("<slug>"),
            "system prompt should explain the slug naming convention"
        );
    }

    #[test]
    fn prompt_system_prompt_mentions_markdown() {
        let prompt = build_prompt_system_prompt(".ralph/prompts");
        assert!(
            prompt.contains(".md"),
            "system prompt should mention .md extension"
        );
    }

    #[test]
    fn prompt_system_prompt_mentions_write_tool() {
        let prompt = build_prompt_system_prompt(".ralph/prompts");
        assert!(
            prompt.contains("Write"),
            "system prompt should instruct Claude to use the Write tool"
        );
    }

    #[test]
    fn specs_system_prompt_contains_specs_directories() {
        let dirs = vec![".ralph/specs".to_string(), "docs/specs".to_string()];
        let prompt = build_specs_system_prompt(&dirs);
        assert!(
            prompt.contains(".ralph/specs"),
            "system prompt should contain the first specs directory"
        );
        assert!(
            prompt.contains("docs/specs"),
            "system prompt should contain the second specs directory"
        );
    }

    #[test]
    fn specs_system_prompt_mentions_markdown() {
        let dirs = vec![".ralph/specs".to_string()];
        let prompt = build_specs_system_prompt(&dirs);
        assert!(
            prompt.contains(".md"),
            "system prompt should mention .md extension"
        );
    }

    #[test]
    fn specs_system_prompt_mentions_write_tool() {
        let dirs = vec![".ralph/specs".to_string()];
        let prompt = build_specs_system_prompt(&dirs);
        assert!(
            prompt.contains("Write"),
            "system prompt should instruct Claude to use the Write tool"
        );
    }

    #[test]
    fn specs_system_prompt_mentions_requirements() {
        let dirs = vec![".ralph/specs".to_string()];
        let prompt = build_specs_system_prompt(&dirs);
        assert!(
            prompt.contains("Requirements"),
            "system prompt should mention Requirements section"
        );
    }

    #[test]
    fn specs_system_prompt_mentions_architecture() {
        let dirs = vec![".ralph/specs".to_string()];
        let prompt = build_specs_system_prompt(&dirs);
        assert!(
            prompt.contains("Architecture"),
            "system prompt should mention Architecture section"
        );
    }

    #[test]
    fn specs_system_prompt_mentions_testing() {
        let dirs = vec![".ralph/specs".to_string()];
        let prompt = build_specs_system_prompt(&dirs);
        assert!(
            prompt.contains("Testing"),
            "system prompt should mention Testing section"
        );
    }

    #[test]
    fn plan_system_prompt_contains_specs_content() {
        let specs_content = "## Auth Spec\nUse JWT tokens";
        let system_prompt = build_plan_system_prompt(specs_content);
        assert!(
            system_prompt.contains("Use JWT tokens"),
            "system prompt should contain the specs content"
        );
    }

    #[test]
    fn plan_system_prompt_mentions_task_dag() {
        let system_prompt = build_plan_system_prompt("Test specs");
        assert!(
            system_prompt.contains("DAG") || system_prompt.contains("task"),
            "system prompt should mention task DAG concepts"
        );
    }

    #[test]
    fn plan_system_prompt_mentions_dependencies() {
        let system_prompt = build_plan_system_prompt("Test specs");
        assert!(
            system_prompt.contains("depends_on"),
            "system prompt should mention dependencies"
        );
    }

    #[test]
    fn plan_system_prompt_requests_json() {
        let system_prompt = build_plan_system_prompt("Test specs");
        assert!(
            system_prompt.contains("JSON"),
            "system prompt should request JSON output"
        );
    }

    #[test]
    fn plan_system_prompt_mentions_priority() {
        let system_prompt = build_plan_system_prompt("Test specs");
        assert!(
            system_prompt.contains("priority"),
            "system prompt should mention priority"
        );
    }

    // --- extract_plan_json tests ---

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

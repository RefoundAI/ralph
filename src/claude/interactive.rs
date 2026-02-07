//! Interactive Claude CLI invocation for prompt, specs, and plan subcommands.

use anyhow::{Context, Result};
use std::process::Command;

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
}

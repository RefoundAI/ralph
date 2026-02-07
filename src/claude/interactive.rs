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
}

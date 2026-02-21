//! Prompt text construction for ACP sessions.
//!
//! Migrated from src/claude/client.rs:
//! - build_system_instructions() (formerly build_system_prompt())
//! - build_task_context()
//! - build_prompt_text() (new: concatenates system + task context)

use crate::acp::types::{BlockerContext, IterationContext, ParentContext, TaskInfo};
use crate::config::Config;

/// Build the system instructions portion of a prompt.
///
/// Returns the static system prompt with Ralph loop instructions, sigil definitions, etc.
/// This is separated from task context so it can be reused in autonomous sessions.
pub fn build_system_instructions(_config: &Config) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        r#"You are operating in a Ralph loop - an autonomous, iterative coding workflow.

## Your Task

Ralph assigns you ONE task per iteration. Your job is to:

1. Read the assigned task context (provided below)
2. Implement ONLY the assigned task - do not work on multiple tasks
3. Do not assume code exists - search the codebase before implementing
4. Do not implement placeholders or stubs - implement fully working code
5. Run tests and type checks to verify your work
6. Commit your changes with a descriptive message; load the committing:git skill first
7. Signal task completion or failure using the appropriate sigil

## Critical Rules

- ONE TASK PER LOOP. This is essential. Do not implement multiple features.
- Work on the assigned task only - do not pick or reorder tasks
- Do not assume code exists - search the codebase before implementing
- Do not implement placeholders or stubs - implement fully working code
- If tests fail, fix them before completing
- Update AGENTS.md if you encounter any problems and learn how to solve them, e.g.:
    - Bash tool calls that took multiple attempts to write correctly

## Task Completion Sigils

After completing your work, signal the result:

- `<task-done>{task_id}</task-done>` — Task completed successfully
- `<task-failed>{task_id}</task-failed>` — Task cannot be completed (provide reason in output)

Emit one of these sigils every iteration with the task ID you were assigned.

## Project Completion

When the ENTIRE project/DAG is complete (not just your assigned task), output:
<promise>COMPLETE</promise>

Ralph will verify this against the task database. Only use when genuinely no more work exists.

## Critical Failure

If you encounter an unrecoverable situation where further iterations would be futile, output:
<promise>FAILURE</promise>

Use this when:
- The prompt contains contradictory or impossible requirements
- You are stuck in a loop making no progress after multiple attempts

## Model Hint

You can influence which model Ralph selects for the NEXT iteration by emitting a
model hint sigil anywhere in your output:

- `<next-model>opus</next-model>` — request Opus 4.6, the most capable (and expensive) model
- `<next-model>sonnet</next-model>` — request Sonnet 4.6, the balanced model
- `<next-model>haiku</next-model>` — request Haiku 4.5, the fastest and cheapest model

Rules:
- The hint applies to the NEXT iteration only; it is not persistent
- Valid values are exactly: `opus`, `sonnet`, `haiku`
- If omitted, Ralph's configured model strategy decides automatically
- Use this when you can tell the next task is trivial (hint haiku) or complex (hint opus)"#,
    );

    prompt
}

/// Build a task context block for the assigned task.
///
/// Returns a formatted markdown block with task details, parent context,
/// and completed prerequisites.
pub fn build_task_context(task: &TaskInfo) -> String {
    let mut output = String::new();

    output.push_str("## Assigned Task\n\n");
    output.push_str(&format!("**ID:** {}\n", task.task_id));
    output.push_str(&format!("**Title:** {}\n", task.title));
    output.push_str("\n### Description\n");
    output.push_str(&task.description);
    output.push('\n');

    // Add parent context if present
    if let Some(ref parent) = task.parent {
        output.push_str("\n### Parent Context\n");
        output.push_str(&format!("**Parent:** {}\n", parent.title));
        output.push_str(&parent.description);
        output.push('\n');
    }

    // Add completed blockers if present
    if !task.completed_blockers.is_empty() {
        output.push_str("\n### Completed Prerequisites\n");
        for blocker in &task.completed_blockers {
            output.push_str(&format!(
                "- [{}] {}: {}\n",
                blocker.task_id, blocker.title, blocker.summary
            ));
        }
    }

    output
}

/// Build the full prompt text for an ACP iteration.
///
/// Concatenates the system prompt instructions and task context into a single string,
/// since ACP has no separate system prompt channel. The system prompt portion is
/// placed first (Ralph loop instructions, sigil definitions, etc.), followed by
/// task assignment, spec/plan, retry info, journal/knowledge context.
pub fn build_prompt_text(config: &Config, context: &IterationContext) -> String {
    let mut prompt = String::new();

    // Start with system instructions
    prompt.push_str(&build_system_instructions(config));

    // Append iteration context
    prompt.push_str("\n\n");
    prompt.push_str(&build_task_context(&context.task));

    if let Some(ref spec) = context.spec_content {
        prompt.push_str("\n## Feature Specification\n\n");
        prompt.push_str(spec);
        prompt.push('\n');
    }

    if let Some(ref plan) = context.plan_content {
        prompt.push_str("\n## Feature Plan\n\n");
        prompt.push_str(plan);
        prompt.push('\n');
    }

    if let Some(ref retry) = context.retry_info {
        prompt.push_str("\n## Retry Information\n\n");
        prompt.push_str(&format!(
            "**This is retry attempt {} of {}.**\n\n",
            retry.attempt, retry.max_retries
        ));
        prompt.push_str("The previous attempt failed verification with the following reason:\n\n");
        prompt.push_str(&format!("> {}\n\n", retry.previous_failure_reason));
        prompt.push_str("Fix the issues identified above before marking the task as done.\n");
    }

    // Run Journal section (pre-rendered markdown from journal::render_journal_context)
    if !context.journal_context.is_empty() {
        prompt.push('\n');
        prompt.push_str(&context.journal_context);
    }

    // Project Knowledge section (pre-rendered markdown from knowledge::render_knowledge_context)
    if !context.knowledge_context.is_empty() {
        prompt.push('\n');
        prompt.push_str(&context.knowledge_context);
    }

    // Memory Instructions section — always included
    prompt.push_str("\n## Memory\n\n");
    prompt.push_str(
        "You have access to a persistent memory system. Use these sigils to record knowledge:\n\n",
    );
    prompt.push_str("### End-of-Task Journal\n");
    prompt.push_str("At the end of your work on this task, emit a `<journal>` sigil summarizing key decisions,\n");
    prompt.push_str("discoveries, and context that would help the next iteration:\n\n");
    prompt.push_str("```\n<journal>\nWhat you decided and why. What you discovered. What the next task should know.\n</journal>\n```\n\n");
    prompt.push_str("### Project Knowledge\n");
    prompt.push_str(
        "When you discover reusable project knowledge (patterns, gotchas, conventions,\n",
    );
    prompt.push_str("environment quirks), emit a `<knowledge>` sigil:\n\n");
    prompt.push_str("```\n<knowledge tags=\"tag1,tag2\" title=\"Short descriptive title\">\nDetailed explanation of the knowledge. Maximum ~500 words.\n</knowledge>\n```\n\n");
    prompt
        .push_str("Tags should be lowercase, relevant keywords. At least one tag is required.\n\n");
    prompt.push_str(
        "You should also continue to update CLAUDE.md with project-wide knowledge that\n",
    );
    prompt.push_str("benefits all future Claude sessions (not just Ralph runs).\n");

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectConfig, RalphConfig};
    use std::path::PathBuf;

    fn test_config() -> Config {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig::default(),
        };
        Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("cost-optimized".to_string()),
            None,
            project,
            None,
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn system_prompt_contains_next_model_tag() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            prompt.contains("<next-model>"),
            "system prompt should document the <next-model> sigil"
        );
    }

    #[test]
    fn system_prompt_contains_all_three_model_names() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(prompt.contains("opus"), "system prompt should mention opus");
        assert!(
            prompt.contains("sonnet"),
            "system prompt should mention sonnet"
        );
        assert!(
            prompt.contains("haiku"),
            "system prompt should mention haiku"
        );
    }

    #[test]
    fn system_prompt_contains_next_model_opus_example() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            prompt.contains("<next-model>opus</next-model>"),
            "system prompt should show opus example"
        );
    }

    #[test]
    fn system_prompt_contains_next_model_sonnet_example() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            prompt.contains("<next-model>sonnet</next-model>"),
            "system prompt should show sonnet example"
        );
    }

    #[test]
    fn system_prompt_contains_next_model_haiku_example() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            prompt.contains("<next-model>haiku</next-model>"),
            "system prompt should show haiku example"
        );
    }

    #[test]
    fn system_prompt_contains_completion_sigils() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            prompt.contains("<promise>COMPLETE</promise>"),
            "system prompt should document COMPLETE sigil"
        );
        assert!(
            prompt.contains("<promise>FAILURE</promise>"),
            "system prompt should document FAILURE sigil"
        );
    }

    #[test]
    fn system_prompt_contains_one_task_per_loop() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            prompt.contains("ONE TASK PER LOOP"),
            "system prompt should contain ONE TASK PER LOOP rule"
        );
    }

    #[test]
    fn system_prompt_contains_task_sigils() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            prompt.contains("<task-done>"),
            "system prompt should document task-done sigil"
        );
        assert!(
            prompt.contains("<task-failed>"),
            "system prompt should document task-failed sigil"
        );
    }

    #[test]
    fn system_prompt_no_progress_file_references() {
        let config = test_config();
        let prompt = build_system_instructions(&config);
        assert!(
            !prompt.contains("progress.txt"),
            "system prompt should not reference progress.txt"
        );
        assert!(
            !prompt.contains("progress_file"),
            "system prompt should not reference progress_file"
        );
        assert!(
            !prompt.to_lowercase().contains("append"),
            "system prompt should not have append instructions (for progress file)"
        );
    }

    // --- build_task_context tests ---

    #[test]
    fn task_context_with_all_fields() {
        let task = TaskInfo {
            task_id: "t-abc123".to_string(),
            title: "Implement feature X".to_string(),
            description: "Add the new feature X to the codebase.".to_string(),
            parent: Some(ParentContext {
                title: "Epic Y".to_string(),
                description: "The larger epic Y that encompasses this task.".to_string(),
            }),
            completed_blockers: vec![
                BlockerContext {
                    task_id: "t-prereq1".to_string(),
                    title: "Setup foundation".to_string(),
                    summary: "Created the base structure".to_string(),
                },
                BlockerContext {
                    task_id: "t-prereq2".to_string(),
                    title: "Add dependencies".to_string(),
                    summary: "Installed required packages".to_string(),
                },
            ],
        };

        let output = build_task_context(&task);

        assert!(output.contains("## Assigned Task"));
        assert!(output.contains("**ID:** t-abc123"));
        assert!(output.contains("**Title:** Implement feature X"));
        assert!(output.contains("### Description"));
        assert!(output.contains("Add the new feature X to the codebase."));
        assert!(output.contains("### Parent Context"));
        assert!(output.contains("**Parent:** Epic Y"));
        assert!(output.contains("The larger epic Y that encompasses this task."));
        assert!(output.contains("### Completed Prerequisites"));
        assert!(output.contains("- [t-prereq1] Setup foundation: Created the base structure"));
        assert!(output.contains("- [t-prereq2] Add dependencies: Installed required packages"));
    }

    #[test]
    fn task_context_no_parent_omits_parent_section() {
        let task = TaskInfo {
            task_id: "t-xyz789".to_string(),
            title: "Standalone task".to_string(),
            description: "A task with no parent.".to_string(),
            parent: None,
            completed_blockers: vec![],
        };

        let output = build_task_context(&task);

        assert!(output.contains("## Assigned Task"));
        assert!(output.contains("**ID:** t-xyz789"));
        assert!(output.contains("**Title:** Standalone task"));
        assert!(
            !output.contains("### Parent Context"),
            "Should not contain parent section"
        );
        assert!(
            !output.contains("**Parent:**"),
            "Should not contain parent field"
        );
    }

    #[test]
    fn task_context_no_blockers_omits_prerequisites_section() {
        let task = TaskInfo {
            task_id: "t-def456".to_string(),
            title: "Initial task".to_string(),
            description: "A task with no prerequisites.".to_string(),
            parent: Some(ParentContext {
                title: "Parent task".to_string(),
                description: "Parent description.".to_string(),
            }),
            completed_blockers: vec![],
        };

        let output = build_task_context(&task);

        assert!(output.contains("## Assigned Task"));
        assert!(output.contains("**ID:** t-def456"));
        assert!(output.contains("### Parent Context"));
        assert!(
            !output.contains("### Completed Prerequisites"),
            "Should not contain prerequisites section"
        );
    }

    #[test]
    fn task_context_with_two_blockers() {
        let task = TaskInfo {
            task_id: "t-multi".to_string(),
            title: "Task with multiple blockers".to_string(),
            description: "Depends on two tasks.".to_string(),
            parent: None,
            completed_blockers: vec![
                BlockerContext {
                    task_id: "t-blocker1".to_string(),
                    title: "First blocker".to_string(),
                    summary: "Completed first".to_string(),
                },
                BlockerContext {
                    task_id: "t-blocker2".to_string(),
                    title: "Second blocker".to_string(),
                    summary: "Completed second".to_string(),
                },
            ],
        };

        let output = build_task_context(&task);

        assert!(output.contains("### Completed Prerequisites"));
        assert!(output.contains("- [t-blocker1] First blocker: Completed first"));
        assert!(output.contains("- [t-blocker2] Second blocker: Completed second"));
    }

    #[test]
    fn task_context_verbatim_fields() {
        let task = TaskInfo {
            task_id: "t-verbatim-123".to_string(),
            title: "Special chars: <>&\"'".to_string(),
            description: "Description with\nnewlines and\ttabs.".to_string(),
            parent: None,
            completed_blockers: vec![],
        };

        let output = build_task_context(&task);

        assert!(output.contains("**ID:** t-verbatim-123"));
        assert!(output.contains("**Title:** Special chars: <>&\"'"));
        assert!(output.contains("Description with\nnewlines and\ttabs."));
    }

    // --- Memory / Journal / Knowledge system prompt tests ---

    fn test_iteration_context(journal_context: &str, knowledge_context: &str) -> IterationContext {
        IterationContext {
            task: TaskInfo {
                task_id: "t-test01".to_string(),
                title: "Test task".to_string(),
                description: "Test description".to_string(),
                parent: None,
                completed_blockers: vec![],
            },
            spec_content: None,
            plan_content: None,
            retry_info: None,
            run_id: "run-00000001".to_string(),
            journal_context: journal_context.to_string(),
            knowledge_context: knowledge_context.to_string(),
        }
    }

    #[test]
    fn test_system_prompt_includes_journal() {
        let config = test_config();
        let journal_md = "## Run Journal\n\n### Iteration 1 [done]\n- **Task**: t-abc123\n- **Notes**: Did some work\n";
        let ctx = test_iteration_context(journal_md, "");
        let prompt = build_prompt_text(&config, &ctx);
        assert!(
            prompt.contains("## Run Journal"),
            "system prompt should include the Run Journal section when journal_context is non-empty"
        );
        assert!(
            prompt.contains("Iteration 1 [done]"),
            "system prompt should include journal entry content"
        );
    }

    #[test]
    fn test_system_prompt_no_journal_when_empty() {
        let config = test_config();
        let ctx = test_iteration_context("", "");
        let prompt = build_prompt_text(&config, &ctx);
        assert!(
            !prompt.contains("## Run Journal"),
            "system prompt should NOT include Run Journal section when journal_context is empty"
        );
    }

    #[test]
    fn test_system_prompt_includes_knowledge() {
        let config = test_config();
        let knowledge_md = "## Project Knowledge\n\n### Cargo bench requires nightly toolchain\n_Tags: testing, cargo_\n\nSome knowledge content.\n\n";
        let ctx = test_iteration_context("", knowledge_md);
        let prompt = build_prompt_text(&config, &ctx);
        assert!(
            prompt.contains("## Project Knowledge"),
            "system prompt should include the Project Knowledge section when knowledge_context is non-empty"
        );
        assert!(
            prompt.contains("Cargo bench requires nightly toolchain"),
            "system prompt should include knowledge entry content"
        );
    }

    #[test]
    fn test_system_prompt_no_knowledge_when_empty() {
        let config = test_config();
        let ctx = test_iteration_context("", "");
        let prompt = build_prompt_text(&config, &ctx);
        // The knowledge section header uses "## Project Knowledge" (2 hashes), not the
        // "### Project Knowledge" (3 hashes) used inside the Memory Instructions.
        // We check the prompt does not contain a standalone "## Project Knowledge" line.
        assert!(
            !prompt.contains("\n## Project Knowledge\n"),
            "system prompt should NOT include a standalone '## Project Knowledge' section when knowledge_context is empty"
        );
    }

    #[test]
    fn test_system_prompt_includes_memory_instructions() {
        let config = test_config();
        let ctx = test_iteration_context("", "");
        let prompt = build_prompt_text(&config, &ctx);
        assert!(
            prompt.contains("## Memory"),
            "system prompt should always include the Memory section"
        );
        assert!(
            prompt.contains("<journal>"),
            "system prompt Memory section should document the journal sigil"
        );
        assert!(
            prompt.contains("<knowledge tags="),
            "system prompt Memory section should document the knowledge sigil"
        );
        assert!(
            prompt.contains("End-of-Task Journal"),
            "system prompt should include journal instructions"
        );
        assert!(
            prompt.contains("Project Knowledge"),
            "system prompt should include knowledge instructions"
        );
        assert!(
            prompt.contains("CLAUDE.md"),
            "system prompt should mention CLAUDE.md updates"
        );
    }

    #[test]
    fn test_system_prompt_no_skills_section() {
        let config = test_config();
        // Test with build_system_instructions (no context)
        let prompt_no_ctx = build_system_instructions(&config);
        assert!(
            !prompt_no_ctx.contains("Available Skills"),
            "system prompt should NOT contain 'Available Skills' section (removed in favor of native skill discovery)"
        );
        // Test with build_prompt_text (with context)
        let ctx = test_iteration_context("", "");
        let prompt_with_ctx = build_prompt_text(&config, &ctx);
        assert!(
            !prompt_with_ctx.contains("Available Skills"),
            "system prompt should NOT contain 'Available Skills' section even with context"
        );
    }

    #[test]
    fn test_system_prompt_no_learning_section() {
        let config = test_config();
        let ctx = test_iteration_context("", "");
        let prompt = build_prompt_text(&config, &ctx);
        // The old "Learning" section had text about SKILL.md creation
        assert!(
            !prompt.contains("## Learning"),
            "system prompt should NOT contain the old '## Learning' section"
        );
        assert!(
            !prompt.contains("skill creation"),
            "system prompt should NOT contain old skill creation instructions"
        );
    }

    #[test]
    fn test_iteration_context_fields_no_skills_summary() {
        // Compile-time check: verify IterationContext has the new fields
        let ctx = test_iteration_context("journal content", "knowledge content");
        assert_eq!(ctx.run_id, "run-00000001");
        assert_eq!(ctx.journal_context, "journal content");
        assert_eq!(ctx.knowledge_context, "knowledge content");
    }
}

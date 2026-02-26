//! Prompt and context builders used by `feature create` and task-creation flows.

use crate::{dag, feature, project};

pub const MAX_CONTEXT_FILE_CHARS: usize = 10_000;

pub fn build_feature_spec_system_prompt(name: &str, spec_path: &str, context: &str) -> String {
    format!(
        r#"You are co-authoring a specification for a new project or feature with the user - "{name}".

## Your Role

Interview the user thoroughly to understand their requirements, then write a comprehensive specification document.

## Scope — SPECIFICATION DOCUMENT ONLY

This is a SPECIFICATION session. You are authoring a spec document — nothing else.

Your ONLY deliverable is the spec document at `{spec_path}`. You must NOT:
- Write or modify any source code, tests, or configuration files
- Run build commands, test commands, or any implementation steps
- Create any files other than the spec document itself
- Start implementing the spec — that happens in later phases of `ralph feature create` and via `ralph run`

IMPORTANT: Once you have written the spec document to `{spec_path}`, your work is DONE.
Do NOT proceed to implement anything. Do NOT try to create tasks. Do NOT run any commands.
Tell the user the spec is written and emit the completion sigil: `<phase-complete>spec</phase-complete>`

## Workflow

1. **Interview** — Ask the user about requirements, constraints, edge cases, and acceptance criteria.
2. **Write** — Once you have enough information, write the spec to `{spec_path}`.
3. **Signal completion** — After writing the spec file, tell the user it's done and emit `<phase-complete>spec</phase-complete>`. Do NOT continue working.

## Interview Style

- Ask ONE focused question at a time. Wait for the user's answer before asking the next.
- For choices, present numbered options so the user can reply with just a number:
  ```
  How should auth tokens be stored?
  1. HTTP-only cookies
  2. localStorage
  3. In-memory only
  ```
- Summarize what you've learned periodically and ask if anything is missing.
- The user has multi-line input — they can provide detailed answers. Don't rush them.

## Guidelines

- Ask about:
  - What the feature should do (functional requirements)
  - Technical constraints and preferences
  - Expected behavior and edge cases
  - Testing requirements and acceptance criteria
  - Dependencies and integration points

- The spec should be:
  - Detailed & clear enough for one or more AI agents to implement without prior context
  - Structured with markdown sections
  - Concrete with examples and schemas
  - Testable with clear acceptance criteria

{context}

## Output

Write the final spec to: `{spec_path}`

Include sections for:
1. **Overview** - What this feature does
2. **Requirements** - Functional and non-functional
3. **Architecture** - Components, data flow
4. **API / Interface** - Function signatures, contracts
5. **Data Models** - Types, schemas, validation
6. **Testing** - Test cases, acceptance criteria
7. **Dependencies** - Libraries, services"#,
        name = name,
        spec_path = spec_path,
        context = context,
    )
}

pub fn build_feature_plan_system_prompt(
    name: &str,
    spec_content: &str,
    plan_path: &str,
    context: &str,
) -> String {
    format!(
        r#"You are helping the user create an implementation plan for feature "{name}".

## Your Role

Interview the user to discuss implementation approach and trade-offs, then write a detailed implementation plan document. The plan should break down the work into logical phases.

## Scope — PLANNING DOCUMENT ONLY

This is a PLANNING session. You are authoring a plan document — nothing else.

Your ONLY deliverable is the plan document at `{plan_path}`. You must NOT:
- Write or modify any source code, tests, or configuration files
- Run build commands, test commands, or any implementation steps
- Create any files other than the plan document itself
- Read source code for the purpose of starting implementation
- Start implementing the plan — that happens in later phases of `ralph feature create` and via `ralph run`

IMPORTANT: Once you have written the plan document to `{plan_path}`, your work is DONE.
Do NOT proceed to implement anything. Do NOT try to create tasks. Do NOT run any commands.
Tell the user the plan is written and emit the completion sigil: `<phase-complete>plan</phase-complete>`

## Workflow

1. **Interview** — Discuss the spec with the user. Ask about implementation preferences,
   architectural choices, ordering, and any ambiguities. You may read the codebase to
   understand existing patterns relevant to planning.
2. **Write** — Once the user agrees on the approach, write the plan to `{plan_path}`.
3. **Signal completion** — After writing the plan file, tell the user it's done and emit `<phase-complete>plan</phase-complete>`. Do NOT continue working.

## Interview Style

- Ask ONE focused question at a time. Wait for the user's answer before asking the next.
- For choices, present numbered options so the user can reply with just a number:
  ```
  Which pattern should we use for state management?
  1. Single global store
  2. Per-component local state
  3. Hybrid approach
  ```
- Summarize what you've learned periodically and ask if anything is missing.
- The user has multi-line input — they can provide detailed answers. Don't rush them.

## Guidelines

- Ask clarifying questions about anything ambiguous in the spec
- Consider implementation order and dependencies
- Include verification criteria for each section
- Reference the spec sections by name

{context}

## Specification

{spec_content}

## Output

Write the final plan to: `{plan_path}`

The plan should include:
1. **Implementation phases** - Ordered list of work to do
2. **Per-phase details** - What to implement, what to test
3. **Verification criteria** - How to know each phase is done
4. **Risk areas** - Things that might go wrong

After writing the plan file, STOP. Do not implement anything."#,
        name = name,
        spec_content = spec_content,
        plan_path = plan_path,
        context = context,
    )
}

pub fn build_feature_build_system_prompt(
    spec_content: &str,
    plan_content: &str,
    root_id: &str,
    feature_id: &str,
) -> String {
    format!(
        r#"You are a planning agent for Ralph, an autonomous AI agent loop that drives Claude Code.

Decompose the feature's plan into a task DAG by creating tasks using the `ralph` CLI.

## How Ralph Executes Tasks

- Picks ONE ready leaf task per iteration, assigns it to a fresh Claude Code session
- The session gets: task title, description, parent context, completed prerequisite summaries, plus the full spec and plan content
- Only leaf tasks execute — parent tasks auto-complete when all children complete

## Specification

{spec_content}

## Plan

{plan_content}

## Root Task

A root task has already been created for this feature:
- **Root Task ID:** `{root_id}`
- **Feature ID:** `{feature_id}`

All tasks you create should be children of this root task (or children of other tasks you create under it).

## CLI Commands

Use these `ralph` commands via the Bash tool to create the task DAG:

### Create a task
```bash
ralph task add "Short imperative title" \
  -d "Detailed description: what to do, which files to touch, how to verify." \
  --parent {root_id} \
  --feature {feature_id}
```

This prints the new task ID (e.g., `t-a1b2c3`) to stdout. Capture it:
```bash
ID=$(ralph task add "Title" -d "Description" --parent {root_id} --feature {feature_id})
```

### Create a child task under another task
```bash
CHILD=$(ralph task add "Child task" -d "Description" --parent $PARENT_ID --feature {feature_id})
```

### Add a dependency (A must complete before B)
```bash
ralph task deps add $BLOCKER_ID $BLOCKED_ID
```

## Decomposition Rules

1. **Right-size tasks**: One coherent unit of work per task. Good tasks touch 1-3 files.
2. **Reference spec/plan sections**: Each task description must reference which spec/plan section it implements
3. **Include acceptance criteria**: Each task must include how to verify it's done
4. **Parent tasks for grouping**: Parents organize related children, they never execute
5. **Dependencies for ordering**: Only when task B needs artifacts from task A
6. **Foundation first**: Schemas and types before the code that uses them

## Completion Signal

When you are done creating all tasks and dependencies, emit the completion sigil:
`<phase-complete>build</phase-complete>`

This tells Ralph the task DAG is complete and it can print a summary.

## Instructions

1. Read the spec and plan carefully
2. Create parent tasks for logical groupings (as children of `{root_id}`)
3. Create leaf tasks under each parent
4. Add dependencies between tasks where order matters
5. Emit `<phase-complete>build</phase-complete>` when done"#,
        spec_content = spec_content,
        plan_content = plan_content,
        root_id = root_id,
        feature_id = feature_id,
    )
}

pub fn build_task_new_system_prompt(context: &str) -> String {
    format!(
        r#"You are helping the user create a standalone task for Ralph, an autonomous AI agent loop.

## Your Role

Interview the user about what they want done, then create a standalone task in the Ralph database.

## Interview Style

- Ask ONE focused question at a time. Wait for the user's answer before asking the next.
- For choices, present numbered options so the user can reply with just a number.
- The user has multi-line input — they can provide detailed answers. Don't rush them.

## Guidelines

- Ask the user:
  - What they want accomplished
  - Any specific files or areas of the codebase
  - Acceptance criteria (how to know it's done)
  - Priority level

- Keep the task focused — one logical unit of work

{context}

## Creating the Task

After gathering requirements, create the task by running the `ralph task add` command via the terminal.

Usage: `ralph task add <TITLE> -d <DESCRIPTION> --priority <N>`

Example:
```
ralph task add "Refactor auth module" -d "Extract token validation into a separate function. Acceptance criteria: all existing tests pass, new unit test for the extracted function." --priority 0
```

The command prints the new task ID on success. Confirm the task was created by showing the ID to the user.

After creating the task, emit the completion sigil: `<tasks-created></tasks-created>`
This tells Ralph the task has been created and the session can end.

IMPORTANT: You MUST use the terminal to run `ralph task add`. Do NOT just output JSON — it will not be processed."#,
        context = context,
    )
}

/// Truncate content to a character limit with a notice.
/// Uses char_indices() for unicode-safe boundary (not byte slicing).
pub fn truncate_context(content: &str, limit: usize, file_hint: &str) -> String {
    if content.chars().count() <= limit {
        return content.to_string();
    }

    // Find byte offset of the limit-th character for safe slicing
    let byte_offset = content
        .char_indices()
        .nth(limit)
        .map(|(idx, _)| idx)
        .unwrap_or(content.len());

    let mut result = content[..byte_offset].to_string();
    result.push_str(&format!("\n\n[Truncated -- full file at {}]", file_hint));
    result
}

/// Gather project context for interactive session system prompts.
///
/// Reads CLAUDE.md, .ralph.toml, feature list, and optionally task list.
/// Returns a formatted markdown string to embed in the system prompt.
/// Never errors — gracefully degrades if any source is unavailable.
pub fn gather_project_context(
    project: &project::ProjectConfig,
    db: &dag::Db,
    include_tasks: bool,
) -> String {
    use std::fs;

    let mut sections = Vec::new();

    // Read CLAUDE.md
    let claude_md_path = project.root.join("CLAUDE.md");
    let claude_md = fs::read_to_string(&claude_md_path).ok();
    let claude_md_section = if let Some(content) = claude_md {
        let truncated = truncate_context(&content, MAX_CONTEXT_FILE_CHARS, "CLAUDE.md");
        format!("### CLAUDE.md\n\n{}", truncated)
    } else {
        "### CLAUDE.md\n\n[Not found]".to_string()
    };
    sections.push(claude_md_section);

    // Read .ralph.toml
    let config_path = project.root.join(".ralph.toml");
    if let Ok(config_content) = fs::read_to_string(&config_path) {
        sections.push(format!(
            "### Configuration (.ralph.toml)\n\n```toml\n{}\n```",
            config_content
        ));
    }

    // List existing features
    let features = feature::list_features(db).unwrap_or_default();
    if !features.is_empty() {
        let mut feature_table = String::from("### Existing Features\n\n");
        feature_table.push_str("| Name | Status | Has Spec | Has Plan |\n");
        feature_table.push_str("|------|--------|----------|----------|\n");
        for feat in features {
            let has_spec = if feat.spec_path.is_some() { "✓" } else { "" };
            let has_plan = if feat.plan_path.is_some() { "✓" } else { "" };
            feature_table.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                feat.name, feat.status, has_spec, has_plan
            ));
        }
        sections.push(feature_table);
    }

    // List standalone tasks if requested
    if include_tasks {
        let tasks = dag::get_standalone_tasks(db).unwrap_or_default();
        if !tasks.is_empty() {
            let mut task_list = String::from("### Existing Standalone Tasks\n\n");
            for task in tasks {
                task_list.push_str(&format!(
                    "- **{}** ({}): {}\n",
                    task.id, task.status, task.title
                ));
            }
            sections.push(task_list);
        }
    }

    format!("## Project Context\n\n{}", sections.join("\n\n"))
}

/// Build initial message for feature spec interview.
pub fn build_initial_message_spec(name: &str, resuming: bool) -> String {
    if resuming {
        format!(
            "Resume the spec interview for feature \"{}\". The current spec draft is in your system prompt.",
            name
        )
    } else {
        format!("Start the spec interview for feature \"{}\".", name)
    }
}

/// Build initial message for feature plan interview.
pub fn build_initial_message_plan(name: &str, resuming: bool) -> String {
    if resuming {
        format!(
            "Resume the plan interview for feature \"{}\". The current plan draft is in your system prompt.",
            name
        )
    } else {
        format!(
            "Start the plan interview for feature \"{}\". The spec is included in your system prompt. Discuss the implementation approach with me before writing the plan file.",
            name
        )
    }
}

/// Build initial message for task creation interview.
pub fn build_initial_message_task_new() -> String {
    "Start the task creation interview.".to_string()
}

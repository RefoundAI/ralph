//! Ralph - Autonomous agent loop harness for Claude Code

mod cli;
mod config;
mod claude;
mod dag;
mod feature;
mod sandbox;
mod output;
mod project;
mod run_loop;
mod strategy;
mod verification;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_variants_exist() {
        let _complete = run_loop::Outcome::Complete;
        let _failure = run_loop::Outcome::Failure;
        let _limit = run_loop::Outcome::LimitReached;
        let _blocked = run_loop::Outcome::Blocked;
        let _noplan = run_loop::Outcome::NoPlan;
    }

    #[test]
    fn outcome_complete_vs_failure() {
        assert_ne!(run_loop::Outcome::Complete, run_loop::Outcome::Failure);
    }

    #[test]
    fn outcome_blocked_vs_noplan() {
        assert_ne!(run_loop::Outcome::Blocked, run_loop::Outcome::NoPlan);
    }
}

fn run() -> Result<ExitCode> {
    let args = cli::Args::parse_args();

    match args.command {
        Some(cli::Command::Init) => {
            project::init()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(cli::Command::Feature { action }) => handle_feature(action),
        Some(cli::Command::Task { action }) => handle_task(action),
        Some(cli::Command::Run {
            target,
            once,
            no_sandbox,
            limit,
            allow,
            model_strategy,
            model,
            max_retries,
            no_verify,
            no_learn,
        }) => {
            let project = project::discover()?;

            // Resolve target: check feature names first, then task IDs
            let db_path = project.root.join(".ralph/progress.db");
            let db = dag::open_db(db_path.to_str().unwrap())?;

            let run_target = if target.starts_with("t-") {
                // Task ID
                dag::get_task(&db, &target)?;
                config::RunTarget::Task(target)
            } else {
                // Feature name
                let feat = feature::get_feature(&db, &target)?;
                if feat.status != "ready" && feat.status != "running" {
                    anyhow::bail!(
                        "Feature '{}' is not ready to run (status: {}). Run 'ralph feature build {}' first.",
                        target, feat.status, target
                    );
                }
                config::RunTarget::Feature(target)
            };

            let config = config::Config::from_run_args(
                None,
                once,
                no_sandbox,
                limit,
                allow,
                model_strategy,
                model,
                project,
                Some(run_target),
                max_retries,
                no_verify,
                no_learn,
            )?;

            output::formatter::print_iteration_info(&config);

            match run_loop::run(config)? {
                run_loop::Outcome::Complete => {
                    output::formatter::print_complete();
                    Ok(ExitCode::SUCCESS)
                }
                run_loop::Outcome::Failure => {
                    output::formatter::print_failure();
                    Ok(ExitCode::FAILURE)
                }
                run_loop::Outcome::LimitReached => {
                    output::formatter::print_limit_reached();
                    Ok(ExitCode::SUCCESS)
                }
                run_loop::Outcome::Blocked => {
                    eprintln!("Loop blocked: no ready tasks, but incomplete tasks remain");
                    Ok(ExitCode::from(2))
                }
                run_loop::Outcome::NoPlan => {
                    eprintln!("No plan: DAG is empty. Run 'ralph feature build <name>' to create tasks");
                    Ok(ExitCode::from(3))
                }
            }
        }
        None => {
            cli::Args::parse_from(["ralph", "--help"]);
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Handle `ralph feature <action>` subcommands.
fn handle_feature(action: cli::FeatureAction) -> Result<ExitCode> {
    let project = project::discover()?;
    let db_path = project.root.join(".ralph/progress.db");
    let db = dag::open_db(db_path.to_str().unwrap())?;

    match action {
        cli::FeatureAction::Spec { name, model: _ } => {
            // Create or get feature
            let feat = if feature::feature_exists(&db, &name)? {
                feature::get_feature(&db, &name)?
            } else {
                feature::create_feature(&db, &name)?
            };

            // Ensure directory structure
            feature::ensure_feature_dirs(&project.root, &name)?;

            let spec_path = project.root.join(".ralph/features").join(&name).join("spec.md");
            let spec_path_str = spec_path.to_string_lossy().to_string();

            // Build system prompt for spec authoring
            let system_prompt = build_feature_spec_system_prompt(&name, &spec_path_str);
            claude::interactive::run_interactive(&system_prompt)?;

            // Update feature with spec path
            feature::update_feature_spec_path(&db, &feat.id, &spec_path_str)?;

            println!("Feature '{}' spec saved.", name);
            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::Plan { name, model: _ } => {
            // Validate feature exists with spec
            let feat = feature::get_feature(&db, &name)?;
            if feat.spec_path.is_none() {
                anyhow::bail!(
                    "Feature '{}' has no spec. Run 'ralph feature spec {}' first.",
                    name, name
                );
            }

            // Read spec content
            let spec_content = feature::read_spec(&project.root, &name)?;

            let plan_path = project.root.join(".ralph/features").join(&name).join("plan.md");
            let plan_path_str = plan_path.to_string_lossy().to_string();

            // Build system prompt for plan authoring
            let system_prompt = build_feature_plan_system_prompt(&name, &spec_content, &plan_path_str);
            claude::interactive::run_interactive(&system_prompt)?;

            // Update feature with plan path and status
            feature::update_feature_plan_path(&db, &feat.id, &plan_path_str)?;
            feature::update_feature_status(&db, &feat.id, "planned")?;

            println!("Feature '{}' plan saved.", name);
            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::Build { name, model } => {
            // Validate feature has spec and plan
            let feat = feature::get_feature(&db, &name)?;
            if feat.spec_path.is_none() {
                anyhow::bail!(
                    "Feature '{}' has no spec. Run 'ralph feature spec {}' first.",
                    name, name
                );
            }
            if feat.plan_path.is_none() {
                anyhow::bail!(
                    "Feature '{}' has no plan. Run 'ralph feature plan {}' first.",
                    name, name
                );
            }

            // Read spec + plan content
            let spec_content = feature::read_spec(&project.root, &name)?;
            let plan_content = feature::read_plan(&project.root, &name)?;

            // Build DAG decomposition prompt
            let system_prompt = build_feature_build_system_prompt(&spec_content, &plan_content);

            let build_model = model.as_deref().unwrap_or("opus");
            eprintln!("Decomposing with {}...", build_model);

            // Run Claude in streaming mode to get task breakdown
            let combined_input = format!("Feature: {}\n\nSpec:\n{}\n\nPlan:\n{}", name, spec_content, plan_content);
            let output = claude::interactive::run_streaming(&system_prompt, &combined_input, build_model)?;

            // Parse the plan JSON
            let plan = claude::interactive::extract_plan_json(&output)?;

            if plan.tasks.is_empty() {
                anyhow::bail!("Claude returned an empty task list");
            }

            // Create a root parent task for the feature
            let max_retries = project.config.execution.max_retries as i32;
            let root = dag::create_task_with_feature(
                &db,
                &format!("Feature: {}", name),
                Some(&format!("Root task for feature '{}'", name)),
                None,
                0,
                Some(&feat.id),
                "feature",
                max_retries,
            )?;
            eprintln!("  {} Feature: {}", root.id, name);

            // Insert tasks, mapping temp IDs to real IDs
            let mut id_map = std::collections::HashMap::new();

            for task in &plan.tasks {
                let parent_real_id = task.parent_id
                    .as_ref()
                    .and_then(|pid| id_map.get(pid).cloned())
                    .unwrap_or_else(|| root.id.clone());

                let created = dag::create_task_with_feature(
                    &db,
                    &task.title,
                    Some(&task.description),
                    Some(&parent_real_id),
                    task.priority,
                    Some(&feat.id),
                    "feature",
                    max_retries,
                )?;
                eprintln!("  {} {}", created.id, task.title);
                id_map.insert(task.id.clone(), created.id);
            }

            // Add dependencies
            for task in &plan.tasks {
                let real_id = id_map.get(&task.id).unwrap();
                for dep_id in &task.depends_on {
                    if let Some(real_dep_id) = id_map.get(dep_id) {
                        dag::add_dependency(&db, real_dep_id, real_id)?;
                    } else {
                        eprintln!(
                            "  Warning: task {} depends on unknown task {}, skipping",
                            task.id, dep_id
                        );
                    }
                }
            }

            // Update feature status
            feature::update_feature_status(&db, &feat.id, "ready")?;

            eprintln!("\nCreated {} tasks for feature '{}'", plan.tasks.len(), name);
            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::List => {
            let features = feature::list_features(&db)?;

            if features.is_empty() {
                println!("No features. Run 'ralph feature spec <name>' to create one.");
                return Ok(ExitCode::SUCCESS);
            }

            for feat in &features {
                let counts = dag::get_feature_task_counts(&db, &feat.id)?;
                let status_display = match feat.status.as_str() {
                    "draft" => feat.status.yellow().to_string(),
                    "planned" => feat.status.blue().to_string(),
                    "ready" => feat.status.cyan().to_string(),
                    "running" => feat.status.magenta().to_string(),
                    "done" => feat.status.green().to_string(),
                    "failed" => feat.status.red().to_string(),
                    _ => feat.status.clone(),
                };

                if counts.total > 0 {
                    println!(
                        "  {:<16} [{}]  {}/{} done, {} ready",
                        feat.name, status_display, counts.done, counts.total, counts.ready
                    );
                } else {
                    let detail = match feat.status.as_str() {
                        "draft" => "spec only",
                        "planned" => "spec + plan ready",
                        _ => "",
                    };
                    println!("  {:<16} [{}]  {}", feat.name, status_display, detail);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Handle `ralph task <action>` subcommands.
fn handle_task(action: cli::TaskAction) -> Result<ExitCode> {
    let project = project::discover()?;
    let db_path = project.root.join(".ralph/progress.db");
    let db = dag::open_db(db_path.to_str().unwrap())?;

    match action {
        cli::TaskAction::New { model: _ } => {
            let system_prompt = build_task_new_system_prompt();
            claude::interactive::run_interactive(&system_prompt)?;

            // After the interactive session, check if a task was created
            // The Claude session will have used the Write tool to create a task file
            // For now, we ask the user to provide task details
            println!("Task creation session complete.");
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::List => {
            let tasks = dag::get_standalone_tasks(&db)?;

            if tasks.is_empty() {
                println!("No standalone tasks. Run 'ralph task new' to create one.");
                return Ok(ExitCode::SUCCESS);
            }

            for task in &tasks {
                // Get the task's current status from the DB
                let status: String = db.conn().query_row(
                    "SELECT status FROM tasks WHERE id = ?",
                    [&task.id],
                    |row| row.get(0),
                )?;

                let status_display = match status.as_str() {
                    "pending" => status.yellow().to_string(),
                    "in_progress" => status.cyan().to_string(),
                    "done" => status.green().to_string(),
                    "failed" => status.red().to_string(),
                    _ => status,
                };

                println!("  {}  [{}]  {}", task.id, status_display, task.title);
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

// --- System prompt builders ---

fn build_feature_spec_system_prompt(name: &str, spec_path: &str) -> String {
    format!(
        r#"You are helping the user craft a specification for feature "{name}".

## Your Role

Interview the user to understand their requirements, then write a comprehensive specification document.

## Guidelines

- Ask about:
  - What the feature should do (functional requirements)
  - Technical constraints and preferences
  - Expected behavior and edge cases
  - Testing requirements and acceptance criteria
  - Dependencies and integration points

- The spec should be:
  - Detailed enough for an AI agent to implement without ambiguity
  - Structured with markdown sections
  - Concrete with examples and schemas
  - Testable with clear acceptance criteria

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
    )
}

fn build_feature_plan_system_prompt(name: &str, spec_content: &str, plan_path: &str) -> String {
    format!(
        r#"You are helping the user create an implementation plan for feature "{name}".

## Specification

{spec_content}

## Your Role

Based on the specification above, work with the user to create a detailed implementation plan. The plan should break down the work into logical phases.

## Guidelines

- Ask clarifying questions about anything ambiguous in the spec
- Consider implementation order and dependencies
- Include verification criteria for each section
- Reference the spec sections by name

## Output

Write the final plan to: `{plan_path}`

The plan should include:
1. **Implementation phases** - Ordered list of work to do
2. **Per-phase details** - What to implement, what to test
3. **Verification criteria** - How to know each phase is done
4. **Risk areas** - Things that might go wrong"#,
        name = name,
        spec_content = spec_content,
        plan_path = plan_path,
    )
}

fn build_feature_build_system_prompt(spec_content: &str, plan_content: &str) -> String {
    format!(
        r#"You are a planning agent for Ralph, an autonomous AI agent loop that drives Claude Code.

Decompose the feature's plan into a task DAG. Each task runs in a separate, isolated Claude Code session — one task per iteration.

## How Ralph Executes Tasks

- Picks ONE ready leaf task per iteration, assigns it to a fresh Claude Code session
- The session gets: task title, description, parent context, completed prerequisite summaries, plus the full spec and plan content
- Only leaf tasks execute — parent tasks auto-complete when all children complete

## Specification

{spec_content}

## Plan

{plan_content}

## Decomposition Rules

1. **Right-size tasks**: One coherent unit of work per task. Good tasks touch 1-3 files.
2. **Reference spec/plan sections**: Each task description must reference which spec/plan section it implements
3. **Include acceptance criteria**: Each task must include how to verify it's done
4. **Parent tasks for grouping**: Parents organize related children, they never execute
5. **depends_on for real dependencies**: Only when task B needs artifacts from task A
6. **Foundation first**: Schemas and types before the code that uses them

## Output Format

Output ONLY a JSON object:

{{
  "tasks": [
    {{
      "id": "1",
      "title": "Short imperative title",
      "description": "What to do, which files, how to verify. References spec/plan section.",
      "parent_id": null,
      "depends_on": [],
      "priority": 0
    }}
  ]
}}"#,
        spec_content = spec_content,
        plan_content = plan_content,
    )
}

fn build_task_new_system_prompt() -> String {
    r#"You are helping the user create a standalone task for Ralph, an autonomous AI agent loop.

## Your Role

Interview the user about what they want done, then create a standalone task in the Ralph database.

## Guidelines

- Ask the user:
  - What they want accomplished
  - Any specific files or areas of the codebase
  - Acceptance criteria (how to know it's done)
  - Priority level

- Keep the task focused — one logical unit of work

## Output

After gathering requirements, create the task by outputting:

```json
{
  "title": "Short imperative title",
  "description": "Detailed description with acceptance criteria",
  "priority": 0
}
```

The task will be created as a standalone task (not part of any feature)."#
    .to_string()
}

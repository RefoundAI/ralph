//! Ralph - Autonomous agent loop harness for Claude Code

mod acp;
mod claude;
mod cli;
mod config;
mod dag;
mod feature;
mod interrupt;
mod journal;
mod knowledge;
mod output;
mod project;
mod review;
mod run_loop;
mod sandbox;
mod strategy;
mod verification;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
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
        let _interrupted = run_loop::Outcome::Interrupted;
    }

    #[test]
    fn outcome_complete_vs_failure() {
        assert_ne!(run_loop::Outcome::Complete, run_loop::Outcome::Failure);
    }

    #[test]
    fn outcome_blocked_vs_noplan() {
        assert_ne!(run_loop::Outcome::Blocked, run_loop::Outcome::NoPlan);
    }

    mod context_tests {
        use super::super::*;
        use std::fs;
        use tempfile::TempDir;

        #[test]
        fn test_truncate_under_limit() {
            let content = "Hello, world!";
            let result = truncate_context(content, 100, "test.md");
            assert_eq!(result, content);
        }

        #[test]
        fn test_truncate_over_limit() {
            let content = "a".repeat(15_000);
            let result = truncate_context(&content, MAX_CONTEXT_FILE_CHARS, "test.md");
            assert!(result.contains("[Truncated -- full file at test.md]"));
            assert!(result.chars().count() > MAX_CONTEXT_FILE_CHARS);
            assert!(result.chars().count() < MAX_CONTEXT_FILE_CHARS + 100);
        }

        #[test]
        fn test_truncate_unicode_safe() {
            // Create string with multi-byte unicode chars
            let content = "🎉".repeat(15_000);
            let result = truncate_context(&content, MAX_CONTEXT_FILE_CHARS, "test.md");
            // Should not panic, and should contain truncation notice
            assert!(result.contains("[Truncated"));
        }

        #[test]
        fn test_gather_context_empty_project() {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("progress.db");
            let db = dag::open_db(db_path.to_str().unwrap()).unwrap();

            let project = project::ProjectConfig {
                root: temp_dir.path().to_path_buf(),
                config: project::RalphConfig::default(),
            };

            let context = gather_project_context(&project, &db, false);
            assert!(context.contains("## Project Context"));
            assert!(context.contains("[Not found]"));
        }

        #[test]
        fn test_gather_context_with_claude_md() {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("progress.db");
            let db = dag::open_db(db_path.to_str().unwrap()).unwrap();

            // Write CLAUDE.md
            let claude_md_content = "This is test project context.";
            fs::write(temp_dir.path().join("CLAUDE.md"), claude_md_content).unwrap();

            let project = project::ProjectConfig {
                root: temp_dir.path().to_path_buf(),
                config: project::RalphConfig::default(),
            };

            let context = gather_project_context(&project, &db, false);
            assert!(context.contains("## Project Context"));
            assert!(context.contains(claude_md_content));
        }

        #[test]
        fn test_gather_context_truncates_large_file() {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("progress.db");
            let db = dag::open_db(db_path.to_str().unwrap()).unwrap();

            // Write large CLAUDE.md
            let claude_md_content = "x".repeat(15_000);
            fs::write(temp_dir.path().join("CLAUDE.md"), &claude_md_content).unwrap();

            let project = project::ProjectConfig {
                root: temp_dir.path().to_path_buf(),
                config: project::RalphConfig::default(),
            };

            let context = gather_project_context(&project, &db, false);
            assert!(context.contains("[Truncated -- full file at CLAUDE.md]"));
        }

        #[test]
        fn test_gather_context_with_features() {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("progress.db");
            let db = dag::open_db(db_path.to_str().unwrap()).unwrap();

            // Create features in the database with different statuses
            let _feat1 = feature::create_feature(&db, "feature-one").unwrap();
            let feat2 = feature::create_feature(&db, "feature-two").unwrap();

            // Update one feature to have spec and plan paths
            feature::update_feature_spec_path(&db, &feat2.id, "spec.md").unwrap();
            feature::update_feature_plan_path(&db, &feat2.id, "plan.md").unwrap();
            feature::update_feature_status(&db, &feat2.id, "ready").unwrap();

            let project = project::ProjectConfig {
                root: temp_dir.path().to_path_buf(),
                config: project::RalphConfig::default(),
            };

            let context = gather_project_context(&project, &db, false);

            // Verify the context contains the feature table
            assert!(context.contains("## Project Context"));
            assert!(context.contains("### Existing Features"));
            assert!(context.contains("| Name | Status | Has Spec | Has Plan |"));
            assert!(context.contains("feature-one"));
            assert!(context.contains("feature-two"));
            assert!(context.contains("draft"));
            assert!(context.contains("ready"));
            // Verify checkmarks for spec/plan
            assert!(context.contains("✓"));
        }

        #[test]
        fn test_initial_message_spec_start() {
            let msg = build_initial_message_spec("my-feature", false);
            assert!(msg.contains("Start"));
            assert!(msg.contains("my-feature"));
            assert!(!msg.contains("Resume"));
        }

        #[test]
        fn test_initial_message_spec_resume() {
            let msg = build_initial_message_spec("my-feature", true);
            assert!(msg.contains("Resume"));
            assert!(msg.contains("my-feature"));
            assert!(msg.contains("current spec draft"));
        }

        #[test]
        fn test_initial_message_plan_start() {
            let msg = build_initial_message_plan("my-feature", false);
            assert!(msg.contains("Start"));
            assert!(msg.contains("my-feature"));
            assert!(msg.contains("spec is included"));
        }

        #[test]
        fn test_initial_message_plan_resume() {
            let msg = build_initial_message_plan("my-feature", true);
            assert!(msg.contains("Resume"));
            assert!(msg.contains("my-feature"));
            assert!(msg.contains("current plan draft"));
        }

        #[test]
        fn test_initial_message_task_new() {
            let msg = build_initial_message_task_new();
            assert!(msg.contains("Start the task creation interview"));
        }

        #[test]
        fn test_system_prompt_includes_context() {
            let test_context = "## Project Context\n\n### Test Section\n\nThis is test context.";

            // Test feature spec prompt
            let spec_prompt =
                build_feature_spec_system_prompt("test-feature", "/tmp/spec.md", test_context);
            assert!(spec_prompt.contains(test_context));
            assert!(spec_prompt.contains("## Guidelines"));
            assert!(spec_prompt.contains("## Output"));

            // Test feature plan prompt
            let plan_prompt = build_feature_plan_system_prompt(
                "test-feature",
                "Test spec content",
                "/tmp/plan.md",
                test_context,
            );
            assert!(plan_prompt.contains(test_context));
            assert!(plan_prompt.contains("## Guidelines"));
            assert!(plan_prompt.contains("## Output"));

            // Test task new prompt
            let task_prompt = build_task_new_system_prompt(test_context);
            assert!(task_prompt.contains(test_context));
            assert!(task_prompt.contains("## Guidelines"));
            assert!(task_prompt.contains("## Output"));
        }
    }
}

async fn run() -> Result<ExitCode> {
    let args = cli::Args::parse_args();

    match args.command {
        Some(cli::Command::Init) => {
            project::init()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(cli::Command::Feature { action }) => handle_feature(action).await,
        Some(cli::Command::Task { action }) => handle_task(action).await,
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
            )?;

            output::formatter::print_iteration_info(&config);

            match run_loop::run(config).await? {
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
                    eprintln!(
                        "No plan: DAG is empty. Run 'ralph feature build <name>' to create tasks"
                    );
                    Ok(ExitCode::from(3))
                }
                run_loop::Outcome::Interrupted => {
                    println!("Run interrupted by user.");
                    Ok(ExitCode::SUCCESS)
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
async fn handle_feature(action: cli::FeatureAction) -> Result<ExitCode> {
    let project = project::discover()?;
    let db_path = project.root.join(".ralph/progress.db");
    let db = dag::open_db(db_path.to_str().unwrap())?;

    match action {
        cli::FeatureAction::Spec { name, model } => {
            // Create or get feature
            let feat = if feature::feature_exists(&db, &name)? {
                feature::get_feature(&db, &name)?
            } else {
                feature::create_feature(&db, &name)?
            };

            // Ensure directory structure
            feature::ensure_feature_dirs(&project.root, &name)?;

            let spec_path = project
                .root
                .join(".ralph/features")
                .join(&name)
                .join("spec.md");
            let spec_path_str = spec_path.to_string_lossy().to_string();

            // Detect existing spec for resume
            let existing_spec = feature::read_spec(&project.root, &name).ok();
            let resuming = existing_spec.is_some();

            // Gather project context
            let mut context = gather_project_context(&project, &db, false);

            // If resuming, append existing spec content
            if let Some(spec_content) = existing_spec {
                let truncated =
                    truncate_context(&spec_content, MAX_CONTEXT_FILE_CHARS, &spec_path_str);
                context.push_str(&format!("\n\n## Existing Spec (Resume)\n\n{}", truncated));
            }

            // Build system prompt and initial message
            let system_prompt = build_feature_spec_system_prompt(&name, &spec_path_str, &context);
            let initial_message = build_initial_message_spec(&name, resuming);

            // Launch interactive session (plan mode, always opus)
            claude::interactive::run_interactive(
                &system_prompt,
                &initial_message,
                Some(model.as_deref().unwrap_or("opus")),
                true,
            )?;

            // Iterative review loop
            if spec_path.exists() {
                review::review_document(
                    &spec_path_str,
                    review::DocumentKind::Spec,
                    &name,
                    None,
                    &context,
                )?;
            }

            // Update feature with spec path
            feature::update_feature_spec_path(&db, &feat.id, &spec_path_str)?;

            println!("Feature '{}' spec saved.", name);
            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::Plan { name, model } => {
            // Validate feature exists with spec
            let feat = feature::get_feature(&db, &name)?;
            if feat.spec_path.is_none() {
                anyhow::bail!(
                    "Feature '{}' has no spec. Run 'ralph feature spec {}' first.",
                    name,
                    name
                );
            }

            // Read spec content
            let spec_content = feature::read_spec(&project.root, &name)?;

            let plan_path = project
                .root
                .join(".ralph/features")
                .join(&name)
                .join("plan.md");
            let plan_path_str = plan_path.to_string_lossy().to_string();

            // Detect existing plan for resume
            let existing_plan = feature::read_plan(&project.root, &name).ok();
            let resuming = existing_plan.is_some();

            // Gather project context
            let mut context = gather_project_context(&project, &db, false);

            // If resuming, append existing plan content
            if let Some(plan_content) = existing_plan {
                let truncated =
                    truncate_context(&plan_content, MAX_CONTEXT_FILE_CHARS, &plan_path_str);
                context.push_str(&format!("\n\n## Existing Plan (Resume)\n\n{}", truncated));
            }

            // Build system prompt and initial message
            let system_prompt =
                build_feature_plan_system_prompt(&name, &spec_content, &plan_path_str, &context);
            let initial_message = build_initial_message_plan(&name, resuming);

            // Launch interactive session (plan mode, always opus)
            claude::interactive::run_interactive(
                &system_prompt,
                &initial_message,
                Some(model.as_deref().unwrap_or("opus")),
                true,
            )?;

            // Iterative review loop
            if plan_path.exists() {
                review::review_document(
                    &plan_path_str,
                    review::DocumentKind::Plan,
                    &name,
                    Some(&spec_content),
                    &context,
                )?;
            }

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
                    name,
                    name
                );
            }
            if feat.plan_path.is_none() {
                anyhow::bail!(
                    "Feature '{}' has no plan. Run 'ralph feature plan {}' first.",
                    name,
                    name
                );
            }

            // Read spec + plan content
            let spec_content = feature::read_spec(&project.root, &name)?;
            let plan_content = feature::read_plan(&project.root, &name)?;

            // Create root task for the feature
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
            eprintln!("Created root task: {} Feature: {}", root.id, name);

            // Build system prompt for non-interactive DAG creation
            let system_prompt =
                build_feature_build_system_prompt(&spec_content, &plan_content, &root.id, &feat.id);

            // Launch Claude in streaming mode — it autonomously creates the task DAG
            // via `ralph task add` and `ralph task deps add` CLI commands.
            claude::interactive::run_streaming(
                &system_prompt,
                "Read the spec and plan, then create the task DAG. When done, stop.",
                model.as_deref(),
            )?;

            // Read back from DB and print summary
            let tree = dag::get_task_tree(&db, &root.id)?;
            let child_count = tree.len() - 1; // exclude root

            if child_count == 0 {
                eprintln!("Warning: no child tasks were created under {}", root.id);
            } else {
                eprintln!("\nCreated {} tasks for feature '{}':", child_count, name);
                print_task_tree(&tree, &root.id, "", true);
            }

            // Update feature status
            feature::update_feature_status(&db, &feat.id, "ready")?;

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
async fn handle_task(action: cli::TaskAction) -> Result<ExitCode> {
    let project = project::discover()?;
    let db_path = project.root.join(".ralph/progress.db");
    let db = dag::open_db(db_path.to_str().unwrap())?;

    match action {
        cli::TaskAction::Add {
            title,
            description,
            parent,
            feature,
            priority,
            max_retries,
        } => {
            let task_type = if feature.is_some() {
                "feature"
            } else {
                "standalone"
            };
            let task = dag::create_task_with_feature(
                &db,
                &title,
                description.as_deref(),
                parent.as_deref(),
                priority,
                feature.as_deref(),
                task_type,
                max_retries,
            )?;
            // Print just the ID for scriptability
            println!("{}", task.id);
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Create { model } => {
            // Gather project context including standalone tasks
            let context = gather_project_context(&project, &db, true);

            // Build system prompt and initial message
            let system_prompt = build_task_new_system_prompt(&context);
            let initial_message = build_initial_message_task_new();

            // Launch interactive session
            claude::interactive::run_interactive(
                &system_prompt,
                &initial_message,
                model.as_deref(),
                false,
            )?;
            println!("Task creation session complete.");
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Show { id, json } => {
            let task = dag::get_task(&db, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&task)?);
            } else {
                print_task_details(&db, &task)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::List {
            feature,
            status,
            ready,
            all,
            json,
        } => {
            let tasks = if ready {
                dag::get_ready_tasks(&db)?
            } else if all {
                dag::get_all_tasks(&db)?
            } else if let Some(ref feat_name) = feature {
                let feat = feature::get_feature(&db, feat_name)?;
                dag::get_all_tasks_for_feature(&db, &feat.id)?
            } else {
                dag::get_standalone_tasks(&db)?
            };

            // Apply status filter
            let tasks: Vec<_> = if let Some(ref s) = status {
                tasks.into_iter().filter(|t| t.status == *s).collect()
            } else {
                tasks
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&tasks)?);
                return Ok(ExitCode::SUCCESS);
            }

            if tasks.is_empty() {
                if feature.is_some() {
                    println!("No tasks for this feature.");
                } else {
                    println!("No tasks found. Run 'ralph task add' or 'ralph task create' to create one.");
                }
                return Ok(ExitCode::SUCCESS);
            }

            for task in &tasks {
                let status_display = colorize_status(&task.status);
                println!("  {}  [{}]  {}", task.id, status_display, task.title);
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Update {
            id,
            title,
            description,
            priority,
        } => {
            let fields = dag::TaskUpdate {
                title,
                description,
                priority,
            };
            let updated = dag::update_task(&db, &id, fields)?;
            println!("Updated task {}", updated.id);
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Delete { id } => {
            dag::delete_task(&db, &id)?;
            println!("Deleted task {}", id);
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Done { id } => {
            dag::force_complete_task(db.conn(), &id)?;
            println!("Marked {} as done", id);
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Fail { id, reason } => {
            let reason = reason.as_deref().unwrap_or("Manually marked as failed");
            dag::force_fail_task(db.conn(), &id)?;
            dag::add_log(&db, &id, reason)?;
            println!("Marked {} as failed", id);
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Reset { id } => {
            dag::force_reset_task(db.conn(), &id)?;
            println!("Reset {} to pending", id);
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Log { id, message } => {
            if let Some(msg) = message {
                dag::add_log(&db, &id, &msg)?;
                println!("Added log entry to {}", id);
            } else {
                let logs = dag::get_task_logs(&db, &id)?;
                if logs.is_empty() {
                    println!("No log entries for {}", id);
                } else {
                    for log in &logs {
                        println!("  [{}]  {}", log.timestamp, log.message);
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Deps { action } => match action {
            cli::DepsAction::Add { blocker, blocked } => {
                dag::add_dependency(&db, &blocker, &blocked)?;
                println!(
                    "Added dependency: {} must complete before {}",
                    blocker, blocked
                );
                Ok(ExitCode::SUCCESS)
            }
            cli::DepsAction::Rm { blocker, blocked } => {
                dag::remove_dependency(&db, &blocker, &blocked)?;
                println!("Removed dependency: {} -> {}", blocker, blocked);
                Ok(ExitCode::SUCCESS)
            }
            cli::DepsAction::List { id } => {
                let blockers = dag::get_task_blockers(&db, &id)?;
                let blocked_by_me = dag::get_tasks_blocked_by(&db, &id)?;

                if blockers.is_empty() && blocked_by_me.is_empty() {
                    println!("No dependencies for {}", id);
                    return Ok(ExitCode::SUCCESS);
                }

                if !blockers.is_empty() {
                    println!("  Blocked by:");
                    for t in &blockers {
                        let status_display = colorize_status(&t.status);
                        println!("    {}  [{}]  {}", t.id, status_display, t.title);
                    }
                }
                if !blocked_by_me.is_empty() {
                    println!("  Blocks:");
                    for t in &blocked_by_me {
                        let status_display = colorize_status(&t.status);
                        println!("    {}  [{}]  {}", t.id, status_display, t.title);
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        cli::TaskAction::Tree { id, json } => {
            let tree = dag::get_task_tree(&db, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&tree)?);
            } else {
                print_task_tree(&tree, &id, "", true);
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Colorize a status string for terminal display.
fn colorize_status(status: &str) -> String {
    match status {
        "pending" => status.yellow().to_string(),
        "in_progress" => status.cyan().to_string(),
        "done" => status.green().to_string(),
        "failed" => status.red().to_string(),
        "blocked" => status.dimmed().to_string(),
        _ => status.to_string(),
    }
}

/// Print full task details.
fn print_task_details(db: &dag::Db, task: &dag::Task) -> Result<()> {
    let status_display = colorize_status(&task.status);
    println!("{}  [{}]  {}", task.id, status_display, task.title);

    if let Some(ref pid) = task.parent_id {
        println!("  parent:       {}", pid);
    }
    if let Some(ref fid) = task.feature_id {
        // Try to resolve feature name
        if let Ok(feat) = feature::get_feature_by_id(db, fid) {
            println!("  feature:      {} ({})", feat.name, fid);
        } else {
            println!("  feature:      {}", fid);
        }
    }
    println!("  priority:     {}", task.priority);
    println!("  retries:      {}/{}", task.retry_count, task.max_retries);
    println!(
        "  verification: {}",
        task.verification_status.as_deref().unwrap_or("none")
    );
    println!("  created:      {}", task.created_at);

    if !task.description.is_empty() {
        println!("\n  Description:");
        for line in task.description.lines() {
            println!("    {}", line);
        }
    }

    // Show blockers
    let blockers = dag::get_task_blockers(db, &task.id)?;
    if !blockers.is_empty() {
        println!("\n  Blocked by:");
        for t in &blockers {
            let s = colorize_status(&t.status);
            println!("    {}  [{}]  {}", t.id, s, t.title);
        }
    }

    // Show what this task blocks
    let blocks = dag::get_tasks_blocked_by(db, &task.id)?;
    if !blocks.is_empty() {
        println!("\n  Blocks:");
        for t in &blocks {
            let s = colorize_status(&t.status);
            println!("    {}  [{}]  {}", t.id, s, t.title);
        }
    }

    Ok(())
}

/// Print a task tree with Unicode box-drawing characters.
fn print_task_tree(tree: &[dag::Task], current_id: &str, prefix: &str, is_last: bool) {
    // Find the current task
    let task = match tree.iter().find(|t| t.id == current_id) {
        Some(t) => t,
        None => return,
    };

    let status_display = colorize_status(&task.status);

    if prefix.is_empty() {
        // Root node
        println!("{}  [{}]  {}", task.id, status_display, task.title);
    } else {
        let connector = if is_last { "└─" } else { "├─" };
        println!(
            "{}{} {}  [{}]  {}",
            prefix, connector, task.id, status_display, task.title
        );
    }

    // Find children
    let children: Vec<&dag::Task> = tree
        .iter()
        .filter(|t| t.parent_id.as_deref() == Some(current_id))
        .collect();

    for (i, child) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;
        let child_prefix = if prefix.is_empty() {
            "".to_string()
        } else if is_last {
            format!("{}   ", prefix)
        } else {
            format!("{}│  ", prefix)
        };

        // For root's children, use empty prefix
        let next_prefix = if prefix.is_empty() {
            "".to_string()
        } else {
            child_prefix
        };

        print_task_tree(tree, &child.id, &next_prefix, is_last_child);
    }
}

// --- System prompt builders ---

fn build_feature_spec_system_prompt(name: &str, spec_path: &str, context: &str) -> String {
    format!(
        r#"You are co-authoring a specification for a new project or feature with the user - "{name}".

## Your Role

Interview the user thoroughly to understand their requirements, then write a comprehensive specification document.

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

fn build_feature_plan_system_prompt(
    name: &str,
    spec_content: &str,
    plan_path: &str,
    context: &str,
) -> String {
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

{context}

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
        context = context,
    )
}

fn build_feature_build_system_prompt(
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

## Instructions

1. Read the spec and plan carefully
2. Create parent tasks for logical groupings (as children of `{root_id}`)
3. Create leaf tasks under each parent
4. Add dependencies between tasks where order matters
5. When done, simply stop — Ralph will read the DB and print a summary"#,
        spec_content = spec_content,
        plan_content = plan_content,
        root_id = root_id,
        feature_id = feature_id,
    )
}

fn build_task_new_system_prompt(context: &str) -> String {
    format!(
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

{context}

## Output

After gathering requirements, create the task by outputting:

```json
{{
  "title": "Short imperative title",
  "description": "Detailed description with acceptance criteria",
  "priority": 0
}}
```

The task will be created as a standalone task (not part of any feature)."#,
        context = context,
    )
}

// Context gathering for interactive sessions

const MAX_CONTEXT_FILE_CHARS: usize = 10_000;

/// Truncate content to a character limit with a notice.
/// Uses char_indices() for unicode-safe boundary (not byte slicing).
fn truncate_context(content: &str, limit: usize, file_hint: &str) -> String {
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
fn gather_project_context(
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
fn build_initial_message_spec(name: &str, resuming: bool) -> String {
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
fn build_initial_message_plan(name: &str, resuming: bool) -> String {
    if resuming {
        format!(
            "Resume the plan interview for feature \"{}\". The current plan draft is in your system prompt.",
            name
        )
    } else {
        format!(
            "Start the plan interview for feature \"{}\". The spec is included in your system prompt.",
            name
        )
    }
}

/// Build initial message for task creation interview.
fn build_initial_message_task_new() -> String {
    "Start the task creation interview.".to_string()
}

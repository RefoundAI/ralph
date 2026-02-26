//! Ralph - Autonomous agent loop harness for Claude Code

mod acp;
mod cli;
mod config;
mod dag;
mod feature;
mod feature_prompts;
mod interrupt;
mod journal;
mod knowledge;
mod output;
mod project;
mod review;
mod run_loop;
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
#[allow(clippy::items_after_test_module)]
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
            assert!(
                msg.contains("Discuss"),
                "initial message should ask agent to discuss before writing"
            );
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
            assert!(
                plan_prompt.contains("## Workflow"),
                "plan prompt should have explicit workflow steps"
            );
            assert!(
                plan_prompt.contains("Interview"),
                "plan prompt workflow should start with interview"
            );
            assert!(
                plan_prompt.contains("PLANNING session"),
                "plan prompt should identify as planning session"
            );

            // Test task new prompt
            let task_prompt = build_task_new_system_prompt(test_context);
            assert!(task_prompt.contains(test_context));
            assert!(task_prompt.contains("## Guidelines"));
            assert!(task_prompt.contains("## Creating the Task"));
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
        Some(cli::Command::Auth { agent }) => handle_auth(agent).await,
        Some(cli::Command::Feature { action }) => handle_feature(action).await,
        Some(cli::Command::Task { action }) => handle_task(action).await,
        Some(cli::Command::Run {
            target,
            limit,
            model_strategy,
            model,
            max_retries,
            no_verify,
            agent,
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
                        "Feature '{}' is not ready to run (status: {}). Run 'ralph feature create {}' first.",
                        target, feat.status, target
                    );
                }
                config::RunTarget::Feature(target)
            };

            let config = config::Config::from_run_args(
                limit,
                model_strategy,
                model,
                project,
                Some(run_target),
                max_retries,
                no_verify,
                agent,
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
                        "No plan: DAG is empty. Run 'ralph feature create <name>' to create tasks"
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

/// Handle `ralph auth` — run `claude auth login` for the underlying Claude CLI.
///
/// The ACP agent binary (e.g. `claude-agent-acp`) may not have its own auth command;
/// authentication is managed by the `claude` CLI which the agent delegates to.
async fn handle_auth(_agent: Option<String>) -> Result<ExitCode> {
    println!("{}", "Running: claude auth login".bright_cyan());

    let status = std::process::Command::new("claude")
        .args(["auth", "login"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run 'claude auth login': {e}"))?;

    if status.success() {
        println!("{}", "Authentication successful.".bright_green());
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!(
            "{}",
            format!(
                "Authentication failed (exit code: {}).",
                status.code().unwrap_or(-1)
            )
            .bright_red()
        );
        Ok(ExitCode::FAILURE)
    }
}

/// Handle `ralph feature <action>` subcommands.
async fn handle_feature(action: cli::FeatureAction) -> Result<ExitCode> {
    let project = project::discover()?;
    let db_path = project.root.join(".ralph/progress.db");
    let db = dag::open_db(db_path.to_str().unwrap())?;

    match action {
        cli::FeatureAction::Create { name, model, agent } => {
            // Resolve agent command: --agent flag > RALPH_AGENT env > config > "claude"
            let agent_command = agent
                .or_else(|| std::env::var("RALPH_AGENT").ok())
                .unwrap_or_else(|| project.config.agent.command.clone());

            let model_name = model.as_deref().unwrap_or("opus");

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
            let plan_path = project
                .root
                .join(".ralph/features")
                .join(&name)
                .join("plan.md");
            let plan_path_str = plan_path.to_string_lossy().to_string();

            // ── Phase 1: Spec ────────────────────────────────────────────
            // Skip if feature already has a spec on disk
            let has_spec = spec_path.exists();
            if has_spec {
                eprintln!(
                    "Spec already exists at {}, skipping spec phase.",
                    spec_path_str
                );
            } else {
                eprintln!("\n{}", "Phase 1: Specification".bright_cyan().bold());
                eprintln!("{}\n", "Interview → write spec → review".dimmed());

                // Gather project context
                let context = gather_project_context(&project, &db, false);

                // Build system prompt and initial message
                let system_prompt =
                    build_feature_spec_system_prompt(&name, &spec_path_str, &context);
                let initial_message = build_initial_message_spec(&name, false);

                // Launch interactive session via ACP (no terminal — spec authoring only)
                acp::interactive::run_interactive(
                    &agent_command,
                    &system_prompt,
                    &initial_message,
                    &project.root,
                    Some(model_name),
                    false,
                    Some(vec![spec_path.clone()]),
                )
                .await?;

                // Iterative review loop
                if spec_path.exists() {
                    review::review_document(
                        &spec_path_str,
                        review::DocumentKind::Spec,
                        &name,
                        None,
                        &context,
                        &agent_command,
                        &project.root,
                    )
                    .await?;
                }

                // Update feature with spec path
                feature::update_feature_spec_path(&db, &feat.id, &spec_path_str)?;
                eprintln!("Spec saved to {}", spec_path_str);
            }

            // Bail if spec wasn't actually written
            if !spec_path.exists() {
                anyhow::bail!(
                    "Spec file was not created at {}. Cannot proceed to planning.",
                    spec_path_str
                );
            }

            // ── Phase 2: Plan ────────────────────────────────────────────
            // Skip if feature already has a plan on disk
            let has_plan = plan_path.exists();
            if has_plan {
                eprintln!(
                    "Plan already exists at {}, skipping plan phase.",
                    plan_path_str
                );
            } else {
                eprintln!("\n{}", "Phase 2: Implementation Plan".bright_cyan().bold());
                eprintln!("{}\n", "Interview → write plan → review".dimmed());

                let spec_content = feature::read_spec(&project.root, &name)?;

                // Gather project context
                let context = gather_project_context(&project, &db, false);

                // Build system prompt and initial message
                let system_prompt = build_feature_plan_system_prompt(
                    &name,
                    &spec_content,
                    &plan_path_str,
                    &context,
                );
                let initial_message = build_initial_message_plan(&name, false);

                // Launch interactive session via ACP (no terminal — plan authoring only)
                acp::interactive::run_interactive(
                    &agent_command,
                    &system_prompt,
                    &initial_message,
                    &project.root,
                    Some(model_name),
                    false,
                    Some(vec![plan_path.clone()]),
                )
                .await?;

                // Iterative review loop
                if plan_path.exists() {
                    let spec_content = feature::read_spec(&project.root, &name)?;
                    review::review_document(
                        &plan_path_str,
                        review::DocumentKind::Plan,
                        &name,
                        Some(&spec_content),
                        &context,
                        &agent_command,
                        &project.root,
                    )
                    .await?;
                }

                // Update feature with plan path and status
                feature::update_feature_plan_path(&db, &feat.id, &plan_path_str)?;
                feature::update_feature_status(&db, &feat.id, "planned")?;
                eprintln!("Plan saved to {}", plan_path_str);
            }

            // Bail if plan wasn't actually written
            if !plan_path.exists() {
                anyhow::bail!(
                    "Plan file was not created at {}. Cannot proceed to task creation.",
                    plan_path_str
                );
            }

            // ── Phase 3: Task DAG ────────────────────────────────────────
            eprintln!("\n{}", "Phase 3: Task Decomposition".bright_cyan().bold());
            eprintln!("{}\n", "Creating task DAG from plan".dimmed());

            let spec_content = feature::read_spec(&project.root, &name)?;
            let plan_content = feature::read_plan(&project.root, &name)?;

            // Ensure spec/plan paths are recorded (in case we skipped earlier phases)
            if feat.spec_path.is_none() {
                feature::update_feature_spec_path(&db, &feat.id, &spec_path_str)?;
            }
            if feat.plan_path.is_none() {
                feature::update_feature_plan_path(&db, &feat.id, &plan_path_str)?;
                feature::update_feature_status(&db, &feat.id, "planned")?;
            }

            // Create root task for the feature
            let max_retries = project.config.execution.max_retries as i32;
            let root = dag::create_task_with_feature(
                &db,
                dag::CreateTaskParams {
                    title: &format!("Feature: {}", name),
                    description: Some(&format!("Root task for feature '{}'", name)),
                    parent_id: None,
                    priority: 0,
                    feature_id: Some(&feat.id),
                    task_type: "feature",
                    max_retries,
                },
            )?;
            eprintln!("Created root task: {} Feature: {}", root.id, name);

            // Build system prompt for non-interactive DAG creation
            let system_prompt =
                build_feature_build_system_prompt(&spec_content, &plan_content, &root.id, &feat.id);

            // Launch ACP streaming session — agent autonomously creates the task DAG
            acp::interactive::run_streaming(
                &agent_command,
                &system_prompt,
                "Read the spec and plan, then create the task DAG. When done, stop.",
                &project.root,
                Some(model_name),
            )
            .await?;

            // Read back from DB and print summary
            let tree = dag::get_task_tree(&db, &root.id)?;
            let child_count = tree.len() - 1; // exclude root

            if child_count == 0 {
                eprintln!("Warning: no child tasks were created under {}", root.id);
            } else {
                eprintln!("\nCreated {} tasks for feature '{}':", child_count, name);
                print_task_tree(&tree, &root.id, "", true);
            }

            // Update feature status to ready
            feature::update_feature_status(&db, &feat.id, "ready")?;

            eprintln!(
                "\n{}",
                format!(
                    "Feature '{}' is ready. Run 'ralph run {}' to start.",
                    name, name
                )
                .bright_green()
            );

            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::Delete { name, yes } => {
            let feat = feature::get_feature(&db, &name)?;
            let counts = dag::get_feature_task_counts(&db, &feat.id)?;

            // Show what will be deleted
            println!("Feature: {}", feat.name.bold());
            println!("Status:  {}", colorize_status(&feat.status));
            if counts.total > 0 {
                let in_progress: usize = db.conn().query_row(
                    "SELECT COUNT(*) FROM tasks WHERE feature_id = ? AND status = 'in_progress'",
                    [&feat.id],
                    |row| row.get(0),
                )?;
                println!(
                    "Tasks:   {} total ({} done, {} in progress, {} blocked)",
                    counts.total, counts.done, in_progress, counts.blocked
                );
                if counts.done > 0 && counts.done < counts.total {
                    println!(
                        "{}",
                        "Warning: this feature is partially completed.".yellow()
                    );
                }
                if in_progress > 0 {
                    println!(
                        "{}",
                        "Warning: some tasks are currently in progress.".yellow()
                    );
                }
            }

            // Check for feature directory on disk
            let feature_dir = project.root.join(".ralph/features").join(&name);
            if feature_dir.exists() {
                println!("Files:   {}/", feature_dir.display());
            }

            // Confirm unless --yes
            if !yes {
                eprint!(
                    "\nDelete feature '{}' and all associated data? [y/N] ",
                    name
                );
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Cancelled.");
                    return Ok(ExitCode::SUCCESS);
                }
            }

            // Delete tasks
            let deleted_tasks = dag::delete_tasks_for_feature(&db, &feat.id)?;

            // Delete feature directory on disk
            if feature_dir.exists() {
                std::fs::remove_dir_all(&feature_dir)?;
            }

            // Delete feature from DB
            feature::delete_feature(&db, &feat.id)?;

            println!(
                "{}",
                format!(
                    "Deleted feature '{}' ({} tasks removed).",
                    name, deleted_tasks
                )
                .bright_green()
            );
            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::List => {
            let features = feature::list_features(&db)?;

            if features.is_empty() {
                println!("No features. Run 'ralph feature create <name>' to create one.");
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
                dag::CreateTaskParams {
                    title: &title,
                    description: description.as_deref(),
                    parent_id: parent.as_deref(),
                    priority,
                    feature_id: feature.as_deref(),
                    task_type,
                    max_retries,
                },
            )?;
            // Print just the ID for scriptability
            println!("{}", task.id);
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Create { model, agent } => {
            // Resolve agent command: --agent flag > RALPH_AGENT env > config > "claude"
            let agent_command = agent
                .or_else(|| std::env::var("RALPH_AGENT").ok())
                .unwrap_or_else(|| project.config.agent.command.clone());

            // Gather project context including standalone tasks
            let context = gather_project_context(&project, &db, true);

            // Build system prompt and initial message
            let system_prompt = build_task_new_system_prompt(&context);
            let initial_message = build_initial_message_task_new();

            // Launch interactive session via ACP
            acp::interactive::run_interactive(
                &agent_command,
                &system_prompt,
                &initial_message,
                &project.root,
                model.as_deref(),
                true, // allow_terminal: task creation may need codebase exploration
                None, // no write path restrictions
            )
            .await?;
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

pub(crate) use feature_prompts::{
    build_feature_build_system_prompt, build_feature_plan_system_prompt,
    build_feature_spec_system_prompt, build_initial_message_plan, build_initial_message_spec,
    build_initial_message_task_new, build_task_new_system_prompt, gather_project_context,
};

#[cfg(test)]
pub(crate) use feature_prompts::{truncate_context, MAX_CONTEXT_FILE_CHARS};

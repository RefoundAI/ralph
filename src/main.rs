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
mod ui;
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
    let ui_mode = ui::UiMode::resolve(args.no_ui);

    match args.command {
        Some(cli::Command::Init) => {
            project::init()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(cli::Command::Auth { agent }) => handle_auth(agent).await,
        Some(cli::Command::Feature { action }) => handle_feature(action, ui_mode).await,
        Some(cli::Command::Task { action }) => handle_task(action, ui_mode).await,
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
            ui::theme::init_with_overrides(
                ui::theme::resolve_theme_name(&project.config.ui.theme),
                Some(&project.config.ui.colors),
            );
            let ui_guard = ui::start(ui_mode);

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

            let (exit_code, summary) = match run_loop::run(config).await? {
                run_loop::Outcome::Complete => {
                    output::formatter::print_complete();
                    (ExitCode::SUCCESS, Some("Tasks complete.".to_string()))
                }
                run_loop::Outcome::Failure => {
                    output::formatter::print_failure();
                    (
                        ExitCode::FAILURE,
                        Some("Critical failure. See progress file for details.".to_string()),
                    )
                }
                run_loop::Outcome::LimitReached => {
                    output::formatter::print_limit_reached();
                    (
                        ExitCode::SUCCESS,
                        Some("Iteration limit reached.".to_string()),
                    )
                }
                run_loop::Outcome::Blocked => {
                    output::formatter::print_warning(
                        "Loop blocked: no ready tasks, but incomplete tasks remain",
                    );
                    (ExitCode::from(2), Some("Loop blocked.".to_string()))
                }
                run_loop::Outcome::NoPlan => {
                    output::formatter::print_warning(
                        "No plan: DAG is empty. Run 'ralph feature create <name>' to create tasks",
                    );
                    (ExitCode::from(3), Some("No plan available.".to_string()))
                }
                run_loop::Outcome::Interrupted => {
                    output::formatter::print_warning("Run interrupted by user.");
                    (
                        ExitCode::SUCCESS,
                        Some("Run interrupted by user.".to_string()),
                    )
                }
            };

            if ui_guard.is_active() {
                drop(ui_guard);
                if let Some(line) = summary {
                    println!("{line}");
                }
            }

            Ok(exit_code)
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
    if ui::is_active() {
        ui::stop();
    }

    if let Some(agent) = _agent {
        output::formatter::print_info(&format!(
            "Auth is delegated to `claude auth login`; ignoring --agent={agent}."
        ));
    }
    output::formatter::print_info("Running: claude auth login");

    let status = std::process::Command::new("claude")
        .args(["auth", "login"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run 'claude auth login': {e}"))?;

    if status.success() {
        output::formatter::print_info("Authentication successful.");
        Ok(ExitCode::SUCCESS)
    } else {
        output::formatter::print_error(&format!(
            "Authentication failed (exit code: {}).",
            status.code().unwrap_or(-1)
        ));
        Ok(ExitCode::FAILURE)
    }
}

/// Handle `ralph feature <action>` subcommands.
async fn handle_feature(action: cli::FeatureAction, ui_mode: ui::UiMode) -> Result<ExitCode> {
    let project = project::discover()?;
    ui::theme::init_with_overrides(
        ui::theme::resolve_theme_name(&project.config.ui.theme),
        Some(&project.config.ui.colors),
    );
    let db_path = project.root.join(".ralph/progress.db");
    let db = dag::open_db(db_path.to_str().unwrap())?;

    match action {
        cli::FeatureAction::Create { name, model, agent } => {
            let ui_guard = ui::start(ui_mode);

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
                output::formatter::print_info(&format!(
                    "Spec already exists at {}, skipping spec phase.",
                    spec_path_str
                ));
            } else {
                output::formatter::print_info("Phase 1: Specification");
                output::formatter::print_info("Interview -> write spec -> review");

                // Gather project context
                let context = gather_project_context(&project, &db, false);

                // Build system prompt and initial message
                let system_prompt =
                    build_feature_spec_system_prompt(&name, &spec_path_str, &context);
                let initial_message = build_initial_message_spec(&name, false);

                // Launch interactive session via ACP (no terminal — spec authoring only)
                let _agent_text = acp::interactive::run_interactive(
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
                output::formatter::print_info(&format!("Spec saved to {}", spec_path_str));
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
                output::formatter::print_info(&format!(
                    "Plan already exists at {}, skipping plan phase.",
                    plan_path_str
                ));
            } else {
                output::formatter::print_info("Phase 2: Implementation Plan");
                output::formatter::print_info("Interview -> write plan -> review");

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
                let _agent_text = acp::interactive::run_interactive(
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
                output::formatter::print_info(&format!("Plan saved to {}", plan_path_str));
            }

            // Bail if plan wasn't actually written
            if !plan_path.exists() {
                anyhow::bail!(
                    "Plan file was not created at {}. Cannot proceed to task creation.",
                    plan_path_str
                );
            }

            // ── Phase 3: Task DAG ────────────────────────────────────────
            output::formatter::print_info("Phase 3: Task Decomposition");
            output::formatter::print_info("Creating task DAG from plan");

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
            output::formatter::print_info(&format!(
                "Created root task: {} Feature: {}",
                root.id, name
            ));

            // Build system prompt for non-interactive DAG creation
            let system_prompt =
                build_feature_build_system_prompt(&spec_content, &plan_content, &root.id, &feat.id);

            // Launch ACP streaming session — agent autonomously creates the task DAG
            let _agent_text = acp::interactive::run_streaming(
                &agent_command,
                &system_prompt,
                "Read the spec and plan, then create the task DAG. When done, emit <phase-complete>build</phase-complete> and stop.",
                &project.root,
                Some(model_name),
            )
            .await?;

            // Read back from DB and print summary
            let tree = dag::get_task_tree(&db, &root.id)?;
            let child_count = tree.len() - 1; // exclude root

            if child_count == 0 {
                output::formatter::print_warning(&format!(
                    "Warning: no child tasks were created under {}",
                    root.id
                ));
            } else {
                output::formatter::print_info(&format!(
                    "Created {} tasks for feature '{}'",
                    child_count, name
                ));
                if ui_guard.is_active() {
                    let lines = render_task_tree_lines(&tree, &root.id);
                    let _ = ui::show_explorer("Created Task DAG", lines);
                } else {
                    print_task_tree(&tree, &root.id, "", true);
                }
            }

            // Update feature status to ready
            feature::update_feature_status(&db, &feat.id, "ready")?;

            output::formatter::print_info(&format!(
                "Feature '{}' is ready. Run 'ralph run {}' to start.",
                name, name
            ));

            if ui_guard.is_active() {
                drop(ui_guard);
            }

            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::Delete { name, yes } => {
            let ui_guard = ui::start(ui_mode);
            let feat = feature::get_feature(&db, &name)?;
            let counts = dag::get_feature_task_counts(&db, &feat.id)?;

            // Show what will be deleted
            output::formatter::print_info(&format!("Feature: {}", feat.name.bold()));
            output::formatter::print_info(&format!("Status:  {}", colorize_status(&feat.status)));
            if counts.total > 0 {
                let in_progress: usize = db.conn().query_row(
                    "SELECT COUNT(*) FROM tasks WHERE feature_id = ? AND status = 'in_progress'",
                    [&feat.id],
                    |row| row.get(0),
                )?;
                output::formatter::print_info(&format!(
                    "Tasks:   {} total ({} done, {} in progress, {} blocked)",
                    counts.total, counts.done, in_progress, counts.blocked
                ));
                if counts.done > 0 && counts.done < counts.total {
                    output::formatter::print_warning(
                        "Warning: this feature is partially completed.",
                    );
                }
                if in_progress > 0 {
                    output::formatter::print_warning(
                        "Warning: some tasks are currently in progress.",
                    );
                }
            }

            // Check for feature directory on disk
            let feature_dir = project.root.join(".ralph/features").join(&name);
            if feature_dir.exists() {
                output::formatter::print_info(&format!("Files:   {}/", feature_dir.display()));
            }

            if !confirm_if_ui_active(
                &ui_guard,
                yes,
                "Delete Feature",
                &format!("Delete feature '{}' and all associated data?", name),
                false,
            ) {
                output::formatter::print_info("Cancelled.");
                return Ok(ExitCode::SUCCESS);
            }

            // Delete tasks
            let deleted_tasks = dag::delete_tasks_for_feature(&db, &feat.id)?;

            // Delete feature directory on disk
            if feature_dir.exists() {
                std::fs::remove_dir_all(&feature_dir)?;
            }

            // Delete feature from DB
            feature::delete_feature(&db, &feat.id)?;

            output::formatter::print_info(&format!(
                "Deleted feature '{}' ({} tasks removed).",
                name, deleted_tasks
            ));
            Ok(ExitCode::SUCCESS)
        }
        cli::FeatureAction::List => {
            let features = feature::list_features(&db)?;

            if features.is_empty() {
                output::formatter::print_info(
                    "No features. Run 'ralph feature create <name>' to create one.",
                );
                return Ok(ExitCode::SUCCESS);
            }

            let mut lines: Vec<String> = Vec::new();
            for feat in &features {
                let counts = dag::get_feature_task_counts(&db, &feat.id)?;
                let status_display = feat.status.clone();

                if counts.total > 0 {
                    lines.push(format!(
                        "  {:<16} [{}]  {}/{} done, {} ready",
                        feat.name, status_display, counts.done, counts.total, counts.ready
                    ));
                } else {
                    let detail = match feat.status.as_str() {
                        "draft" => "spec only",
                        "planned" => "spec + plan ready",
                        _ => "",
                    };
                    lines.push(format!(
                        "  {:<16} [{}]  {}",
                        feat.name, status_display, detail
                    ));
                }
            }

            let ui_guard = ui::start(ui_mode);
            if ui_guard.is_active() {
                let _ = ui::show_explorer("Feature Explorer", lines);
            } else {
                for line in lines {
                    println!("{line}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Handle `ralph task <action>` subcommands.
async fn handle_task(action: cli::TaskAction, ui_mode: ui::UiMode) -> Result<ExitCode> {
    let project = project::discover()?;
    ui::theme::init_with_overrides(
        ui::theme::resolve_theme_name(&project.config.ui.theme),
        Some(&project.config.ui.colors),
    );
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
            let ui_guard = ui::start(ui_mode);
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
            let _agent_text = acp::interactive::run_interactive(
                &agent_command,
                &system_prompt,
                &initial_message,
                &project.root,
                model.as_deref(),
                true, // allow_terminal: task creation may need codebase exploration
                None, // no write path restrictions
            )
            .await?;
            output::formatter::print_info("Task creation session complete.");
            if ui_guard.is_active() {
                drop(ui_guard);
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Show { id, json } => {
            let task = dag::get_task(&db, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&task)?);
            } else {
                let lines = render_task_details_lines(&db, &task)?;
                let ui_guard = ui::start(ui_mode);
                if ui_guard.is_active() {
                    let _ = ui::show_explorer(&format!("Task {}", id), lines);
                } else {
                    for line in lines {
                        println!("{line}");
                    }
                }
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
                    output::formatter::print_info("No tasks for this feature.");
                } else {
                    output::formatter::print_info(
                        "No tasks found. Run 'ralph task add' or 'ralph task create' to create one.",
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }

            let mut lines: Vec<String> = Vec::new();
            for task in &tasks {
                lines.push(format!("  {}  [{}]  {}", task.id, task.status, task.title));
            }

            let ui_guard = ui::start(ui_mode);
            if ui_guard.is_active() {
                let _ = ui::show_explorer("Task Explorer", lines);
            } else {
                for line in lines {
                    println!("{line}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Update {
            id,
            title,
            description,
            priority,
        } => {
            let ui_guard = ui::start(ui_mode);
            let fields = dag::TaskUpdate {
                title,
                description,
                priority,
            };
            let updated = dag::update_task(&db, &id, fields)?;
            show_result_if_ui_active(
                &ui_guard,
                "Task Updated",
                vec![format!("Updated task {}", updated.id)],
            );
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Delete { id, yes } => {
            let ui_guard = ui::start(ui_mode);
            if !confirm_if_ui_active(
                &ui_guard,
                yes,
                "Delete Task",
                &format!("Delete task '{}'?", id),
                false,
            ) {
                output::formatter::print_info("Cancelled.");
                return Ok(ExitCode::SUCCESS);
            }
            dag::delete_task(&db, &id)?;
            show_result_if_ui_active(
                &ui_guard,
                "Task Deleted",
                vec![format!("Deleted task {id}")],
            );
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Done { id, yes } => {
            let ui_guard = ui::start(ui_mode);
            if !confirm_if_ui_active(
                &ui_guard,
                yes,
                "Mark Task Done",
                &format!("Mark task '{}' as done?", id),
                false,
            ) {
                output::formatter::print_info("Cancelled.");
                return Ok(ExitCode::SUCCESS);
            }
            dag::force_complete_task(db.conn(), &id)?;
            show_result_if_ui_active(
                &ui_guard,
                "Task Updated",
                vec![format!("Marked {id} as done")],
            );
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Fail { id, reason, yes } => {
            let ui_guard = ui::start(ui_mode);
            if !confirm_if_ui_active(
                &ui_guard,
                yes,
                "Mark Task Failed",
                &format!("Mark task '{}' as failed?", id),
                false,
            ) {
                output::formatter::print_info("Cancelled.");
                return Ok(ExitCode::SUCCESS);
            }
            let reason = reason.as_deref().unwrap_or("Manually marked as failed");
            dag::force_fail_task(db.conn(), &id)?;
            dag::add_log(&db, &id, reason)?;
            show_result_if_ui_active(
                &ui_guard,
                "Task Updated",
                vec![
                    format!("Marked {id} as failed"),
                    format!("Reason: {reason}"),
                ],
            );
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Reset { id, yes } => {
            let ui_guard = ui::start(ui_mode);
            if !confirm_if_ui_active(
                &ui_guard,
                yes,
                "Reset Task",
                &format!("Reset task '{}' to pending?", id),
                false,
            ) {
                output::formatter::print_info("Cancelled.");
                return Ok(ExitCode::SUCCESS);
            }
            dag::force_reset_task(db.conn(), &id)?;
            show_result_if_ui_active(
                &ui_guard,
                "Task Updated",
                vec![format!("Reset {id} to pending")],
            );
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Log { id, message } => {
            let ui_guard = ui::start(ui_mode);
            if let Some(msg) = message {
                dag::add_log(&db, &id, &msg)?;
                show_result_if_ui_active(
                    &ui_guard,
                    "Task Log Updated",
                    vec![format!("Added log entry to {id}")],
                );
            } else {
                let logs = dag::get_task_logs(&db, &id)?;
                if logs.is_empty() {
                    show_result_if_ui_active(
                        &ui_guard,
                        "Task Log",
                        vec![format!("No log entries for {id}")],
                    );
                } else {
                    let lines: Vec<String> = logs
                        .iter()
                        .map(|log| format!("  [{}]  {}", log.timestamp, log.message))
                        .collect();
                    if ui_guard.is_active() {
                        let _ = ui::show_explorer(&format!("Task Log {id}"), lines);
                    } else {
                        for line in lines {
                            println!("{line}");
                        }
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::TaskAction::Deps { action } => match action {
            cli::DepsAction::Add { blocker, blocked } => {
                let ui_guard = ui::start(ui_mode);
                dag::add_dependency(&db, &blocker, &blocked)?;
                show_result_if_ui_active(
                    &ui_guard,
                    "Dependency Added",
                    vec![format!(
                        "Added dependency: {blocker} must complete before {blocked}"
                    )],
                );
                Ok(ExitCode::SUCCESS)
            }
            cli::DepsAction::Rm { blocker, blocked } => {
                let ui_guard = ui::start(ui_mode);
                dag::remove_dependency(&db, &blocker, &blocked)?;
                show_result_if_ui_active(
                    &ui_guard,
                    "Dependency Removed",
                    vec![format!("Removed dependency: {blocker} -> {blocked}")],
                );
                Ok(ExitCode::SUCCESS)
            }
            cli::DepsAction::List { id } => {
                let blockers = dag::get_task_blockers(&db, &id)?;
                let blocked_by_me = dag::get_tasks_blocked_by(&db, &id)?;

                if blockers.is_empty() && blocked_by_me.is_empty() {
                    output::formatter::print_info(&format!("No dependencies for {}", id));
                    return Ok(ExitCode::SUCCESS);
                }

                let mut lines = Vec::new();
                if !blockers.is_empty() {
                    lines.push("  Blocked by:".to_string());
                    for t in &blockers {
                        lines.push(format!("    {}  [{}]  {}", t.id, t.status, t.title));
                    }
                }
                if !blocked_by_me.is_empty() {
                    lines.push("  Blocks:".to_string());
                    for t in &blocked_by_me {
                        lines.push(format!("    {}  [{}]  {}", t.id, t.status, t.title));
                    }
                }

                let ui_guard = ui::start(ui_mode);
                if ui_guard.is_active() {
                    let _ = ui::show_explorer(&format!("Dependencies for {}", id), lines);
                } else {
                    for line in lines {
                        println!("{line}");
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
                let lines = render_task_tree_lines(&tree, &id);
                let ui_guard = ui::start(ui_mode);
                if ui_guard.is_active() {
                    let _ = ui::show_explorer(&format!("Tree {}", id), lines);
                } else {
                    print_task_tree(&tree, &id, "", true);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn confirm_if_ui_active(
    ui_guard: &ui::UiGuard,
    bypass: bool,
    title: &str,
    prompt: &str,
    default_yes: bool,
) -> bool {
    if bypass || !ui_guard.is_active() {
        return true;
    }
    ui::prompt_confirm(title, prompt, default_yes).unwrap_or(false)
}

fn show_result_if_ui_active(ui_guard: &ui::UiGuard, title: &str, lines: Vec<String>) {
    if ui_guard.is_active() {
        let _ = ui::show_explorer(title, lines);
        return;
    }
    for line in lines {
        output::formatter::print_info(&line);
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

fn render_task_details_lines(db: &dag::Db, task: &dag::Task) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    lines.push(format!("{}  [{}]  {}", task.id, task.status, task.title));

    if let Some(ref pid) = task.parent_id {
        lines.push(format!("  parent:       {}", pid));
    }
    if let Some(ref fid) = task.feature_id {
        if let Ok(feat) = feature::get_feature_by_id(db, fid) {
            lines.push(format!("  feature:      {} ({})", feat.name, fid));
        } else {
            lines.push(format!("  feature:      {}", fid));
        }
    }
    lines.push(format!("  priority:     {}", task.priority));
    lines.push(format!(
        "  retries:      {}/{}",
        task.retry_count, task.max_retries
    ));
    lines.push(format!(
        "  verification: {}",
        task.verification_status.as_deref().unwrap_or("none")
    ));
    lines.push(format!("  created:      {}", task.created_at));

    if !task.description.is_empty() {
        lines.push(String::new());
        lines.push("  Description:".to_string());
        for line in task.description.lines() {
            lines.push(format!("    {}", line));
        }
    }

    let blockers = dag::get_task_blockers(db, &task.id)?;
    if !blockers.is_empty() {
        lines.push(String::new());
        lines.push("  Blocked by:".to_string());
        for t in &blockers {
            lines.push(format!("    {}  [{}]  {}", t.id, t.status, t.title));
        }
    }

    let blocks = dag::get_tasks_blocked_by(db, &task.id)?;
    if !blocks.is_empty() {
        lines.push(String::new());
        lines.push("  Blocks:".to_string());
        for t in &blocks {
            lines.push(format!("    {}  [{}]  {}", t.id, t.status, t.title));
        }
    }

    Ok(lines)
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

fn render_task_tree_lines(tree: &[dag::Task], current_id: &str) -> Vec<String> {
    let mut lines = Vec::new();
    render_task_tree_lines_inner(tree, current_id, "", true, &mut lines);
    lines
}

fn render_task_tree_lines_inner(
    tree: &[dag::Task],
    current_id: &str,
    prefix: &str,
    is_last: bool,
    out: &mut Vec<String>,
) {
    let task = match tree.iter().find(|t| t.id == current_id) {
        Some(t) => t,
        None => return,
    };

    if prefix.is_empty() {
        out.push(format!("{}  [{}]  {}", task.id, task.status, task.title));
    } else {
        let connector = if is_last { "└─" } else { "├─" };
        out.push(format!(
            "{}{} {}  [{}]  {}",
            prefix, connector, task.id, task.status, task.title
        ));
    }

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
        let next_prefix = if prefix.is_empty() {
            "".to_string()
        } else {
            child_prefix
        };
        render_task_tree_lines_inner(tree, &child.id, &next_prefix, is_last_child, out);
    }
}

pub(crate) use feature_prompts::{
    build_feature_build_system_prompt, build_feature_plan_system_prompt,
    build_feature_spec_system_prompt, build_initial_message_plan, build_initial_message_spec,
    build_initial_message_task_new, build_task_new_system_prompt, gather_project_context,
};

#[cfg(test)]
pub(crate) use feature_prompts::{truncate_context, MAX_CONTEXT_FILE_CHARS};

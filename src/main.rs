//! Ralph - Autonomous agent loop harness for Claude Code

mod cli;
mod config;
mod claude;
mod dag;
mod sandbox;
mod output;
mod project;
mod run_loop;
mod strategy;

use anyhow::Result;
use clap::Parser;
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
        // Verify all Outcome variants are defined and accessible
        let _complete = run_loop::Outcome::Complete;
        let _failure = run_loop::Outcome::Failure;
        let _limit = run_loop::Outcome::LimitReached;
        let _blocked = run_loop::Outcome::Blocked;
        let _noplan = run_loop::Outcome::NoPlan;
    }

    #[test]
    fn outcome_complete_vs_failure() {
        // Complete and Failure should be different
        assert_ne!(run_loop::Outcome::Complete, run_loop::Outcome::Failure);
    }

    #[test]
    fn outcome_blocked_vs_noplan() {
        // Blocked and NoPlan should be different
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
        Some(cli::Command::Prompt) => {
            let project = project::discover()?;
            let prompts_dir = project.config.prompts.dir.clone();

            // Resolve prompts directory relative to project root
            let resolved_dir = project.root.join(&prompts_dir);
            let prompts_dir_display = resolved_dir.to_string_lossy().to_string();

            // Ensure prompts directory exists
            std::fs::create_dir_all(&resolved_dir)
                .map_err(|e| anyhow::anyhow!("Failed to create prompts directory: {}", e))?;

            let system_prompt = claude::interactive::build_prompt_system_prompt(&prompts_dir_display);
            claude::interactive::run_interactive(&system_prompt)?;

            Ok(ExitCode::SUCCESS)
        }
        Some(cli::Command::Run {
            prompt_file,
            once,
            no_sandbox,
            limit,
            allow,
            model_strategy,
            model,
        }) => {
            // Discover project config (walk up directory tree to find .ralph.toml)
            let project = project::discover()?;

            let config = config::Config::from_run_args(
                prompt_file,
                once,
                no_sandbox,
                limit,
                allow,
                model_strategy,
                model,
                project,
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
                    eprintln!("No plan: DAG is empty. Run 'ralph plan' to create tasks");
                    Ok(ExitCode::from(3))
                }
            }
        }
        Some(cli::Command::Specs) => {
            let project = project::discover()?;
            let specs_dirs = &project.config.specs.dirs;

            // Resolve specs directories relative to project root and ensure they exist
            for specs_dir in specs_dirs {
                let resolved_dir = project.root.join(specs_dir);
                std::fs::create_dir_all(&resolved_dir)
                    .map_err(|e| anyhow::anyhow!("Failed to create specs directory '{}': {}", specs_dir, e))?;
            }

            // Build system prompt with all configured specs directories
            let resolved_dirs: Vec<String> = specs_dirs
                .iter()
                .map(|d| project.root.join(d).to_string_lossy().to_string())
                .collect();
            let system_prompt = claude::interactive::build_specs_system_prompt(&resolved_dirs);
            claude::interactive::run_interactive(&system_prompt)?;

            Ok(ExitCode::SUCCESS)
        }
        Some(cli::Command::Plan { prompt_file, model }) => {
            let project = project::discover()?;
            let prompts_dir = project.root.join(&project.config.prompts.dir);

            // Resolve prompt file path
            let prompt_path = if let Some(ref file) = prompt_file {
                // Explicit file: check prompts dir first, then as-is
                let in_prompts_dir = prompts_dir.join(file);
                if in_prompts_dir.exists() {
                    in_prompts_dir
                } else {
                    let as_is = std::path::PathBuf::from(file);
                    if as_is.exists() {
                        as_is
                    } else {
                        anyhow::bail!(
                            "Prompt file '{}' not found.\nLooked in: {}\n           {}",
                            file,
                            prompts_dir.display(),
                            std::env::current_dir().unwrap_or_default().display()
                        );
                    }
                }
            } else {
                // No file supplied: list prompts from the prompts directory
                let mut prompts: Vec<std::path::PathBuf> = Vec::new();
                if prompts_dir.exists() {
                    if let Ok(entries) = std::fs::read_dir(&prompts_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                                prompts.push(path);
                            }
                        }
                    }
                }
                prompts.sort();

                if prompts.is_empty() {
                    eprintln!("No prompt files found in {}", prompts_dir.display());
                    eprintln!("Run 'ralph prompt' to create one.");
                    return Ok(ExitCode::FAILURE);
                }

                if prompts.len() == 1 {
                    prompts.into_iter().next().unwrap()
                } else {
                    eprintln!("Available prompts:\n");
                    for (i, p) in prompts.iter().enumerate() {
                        eprintln!("  {}. {}", i + 1, p.file_name().unwrap().to_string_lossy());
                    }
                    eprint!("\nSelect a prompt [1-{}]: ", prompts.len());

                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)
                        .map_err(|e| anyhow::anyhow!("Failed to read input: {}", e))?;

                    let choice: usize = input.trim().parse()
                        .map_err(|_| anyhow::anyhow!("Invalid selection: '{}'", input.trim()))?;

                    if choice < 1 || choice > prompts.len() {
                        anyhow::bail!("Selection out of range: {}", choice);
                    }

                    prompts.into_iter().nth(choice - 1).unwrap()
                }
            };

            // Read prompt content
            let prompt_content = std::fs::read_to_string(&prompt_path)
                .map_err(|e| anyhow::anyhow!("Failed to read prompt file '{}': {}", prompt_path.display(), e))?;

            // Read all specs from configured directories
            let mut specs_content = String::new();
            for specs_dir in &project.config.specs.dirs {
                let resolved_dir = project.root.join(specs_dir);
                if !resolved_dir.exists() {
                    continue;
                }

                if let Ok(entries) = std::fs::read_dir(&resolved_dir) {
                    let mut spec_files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
                    spec_files.sort();
                    for path in spec_files {
                        if path.extension().is_some_and(|ext| ext == "md") {
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                specs_content.push_str(&format!("\n## {}\n\n{}\n", path.file_name().unwrap().to_string_lossy(), content));
                            }
                        }
                    }
                }
            }

            if specs_content.is_empty() {
                specs_content = "No specifications available.".to_string();
            }

            // Open the task database
            let db_path = project.root.join(".ralph/progress.db");
            let db = dag::open_db(db_path.to_str().unwrap())?;

            // Check for existing tasks
            let counts = dag::get_task_counts(&db)?;
            if counts.total > 0 {
                anyhow::bail!(
                    "progress.db already has {} tasks. Delete .ralph/progress.db to re-plan.",
                    counts.total
                );
            }

            let plan_model = model.as_deref().unwrap_or("opus");
            eprintln!("Planning with {}...", plan_model);

            // Run Claude with streaming output to get the task breakdown
            let system_prompt = claude::interactive::build_plan_system_prompt(&specs_content);
            let output = claude::interactive::run_streaming(&system_prompt, &prompt_content, plan_model)?;

            // Parse the plan JSON
            let plan = claude::interactive::extract_plan_json(&output)?;

            if plan.tasks.is_empty() {
                anyhow::bail!("Claude returned an empty task list");
            }

            // Insert tasks into the database
            // First pass: create all tasks, mapping temp IDs to real IDs
            let mut id_map = std::collections::HashMap::new();

            for task in &plan.tasks {
                let parent_real_id = task.parent_id.as_ref().and_then(|pid| id_map.get(pid)).cloned();
                let created = dag::create_task(
                    &db,
                    &task.title,
                    Some(&task.description),
                    parent_real_id.as_deref(),
                    task.priority,
                )?;
                eprintln!("  {} {}", created.id, task.title);
                id_map.insert(task.id.clone(), created.id);
            }

            // Second pass: add dependencies
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

            eprintln!("\nCreated {} tasks in progress.db", plan.tasks.len());

            Ok(ExitCode::SUCCESS)
        }
        None => {
            // Bare `ralph` with no subcommand prints help
            cli::Args::parse_from(["ralph", "--help"]);
            Ok(ExitCode::SUCCESS)
        }
    }
}

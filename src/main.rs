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
        Some(cli::Command::Plan { prompt_file }) => {
            let project = project::discover()?;

            // Resolve prompt file (default to "prompt" if not specified)
            let prompt_path = project
                .root
                .join(prompt_file.as_deref().unwrap_or("prompt"));

            // Read prompt content
            let prompt_content = std::fs::read_to_string(&prompt_path)
                .map_err(|e| anyhow::anyhow!("Failed to read prompt file '{}': {}", prompt_path.display(), e))?;

            // Read all specs from configured directories
            let mut specs_content = String::new();
            for specs_dir in &project.config.specs.dirs {
                let resolved_dir = project.root.join(specs_dir);
                if !resolved_dir.exists() {
                    continue; // Skip if directory doesn't exist
                }

                // Read all .md files in the specs directory
                if let Ok(entries) = std::fs::read_dir(&resolved_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
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

            // Build system prompt and launch interactive Claude session
            let system_prompt = claude::interactive::build_plan_system_prompt(&prompt_content, &specs_content);
            claude::interactive::run_interactive(&system_prompt)?;

            Ok(ExitCode::SUCCESS)
        }
        None => {
            // Bare `ralph` with no subcommand prints help
            cli::Args::parse_from(["ralph", "--help"]);
            Ok(ExitCode::SUCCESS)
        }
    }
}

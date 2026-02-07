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
            eprintln!("ralph prompt: not yet implemented");
            Ok(ExitCode::FAILURE)
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
            eprintln!("ralph specs: not yet implemented");
            Ok(ExitCode::FAILURE)
        }
        Some(cli::Command::Plan { prompt_file: _ }) => {
            eprintln!("ralph plan: not yet implemented");
            Ok(ExitCode::FAILURE)
        }
        None => {
            // Bare `ralph` with no subcommand prints help
            cli::Args::parse_from(["ralph", "--help"]);
            Ok(ExitCode::SUCCESS)
        }
    }
}

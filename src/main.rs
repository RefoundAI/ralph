//! Ralph - Autonomous agent loop harness for Claude Code

mod cli;
mod config;
mod claude;
mod sandbox;
mod output;
mod project;
mod run_loop;
mod strategy;

use anyhow::Result;
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

fn run() -> Result<ExitCode> {
    let args = cli::Args::parse_args();

    // Handle init subcommand
    if let Some(cli::Command::Init) = args.command {
        project::init()?;
        return Ok(ExitCode::SUCCESS);
    }

    let config = config::Config::from_args(args)?;

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
    }
}

//! Main iteration loop.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::claude;
use crate::config::Config;
use crate::output::{formatter, logger};
use crate::strategy;

/// Outcome of the loop execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// All DAG tasks done
    Complete,
    /// <promise>FAILURE</promise> emitted
    Failure,
    /// Iteration limit hit
    LimitReached,
    /// No ready tasks, but incomplete tasks exist
    Blocked,
    /// DAG is empty, user must run `ralph plan`
    NoPlan,
}

/// Run the main loop until completion, failure, or limit.
pub fn run(mut config: Config) -> Result<Outcome> {
    // Ensure prompt file exists
    touch_file(&config.prompt_file)?;

    // Progress file is always .ralph/progress.db, created by ralph init
    // No need to touch it here

    loop {
        // Set up logging
        let log_file = logger::setup_log_file();
        println!("Log will be written to: ");
        formatter::hyperlink(&log_file);

        if config.use_sandbox {
            formatter::print_sandbox_warning();
        }

        // Run Claude
        let result = claude::client::run(&config, Some(&log_file))
            .context("Failed to run Claude")?;

        println!("Log available at: ");
        formatter::hyperlink(&log_file);

        // Extract model hint before checking completion/failure
        let next_model_hint = result
            .as_ref()
            .and_then(|r| r.next_model_hint.clone());

        // Check result
        if let Some(ref r) = result {
            if r.is_complete() {
                return Ok(Outcome::Complete);
            }
            if r.is_failure() {
                return Ok(Outcome::Failure);
            }
        }

        // Check limit
        if config.limit_reached() {
            return Ok(Outcome::LimitReached);
        }

        // Continue to next iteration
        formatter::print_separator();
        config = config.next_iteration();

        // Select model for the next iteration based on strategy,
        // passing Claude's hint (if any) from the previous result
        let selection =
            strategy::select_model(&mut config, next_model_hint.as_deref());

        // Log override events when hint disagrees with strategy
        let progress_db = config.project_root.join(".ralph/progress.db");
        if selection.was_overridden {
            strategy::log_model_override(
                progress_db.to_str().unwrap(),
                config.iteration,
                &selection,
            );
        }

        config.current_model = selection.model;

        formatter::print_iteration_info(&config);
    }
}

fn touch_file(path: &str) -> Result<()> {
    if !Path::new(path).exists() {
        fs::write(path, "").context("Failed to create file")?;
    }
    Ok(())
}

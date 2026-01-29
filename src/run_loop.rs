//! Main iteration loop.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::claude;
use crate::config::Config;
use crate::output::{formatter, logger};

/// Outcome of the loop execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Failure,
    LimitReached,
}

/// Run the main loop until completion, failure, or limit.
pub fn run(mut config: Config) -> Result<Outcome> {
    // Ensure prompt and progress files exist
    touch_file(&config.prompt_file)?;
    touch_file(&config.progress_file)?;

    // Check if specs exist; if not, run interactive mode
    if !has_specs(&config.specs_dir) {
        fs::create_dir_all(&config.specs_dir).ok();
        run_interactive_specs()?;
    }

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
        formatter::print_iteration_info(&config);
    }
}

fn touch_file(path: &str) -> Result<()> {
    if !Path::new(path).exists() {
        fs::write(path, "").context("Failed to create file")?;
    }
    Ok(())
}

fn has_specs(dir: &str) -> bool {
    let path = Path::new(dir);
    if !path.is_dir() {
        return false;
    }

    fs::read_dir(path)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .chars()
                    .next()
                    .map(|c| c != '.')
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn run_interactive_specs() -> Result<()> {
    use std::process::Command;

    let status = Command::new("claude")
        .args([
            "--system-prompt",
            r#"You are in interactive specs mode. The user needs help defining specifications
for their project. Ask questions one at a time to understand what they want to build.
Write specification files to the specs/ directory."#,
        ])
        .status()
        .context("Failed to run interactive specs")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Interactive specs exited with code: {}", status);
    }
}

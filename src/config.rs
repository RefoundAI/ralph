//! Configuration struct and validation.

use anyhow::{bail, Result};
use std::env;

use crate::cli::Args;

/// Default allowed tools when not using sandbox.
const DEFAULT_ALLOWED_TOOLS: &[&str] = &[
    "Bash", "Edit", "Write", "Read", "Glob", "Grep", "Task",
    "TodoWrite", "NotebookEdit", "WebFetch", "WebSearch", "mcp__*",
];

#[derive(Debug, Clone)]
pub struct Config {
    pub prompt_file: String,
    pub progress_file: String,
    pub specs_dir: String,
    pub limit: u32,
    pub iteration: u32,
    pub total: u32,
    pub use_sandbox: bool,
    pub allowed_tools: Vec<String>,
    pub allow_rules: Vec<String>,
}

impl Config {
    /// Build config from CLI args.
    pub fn from_args(args: Args) -> Result<Self> {
        // Check for mutually exclusive flags
        if args.once && args.limit.is_some() && args.limit.unwrap() > 0 {
            bail!("--once and --limit are mutually exclusive");
        }

        let prompt_file = args
            .prompt_file
            .unwrap_or_else(|| "prompt".to_string());

        let progress_file = args
            .progress_file
            .unwrap_or_else(|| "progress.txt".to_string());

        let specs_dir = args
            .specs_dir
            .unwrap_or_else(|| "specs".to_string());

        let limit = if args.once {
            1
        } else {
            args.limit.unwrap_or(0)
        };

        let iteration = env::var("RALPH_ITERATION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        let total = env::var("RALPH_TOTAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(limit);

        let use_sandbox = !args.no_sandbox;

        let allowed_tools = if let Some(tools) = args.allowed_tools {
            tools.split_whitespace().map(String::from).collect()
        } else {
            DEFAULT_ALLOWED_TOOLS.iter().map(|s| s.to_string()).collect()
        };

        Ok(Config {
            prompt_file,
            progress_file,
            specs_dir,
            limit,
            iteration,
            total,
            use_sandbox,
            allowed_tools,
            allow_rules: args.allow,
        })
    }

    /// Create config for next iteration.
    pub fn next_iteration(&self) -> Self {
        Config {
            iteration: self.iteration + 1,
            ..self.clone()
        }
    }

    /// Check if iteration limit has been reached.
    pub fn limit_reached(&self) -> bool {
        self.limit > 0 && self.iteration > self.limit
    }
}

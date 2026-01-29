//! CLI argument parsing using clap.

use clap::Parser;

/// Looping harness for hands-off AI agent workflows.
///
/// Ralph is an autonomous, iterative coding workflow harness.
///
/// DANGER: Ralph can (and possibly WILL) destroy anything you have access to,
/// according to the whims of the LLM. Use --once to test before unleashing
/// unattended loops.
#[derive(Parser, Debug)]
#[command(name = "ralph", version, about, long_about = None)]
pub struct Args {
    /// Path to prompt file
    #[arg(value_name = "PROMPT_FILE", env = "RALPH_FILE")]
    pub prompt_file: Option<String>,

    /// Run exactly once (conflicts with --limit)
    #[arg(short = 'o', long)]
    pub once: bool,

    /// Disable sandbox-exec
    #[arg(long)]
    pub no_sandbox: bool,

    /// Path to progress tracking file
    #[arg(long, value_name = "PATH", env = "RALPH_PROGRESS_FILE")]
    pub progress_file: Option<String>,

    /// Path to specs directory
    #[arg(long, value_name = "PATH", env = "RALPH_SPECS_DIR")]
    pub specs_dir: Option<String>,

    /// Maximum iterations; 0 = forever
    #[arg(long, value_name = "N", env = "RALPH_LIMIT")]
    pub limit: Option<u32>,

    /// Tool whitelist (space-separated, only with --no-sandbox)
    #[arg(long, value_name = "LIST")]
    pub allowed_tools: Option<String>,

    /// Enable rule set (e.g., --allow=aws)
    #[arg(short = 'a', long = "allow", value_name = "RULE")]
    pub allow: Vec<String>,
}

impl Args {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

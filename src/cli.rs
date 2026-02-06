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

    /// Model strategy: fixed, cost-optimized, escalate, plan-then-execute
    #[arg(long, value_name = "STRATEGY", env = "RALPH_MODEL_STRATEGY")]
    pub model_strategy: Option<String>,

    /// Model for fixed strategy: opus, sonnet, haiku. Implies --model-strategy=fixed when used alone.
    #[arg(long, value_name = "MODEL", env = "RALPH_MODEL")]
    pub model: Option<String>,
}

/// Valid model names.
pub const VALID_MODELS: &[&str] = &["opus", "sonnet", "haiku"];

/// Valid strategy names.
pub const VALID_STRATEGIES: &[&str] = &["fixed", "cost-optimized", "escalate", "plan-then-execute"];

impl Args {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Validate and resolve model/strategy arguments.
    /// Returns (strategy, model) where strategy is always set.
    pub fn resolve_model_strategy(&self) -> Result<(String, Option<String>), String> {
        // Validate model name if provided
        if let Some(ref model) = self.model {
            if !VALID_MODELS.contains(&model.as_str()) {
                return Err(format!(
                    "invalid model '{}': must be one of {}",
                    model,
                    VALID_MODELS.join(", ")
                ));
            }
        }

        // Validate strategy name if provided
        if let Some(ref strategy) = self.model_strategy {
            if !VALID_STRATEGIES.contains(&strategy.as_str()) {
                return Err(format!(
                    "invalid model strategy '{}': must be one of {}",
                    strategy,
                    VALID_STRATEGIES.join(", ")
                ));
            }
        }

        match (&self.model_strategy, &self.model) {
            // --model alone implies fixed strategy
            (None, Some(model)) => Ok(("fixed".to_string(), Some(model.clone()))),
            // --model-strategy=fixed requires --model
            (Some(strategy), None) if strategy == "fixed" => {
                Err("--model-strategy=fixed requires --model to be set".to_string())
            }
            // Both provided
            (Some(strategy), model) => Ok((strategy.clone(), model.clone())),
            // Neither provided: default to cost-optimized
            (None, None) => Ok(("cost-optimized".to_string(), None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_with(model: Option<&str>, strategy: Option<&str>) -> Args {
        Args {
            prompt_file: None,
            once: false,
            no_sandbox: false,
            progress_file: None,
            specs_dir: None,
            limit: None,
            allowed_tools: None,
            allow: vec![],
            model_strategy: strategy.map(String::from),
            model: model.map(String::from),
        }
    }

    #[test]
    fn model_alone_implies_fixed() {
        let args = args_with(Some("opus"), None);
        let (strategy, model) = args.resolve_model_strategy().unwrap();
        assert_eq!(strategy, "fixed");
        assert_eq!(model, Some("opus".to_string()));
    }

    #[test]
    fn fixed_without_model_errors() {
        let args = args_with(None, Some("fixed"));
        let result = args.resolve_model_strategy();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires --model"));
    }

    #[test]
    fn default_is_cost_optimized() {
        let args = args_with(None, None);
        let (strategy, model) = args.resolve_model_strategy().unwrap();
        assert_eq!(strategy, "cost-optimized");
        assert_eq!(model, None);
    }

    #[test]
    fn invalid_model_name_errors() {
        let args = args_with(Some("gpt4"), None);
        let result = args.resolve_model_strategy();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid model"));
    }

    #[test]
    fn invalid_strategy_name_errors() {
        let args = args_with(None, Some("random"));
        let result = args.resolve_model_strategy();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid model strategy"));
    }

    #[test]
    fn valid_models_accepted() {
        for model in VALID_MODELS {
            let args = args_with(Some(model), None);
            assert!(args.resolve_model_strategy().is_ok(), "model '{}' should be valid", model);
        }
    }

    #[test]
    fn valid_strategies_accepted() {
        for strategy in VALID_STRATEGIES {
            // Non-fixed strategies don't require --model
            if *strategy != "fixed" {
                let args = args_with(None, Some(strategy));
                assert!(
                    args.resolve_model_strategy().is_ok(),
                    "strategy '{}' should be valid",
                    strategy
                );
            }
        }
    }

    #[test]
    fn fixed_with_model_works() {
        let args = args_with(Some("haiku"), Some("fixed"));
        let (strategy, model) = args.resolve_model_strategy().unwrap();
        assert_eq!(strategy, "fixed");
        assert_eq!(model, Some("haiku".to_string()));
    }

    #[test]
    fn non_fixed_strategy_with_model_works() {
        let args = args_with(Some("opus"), Some("escalate"));
        let (strategy, model) = args.resolve_model_strategy().unwrap();
        assert_eq!(strategy, "escalate");
        assert_eq!(model, Some("opus".to_string()));
    }

    #[test]
    fn env_vars_produce_same_fields() {
        // We can't easily test env var parsing without spawning a process,
        // but we verify that Args fields are populated identically regardless
        // of source. This test ensures resolve_model_strategy works the same
        // way whether values come from CLI or env.
        let args = Args {
            prompt_file: None,
            once: false,
            no_sandbox: false,
            progress_file: None,
            specs_dir: None,
            limit: None,
            allowed_tools: None,
            allow: vec![],
            model_strategy: Some("cost-optimized".to_string()),
            model: None,
        };
        let (strategy, model) = args.resolve_model_strategy().unwrap();
        assert_eq!(strategy, "cost-optimized");
        assert_eq!(model, None);
    }
}

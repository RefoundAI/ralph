//! CLI argument parsing using clap.

use clap::{Parser, Subcommand};

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
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize a new Ralph project
    Init,
    /// Manage features (spec, plan, build, list)
    Feature {
        #[command(subcommand)]
        action: FeatureAction,
    },
    /// Manage standalone tasks (new, list)
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// Run the agent loop on a feature or task
    Run {
        /// Feature name or task ID (t-...) to run
        #[arg(value_name = "TARGET")]
        target: String,

        /// Run exactly once (conflicts with --limit)
        #[arg(short = 'o', long)]
        once: bool,

        /// Disable sandbox-exec
        #[arg(long)]
        no_sandbox: bool,

        /// Maximum iterations; 0 = forever
        #[arg(long, value_name = "N", env = "RALPH_LIMIT")]
        limit: Option<u32>,

        /// Enable rule set (e.g., --allow=aws)
        #[arg(short = 'a', long = "allow", value_name = "RULE")]
        allow: Vec<String>,

        /// Model strategy: fixed, cost-optimized, escalate, plan-then-execute
        #[arg(long, value_name = "STRATEGY", env = "RALPH_MODEL_STRATEGY")]
        model_strategy: Option<String>,

        /// Model for fixed strategy: opus, sonnet, haiku. Implies --model-strategy=fixed when used alone.
        #[arg(long, value_name = "MODEL", env = "RALPH_MODEL")]
        model: Option<String>,

        /// Maximum retries for failed tasks
        #[arg(long, value_name = "N")]
        max_retries: Option<u32>,

        /// Disable autonomous verification
        #[arg(long)]
        no_verify: bool,

        /// Disable skill creation + CLAUDE.md updates
        #[arg(long)]
        no_learn: bool,
    },
}

/// Feature subcommands.
#[derive(Subcommand, Debug)]
pub enum FeatureAction {
    /// Interactively craft a specification for a feature
    Spec {
        /// Feature name
        #[arg(value_name = "NAME")]
        name: String,

        /// Model to use: opus (default), sonnet, haiku
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
    /// Interactively create an implementation plan from a spec
    Plan {
        /// Feature name
        #[arg(value_name = "NAME")]
        name: String,

        /// Model to use: opus (default), sonnet, haiku
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
    /// Decompose a plan into a task DAG
    Build {
        /// Feature name
        #[arg(value_name = "NAME")]
        name: String,

        /// Model to use: opus (default), sonnet, haiku
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
    /// List all features and their status
    List,
}

/// Task subcommands.
#[derive(Subcommand, Debug)]
pub enum TaskAction {
    /// Interactively create a new standalone task
    New {
        /// Model to use: opus (default), sonnet, haiku
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
    /// List standalone tasks
    List,
}

/// Valid model names.
pub const VALID_MODELS: &[&str] = &["opus", "sonnet", "haiku"];

/// Valid strategy names.
pub const VALID_STRATEGIES: &[&str] = &["fixed", "cost-optimized", "escalate", "plan-then-execute"];

impl Args {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

/// Validate and resolve model/strategy arguments.
/// Returns (strategy, model) where strategy is always set.
pub fn resolve_model_strategy(
    model: &Option<String>,
    model_strategy: &Option<String>,
) -> Result<(String, Option<String>), String> {
    // Validate model name if provided
    if let Some(ref m) = model {
        if !VALID_MODELS.contains(&m.as_str()) {
            return Err(format!(
                "invalid model '{}': must be one of {}",
                m,
                VALID_MODELS.join(", ")
            ));
        }
    }

    // Validate strategy name if provided
    if let Some(ref strategy) = model_strategy {
        if !VALID_STRATEGIES.contains(&strategy.as_str()) {
            return Err(format!(
                "invalid model strategy '{}': must be one of {}",
                strategy,
                VALID_STRATEGIES.join(", ")
            ));
        }
    }

    match (model_strategy, model) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_alone_implies_fixed() {
        let model = Some("opus".to_string());
        let strategy = None;
        let (resolved_strategy, resolved_model) = resolve_model_strategy(&model, &strategy).unwrap();
        assert_eq!(resolved_strategy, "fixed");
        assert_eq!(resolved_model, Some("opus".to_string()));
    }

    #[test]
    fn fixed_without_model_errors() {
        let model = None;
        let strategy = Some("fixed".to_string());
        let result = resolve_model_strategy(&model, &strategy);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires --model"));
    }

    #[test]
    fn default_is_cost_optimized() {
        let model = None;
        let strategy = None;
        let (resolved_strategy, resolved_model) = resolve_model_strategy(&model, &strategy).unwrap();
        assert_eq!(resolved_strategy, "cost-optimized");
        assert_eq!(resolved_model, None);
    }

    #[test]
    fn invalid_model_name_errors() {
        let model = Some("gpt4".to_string());
        let strategy = None;
        let result = resolve_model_strategy(&model, &strategy);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid model"));
    }

    #[test]
    fn invalid_strategy_name_errors() {
        let model = None;
        let strategy = Some("random".to_string());
        let result = resolve_model_strategy(&model, &strategy);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid model strategy"));
    }

    #[test]
    fn valid_models_accepted() {
        for model_name in VALID_MODELS {
            let model = Some(model_name.to_string());
            let strategy = None;
            assert!(
                resolve_model_strategy(&model, &strategy).is_ok(),
                "model '{}' should be valid",
                model_name
            );
        }
    }

    #[test]
    fn valid_strategies_accepted() {
        for strategy_name in VALID_STRATEGIES {
            // Non-fixed strategies don't require --model
            if *strategy_name != "fixed" {
                let model = None;
                let strategy = Some(strategy_name.to_string());
                assert!(
                    resolve_model_strategy(&model, &strategy).is_ok(),
                    "strategy '{}' should be valid",
                    strategy_name
                );
            }
        }
    }

    #[test]
    fn fixed_with_model_works() {
        let model = Some("haiku".to_string());
        let strategy = Some("fixed".to_string());
        let (resolved_strategy, resolved_model) = resolve_model_strategy(&model, &strategy).unwrap();
        assert_eq!(resolved_strategy, "fixed");
        assert_eq!(resolved_model, Some("haiku".to_string()));
    }

    #[test]
    fn non_fixed_strategy_with_model_works() {
        let model = Some("opus".to_string());
        let strategy = Some("escalate".to_string());
        let (resolved_strategy, resolved_model) = resolve_model_strategy(&model, &strategy).unwrap();
        assert_eq!(resolved_strategy, "escalate");
        assert_eq!(resolved_model, Some("opus".to_string()));
    }
}

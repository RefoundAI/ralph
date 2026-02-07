//! Configuration struct and validation.

use anyhow::{bail, Result};
use std::env;
use std::fmt;
use std::path::PathBuf;

use crate::cli::Args;
use crate::project::{ProjectConfig, RalphConfig};

/// Default allowed tools when not using sandbox.
const DEFAULT_ALLOWED_TOOLS: &[&str] = &[
    "Bash", "Edit", "Write", "Read", "Glob", "Grep", "Task",
    "TodoWrite", "NotebookEdit", "WebFetch", "WebSearch", "mcp__*",
];

/// Model strategy for selecting Claude models across loop iterations.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelStrategy {
    /// Always use the configured model.
    Fixed,
    /// Pick cheapest model that can handle each iteration based on heuristics.
    CostOptimized,
    /// Start cheap, escalate on failure signals.
    Escalate,
    /// Use opus for planning (iteration 1), sonnet for execution.
    PlanThenExecute,
}

impl fmt::Display for ModelStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelStrategy::Fixed => write!(f, "fixed"),
            ModelStrategy::CostOptimized => write!(f, "cost-optimized"),
            ModelStrategy::Escalate => write!(f, "escalate"),
            ModelStrategy::PlanThenExecute => write!(f, "plan-then-execute"),
        }
    }
}

impl ModelStrategy {
    /// Parse a strategy name string into a ModelStrategy enum variant.
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "fixed" => Ok(ModelStrategy::Fixed),
            "cost-optimized" => Ok(ModelStrategy::CostOptimized),
            "escalate" => Ok(ModelStrategy::Escalate),
            "plan-then-execute" => Ok(ModelStrategy::PlanThenExecute),
            _ => bail!("invalid model strategy '{}'", s),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub prompt_file: String,
    pub limit: u32,
    pub iteration: u32,
    pub total: u32,
    pub use_sandbox: bool,
    pub allowed_tools: Vec<String>,
    pub allow_rules: Vec<String>,
    /// The active model strategy for this run.
    pub model_strategy: ModelStrategy,
    /// The model name for `Fixed` strategy (only required when strategy is Fixed).
    pub model: Option<String>,
    /// The model selected for the current iteration, updated each loop.
    pub current_model: String,
    /// Escalation level for the `Escalate` strategy (0=haiku, 1=sonnet, 2=opus).
    /// Tracks the minimum model tier; the strategy never auto-de-escalates below this.
    pub escalation_level: u8,
    /// Project root directory (directory containing .ralph.toml).
    pub project_root: PathBuf,
    /// Parsed project configuration.
    pub ralph_config: RalphConfig,
}

impl Config {
    /// Build config from CLI args and project config.
    pub fn from_args(args: Args, project: ProjectConfig) -> Result<Self> {
        // Check for mutually exclusive flags
        if args.once && args.limit.is_some() && args.limit.unwrap() > 0 {
            bail!("--once and --limit are mutually exclusive");
        }

        // Resolve model strategy early (before consuming args fields)
        let (strategy_str, model) = args
            .resolve_model_strategy()
            .map_err(|e| anyhow::anyhow!(e))?;

        let model_strategy = ModelStrategy::from_str(&strategy_str)?;

        // Validate: fixed strategy requires a model
        if model_strategy == ModelStrategy::Fixed && model.is_none() {
            bail!("--model-strategy=fixed requires --model to be set");
        }

        // Determine initial current_model based on strategy
        let current_model = match &model_strategy {
            ModelStrategy::Fixed => model.clone().unwrap(), // safe: validated above
            ModelStrategy::CostOptimized => "sonnet".to_string(),
            ModelStrategy::Escalate => "haiku".to_string(),
            ModelStrategy::PlanThenExecute => "opus".to_string(),
        };

        let prompt_file = args
            .prompt_file
            .unwrap_or_else(|| "prompt".to_string());

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
            limit,
            iteration,
            total,
            use_sandbox,
            allowed_tools,
            allow_rules: args.allow,
            model_strategy,
            model,
            current_model,
            escalation_level: 0,
            project_root: project.root,
            ralph_config: project.config,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use crate::project::{ProjectConfig, RalphConfig, SpecsConfig, PromptsConfig};

    /// Helper to build Args with model fields set.
    fn args_with_model(model: Option<&str>, strategy: Option<&str>) -> Args {
        Args {
            command: None,
            prompt_file: None,
            once: false,
            no_sandbox: false,
            limit: None,
            allowed_tools: None,
            allow: vec![],
            model_strategy: strategy.map(String::from),
            model: model.map(String::from),
        }
    }

    /// Helper to build a test ProjectConfig.
    fn test_project() -> ProjectConfig {
        ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig {
                specs: SpecsConfig { dirs: vec![".ralph/specs".to_string()] },
                prompts: PromptsConfig { dir: ".ralph/prompts".to_string() },
            },
        }
    }

    #[test]
    fn config_default_strategy_is_cost_optimized() {
        let args = args_with_model(None, None);
        let config = Config::from_args(args, test_project()).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::CostOptimized);
        assert_eq!(config.model, None);
        assert_eq!(config.current_model, "sonnet");
    }

    #[test]
    fn config_fixed_strategy_requires_model() {
        let args = args_with_model(None, Some("fixed"));
        let result = Config::from_args(args, test_project());
        assert!(result.is_err());
    }

    #[test]
    fn config_fixed_strategy_with_model() {
        let args = args_with_model(Some("opus"), Some("fixed"));
        let config = Config::from_args(args, test_project()).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::Fixed);
        assert_eq!(config.model, Some("opus".to_string()));
        assert_eq!(config.current_model, "opus");
    }

    #[test]
    fn config_model_alone_implies_fixed() {
        let args = args_with_model(Some("haiku"), None);
        let config = Config::from_args(args, test_project()).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::Fixed);
        assert_eq!(config.current_model, "haiku");
    }

    #[test]
    fn config_escalate_starts_at_haiku() {
        let args = args_with_model(None, Some("escalate"));
        let config = Config::from_args(args, test_project()).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::Escalate);
        assert_eq!(config.current_model, "haiku");
    }

    #[test]
    fn config_plan_then_execute_starts_at_opus() {
        let args = args_with_model(None, Some("plan-then-execute"));
        let config = Config::from_args(args, test_project()).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::PlanThenExecute);
        assert_eq!(config.current_model, "opus");
    }

    #[test]
    fn config_next_iteration_preserves_strategy() {
        let args = args_with_model(Some("sonnet"), Some("fixed"));
        let config = Config::from_args(args, test_project()).unwrap();
        let next = config.next_iteration();
        assert_eq!(next.model_strategy, ModelStrategy::Fixed);
        assert_eq!(next.model, Some("sonnet".to_string()));
        assert_eq!(next.current_model, "sonnet");
        assert_eq!(next.iteration, 2);
    }

    #[test]
    fn model_strategy_display() {
        assert_eq!(ModelStrategy::Fixed.to_string(), "fixed");
        assert_eq!(ModelStrategy::CostOptimized.to_string(), "cost-optimized");
        assert_eq!(ModelStrategy::Escalate.to_string(), "escalate");
        assert_eq!(ModelStrategy::PlanThenExecute.to_string(), "plan-then-execute");
    }

    #[test]
    fn model_strategy_from_str_valid() {
        assert_eq!(ModelStrategy::from_str("fixed").unwrap(), ModelStrategy::Fixed);
        assert_eq!(ModelStrategy::from_str("cost-optimized").unwrap(), ModelStrategy::CostOptimized);
        assert_eq!(ModelStrategy::from_str("escalate").unwrap(), ModelStrategy::Escalate);
        assert_eq!(ModelStrategy::from_str("plan-then-execute").unwrap(), ModelStrategy::PlanThenExecute);
    }

    #[test]
    fn model_strategy_from_str_invalid() {
        assert!(ModelStrategy::from_str("invalid").is_err());
    }
}

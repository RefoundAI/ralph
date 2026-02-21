//! Configuration struct and validation.

use anyhow::{bail, Result};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli;
use crate::project::{ProjectConfig, RalphConfig};

/// Default allowed tools when not using sandbox.
const DEFAULT_ALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "Edit",
    "Write",
    "Read",
    "Glob",
    "Grep",
    "Task",
    "TodoWrite",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "mcp__*",
];

/// Target for the `ralph run` command.
#[derive(Debug, Clone)]
pub enum RunTarget {
    /// Run a feature by name.
    Feature(String),
    /// Run a standalone task by ID (t-...).
    Task(String),
}

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

/// Generate a unique agent ID: `agent-{8 hex chars}`.
/// Uses a hash of timestamp and process ID.
fn generate_agent_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();

    let mut hasher = DefaultHasher::new();
    timestamp.hash(&mut hasher);
    pid.hash(&mut hasher);
    let hash = hasher.finish();

    // Take the lower 32 bits and convert to 8 hex chars
    let short_hash = (hash & 0xFFFFFFFF) as u32;
    format!("agent-{:08x}", short_hash)
}

/// Generate a unique run ID: `run-{8 hex chars}`.
/// Uses a hash of timestamp and process ID.
fn generate_run_id() -> String {
    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    let hash = hasher.finish();
    format!("run-{:08x}", hash as u32)
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
    #[allow(dead_code)]
    pub ralph_config: RalphConfig,
    /// Unique agent ID for this run (format: agent-{8 hex chars}).
    pub agent_id: String,
    /// Maximum retries for failed tasks.
    pub max_retries: u32,
    /// Whether to enable autonomous verification.
    pub verify: bool,
    /// Unique run ID for this invocation (format: run-{8 hex chars}).
    pub run_id: String,
    /// The target for this run (feature or standalone task).
    pub run_target: Option<RunTarget>,
}

impl Config {
    /// Build config from run command args and project config.
    #[allow(clippy::too_many_arguments)]
    pub fn from_run_args(
        prompt_file: Option<String>,
        once: bool,
        no_sandbox: bool,
        limit: Option<u32>,
        allow: Vec<String>,
        model_strategy: Option<String>,
        model: Option<String>,
        project: ProjectConfig,
        run_target: Option<RunTarget>,
        max_retries_override: Option<u32>,
        no_verify: bool,
    ) -> Result<Self> {
        // Check for mutually exclusive flags
        if once && limit.is_some() && limit.unwrap() > 0 {
            bail!("--once and --limit are mutually exclusive");
        }

        // Resolve model strategy early
        let (strategy_str, model) =
            cli::resolve_model_strategy(&model, &model_strategy).map_err(|e| anyhow::anyhow!(e))?;

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

        let prompt_file = prompt_file.unwrap_or_else(|| "prompt".to_string());

        let limit = if once { 1 } else { limit.unwrap_or(0) };

        let iteration = env::var("RALPH_ITERATION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        let total = env::var("RALPH_TOTAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(limit);

        let use_sandbox = !no_sandbox;

        let allowed_tools = DEFAULT_ALLOWED_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect();

        let execution = &project.config.execution;
        let max_retries = max_retries_override.unwrap_or(execution.max_retries);
        let verify = !no_verify && execution.verify;

        Ok(Config {
            prompt_file,
            limit,
            iteration,
            total,
            use_sandbox,
            allowed_tools,
            allow_rules: allow,
            model_strategy,
            model,
            current_model,
            escalation_level: 0,
            project_root: project.root,
            ralph_config: project.config,
            agent_id: generate_agent_id(),
            max_retries,
            verify,
            run_id: generate_run_id(),
            run_target,
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
    use crate::project::{ProjectConfig, RalphConfig};

    /// Helper to build a test ProjectConfig.
    fn test_project() -> ProjectConfig {
        ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig::default(),
        }
    }

    /// Helper to build config from run args.
    fn config_from_run(model: Option<&str>, strategy: Option<&str>) -> Result<Config> {
        Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            strategy.map(String::from),
            model.map(String::from),
            test_project(),
            None,
            None,
            false,
        )
    }

    #[test]
    fn config_default_strategy_is_cost_optimized() {
        let config = config_from_run(None, None).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::CostOptimized);
        assert_eq!(config.model, None);
        assert_eq!(config.current_model, "sonnet");
    }

    #[test]
    fn config_fixed_strategy_requires_model() {
        let result = config_from_run(None, Some("fixed"));
        assert!(result.is_err());
    }

    #[test]
    fn config_fixed_strategy_with_model() {
        let config = config_from_run(Some("opus"), Some("fixed")).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::Fixed);
        assert_eq!(config.model, Some("opus".to_string()));
        assert_eq!(config.current_model, "opus");
    }

    #[test]
    fn config_model_alone_implies_fixed() {
        let config = config_from_run(Some("haiku"), None).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::Fixed);
        assert_eq!(config.current_model, "haiku");
    }

    #[test]
    fn config_escalate_starts_at_haiku() {
        let config = config_from_run(None, Some("escalate")).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::Escalate);
        assert_eq!(config.current_model, "haiku");
    }

    #[test]
    fn config_plan_then_execute_starts_at_opus() {
        let config = config_from_run(None, Some("plan-then-execute")).unwrap();
        assert_eq!(config.model_strategy, ModelStrategy::PlanThenExecute);
        assert_eq!(config.current_model, "opus");
    }

    #[test]
    fn config_next_iteration_preserves_strategy() {
        let config = config_from_run(Some("sonnet"), Some("fixed")).unwrap();
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
        assert_eq!(
            ModelStrategy::PlanThenExecute.to_string(),
            "plan-then-execute"
        );
    }

    #[test]
    fn model_strategy_from_str_valid() {
        assert_eq!(
            ModelStrategy::from_str("fixed").unwrap(),
            ModelStrategy::Fixed
        );
        assert_eq!(
            ModelStrategy::from_str("cost-optimized").unwrap(),
            ModelStrategy::CostOptimized
        );
        assert_eq!(
            ModelStrategy::from_str("escalate").unwrap(),
            ModelStrategy::Escalate
        );
        assert_eq!(
            ModelStrategy::from_str("plan-then-execute").unwrap(),
            ModelStrategy::PlanThenExecute
        );
    }

    #[test]
    fn model_strategy_from_str_invalid() {
        assert!(ModelStrategy::from_str("invalid").is_err());
    }

    #[test]
    fn config_has_agent_id() {
        let config = config_from_run(None, None).unwrap();
        assert!(!config.agent_id.is_empty());
        assert!(config.agent_id.starts_with("agent-"));
    }

    #[test]
    fn agent_id_matches_format() {
        let config = config_from_run(None, None).unwrap();
        // Format: agent-{8 hex chars}
        assert_eq!(config.agent_id.len(), 14); // "agent-" (6) + 8 hex chars
        assert!(config
            .agent_id
            .chars()
            .skip(6)
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn agent_id_different_on_separate_invocations() {
        let config1 = config_from_run(None, None).unwrap();
        let config2 = config_from_run(None, None).unwrap();

        // Two invocations should (very likely) produce different IDs
        // Note: this is probabilistic, but collision chance is extremely low with 32-bit hash
        assert_ne!(config1.agent_id, config2.agent_id);
    }

    #[test]
    fn agent_id_preserved_across_iterations() {
        let config = config_from_run(None, None).unwrap();
        let agent_id_1 = config.agent_id.clone();

        let next = config.next_iteration();
        assert_eq!(next.agent_id, agent_id_1);
    }

    #[test]
    fn test_config_has_run_id() {
        let config = config_from_run(None, None).unwrap();
        assert!(!config.run_id.is_empty());
        assert!(config.run_id.starts_with("run-"));
        // Format: run-{8 hex chars}
        assert_eq!(config.run_id.len(), 12); // "run-" (4) + 8 hex chars
        assert!(config.run_id.chars().skip(4).all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_execution_config_backward_compat() {
        use crate::project::RalphConfig;
        // TOML with learn = false should parse without error (backward compat)
        let toml_content = r#"
[execution]
learn = false
max_retries = 3
verify = true
"#;
        let config: RalphConfig = toml::from_str(toml_content).unwrap();
        // learn field is retained in ExecutionConfig for backward compat
        assert_eq!(config.execution.max_retries, 3);
        assert!(config.execution.verify);
    }
}

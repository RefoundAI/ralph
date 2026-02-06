//! Model strategy selection logic.
//!
//! Determines which Claude model to use for each iteration based on the
//! configured strategy.

use crate::config::{Config, ModelStrategy};

/// Select the model for the current iteration based on the active strategy.
///
/// Returns the model name to use (e.g. "opus", "sonnet", "haiku").
/// The `next_model_hint` parameter allows Claude to override the strategy's
/// choice via the `<next-model>` sigil (wired in a later task).
pub fn select_model(config: &Config, next_model_hint: Option<&str>) -> String {
    // Claude hint always wins if provided
    if let Some(hint) = next_model_hint {
        return hint.to_string();
    }

    match config.model_strategy {
        ModelStrategy::Fixed => select_fixed(config),
        // Non-fixed strategies return current_model for now;
        // each will be implemented in subsequent tasks (R4-R6).
        ModelStrategy::CostOptimized => config.current_model.clone(),
        ModelStrategy::Escalate => config.current_model.clone(),
        ModelStrategy::PlanThenExecute => config.current_model.clone(),
    }
}

/// Fixed strategy: always return the configured model, unconditionally.
fn select_fixed(config: &Config) -> String {
    config
        .model
        .clone()
        .expect("fixed strategy requires model to be set")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use crate::config::Config;

    /// Helper to build a Config with fixed strategy.
    fn fixed_config(model: &str) -> Config {
        let args = Args {
            prompt_file: None,
            once: false,
            no_sandbox: false,
            progress_file: None,
            specs_dir: None,
            limit: None,
            allowed_tools: None,
            allow: vec![],
            model_strategy: Some("fixed".to_string()),
            model: Some(model.to_string()),
        };
        Config::from_args(args).unwrap()
    }

    #[test]
    fn fixed_strategy_returns_configured_model() {
        let config = fixed_config("opus");
        assert_eq!(select_model(&config, None), "opus");
    }

    #[test]
    fn fixed_strategy_returns_haiku_when_configured() {
        let config = fixed_config("haiku");
        assert_eq!(select_model(&config, None), "haiku");
    }

    #[test]
    fn fixed_strategy_returns_sonnet_when_configured() {
        let config = fixed_config("sonnet");
        assert_eq!(select_model(&config, None), "sonnet");
    }

    #[test]
    fn fixed_strategy_same_model_across_iterations() {
        let mut config = fixed_config("opus");
        for i in 1..=10 {
            assert_eq!(
                select_model(&config, None),
                "opus",
                "iteration {} should still return opus",
                i
            );
            config = config.next_iteration();
        }
    }

    #[test]
    fn fixed_strategy_ignores_iteration_number() {
        let config = fixed_config("haiku");
        // Simulate being at iteration 50
        let mut cfg = config;
        for _ in 0..49 {
            cfg = cfg.next_iteration();
        }
        assert_eq!(cfg.iteration, 50);
        assert_eq!(select_model(&cfg, None), "haiku");
    }

    #[test]
    fn hint_overrides_fixed_strategy() {
        let config = fixed_config("haiku");
        assert_eq!(select_model(&config, Some("opus")), "opus");
    }
}

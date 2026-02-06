//! Model strategy selection logic.
//!
//! Determines which Claude model to use for each iteration based on the
//! configured strategy.

use std::fs;

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
        ModelStrategy::CostOptimized => select_cost_optimized(config),
        // Non-fixed strategies return current_model for now;
        // each will be implemented in subsequent tasks (R5-R6).
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

/// Cost-optimized strategy: read the progress file and pick the cheapest
/// model that can handle the current iteration.
fn select_cost_optimized(config: &Config) -> String {
    let content = fs::read_to_string(&config.progress_file).unwrap_or_default();
    analyze_progress(&content, config.iteration)
}

/// Pure heuristic that analyzes progress file content and iteration number
/// to select the cheapest adequate model.
///
/// Rules (in priority order):
/// 1. If progress mentions errors, failures, or being stuck → `opus`
/// 2. If progress shows steady completion of simple tasks (multiple DONE
///    entries with no error signals) → `haiku`
/// 3. Otherwise (empty, ambiguous, early iterations) → `sonnet`
fn analyze_progress(content: &str, _iteration: u32) -> String {
    let lower = content.to_lowercase();

    // Check for distress signals → escalate to opus
    let error_signals = [
        "error",
        "failure",
        "failed",
        "stuck",
        "cannot",
        "unable",
        "panic",
        "crash",
        "bug",
        "broken",
        "regression",
        "FAILURE",
    ];
    let has_errors = error_signals
        .iter()
        .any(|signal| lower.contains(&signal.to_lowercase()));

    if has_errors {
        return "opus".to_string();
    }

    // Check for steady progress signals → downgrade to haiku
    // Look for multiple completed task markers (e.g. "[R1] DONE", "DONE —", "✓", "completed")
    let done_count = lower.matches("done").count()
        + lower.matches("completed").count()
        + lower.matches("✓").count();

    // Only use haiku if there are multiple completions and the content is
    // non-trivial (indicates a pattern of simple, successful work)
    if done_count >= 3 && !content.trim().is_empty() {
        return "haiku".to_string();
    }

    // Default: sonnet (uncertain, empty progress, early iterations, etc.)
    "sonnet".to_string()
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

    // --- cost-optimized strategy tests (analyze_progress heuristic) ---

    #[test]
    fn cost_optimized_empty_progress_returns_sonnet() {
        assert_eq!(analyze_progress("", 1), "sonnet");
    }

    #[test]
    fn cost_optimized_ambiguous_progress_returns_sonnet() {
        assert_eq!(
            analyze_progress("Some work was done but unclear status.", 3),
            "sonnet"
        );
    }

    #[test]
    fn cost_optimized_error_signals_return_opus() {
        let content = "[R1] DONE — Implemented feature X\n\
                        [R2] FAILED — Tests are broken, error in parser";
        assert_eq!(analyze_progress(content, 3), "opus");
    }

    #[test]
    fn cost_optimized_failure_signal_returns_opus() {
        let content = "Iteration 5: stuck on a regression in the build system.";
        assert_eq!(analyze_progress(content, 5), "opus");
    }

    #[test]
    fn cost_optimized_stuck_signal_returns_opus() {
        let content = "Cannot proceed, stuck on dependency issue.";
        assert_eq!(analyze_progress(content, 2), "opus");
    }

    #[test]
    fn cost_optimized_steady_completions_return_haiku() {
        let content = "[R1] DONE — Added CLI flags\n\
                        [R2] DONE — Added config fields\n\
                        [R3] DONE — Implemented fixed strategy";
        assert_eq!(analyze_progress(content, 4), "haiku");
    }

    #[test]
    fn cost_optimized_two_completions_still_returns_sonnet() {
        // Only 2 DONE entries — not enough to confidently use haiku
        let content = "[R1] DONE — Added CLI flags\n\
                        [R2] DONE — Added config fields";
        assert_eq!(analyze_progress(content, 3), "sonnet");
    }

    #[test]
    fn cost_optimized_errors_override_completions() {
        // Even with many completions, error signals take priority
        let content = "[R1] DONE — Added CLI flags\n\
                        [R2] DONE — Added config fields\n\
                        [R3] DONE — Implemented strategy\n\
                        [R4] Error: tests are failing";
        assert_eq!(analyze_progress(content, 5), "opus");
    }

    #[test]
    fn cost_optimized_default_iteration_1_returns_sonnet() {
        assert_eq!(analyze_progress("", 1), "sonnet");
    }

    #[test]
    fn hint_overrides_cost_optimized_strategy() {
        // Even though analyze_progress would return sonnet for empty content,
        // the hint should win
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
        let config = Config::from_args(args).unwrap();
        assert_eq!(select_model(&config, Some("haiku")), "haiku");
    }
}

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
///
/// For the `Escalate` strategy, this mutates `config.escalation_level` to
/// track upward movement through the model tiers.
pub fn select_model(config: &mut Config, next_model_hint: Option<&str>) -> String {
    // Claude hint always wins if provided.
    // For escalate strategy, a hint can also de-escalate the level.
    if let Some(hint) = next_model_hint {
        if config.model_strategy == ModelStrategy::Escalate {
            let hint_level = model_to_level(hint);
            config.escalation_level = hint_level;
        }
        return hint.to_string();
    }

    match config.model_strategy {
        ModelStrategy::Fixed => select_fixed(config),
        ModelStrategy::CostOptimized => select_cost_optimized(config),
        ModelStrategy::Escalate => select_escalate(config),
        // Plan-then-execute will be implemented in R6.
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

/// Map a model name to an escalation level: haiku=0, sonnet=1, opus=2.
fn model_to_level(model: &str) -> u8 {
    match model {
        "haiku" => 0,
        "sonnet" => 1,
        "opus" => 2,
        _ => 1, // fallback to sonnet-level
    }
}

/// Map an escalation level to a model name.
fn level_to_model(level: u8) -> String {
    match level {
        0 => "haiku".to_string(),
        1 => "sonnet".to_string(),
        _ => "opus".to_string(), // 2+ caps at opus
    }
}

/// Escalate strategy: start at haiku, escalate on failure signals, never
/// auto-de-escalate. Reads the progress file to detect distress signals.
///
/// Escalation is monotonic upward (haiku → sonnet → opus) unless a Claude
/// hint explicitly requests a lower model (handled in `select_model`).
fn select_escalate(config: &mut Config) -> String {
    let content = fs::read_to_string(&config.progress_file).unwrap_or_default();
    let needed_level = assess_escalation_need(&content);

    // Only escalate: take the max of current level and assessed need
    if needed_level > config.escalation_level {
        config.escalation_level = needed_level;
    }

    level_to_model(config.escalation_level)
}

/// Assess what escalation level the progress file content warrants.
///
/// Returns 0 (haiku) when everything looks fine, 1 (sonnet) for moderate
/// complexity signals, 2 (opus) for clear failure/stuck signals.
fn assess_escalation_need(content: &str) -> u8 {
    let lower = content.to_lowercase();

    // Severe distress → opus (level 2)
    let severe_signals = [
        "stuck",
        "cannot",
        "unable",
        "panic",
        "crash",
        "broken",
        "regression",
    ];
    if severe_signals
        .iter()
        .any(|s| lower.contains(&s.to_lowercase()))
    {
        return 2;
    }

    // Moderate distress → sonnet (level 1)
    let moderate_signals = ["error", "failure", "failed", "bug"];
    if moderate_signals
        .iter()
        .any(|s| lower.contains(&s.to_lowercase()))
    {
        return 1;
    }

    // No distress signals detected → no escalation needed
    0
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
        let mut config = fixed_config("opus");
        assert_eq!(select_model(&mut config, None), "opus");
    }

    #[test]
    fn fixed_strategy_returns_haiku_when_configured() {
        let mut config = fixed_config("haiku");
        assert_eq!(select_model(&mut config, None), "haiku");
    }

    #[test]
    fn fixed_strategy_returns_sonnet_when_configured() {
        let mut config = fixed_config("sonnet");
        assert_eq!(select_model(&mut config, None), "sonnet");
    }

    #[test]
    fn fixed_strategy_same_model_across_iterations() {
        let mut config = fixed_config("opus");
        for i in 1..=10 {
            assert_eq!(
                select_model(&mut config, None),
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
        assert_eq!(select_model(&mut cfg, None), "haiku");
    }

    #[test]
    fn hint_overrides_fixed_strategy() {
        let mut config = fixed_config("haiku");
        assert_eq!(select_model(&mut config, Some("opus")), "opus");
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
        let mut config = Config::from_args(args).unwrap();
        assert_eq!(select_model(&mut config, Some("haiku")), "haiku");
    }

    // --- escalate strategy tests ---

    /// Helper to build a Config with escalate strategy.
    fn escalate_config() -> Config {
        let args = Args {
            prompt_file: None,
            once: false,
            no_sandbox: false,
            progress_file: None,
            specs_dir: None,
            limit: None,
            allowed_tools: None,
            allow: vec![],
            model_strategy: Some("escalate".to_string()),
            model: None,
        };
        Config::from_args(args).unwrap()
    }

    #[test]
    fn escalate_starts_at_haiku() {
        let config = escalate_config();
        assert_eq!(config.current_model, "haiku");
        assert_eq!(config.escalation_level, 0);
    }

    #[test]
    fn escalate_no_distress_stays_haiku() {
        assert_eq!(assess_escalation_need(""), 0);
        assert_eq!(assess_escalation_need("[R1] DONE — Added CLI flags"), 0);
    }

    #[test]
    fn escalate_moderate_distress_returns_sonnet_level() {
        assert_eq!(
            assess_escalation_need("[R2] FAILED — Tests have an error"),
            1
        );
        assert_eq!(assess_escalation_need("There is a bug in the parser"), 1);
    }

    #[test]
    fn escalate_severe_distress_returns_opus_level() {
        assert_eq!(
            assess_escalation_need("Stuck on a regression in the build system"),
            2
        );
        assert_eq!(assess_escalation_need("Cannot proceed, broken dependency"), 2);
        assert_eq!(assess_escalation_need("Unable to resolve the panic"), 2);
    }

    #[test]
    fn escalate_never_auto_de_escalates() {
        let mut config = escalate_config();
        // Manually set escalation level to sonnet (1)
        config.escalation_level = 1;

        // With clean progress, assess_escalation_need returns 0,
        // but escalation_level should stay at 1
        let result = assess_escalation_need("");
        assert_eq!(result, 0); // need is 0
        // But select_escalate keeps max(current, needed)
        // We can't call select_escalate directly (reads file), so test the logic:
        let new_level = std::cmp::max(config.escalation_level, result);
        assert_eq!(new_level, 1); // stays at 1, not de-escalated to 0
        assert_eq!(level_to_model(new_level), "sonnet");
    }

    #[test]
    fn escalate_escalation_sequence_haiku_to_sonnet_to_opus() {
        // Level 0 → haiku
        assert_eq!(level_to_model(0), "haiku");
        // Level 1 → sonnet
        assert_eq!(level_to_model(1), "sonnet");
        // Level 2 → opus
        assert_eq!(level_to_model(2), "opus");
    }

    #[test]
    fn escalate_hint_de_escalates_level() {
        let mut config = escalate_config();
        config.escalation_level = 2; // at opus level

        // Hint to haiku should de-escalate
        let model = select_model(&mut config, Some("haiku"));
        assert_eq!(model, "haiku");
        assert_eq!(config.escalation_level, 0); // level reset to haiku
    }

    #[test]
    fn escalate_hint_can_escalate_level() {
        let mut config = escalate_config();
        assert_eq!(config.escalation_level, 0); // starts at haiku

        // Hint to opus should escalate
        let model = select_model(&mut config, Some("opus"));
        assert_eq!(model, "opus");
        assert_eq!(config.escalation_level, 2); // level moved to opus
    }

    #[test]
    fn escalate_model_to_level_mapping() {
        assert_eq!(model_to_level("haiku"), 0);
        assert_eq!(model_to_level("sonnet"), 1);
        assert_eq!(model_to_level("opus"), 2);
        assert_eq!(model_to_level("unknown"), 1); // fallback
    }

    #[test]
    fn escalate_stays_at_escalated_level_across_iterations() {
        let mut config = escalate_config();
        config.escalation_level = 2; // already at opus

        // Even with no distress, level stays at 2
        let needed = assess_escalation_need("[R1] DONE — All good");
        assert_eq!(needed, 0);
        let level = std::cmp::max(config.escalation_level, needed);
        assert_eq!(level, 2);
        assert_eq!(level_to_model(level), "opus");

        // Advance iteration, level persists via clone
        let next_config = config.next_iteration();
        assert_eq!(next_config.escalation_level, 2);
    }
}

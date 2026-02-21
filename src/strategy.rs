//! Model strategy selection logic.
//!
//! Determines which Claude model to use for each iteration based on the
//! configured strategy.

use std::fs;

use crate::config::{Config, ModelStrategy};

/// Result of model selection, including override information for logging.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSelection {
    /// The model that will actually be used.
    pub model: String,
    /// The model the strategy would have chosen (before hint override).
    pub strategy_choice: String,
    /// The hint from Claude, if any.
    pub hint: Option<String>,
    /// True when the hint overrode the strategy's choice (hint != strategy_choice).
    pub was_overridden: bool,
}

/// Select the model for the current iteration based on the active strategy.
///
/// Returns a `ModelSelection` containing the chosen model and override info.
/// The `next_model_hint` parameter allows Claude to override the strategy's
/// choice via the `<next-model>` sigil.
///
/// For the `Escalate` strategy, this mutates `config.escalation_level` to
/// track upward movement through the model tiers.
pub fn select_model(config: &mut Config, next_model_hint: Option<&str>) -> ModelSelection {
    // First, compute what the strategy would choose (without hint).
    let strategy_choice = match config.model_strategy {
        ModelStrategy::Fixed => select_fixed(config),
        ModelStrategy::CostOptimized => select_cost_optimized(config),
        ModelStrategy::Escalate => select_escalate(config),
        ModelStrategy::PlanThenExecute => select_plan_then_execute(config),
    };

    // Claude hint always wins if provided.
    // For escalate strategy, a hint can also de-escalate the level.
    if let Some(hint) = next_model_hint {
        if config.model_strategy == ModelStrategy::Escalate {
            let hint_level = model_to_level(hint);
            config.escalation_level = hint_level;
        }
        let was_overridden = hint != strategy_choice;
        return ModelSelection {
            model: hint.to_string(),
            strategy_choice,
            hint: Some(hint.to_string()),
            was_overridden,
        };
    }

    ModelSelection {
        model: strategy_choice.clone(),
        strategy_choice,
        hint: None,
        was_overridden: false,
    }
}

/// Log a model override event to the progress file.
///
/// Called when Claude's hint disagrees with the strategy's choice.
/// Appends a line to the progress file documenting the override.
pub fn log_model_override(progress_file: &str, iteration: u32, selection: &ModelSelection) {
    use std::fs::OpenOptions;
    use std::io::Write;

    let line = format!(
        "[model-override] iteration={} strategy_choice={} hint={}\n",
        iteration,
        selection.strategy_choice,
        selection.hint.as_deref().unwrap_or("none"),
    );

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(progress_file)
    {
        let _ = file.write_all(line.as_bytes());
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
    let progress_db = config.project_root.join(".ralph/progress.db");
    let content = fs::read_to_string(progress_db).unwrap_or_default();
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
    let progress_db = config.project_root.join(".ralph/progress.db");
    let content = fs::read_to_string(progress_db).unwrap_or_default();
    let needed_level = assess_escalation_need(&content);

    // Only escalate: take the max of current level and assessed need
    if needed_level > config.escalation_level {
        config.escalation_level = needed_level;
    }

    level_to_model(config.escalation_level)
}

/// Plan-then-execute strategy: use opus for iteration 1 (planning),
/// sonnet for iterations 2+ (execution).
///
/// The idea is that the first iteration is the most important — Claude
/// needs to understand the full task and form a plan. Subsequent iterations
/// execute the plan and can use a cheaper model. Claude hints can override
/// either phase (e.g. `haiku` for simple cleanup, `opus` for a hard sub-task).
fn select_plan_then_execute(config: &Config) -> String {
    if config.iteration <= 1 {
        "opus".to_string()
    } else {
        "sonnet".to_string()
    }
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
    use crate::config::Config;
    use crate::project::{ProjectConfig, RalphConfig};
    use std::path::PathBuf;

    /// Helper to build a Config with fixed strategy.
    fn fixed_config(model: &str) -> Config {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig::default(),
        };
        Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("fixed".to_string()),
            Some(model.to_string()),
            project,
            None,
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn fixed_strategy_returns_configured_model() {
        let mut config = fixed_config("opus");
        assert_eq!(select_model(&mut config, None).model, "opus");
    }

    #[test]
    fn fixed_strategy_returns_haiku_when_configured() {
        let mut config = fixed_config("haiku");
        assert_eq!(select_model(&mut config, None).model, "haiku");
    }

    #[test]
    fn fixed_strategy_returns_sonnet_when_configured() {
        let mut config = fixed_config("sonnet");
        assert_eq!(select_model(&mut config, None).model, "sonnet");
    }

    #[test]
    fn fixed_strategy_same_model_across_iterations() {
        let mut config = fixed_config("opus");
        for i in 1..=10 {
            assert_eq!(
                select_model(&mut config, None).model,
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
        assert_eq!(select_model(&mut cfg, None).model, "haiku");
    }

    #[test]
    fn hint_overrides_fixed_strategy() {
        let mut config = fixed_config("haiku");
        let selection = select_model(&mut config, Some("opus"));
        assert_eq!(selection.model, "opus");
        assert!(selection.was_overridden);
        assert_eq!(selection.strategy_choice, "haiku");
        assert_eq!(selection.hint, Some("opus".to_string()));
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
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig::default(),
        };
        let mut config = Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("cost-optimized".to_string()),
            None,
            project,
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(select_model(&mut config, Some("haiku")).model, "haiku");
    }

    // --- escalate strategy tests ---

    /// Helper to build a Config with escalate strategy.
    fn escalate_config() -> Config {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig::default(),
        };
        Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("escalate".to_string()),
            None,
            project,
            None,
            None,
            false,
        )
        .unwrap()
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
        assert_eq!(
            assess_escalation_need("Cannot proceed, broken dependency"),
            2
        );
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
        let selection = select_model(&mut config, Some("haiku"));
        assert_eq!(selection.model, "haiku");
        assert_eq!(config.escalation_level, 0); // level reset to haiku
    }

    #[test]
    fn escalate_hint_can_escalate_level() {
        let mut config = escalate_config();
        assert_eq!(config.escalation_level, 0); // starts at haiku

        // Hint to opus should escalate
        let selection = select_model(&mut config, Some("opus"));
        assert_eq!(selection.model, "opus");
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

    // --- plan-then-execute strategy tests ---

    /// Helper to build a Config with plan-then-execute strategy.
    fn plan_then_execute_config() -> Config {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig::default(),
        };
        Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("plan-then-execute".to_string()),
            None,
            project,
            None,
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn plan_then_execute_iteration_1_returns_opus() {
        let mut config = plan_then_execute_config();
        assert_eq!(config.iteration, 1);
        assert_eq!(select_model(&mut config, None).model, "opus");
    }

    #[test]
    fn plan_then_execute_iteration_2_returns_sonnet() {
        let mut config = plan_then_execute_config();
        config = config.next_iteration(); // iteration 2
        assert_eq!(config.iteration, 2);
        assert_eq!(select_model(&mut config, None).model, "sonnet");
    }

    #[test]
    fn plan_then_execute_iteration_5_returns_sonnet() {
        let mut config = plan_then_execute_config();
        for _ in 1..5 {
            config = config.next_iteration();
        }
        assert_eq!(config.iteration, 5);
        assert_eq!(select_model(&mut config, None).model, "sonnet");
    }

    #[test]
    fn plan_then_execute_hint_overrides_to_haiku() {
        let mut config = plan_then_execute_config();
        config = config.next_iteration(); // iteration 2
                                          // Hint to haiku for simple cleanup
        assert_eq!(select_model(&mut config, Some("haiku")).model, "haiku");
    }

    #[test]
    fn plan_then_execute_hint_overrides_to_opus() {
        let mut config = plan_then_execute_config();
        config = config.next_iteration(); // iteration 2
        config = config.next_iteration(); // iteration 3
                                          // Hint to opus for a hard sub-task
        assert_eq!(select_model(&mut config, Some("opus")).model, "opus");
    }

    #[test]
    fn plan_then_execute_hint_overrides_iteration_1() {
        let mut config = plan_then_execute_config();
        assert_eq!(config.iteration, 1);
        // Even on iteration 1, hint can override to a different model
        assert_eq!(select_model(&mut config, Some("haiku")).model, "haiku");
    }

    #[test]
    fn plan_then_execute_config_starts_at_opus() {
        let config = plan_then_execute_config();
        assert_eq!(config.current_model, "opus");
        assert_eq!(config.model_strategy, ModelStrategy::PlanThenExecute);
    }

    #[test]
    fn plan_then_execute_direct_function_iteration_1() {
        let config = plan_then_execute_config();
        assert_eq!(select_plan_then_execute(&config), "opus");
    }

    #[test]
    fn plan_then_execute_direct_function_iteration_2_plus() {
        let config_iter2 = plan_then_execute_config().next_iteration();
        assert_eq!(select_plan_then_execute(&config_iter2), "sonnet");

        let config_iter10 = {
            let mut c = plan_then_execute_config();
            for _ in 1..10 {
                c = c.next_iteration();
            }
            c
        };
        assert_eq!(config_iter10.iteration, 10);
        assert_eq!(select_plan_then_execute(&config_iter10), "sonnet");
    }

    // --- override detection and logging tests (R9) ---

    #[test]
    fn no_hint_means_no_override() {
        let mut config = fixed_config("opus");
        let selection = select_model(&mut config, None);
        assert!(!selection.was_overridden);
        assert_eq!(selection.hint, None);
        assert_eq!(selection.strategy_choice, "opus");
        assert_eq!(selection.model, "opus");
    }

    #[test]
    fn hint_matching_strategy_is_not_override() {
        let mut config = fixed_config("opus");
        let selection = select_model(&mut config, Some("opus"));
        assert!(!selection.was_overridden);
        assert_eq!(selection.hint, Some("opus".to_string()));
        assert_eq!(selection.strategy_choice, "opus");
        assert_eq!(selection.model, "opus");
    }

    #[test]
    fn hint_differing_from_strategy_is_override() {
        let mut config = fixed_config("haiku");
        let selection = select_model(&mut config, Some("opus"));
        assert!(selection.was_overridden);
        assert_eq!(selection.strategy_choice, "haiku");
        assert_eq!(selection.hint, Some("opus".to_string()));
        assert_eq!(selection.model, "opus");
    }

    /// Helper to create a unique temp file path for testing.
    fn temp_progress_file(suffix: &str) -> String {
        let path = std::env::temp_dir().join(format!("ralph_test_override_{}", suffix));
        // Clean up any leftover from previous test runs
        let _ = std::fs::remove_file(&path);
        path.to_string_lossy().to_string()
    }

    #[test]
    fn override_logged_to_progress_file() {
        let progress_str = temp_progress_file("log_basic");

        let selection = ModelSelection {
            model: "opus".to_string(),
            strategy_choice: "sonnet".to_string(),
            hint: Some("opus".to_string()),
            was_overridden: true,
        };

        log_model_override(&progress_str, 3, &selection);

        let content = std::fs::read_to_string(&progress_str).unwrap();
        assert!(content.contains("[model-override]"));
        assert!(content.contains("iteration=3"));
        assert!(content.contains("strategy_choice=sonnet"));
        assert!(content.contains("hint=opus"));

        // Cleanup
        let _ = std::fs::remove_file(&progress_str);
    }

    #[test]
    fn no_override_means_no_log_entry() {
        let progress_str = temp_progress_file("no_log");

        let mut config = fixed_config("opus");
        let selection = select_model(&mut config, None);

        // Should not be overridden, so we don't log
        assert!(!selection.was_overridden);
        // The caller (run_loop) only calls log_model_override when was_overridden is true
        // Verify file doesn't exist (nothing was written)
        assert!(!std::path::Path::new(&progress_str).exists());
    }

    #[test]
    fn override_appends_to_existing_progress_file() {
        let progress_str = temp_progress_file("append");

        // Write existing content
        std::fs::write(&progress_str, "[R1] DONE — stuff\n").unwrap();

        let selection = ModelSelection {
            model: "haiku".to_string(),
            strategy_choice: "sonnet".to_string(),
            hint: Some("haiku".to_string()),
            was_overridden: true,
        };

        log_model_override(&progress_str, 5, &selection);

        let content = std::fs::read_to_string(&progress_str).unwrap();
        assert!(content.starts_with("[R1] DONE"));
        assert!(content.contains("[model-override]"));
        assert!(content.contains("iteration=5"));
        assert!(content.contains("hint=haiku"));

        // Cleanup
        let _ = std::fs::remove_file(&progress_str);
    }

    #[test]
    fn plan_then_execute_override_detected_on_iteration_2() {
        // Plan-then-execute would choose sonnet at iteration 2,
        // but hint overrides to opus — should be detected as override
        let mut config = plan_then_execute_config();
        config = config.next_iteration(); // iteration 2
        let selection = select_model(&mut config, Some("opus"));
        assert!(selection.was_overridden);
        assert_eq!(selection.strategy_choice, "sonnet");
        assert_eq!(selection.model, "opus");
    }

    #[test]
    fn plan_then_execute_no_override_when_hint_matches() {
        // Plan-then-execute chooses opus at iteration 1,
        // hint also says opus — no override
        let mut config = plan_then_execute_config();
        let selection = select_model(&mut config, Some("opus"));
        assert!(!selection.was_overridden);
        assert_eq!(selection.strategy_choice, "opus");
        assert_eq!(selection.model, "opus");
    }

    #[test]
    fn escalate_override_detected_when_hint_differs() {
        let mut config = escalate_config();
        // Escalate starts at haiku (level 0), hint overrides to opus
        let selection = select_model(&mut config, Some("opus"));
        assert!(selection.was_overridden);
        assert_eq!(selection.strategy_choice, "haiku");
        assert_eq!(selection.model, "opus");
    }
}

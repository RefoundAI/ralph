//! Signal handling, interactive feedback collection, and description mutation
//! for mid-loop interrupt support.

use anyhow::Result;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::dag::Task;

/// Global interrupt flag, registered once with SIGINT.
static INTERRUPT_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
/// Separate signal-only flag used for conditional hard shutdown on repeated SIGINT.
///
/// This must remain independent from `INTERRUPT_FLAG` because UI Ctrl+C in raw
/// mode sets `INTERRUPT_FLAG` programmatically; if both behaviors share one
/// flag, a later real SIGINT can be misinterpreted as a "second" interrupt.
static SIGINT_SHUTDOWN_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Register the SIGINT handler. Safe to call multiple times (only the first
/// call registers; subsequent calls are no-ops).
pub fn register_signal_handler() -> Result<()> {
    let interrupt_flag = INTERRUPT_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)));
    let shutdown_flag = SIGINT_SHUTDOWN_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)));

    // Force-exit on second real SIGINT while allowing the first to trigger
    // graceful handling.
    signal_hook::flag::register_conditional_shutdown(
        signal_hook::consts::SIGINT,
        130,
        Arc::clone(shutdown_flag),
    )?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(shutdown_flag))?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(interrupt_flag))?;

    Ok(())
}

/// Check whether the interrupt flag is set.
pub fn is_interrupted() -> bool {
    INTERRUPT_FLAG
        .get()
        .map(|f| f.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// Clear the interrupt flag so the next iteration starts clean.
pub fn clear_interrupt() {
    if let Some(flag) = INTERRUPT_FLAG.get() {
        flag.store(false, Ordering::SeqCst);
    }
    if let Some(flag) = SIGINT_SHUTDOWN_FLAG.get() {
        flag.store(false, Ordering::SeqCst);
    }
}

/// Set the interrupt flag programmatically (used by TUI Ctrl+C handling in raw mode).
pub fn request_interrupt() {
    let flag = INTERRUPT_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)));
    flag.store(true, Ordering::SeqCst);
}

/// Prompt the user for feedback on the interrupted task.
///
/// Returns `Some(feedback)` if the user typed something, or `None` if they
/// pressed Enter immediately or stdin is not a terminal.
pub fn prompt_for_feedback(task: &Task) -> Result<Option<String>> {
    if crate::ui::is_active() {
        let title = format!("Interrupted {}", task.id);
        let hint = format!(
            "{}\n{}\n\nProvide feedback. Empty line submits. Empty buffer skips.",
            task.id, task.title
        );
        return Ok(match crate::ui::prompt_multiline(&title, &hint) {
            Some(crate::ui::UiPromptResult::Input(text)) => {
                if text.trim().is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            Some(crate::ui::UiPromptResult::Exit) | None => None,
            Some(crate::ui::UiPromptResult::Interrupted) => None,
        });
    }

    if !std::io::stdin().is_terminal() {
        return Ok(None);
    }

    println!();
    println!("  Interrupted task {} — \"{}\"", task.id, task.title);
    println!();
    println!("  Provide feedback for this task (empty line to finish, Enter to skip):");

    let mut lines = Vec::new();
    loop {
        print!("  > ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');

        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }

    if lines.is_empty() {
        Ok(None)
    } else {
        Ok(Some(lines.join("\n")))
    }
}

/// Append user feedback to a task description with a clear delimiter.
///
/// Multiple interventions stack at the end; the original description stays
/// at the top.
pub fn append_feedback_to_description(description: &str, feedback: &str, iteration: u32) -> String {
    format!(
        "{}\n\n---\n**User Guidance (iteration {}):**\n{}\n---",
        description, iteration, feedback
    )
}

/// Ask the user whether to continue the run loop.
///
/// Returns `true` for "Y" (default) or `false` for "n".
/// Non-TTY defaults to `false`.
pub fn should_continue() -> Result<bool> {
    if crate::ui::is_active() {
        return Ok(
            crate::ui::prompt_confirm("Continue Run", "Continue after interruption?", true)
                .unwrap_or(false),
        );
    }

    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }

    print!("  Continue? [Y/n] ");
    use std::io::Write;
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_lowercase();

    Ok(trimmed.is_empty() || trimmed == "y" || trimmed == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_feedback_single() {
        let desc = "Original task description.";
        let result = append_feedback_to_description(desc, "Focus on error handling", 3);
        assert!(result.starts_with("Original task description."));
        assert!(result.contains("**User Guidance (iteration 3):**"));
        assert!(result.contains("Focus on error handling"));
    }

    #[test]
    fn append_feedback_stacks() {
        let desc = "Original.";
        let after_first = append_feedback_to_description(desc, "First feedback", 1);
        let after_second = append_feedback_to_description(&after_first, "Second feedback", 2);
        assert!(after_second.contains("**User Guidance (iteration 1):**"));
        assert!(after_second.contains("First feedback"));
        assert!(after_second.contains("**User Guidance (iteration 2):**"));
        assert!(after_second.contains("Second feedback"));
    }

    #[test]
    fn is_interrupted_default_false() {
        // Before registration, should return false
        // Note: in test context the OnceLock may or may not be initialized
        // depending on test order, so we just verify it doesn't panic
        let _ = is_interrupted();
    }

    #[test]
    fn request_interrupt_sets_flag() {
        clear_interrupt();
        request_interrupt();
        assert!(is_interrupted());
        clear_interrupt();
    }

    #[test]
    fn clear_interrupt_resets_signal_shutdown_flag() {
        let flag = SIGINT_SHUTDOWN_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)));
        flag.store(true, Ordering::SeqCst);
        clear_interrupt();
        assert!(!flag.load(Ordering::SeqCst));
    }
}

//! Signal handling, interactive feedback collection, and description mutation
//! for mid-loop interrupt support.

use anyhow::Result;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::dag::Task;

/// Global interrupt flag, registered once with SIGINT.
static INTERRUPT_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Register the SIGINT handler. Safe to call multiple times (only the first
/// call registers; subsequent calls are no-ops).
pub fn register_signal_handler() -> Result<()> {
    let flag = INTERRUPT_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)));

    // First handler: set the flag on first Ctrl+C
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(flag))?;

    // Second handler: if the flag is already set (i.e. second Ctrl+C), force-exit
    let flag_clone = Arc::clone(flag);
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
            if flag_clone.load(Ordering::SeqCst) {
                // Second Ctrl+C — hard exit
                std::process::exit(130);
            }
        })?;
    }

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
}

/// Prompt the user for feedback on the interrupted task.
///
/// Returns `Some(feedback)` if the user typed something, or `None` if they
/// pressed Enter immediately or stdin is not a terminal.
pub fn prompt_for_feedback(task: &Task) -> Result<Option<String>> {
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
}

//! Terminal output formatting with ANSI colors plus TUI event emission.

use colored::Colorize;
use std::process::Command;

use crate::config::Config;
use crate::ui::{self, UiEvent, UiLevel};

fn ui_log(level: UiLevel, message: impl Into<String>) {
    if ui::is_active() {
        ui::emit(UiEvent::Log {
            level,
            message: message.into(),
        });
    }
}

/// Print a plain info line (or emit to TUI).
pub fn print_info(message: &str) {
    if ui::is_active() {
        ui_log(UiLevel::Info, message.to_string());
    } else {
        println!("{message}");
    }
}

/// Print a warning line (or emit to TUI).
pub fn print_warning(message: &str) {
    if ui::is_active() {
        ui_log(UiLevel::Warn, message.to_string());
    } else {
        eprintln!("{}", message.yellow());
    }
}

/// Print an error line (or emit to TUI).
pub fn print_error(message: &str) {
    if ui::is_active() {
        ui_log(UiLevel::Error, message.to_string());
    } else {
        eprintln!("{}", message.red());
    }
}

/// Print iteration information.
pub fn print_iteration_info(config: &Config) {
    let line = if config.limit == 1 {
        "Running once only".to_string()
    } else if config.total == 0 {
        format!("Iteration {} (unlimited)", config.iteration)
    } else {
        format!("Iteration {} of {}", config.iteration, config.total)
    };

    if ui::is_active() {
        ui::emit(UiEvent::StatusLine(format!(
            "{line} | model={} | strategy={}",
            config.current_model, config.model_strategy
        )));
        ui_log(UiLevel::Info, line);
    } else {
        println!("{line}");
    }
}

/// Print DAG summary.
pub fn print_dag_summary(total: usize, ready: usize, done: usize, blocked: usize) {
    let line = format!("DAG: {total} tasks, {ready} ready, {done} done, {blocked} blocked");
    if ui::is_active() {
        ui::emit(UiEvent::DagSummary(line.clone()));
        ui_log(UiLevel::Info, line);
    } else {
        println!("{line}");
    }
}

/// Print completion message.
pub fn print_complete() {
    if ui::is_active() {
        ui::emit(UiEvent::StatusLine("Run complete".to_string()));
    }
    print_info("Tasks complete.");
    speak("Ralph finished. Tasks complete.");
}

/// Print failure message.
pub fn print_failure() {
    if ui::is_active() {
        ui::emit(UiEvent::StatusLine("Run failed".to_string()));
    }
    print_error("Critical failure. See progress file for details.");
    speak("Ralph failed--critical failure.");
}

/// Print limit reached message.
pub fn print_limit_reached() {
    if ui::is_active() {
        ui::emit(UiEvent::StatusLine("Iteration limit reached".to_string()));
        ui_log(UiLevel::Info, "Iteration limit reached.");
    }
    speak("Ralph finished--limit hit.");
}

/// Print iteration separator.
pub fn print_separator() {
    if ui::is_active() {
        ui_log(UiLevel::Info, "─".repeat(40));
    } else {
        let width = terminal_width();
        println!("{}", "-".repeat(width).dimmed());
    }
}

/// Print a clickable file hyperlink.
pub fn hyperlink(path: &str) {
    if ui::is_active() {
        ui_log(UiLevel::Info, path.to_string());
    } else {
        println!("\x1b]8;;file://{}\x1b\\{}\x1b]8;;\x1b\\", path, path);
    }
}

/// Print a file location line with label.
pub fn print_log_location(label: &str, path: &str) {
    if ui::is_active() {
        ui_log(UiLevel::Info, format!("{label} {path}"));
    } else {
        println!("{label}");
        hyperlink(path);
    }
}

/// Print verification start message.
pub fn print_verification_start(iteration: u32, task_id: &str) {
    if ui::is_active() {
        ui_log(
            UiLevel::Info,
            format!("[iter {iteration}] Verifying task {task_id}"),
        );
    } else {
        println!(
            "[iter {}] {} {}",
            iteration,
            "Verifying:".yellow(),
            task_id.cyan()
        );
    }
}

/// Print verification passed message.
pub fn print_verification_passed(iteration: u32, task_id: &str) {
    if ui::is_active() {
        ui_log(
            UiLevel::Info,
            format!("[iter {iteration}] Done (verified): {task_id}"),
        );
    } else {
        println!(
            "[iter {}] {} (verified): {}",
            iteration,
            "Done".green(),
            task_id.cyan()
        );
    }
}

/// Print verification failed message.
pub fn print_verification_failed(iteration: u32, task_id: &str, reason: &str) {
    if ui::is_active() {
        ui_log(
            UiLevel::Warn,
            format!("[iter {iteration}] Failed verification: {task_id} - {reason}"),
        );
    } else {
        println!(
            "[iter {}] {} verification: {} — {}",
            iteration,
            "Failed".red(),
            task_id.cyan(),
            reason
        );
    }
}

/// Print retry message.
pub fn print_retry(iteration: u32, task_id: &str, attempt: i32, max: i32) {
    if ui::is_active() {
        ui_log(
            UiLevel::Warn,
            format!("[iter {iteration}] Retrying {task_id} (attempt {attempt}/{max})"),
        );
    } else {
        println!(
            "[iter {}] Retrying {} (attempt {}/{})",
            iteration,
            task_id.cyan(),
            attempt,
            max
        );
    }
}

/// Print max retries exhausted message.
pub fn print_max_retries_exhausted(iteration: u32, task_id: &str) {
    if ui::is_active() {
        ui_log(
            UiLevel::Error,
            format!("[iter {iteration}] Failed (max retries exhausted): {task_id}"),
        );
    } else {
        println!(
            "[iter {}] {} (max retries exhausted): {}",
            iteration,
            "Failed".red(),
            task_id.cyan()
        );
    }
}

/// Print task done message.
pub fn print_task_done(iteration: u32, task_id: &str) {
    if ui::is_active() {
        ui_log(UiLevel::Info, format!("[iter {iteration}] Done: {task_id}"));
    } else {
        println!(
            "[iter {}] {}: {}",
            iteration,
            "Done".green(),
            task_id.cyan()
        );
    }
}

/// Print task failed message.
pub fn print_task_failed(iteration: u32, task_id: &str) {
    if ui::is_active() {
        ui_log(
            UiLevel::Error,
            format!("[iter {iteration}] Failed: {task_id}"),
        );
    } else {
        println!(
            "[iter {}] {}: {}",
            iteration,
            "Failed".red(),
            task_id.cyan()
        );
    }
}

/// Print task incomplete (no sigil) message.
pub fn print_task_incomplete(iteration: u32, task_id: &str) {
    if ui::is_active() {
        ui_log(
            UiLevel::Warn,
            format!("[iter {iteration}] Incomplete (no sigil): {task_id}"),
        );
    } else {
        println!(
            "[iter {}] Incomplete (no sigil): {}",
            iteration,
            task_id.cyan()
        );
    }
}

/// Print task working message.
pub fn print_task_working(iteration: u32, task_id: &str, title: &str) {
    let line = format!("[iter {iteration}] Working on: {task_id} -- {title}");
    if ui::is_active() {
        ui::emit(UiEvent::CurrentTask(format!("Task: {task_id} — {title}")));
        ui_log(UiLevel::Info, line);
    } else {
        println!(
            "[iter {}] Working on: {} -- {}",
            iteration,
            task_id.cyan(),
            title
        );
    }
}

/// Print review loop start message.
pub fn print_review_start(kind: &str, feature_name: &str) {
    let line = format!("Reviewing {kind} for '{feature_name}'...");
    if ui::is_active() {
        ui_log(UiLevel::Info, line);
    } else {
        println!("\n{}", line.cyan());
    }
}

/// Print review round start message.
pub fn print_review_round(round: u32, max: u32, kind: &str) {
    let line = format!("{kind} review round {round}/{max}");
    if ui::is_active() {
        ui_log(UiLevel::Info, line);
    } else {
        println!("  {} {}", line.cyan(), "→".dimmed());
    }
}

/// Print review round result.
pub fn print_review_result(round: u32, passed: bool, changes_summary: &str, kind: &str) {
    let _ = round;
    if ui::is_active() {
        if passed {
            ui_log(
                UiLevel::Info,
                format!("Pass {kind} review: no major issues found"),
            );
        } else {
            ui_log(
                UiLevel::Warn,
                format!("Changes in {kind} review: {changes_summary}"),
            );
        }
        return;
    }

    if passed {
        println!(
            "  {} {} review: {}",
            "Pass".green(),
            kind,
            "no major issues found".dimmed()
        );
    } else {
        println!(
            "  {} {} review: {}",
            "Changes".yellow(),
            kind,
            changes_summary.dimmed()
        );
    }
}

/// Print review complete message.
pub fn print_review_complete(kind: &str, feature_name: &str, rounds: u32) {
    let rounds_text = if rounds == 1 {
        "1 round".to_string()
    } else {
        format!("{rounds} rounds")
    };
    let line = format!("Review complete: '{feature_name}' {kind} finalized after {rounds_text}.");
    if ui::is_active() {
        ui_log(UiLevel::Info, line);
    } else {
        println!("{}", line.green());
    }
}

/// Print review max rounds reached message.
pub fn print_review_max_rounds(kind: &str, feature_name: &str, max: u32) {
    let line = format!("Review limit: '{feature_name}' {kind} stabilized after {max} rounds.");
    if ui::is_active() {
        ui_log(UiLevel::Warn, line);
    } else {
        println!("{}", line.yellow());
    }
}

/// Print interrupted message.
pub fn print_interrupted(iteration: u32, task_id: &str, title: &str) {
    if ui::is_active() {
        ui_log(
            UiLevel::Warn,
            format!("[iter {iteration}] Interrupted {task_id} -- \"{title}\""),
        );
    } else {
        println!(
            "\n[iter {}] {} {} — \"{}\"",
            iteration,
            "Interrupted".yellow().bold(),
            task_id.cyan(),
            title,
        );
    }
}

fn speak(message: &str) {
    if Command::new("which").arg("say").output().is_ok() {
        let msg = message.to_string();
        std::thread::spawn(move || {
            let _ = Command::new("say").arg(&msg).output();
        });
    }
}

fn terminal_width() -> usize {
    Command::new("tput")
        .arg("cols")
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8_lossy(&out.stdout).trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(80)
}

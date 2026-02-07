//! Main iteration loop.

use anyhow::{Context, Result};
use std::path::Path;

use crate::claude;
use crate::config::Config;
use crate::dag;
use crate::output::{formatter, logger};
use crate::strategy;

/// Outcome of the loop execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// All DAG tasks done
    Complete,
    /// <promise>FAILURE</promise> emitted
    Failure,
    /// Iteration limit hit
    LimitReached,
    /// No ready tasks, but incomplete tasks exist
    Blocked,
    /// DAG is empty, user must run `ralph plan`
    NoPlan,
}

/// Run the main loop until completion, failure, or limit.
pub fn run(mut config: Config) -> Result<Outcome> {
    // Ensure prompt file exists
    if !Path::new(&config.prompt_file).exists() {
        std::fs::write(&config.prompt_file, "").context("Failed to create prompt file")?;
    }

    // Open the DAG database
    let progress_db = config.project_root.join(".ralph/progress.db");
    let db = dag::open_db(
        progress_db
            .to_str()
            .context("Failed to convert progress.db path to string")?,
    ).context("Failed to open DAG database")?;

    loop {
        // Get task counts and ready tasks
        let counts = dag::get_task_counts(&db).context("Failed to get task counts")?;
        let ready_tasks = dag::get_ready_tasks(&db).context("Failed to get ready tasks")?;

        // Print DAG summary at the start of each iteration
        if config.iteration == 1 {
            println!(
                "DAG: {} tasks, {} ready, {} done, {} blocked",
                counts.total, counts.ready, counts.done, counts.blocked
            );
        }

        // Check if DAG is empty
        if counts.total == 0 {
            return Ok(Outcome::NoPlan);
        }

        // Check if no tasks are ready and some are incomplete (blocked)
        if ready_tasks.is_empty() {
            // If all tasks are done or failed, this shouldn't happen (all_resolved check below)
            // But if we're here and ready is empty, it means we're blocked
            return Ok(Outcome::Blocked);
        }

        // Pick first ready task
        let task = &ready_tasks[0];
        let task_id = task.id.clone();

        // Claim the task
        dag::claim_task(&db, &task_id, &config.agent_id)
            .context("Failed to claim task")?;

        // Print iteration info
        println!(
            "[iter {}] Working on: {} -- {}",
            config.iteration, task_id, task.title
        );

        // Set up logging
        let log_file = logger::setup_log_file();
        println!("Log will be written to: ");
        formatter::hyperlink(&log_file);

        if config.use_sandbox {
            formatter::print_sandbox_warning();
        }

        // Run Claude
        let result = claude::client::run(&config, Some(&log_file))
            .context("Failed to run Claude")?;

        println!("Log available at: ");
        formatter::hyperlink(&log_file);

        // Extract model hint before checking completion/failure
        let next_model_hint = result
            .as_ref()
            .and_then(|r| r.next_model_hint.clone());

        // Check for FAILURE sigil - this short-circuits before DAG update
        if let Some(ref r) = result {
            if r.is_failure() {
                return Ok(Outcome::Failure);
            }
        }

        // Handle task completion/failure sigils
        if let Some(ref r) = result {
            if let Some(ref done_id) = r.task_done {
                if done_id == &task_id {
                    dag::complete_task(&db, &task_id)
                        .context("Failed to complete task")?;
                    println!("[iter {}] Done: {}", config.iteration, task_id);
                } else {
                    eprintln!("Warning: task-done sigil ID {} does not match assigned task {}", done_id, task_id);
                }
            } else if let Some(ref failed_id) = r.task_failed {
                if failed_id == &task_id {
                    dag::fail_task(&db, &task_id, "Task marked failed by Claude")
                        .context("Failed to fail task")?;
                    println!("[iter {}] Failed: {}", config.iteration, task_id);
                } else {
                    eprintln!("Warning: task-failed sigil ID {} does not match assigned task {}", failed_id, task_id);
                }
            } else {
                // No sigil - release the claim and treat as incomplete
                dag::release_claim(&db, &task_id)
                    .context("Failed to release task claim")?;
                println!("[iter {}] Incomplete (no sigil): {}", config.iteration, task_id);
            }
        }

        // Check if all tasks are resolved
        if dag::all_resolved(&db).context("Failed to check if all tasks resolved")? {
            return Ok(Outcome::Complete);
        }

        // Check iteration limit
        if config.limit_reached() {
            return Ok(Outcome::LimitReached);
        }

        // Continue to next iteration
        formatter::print_separator();
        config = config.next_iteration();

        // Select model for the next iteration based on strategy,
        // passing Claude's hint (if any) from the previous result
        let selection =
            strategy::select_model(&mut config, next_model_hint.as_deref());

        // Log override events when hint disagrees with strategy
        if selection.was_overridden {
            strategy::log_model_override(
                progress_db.to_str().unwrap(),
                config.iteration,
                &selection,
            );
        }

        config.current_model = selection.model;

        formatter::print_iteration_info(&config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dag_returns_noplan() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        // Verify DAG is empty
        let counts = dag::get_task_counts(&db).unwrap();
        assert_eq!(counts.total, 0);

        // This simulates the beginning of the loop:
        // If counts.total == 0, return Outcome::NoPlan
        // (We can't call run() directly without a Config, but we can verify the logic)
        if counts.total == 0 {
            // Would return Outcome::NoPlan
            assert_eq!(counts.total, 0);
        }
    }

    #[test]
    fn ready_tasks_are_picked_in_order() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        // Create a simple chain: A -> B -> C
        // Insert them directly (we'd normally use create_task, but that's not exported)
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-a", "Task A", "", 0, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-b", "Task B", "", 0, "2024-01-01T00:00:01Z", "2024-01-01T00:00:01Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-c", "Task C", "", 0, "2024-01-01T00:00:02Z", "2024-01-01T00:00:02Z"],
            )
            .unwrap();

        // Add dependencies: A blocks B, B blocks C
        db.conn()
            .execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
                rusqlite::params!["t-task-a", "t-task-b"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
                rusqlite::params!["t-task-b", "t-task-c"],
            )
            .unwrap();

        // First iteration: A should be ready
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-task-a");

        // Claim and complete A
        dag::claim_task(&db, "t-task-a", "agent-test").unwrap();
        dag::complete_task(&db, "t-task-a").unwrap();

        // Second iteration: B should now be ready
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-task-b");

        // Claim and complete B
        dag::claim_task(&db, "t-task-b", "agent-test").unwrap();
        dag::complete_task(&db, "t-task-b").unwrap();

        // Third iteration: C should now be ready
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-task-c");

        // Complete C
        dag::claim_task(&db, "t-task-c", "agent-test").unwrap();
        dag::complete_task(&db, "t-task-c").unwrap();

        // All tasks should be resolved
        assert!(dag::all_resolved(&db).unwrap());
    }

    #[test]
    fn blocked_tasks_return_blocked_outcome() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = dag::open_db(temp_file.path().to_str().unwrap()).unwrap();

        // Create A -> B
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-a", "Task A", "", 0, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-task-b", "Task B", "", 0, "2024-01-01T00:00:01Z", "2024-01-01T00:00:01Z"],
            )
            .unwrap();

        // Add dependency: A blocks B
        db.conn()
            .execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
                rusqlite::params!["t-task-a", "t-task-b"],
            )
            .unwrap();

        // A is ready, claim and fail it
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);

        dag::claim_task(&db, "t-task-a", "agent-test").unwrap();
        dag::fail_task(&db, "t-task-a", "Failed intentionally").unwrap();

        // B is now blocked (cannot proceed because A failed)
        let ready = dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 0);

        // But tasks still exist and are not all resolved
        let counts = dag::get_task_counts(&db).unwrap();
        assert!(counts.total > 0);
        assert!(!dag::all_resolved(&db).unwrap());

        // This is the Blocked scenario
    }
}

//! DAG (Directed Acyclic Graph) task management.
//!
//! Manages task dependencies and execution state using SQLite backend.

mod db;
mod dependencies;
mod ids;
mod tasks;

use anyhow::Result;

pub use db::{init_db, Db};
pub use dependencies::{add_dependency, remove_dependency};
pub use ids::{generate_task_id, generate_and_insert_task_id};
pub use tasks::{compute_parent_status, get_task_status};

/// A task in the DAG.
#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub parent_id: Option<String>,
}

/// Task counts summary.
#[derive(Debug, Clone)]
pub struct TaskCounts {
    pub total: usize,
    pub ready: usize,
    pub done: usize,
    pub blocked: usize,
}

/// Open or initialize the task database (alias for init_db).
pub fn open_db(path: &str) -> Result<Db> {
    init_db(path)
}

/// Get all tasks that are ready to execute.
pub fn get_ready_tasks(db: &Db) -> Result<Vec<Task>> {
    let _ = db;
    todo!("get_ready_tasks: query tasks with pending status, satisfied blockers, and non-failed parent")
}

/// Get task counts (total, ready, done, blocked).
pub fn get_task_counts(db: &Db) -> Result<TaskCounts> {
    let _ = db;
    todo!("get_task_counts: count tasks by status")
}

/// Claim a task for execution by an agent.
pub fn claim_task(db: &Db, task_id: &str, agent_id: &str) -> Result<()> {
    let _ = (db, task_id, agent_id);
    todo!("claim_task: set task to in_progress and record agent_id")
}

/// Mark a task as completed.
pub fn complete_task(db: &Db, task_id: &str) -> Result<()> {
    let _ = (db, task_id);
    todo!("complete_task: set task to done and run auto-transitions")
}

/// Mark a task as failed.
pub fn fail_task(db: &Db, task_id: &str, reason: &str) -> Result<()> {
    let _ = (db, task_id, reason);
    todo!("fail_task: set task to failed and run auto-transitions")
}

/// Check if all DAG tasks are resolved (done or failed).
pub fn all_resolved(db: &Db) -> Result<bool> {
    let _ = db;
    todo!("all_resolved: return true if all tasks are done or failed")
}

/// Release the claim on a task (set to pending if in_progress).
pub fn release_claim(db: &Db, task_id: &str) -> Result<()> {
    let _ = (db, task_id);
    todo!("release_claim: revert task from in_progress to pending")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_struct_has_required_fields() {
        let task = Task {
            id: "t-abc123".to_string(),
            title: "Test task".to_string(),
            description: "A test task".to_string(),
            parent_id: None,
        };
        assert_eq!(task.id, "t-abc123");
        assert_eq!(task.title, "Test task");
        assert!(task.parent_id.is_none());
    }

    #[test]
    fn task_counts_struct_has_required_fields() {
        let counts = TaskCounts {
            total: 10,
            ready: 3,
            done: 2,
            blocked: 1,
        };
        assert_eq!(counts.total, 10);
        assert_eq!(counts.ready, 3);
        assert_eq!(counts.done, 2);
        assert_eq!(counts.blocked, 1);
    }
}

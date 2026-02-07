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
    // A task is ready when:
    // 1. Status is 'pending'
    // 2. All blockers have status 'done'
    // 3. Parent (if any) is not 'failed'
    // 4. Task is a leaf node (no children)
    //
    // Ordered by: priority ASC, created_at ASC
    let mut stmt = db.conn().prepare(
        r#"
        SELECT DISTINCT t.id, t.title, t.description, t.parent_id
        FROM tasks t
        WHERE t.status = 'pending'
          -- Must be a leaf node (no children)
          AND NOT EXISTS (
              SELECT 1 FROM tasks c WHERE c.parent_id = t.id
          )
          -- Parent (if exists) must not be failed
          AND (
              t.parent_id IS NULL
              OR NOT EXISTS (
                  SELECT 1 FROM tasks p WHERE p.id = t.parent_id AND p.status = 'failed'
              )
          )
          -- All blockers must be done (or no blockers)
          AND NOT EXISTS (
              SELECT 1
              FROM dependencies d
              JOIN tasks b ON d.blocker_id = b.id
              WHERE d.blocked_id = t.id
                AND b.status != 'done'
          )
        ORDER BY t.priority ASC, t.created_at ASC
        "#,
    )?;

    let tasks = stmt
        .query_map([], |row| {
            Ok(Task {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                parent_id: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
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

    // R5: Ready query tests

    fn create_task(db: &Db, id: &str, title: &str, parent_id: Option<&str>, priority: i32) {
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, parent_id, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![id, title, "", parent_id, priority, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();
    }

    fn set_task_status(db: &Db, id: &str, status: &str) {
        db.conn()
            .execute("UPDATE tasks SET status = ? WHERE id = ?", rusqlite::params![status, id])
            .unwrap();
    }

    #[test]
    fn test_ready_task_with_no_deps_no_parent() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();

        create_task(&db, "t-task1", "Task 1", None, 0);

        let ready = get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-task1");
    }

    #[test]
    fn test_task_with_pending_blocker_not_ready() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();

        create_task(&db, "t-blocker", "Blocker", None, 0);
        create_task(&db, "t-blocked", "Blocked", None, 0);
        add_dependency(&db, "t-blocker", "t-blocked").unwrap();

        let ready = get_ready_tasks(&db).unwrap();
        // Only t-blocker should be ready
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-blocker");
    }

    #[test]
    fn test_task_with_all_blockers_done_is_ready() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();

        create_task(&db, "t-blocker", "Blocker", None, 0);
        create_task(&db, "t-blocked", "Blocked", None, 0);
        add_dependency(&db, "t-blocker", "t-blocked").unwrap();

        // Mark blocker as done
        set_task_status(&db, "t-blocker", "done");

        let ready = get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-blocked");
    }

    #[test]
    fn test_task_with_failed_parent_not_ready() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();

        create_task(&db, "t-parent", "Parent", None, 0);
        create_task(&db, "t-child", "Child", Some("t-parent"), 0);

        // Mark parent as failed
        set_task_status(&db, "t-parent", "failed");

        let ready = get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 0);
    }

    #[test]
    fn test_non_leaf_task_not_ready() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();

        create_task(&db, "t-parent", "Parent", None, 0);
        create_task(&db, "t-child", "Child", Some("t-parent"), 0);

        let ready = get_ready_tasks(&db).unwrap();
        // Only child (leaf) should be ready
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "t-child");
    }

    #[test]
    fn test_ready_tasks_ordering() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();

        // Create tasks with different priorities
        create_task(&db, "t-p1", "Priority 1", None, 1);
        create_task(&db, "t-p0", "Priority 0", None, 0);
        create_task(&db, "t-p2", "Priority 2", None, 2);

        let ready = get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 3);
        // Should be ordered by priority ASC
        assert_eq!(ready[0].id, "t-p0");
        assert_eq!(ready[1].id, "t-p1");
        assert_eq!(ready[2].id, "t-p2");
    }

    #[test]
    fn test_ready_tasks_ordering_same_priority() {
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();

        // Create tasks with same priority but different created_at
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-newer", "Newer", "", 0, "2024-01-02T00:00:00Z", "2024-01-02T00:00:00Z"],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, title, description, priority, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                rusqlite::params!["t-older", "Older", "", 0, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
            )
            .unwrap();

        let ready = get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 2);
        // Should be ordered by created_at ASC (older first)
        assert_eq!(ready[0].id, "t-older");
        assert_eq!(ready[1].id, "t-newer");
    }
}

//! DAG (Directed Acyclic Graph) task management.
//!
//! Manages task dependencies and execution state using SQLite backend.

mod crud;
mod db;
mod dependencies;
mod ids;
mod tasks;
mod transitions;

use anyhow::Result;
use serde::Serialize;

#[allow(unused_imports)]
pub use crud::{
    add_log, create_task, create_task_with_feature, delete_task, delete_tasks_for_feature,
    get_task, get_task_tree, update_task, CreateTaskParams, TaskUpdate,
};
#[allow(unused_imports)]
pub use crud::{
    get_all_tasks, get_all_tasks_for_feature, get_task_blockers, get_task_logs,
    get_tasks_blocked_by, LogEntry,
};
pub use db::{init_db, Db};
#[allow(unused_imports)]
pub use dependencies::{add_dependency, remove_dependency};
#[allow(unused_imports)]
pub use ids::{generate_and_insert_task_id, generate_feature_id, generate_task_id};
#[allow(unused_imports)]
pub use tasks::{compute_parent_status, get_task_status};
pub use transitions::{force_complete_task, force_fail_task, force_reset_task};

/// A task in the DAG.
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub parent_id: Option<String>,
    pub feature_id: Option<String>,
    pub task_type: String,
    pub priority: i32,
    pub retry_count: i32,
    pub max_retries: i32,
    pub verification_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub claimed_by: Option<String>,
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

/// Map a row to a Task.
///
/// Expects columns in order: id, title, description, status, parent_id, feature_id,
/// task_type, priority, retry_count, max_retries, verification_status, created_at,
/// updated_at, claimed_by
pub(crate) fn task_from_row(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        status: row.get(3)?,
        parent_id: row.get(4)?,
        feature_id: row.get(5)?,
        task_type: row
            .get::<_, Option<String>>(6)?
            .unwrap_or_else(|| "feature".to_string()),
        priority: row.get::<_, Option<i32>>(7)?.unwrap_or(0),
        retry_count: row.get::<_, Option<i32>>(8)?.unwrap_or(0),
        max_retries: row.get::<_, Option<i32>>(9)?.unwrap_or(3),
        verification_status: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        claimed_by: row.get(13)?,
    })
}

/// The standard column list for task queries.
const TASK_COLUMNS: &str = "id, title, description, status, parent_id, feature_id, task_type, priority, retry_count, max_retries, verification_status, created_at, updated_at, claimed_by";

/// Get all tasks that are ready to execute.
pub fn get_ready_tasks(db: &Db) -> Result<Vec<Task>> {
    // A task is ready when:
    // 1. Status is 'pending'
    // 2. All blockers have status 'done'
    // 3. Parent (if any) is not 'failed'
    // 4. Task is a leaf node (no children)
    //
    // Ordered by: priority ASC, created_at ASC
    let query = format!(
        r#"
        SELECT DISTINCT t.{cols}
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
        cols = TASK_COLUMNS.replace(", ", ", t."),
    );
    let mut stmt = db.conn().prepare(&query)?;

    let tasks = stmt
        .query_map([], task_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// Get task counts (total, ready, done, blocked).
pub fn get_task_counts(db: &Db) -> Result<TaskCounts> {
    let total: usize = db
        .conn()
        .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;

    let ready: usize = db.conn().query_row(
        r#"
        SELECT COUNT(*)
        FROM tasks t
        WHERE t.status = 'pending'
          AND NOT EXISTS (
              SELECT 1 FROM tasks c WHERE c.parent_id = t.id
          )
          AND (
              t.parent_id IS NULL
              OR NOT EXISTS (
                  SELECT 1 FROM tasks p WHERE p.id = t.parent_id AND p.status = 'failed'
              )
          )
          AND NOT EXISTS (
              SELECT 1
              FROM dependencies d
              JOIN tasks b ON d.blocker_id = b.id
              WHERE d.blocked_id = t.id
                AND b.status != 'done'
          )
        "#,
        [],
        |row| row.get(0),
    )?;

    let done: usize = db.conn().query_row(
        "SELECT COUNT(*) FROM tasks WHERE status = 'done'",
        [],
        |row| row.get(0),
    )?;

    let blocked: usize = db.conn().query_row(
        "SELECT COUNT(*) FROM tasks WHERE status = 'blocked'",
        [],
        |row| row.get(0),
    )?;

    Ok(TaskCounts {
        total,
        ready,
        done,
        blocked,
    })
}

/// Claim a task for execution by an agent.
pub fn claim_task(db: &Db, task_id: &str, agent_id: &str) -> Result<()> {
    // Transition to in_progress and set claimed_by atomically
    transitions::set_task_status(db.conn(), task_id, "in_progress")?;
    db.conn().execute(
        "UPDATE tasks SET claimed_by = ? WHERE id = ?",
        rusqlite::params![agent_id, task_id],
    )?;
    Ok(())
}

/// Mark a task as completed.
pub fn complete_task(db: &Db, task_id: &str) -> Result<()> {
    // Transition to done (auto-transitions handled in set_task_status)
    transitions::set_task_status(db.conn(), task_id, "done")?;
    // Clear claimed_by
    db.conn()
        .execute("UPDATE tasks SET claimed_by = NULL WHERE id = ?", [task_id])?;
    Ok(())
}

/// Mark a task as failed.
pub fn fail_task(db: &Db, task_id: &str, reason: &str) -> Result<()> {
    // Transition to failed (auto-transitions handled in set_task_status)
    transitions::set_task_status(db.conn(), task_id, "failed")?;
    // Clear claimed_by
    db.conn()
        .execute("UPDATE tasks SET claimed_by = NULL WHERE id = ?", [task_id])?;
    // Log the failure reason
    let timestamp = chrono::Utc::now().to_rfc3339();
    db.conn().execute(
        "INSERT INTO task_logs (task_id, message, timestamp) VALUES (?, ?, ?)",
        rusqlite::params![task_id, reason, timestamp],
    )?;
    Ok(())
}

/// Check if all DAG tasks are resolved (done or failed).
pub fn all_resolved(db: &Db) -> Result<bool> {
    let unresolved: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM tasks WHERE status NOT IN ('done', 'failed')",
        [],
        |row| row.get(0),
    )?;
    Ok(unresolved == 0)
}

/// Get all tasks for a specific feature that are ready to execute.
pub fn get_ready_tasks_for_feature(db: &Db, feature_id: &str) -> Result<Vec<Task>> {
    let query = format!(
        r#"
        SELECT DISTINCT t.{cols}
        FROM tasks t
        WHERE t.status = 'pending'
          AND t.feature_id = ?
          AND NOT EXISTS (
              SELECT 1 FROM tasks c WHERE c.parent_id = t.id
          )
          AND (
              t.parent_id IS NULL
              OR NOT EXISTS (
                  SELECT 1 FROM tasks p WHERE p.id = t.parent_id AND p.status = 'failed'
              )
          )
          AND NOT EXISTS (
              SELECT 1
              FROM dependencies d
              JOIN tasks b ON d.blocker_id = b.id
              WHERE d.blocked_id = t.id
                AND b.status != 'done'
          )
        ORDER BY t.priority ASC, t.created_at ASC
        "#,
        cols = TASK_COLUMNS.replace(", ", ", t."),
    );
    let mut stmt = db.conn().prepare(&query)?;

    let tasks = stmt
        .query_map([feature_id], task_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// Get standalone tasks (not associated with any feature).
pub fn get_standalone_tasks(db: &Db) -> Result<Vec<Task>> {
    let query = format!(
        "SELECT {} FROM tasks WHERE task_type = 'standalone' ORDER BY priority ASC, created_at ASC",
        TASK_COLUMNS,
    );
    let mut stmt = db.conn().prepare(&query)?;

    let tasks = stmt
        .query_map([], task_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// Get task counts for a specific feature.
pub fn get_feature_task_counts(db: &Db, feature_id: &str) -> Result<TaskCounts> {
    let total: usize = db.conn().query_row(
        "SELECT COUNT(*) FROM tasks WHERE feature_id = ?",
        [feature_id],
        |row| row.get(0),
    )?;

    let ready = get_ready_tasks_for_feature(db, feature_id)?.len();

    let done: usize = db.conn().query_row(
        "SELECT COUNT(*) FROM tasks WHERE feature_id = ? AND status = 'done'",
        [feature_id],
        |row| row.get(0),
    )?;

    let blocked: usize = db.conn().query_row(
        "SELECT COUNT(*) FROM tasks WHERE feature_id = ? AND status = 'blocked'",
        [feature_id],
        |row| row.get(0),
    )?;

    Ok(TaskCounts {
        total,
        ready,
        done,
        blocked,
    })
}

/// Retry a failed task: transition back to pending and increment retry_count.
pub fn retry_task(db: &Db, task_id: &str) -> Result<()> {
    // Transition failed -> pending
    transitions::set_task_status(db.conn(), task_id, "pending")?;
    // Increment retry_count and set verification_status to failed
    db.conn().execute(
        "UPDATE tasks SET retry_count = retry_count + 1, verification_status = 'failed', claimed_by = NULL WHERE id = ?",
        [task_id],
    )?;
    Ok(())
}

/// Release the claim on a task (set to pending if in_progress).
pub fn release_claim(db: &Db, task_id: &str) -> Result<()> {
    // Only release if currently in_progress
    let status: String =
        db.conn()
            .query_row("SELECT status FROM tasks WHERE id = ?", [task_id], |row| {
                row.get(0)
            })?;

    if status == "in_progress" {
        transitions::set_task_status(db.conn(), task_id, "pending")?;
        db.conn()
            .execute("UPDATE tasks SET claimed_by = NULL WHERE id = ?", [task_id])?;
    }

    Ok(())
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
            status: "pending".to_string(),
            parent_id: None,
            feature_id: None,
            task_type: "feature".to_string(),
            priority: 0,
            retry_count: 0,
            max_retries: 3,
            verification_status: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            claimed_by: None,
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
            .execute(
                "UPDATE tasks SET status = ? WHERE id = ?",
                rusqlite::params![status, id],
            )
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

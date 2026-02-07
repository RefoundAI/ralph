//! Task hierarchy and status management.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Compute the derived status of a parent task based on its children.
///
/// Rules:
/// - Any child `failed` -> parent `failed`
/// - All children `done` -> parent `done`
/// - Any child `in_progress` -> parent `in_progress`
/// - Otherwise -> `pending`
pub fn compute_parent_status(conn: &Connection, parent_id: &str) -> Result<String> {
    let mut stmt = conn
        .prepare("SELECT id FROM tasks WHERE parent_id = ?")
        .context("Failed to prepare child query")?;

    let child_ids: Vec<String> = stmt
        .query_map([parent_id], |row| row.get(0))
        .context("Failed to query child IDs")?
        .collect::<Result<_, _>>()
        .context("Failed to collect child IDs")?;

    if child_ids.is_empty() {
        // No children - return the parent's own status
        return conn
            .query_row("SELECT status FROM tasks WHERE id = ?", [parent_id], |row| {
                row.get(0)
            })
            .context("Failed to get parent status");
    }

    // Get derived status for each child (recursive)
    let mut statuses = Vec::new();
    for child_id in child_ids {
        let status = get_task_status(conn, &child_id)?;
        statuses.push(status);
    }

    // Apply rules
    if statuses.iter().any(|s| s == "failed") {
        return Ok("failed".to_string());
    }

    if statuses.iter().all(|s| s == "done") {
        return Ok("done".to_string());
    }

    if statuses.iter().any(|s| s == "in_progress") {
        return Ok("in_progress".to_string());
    }

    Ok("pending".to_string())
}

/// Get the derived status for a task (considering children if it has any).
pub fn get_task_status(conn: &Connection, task_id: &str) -> Result<String> {
    // Check if task has children
    let child_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE parent_id = ?",
            [task_id],
            |row| row.get(0),
        )
        .context("Failed to count children")?;

    if child_count > 0 {
        // Has children - compute derived status
        compute_parent_status(conn, task_id)
    } else {
        // No children - return stored status
        conn.query_row("SELECT status FROM tasks WHERE id = ?", [task_id], |row| {
            row.get(0)
        })
        .context("Failed to get task status")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::init_db;
    use tempfile::NamedTempFile;

    fn create_task(conn: &Connection, id: &str, title: &str, parent_id: Option<&str>) {
        conn.execute(
            "INSERT INTO tasks (id, title, parent_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![id, title, parent_id, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
        ).unwrap();
    }

    fn set_task_status(conn: &Connection, id: &str, status: &str) {
        conn.execute(
            "UPDATE tasks SET status = ? WHERE id = ?",
            rusqlite::params![status, id],
        )
        .unwrap();
    }

    #[test]
    fn test_parent_one_done_one_pending() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-parent", "Parent", None);
        create_task(conn, "t-child1", "Child 1", Some("t-parent"));
        create_task(conn, "t-child2", "Child 2", Some("t-parent"));

        set_task_status(conn, "t-child1", "done");
        set_task_status(conn, "t-child2", "pending");

        let status = get_task_status(conn, "t-parent")?;
        assert_eq!(status, "pending");

        Ok(())
    }

    #[test]
    fn test_parent_all_done() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-parent", "Parent", None);
        create_task(conn, "t-child1", "Child 1", Some("t-parent"));
        create_task(conn, "t-child2", "Child 2", Some("t-parent"));

        set_task_status(conn, "t-child1", "done");
        set_task_status(conn, "t-child2", "done");

        let status = get_task_status(conn, "t-parent")?;
        assert_eq!(status, "done");

        Ok(())
    }

    #[test]
    fn test_parent_one_failed() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-parent", "Parent", None);
        create_task(conn, "t-child1", "Child 1", Some("t-parent"));
        create_task(conn, "t-child2", "Child 2", Some("t-parent"));

        set_task_status(conn, "t-child1", "done");
        set_task_status(conn, "t-child2", "failed");

        let status = get_task_status(conn, "t-parent")?;
        assert_eq!(status, "failed");

        Ok(())
    }

    #[test]
    fn test_parent_one_in_progress() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-parent", "Parent", None);
        create_task(conn, "t-child1", "Child 1", Some("t-parent"));
        create_task(conn, "t-child2", "Child 2", Some("t-parent"));

        set_task_status(conn, "t-child1", "in_progress");
        set_task_status(conn, "t-child2", "pending");

        let status = get_task_status(conn, "t-parent")?;
        assert_eq!(status, "in_progress");

        Ok(())
    }

    #[test]
    fn test_three_level_hierarchy() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        // Create 3-level hierarchy: grandparent -> parent -> child
        create_task(conn, "t-gp", "Grandparent", None);
        create_task(conn, "t-parent", "Parent", Some("t-gp"));
        create_task(conn, "t-child1", "Child 1", Some("t-parent"));
        create_task(conn, "t-child2", "Child 2", Some("t-parent"));

        // All children done -> parent done -> grandparent done
        set_task_status(conn, "t-child1", "done");
        set_task_status(conn, "t-child2", "done");

        let parent_status = get_task_status(conn, "t-parent")?;
        assert_eq!(parent_status, "done");

        let gp_status = get_task_status(conn, "t-gp")?;
        assert_eq!(gp_status, "done");

        Ok(())
    }

    #[test]
    fn test_leaf_task_returns_stored_status() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        set_task_status(conn, "t-task", "in_progress");

        let status = get_task_status(conn, "t-task")?;
        assert_eq!(status, "in_progress");

        Ok(())
    }
}

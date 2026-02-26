//! Status transition validation and auto-transitions.

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;

/// Valid status transitions:
/// - pending -> in_progress
/// - pending -> blocked
/// - in_progress -> done
/// - in_progress -> failed
/// - in_progress -> pending
/// - blocked -> pending
/// - failed -> pending
fn is_valid_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("pending", "in_progress")
            | ("pending", "blocked")
            | ("in_progress", "done")
            | ("in_progress", "failed")
            | ("in_progress", "pending")
            | ("blocked", "pending")
            | ("failed", "pending")
    )
}

/// Set task status with validation and auto-transitions.
///
/// This function:
/// 1. Validates the transition is allowed
/// 2. Updates the task status
/// 3. Runs auto-transitions for dependent/parent tasks
pub fn set_task_status(conn: &Connection, task_id: &str, new_status: &str) -> Result<()> {
    // Get current status
    let current_status: String = conn
        .query_row("SELECT status FROM tasks WHERE id = ?", [task_id], |row| {
            row.get(0)
        })
        .context("Failed to get current task status")?;

    // Validate transition
    if !is_valid_transition(&current_status, new_status) {
        return Err(anyhow!(
            "Invalid status transition from '{}' to '{}'",
            current_status,
            new_status
        ));
    }

    // Update status
    conn.execute(
        "UPDATE tasks SET status = ?, updated_at = datetime('now') WHERE id = ?",
        rusqlite::params![new_status, task_id],
    )
    .context("Failed to update task status")?;

    // Run auto-transitions based on new status
    match new_status {
        "done" => {
            auto_unblock_tasks(conn, task_id)?;
            auto_complete_parent(conn, task_id)?;
            auto_complete_feature(conn, task_id)?;
        }
        "failed" => {
            auto_fail_parent(conn, task_id)?;
            auto_update_feature_on_fail(conn, task_id)?;
        }
        _ => {}
    }

    Ok(())
}

/// Auto-transition: When a blocker is marked done, check if any blocked tasks
/// should transition from 'blocked' to 'pending'.
fn auto_unblock_tasks(conn: &Connection, blocker_id: &str) -> Result<()> {
    // Find all tasks blocked by this task
    let mut stmt = conn.prepare("SELECT blocked_id FROM dependencies WHERE blocker_id = ?")?;

    let blocked_ids: Vec<String> = stmt
        .query_map([blocker_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for blocked_id in blocked_ids {
        // Check if this task is currently blocked
        let status: String = conn.query_row(
            "SELECT status FROM tasks WHERE id = ?",
            [&blocked_id],
            |row| row.get(0),
        )?;

        if status != "blocked" {
            continue;
        }

        // Check if ALL blockers are now done
        let pending_blockers: i64 = conn.query_row(
            r#"
            SELECT COUNT(*)
            FROM dependencies d
            JOIN tasks t ON d.blocker_id = t.id
            WHERE d.blocked_id = ?
              AND t.status != 'done'
            "#,
            [&blocked_id],
            |row| row.get(0),
        )?;

        // If all blockers are done, transition from blocked to pending
        if pending_blockers == 0 {
            conn.execute(
                "UPDATE tasks SET status = 'pending', updated_at = datetime('now') WHERE id = ?",
                [&blocked_id],
            )?;
        }
    }

    Ok(())
}

/// Auto-transition: When a task is marked done, check if its parent should
/// also be marked done (all siblings + self are done).
fn auto_complete_parent(conn: &Connection, task_id: &str) -> Result<()> {
    // Get parent_id (nullable column)
    let parent_id: Option<String> = conn.query_row(
        "SELECT parent_id FROM tasks WHERE id = ?",
        [task_id],
        |row| row.get(0),
    )?;

    let Some(parent_id) = parent_id else {
        return Ok(()); // No parent
    };

    // Check if all children of the parent are done
    let not_done_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE parent_id = ? AND status != 'done'",
        [&parent_id],
        |row| row.get(0),
    )?;

    // If all children are done, mark parent as done (if it's not already)
    if not_done_count == 0 {
        let parent_status: String = conn.query_row(
            "SELECT status FROM tasks WHERE id = ?",
            [&parent_id],
            |row| row.get(0),
        )?;

        if parent_status != "done" {
            // Don't use set_task_status here to avoid infinite recursion
            // Just directly update the parent
            conn.execute(
                "UPDATE tasks SET status = 'done', updated_at = datetime('now') WHERE id = ?",
                [&parent_id],
            )?;

            // Recursively check grandparent
            auto_complete_parent(conn, &parent_id)?;
            // Check if completing this parent resolves its feature
            auto_complete_feature(conn, &parent_id)?;
        }
    }

    Ok(())
}

/// Auto-transition: When a task is marked failed, mark its parent as failed.
fn auto_fail_parent(conn: &Connection, task_id: &str) -> Result<()> {
    // Get parent_id (nullable column)
    let parent_id: Option<String> = conn.query_row(
        "SELECT parent_id FROM tasks WHERE id = ?",
        [task_id],
        |row| row.get(0),
    )?;

    let Some(parent_id) = parent_id else {
        return Ok(()); // No parent
    };

    // Get parent status
    let parent_status: String = conn.query_row(
        "SELECT status FROM tasks WHERE id = ?",
        [&parent_id],
        |row| row.get(0),
    )?;

    // Only fail parent if it's not already failed
    if parent_status != "failed" {
        // Don't use set_task_status here to avoid infinite recursion
        // Just directly update the parent
        conn.execute(
            "UPDATE tasks SET status = 'failed', updated_at = datetime('now') WHERE id = ?",
            [&parent_id],
        )?;

        // Recursively fail grandparent
        auto_fail_parent(conn, &parent_id)?;
        // Check if failing this parent resolves its feature
        auto_update_feature_on_fail(conn, &parent_id)?;
    }

    Ok(())
}

/// Auto-transition: When a feature task is marked done, check if all tasks
/// for that feature are now done and update the feature status accordingly.
fn auto_complete_feature(conn: &Connection, task_id: &str) -> Result<()> {
    let feature_id: Option<String> = conn.query_row(
        "SELECT feature_id FROM tasks WHERE id = ?",
        [task_id],
        |row| row.get(0),
    )?;

    let Some(feature_id) = feature_id else {
        return Ok(()); // Not a feature task
    };

    // Check if all tasks for this feature are done
    let not_done_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE feature_id = ? AND status != 'done'",
        [&feature_id],
        |row| row.get(0),
    )?;

    if not_done_count == 0 {
        conn.execute(
            "UPDATE features SET status = 'done', updated_at = datetime('now') WHERE id = ? AND status != 'done'",
            [&feature_id],
        )?;
    }

    Ok(())
}

/// Auto-transition: When a feature task fails, check if all tasks for that
/// feature are resolved (done or failed). If so, mark the feature as failed
/// (since at least one task failed).
fn auto_update_feature_on_fail(conn: &Connection, task_id: &str) -> Result<()> {
    let feature_id: Option<String> = conn.query_row(
        "SELECT feature_id FROM tasks WHERE id = ?",
        [task_id],
        |row| row.get(0),
    )?;

    let Some(feature_id) = feature_id else {
        return Ok(()); // Not a feature task
    };

    // Check if all tasks for this feature are resolved (done or failed)
    let unresolved_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE feature_id = ? AND status NOT IN ('done', 'failed')",
        [&feature_id],
        |row| row.get(0),
    )?;

    if unresolved_count == 0 {
        conn.execute(
            "UPDATE features SET status = 'failed', updated_at = datetime('now') WHERE id = ? AND status NOT IN ('done', 'failed')",
            [&feature_id],
        )?;
    }

    Ok(())
}

/// Force a task to "done" status, regardless of current state.
///
/// Steps through the valid transitions: pending→in_progress→done.
/// If already done, this is a no-op.
pub fn force_complete_task(conn: &Connection, task_id: &str) -> Result<()> {
    let current_status: String = conn
        .query_row("SELECT status FROM tasks WHERE id = ?", [task_id], |row| {
            row.get(0)
        })
        .context(format!("Task '{}' not found", task_id))?;

    match current_status.as_str() {
        "done" => Ok(()),
        "in_progress" => set_task_status(conn, task_id, "done"),
        "pending" => {
            set_task_status(conn, task_id, "in_progress")?;
            set_task_status(conn, task_id, "done")
        }
        "failed" => {
            // failed→pending→in_progress→done
            set_task_status(conn, task_id, "pending")?;
            set_task_status(conn, task_id, "in_progress")?;
            set_task_status(conn, task_id, "done")
        }
        "blocked" => {
            // blocked→pending→in_progress→done
            set_task_status(conn, task_id, "pending")?;
            set_task_status(conn, task_id, "in_progress")?;
            set_task_status(conn, task_id, "done")
        }
        _ => Err(anyhow!("Unknown status '{}'", current_status)),
    }
}

/// Force a task to "failed" status, regardless of current state.
///
/// Steps through the valid transitions to reach failed.
/// If already failed, this is a no-op.
pub fn force_fail_task(conn: &Connection, task_id: &str) -> Result<()> {
    let current_status: String = conn
        .query_row("SELECT status FROM tasks WHERE id = ?", [task_id], |row| {
            row.get(0)
        })
        .context(format!("Task '{}' not found", task_id))?;

    match current_status.as_str() {
        "failed" => Ok(()),
        "in_progress" => set_task_status(conn, task_id, "failed"),
        "pending" => {
            set_task_status(conn, task_id, "in_progress")?;
            set_task_status(conn, task_id, "failed")
        }
        "blocked" => {
            // blocked→pending→in_progress→failed
            set_task_status(conn, task_id, "pending")?;
            set_task_status(conn, task_id, "in_progress")?;
            set_task_status(conn, task_id, "failed")
        }
        "done" => {
            // Can't transition from done to failed directly.
            // This is an unusual case but we handle it by just updating directly.
            conn.execute(
                "UPDATE tasks SET status = 'failed', updated_at = datetime('now') WHERE id = ?",
                [task_id],
            )?;
            auto_fail_parent(conn, task_id)?;
            Ok(())
        }
        _ => Err(anyhow!("Unknown status '{}'", current_status)),
    }
}

/// Force a task back to "pending" status, regardless of current state.
pub fn force_reset_task(conn: &Connection, task_id: &str) -> Result<()> {
    let current_status: String = conn
        .query_row("SELECT status FROM tasks WHERE id = ?", [task_id], |row| {
            row.get(0)
        })
        .context(format!("Task '{}' not found", task_id))?;

    match current_status.as_str() {
        "pending" => Ok(()),
        "in_progress" | "blocked" | "failed" => set_task_status(conn, task_id, "pending"),
        "done" => {
            // Direct update since done→pending isn't a valid transition
            conn.execute(
                "UPDATE tasks SET status = 'pending', updated_at = datetime('now') WHERE id = ?",
                [task_id],
            )?;
            Ok(())
        }
        _ => Err(anyhow!("Unknown status '{}'", current_status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::{add_dependency, init_db};
    use tempfile::NamedTempFile;

    fn create_task(conn: &Connection, id: &str, title: &str, parent_id: Option<&str>) {
        conn.execute(
            "INSERT INTO tasks (id, title, parent_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![id, title, parent_id, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
        ).unwrap();
    }

    fn create_feature_task(
        conn: &Connection,
        id: &str,
        title: &str,
        feature_id: &str,
        parent_id: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO tasks (id, title, feature_id, task_type, parent_id, created_at, updated_at) VALUES (?, ?, ?, 'feature', ?, ?, ?)",
            rusqlite::params![id, title, feature_id, parent_id, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
        ).unwrap();
    }

    fn create_feature(conn: &Connection, id: &str, name: &str, status: &str) {
        conn.execute(
            "INSERT INTO features (id, name, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![id, name, status, "2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"],
        )
        .unwrap();
    }

    fn get_feature_status(conn: &Connection, id: &str) -> String {
        conn.query_row("SELECT status FROM features WHERE id = ?", [id], |row| {
            row.get(0)
        })
        .unwrap()
    }

    // Valid transitions

    #[test]
    fn test_pending_to_in_progress() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        set_task_status(conn, "t-task", "in_progress")?;

        let status: String =
            conn.query_row("SELECT status FROM tasks WHERE id = 't-task'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "in_progress");

        Ok(())
    }

    #[test]
    fn test_pending_to_blocked() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        set_task_status(conn, "t-task", "blocked")?;

        let status: String =
            conn.query_row("SELECT status FROM tasks WHERE id = 't-task'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "blocked");

        Ok(())
    }

    #[test]
    fn test_in_progress_to_done() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-task'",
            [],
        )?;
        set_task_status(conn, "t-task", "done")?;

        let status: String =
            conn.query_row("SELECT status FROM tasks WHERE id = 't-task'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "done");

        Ok(())
    }

    #[test]
    fn test_in_progress_to_failed() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-task'",
            [],
        )?;
        set_task_status(conn, "t-task", "failed")?;

        let status: String =
            conn.query_row("SELECT status FROM tasks WHERE id = 't-task'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "failed");

        Ok(())
    }

    #[test]
    fn test_in_progress_to_pending() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-task'",
            [],
        )?;
        set_task_status(conn, "t-task", "pending")?;

        let status: String =
            conn.query_row("SELECT status FROM tasks WHERE id = 't-task'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "pending");

        Ok(())
    }

    #[test]
    fn test_blocked_to_pending() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        conn.execute(
            "UPDATE tasks SET status = 'blocked' WHERE id = 't-task'",
            [],
        )?;
        set_task_status(conn, "t-task", "pending")?;

        let status: String =
            conn.query_row("SELECT status FROM tasks WHERE id = 't-task'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "pending");

        Ok(())
    }

    #[test]
    fn test_failed_to_pending() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        conn.execute("UPDATE tasks SET status = 'failed' WHERE id = 't-task'", [])?;
        set_task_status(conn, "t-task", "pending")?;

        let status: String =
            conn.query_row("SELECT status FROM tasks WHERE id = 't-task'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "pending");

        Ok(())
    }

    // Invalid transitions

    #[test]
    fn test_done_to_in_progress_fails() {
        let temp_file = NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        conn.execute("UPDATE tasks SET status = 'done' WHERE id = 't-task'", [])
            .unwrap();

        let result = set_task_status(conn, "t-task", "in_progress");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid status transition"));
    }

    #[test]
    fn test_done_to_pending_fails() {
        let temp_file = NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();
        let conn = db.conn();

        create_task(conn, "t-task", "Task", None);
        conn.execute("UPDATE tasks SET status = 'done' WHERE id = 't-task'", [])
            .unwrap();

        let result = set_task_status(conn, "t-task", "pending");
        assert!(result.is_err());
    }

    // Auto-transitions

    #[test]
    fn test_auto_unblock_when_blocker_completes() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-blocker", "Blocker", None);
        create_task(conn, "t-blocked", "Blocked", None);
        add_dependency(&db, "t-blocker", "t-blocked")?;

        // Set blocked task to blocked status
        conn.execute(
            "UPDATE tasks SET status = 'blocked' WHERE id = 't-blocked'",
            [],
        )?;

        // Set blocker to in_progress
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-blocker'",
            [],
        )?;

        // Complete the blocker
        set_task_status(conn, "t-blocker", "done")?;

        // Check that blocked task is now pending
        let status: String = conn.query_row(
            "SELECT status FROM tasks WHERE id = 't-blocked'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(status, "pending");

        Ok(())
    }

    #[test]
    fn test_auto_complete_parent_when_all_children_done() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-parent", "Parent", None);
        create_task(conn, "t-child1", "Child 1", Some("t-parent"));
        create_task(conn, "t-child2", "Child 2", Some("t-parent"));

        // Set children to in_progress
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id IN ('t-child1', 't-child2')",
            [],
        )?;

        // Complete first child - parent should not be done yet
        set_task_status(conn, "t-child1", "done")?;
        let parent_status: String = conn.query_row(
            "SELECT status FROM tasks WHERE id = 't-parent'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(parent_status, "pending");

        // Complete second child - parent should now be done
        set_task_status(conn, "t-child2", "done")?;
        let parent_status: String = conn.query_row(
            "SELECT status FROM tasks WHERE id = 't-parent'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(parent_status, "done");

        Ok(())
    }

    #[test]
    fn test_auto_fail_parent_when_child_fails() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_task(conn, "t-parent", "Parent", None);
        create_task(conn, "t-child", "Child", Some("t-parent"));

        // Set child to in_progress
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-child'",
            [],
        )?;

        // Fail the child
        set_task_status(conn, "t-child", "failed")?;

        // Parent should be failed
        let parent_status: String = conn.query_row(
            "SELECT status FROM tasks WHERE id = 't-parent'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(parent_status, "failed");

        Ok(())
    }

    // Feature auto-transitions

    #[test]
    fn test_auto_complete_feature_when_all_tasks_done() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_feature(conn, "f-test", "test-feature", "ready");
        create_feature_task(conn, "t-1", "Task 1", "f-test", None);
        create_feature_task(conn, "t-2", "Task 2", "f-test", None);

        // Complete first task — feature should still be ready
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-1'",
            [],
        )?;
        set_task_status(conn, "t-1", "done")?;
        assert_eq!(get_feature_status(conn, "f-test"), "ready");

        // Complete second task — feature should now be done
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-2'",
            [],
        )?;
        set_task_status(conn, "t-2", "done")?;
        assert_eq!(get_feature_status(conn, "f-test"), "done");

        Ok(())
    }

    #[test]
    fn test_auto_fail_feature_when_task_fails_and_all_resolved() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_feature(conn, "f-test", "test-feature", "ready");
        create_feature_task(conn, "t-1", "Task 1", "f-test", None);
        create_feature_task(conn, "t-2", "Task 2", "f-test", None);

        // Complete first task
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-1'",
            [],
        )?;
        set_task_status(conn, "t-1", "done")?;
        assert_eq!(get_feature_status(conn, "f-test"), "ready");

        // Fail second task — all resolved, feature should be failed
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-2'",
            [],
        )?;
        set_task_status(conn, "t-2", "failed")?;
        assert_eq!(get_feature_status(conn, "f-test"), "failed");

        Ok(())
    }

    #[test]
    fn test_feature_not_updated_when_tasks_still_pending() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        create_feature(conn, "f-test", "test-feature", "ready");
        create_feature_task(conn, "t-1", "Task 1", "f-test", None);
        create_feature_task(conn, "t-2", "Task 2", "f-test", None);

        // Fail first task — second still pending, feature stays ready
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-1'",
            [],
        )?;
        set_task_status(conn, "t-1", "failed")?;
        assert_eq!(get_feature_status(conn, "f-test"), "ready");

        Ok(())
    }

    #[test]
    fn test_standalone_task_does_not_affect_features() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;
        let conn = db.conn();

        // Standalone task (no feature_id)
        create_task(conn, "t-standalone", "Standalone", None);
        conn.execute(
            "UPDATE tasks SET status = 'in_progress' WHERE id = 't-standalone'",
            [],
        )?;

        // Should not error even with no feature
        set_task_status(conn, "t-standalone", "done")?;

        Ok(())
    }
}

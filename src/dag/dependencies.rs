//! Dependency management with cycle detection.

#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use rusqlite::params;
use std::collections::{HashSet, VecDeque};

use crate::dag::Db;

/// Add a dependency between two tasks.
///
/// A dependency `(blocker_id, blocked_id)` means `blocked_id` cannot start
/// until `blocker_id` is done.
///
/// Rejects the dependency if it would create a cycle.
pub fn add_dependency(db: &Db, blocker_id: &str, blocked_id: &str) -> Result<()> {
    // Check if adding this edge would create a cycle
    if would_create_cycle(db, blocker_id, blocked_id)? {
        bail!(
            "Adding dependency {} -> {} would create a cycle",
            blocker_id,
            blocked_id
        );
    }

    // Insert the dependency
    db.conn()
        .execute(
            "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
            params![blocker_id, blocked_id],
        )
        .with_context(|| {
            format!(
                "Failed to add dependency {} -> {}",
                blocker_id, blocked_id
            )
        })?;

    Ok(())
}

/// Remove a dependency between two tasks.
pub fn remove_dependency(db: &Db, blocker_id: &str, blocked_id: &str) -> Result<()> {
    db.conn()
        .execute(
            "DELETE FROM dependencies WHERE blocker_id = ? AND blocked_id = ?",
            params![blocker_id, blocked_id],
        )
        .with_context(|| {
            format!(
                "Failed to remove dependency {} -> {}",
                blocker_id, blocked_id
            )
        })?;

    Ok(())
}

/// Check if adding a dependency would create a cycle.
///
/// Returns true if `blocker_id` is reachable from `blocked_id` through existing edges.
/// This uses BFS to traverse the dependency graph.
fn would_create_cycle(db: &Db, blocker_id: &str, blocked_id: &str) -> Result<bool> {
    // If blocker_id == blocked_id, it's a self-loop (caught by CHECK constraint, but we check here too)
    if blocker_id == blocked_id {
        return Ok(true);
    }

    // BFS from blocked_id to see if we can reach blocker_id
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(blocked_id.to_string());
    visited.insert(blocked_id.to_string());

    while let Some(current) = queue.pop_front() {
        // Get all tasks that current blocks (i.e., tasks that depend on current)
        let mut stmt = db.conn().prepare(
            "SELECT blocked_id FROM dependencies WHERE blocker_id = ?",
        )?;
        let dependents: Vec<String> = stmt
            .query_map([&current], |row| row.get(0))?
            .collect::<Result<_, _>>()?;

        for dependent in dependents {
            // If we found blocker_id, adding the edge would create a cycle
            if dependent == blocker_id {
                return Ok(true);
            }

            if !visited.contains(&dependent) {
                visited.insert(dependent.clone());
                queue.push_back(dependent);
            }
        }
    }

    // blocker_id is not reachable from blocked_id, so no cycle
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::init_db;
    use tempfile::NamedTempFile;

    fn create_task(db: &Db, id: &str, title: &str) -> Result<()> {
        db.conn().execute(
            "INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?, ?, datetime('now'), datetime('now'))",
            params![id, title],
        )?;
        Ok(())
    }

    #[test]
    fn test_add_valid_dependency() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;
        create_task(&db, "t-bbb222", "Task B")?;

        add_dependency(&db, "t-aaa111", "t-bbb222")?;

        // Verify dependency exists
        let count: i64 = db.conn().query_row(
            "SELECT COUNT(*) FROM dependencies WHERE blocker_id = ? AND blocked_id = ?",
            params!["t-aaa111", "t-bbb222"],
            |row| row.get(0),
        )?;

        assert_eq!(count, 1);
        Ok(())
    }

    #[test]
    fn test_direct_cycle_rejected() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;
        create_task(&db, "t-bbb222", "Task B")?;

        // A blocks B
        add_dependency(&db, "t-aaa111", "t-bbb222")?;

        // Try B blocks A (would create cycle)
        let result = add_dependency(&db, "t-bbb222", "t-aaa111");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("would create a cycle"));

        Ok(())
    }

    #[test]
    fn test_transitive_cycle_rejected() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;
        create_task(&db, "t-bbb222", "Task B")?;
        create_task(&db, "t-ccc333", "Task C")?;

        // A -> B -> C
        add_dependency(&db, "t-aaa111", "t-bbb222")?;
        add_dependency(&db, "t-bbb222", "t-ccc333")?;

        // Try C -> A (would create cycle)
        let result = add_dependency(&db, "t-ccc333", "t-aaa111");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("would create a cycle"));

        Ok(())
    }

    #[test]
    fn test_remove_dependency() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;
        create_task(&db, "t-bbb222", "Task B")?;

        add_dependency(&db, "t-aaa111", "t-bbb222")?;
        remove_dependency(&db, "t-aaa111", "t-bbb222")?;

        // Verify dependency is gone
        let count: i64 = db.conn().query_row(
            "SELECT COUNT(*) FROM dependencies WHERE blocker_id = ? AND blocked_id = ?",
            params!["t-aaa111", "t-bbb222"],
            |row| row.get(0),
        )?;

        assert_eq!(count, 0);
        Ok(())
    }

    #[test]
    fn test_dependency_on_nonexistent_task_fails() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;

        // Try to add dependency with nonexistent blocker
        let result = add_dependency(&db, "t-nonexistent", "t-aaa111");
        assert!(result.is_err());

        // Try to add dependency with nonexistent blocked
        let result = add_dependency(&db, "t-aaa111", "t-nonexistent");
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_self_dependency_rejected() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;

        // Try to add self-dependency
        let result = add_dependency(&db, "t-aaa111", "t-aaa111");
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_longer_cycle_rejected() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;
        create_task(&db, "t-bbb222", "Task B")?;
        create_task(&db, "t-ccc333", "Task C")?;
        create_task(&db, "t-ddd444", "Task D")?;

        // A -> B -> C -> D
        add_dependency(&db, "t-aaa111", "t-bbb222")?;
        add_dependency(&db, "t-bbb222", "t-ccc333")?;
        add_dependency(&db, "t-ccc333", "t-ddd444")?;

        // Try D -> A (would create cycle)
        let result = add_dependency(&db, "t-ddd444", "t-aaa111");
        assert!(result.is_err());

        // Try D -> B (would create cycle)
        let result = add_dependency(&db, "t-ddd444", "t-bbb222");
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_diamond_structure_allowed() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        create_task(&db, "t-aaa111", "Task A")?;
        create_task(&db, "t-bbb222", "Task B")?;
        create_task(&db, "t-ccc333", "Task C")?;
        create_task(&db, "t-ddd444", "Task D")?;

        // Diamond: A -> B -> D, A -> C -> D (no cycle)
        add_dependency(&db, "t-aaa111", "t-bbb222")?;
        add_dependency(&db, "t-aaa111", "t-ccc333")?;
        add_dependency(&db, "t-bbb222", "t-ddd444")?;
        add_dependency(&db, "t-ccc333", "t-ddd444")?;

        // Verify all dependencies exist
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM dependencies", [], |row| row.get(0))?;

        assert_eq!(count, 4);
        Ok(())
    }
}

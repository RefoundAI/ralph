//! CRUD operations for task management.

#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};

use crate::dag::{generate_and_insert_task_id, task_from_row, Db, Task, TASK_COLUMNS};

/// Fields that can be updated on a task.
#[derive(Debug, Clone, Default)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<i32>,
}

/// Inputs for creating a task with optional feature association.
#[derive(Debug, Clone)]
pub struct CreateTaskParams<'a> {
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub parent_id: Option<&'a str>,
    pub priority: i32,
    pub feature_id: Option<&'a str>,
    pub task_type: &'a str,
    pub max_retries: i32,
}

/// Create a new task.
pub fn create_task(
    db: &Db,
    title: &str,
    description: Option<&str>,
    parent_id: Option<&str>,
    priority: i32,
) -> Result<Task> {
    create_task_with_feature(
        db,
        CreateTaskParams {
            title,
            description,
            parent_id,
            priority,
            feature_id: None,
            task_type: "feature",
            max_retries: 3,
        },
    )
}

/// Create a new task with feature association and task type.
pub fn create_task_with_feature(db: &Db, params: CreateTaskParams<'_>) -> Result<Task> {
    let CreateTaskParams {
        title,
        description,
        parent_id,
        priority,
        feature_id,
        task_type,
        max_retries,
    } = params;

    // Validate parent exists if specified
    if let Some(pid) = parent_id {
        let exists: bool = db
            .conn()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?)",
                [pid],
                |row| row.get(0),
            )
            .context("Failed to check parent existence")?;
        if !exists {
            return Err(anyhow!("Parent task '{}' does not exist", pid));
        }
    }

    let timestamp = chrono::Utc::now().to_rfc3339();
    let desc = description.unwrap_or("");

    // Generate unique ID with retry logic
    let id = generate_and_insert_task_id(
        |id| {
            db.conn().execute(
                "INSERT INTO tasks (id, title, description, parent_id, priority, feature_id, task_type, max_retries, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![id, title, desc, parent_id, priority, feature_id, task_type, max_retries, &timestamp, &timestamp],
            )?;
            Ok(())
        },
        10, // max retries
    )?;

    Ok(Task {
        id: id.clone(),
        title: title.to_string(),
        description: desc.to_string(),
        status: "pending".to_string(),
        parent_id: parent_id.map(|s| s.to_string()),
        feature_id: feature_id.map(|s| s.to_string()),
        task_type: task_type.to_string(),
        priority,
        retry_count: 0,
        max_retries,
        verification_status: None,
        created_at: timestamp.clone(),
        updated_at: timestamp,
        claimed_by: None,
    })
}

/// Get a task by ID.
pub fn get_task(db: &Db, id: &str) -> Result<Task> {
    let query = format!("SELECT {} FROM tasks WHERE id = ?", TASK_COLUMNS);
    db.conn()
        .query_row(&query, [id], task_from_row)
        .context(format!("Failed to get task '{}'", id))
}

/// Update a task's fields.
pub fn update_task(db: &Db, id: &str, fields: TaskUpdate) -> Result<Task> {
    // Check task exists
    let exists: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?)",
            [id],
            |row| row.get(0),
        )
        .context("Failed to check task existence")?;
    if !exists {
        return Err(anyhow!("Task '{}' does not exist", id));
    }

    let timestamp = chrono::Utc::now().to_rfc3339();

    // Build update query dynamically based on which fields are set
    let mut updates = vec![];
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![];

    if let Some(title) = &fields.title {
        updates.push("title = ?");
        params.push(Box::new(title.clone()));
    }
    if let Some(description) = &fields.description {
        updates.push("description = ?");
        params.push(Box::new(description.clone()));
    }
    if let Some(priority) = fields.priority {
        updates.push("priority = ?");
        params.push(Box::new(priority));
    }

    if updates.is_empty() {
        // No fields to update, just return the task
        return get_task(db, id);
    }

    updates.push("updated_at = ?");
    params.push(Box::new(timestamp));

    let query = format!("UPDATE tasks SET {} WHERE id = ?", updates.join(", "));
    params.push(Box::new(id.to_string()));

    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    db.conn().execute(&query, param_refs.as_slice())?;

    get_task(db, id)
}

/// Delete a task.
///
/// Rejects if other tasks depend on it (blocker in dependencies table).
/// Cascade deletes children.
pub fn delete_task(db: &Db, id: &str) -> Result<()> {
    // Check if task exists
    let exists: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?)",
            [id],
            |row| row.get(0),
        )
        .context("Failed to check task existence")?;
    if !exists {
        return Err(anyhow!("Task '{}' does not exist", id));
    }

    // Check if any other tasks depend on this task (it's a blocker)
    let blocker_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM dependencies WHERE blocker_id = ?",
            [id],
            |row| row.get(0),
        )
        .context("Failed to check blocker dependencies")?;

    if blocker_count > 0 {
        return Err(anyhow!(
            "Cannot delete task '{}': other tasks depend on it",
            id
        ));
    }

    // Recursively delete children
    let children: Vec<String> = db
        .conn()
        .prepare("SELECT id FROM tasks WHERE parent_id = ?")?
        .query_map([id], |row| row.get(0))?
        .collect::<Result<_, _>>()
        .context("Failed to get child tasks")?;

    for child_id in children {
        delete_task(db, &child_id)?;
    }

    // Delete dependencies where this task is blocked
    db.conn()
        .execute("DELETE FROM dependencies WHERE blocked_id = ?", [id])?;

    // Delete task logs
    db.conn()
        .execute("DELETE FROM task_logs WHERE task_id = ?", [id])?;

    // Delete the task itself
    db.conn().execute("DELETE FROM tasks WHERE id = ?", [id])?;

    Ok(())
}

/// Delete all tasks associated with a feature.
///
/// Removes dependencies, logs, journal entries, and the tasks themselves.
pub fn delete_tasks_for_feature(db: &Db, feature_id: &str) -> Result<usize> {
    // Get all task IDs for this feature
    let task_ids: Vec<String> = db
        .conn()
        .prepare("SELECT id FROM tasks WHERE feature_id = ?")?
        .query_map([feature_id], |row| row.get(0))?
        .collect::<Result<_, _>>()
        .context("Failed to get feature tasks")?;

    if task_ids.is_empty() {
        return Ok(0);
    }

    let count = task_ids.len();
    let placeholders: Vec<&str> = task_ids.iter().map(|_| "?").collect();
    let placeholder_str = placeholders.join(", ");

    // Delete dependencies involving these tasks
    let sql = format!(
        "DELETE FROM dependencies WHERE blocker_id IN ({ph}) OR blocked_id IN ({ph})",
        ph = placeholder_str,
    );
    let mut stmt = db.conn().prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = task_ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .chain(task_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql))
        .collect();
    stmt.execute(params.as_slice())?;

    // Delete task logs
    let sql = format!(
        "DELETE FROM task_logs WHERE task_id IN ({})",
        placeholder_str
    );
    let mut stmt = db.conn().prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = task_ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect();
    stmt.execute(params.as_slice())?;

    // Delete journal entries referencing this feature
    db.conn()
        .execute("DELETE FROM journal WHERE feature_id = ?", [feature_id])?;

    // Delete the tasks themselves
    let sql = format!("DELETE FROM tasks WHERE feature_id = ?");
    db.conn().execute(&sql, [feature_id])?;

    Ok(count)
}

/// Get task tree rooted at a task.
///
/// Returns the root task and all its descendants in a flat list.
pub fn get_task_tree(db: &Db, root_id: &str) -> Result<Vec<Task>> {
    // Check root exists
    let root = get_task(db, root_id)?;

    let mut tasks = vec![root];
    let mut queue = vec![root_id.to_string()];

    while let Some(parent_id) = queue.pop() {
        let query = format!("SELECT {} FROM tasks WHERE parent_id = ?", TASK_COLUMNS);
        let children: Vec<Task> = db
            .conn()
            .prepare(&query)?
            .query_map([&parent_id], task_from_row)?
            .collect::<Result<_, _>>()
            .context("Failed to get child tasks")?;

        for child in children {
            queue.push(child.id.clone());
            tasks.push(child);
        }
    }

    Ok(tasks)
}

/// Add a log message for a task.
pub fn add_log(db: &Db, task_id: &str, message: &str) -> Result<()> {
    // Check task exists
    let exists: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?)",
            [task_id],
            |row| row.get(0),
        )
        .context("Failed to check task existence")?;
    if !exists {
        return Err(anyhow!("Task '{}' does not exist", task_id));
    }

    let timestamp = chrono::Utc::now().to_rfc3339();
    db.conn().execute(
        "INSERT INTO task_logs (task_id, message, timestamp) VALUES (?, ?, ?)",
        rusqlite::params![task_id, message, timestamp],
    )?;

    Ok(())
}

/// A log entry for a task.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub task_id: String,
    pub message: String,
    pub timestamp: String,
}

/// Get all log entries for a task.
pub fn get_task_logs(db: &Db, task_id: &str) -> Result<Vec<LogEntry>> {
    let mut stmt = db.conn().prepare(
        "SELECT task_id, message, timestamp FROM task_logs WHERE task_id = ? ORDER BY timestamp ASC",
    )?;

    let logs = stmt
        .query_map([task_id], |row| {
            Ok(LogEntry {
                task_id: row.get(0)?,
                message: row.get(1)?,
                timestamp: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(logs)
}

/// Get tasks that block a given task (its blockers/prerequisites).
pub fn get_task_blockers(db: &Db, task_id: &str) -> Result<Vec<Task>> {
    let query = format!(
        "SELECT t.{} FROM dependencies d JOIN tasks t ON d.blocker_id = t.id WHERE d.blocked_id = ?",
        TASK_COLUMNS.replace(", ", ", t."),
    );
    let mut stmt = db.conn().prepare(&query)?;

    let tasks = stmt
        .query_map([task_id], task_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// Get tasks that are blocked by a given task (its dependents).
pub fn get_tasks_blocked_by(db: &Db, task_id: &str) -> Result<Vec<Task>> {
    let query = format!(
        "SELECT t.{} FROM dependencies d JOIN tasks t ON d.blocked_id = t.id WHERE d.blocker_id = ?",
        TASK_COLUMNS.replace(", ", ", t."),
    );
    let mut stmt = db.conn().prepare(&query)?;

    let tasks = stmt
        .query_map([task_id], task_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// Get all tasks for a feature.
pub fn get_all_tasks_for_feature(db: &Db, feature_id: &str) -> Result<Vec<Task>> {
    let query = format!(
        "SELECT {} FROM tasks WHERE feature_id = ? ORDER BY priority ASC, created_at ASC",
        TASK_COLUMNS,
    );
    let mut stmt = db.conn().prepare(&query)?;

    let tasks = stmt
        .query_map([feature_id], task_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

/// Get all tasks in the database.
pub fn get_all_tasks(db: &Db) -> Result<Vec<Task>> {
    let query = format!(
        "SELECT {} FROM tasks ORDER BY priority ASC, created_at ASC",
        TASK_COLUMNS,
    );
    let mut stmt = db.conn().prepare(&query)?;

    let tasks = stmt
        .query_map([], task_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::{add_dependency, init_db};
    use tempfile::NamedTempFile;

    #[test]
    fn test_create_task() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let task = create_task(&db, "Test Task", Some("A test task"), None, 0).unwrap();
        assert_eq!(task.title, "Test Task");
        assert_eq!(task.description, "A test task");
        assert!(task.id.starts_with("t-"));
        assert!(task.parent_id.is_none());
    }

    #[test]
    fn test_create_task_with_parent() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let parent = create_task(&db, "Parent", None, None, 0).unwrap();
        let child = create_task(&db, "Child", None, Some(&parent.id), 0).unwrap();

        assert_eq!(child.parent_id, Some(parent.id));
    }

    #[test]
    fn test_create_task_with_invalid_parent() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let result = create_task(&db, "Child", None, Some("t-nonexistent"), 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_get_task() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let created = create_task(&db, "Test Task", Some("Description"), None, 5).unwrap();
        let retrieved = get_task(&db, &created.id).unwrap();

        assert_eq!(retrieved.id, created.id);
        assert_eq!(retrieved.title, "Test Task");
        assert_eq!(retrieved.description, "Description");
    }

    #[test]
    fn test_get_nonexistent_task() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let result = get_task(&db, "t-nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_task_title() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let task = create_task(&db, "Original", None, None, 0).unwrap();
        let update = TaskUpdate {
            title: Some("Updated".to_string()),
            ..Default::default()
        };

        let updated = update_task(&db, &task.id, update).unwrap();
        assert_eq!(updated.title, "Updated");
        assert_eq!(updated.description, "");
    }

    #[test]
    fn test_update_task_multiple_fields() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let task = create_task(&db, "Original", Some("Original desc"), None, 0).unwrap();
        let update = TaskUpdate {
            title: Some("Updated".to_string()),
            description: Some("Updated desc".to_string()),
            priority: Some(10),
        };

        let updated = update_task(&db, &task.id, update).unwrap();
        assert_eq!(updated.title, "Updated");
        assert_eq!(updated.description, "Updated desc");
    }

    #[test]
    fn test_update_nonexistent_task() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let update = TaskUpdate {
            title: Some("Updated".to_string()),
            ..Default::default()
        };
        let result = update_task(&db, "t-nonexistent", update);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_task() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let task = create_task(&db, "To Delete", None, None, 0).unwrap();
        delete_task(&db, &task.id).unwrap();

        let result = get_task(&db, &task.id);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_task_cascade_children() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let parent = create_task(&db, "Parent", None, None, 0).unwrap();
        let child = create_task(&db, "Child", None, Some(&parent.id), 0).unwrap();

        delete_task(&db, &parent.id).unwrap();

        // Both parent and child should be deleted
        assert!(get_task(&db, &parent.id).is_err());
        assert!(get_task(&db, &child.id).is_err());
    }

    #[test]
    fn test_delete_task_rejects_if_blocker() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let blocker = create_task(&db, "Blocker", None, None, 0).unwrap();
        let blocked = create_task(&db, "Blocked", None, None, 0).unwrap();
        add_dependency(&db, &blocker.id, &blocked.id).unwrap();

        let result = delete_task(&db, &blocker.id);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("other tasks depend on it"));
    }

    #[test]
    fn test_delete_task_allows_if_blocked() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let blocker = create_task(&db, "Blocker", None, None, 0).unwrap();
        let blocked = create_task(&db, "Blocked", None, None, 0).unwrap();
        add_dependency(&db, &blocker.id, &blocked.id).unwrap();

        // Deleting the blocked task should succeed
        delete_task(&db, &blocked.id).unwrap();
        assert!(get_task(&db, &blocked.id).is_err());
    }

    #[test]
    fn test_get_task_tree_single() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let task = create_task(&db, "Root", None, None, 0).unwrap();
        let tree = get_task_tree(&db, &task.id).unwrap();

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].id, task.id);
    }

    #[test]
    fn test_get_task_tree_with_children() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let root = create_task(&db, "Root", None, None, 0).unwrap();
        let child1 = create_task(&db, "Child 1", None, Some(&root.id), 0).unwrap();
        let child2 = create_task(&db, "Child 2", None, Some(&root.id), 0).unwrap();
        let grandchild = create_task(&db, "Grandchild", None, Some(&child1.id), 0).unwrap();

        let tree = get_task_tree(&db, &root.id).unwrap();

        assert_eq!(tree.len(), 4);
        let ids: Vec<&str> = tree.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&root.id.as_str()));
        assert!(ids.contains(&child1.id.as_str()));
        assert!(ids.contains(&child2.id.as_str()));
        assert!(ids.contains(&grandchild.id.as_str()));
    }

    #[test]
    fn test_get_task_tree_nonexistent() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let result = get_task_tree(&db, "t-nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_add_log() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let task = create_task(&db, "Task", None, None, 0).unwrap();
        add_log(&db, &task.id, "Test log message").unwrap();

        // Verify log was inserted
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM task_logs WHERE task_id = ? AND message = ?",
                rusqlite::params![task.id, "Test log message"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_add_log_nonexistent_task() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let result = add_log(&db, "t-nonexistent", "Message");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_delete_tasks_for_feature() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        // Create a feature
        let feat = crate::feature::create_feature(&db, "test-feat").unwrap();

        // Create tasks for the feature
        let t1 = create_task_with_feature(
            &db,
            CreateTaskParams {
                title: "Task 1",
                description: None,
                parent_id: None,
                priority: 0,
                feature_id: Some(&feat.id),
                task_type: "feature",
                max_retries: 3,
            },
        )
        .unwrap();
        let t2 = create_task_with_feature(
            &db,
            CreateTaskParams {
                title: "Task 2",
                description: None,
                parent_id: Some(&t1.id),
                priority: 0,
                feature_id: Some(&feat.id),
                task_type: "feature",
                max_retries: 3,
            },
        )
        .unwrap();

        // Add a log entry
        add_log(&db, &t1.id, "some log").unwrap();

        // Add a dependency between feature tasks
        add_dependency(&db, &t1.id, &t2.id).unwrap();

        // Create a standalone task that should NOT be deleted
        let standalone = create_task(&db, "Standalone", None, None, 0).unwrap();

        // Delete feature tasks
        let count = delete_tasks_for_feature(&db, &feat.id).unwrap();
        assert_eq!(count, 2);

        // Feature tasks should be gone
        assert!(get_task(&db, &t1.id).is_err());
        assert!(get_task(&db, &t2.id).is_err());

        // Standalone task should still exist
        assert!(get_task(&db, &standalone.id).is_ok());
    }

    #[test]
    fn test_delete_tasks_for_feature_no_tasks() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let count = delete_tasks_for_feature(&db, "f-nonexistent").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_integration_create_graph_claim_complete() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        // Create: A blocks B, B blocks C
        let a = create_task(&db, "Task A", None, None, 0).unwrap();
        let b = create_task(&db, "Task B", None, None, 0).unwrap();
        let c = create_task(&db, "Task C", None, None, 0).unwrap();

        add_dependency(&db, &a.id, &b.id).unwrap();
        add_dependency(&db, &b.id, &c.id).unwrap();

        // Initially, only A is ready
        let ready = crate::dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, a.id);

        // Claim A and complete it
        crate::dag::claim_task(&db, &a.id, "agent-test").unwrap();
        crate::dag::complete_task(&db, &a.id).unwrap();

        // Now B should be ready
        let ready = crate::dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, b.id);

        // Claim B and complete it
        crate::dag::claim_task(&db, &b.id, "agent-test").unwrap();
        crate::dag::complete_task(&db, &b.id).unwrap();

        // Now C should be ready
        let ready = crate::dag::get_ready_tasks(&db).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, c.id);
    }
}

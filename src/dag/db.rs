//! Database connection and initialization.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Current schema version.
const SCHEMA_VERSION: i32 = 3;

/// SQLite database wrapper.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Get a reference to the underlying connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Open or initialize the database at the given path.
pub fn init_db(path: &str) -> Result<Db> {
    // Create parent directories if needed
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory for {}", path))?;
    }

    // Open or create database
    let conn =
        Connection::open(path).with_context(|| format!("Failed to open database at {}", path))?;

    // Enable WAL mode
    conn.pragma_update(None, "journal_mode", "WAL")
        .context("Failed to enable WAL mode")?;

    // Enable foreign keys
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("Failed to enable foreign keys")?;

    // Check schema version
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .context("Failed to read schema version")?;

    if version < SCHEMA_VERSION {
        // Run migrations
        migrate(&conn, version, SCHEMA_VERSION)?;
    }

    Ok(Db { conn })
}

/// Run migrations from `from_version` to `to_version`.
fn migrate(conn: &Connection, from_version: i32, to_version: i32) -> Result<()> {
    if from_version < 1 && to_version >= 1 {
        // Initial schema
        conn.execute_batch(
            r#"
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                parent_id TEXT REFERENCES tasks(id),
                title TEXT NOT NULL,
                description TEXT,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','in_progress','done','blocked','failed')),
                priority INTEGER DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                claimed_by TEXT
            );

            CREATE TABLE dependencies (
                blocker_id TEXT NOT NULL REFERENCES tasks(id),
                blocked_id TEXT NOT NULL REFERENCES tasks(id),
                PRIMARY KEY (blocker_id, blocked_id),
                CHECK (blocker_id != blocked_id)
            );

            CREATE TABLE task_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL REFERENCES tasks(id),
                message TEXT NOT NULL,
                timestamp TEXT NOT NULL
            );
            "#,
        )
        .context("Failed to create schema v1")?;
    }

    if from_version < 2 && to_version >= 2 {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS features (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                spec_path TEXT,
                plan_path TEXT,
                status TEXT NOT NULL DEFAULT 'draft'
                    CHECK (status IN ('draft','planned','ready','running','done','failed')),
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            ALTER TABLE tasks ADD COLUMN feature_id TEXT REFERENCES features(id);
            ALTER TABLE tasks ADD COLUMN task_type TEXT DEFAULT 'feature'
                CHECK (task_type IN ('feature','standalone'));
            ALTER TABLE tasks ADD COLUMN retry_count INTEGER DEFAULT 0;
            ALTER TABLE tasks ADD COLUMN max_retries INTEGER DEFAULT 3;
            ALTER TABLE tasks ADD COLUMN verification_status TEXT
                CHECK (verification_status IN ('pending','passed','failed'));
            "#,
        )
        .context("Failed to create schema v2")?;
    }

    if from_version < 3 && to_version >= 3 {
        conn.execute_batch(
            r#"
            CREATE TABLE journal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                iteration INTEGER NOT NULL,
                task_id TEXT REFERENCES tasks(id),
                feature_id TEXT REFERENCES features(id),
                outcome TEXT NOT NULL
                    CHECK (outcome IN ('done','failed','retried','blocked')),
                model TEXT,
                duration_secs REAL,
                cost_usd REAL,
                files_modified TEXT,
                notes TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX idx_journal_run_id ON journal(run_id, iteration);
            CREATE INDEX idx_journal_feature_id ON journal(feature_id);

            CREATE VIRTUAL TABLE journal_fts USING fts5(
                notes,
                content='journal',
                content_rowid='id'
            );

            CREATE TRIGGER journal_ai AFTER INSERT ON journal BEGIN
                INSERT INTO journal_fts(rowid, notes) VALUES (new.id, new.notes);
            END;

            CREATE TRIGGER journal_ad AFTER DELETE ON journal BEGIN
                INSERT INTO journal_fts(journal_fts, rowid, notes)
                    VALUES('delete', old.id, old.notes);
            END;

            CREATE TRIGGER journal_au AFTER UPDATE ON journal BEGIN
                INSERT INTO journal_fts(journal_fts, rowid, notes)
                    VALUES('delete', old.id, old.notes);
                INSERT INTO journal_fts(rowid, notes) VALUES (new.id, new.notes);
            END;
            "#,
        )
        .context("Failed to create schema v3")?;
    }

    // Set schema version
    conn.pragma_update(None, "user_version", to_version)
        .context("Failed to update schema version")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_fresh_db_creates_tables() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        // Check that tables exist
        let mut stmt = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;

        assert!(tables.contains(&"tasks".to_string()));
        assert!(tables.contains(&"dependencies".to_string()));
        assert!(tables.contains(&"task_logs".to_string()));

        Ok(())
    }

    #[test]
    fn test_foreign_key_constraint_enforced() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        // Try to insert dependency with nonexistent task
        let result = db.conn().execute(
            "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
            ["t-abc123", "t-def456"],
        );

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_duplicate_dependency_rejected() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        // Create two tasks
        db.conn().execute(
            "INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)",
            [
                "t-abc123",
                "Task A",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z",
            ],
        )?;
        db.conn().execute(
            "INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)",
            [
                "t-def456",
                "Task B",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z",
            ],
        )?;

        // Insert dependency
        db.conn().execute(
            "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
            ["t-abc123", "t-def456"],
        )?;

        // Try to insert duplicate
        let result = db.conn().execute(
            "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
            ["t-abc123", "t-def456"],
        );

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_self_dependency_rejected() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        // Create a task
        db.conn().execute(
            "INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)",
            [
                "t-abc123",
                "Task A",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z",
            ],
        )?;

        // Try to insert self-dependency
        let result = db.conn().execute(
            "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?, ?)",
            ["t-abc123", "t-abc123"],
        );

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_wal_mode_enabled() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        let mode: String = db
            .conn()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))?;

        assert_eq!(mode.to_lowercase(), "wal");
        Ok(())
    }

    #[test]
    fn test_foreign_keys_enabled() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        let enabled: i32 = db
            .conn()
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))?;

        assert_eq!(enabled, 1);
        Ok(())
    }

    #[test]
    fn test_idempotent_reopen() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_str().unwrap();

        // Initialize once
        let db1 = init_db(path)?;
        let version1: i32 = db1
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))?;

        drop(db1);

        // Re-open
        let db2 = init_db(path)?;
        let version2: i32 = db2
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))?;

        assert_eq!(version1, version2);
        assert_eq!(version1, SCHEMA_VERSION);

        Ok(())
    }

    #[test]
    fn test_fts5_available() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE test_fts USING fts5(content);")
            .unwrap();
        conn.execute("INSERT INTO test_fts(content) VALUES (?1)", ["hello world"])
            .unwrap();
        let count: i32 = conn
            .query_row(
                "SELECT count(*) FROM test_fts WHERE test_fts MATCH 'hello'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_schema_v3_fresh_db() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        // Check that all v3 tables/virtual tables exist
        let mut stmt = db.conn().prepare(
            "SELECT name FROM sqlite_master WHERE type IN ('table','shadow') ORDER BY name",
        )?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;

        assert!(
            names.contains(&"journal".to_string()),
            "journal table missing"
        );
        assert!(
            names.contains(&"journal_fts".to_string()),
            "journal_fts virtual table missing"
        );

        Ok(())
    }

    #[test]
    fn test_schema_v3_migration_from_v2() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let path = temp_file.path().to_str().unwrap();

        // Manually create a v2 database
        {
            let conn = Connection::open(path)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
            // Run migrations 0→2 only
            migrate(&conn, 0, 2)?;
            // Mark as version 2
            conn.pragma_update(None, "user_version", 2)?;
        }

        // Now open via init_db which should migrate from v2 to v3
        let db = init_db(path)?;

        // Verify version is now 3
        let version: i32 = db
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, 3);

        // Verify journal table exists
        let count: i32 = db.conn().query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='journal'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "journal table should exist after v2→v3 migration");

        // Verify v1/v2 tables are still intact
        let mut stmt = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert!(tables.contains(&"tasks".to_string()), "tasks table missing");
        assert!(
            tables.contains(&"dependencies".to_string()),
            "dependencies table missing"
        );
        assert!(
            tables.contains(&"task_logs".to_string()),
            "task_logs table missing"
        );
        assert!(
            tables.contains(&"features".to_string()),
            "features table missing"
        );

        Ok(())
    }

    #[test]
    fn test_schema_v3_fts_triggers() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let db = init_db(temp_file.path().to_str().unwrap())?;

        // Insert a row with notes
        db.conn().execute(
            "INSERT INTO journal (run_id, iteration, outcome, notes) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["run-test01", 1, "done", "implemented the FTS5 parser"],
        )?;

        // Verify FTS matches
        let count: i32 = db.conn().query_row(
            "SELECT count(*) FROM journal_fts WHERE journal_fts MATCH 'FTS5'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "FTS should match 'FTS5' after insert");

        // Get the inserted row id
        let row_id: i64 = db.conn().last_insert_rowid();

        // Update the notes
        db.conn().execute(
            "UPDATE journal SET notes = ?1 WHERE id = ?2",
            rusqlite::params!["updated notes about database migration", row_id],
        )?;

        // Old text should no longer match
        let old_count: i32 = db.conn().query_row(
            "SELECT count(*) FROM journal_fts WHERE journal_fts MATCH 'FTS5'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(old_count, 0, "FTS should not match old text after update");

        // New text should match
        let new_count: i32 = db.conn().query_row(
            "SELECT count(*) FROM journal_fts WHERE journal_fts MATCH 'migration'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(new_count, 1, "FTS should match new text after update");

        // Delete the row
        db.conn().execute(
            "DELETE FROM journal WHERE id = ?1",
            rusqlite::params![row_id],
        )?;

        // FTS should be empty
        let after_delete: i32 = db.conn().query_row(
            "SELECT count(*) FROM journal_fts WHERE journal_fts MATCH 'migration'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(after_delete, 0, "FTS should be empty after delete");

        Ok(())
    }
}

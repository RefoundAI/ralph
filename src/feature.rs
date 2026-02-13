//! Feature management: CRUD operations and file management.

use anyhow::{anyhow, Context, Result};
use std::path::Path;

use crate::dag::{generate_feature_id, Db};

/// A feature in the DAG.
#[derive(Debug, Clone)]
pub struct Feature {
    pub id: String,
    pub name: String,
    pub spec_path: Option<String>,
    pub plan_path: Option<String>,
    pub status: String,
}

/// Create a new feature in the database.
pub fn create_feature(db: &Db, name: &str) -> Result<Feature> {
    // Check if feature with this name already exists
    let exists: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM features WHERE name = ?)",
            [name],
            |row| row.get(0),
        )
        .context("Failed to check feature existence")?;
    if exists {
        return Err(anyhow!("Feature '{}' already exists", name));
    }

    let id = generate_feature_id();
    let timestamp = chrono::Utc::now().to_rfc3339();

    db.conn().execute(
        "INSERT INTO features (id, name, status, created_at, updated_at) VALUES (?, ?, 'draft', ?, ?)",
        rusqlite::params![id, name, timestamp, timestamp],
    ).context("Failed to create feature")?;

    Ok(Feature {
        id,
        name: name.to_string(),
        spec_path: None,
        plan_path: None,
        status: "draft".to_string(),
    })
}

/// Get a feature by name.
pub fn get_feature(db: &Db, name: &str) -> Result<Feature> {
    db.conn()
        .query_row(
            "SELECT id, name, spec_path, plan_path, status FROM features WHERE name = ?",
            [name],
            |row| {
                Ok(Feature {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    spec_path: row.get(2)?,
                    plan_path: row.get(3)?,
                    status: row.get(4)?,
                })
            },
        )
        .context(format!("Feature '{}' not found", name))
}

/// Get a feature by ID.
pub fn get_feature_by_id(db: &Db, id: &str) -> Result<Feature> {
    db.conn()
        .query_row(
            "SELECT id, name, spec_path, plan_path, status FROM features WHERE id = ?",
            [id],
            |row| {
                Ok(Feature {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    spec_path: row.get(2)?,
                    plan_path: row.get(3)?,
                    status: row.get(4)?,
                })
            },
        )
        .context(format!("Feature with id '{}' not found", id))
}

/// List all features.
pub fn list_features(db: &Db) -> Result<Vec<Feature>> {
    let mut stmt = db.conn().prepare(
        "SELECT id, name, spec_path, plan_path, status FROM features ORDER BY created_at ASC",
    )?;

    let features = stmt
        .query_map([], |row| {
            Ok(Feature {
                id: row.get(0)?,
                name: row.get(1)?,
                spec_path: row.get(2)?,
                plan_path: row.get(3)?,
                status: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(features)
}

/// Update a feature's status.
pub fn update_feature_status(db: &Db, id: &str, status: &str) -> Result<()> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    let updated = db.conn().execute(
        "UPDATE features SET status = ?, updated_at = ? WHERE id = ?",
        rusqlite::params![status, timestamp, id],
    )?;
    if updated == 0 {
        return Err(anyhow!("Feature '{}' not found", id));
    }
    Ok(())
}

/// Update a feature's spec_path.
pub fn update_feature_spec_path(db: &Db, id: &str, spec_path: &str) -> Result<()> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    db.conn().execute(
        "UPDATE features SET spec_path = ?, updated_at = ? WHERE id = ?",
        rusqlite::params![spec_path, timestamp, id],
    )?;
    Ok(())
}

/// Update a feature's plan_path.
pub fn update_feature_plan_path(db: &Db, id: &str, plan_path: &str) -> Result<()> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    db.conn().execute(
        "UPDATE features SET plan_path = ?, updated_at = ? WHERE id = ?",
        rusqlite::params![plan_path, timestamp, id],
    )?;
    Ok(())
}

/// Ensure feature directories exist under the project root.
///
/// Creates `.ralph/features/<name>/` if it doesn't exist.
pub fn ensure_feature_dirs(project_root: &Path, name: &str) -> Result<()> {
    let feature_dir = project_root.join(".ralph/features").join(name);
    std::fs::create_dir_all(&feature_dir)
        .with_context(|| format!("Failed to create feature directory: {}", feature_dir.display()))?;
    Ok(())
}

/// Read the spec file for a feature.
pub fn read_spec(project_root: &Path, name: &str) -> Result<String> {
    let spec_path = project_root.join(".ralph/features").join(name).join("spec.md");
    std::fs::read_to_string(&spec_path)
        .with_context(|| format!("Failed to read spec: {}", spec_path.display()))
}

/// Read the plan file for a feature.
pub fn read_plan(project_root: &Path, name: &str) -> Result<String> {
    let plan_path = project_root.join(".ralph/features").join(name).join("plan.md");
    std::fs::read_to_string(&plan_path)
        .with_context(|| format!("Failed to read plan: {}", plan_path.display()))
}

/// Check if a feature name exists in the database.
pub fn feature_exists(db: &Db, name: &str) -> Result<bool> {
    let exists: bool = db
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM features WHERE name = ?)",
            [name],
            |row| row.get(0),
        )?;
    Ok(exists)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::init_db;
    use tempfile::NamedTempFile;

    #[test]
    fn test_create_feature() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let feature = create_feature(&db, "auth").unwrap();
        assert!(feature.id.starts_with("f-"));
        assert_eq!(feature.name, "auth");
        assert_eq!(feature.status, "draft");
        assert!(feature.spec_path.is_none());
        assert!(feature.plan_path.is_none());
    }

    #[test]
    fn test_create_duplicate_feature_fails() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        create_feature(&db, "auth").unwrap();
        let result = create_feature(&db, "auth");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_get_feature() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let created = create_feature(&db, "auth").unwrap();
        let retrieved = get_feature(&db, "auth").unwrap();
        assert_eq!(retrieved.id, created.id);
        assert_eq!(retrieved.name, "auth");
    }

    #[test]
    fn test_get_nonexistent_feature() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let result = get_feature(&db, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_features() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        create_feature(&db, "auth").unwrap();
        create_feature(&db, "cache").unwrap();

        let features = list_features(&db).unwrap();
        assert_eq!(features.len(), 2);
    }

    #[test]
    fn test_update_feature_status() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        let feature = create_feature(&db, "auth").unwrap();
        update_feature_status(&db, &feature.id, "planned").unwrap();

        let updated = get_feature(&db, "auth").unwrap();
        assert_eq!(updated.status, "planned");
    }

    #[test]
    fn test_feature_exists() {
        let temp = NamedTempFile::new().unwrap();
        let db = init_db(temp.path().to_str().unwrap()).unwrap();

        assert!(!feature_exists(&db, "auth").unwrap());
        create_feature(&db, "auth").unwrap();
        assert!(feature_exists(&db, "auth").unwrap());
    }

    #[test]
    fn test_ensure_feature_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Create .ralph/features base dir
        std::fs::create_dir_all(root.join(".ralph/features")).unwrap();

        ensure_feature_dirs(root, "auth").unwrap();
        assert!(root.join(".ralph/features/auth").is_dir());
    }
}

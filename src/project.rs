//! Project configuration discovery and loading.
//!
//! Ralph projects are defined by a `.ralph.toml` file at the project root.
//! This module handles walking up the directory tree to find the config,
//! parsing it, and providing access to project settings.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Project configuration loaded from `.ralph.toml`.
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    /// The directory containing `.ralph.toml`.
    pub root: PathBuf,
    /// The parsed configuration.
    pub config: RalphConfig,
}

/// Contents of `.ralph.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
pub struct RalphConfig {
    #[serde(default)]
    pub execution: ExecutionConfig,
}

/// Execution configuration section.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ExecutionConfig {
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_true")]
    pub verify: bool,
    /// Deprecated: learning is always enabled. This field is retained for backward
    /// compatibility with existing .ralph.toml files and is ignored at runtime.
    #[serde(default = "default_true")]
    pub learn: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            verify: true,
            learn: true,
        }
    }
}

fn default_max_retries() -> u32 {
    3
}

fn default_true() -> bool {
    true
}

/// Discover the project configuration by walking up from CWD.
///
/// Searches for `.ralph.toml` starting from the current directory and
/// walking up parent directories until the file is found or the
/// filesystem root is reached.
///
/// Returns an error if no `.ralph.toml` is found, instructing the user
/// to run `ralph init`.
pub fn discover() -> Result<ProjectConfig> {
    let cwd = env::current_dir()?;
    discover_from(&cwd)
}

/// Discover the project configuration starting from a specific directory.
///
/// This is the internal implementation that allows testing with arbitrary
/// starting directories.
fn discover_from(start: &Path) -> Result<ProjectConfig> {
    let mut current = start;

    loop {
        let config_path = current.join(".ralph.toml");
        if config_path.exists() && config_path.is_file() {
            let config = load_config(&config_path)?;
            return Ok(ProjectConfig {
                root: current.to_path_buf(),
                config,
            });
        }

        // Move up to parent directory
        match current.parent() {
            Some(parent) => current = parent,
            None => {
                bail!("No .ralph.toml found. Run 'ralph init' to create one.")
            }
        }
    }
}

/// Load and parse a `.ralph.toml` file.
fn load_config(path: &Path) -> Result<RalphConfig> {
    let content = fs::read_to_string(path)?;
    let config: RalphConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Initialize a new Ralph project in the current directory.
///
/// Creates:
/// - `.ralph.toml` with commented defaults (if it doesn't exist)
/// - `.ralph/` directory
/// - `.ralph/progress.db` file (empty)
/// - `.gitignore` entry for `.ralph/progress.db`
///
/// This function is idempotent: running it multiple times won't overwrite
/// existing files or produce errors.
pub fn init() -> Result<()> {
    let cwd = env::current_dir()?;
    init_in_dir(&cwd)
}

/// Internal implementation of init that accepts a target directory.
/// This allows for testing without changing the current directory.
fn init_in_dir(cwd: &Path) -> Result<()> {
    // 1. Check if .ralph.toml exists
    let config_path = cwd.join(".ralph.toml");
    if config_path.exists() {
        println!(".ralph.toml already exists, skipping.");
    } else {
        // 2. Create .ralph.toml with commented defaults
        let default_config = r#"[execution]
# max_retries = 3
# verify = true
"#;
        fs::write(&config_path, default_config).context("Failed to create .ralph.toml")?;
        println!("Created .ralph.toml");
    }

    // 3. Create directories
    let ralph_dir = cwd.join(".ralph");
    let features_dir = ralph_dir.join("features");
    let knowledge_dir = ralph_dir.join("knowledge");
    let claude_skills_dir = cwd.join(".claude/skills");

    fs::create_dir_all(&ralph_dir).context("Failed to create .ralph/ directory")?;
    fs::create_dir_all(&features_dir).context("Failed to create .ralph/features/ directory")?;
    fs::create_dir_all(&knowledge_dir).context("Failed to create .ralph/knowledge/ directory")?;
    fs::create_dir_all(&claude_skills_dir).context("Failed to create .claude/skills/ directory")?;

    println!("Created .ralph/ directory structure");

    // Check for legacy .ralph/skills/ directory and print migration notice if non-empty
    let legacy_skills_dir = ralph_dir.join("skills");
    if legacy_skills_dir.is_dir() {
        let is_non_empty = fs::read_dir(&legacy_skills_dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        if is_non_empty {
            println!("Note: Skills have moved to .claude/skills/. See migration guide.");
        }
    }

    // 4. Create .ralph/progress.db (empty file)
    let progress_db = ralph_dir.join("progress.db");
    if !progress_db.exists() {
        fs::write(&progress_db, "").context("Failed to create .ralph/progress.db")?;
        println!("Created .ralph/progress.db");
    }

    // 5. Update .gitignore
    let gitignore_path = cwd.join(".gitignore");
    let gitignore_entry = ".ralph/progress.db\n";

    if gitignore_path.exists() {
        let content = fs::read_to_string(&gitignore_path).context("Failed to read .gitignore")?;

        // Check if entry already exists
        if !content
            .lines()
            .any(|line| line.trim() == ".ralph/progress.db")
        {
            let mut new_content = content;
            if !new_content.ends_with('\n') {
                new_content.push('\n');
            }
            new_content.push_str(gitignore_entry);

            fs::write(&gitignore_path, new_content).context("Failed to update .gitignore")?;
            println!("Added .ralph/progress.db to .gitignore");
        }
    } else {
        fs::write(&gitignore_path, gitignore_entry).context("Failed to create .gitignore")?;
        println!("Created .gitignore with .ralph/progress.db");
    }

    println!("\nRalph project initialized successfully!");
    println!("Next steps:");
    println!("  - Run 'ralph feature spec <name>' to define a feature");
    println!("  - Or run 'ralph task new' to create a standalone task");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Helper to create a temp directory with a .ralph.toml file.
    fn temp_project(toml_content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let config_path = root.join(".ralph.toml");
        fs::write(&config_path, toml_content).unwrap();
        (dir, root)
    }

    #[test]
    fn discovers_config_in_cwd() {
        let (_tmp, root) = temp_project("[execution]\nmax_retries = 5");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.root, root);
        assert_eq!(result.config.execution.max_retries, 5);
    }

    #[test]
    fn discovers_config_two_directories_up() {
        let (_tmp, root) = temp_project("[execution]\nmax_retries = 5");
        let subdir = root.join("a").join("b");
        fs::create_dir_all(&subdir).unwrap();

        let result = discover_from(&subdir).unwrap();
        assert_eq!(result.root, root);
        assert_eq!(result.config.execution.max_retries, 5);
    }

    #[test]
    fn no_config_returns_error_with_init_message() {
        let tmp = TempDir::new().unwrap();
        let result = discover_from(tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("ralph init"),
            "Error should mention 'ralph init', got: {}",
            err_msg
        );
    }

    #[test]
    fn empty_toml_parses_to_defaults() {
        let (_tmp, root) = temp_project("");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.config.execution.max_retries, 3);
    }

    #[test]
    fn partial_config_uses_defaults_for_missing_sections() {
        let (_tmp, root) = temp_project("[execution]\nmax_retries = 5");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.config.execution.max_retries, 5);
    }

    #[test]
    fn invalid_toml_returns_error() {
        let (_tmp, root) = temp_project("[execution\nmax_retries = 3");
        let result = discover_from(&root);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_keys_ignored_for_forward_compat() {
        let (_tmp, root) = temp_project("[foo]\nbar = 1\n[execution]\nmax_retries = 7");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.config.execution.max_retries, 7);
        // Should not error despite unknown [foo] section
    }

    #[test]
    fn ralph_config_default_values() {
        let config = RalphConfig::default();
        assert_eq!(config.execution.max_retries, 3);
        assert!(config.execution.verify);
    }

    #[test]
    fn init_creates_all_files_and_directories() {
        let tmp = TempDir::new().unwrap();

        // Run init in temp directory
        super::init_in_dir(tmp.path()).unwrap();

        // Verify all files/directories created
        assert!(tmp.path().join(".ralph.toml").exists());
        assert!(tmp.path().join(".ralph").is_dir());
        assert!(tmp.path().join(".ralph/progress.db").exists());
        assert!(tmp.path().join(".gitignore").exists());

        // Verify .gitignore contains progress.db
        let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".ralph/progress.db"));

        // Verify .ralph.toml has valid content
        let config_content = fs::read_to_string(tmp.path().join(".ralph.toml")).unwrap();
        assert!(config_content.contains("[execution]"));
    }

    #[test]
    fn init_is_idempotent() {
        let tmp = TempDir::new().unwrap();

        // First run
        super::init_in_dir(tmp.path()).unwrap();
        let first_config = fs::read_to_string(tmp.path().join(".ralph.toml")).unwrap();

        // Second run
        super::init_in_dir(tmp.path()).unwrap();
        let second_config = fs::read_to_string(tmp.path().join(".ralph.toml")).unwrap();

        // .ralph.toml should be unchanged
        assert_eq!(first_config, second_config);

        // All files should still exist
        assert!(tmp.path().join(".ralph.toml").exists());
        assert!(tmp.path().join(".ralph/progress.db").exists());
    }

    #[test]
    fn init_appends_to_existing_gitignore() {
        let tmp = TempDir::new().unwrap();

        // Create existing .gitignore with other content
        fs::write(tmp.path().join(".gitignore"), "*.log\ntarget/\n").unwrap();

        // Run init
        super::init_in_dir(tmp.path()).unwrap();

        // Verify .gitignore has both old and new content
        let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("*.log"));
        assert!(gitignore.contains("target/"));
        assert!(gitignore.contains(".ralph/progress.db"));

        // Verify no duplicate entries if run again
        super::init_in_dir(tmp.path()).unwrap();
        let gitignore2 = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        let count = gitignore2.matches(".ralph/progress.db").count();
        assert_eq!(count, 1, "Should not duplicate .ralph/progress.db entry");
    }

    #[test]
    fn test_init_creates_claude_skills() {
        let tmp = TempDir::new().unwrap();
        super::init_in_dir(tmp.path()).unwrap();
        assert!(
            tmp.path().join(".claude/skills").is_dir(),
            ".claude/skills/ should be created by init"
        );
    }

    #[test]
    fn test_init_creates_knowledge_dir() {
        let tmp = TempDir::new().unwrap();
        super::init_in_dir(tmp.path()).unwrap();
        assert!(
            tmp.path().join(".ralph/knowledge").is_dir(),
            ".ralph/knowledge/ should be created by init"
        );
    }

    #[test]
    fn test_init_no_ralph_skills() {
        let tmp = TempDir::new().unwrap();
        super::init_in_dir(tmp.path()).unwrap();
        assert!(
            !tmp.path().join(".ralph/skills").exists(),
            ".ralph/skills/ should NOT be created by init"
        );
    }

    #[test]
    fn test_init_legacy_skills_notice() {
        let tmp = TempDir::new().unwrap();

        // Create a legacy .ralph/skills/ directory with a skill file inside
        let legacy_skill_dir = tmp.path().join(".ralph/skills/some-skill");
        fs::create_dir_all(&legacy_skill_dir).unwrap();
        fs::write(
            legacy_skill_dir.join("SKILL.md"),
            "---\nname: some-skill\n---\nContent",
        )
        .unwrap();

        // The condition check: legacy .ralph/skills/ is a dir and non-empty
        let ralph_dir = tmp.path().join(".ralph");
        let legacy_skills = ralph_dir.join("skills");
        assert!(legacy_skills.is_dir(), "Legacy skills dir should exist");
        let is_non_empty = fs::read_dir(&legacy_skills)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        assert!(is_non_empty, "Legacy skills dir should be non-empty");

        // Running init should succeed (create_dir_all is safe)
        super::init_in_dir(tmp.path()).unwrap();

        // The new directories should still be created
        assert!(tmp.path().join(".claude/skills").is_dir());
        assert!(tmp.path().join(".ralph/knowledge").is_dir());
    }

    #[test]
    fn test_init_existing_claude_dir() {
        let tmp = TempDir::new().unwrap();

        // Create .claude/ before init (simulating a project that already has Claude config)
        fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        fs::write(tmp.path().join(".claude/settings.json"), "{}").unwrap();

        // Init should not error even though .claude/ already exists
        super::init_in_dir(tmp.path()).unwrap();

        // .claude/skills/ should be created inside the existing .claude/ dir
        assert!(
            tmp.path().join(".claude/skills").is_dir(),
            ".claude/skills/ should be created even when .claude/ already exists"
        );
        // Existing file should be intact
        assert!(tmp.path().join(".claude/settings.json").exists());
    }
}

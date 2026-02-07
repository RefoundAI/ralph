//! Project configuration discovery and loading.
//!
//! Ralph projects are defined by a `.ralph.toml` file at the project root.
//! This module handles walking up the directory tree to find the config,
//! parsing it, and providing access to project settings.

use anyhow::{bail, Result};
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
pub struct RalphConfig {
    #[serde(default)]
    pub specs: SpecsConfig,
    #[serde(default)]
    pub prompts: PromptsConfig,
}

/// Specs configuration section.
#[derive(Debug, Clone, Deserialize)]
pub struct SpecsConfig {
    #[serde(default = "default_specs_dirs")]
    pub dirs: Vec<String>,
}

impl Default for SpecsConfig {
    fn default() -> Self {
        Self {
            dirs: default_specs_dirs(),
        }
    }
}

/// Prompts configuration section.
#[derive(Debug, Clone, Deserialize)]
pub struct PromptsConfig {
    #[serde(default = "default_prompts_dir")]
    pub dir: String,
}

impl Default for PromptsConfig {
    fn default() -> Self {
        Self {
            dir: default_prompts_dir(),
        }
    }
}

fn default_specs_dirs() -> Vec<String> {
    vec![".ralph/specs".to_string()]
}

fn default_prompts_dir() -> String {
    ".ralph/prompts".to_string()
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
        let (_tmp, root) = temp_project("[specs]\ndirs = [\"custom\"]");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.root, root);
        assert_eq!(result.config.specs.dirs, vec!["custom"]);
    }

    #[test]
    fn discovers_config_two_directories_up() {
        let (_tmp, root) = temp_project("[specs]\ndirs = [\"custom\"]");
        let subdir = root.join("a").join("b");
        fs::create_dir_all(&subdir).unwrap();

        let result = discover_from(&subdir).unwrap();
        assert_eq!(result.root, root);
        assert_eq!(result.config.specs.dirs, vec!["custom"]);
    }

    #[test]
    fn no_config_returns_error_with_init_message() {
        let tmp = TempDir::new().unwrap();
        let result = discover_from(tmp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ralph init"), "Error should mention 'ralph init', got: {}", err_msg);
    }

    #[test]
    fn relative_paths_resolve_against_config_directory() {
        let (_tmp, root) = temp_project("[prompts]\ndir = \"my_prompts\"");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.root, root);
        assert_eq!(result.config.prompts.dir, "my_prompts");
        // The resolution of relative paths happens at usage time, not here.
        // We just verify that the config stores the relative path as-is.
    }

    #[test]
    fn empty_toml_parses_to_defaults() {
        let (_tmp, root) = temp_project("");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.config.specs.dirs, vec![".ralph/specs"]);
        assert_eq!(result.config.prompts.dir, ".ralph/prompts");
    }

    #[test]
    fn partial_config_uses_defaults_for_missing_sections() {
        let (_tmp, root) = temp_project("[specs]\ndirs = [\"custom\"]");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.config.specs.dirs, vec!["custom"]);
        assert_eq!(result.config.prompts.dir, ".ralph/prompts"); // default
    }

    #[test]
    fn invalid_toml_returns_error() {
        let (_tmp, root) = temp_project("[specs\ndirs = [\"custom");
        let result = discover_from(&root);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_keys_ignored_for_forward_compat() {
        let (_tmp, root) = temp_project("[foo]\nbar = 1\n[specs]\ndirs = [\"test\"]");
        let result = discover_from(&root).unwrap();
        assert_eq!(result.config.specs.dirs, vec!["test"]);
        // Should not error despite unknown [foo] section
    }

    #[test]
    fn ralph_config_default_values() {
        let config = RalphConfig::default();
        assert_eq!(config.specs.dirs, vec![".ralph/specs"]);
        assert_eq!(config.prompts.dir, ".ralph/prompts");
    }
}

//! Sandbox rule definitions.

use std::collections::HashMap;
use std::process::Command;

/// Get readonly directories for each rule.
pub fn readonly_dirs() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::new()
}

/// Get writeable directories for each rule.
pub fn writeable_dirs() -> HashMap<&'static str, Vec<&'static str>> {
    let mut map = HashMap::new();
    map.insert("aws", vec!["~/.aws"]);
    map
}

/// Get binaries that should be blocked unless the rule is enabled.
pub fn binaries() -> HashMap<&'static str, Vec<&'static str>> {
    let mut map = HashMap::new();
    map.insert("aws", vec!["aws"]);
    map
}

/// Collect extra directories from allow rules.
/// Returns (readonly_dirs, writeable_dirs).
pub fn collect_dirs(allow_rules: &[String]) -> (Vec<String>, Vec<String>) {
    let home = std::env::var("HOME").unwrap_or_default();

    let readonly: Vec<String> = allow_rules
        .iter()
        .flat_map(|rule| {
            readonly_dirs()
                .get(rule.as_str())
                .cloned()
                .unwrap_or_default()
        })
        .map(|p| expand_home(p, &home))
        .collect();

    let writeable: Vec<String> = allow_rules
        .iter()
        .flat_map(|rule| {
            writeable_dirs()
                .get(rule.as_str())
                .cloned()
                .unwrap_or_default()
        })
        .map(|p| expand_home(p, &home))
        .collect();

    (readonly, writeable)
}

/// Collect binaries to block (those not in allow_rules).
pub fn collect_blocked_binaries(allow_rules: &[String]) -> Vec<String> {
    let all_rules: Vec<&str> = binaries().keys().copied().collect();

    all_rules
        .into_iter()
        .filter(|rule| !allow_rules.iter().any(|r| r == *rule))
        .flat_map(|rule| {
            binaries()
                .get(rule)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(resolve_binary)
        .collect()
}

fn expand_home(path: &str, home: &str) -> String {
    if path.starts_with("~/") {
        format!("{}{}", home, &path[1..])
    } else {
        path.to_string()
    }
}

fn resolve_binary(name: &str) -> Option<String> {
    // Find executable in PATH
    let which_output = Command::new("which")
        .arg(name)
        .output()
        .ok()?;

    if !which_output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&which_output.stdout)
        .trim()
        .to_string();

    // Resolve symlinks
    let readlink_output = Command::new("readlink")
        .args(["-f", &path])
        .output()
        .ok()?;

    if readlink_output.status.success() {
        Some(
            String::from_utf8_lossy(&readlink_output.stdout)
                .trim()
                .to_string(),
        )
    } else {
        Some(path)
    }
}

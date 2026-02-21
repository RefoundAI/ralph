//! Generate macOS sandbox-exec profiles.

use super::rules;

/// Base sandbox profile, embedded from resources/sandbox.sb at compile time.
const BASE_PROFILE: &str = include_str!("../../resources/sandbox.sb");

/// Generate a complete sandbox profile for the given allow rules.
#[allow(dead_code)]
pub fn generate(allow_rules: &[String]) -> String {
    let (readonly_dirs, writeable_dirs) = rules::collect_dirs(allow_rules);
    let blocked_binaries = rules::collect_blocked_binaries(allow_rules);

    let mut profile = BASE_PROFILE.to_string();
    profile.push_str(&generate_readonly_rules(&readonly_dirs));
    profile.push_str(&generate_writeable_rules(&writeable_dirs));
    profile.push_str(&generate_blocked_binary_rules(&blocked_binaries));

    profile
}

fn generate_readonly_rules(dirs: &[String]) -> String {
    if dirs.is_empty() {
        return String::new();
    }

    let rules: Vec<String> = dirs
        .iter()
        .map(|d| format!("(allow file-read* (subpath \"{}\"))", d))
        .collect();

    format!("\n;; Extra read rules from --allow\n{}\n", rules.join("\n"))
}

fn generate_writeable_rules(dirs: &[String]) -> String {
    if dirs.is_empty() {
        return String::new();
    }

    let rules: Vec<String> = dirs
        .iter()
        .map(|d| format!("(allow file-write* (subpath \"{}\"))", d))
        .collect();

    format!(
        "\n;; Extra write rules from --allow\n{}\n",
        rules.join("\n")
    )
}

fn generate_blocked_binary_rules(binaries: &[String]) -> String {
    if binaries.is_empty() {
        return String::new();
    }

    let rules: Vec<String> = binaries
        .iter()
        .map(|b| format!("(deny process-exec* (literal \"{}\"))", b))
        .collect();

    format!("\n;; Blocked binaries\n{}\n", rules.join("\n"))
}

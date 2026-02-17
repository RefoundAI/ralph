//! Knowledge base module: discovery, tag matching, deduplication, and file I/O.
//!
//! Knowledge entries are tagged markdown files in `.ralph/knowledge/` with YAML frontmatter.
//! Claude creates entries via the `<knowledge>` sigil; Ralph writes them to disk and
//! surfaces relevant ones each iteration via tag-based scoring.

use crate::claude::events::KnowledgeSigil;
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A single knowledge base entry parsed from a `.ralph/knowledge/*.md` file.
#[derive(Debug, Clone)]
pub struct KnowledgeEntry {
    pub title: String,
    pub tags: Vec<String>,
    pub feature: Option<String>,
    pub body: String,
    pub created_at: String,
    /// Resolved path of the file in `.ralph/knowledge/`.
    pub file_path: PathBuf,
}

/// Convert a title string to a URL-safe slug.
///
/// - Lowercase all characters
/// - Replace non-alphanumeric characters with hyphens
/// - Collapse consecutive hyphens into one
/// - Trim leading and trailing hyphens
/// - Truncate to 80 characters (without trailing hyphens)
pub fn slugify_title(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse consecutive hyphens
    let mut result = String::new();
    let mut prev_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    // Trim leading/trailing hyphens
    let trimmed = result.trim_matches('-').to_string();

    // Truncate to 80 characters without trailing hyphen
    if trimmed.len() > 80 {
        trimmed[..80].trim_end_matches('-').to_string()
    } else {
        trimmed
    }
}

/// Parse YAML frontmatter from a knowledge `.md` file.
///
/// Extracts `title`, `tags` (YAML array format: `[tag1, tag2]`), `feature`,
/// and `created_at` from the frontmatter block delimited by `---`.
///
/// Returns `None` if:
/// - No frontmatter block found (doesn't start with `---`)
/// - The `title` field is missing
/// - The `tags` field is missing or empty
///
/// The `file_path` field of the returned entry is set to an empty `PathBuf`;
/// callers should set it after calling this function.
fn parse_knowledge_frontmatter(content: &str) -> Option<KnowledgeEntry> {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        return None;
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let end_idx = after_first.find("---")?;
    let frontmatter = &after_first[..end_idx];
    let body = after_first[end_idx + 3..].trim().to_string();

    let mut title: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut feature: Option<String> = None;
    let mut created_at: Option<String> = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("title:") {
            title = Some(val.trim().trim_matches('"').to_string());
        } else if let Some(val) = line.strip_prefix("tags:") {
            // Parse [tag1, tag2, tag3] format
            let val = val.trim().trim_start_matches('[').trim_end_matches(']');
            tags = val
                .split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect();
        } else if let Some(val) = line.strip_prefix("feature:") {
            feature = Some(val.trim().trim_matches('"').to_string());
        } else if let Some(val) = line.strip_prefix("created_at:") {
            created_at = Some(val.trim().trim_matches('"').to_string());
        }
    }

    let title = title?;
    if tags.is_empty() {
        return None;
    }

    Some(KnowledgeEntry {
        title,
        tags,
        feature,
        body,
        created_at: created_at.unwrap_or_default(),
        file_path: PathBuf::new(), // caller fills this in
    })
}

/// Discover all knowledge entries from `.ralph/knowledge/*.md`.
///
/// Reads all `.md` files in the knowledge directory, parses frontmatter for each,
/// and skips files with malformed or missing frontmatter. Returns all valid entries
/// with `file_path` set to the resolved file path.
pub fn discover_knowledge(project_root: &Path) -> Vec<KnowledgeEntry> {
    let kb_dir = project_root.join(".ralph/knowledge");
    let mut entries = Vec::new();

    let dir_entries = match std::fs::read_dir(&kb_dir) {
        Ok(e) => e,
        Err(_) => return entries,
    };

    for entry in dir_entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(mut kb_entry) = parse_knowledge_frontmatter(&content) {
                kb_entry.file_path = path;
                entries.push(kb_entry);
            }
        }
    }

    entries
}

/// Score and filter knowledge entries by tag relevance to the current context.
///
/// Scoring per FR-6.3:
/// - +2 for each tag matching a word in the task title or description (lowercased)
/// - +2 for each tag matching the current feature name (lowercased)
/// - +1 for each tag matching a word in any file path from the last journal entry
///   (file paths are split on `/`, `.`, `-`, `_`; words must be > 2 chars)
///
/// Returns entries with score > 0, sorted by score descending.
pub fn match_knowledge_entries(
    entries: &[KnowledgeEntry],
    task_title: &str,
    task_description: &str,
    feature_name: Option<&str>,
    recent_files: &[String],
) -> Vec<(KnowledgeEntry, u32)> {
    // Build word set from task title + description (lowercased)
    let context_words: HashSet<String> = format!("{} {}", task_title, task_description)
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect();

    // Build word set from file paths (split on /, ., -, _; filter words > 2 chars)
    let file_words: HashSet<String> = recent_files
        .iter()
        .flat_map(|p| p.split(&['/', '.', '-', '_'][..]))
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 2)
        .collect();

    let feature_lower = feature_name.map(|f| f.to_lowercase());

    let mut scored: Vec<(KnowledgeEntry, u32)> = entries
        .iter()
        .map(|entry| {
            let mut score: u32 = 0;
            for tag in &entry.tags {
                if context_words.contains(tag) {
                    score += 2;
                }
                if let Some(ref feat) = feature_lower {
                    if tag == feat {
                        score += 2;
                    }
                }
                if file_words.contains(tag) {
                    score += 1;
                }
            }
            (entry.clone(), score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored
}

/// Write a knowledge entry to disk, handling deduplication.
///
/// Deduplication logic (FR-3.4):
/// - Exact title match → replace existing file content.
/// - >50% tag overlap AND substring title match → update existing file with merged tags.
/// - Otherwise → create new file at `{slug}.md`.
///
/// The body is truncated to 500 words (FR-3.5).
/// Returns an error if `sigil.tags` is empty (FR-3.6).
pub fn write_knowledge_entry(
    project_root: &Path,
    sigil: &KnowledgeSigil,
    feature: Option<&str>,
) -> Result<PathBuf> {
    // FR-3.6: at least one tag required
    if sigil.tags.is_empty() {
        anyhow::bail!("Knowledge entry '{}' has no tags", sigil.title);
    }

    // FR-3.5: truncate body to 500 words
    let body = truncate_to_words(&sigil.body, 500);

    let kb_dir = project_root.join(".ralph/knowledge");
    std::fs::create_dir_all(&kb_dir)?;

    let slug = slugify_title(&sigil.title);
    let default_path = kb_dir.join(format!("{}.md", slug));

    // FR-3.4: deduplication check
    let final_path = find_dedup_target(&kb_dir, &sigil.title, &sigil.tags)
        .unwrap_or(default_path);

    // Merge tags if updating an existing file
    let merged_tags = if final_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&final_path) {
            if let Some(existing) = parse_knowledge_frontmatter(&content) {
                merge_tags(&existing.tags, &sigil.tags)
            } else {
                sigil.tags.clone()
            }
        } else {
            sigil.tags.clone()
        }
    } else {
        sigil.tags.clone()
    };

    // Write the file with YAML frontmatter
    let now = chrono::Utc::now().to_rfc3339();
    let feature_line = feature
        .map(|f| format!("feature: \"{}\"\n", f))
        .unwrap_or_default();
    let content = format!(
        "---\ntitle: \"{}\"\ntags: [{}]\n{}created_at: \"{}\"\n---\n\n{}\n",
        sigil.title,
        merged_tags.join(", "),
        feature_line,
        now,
        body,
    );
    std::fs::write(&final_path, content)?;
    Ok(final_path)
}

/// Render knowledge entries as markdown for the system prompt (FR-6.5).
///
/// Enforces a 2000-token budget (estimated at 4 chars/token, FR-6.4).
/// Stops adding entries once the budget would be exceeded.
pub fn render_knowledge_context(entries: &[(KnowledgeEntry, u32)]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    const KNOWLEDGE_TOKEN_BUDGET: usize = 2000;
    let budget_chars = KNOWLEDGE_TOKEN_BUDGET * 4;
    let mut output = String::from("## Project Knowledge\n\n");
    let mut remaining = budget_chars;

    for (entry, _score) in entries {
        let rendered = format!(
            "### {}\n_Tags: {}_\n\n{}\n\n",
            entry.title,
            entry.tags.join(", "),
            entry.body,
        );
        if rendered.len() > remaining {
            break;
        }
        output.push_str(&rendered);
        remaining -= rendered.len();
    }
    output
}

// --- Private helpers ---

/// Find an existing knowledge file to update instead of creating a new one.
///
/// Returns `Some(path)` if:
/// - A file with the exact same title (case-insensitive) exists, OR
/// - A file with >50% tag overlap AND a substring title match exists.
fn find_dedup_target(kb_dir: &Path, title: &str, tags: &[String]) -> Option<PathBuf> {
    let title_lower = title.to_lowercase();

    let entries = std::fs::read_dir(kb_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(existing) = parse_knowledge_frontmatter(&content) {
                let existing_lower = existing.title.to_lowercase();

                // Exact title match
                if existing_lower == title_lower {
                    return Some(path);
                }

                // >50% tag overlap + substring title match
                let overlap = tag_overlap_ratio(&existing.tags, tags);
                let title_match = title_lower.contains(&existing_lower)
                    || existing_lower.contains(&title_lower);
                if overlap > 0.5 && title_match {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Compute the tag overlap ratio between two tag lists.
///
/// Returns the fraction of the smaller set that intersects with the larger set.
/// Returns 0.0 if either set is empty.
fn tag_overlap_ratio(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: HashSet<&String> = a.iter().collect();
    let set_b: HashSet<&String> = b.iter().collect();
    let intersection = set_a.intersection(&set_b).count();
    let min_len = set_a.len().min(set_b.len());
    intersection as f64 / min_len as f64
}

/// Truncate body text to `max_words` words. Appends `[truncated]` if truncated.
fn truncate_to_words(text: &str, max_words: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= max_words {
        text.to_string()
    } else {
        format!("{} [truncated]", words[..max_words].join(" "))
    }
}

/// Merge two tag lists, preserving order and deduplicating.
///
/// Tags from `existing` come first; new tags not already present are appended.
fn merge_tags(existing: &[String], new: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = existing.to_vec();
    for tag in new {
        if !merged.contains(tag) {
            merged.push(tag.clone());
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // --- slugify_title tests ---

    #[test]
    fn test_slugify_title_basic() {
        assert_eq!(slugify_title("Hello World"), "hello-world");
        assert_eq!(slugify_title("Rust Testing"), "rust-testing");
    }

    #[test]
    fn test_slugify_title_special_chars() {
        assert_eq!(
            slugify_title("Rust's testing: a guide!"),
            "rust-s-testing-a-guide"
        );
    }

    #[test]
    fn test_slugify_title_consecutive_hyphens() {
        assert_eq!(slugify_title("Foo--Bar"), "foo-bar");
        assert_eq!(slugify_title("A  B"), "a-b");
    }

    #[test]
    fn test_slugify_title_leading_trailing_hyphens() {
        assert_eq!(slugify_title("!Hello!"), "hello");
        assert_eq!(slugify_title("  spaces  "), "spaces");
    }

    #[test]
    fn test_slugify_title_long() {
        // 100-char title should be truncated to 80 chars without trailing hyphen
        let long_title = "a".repeat(85) + " extra"; // 92 chars
        let slug = slugify_title(&long_title);
        assert!(slug.len() <= 80, "slug should be at most 80 chars, got {}", slug.len());
        assert!(!slug.ends_with('-'), "slug should not end with hyphen");
    }

    #[test]
    fn test_slugify_title_exactly_80_chars() {
        // A title that produces exactly 80 chars in slug form
        let title = "a".repeat(80);
        let slug = slugify_title(&title);
        assert_eq!(slug.len(), 80);
    }

    // --- parse_knowledge_frontmatter tests ---

    #[test]
    fn test_parse_knowledge_frontmatter() {
        let content = r#"---
title: "Cargo bench requires nightly toolchain"
tags: [testing, cargo, nightly]
feature: "improved-memory"
created_at: "2026-02-17T12:00:00Z"
---

Running `cargo bench` requires the nightly Rust toolchain.
"#;
        let entry = parse_knowledge_frontmatter(content).expect("should parse valid frontmatter");
        assert_eq!(entry.title, "Cargo bench requires nightly toolchain");
        assert_eq!(entry.tags, vec!["testing", "cargo", "nightly"]);
        assert_eq!(entry.feature.as_deref(), Some("improved-memory"));
        assert_eq!(entry.created_at, "2026-02-17T12:00:00Z");
        assert!(entry.body.contains("cargo bench"));
    }

    #[test]
    fn test_parse_knowledge_frontmatter_no_tags() {
        let content = r#"---
title: "No tags here"
created_at: "2026-02-17T12:00:00Z"
---

Body content.
"#;
        assert!(
            parse_knowledge_frontmatter(content).is_none(),
            "should return None when tags are missing"
        );
    }

    #[test]
    fn test_parse_knowledge_frontmatter_empty_tags() {
        let content = r#"---
title: "Empty tags"
tags: []
---

Body content.
"#;
        assert!(
            parse_knowledge_frontmatter(content).is_none(),
            "should return None when tags array is empty"
        );
    }

    #[test]
    fn test_parse_knowledge_frontmatter_no_frontmatter() {
        let content = "Just a regular markdown file without frontmatter.";
        assert!(
            parse_knowledge_frontmatter(content).is_none(),
            "should return None when no frontmatter block"
        );
    }

    #[test]
    fn test_parse_knowledge_frontmatter_no_title() {
        let content = r#"---
tags: [testing, cargo]
created_at: "2026-02-17T12:00:00Z"
---

Body content.
"#;
        assert!(
            parse_knowledge_frontmatter(content).is_none(),
            "should return None when title is missing"
        );
    }

    #[test]
    fn test_parse_knowledge_frontmatter_optional_feature() {
        // Feature field is optional
        let content = r#"---
title: "No feature field"
tags: [rust, testing]
created_at: "2026-02-17T12:00:00Z"
---

Body content.
"#;
        let entry = parse_knowledge_frontmatter(content).expect("should parse");
        assert!(entry.feature.is_none());
    }

    // --- discover_knowledge tests ---

    fn make_valid_md(title: &str, tags: &[&str], body: &str) -> String {
        let tags_str = tags.join(", ");
        format!(
            "---\ntitle: \"{}\"\ntags: [{}]\ncreated_at: \"2026-02-17T12:00:00Z\"\n---\n\n{}\n",
            title, tags_str, body
        )
    }

    #[test]
    fn test_discover_knowledge_empty() {
        let temp = TempDir::new().unwrap();
        // Create the .ralph/knowledge directory but leave it empty
        let kb_dir = temp.path().join(".ralph/knowledge");
        fs::create_dir_all(&kb_dir).unwrap();

        let entries = discover_knowledge(temp.path());
        assert!(entries.is_empty(), "empty knowledge dir should return empty vec");
    }

    #[test]
    fn test_discover_knowledge_missing_dir() {
        let temp = TempDir::new().unwrap();
        // Don't create the knowledge dir at all
        let entries = discover_knowledge(temp.path());
        assert!(entries.is_empty(), "missing knowledge dir should return empty vec");
    }

    #[test]
    fn test_discover_knowledge_parses_files() {
        let temp = TempDir::new().unwrap();
        let kb_dir = temp.path().join(".ralph/knowledge");
        fs::create_dir_all(&kb_dir).unwrap();

        // Write two valid knowledge files
        fs::write(
            kb_dir.join("entry-one.md"),
            make_valid_md("Entry One", &["rust", "testing"], "First entry body."),
        )
        .unwrap();
        fs::write(
            kb_dir.join("entry-two.md"),
            make_valid_md("Entry Two", &["database", "sqlite"], "Second entry body."),
        )
        .unwrap();

        let entries = discover_knowledge(temp.path());
        assert_eq!(entries.len(), 2, "should discover both valid entries");

        // Check that file_path is set correctly
        for e in &entries {
            assert!(e.file_path.exists(), "file_path should point to an existing file");
        }

        // Check titles are parsed
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"Entry One"));
        assert!(titles.contains(&"Entry Two"));
    }

    #[test]
    fn test_discover_knowledge_skips_malformed() {
        let temp = TempDir::new().unwrap();
        let kb_dir = temp.path().join(".ralph/knowledge");
        fs::create_dir_all(&kb_dir).unwrap();

        // Write one valid and one malformed file
        fs::write(
            kb_dir.join("valid.md"),
            make_valid_md("Valid Entry", &["rust"], "Valid body."),
        )
        .unwrap();
        fs::write(
            kb_dir.join("malformed.md"),
            "Just plain text, no frontmatter at all.",
        )
        .unwrap();

        let entries = discover_knowledge(temp.path());
        assert_eq!(entries.len(), 1, "should skip malformed file and return 1 entry");
        assert_eq!(entries[0].title, "Valid Entry");
    }

    #[test]
    fn test_discover_knowledge_skips_non_md_files() {
        let temp = TempDir::new().unwrap();
        let kb_dir = temp.path().join(".ralph/knowledge");
        fs::create_dir_all(&kb_dir).unwrap();

        fs::write(
            kb_dir.join("valid.md"),
            make_valid_md("Valid Entry", &["rust"], "Body."),
        )
        .unwrap();
        fs::write(kb_dir.join("notes.txt"), "Not a markdown file").unwrap();

        let entries = discover_knowledge(temp.path());
        assert_eq!(entries.len(), 1);
    }

    // --- match_knowledge_entries tests ---

    fn make_entry(title: &str, tags: &[&str]) -> KnowledgeEntry {
        KnowledgeEntry {
            title: title.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            feature: None,
            body: "Test body.".to_string(),
            created_at: "2026-02-17T12:00:00Z".to_string(),
            file_path: PathBuf::new(),
        }
    }

    #[test]
    fn test_match_knowledge_entries_by_tags() {
        let entries = vec![
            make_entry("Rust Testing Guide", &["rust", "testing"]),
            make_entry("Python Patterns", &["python", "patterns"]),
        ];

        let matched = match_knowledge_entries(
            &entries,
            "rust implementation",
            "implement rust features",
            None,
            &[],
        );

        // "rust" tag matches "rust" in title and description -> score 2 (title) + 2 (desc) ... actually:
        // context_words = {"rust", "implementation", "implement", "features"}
        // "rust" in context_words -> +2
        // "testing" not in context_words
        // score = 2 for "Rust Testing Guide"
        assert!(!matched.is_empty(), "should find entries matching 'rust' tag");
        assert_eq!(matched[0].0.title, "Rust Testing Guide");
        assert!(matched[0].1 >= 2, "score should be at least 2");

        // Python entry should not match
        let python_matched: Vec<_> = matched
            .iter()
            .filter(|(e, _)| e.title.contains("Python"))
            .collect();
        assert!(python_matched.is_empty(), "Python entry should not match");
    }

    #[test]
    fn test_match_knowledge_feature_bonus() {
        let entries = vec![
            make_entry("Memory System", &["improved-memory", "knowledge"]),
            make_entry("Unrelated Entry", &["css", "frontend"]),
        ];

        let matched = match_knowledge_entries(
            &entries,
            "some task",
            "some description",
            Some("improved-memory"),
            &[],
        );

        // "improved-memory" tag matches feature name -> +2
        assert!(!matched.is_empty());
        assert_eq!(matched[0].0.title, "Memory System");
        assert!(matched[0].1 >= 2, "feature bonus should give score >= 2");
    }

    #[test]
    fn test_match_knowledge_file_path_bonus() {
        let entries = vec![
            make_entry("Config Patterns", &["config"]),
            make_entry("Unrelated", &["python"]),
        ];

        let recent_files = vec!["src/config.rs".to_string(), "src/main.rs".to_string()];

        let matched = match_knowledge_entries(
            &entries,
            "update settings",
            "modify configuration",
            None,
            &recent_files,
        );

        // "config" tag: matches "configuration" in description? No, it's an exact word match.
        // context_words = {"update", "settings", "modify", "configuration"}
        // "config" is NOT in context_words (it's not an exact match)
        // file_words from "src/config.rs": split on / . _ - -> ["src", "config", "rs"]
        //   filter > 2 chars: ["src", "config"] (rs is 2 chars, filtered)
        // "config" IS in file_words -> +1
        assert!(!matched.is_empty(), "should match via file path");
        assert_eq!(matched[0].0.title, "Config Patterns");
        assert_eq!(matched[0].1, 1, "file path match should give score 1");
    }

    #[test]
    fn test_match_knowledge_no_match() {
        let entries = vec![
            make_entry("Python Patterns", &["python", "patterns"]),
            make_entry("CSS Tricks", &["css", "frontend"]),
        ];

        let matched = match_knowledge_entries(
            &entries,
            "rust implementation",
            "implement rust features",
            Some("rust-feature"),
            &[],
        );

        // None of the tags (python, patterns, css, frontend) match "rust" context
        // feature name "rust-feature" won't match "python" or "css"
        assert!(matched.is_empty(), "no entries should match rust context");
    }

    #[test]
    fn test_match_knowledge_multiple_tags_accumulate() {
        let entries = vec![make_entry("Rust DB", &["rust", "database", "sqlite"])];

        let matched = match_knowledge_entries(
            &entries,
            "rust database implementation",
            "sqlite rust storage",
            None,
            &[],
        );

        // context_words = {"rust", "database", "implementation", "sqlite", "storage"}
        // "rust" -> +2, "database" -> +2, "sqlite" -> +2 -> total score 6
        assert!(!matched.is_empty());
        assert!(
            matched[0].1 >= 4,
            "multiple matching tags should accumulate, got {}",
            matched[0].1
        );
    }

    #[test]
    fn test_match_knowledge_sorted_by_score() {
        let entries = vec![
            make_entry("Low Score", &["testing"]),
            make_entry("High Score", &["rust", "testing", "implementation"]),
        ];

        let matched = match_knowledge_entries(
            &entries,
            "rust implementation",
            "testing rust",
            None,
            &[],
        );

        assert_eq!(matched.len(), 2);
        // High Score should come first (higher score)
        assert!(
            matched[0].1 >= matched[1].1,
            "entries should be sorted by score descending"
        );
        assert_eq!(matched[0].0.title, "High Score");
    }

    // --- write_knowledge_entry tests ---

    fn make_sigil(title: &str, tags: &[&str], body: &str) -> KnowledgeSigil {
        KnowledgeSigil {
            title: title.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            body: body.to_string(),
        }
    }

    #[test]
    fn test_write_knowledge_entry_new() {
        let temp = TempDir::new().unwrap();
        let sigil = make_sigil("My New Entry", &["rust", "testing"], "Some body content here.");

        let path = write_knowledge_entry(temp.path(), &sigil, None).unwrap();
        assert!(path.exists(), "file should be created");

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("title: \"My New Entry\""));
        assert!(content.contains("tags: [rust, testing]"));
        assert!(content.contains("Some body content here."));
        assert!(!content.contains("feature:"), "no feature line when feature=None");

        // Filename should be slug of title
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(filename, "my-new-entry.md");
    }

    #[test]
    fn test_write_knowledge_entry_with_feature() {
        let temp = TempDir::new().unwrap();
        let sigil = make_sigil("Feature Entry", &["rust"], "Body content.");

        let path = write_knowledge_entry(temp.path(), &sigil, Some("my-feature")).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("feature: \"my-feature\""));
    }

    #[test]
    fn test_write_knowledge_entry_no_tags() {
        let temp = TempDir::new().unwrap();
        let sigil = KnowledgeSigil {
            title: "No Tags".to_string(),
            tags: vec![],
            body: "Body content.".to_string(),
        };

        let result = write_knowledge_entry(temp.path(), &sigil, None);
        assert!(result.is_err(), "should return error when no tags");
    }

    #[test]
    fn test_write_knowledge_entry_dedup_exact_title() {
        let temp = TempDir::new().unwrap();

        // Write first entry
        let sigil1 = make_sigil("My Entry", &["rust"], "First content.");
        let path1 = write_knowledge_entry(temp.path(), &sigil1, None).unwrap();

        // Write second entry with same title — should update, not create new file
        let sigil2 = make_sigil("My Entry", &["rust", "testing"], "Second content wins.");
        let path2 = write_knowledge_entry(temp.path(), &sigil2, None).unwrap();

        // Should be the same file
        assert_eq!(path1, path2, "dedup should reuse existing file");

        // Count total .md files — should be exactly 1
        let kb_dir = temp.path().join(".ralph/knowledge");
        let count = fs::read_dir(&kb_dir).unwrap().count();
        assert_eq!(count, 1, "should have exactly 1 file after dedup");

        // Content should be from the second write
        let content = fs::read_to_string(&path2).unwrap();
        assert!(content.contains("Second content wins."));
    }

    #[test]
    fn test_write_knowledge_entry_dedup_tag_overlap() {
        let temp = TempDir::new().unwrap();

        // Write initial entry
        let sigil1 = make_sigil("Rust Testing Patterns", &["rust", "testing", "patterns"], "First.");
        write_knowledge_entry(temp.path(), &sigil1, None).unwrap();

        // Write similar entry with >50% tag overlap and substring title
        let sigil2 = make_sigil("Rust Testing", &["rust", "testing", "new-tag"], "Updated content.");
        let path2 = write_knowledge_entry(temp.path(), &sigil2, None).unwrap();

        // Should update the existing file (tag overlap > 0.5, title match)
        let kb_dir = temp.path().join(".ralph/knowledge");
        let count = fs::read_dir(&kb_dir).unwrap().count();
        assert_eq!(count, 1, "should have exactly 1 file after dedup by tag overlap");

        // Tags should be merged
        let content = fs::read_to_string(&path2).unwrap();
        assert!(content.contains("rust"));
        assert!(content.contains("testing"));
        assert!(content.contains("patterns") || content.contains("new-tag"),
            "merged tags should contain both old and new tags");
    }

    #[test]
    fn test_write_knowledge_entry_truncation() {
        let temp = TempDir::new().unwrap();

        // Create body with 600 words
        let words: Vec<String> = (1..=600).map(|i| format!("word{}", i)).collect();
        let long_body = words.join(" ");

        let sigil = make_sigil("Long Entry", &["test"], &long_body);
        let path = write_knowledge_entry(temp.path(), &sigil, None).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("[truncated]"), "body > 500 words should be truncated");

        // Count the words in the body section (after frontmatter)
        let body_start = content.find("---\n\n").unwrap() + 5;
        let body_section = &content[body_start..];
        let word_count = body_section.split_whitespace().count();
        // 500 words + "[truncated]" = 501 items, but "[truncated]" is one token
        // The body has 500 words from the original + [truncated] marker
        assert!(
            word_count <= 502,
            "truncated body should have at most ~501 word tokens, got {}",
            word_count
        );
    }

    // --- render_knowledge_context tests ---

    #[test]
    fn test_render_knowledge_context() {
        let entries = vec![
            (
                KnowledgeEntry {
                    title: "Rust Testing".to_string(),
                    tags: vec!["rust".to_string(), "testing".to_string()],
                    feature: None,
                    body: "Use #[test] attribute.".to_string(),
                    created_at: "2026-02-17T12:00:00Z".to_string(),
                    file_path: PathBuf::new(),
                },
                4u32,
            ),
            (
                KnowledgeEntry {
                    title: "SQLite Patterns".to_string(),
                    tags: vec!["database".to_string(), "sqlite".to_string()],
                    feature: None,
                    body: "Use WAL mode for performance.".to_string(),
                    created_at: "2026-02-17T12:00:00Z".to_string(),
                    file_path: PathBuf::new(),
                },
                2u32,
            ),
        ];

        let rendered = render_knowledge_context(&entries);
        assert!(rendered.starts_with("## Project Knowledge\n\n"));
        assert!(rendered.contains("### Rust Testing"));
        assert!(rendered.contains("_Tags: rust, testing_"));
        assert!(rendered.contains("Use #[test] attribute."));
        assert!(rendered.contains("### SQLite Patterns"));
        assert!(rendered.contains("_Tags: database, sqlite_"));
    }

    #[test]
    fn test_render_knowledge_context_empty() {
        let rendered = render_knowledge_context(&[]);
        assert_eq!(rendered, "");
    }

    #[test]
    fn test_render_knowledge_context_budget() {
        // Create entries that collectively exceed the 2000-token (8000 char) budget
        let large_body = "x".repeat(2000); // 2000 chars each
        let mut entries = Vec::new();
        for i in 1..=10 {
            entries.push((
                KnowledgeEntry {
                    title: format!("Entry {}", i),
                    tags: vec!["test".to_string()],
                    feature: None,
                    body: large_body.clone(),
                    created_at: "2026-02-17T12:00:00Z".to_string(),
                    file_path: PathBuf::new(),
                },
                (10 - i + 1) as u32,
            ));
        }

        let rendered = render_knowledge_context(&entries);
        // Budget is 2000 tokens * 4 chars = 8000 chars
        // Each rendered entry is ~2050+ chars -> at most 3-4 entries fit
        assert!(
            rendered.len() <= 8000 + 200,
            "budget should cap output: got {} chars",
            rendered.len()
        );
        // Must have at least the header
        assert!(rendered.contains("## Project Knowledge"));
        // Should not contain all 10 entries
        assert!(
            !rendered.contains("### Entry 10"),
            "budget should prevent all entries from being included"
        );
    }
}

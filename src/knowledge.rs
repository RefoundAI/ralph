//! Knowledge base module: discovery, tag matching, deduplication, link graph, and file I/O.
//!
//! Knowledge entries are tagged markdown files in `.ralph/knowledge/` with YAML frontmatter.
//! Claude creates entries via the `<knowledge>` sigil; Ralph writes them to disk and
//! surfaces relevant ones each iteration via tag-based scoring.
//!
//! ## Roam-protocol bidirectional linking
//!
//! Entries can reference each other using `[[Title]]` syntax in their body text.
//! Ralph parses these links, builds a bidirectional graph, and uses it to pull in
//! related entries that weren't directly matched by tags but are linked from matched
//! entries. This enables zettelkasten-style densely linked atomic notes where agents
//! can incrementally build context by following links.

use crate::acp::types::KnowledgeSigil;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A single knowledge base entry parsed from a `.ralph/knowledge/*.md` file.
#[derive(Debug, Clone)]
pub struct KnowledgeEntry {
    pub title: String,
    pub tags: Vec<String>,
    #[allow(dead_code)]
    pub feature: Option<String>,
    pub body: String,
    #[allow(dead_code)]
    pub created_at: String,
    /// Resolved path of the file in `.ralph/knowledge/`.
    pub file_path: PathBuf,
}

/// Bidirectional link graph built from `[[Title]]` references in knowledge entry bodies.
///
/// For each entry title, tracks:
/// - `outlinks`: titles this entry links TO (via `[[Target]]` in its body)
/// - `backlinks`: titles that link TO this entry (via `[[This Entry]]` in their body)
#[derive(Debug, Clone, Default)]
pub struct LinkGraph {
    /// title → set of titles this entry links to
    pub outlinks: HashMap<String, HashSet<String>>,
    /// title → set of titles that link to this entry
    pub backlinks: HashMap<String, HashSet<String>>,
}

/// Extract all `[[Title]]` link references from a text body.
///
/// Returns a deduplicated list of referenced titles (preserving case from the link).
/// Handles nested brackets gracefully — `[[foo]]` extracts `foo`.
/// Empty links `[[]]` are skipped.
pub fn extract_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    let mut search_from = 0;
    let bytes = text.as_bytes();

    while search_from + 3 < bytes.len() {
        // Find [[
        let start = match text[search_from..].find("[[") {
            Some(idx) => search_from + idx,
            None => break,
        };
        let content_start = start + 2;

        // Find ]]
        let end = match text[content_start..].find("]]") {
            Some(idx) => content_start + idx,
            None => break,
        };

        let title = text[content_start..end].trim();
        if !title.is_empty() {
            let key = title.to_lowercase();
            if !seen.contains(&key) {
                seen.insert(key);
                links.push(title.to_string());
            }
        }

        search_from = end + 2;
    }

    links
}

/// Build a bidirectional link graph from a set of knowledge entries.
///
/// Scans each entry's body for `[[Title]]` references and populates both
/// outlinks (what this entry links to) and backlinks (what links to this entry).
/// Title matching is case-insensitive.
pub fn build_link_graph(entries: &[KnowledgeEntry]) -> LinkGraph {
    // Build a lookup of lowercase title → canonical title
    let canonical: HashMap<String, String> = entries
        .iter()
        .map(|e| (e.title.to_lowercase(), e.title.clone()))
        .collect();

    let mut graph = LinkGraph::default();

    for entry in entries {
        let links = extract_links(&entry.body);
        let source_lower = entry.title.to_lowercase();

        for link_title in &links {
            let target_lower = link_title.to_lowercase();

            // Only track links to entries that actually exist
            if canonical.contains_key(&target_lower) {
                graph
                    .outlinks
                    .entry(source_lower.clone())
                    .or_default()
                    .insert(target_lower.clone());

                graph
                    .backlinks
                    .entry(target_lower)
                    .or_default()
                    .insert(source_lower.clone());
            }
        }
    }

    graph
}

/// Expand a set of matched entries by following links in the graph.
///
/// Starting from the initially matched entries, follows outlinks and backlinks
/// up to `max_hops` deep. Newly discovered entries get a score bonus that
/// decreases with distance: `base_bonus / hop_number`.
///
/// Returns additional entries (not already in `initial_matched`) with their
/// link-derived scores, sorted by score descending.
pub fn expand_via_links(
    all_entries: &[KnowledgeEntry],
    initial_matched: &[(KnowledgeEntry, u32)],
    graph: &LinkGraph,
    max_hops: u32,
    base_bonus: u32,
) -> Vec<(KnowledgeEntry, u32)> {
    // Build lookup: lowercase title → entry
    let entry_map: HashMap<String, &KnowledgeEntry> = all_entries
        .iter()
        .map(|e| (e.title.to_lowercase(), e))
        .collect();

    // Track which titles are already matched (don't re-add them)
    let mut visited: HashSet<String> = initial_matched
        .iter()
        .map(|(e, _)| e.title.to_lowercase())
        .collect();

    // BFS frontier: (lowercase_title, hop_number)
    let mut frontier: Vec<(String, u32)> = initial_matched
        .iter()
        .map(|(e, _)| (e.title.to_lowercase(), 0))
        .collect();

    let mut expanded: Vec<(KnowledgeEntry, u32)> = Vec::new();

    let mut idx = 0;
    while idx < frontier.len() {
        let (current, hop) = frontier[idx].clone();
        idx += 1;

        if hop >= max_hops {
            continue;
        }

        let next_hop = hop + 1;
        let bonus = base_bonus / next_hop;
        if bonus == 0 {
            continue;
        }

        // Collect neighbors (outlinks + backlinks)
        let mut neighbors: HashSet<String> = HashSet::new();
        if let Some(out) = graph.outlinks.get(&current) {
            neighbors.extend(out.iter().cloned());
        }
        if let Some(back) = graph.backlinks.get(&current) {
            neighbors.extend(back.iter().cloned());
        }

        for neighbor in neighbors {
            if visited.contains(&neighbor) {
                continue;
            }
            visited.insert(neighbor.clone());

            if let Some(entry) = entry_map.get(&neighbor) {
                expanded.push(((*entry).clone(), bonus));
                frontier.push((neighbor, next_hop));
            }
        }
    }

    expanded.sort_by(|a, b| b.1.cmp(&a.1));
    expanded
}

/// Get the backlinks for a specific entry title from the link graph.
///
/// Returns the titles of entries that link TO this entry (case-insensitive lookup).
pub fn get_backlinks(graph: &LinkGraph, title: &str) -> Vec<String> {
    let key = title.to_lowercase();
    graph
        .backlinks
        .get(&key)
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default()
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
    let final_path = find_dedup_target(&kb_dir, &sigil.title, &sigil.tags).unwrap_or(default_path);

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
/// Convenience wrapper around `render_knowledge_context_with_graph` without link data.
#[cfg(test)]
pub fn render_knowledge_context(entries: &[(KnowledgeEntry, u32)]) -> String {
    render_knowledge_context_with_graph(entries, None)
}

/// Render knowledge entries with optional link graph for backlink display.
pub fn render_knowledge_context_with_graph(
    entries: &[(KnowledgeEntry, u32)],
    graph: Option<&LinkGraph>,
) -> String {
    if entries.is_empty() {
        return String::new();
    }

    const KNOWLEDGE_TOKEN_BUDGET: usize = 2000;
    let budget_chars = KNOWLEDGE_TOKEN_BUDGET * 4;
    let mut output = String::from("## Project Knowledge\n\n");
    let mut remaining = budget_chars;

    for (entry, _score) in entries {
        let mut rendered = format!(
            "### {}\n_Tags: {}_\n\n{}\n",
            entry.title,
            entry.tags.join(", "),
            entry.body,
        );

        // Add backlinks if graph is provided
        if let Some(g) = graph {
            let backlinks = get_backlinks(g, &entry.title);
            if !backlinks.is_empty() {
                rendered.push_str(&format!("_Linked from: {}_\n", backlinks.join(", ")));
            }
            let outlinks_lower = entry.title.to_lowercase();
            if let Some(out) = g.outlinks.get(&outlinks_lower) {
                if !out.is_empty() {
                    let out_list: Vec<&str> = out.iter().map(|s| s.as_str()).collect();
                    rendered.push_str(&format!("_Links to: {}_\n", out_list.join(", ")));
                }
            }
        }

        rendered.push('\n');

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
                let title_match =
                    title_lower.contains(&existing_lower) || existing_lower.contains(&title_lower);
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
        assert!(
            slug.len() <= 80,
            "slug should be at most 80 chars, got {}",
            slug.len()
        );
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
        assert!(
            entries.is_empty(),
            "empty knowledge dir should return empty vec"
        );
    }

    #[test]
    fn test_discover_knowledge_missing_dir() {
        let temp = TempDir::new().unwrap();
        // Don't create the knowledge dir at all
        let entries = discover_knowledge(temp.path());
        assert!(
            entries.is_empty(),
            "missing knowledge dir should return empty vec"
        );
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
            assert!(
                e.file_path.exists(),
                "file_path should point to an existing file"
            );
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
        assert_eq!(
            entries.len(),
            1,
            "should skip malformed file and return 1 entry"
        );
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
        assert!(
            !matched.is_empty(),
            "should find entries matching 'rust' tag"
        );
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

        let matched =
            match_knowledge_entries(&entries, "rust implementation", "testing rust", None, &[]);

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
        let sigil = make_sigil(
            "My New Entry",
            &["rust", "testing"],
            "Some body content here.",
        );

        let path = write_knowledge_entry(temp.path(), &sigil, None).unwrap();
        assert!(path.exists(), "file should be created");

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("title: \"My New Entry\""));
        assert!(content.contains("tags: [rust, testing]"));
        assert!(content.contains("Some body content here."));
        assert!(
            !content.contains("feature:"),
            "no feature line when feature=None"
        );

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
        let sigil1 = make_sigil(
            "Rust Testing Patterns",
            &["rust", "testing", "patterns"],
            "First.",
        );
        write_knowledge_entry(temp.path(), &sigil1, None).unwrap();

        // Write similar entry with >50% tag overlap and substring title
        let sigil2 = make_sigil(
            "Rust Testing",
            &["rust", "testing", "new-tag"],
            "Updated content.",
        );
        let path2 = write_knowledge_entry(temp.path(), &sigil2, None).unwrap();

        // Should update the existing file (tag overlap > 0.5, title match)
        let kb_dir = temp.path().join(".ralph/knowledge");
        let count = fs::read_dir(&kb_dir).unwrap().count();
        assert_eq!(
            count, 1,
            "should have exactly 1 file after dedup by tag overlap"
        );

        // Tags should be merged
        let content = fs::read_to_string(&path2).unwrap();
        assert!(content.contains("rust"));
        assert!(content.contains("testing"));
        assert!(
            content.contains("patterns") || content.contains("new-tag"),
            "merged tags should contain both old and new tags"
        );
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
        assert!(
            content.contains("[truncated]"),
            "body > 500 words should be truncated"
        );

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

    // --- extract_links tests ---

    #[test]
    fn test_extract_links_basic() {
        let text = "See [[Rust Testing]] for more details.";
        let links = extract_links(text);
        assert_eq!(links, vec!["Rust Testing"]);
    }

    #[test]
    fn test_extract_links_multiple() {
        let text = "Relates to [[Foo]] and [[Bar]] and also [[Baz]].";
        let links = extract_links(text);
        assert_eq!(links, vec!["Foo", "Bar", "Baz"]);
    }

    #[test]
    fn test_extract_links_deduplicates() {
        let text = "See [[Foo]] and also [[Foo]] again.";
        let links = extract_links(text);
        assert_eq!(links, vec!["Foo"]);
    }

    #[test]
    fn test_extract_links_case_insensitive_dedup() {
        let text = "[[Foo Bar]] and [[foo bar]] should be one.";
        let links = extract_links(text);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "Foo Bar"); // first occurrence wins
    }

    #[test]
    fn test_extract_links_empty_link_skipped() {
        let text = "Empty: [[]] and valid: [[Real]].";
        let links = extract_links(text);
        assert_eq!(links, vec!["Real"]);
    }

    #[test]
    fn test_extract_links_no_links() {
        let text = "No links here, just plain text.";
        let links = extract_links(text);
        assert!(links.is_empty());
    }

    #[test]
    fn test_extract_links_unclosed() {
        let text = "Unclosed [[link here but no closing.";
        let links = extract_links(text);
        assert!(links.is_empty());
    }

    #[test]
    fn test_extract_links_whitespace_trimmed() {
        let text = "[[  Trimmed Title  ]] should work.";
        let links = extract_links(text);
        assert_eq!(links, vec!["Trimmed Title"]);
    }

    // --- build_link_graph tests ---

    fn make_entry_with_body(title: &str, tags: &[&str], body: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            title: title.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            feature: None,
            body: body.to_string(),
            created_at: "2026-02-17T12:00:00Z".to_string(),
            file_path: PathBuf::new(),
        }
    }

    #[test]
    fn test_build_link_graph_basic() {
        let entries = vec![
            make_entry_with_body("Entry A", &["rust"], "Links to [[Entry B]]."),
            make_entry_with_body("Entry B", &["testing"], "Standalone entry."),
        ];

        let graph = build_link_graph(&entries);

        // A -> B outlink
        let a_outlinks = graph.outlinks.get("entry a").unwrap();
        assert!(a_outlinks.contains("entry b"));

        // B has backlink from A
        let b_backlinks = graph.backlinks.get("entry b").unwrap();
        assert!(b_backlinks.contains("entry a"));

        // B has no outlinks
        assert!(graph.outlinks.get("entry b").is_none());
    }

    #[test]
    fn test_build_link_graph_bidirectional() {
        let entries = vec![
            make_entry_with_body("Entry A", &["rust"], "See [[Entry B]] for details."),
            make_entry_with_body("Entry B", &["testing"], "Related to [[Entry A]]."),
        ];

        let graph = build_link_graph(&entries);

        // A -> B
        assert!(graph.outlinks["entry a"].contains("entry b"));
        // B -> A
        assert!(graph.outlinks["entry b"].contains("entry a"));
        // A has backlink from B
        assert!(graph.backlinks["entry a"].contains("entry b"));
        // B has backlink from A
        assert!(graph.backlinks["entry b"].contains("entry a"));
    }

    #[test]
    fn test_build_link_graph_ignores_nonexistent_targets() {
        let entries = vec![make_entry_with_body(
            "Entry A",
            &["rust"],
            "Links to [[Nonexistent Entry]].",
        )];

        let graph = build_link_graph(&entries);

        // Should not have any outlinks (target doesn't exist)
        assert!(graph.outlinks.get("entry a").is_none());
    }

    #[test]
    fn test_build_link_graph_case_insensitive() {
        let entries = vec![
            make_entry_with_body("Entry A", &["rust"], "Links to [[entry b]]."),
            make_entry_with_body("Entry B", &["testing"], "Standalone."),
        ];

        let graph = build_link_graph(&entries);

        // Should match despite different case
        let a_outlinks = graph.outlinks.get("entry a").unwrap();
        assert!(a_outlinks.contains("entry b"));
    }

    // --- expand_via_links tests ---

    #[test]
    fn test_expand_via_links_one_hop() {
        let entries = vec![
            make_entry_with_body("Matched", &["rust"], "See [[Linked]]."),
            make_entry_with_body("Linked", &["testing"], "Connected to [[Matched]]."),
            make_entry_with_body("Unrelated", &["python"], "No connections."),
        ];

        let graph = build_link_graph(&entries);
        let initial = vec![(entries[0].clone(), 4)]; // Only "Matched" is initially matched

        let expanded = expand_via_links(&entries, &initial, &graph, 2, 2);

        // Should pull in "Linked" (1 hop) but not "Unrelated"
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].0.title, "Linked");
        assert_eq!(expanded[0].1, 2); // base_bonus=2, hop=1 → 2/1=2
    }

    #[test]
    fn test_expand_via_links_two_hops() {
        let entries = vec![
            make_entry_with_body("A", &["rust"], "Links to [[B]]."),
            make_entry_with_body("B", &["testing"], "Links to [[C]]."),
            make_entry_with_body("C", &["database"], "End of chain."),
        ];

        let graph = build_link_graph(&entries);
        let initial = vec![(entries[0].clone(), 4)]; // Only "A" matched

        let expanded = expand_via_links(&entries, &initial, &graph, 2, 4);

        // Should get B (hop 1, bonus=4/1=4) and C (hop 2, bonus=4/2=2)
        assert_eq!(expanded.len(), 2);
        let titles: Vec<&str> = expanded.iter().map(|e| e.0.title.as_str()).collect();
        assert!(titles.contains(&"B"));
        assert!(titles.contains(&"C"));

        // B should have higher score (closer)
        let b_score = expanded.iter().find(|e| e.0.title == "B").unwrap().1;
        let c_score = expanded.iter().find(|e| e.0.title == "C").unwrap().1;
        assert!(b_score > c_score, "closer entries should score higher");
    }

    #[test]
    fn test_expand_via_links_max_hops_respected() {
        let entries = vec![
            make_entry_with_body("A", &["rust"], "Links to [[B]]."),
            make_entry_with_body("B", &["testing"], "Links to [[C]]."),
            make_entry_with_body("C", &["database"], "Links to [[D]]."),
            make_entry_with_body("D", &["deep"], "Too far away."),
        ];

        let graph = build_link_graph(&entries);
        let initial = vec![(entries[0].clone(), 4)]; // Only "A" matched

        // max_hops=1: should only get B
        let expanded = expand_via_links(&entries, &initial, &graph, 1, 2);
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].0.title, "B");
    }

    #[test]
    fn test_expand_via_links_no_duplicates_with_initial() {
        let entries = vec![
            make_entry_with_body("A", &["rust"], "Links to [[B]]."),
            make_entry_with_body("B", &["testing"], "Links to [[A]]."),
        ];

        let graph = build_link_graph(&entries);
        // Both A and B are initially matched
        let initial = vec![(entries[0].clone(), 4), (entries[1].clone(), 2)];

        let expanded = expand_via_links(&entries, &initial, &graph, 2, 2);

        // Should not re-add either A or B since both are already in initial
        assert!(
            expanded.is_empty(),
            "should not duplicate already-matched entries"
        );
    }

    #[test]
    fn test_expand_via_links_empty_graph() {
        let entries = vec![
            make_entry_with_body("A", &["rust"], "No links."),
            make_entry_with_body("B", &["testing"], "Also no links."),
        ];

        let graph = build_link_graph(&entries);
        let initial = vec![(entries[0].clone(), 4)];

        let expanded = expand_via_links(&entries, &initial, &graph, 2, 2);
        assert!(expanded.is_empty());
    }

    // --- get_backlinks tests ---

    #[test]
    fn test_get_backlinks() {
        let entries = vec![
            make_entry_with_body("A", &["rust"], "Links to [[B]]."),
            make_entry_with_body("B", &["testing"], "Standalone."),
            make_entry_with_body("C", &["db"], "Also links to [[B]]."),
        ];

        let graph = build_link_graph(&entries);
        let backlinks = get_backlinks(&graph, "B");

        assert_eq!(backlinks.len(), 2);
        assert!(backlinks.contains(&"a".to_string()));
        assert!(backlinks.contains(&"c".to_string()));
    }

    #[test]
    fn test_get_backlinks_none() {
        let entries = vec![make_entry_with_body("A", &["rust"], "No incoming links.")];

        let graph = build_link_graph(&entries);
        let backlinks = get_backlinks(&graph, "A");
        assert!(backlinks.is_empty());
    }

    // --- render_knowledge_context_with_graph tests ---

    #[test]
    fn test_render_with_graph_shows_backlinks() {
        let entries = vec![
            make_entry_with_body("Entry A", &["rust"], "Links to [[Entry B]]."),
            make_entry_with_body("Entry B", &["testing"], "Standalone."),
        ];

        let graph = build_link_graph(&entries);
        let scored = vec![(entries[1].clone(), 4u32)]; // Only showing Entry B

        let rendered = render_knowledge_context_with_graph(&scored, Some(&graph));

        assert!(rendered.contains("### Entry B"));
        assert!(
            rendered.contains("Linked from:"),
            "should show backlinks for Entry B"
        );
        assert!(
            rendered.contains("entry a"),
            "backlink should reference entry a"
        );
    }

    #[test]
    fn test_render_with_graph_shows_outlinks() {
        let entries = vec![
            make_entry_with_body("Entry A", &["rust"], "Links to [[Entry B]]."),
            make_entry_with_body("Entry B", &["testing"], "Standalone."),
        ];

        let graph = build_link_graph(&entries);
        let scored = vec![(entries[0].clone(), 4u32)]; // Only showing Entry A

        let rendered = render_knowledge_context_with_graph(&scored, Some(&graph));

        assert!(rendered.contains("### Entry A"));
        assert!(
            rendered.contains("Links to:"),
            "should show outlinks for Entry A"
        );
        assert!(
            rendered.contains("entry b"),
            "outlink should reference entry b"
        );
    }

    #[test]
    fn test_render_without_graph_no_links() {
        let entries = vec![make_entry_with_body(
            "Entry A",
            &["rust"],
            "Links to [[Entry B]].",
        )];
        let scored = vec![(entries[0].clone(), 4u32)];

        let rendered = render_knowledge_context_with_graph(&scored, None);

        assert!(rendered.contains("### Entry A"));
        assert!(
            !rendered.contains("Linked from:"),
            "should not show link metadata without graph"
        );
        assert!(
            !rendered.contains("Links to:"),
            "should not show link metadata without graph"
        );
    }

    // --- Integration: discover + link graph from files ---

    #[test]
    fn test_discover_and_build_link_graph() {
        let temp = TempDir::new().unwrap();
        let kb_dir = temp.path().join(".ralph/knowledge");
        fs::create_dir_all(&kb_dir).unwrap();

        fs::write(
            kb_dir.join("migrations.md"),
            make_valid_md(
                "Schema Migrations",
                &["sqlite", "schema"],
                "Use version checks. See [[Task Columns]] for column mapping.",
            ),
        )
        .unwrap();
        fs::write(
            kb_dir.join("columns.md"),
            make_valid_md(
                "Task Columns",
                &["sqlite", "tasks"],
                "TASK_COLUMNS constant maps SQL to struct. Related: [[Schema Migrations]].",
            ),
        )
        .unwrap();
        fs::write(
            kb_dir.join("unrelated.md"),
            make_valid_md("Python Tips", &["python"], "Not connected to anything."),
        )
        .unwrap();

        let entries = discover_knowledge(temp.path());
        assert_eq!(entries.len(), 3);

        let graph = build_link_graph(&entries);

        // Schema Migrations -> Task Columns
        let migrations_out = graph.outlinks.get("schema migrations").unwrap();
        assert!(migrations_out.contains("task columns"));

        // Task Columns -> Schema Migrations
        let columns_out = graph.outlinks.get("task columns").unwrap();
        assert!(columns_out.contains("schema migrations"));

        // Both have backlinks from each other
        let migrations_back = graph.backlinks.get("schema migrations").unwrap();
        assert!(migrations_back.contains("task columns"));

        // Python Tips has no links
        assert!(graph.outlinks.get("python tips").is_none());
        assert!(graph.backlinks.get("python tips").is_none());
    }
}

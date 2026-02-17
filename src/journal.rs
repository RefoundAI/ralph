//! Journal module: persistent iteration records with FTS5 search.
//!
//! Each iteration of the agent loop writes a journal entry to SQLite.
//! The journal supports recency queries (per run_id) and FTS5 full-text search
//! over journal notes for cross-run context retrieval.

use crate::dag::Db;
use anyhow::Result;

/// A single journal entry recording metadata about one agent loop iteration.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub id: i64,
    pub run_id: String,
    pub iteration: u32,
    pub task_id: Option<String>,
    pub feature_id: Option<String>,
    pub outcome: String,
    pub model: Option<String>,
    pub duration_secs: f64,
    pub cost_usd: f64,
    pub files_modified: Vec<String>,
    pub notes: Option<String>,
    pub created_at: String,
}

/// Insert a journal entry into the database.
///
/// The `files_modified` field is serialized as a JSON array.
/// The FTS5 index is updated automatically by the `journal_ai` trigger.
/// Returns the `last_insert_rowid()` of the new row.
pub fn insert_journal_entry(db: &Db, entry: &JournalEntry) -> Result<i64> {
    let files_json = serde_json::to_string(&entry.files_modified)?;
    db.conn().execute(
        "INSERT INTO journal (run_id, iteration, task_id, feature_id, outcome,
         model, duration_secs, cost_usd, files_modified, notes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            entry.run_id,
            entry.iteration,
            entry.task_id,
            entry.feature_id,
            entry.outcome,
            entry.model,
            entry.duration_secs,
            entry.cost_usd,
            files_json,
            entry.notes,
            entry.created_at,
        ],
    )?;
    Ok(db.conn().last_insert_rowid())
}

/// Map a `rusqlite::Row` to a `JournalEntry`.
///
/// Expects columns in order:
/// id, run_id, iteration, task_id, feature_id, outcome,
/// model, duration_secs, cost_usd, files_modified, notes, created_at
fn journal_from_row(row: &rusqlite::Row) -> rusqlite::Result<JournalEntry> {
    let files_json: Option<String> = row.get(9)?;
    let files_modified: Vec<String> = files_json
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();

    Ok(JournalEntry {
        id: row.get(0)?,
        run_id: row.get(1)?,
        iteration: row.get::<_, u32>(2)?,
        task_id: row.get(3)?,
        feature_id: row.get(4)?,
        outcome: row.get(5)?,
        model: row.get(6)?,
        duration_secs: row.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
        cost_usd: row.get::<_, Option<f64>>(8)?.unwrap_or(0.0),
        files_modified,
        notes: row.get(10)?,
        created_at: row.get(11)?,
    })
}

/// Get the last N journal entries for a given `run_id`, in chronological order.
///
/// Queries in descending iteration order (most recent first) then reverses
/// so the result is oldest-first (chronological).
pub fn query_journal_recent(db: &Db, run_id: &str, limit: u32) -> Result<Vec<JournalEntry>> {
    let mut stmt = db.conn().prepare(
        "SELECT id, run_id, iteration, task_id, feature_id, outcome,
                model, duration_secs, cost_usd, files_modified, notes, created_at
         FROM journal
         WHERE run_id = ?1
         ORDER BY iteration DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![run_id, limit], journal_from_row)?;
    let mut entries: Vec<JournalEntry> = rows.filter_map(|r| r.ok()).collect();
    entries.reverse(); // chronological order (oldest first)
    Ok(entries)
}

/// FTS5 full-text search across journal notes.
///
/// Returns entries ranked by FTS5 relevance, excluding entries from `exclude_run_id`
/// (those come from `query_journal_recent`). Only entries with non-NULL notes are returned.
pub fn query_journal_fts(
    db: &Db,
    query: &str,
    exclude_run_id: &str,
    limit: u32,
) -> Result<Vec<JournalEntry>> {
    let fts_query = build_fts_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = db.conn().prepare(
        "SELECT j.id, j.run_id, j.iteration, j.task_id, j.feature_id, j.outcome,
                j.model, j.duration_secs, j.cost_usd, j.files_modified, j.notes, j.created_at
         FROM journal j
         JOIN journal_fts ON journal_fts.rowid = j.id
         WHERE journal_fts MATCH ?1
           AND j.run_id != ?2
           AND j.notes IS NOT NULL
         ORDER BY rank
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![fts_query, exclude_run_id, limit], journal_from_row)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Build an FTS5 query string from free text.
///
/// Splits on whitespace, filters short words (<= 2 chars), and caps at 10 words.
/// Returns an OR query: "word1 OR word2 OR word3".
pub fn build_fts_query(text: &str) -> String {
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .take(10)
        .collect();
    if words.is_empty() {
        return String::new();
    }
    words.join(" OR ")
}

/// Smart-select journal entries for system prompt injection.
///
/// Combines:
/// - Up to `recent_limit` entries from the current `run_id` (chronological)
/// - Up to `fts_limit` entries from other runs matching the task title/description via FTS5
pub fn select_journal_entries(
    db: &Db,
    run_id: &str,
    task_title: &str,
    task_description: &str,
    recent_limit: u32,
    fts_limit: u32,
) -> Result<Vec<JournalEntry>> {
    let mut entries = query_journal_recent(db, run_id, recent_limit)?;
    let query_text = format!("{} {}", task_title, task_description);
    let fts_entries = query_journal_fts(db, &query_text, run_id, fts_limit)?;
    entries.extend(fts_entries);
    Ok(entries)
}

const JOURNAL_TOKEN_BUDGET: usize = 3000;
const CHARS_PER_TOKEN: usize = 4;

/// Render journal entries as markdown for the system prompt.
///
/// Enforces a token budget (FR-5.3): stops adding entries once the budget
/// (estimated at 4 chars/token) would be exceeded.
pub fn render_journal_context(entries: &[JournalEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let budget_chars = JOURNAL_TOKEN_BUDGET * CHARS_PER_TOKEN;
    let mut output = String::from("## Run Journal\n\n");
    let mut remaining = budget_chars;

    for entry in entries {
        let rendered = render_single_entry(entry);
        if rendered.len() > remaining {
            break; // budget exhausted
        }
        output.push_str(&rendered);
        output.push('\n');
        remaining -= rendered.len();
    }
    output
}

/// Render a single journal entry as markdown (FR-5.4 format).
fn render_single_entry(entry: &JournalEntry) -> String {
    let files = if entry.files_modified.is_empty() {
        "none".to_string()
    } else {
        entry.files_modified.join(", ")
    };
    let notes = entry.notes.as_deref().unwrap_or("No notes recorded");
    format!(
        "### Iteration {} [{outcome}]\n\
         - **Task**: {task_id}\n\
         - **Model**: {model}\n\
         - **Duration**: {dur:.1}s | **Cost**: ${cost:.4}\n\
         - **Files**: {files}\n\
         - **Notes**: {notes}\n",
        entry.iteration,
        outcome = entry.outcome,
        task_id = entry.task_id.as_deref().unwrap_or("none"),
        model = entry.model.as_deref().unwrap_or("unknown"),
        dur = entry.duration_secs,
        cost = entry.cost_usd,
        files = files,
        notes = notes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::init_db;
    use tempfile::NamedTempFile;

    /// Helper: create a fresh DB with schema v3 and return a JournalEntry template.
    fn open_test_db() -> (NamedTempFile, Db) {
        let temp_file = NamedTempFile::new().unwrap();
        let db = init_db(temp_file.path().to_str().unwrap()).unwrap();
        (temp_file, db)
    }

    fn make_entry(run_id: &str, iteration: u32, outcome: &str) -> JournalEntry {
        JournalEntry {
            id: 0,
            run_id: run_id.to_string(),
            iteration,
            // task_id and feature_id are None to avoid FK constraint failures in tests
            // (the tasks and features tables would need pre-populated rows)
            task_id: None,
            feature_id: None,
            outcome: outcome.to_string(),
            model: Some("sonnet".to_string()),
            duration_secs: 12.5,
            cost_usd: 0.0042,
            files_modified: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            notes: Some(format!("Notes for iteration {} in run {}", iteration, run_id)),
            created_at: format!("2026-02-18T10:{:02}:00Z", iteration),
        }
    }

    #[test]
    fn test_insert_journal_entry() {
        let (_tmp, db) = open_test_db();

        let entry = make_entry("run-aabbccdd", 1, "done");
        let id = insert_journal_entry(&db, &entry).unwrap();
        assert!(id > 0, "insert should return a positive rowid");

        // Retrieve via query_journal_recent
        let results = query_journal_recent(&db, "run-aabbccdd", 10).unwrap();
        assert_eq!(results.len(), 1);

        let r = &results[0];
        assert_eq!(r.run_id, "run-aabbccdd");
        assert_eq!(r.iteration, 1);
        assert!(r.task_id.is_none());
        assert!(r.feature_id.is_none());
        assert_eq!(r.outcome, "done");
        assert_eq!(r.model.as_deref(), Some("sonnet"));
        assert!((r.duration_secs - 12.5).abs() < f64::EPSILON);
        assert!((r.cost_usd - 0.0042).abs() < 1e-9);
        assert_eq!(r.files_modified, vec!["src/main.rs", "src/lib.rs"]);
        assert_eq!(r.notes.as_deref(), Some("Notes for iteration 1 in run run-aabbccdd"));
        assert_eq!(r.created_at, "2026-02-18T10:01:00Z");
    }

    #[test]
    fn test_insert_journal_entry_no_notes() {
        let (_tmp, db) = open_test_db();

        let mut entry = make_entry("run-11223344", 1, "failed");
        entry.notes = None;

        // Should not error even with NULL notes (FTS trigger handles NULL gracefully)
        let id = insert_journal_entry(&db, &entry).unwrap();
        assert!(id > 0);

        let results = query_journal_recent(&db, "run-11223344", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].notes.is_none());
    }

    #[test]
    fn test_query_journal_recent() {
        let (_tmp, db) = open_test_db();

        // Insert 6 entries for run-A and 4 for run-B
        for i in 1..=6 {
            let entry = make_entry("run-A", i, "done");
            insert_journal_entry(&db, &entry).unwrap();
        }
        for i in 1..=4 {
            let entry = make_entry("run-B", i, "done");
            insert_journal_entry(&db, &entry).unwrap();
        }

        // Query run-A with limit 5 → should get 5 entries in chronological order
        let results_a = query_journal_recent(&db, "run-A", 5).unwrap();
        assert_eq!(results_a.len(), 5, "run-A should return 5 (limit)");
        // Chronological order: iterations 2, 3, 4, 5, 6 (most recent 5 reversed)
        assert_eq!(results_a[0].iteration, 2);
        assert_eq!(results_a[4].iteration, 6);

        // Query run-B with limit 10 → should get 4 entries (all of them)
        let results_b = query_journal_recent(&db, "run-B", 10).unwrap();
        assert_eq!(results_b.len(), 4, "run-B should return all 4 entries");
        // Chronological order
        assert_eq!(results_b[0].iteration, 1);
        assert_eq!(results_b[3].iteration, 4);

        // Verify only run-A entries come back for run-A
        for r in &results_a {
            assert_eq!(r.run_id, "run-A");
        }
        // Verify only run-B entries come back for run-B
        for r in &results_b {
            assert_eq!(r.run_id, "run-B");
        }
    }

    #[test]
    fn test_build_fts_query_basic() {
        let q = build_fts_query("implement the parser for JSON");
        // Words with length > 2 are kept: "implement" (9), "the" (3), "parser" (6), "for" (3), "JSON" (4)
        assert!(q.contains("implement"));
        assert!(q.contains("parser"));
        assert!(q.contains("JSON"));
        // The filter is `w.len() > 2` so words with 3+ chars pass through
        // Very short words (1-2 chars) would be filtered, e.g., "a" or "is"
        let q2 = build_fts_query("a is implement");
        assert!(!q2.contains(" a ") && !q2.starts_with("a "));
        assert!(!q2.contains(" is ") && !q2.contains("is OR"));
        assert!(q2.contains("implement"));
    }

    #[test]
    fn test_build_fts_query_empty() {
        assert_eq!(build_fts_query(""), "");
        assert_eq!(build_fts_query("  "), "");
        assert_eq!(build_fts_query("a b"), ""); // all words <= 2 chars
    }

    #[test]
    fn test_render_journal_context_format() {
        let entries = vec![
            JournalEntry {
                id: 1,
                run_id: "run-test".to_string(),
                iteration: 1,
                task_id: Some("t-abc123".to_string()),
                feature_id: None,
                outcome: "done".to_string(),
                model: Some("sonnet".to_string()),
                duration_secs: 30.0,
                cost_usd: 0.005,
                files_modified: vec!["src/main.rs".to_string()],
                notes: Some("Fixed the bug in parser".to_string()),
                created_at: "2026-02-18T10:00:00Z".to_string(),
            },
        ];

        let rendered = render_journal_context(&entries);
        assert!(rendered.contains("## Run Journal"));
        assert!(rendered.contains("### Iteration 1 [done]"));
        assert!(rendered.contains("**Task**: t-abc123"));
        assert!(rendered.contains("**Model**: sonnet"));
        assert!(rendered.contains("30.0s"));
        assert!(rendered.contains("$0.0050"));
        assert!(rendered.contains("src/main.rs"));
        assert!(rendered.contains("Fixed the bug in parser"));
    }

    #[test]
    fn test_render_journal_context_empty() {
        let rendered = render_journal_context(&[]);
        assert_eq!(rendered, "");
    }

    #[test]
    fn test_render_journal_context_no_notes() {
        let entries = vec![JournalEntry {
            id: 1,
            run_id: "run-test".to_string(),
            iteration: 2,
            task_id: None,
            feature_id: None,
            outcome: "failed".to_string(),
            model: None,
            duration_secs: 5.0,
            cost_usd: 0.001,
            files_modified: vec![],
            notes: None,
            created_at: "2026-02-18T10:00:00Z".to_string(),
        }];

        let rendered = render_journal_context(&entries);
        assert!(rendered.contains("No notes recorded"));
        assert!(rendered.contains("none")); // task_id = none
        assert!(rendered.contains("unknown")); // model = unknown
        assert!(rendered.contains("none")); // files = none
    }
}

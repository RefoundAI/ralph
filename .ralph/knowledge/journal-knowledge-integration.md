---
title: "Journal and knowledge system integration"
tags: [journal, knowledge, memory, fts5, context, iteration]
created_at: "2026-02-18T00:00:00Z"
---

The journal/knowledge system provides cross-iteration and cross-run memory.

**Journal** (`src/journal.rs`):
- SQLite table + FTS5 virtual table for full-text search
- Each iteration writes a `JournalEntry`: run_id, iteration, task_id, feature_id, outcome, model, duration_secs, cost_usd, files_modified, notes
- Smart selection in `select_journal_entries()`: combines up to 5 recent entries from current run + up to 5 FTS-matched entries from prior runs
- Rendered within 3000-token budget (~12K chars)
- Notes come from Claude's `<journal>` sigil output

**Knowledge** (`src/knowledge.rs`):
- Tagged markdown files in `.ralph/knowledge/` with YAML frontmatter (title, tags, optional feature, created_at)
- Tag-based scoring: +2 per tag matching task title/description words, +2 per tag matching feature name, +1 per tag matching recent file path segments
- Deduplication on write: exact title → replace, >50% tag overlap + substring title → merge tags + replace body, otherwise → new file
- Body truncated to 500 words, at least 1 tag required
- Rendered within 2000-token budget (~8K chars)

Both are always active (no toggle). Written post-iteration in run_loop.rs after task state updates.

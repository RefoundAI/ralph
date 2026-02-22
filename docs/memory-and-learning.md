# Memory and Learning

Ralph maintains persistent memory across iterations and runs through three
complementary mechanisms: the **journal**, the **knowledge base**, and
**CLAUDE.md / skills**. Together these allow the agent to learn from past work,
avoid repeating mistakes, and accumulate project-specific expertise that
compounds over time.

The journal records what happened each iteration. The knowledge base captures
reusable insights as tagged markdown files. CLAUDE.md and skills provide
project-wide context that benefits all Claude sessions, not just Ralph runs.

## Overview

| Mechanism | Storage | Scope | Created By | Surfaced By |
| --- | --- | --- | --- | --- |
| **Journal** | SQLite table + FTS5 index | Per-run + cross-run | `run_loop.rs` (automatic) | `journal.rs` (smart selection) |
| **Knowledge** | `.ralph/knowledge/*.md` | Cross-run, persistent | Agent via `<knowledge>` sigil | `knowledge.rs` (tag matching) |
| **Skills** | `.claude/skills/<name>/SKILL.md` | Cross-run, persistent | Agent (manual file creation) | Claude Code (native discovery) |
| **CLAUDE.md** | Project root `CLAUDE.md` | All Claude sessions | Agent (manual file update) | Claude Code (automatic) |

## Journal

**Source:** [`src/journal.rs`][journal.rs]

The journal is a persistent record of every iteration in the agent loop. Each
entry captures metadata about what happened -- which task was attempted, whether
it succeeded, how long it took, and what the agent learned. Journal entries are
stored in SQLite with FTS5 full-text search, enabling both recency-based and
relevance-based retrieval.

### Data Model

Each `JournalEntry` records:

```rust
pub struct JournalEntry {
    pub id: i64,                    // SQLite AUTOINCREMENT rowid
    pub run_id: String,             // run-{8hex}, groups entries by invocation
    pub iteration: u32,             // Iteration number within the run
    pub task_id: Option<String>,    // t-{6hex} of the assigned task
    pub feature_id: Option<String>, // f-{6hex} if task belongs to a feature
    pub outcome: String,            // done, failed, retried, blocked, interrupted
    pub model: Option<String>,      // Model used (opus, sonnet, haiku)
    pub duration_secs: f64,         // Wall-clock duration
    pub cost_usd: f64,             // Always 0.0 for ACP (ACP doesn't report cost)
    pub files_modified: Vec<String>,// Serialized as JSON array in SQLite
    pub notes: Option<String>,      // From the <journal> sigil, if emitted
    pub created_at: String,         // RFC 3339 timestamp
}
```

### How Entries Are Created

Journal entries are written automatically by `run_loop.rs` after every
iteration, regardless of outcome. The agent does not need to do anything for the
entry to be created -- the metadata fields (`run_id`, `iteration`, `task_id`,
`outcome`, `model`, `duration_secs`, `files_modified`) are all populated by
Ralph from iteration state.

The one field the agent controls is `notes`. The agent populates it by emitting
a `<journal>` sigil at the end of its output:

```
<journal>
Chose HashMap over BTreeMap for O(1) lookups. The task description mentioned
ordering but after reading the spec, insertion order doesn't matter. Tests pass.
</journal>
```

Good journal notes capture:

- Key decisions and their rationale
- Discoveries about the codebase
- Context the next iteration needs to know
- Gotchas encountered during implementation

### Outcome Values

The `outcome` field reflects how the iteration ended:

| Outcome | Meaning |
| --- | --- |
| `done` | Task completed successfully (agent emitted `<task-done>`) |
| `failed` | Task failed (agent emitted `<task-failed>`, or refusal stop reason) |
| `retried` | Task completed but failed verification; reset for retry |
| `blocked` | No sigil emitted, or agent hit token/turn limits |
| `interrupted` | User pressed Ctrl+C during the iteration |

### Smart Selection

At the start of each iteration, [`select_journal_entries()`][journal.rs]
assembles a context-relevant subset of journal entries for the system prompt.
It combines two queries:

1. **Recent entries** from the current run (`query_journal_recent`): The last
   5 entries (by iteration number) for the current `run_id`, returned in
   chronological order. These provide short-term continuity -- the agent
   knows what happened in recent iterations.

2. **FTS-matched entries** from prior runs (`query_journal_fts`): Up to
   5 entries from other runs whose notes match the current task's title and
   description via FTS5 full-text search. These provide cross-run learning --
   the agent can see relevant notes from past work on similar tasks.

The FTS query is built by [`build_fts_query()`][journal.rs]:

- Split the task title + description on whitespace
- Filter out words with 2 or fewer characters
- Cap at 10 words
- Join with `OR` for a disjunctive FTS5 query

Entries from the current run are excluded from FTS results (they already appear
in the recent set).

### Token Budget

Journal context is rendered as markdown and inserted into the system prompt
under a `## Run Journal` heading. A token budget of **3000 tokens** (estimated
at 4 characters per token = 12,000 characters) caps the total size. Entries are
added in order until the next entry would exceed the budget, at which point
rendering stops.

### Rendered Format

Each entry is rendered as:

```markdown
## Run Journal

### Iteration 3 [done]
- **Task**: t-abc123
- **Model**: sonnet
- **Duration**: 42.0s
- **Files**: src/main.rs, src/lib.rs
- **Notes**: Chose HashMap for O(1) lookups. Tests pass.

### Iteration 4 [retried]
- **Task**: t-def456
- **Model**: opus
- **Duration**: 198.3s | **Cost**: $1.1155
- **Files**: src/parser.rs
- **Notes**: Verification failed on edge case. Fixing in next attempt.
```

When `cost_usd` is 0.0 (which is always the case for ACP iterations), the cost
field is omitted from the duration line. Non-zero costs (from legacy or future
integrations) display as `**Duration**: Xs | **Cost**: $Y.YYYY`.

### Database Schema

The journal uses two tables created in schema version 3:

```sql
CREATE TABLE journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    iteration INTEGER NOT NULL,
    task_id TEXT,
    feature_id TEXT,
    outcome TEXT NOT NULL
        CHECK (outcome IN ('done','failed','retried','blocked')),
    model TEXT,
    duration_secs REAL,
    cost_usd REAL DEFAULT 0.0,
    files_modified TEXT,          -- JSON array
    notes TEXT,
    created_at TEXT NOT NULL
);

-- FTS5 virtual table for full-text search over journal notes
CREATE VIRTUAL TABLE journal_fts USING fts5(
    notes,
    content=journal,
    content_rowid=id
);

-- Trigger: auto-update FTS index when a journal row is inserted
CREATE TRIGGER journal_ai AFTER INSERT ON journal BEGIN
    INSERT INTO journal_fts(rowid, notes) VALUES (new.id, new.notes);
END;
```

The FTS5 virtual table is a content-sync index -- it mirrors the `notes` column
from the `journal` table. The `journal_ai` trigger keeps the index up to date
on inserts. FTS queries use `journal_fts MATCH ?` with `ORDER BY rank` for
relevance-sorted results.

## Knowledge Base

**Source:** [`src/knowledge.rs`][knowledge.rs]

The knowledge base stores reusable project insights as tagged markdown files in
`.ralph/knowledge/`. Unlike the journal (which records what happened), knowledge
entries capture what was learned -- patterns, gotchas, conventions, and
environment quirks that apply across tasks.

### File Format

Each knowledge entry is a markdown file with YAML frontmatter:

```markdown
---
title: "Cargo bench requires nightly"
tags: [testing, cargo, nightly]
feature: "perf-optimization"
created_at: "2026-02-17T12:00:00Z"
---

Running `cargo bench` requires the nightly Rust toolchain. Use `rustup run
nightly cargo bench` or set up a `rust-toolchain.toml` with `channel = "nightly"`
in the benchmark directory.
```

| Field | Required | Description |
| --- | --- | --- |
| `title` | Yes | Short descriptive title for the entry |
| `tags` | Yes | Array of lowercase keywords for matching (at least one) |
| `feature` | No | Feature name this knowledge is associated with |
| `created_at` | No | RFC 3339 timestamp (set automatically on write) |

The body after the frontmatter closing `---` contains the knowledge content,
limited to approximately 500 words. Longer bodies are truncated with a
`[truncated]` marker on write.

### How Entries Are Created

The agent creates knowledge entries by emitting a `<knowledge>` sigil in its
output:

```
<knowledge tags="testing,cargo,nightly" title="Cargo bench requires nightly">
Running `cargo bench` requires the nightly Rust toolchain. Use `rustup run
nightly cargo bench` or set up a `rust-toolchain.toml` with channel = "nightly".
</knowledge>
```

After each iteration, `run_loop.rs` scans the agent's output for `<knowledge>`
sigils (via [`parse_knowledge_sigils()`][sigils.rs]) and writes each one to disk
using [`write_knowledge_entry()`][knowledge.rs].

Tag values are normalized to lowercase and split on commas. At least one tag is
required; entries with no tags produce an error and are skipped. The body must
be non-empty.

Good knowledge entries capture:

- Environment-specific setup requirements
- Project conventions not documented elsewhere
- Workarounds for tooling limitations
- API usage patterns specific to the project
- Common pitfalls and how to avoid them

### Deduplication

When writing a knowledge entry, Ralph checks for existing entries that might
be duplicates. The logic in [`find_dedup_target()`][knowledge.rs]:

1. **Exact title match** (case-insensitive): If an existing file has the same
   title, the new content replaces it entirely. Tags are merged (existing tags
   preserved, new unique tags appended).

2. **Tag overlap + title substring**: If an existing file has >50% tag overlap
   (measured as the intersection divided by the smaller set) AND the titles
   have a substring relationship (either contains the other), the existing file
   is updated with merged tags and the new body.

3. **No match**: A new file is created at `.ralph/knowledge/{slug}.md`, where
   the slug is derived from the title (lowercase, non-alphanumeric characters
   replaced with hyphens, collapsed, truncated to 80 characters).

This prevents the knowledge base from accumulating near-duplicate entries as the
agent repeatedly encounters similar patterns.

### Discovery and Matching

At the start of each iteration, `build_iteration_context()` in `run_loop.rs`
calls two knowledge functions:

1. **[`discover_knowledge()`][knowledge.rs]**: Scans all `.md` files in
   `.ralph/knowledge/`, parses YAML frontmatter, and returns a list of
   `KnowledgeEntry` values. Files without valid frontmatter (missing title or
   empty tags) are silently skipped. If the directory does not exist, an
   empty list is returned.

2. **[`match_knowledge_entries()`][knowledge.rs]**: Scores each discovered
   entry by tag relevance to the current context:

| Signal | Score | Description |
| --- | --- | --- |
| Tag matches word in task title/description | +2 per tag | Direct task relevance |
| Tag matches current feature name | +2 per tag | Feature-level relevance |
| Tag matches word in recent file paths | +1 per tag | File-level relevance |

File path words are extracted by splitting paths on `/`, `.`, `-`, and `_`,
then filtering to words longer than 2 characters. The "recent file paths"
come from the `files_modified` field of the most recent journal entry.

Entries with a score of 0 are excluded. The remaining entries are sorted by
score descending.

### Token Budget

Knowledge context is rendered as markdown and inserted into the system prompt
under a `## Project Knowledge` heading. A token budget of **2000 tokens**
(estimated at 4 characters per token = 8,000 characters) caps the total size.

### Rendered Format

```markdown
## Project Knowledge

### Cargo bench requires nightly
_Tags: testing, cargo, nightly_

Running `cargo bench` requires the nightly Rust toolchain. Use `rustup run
nightly cargo bench` or set up a `rust-toolchain.toml`.

### SQLite WAL mode for concurrent access
_Tags: database, sqlite, concurrency_

Enable WAL mode with `PRAGMA journal_mode=WAL` at connection time for
concurrent readers during writes.
```

### Directory Layout

```
.ralph/knowledge/
  cargo-bench-requires-nightly.md
  sqlite-wal-mode-for-concurrent-access.md
  acp-connection-lifecycle-pattern-with-localset-and-owned-data.md
  config-from-run-args-new-signature-post-acp-changes.md
```

Filenames are derived from the entry title via `slugify_title()`. The `.ralph/`
directory is gitignored by default, but knowledge files can be committed to
version control if desired.

## Skills and CLAUDE.md

Skills and CLAUDE.md represent knowledge that persists outside of Ralph's own
systems. They are managed by the agent through normal file operations rather
than through sigils.

### CLAUDE.md

CLAUDE.md is the standard project context file for Claude Code. It is read at
the start of every Claude session -- not just Ralph-managed ones. The system
prompt instructs the agent to update CLAUDE.md with project-wide knowledge:

> You should also continue to update CLAUDE.md with project-specific knowledge
> that benefits all future Claude sessions (not just Ralph runs).

Typical CLAUDE.md updates:

- Build commands, test commands, and deployment procedures
- Architectural patterns and module conventions
- Common pitfalls encountered during task execution
- Data model details not obvious from the code

CLAUDE.md updates benefit all future Claude interactions with the project,
including interactive sessions, code review, and debugging -- making it the
broadest-reaching memory mechanism.

### Skills

Skills are reusable procedure documents that live at
`.claude/skills/<name>/SKILL.md`. They are **Claude Code native skills**,
discovered and surfaced by Claude Code's own skill system -- not by Ralph.

Ralph's role with skills is limited:

- `ralph init` creates the `.claude/skills/` directory as part of the project
  scaffold
- The system prompt encourages the agent to document reusable procedures

Ralph does **not** scan, parse, or inject skills into the system prompt. That
is handled entirely by Claude Code's native skill discovery mechanism. This
differs from the old system where Ralph had its own `discover_skills()` function
and an "Available Skills" prompt section.

> [!NOTE]
> Skills previously lived at `.ralph/skills/` and were discovered by Ralph's own
> `discover_skills()` function. That system was replaced by Claude Code's native
> skill discovery. The `ralph init` command includes a backward-compatibility
> migration that moves skills from `.ralph/skills/` to `.claude/skills/` if the
> old directory exists.

## System Prompt Integration

The memory systems integrate with the system prompt at three points, all in
[`build_prompt_text()`][prompt.rs]:

### 1. Journal Context (conditional)

If `journal_context` is non-empty, the pre-rendered markdown from
`render_journal_context()` is appended to the prompt. This section appears
as `## Run Journal` with iteration headers.

```rust
if !context.journal_context.is_empty() {
    prompt.push('\n');
    prompt.push_str(&context.journal_context);
}
```

### 2. Knowledge Context (conditional)

If `knowledge_context` is non-empty, the pre-rendered markdown from
`render_knowledge_context()` is appended. This section appears as
`## Project Knowledge` with entry headers.

```rust
if !context.knowledge_context.is_empty() {
    prompt.push('\n');
    prompt.push_str(&context.knowledge_context);
}
```

### 3. Memory Instructions (always)

A `## Memory` section is always appended, documenting both sigils for the
agent:

```markdown
## Memory

You have access to a persistent memory system. Use these sigils to record knowledge:

### End-of-Task Journal
At the end of your work on this task, emit a `<journal>` sigil summarizing key
decisions, discoveries, and context that would help the next iteration:

```
<journal>
What you decided and why. What you discovered. What the next task should know.
</journal>
```

### Project Knowledge
When you discover reusable project knowledge (patterns, gotchas, conventions,
environment quirks), emit a `<knowledge>` sigil:

```
<knowledge tags="tag1,tag2" title="Short descriptive title">
Detailed explanation of the knowledge. Maximum ~500 words.
</knowledge>
```

Tags should be lowercase, relevant keywords. At least one tag is required.

You should also continue to update CLAUDE.md with project-wide knowledge that
benefits all future Claude sessions (not just Ralph runs).
```

The Memory section is always present, even if no journal or knowledge context
exists yet. This ensures the agent knows how to create entries from the very
first iteration.

## Data Flow

The following diagram traces how memory data flows through the system across
iterations:

```
Iteration N
  |
  v
build_iteration_context()               <- run_loop.rs
  |
  +---> select_journal_entries()         <- journal.rs
  |       query_journal_recent(run_id, limit=5)
  |       query_journal_fts(task_title + description, limit=5)
  |       |
  |       v
  |     render_journal_context()
  |       -> "## Run Journal\n..."
  |
  +---> discover_knowledge()             <- knowledge.rs
  |       scan .ralph/knowledge/*.md
  |       parse YAML frontmatter
  |       |
  |       v
  |     match_knowledge_entries()
  |       score by tag relevance
  |       filter score > 0, sort desc
  |       |
  |       v
  |     render_knowledge_context()
  |       -> "## Project Knowledge\n..."
  |
  v
IterationContext {
    journal_context: "## Run Journal\n...",
    knowledge_context: "## Project Knowledge\n...",
    ...
}
  |
  v
build_prompt_text()                      <- acp/prompt.rs
  system instructions
  + task context
  + spec/plan (if feature)
  + retry info (if retry)
  + journal context (if non-empty)
  + knowledge context (if non-empty)
  + memory instructions (always)
  |
  v
ACP Agent Session
  agent works on the task
  |
  +---> emits <journal>notes</journal>
  +---> emits <knowledge tags="..." title="...">body</knowledge>
  +---> emits <task-done>t-xxx</task-done>
  |
  v
Post-iteration processing                <- run_loop.rs
  |
  +---> insert_journal_entry()           <- journal.rs
  |       writes to SQLite journal table
  |       FTS trigger updates journal_fts
  |
  +---> write_knowledge_entry()          <- knowledge.rs
  |       deduplication check
  |       write/update .ralph/knowledge/{slug}.md
  |
  v
Iteration N+1
  (picks up new journal + knowledge entries)
```

## Relationship to Other Systems

### Verification

Memory and verification operate independently. The verification agent is a
read-only ACP session that checks task correctness -- it does not read or write
journal or knowledge entries. However, when a task fails verification and is
retried, the retry information (attempt number and failure reason) appears in
the prompt alongside any relevant journal and knowledge context, helping the
agent avoid repeating the same mistake.

### Retry System

When a task fails and is retried, the journal entry for the failed attempt
records `outcome: "retried"`. On the next attempt, the agent sees:

- The retry information (attempt number, failure reason) in the `## Retry
  Information` section
- The previous iteration's journal entry (with notes about what went wrong)
  in the `## Run Journal` section
- Any relevant knowledge entries that might help avoid the same failure

This creates a feedback loop: failure information feeds into the next attempt's
context.

### Feature Specs and Plans

Knowledge entries complement specs and plans. A spec defines _what_ to build,
a plan defines _how_ to build it, and knowledge entries capture reusable
_insights_ that apply across features. For example, a knowledge entry about
"SQLite WAL mode for concurrent access" might be relevant to many features that
touch the database, even though it is not mentioned in any specific spec or
plan.

### Interrupt Handling

When the user interrupts an iteration with Ctrl+C, the journal entry records
`outcome: "interrupted"` and includes any user feedback (collected via the
interrupt prompt) in the `notes` field. This preserves the user's intent across
the interruption boundary.

## Configuration

There is no configuration flag to enable or disable the memory systems. The
journal and knowledge base are always active. The old `learn` field in
`ExecutionConfig` is retained for backward compatibility with existing
`.ralph.toml` files but is ignored at runtime.

```toml
[execution]
# The "learn" field is accepted but ignored.
# Journal and knowledge are always enabled.
verify = true
max_retries = 3
```

There is no `--no-learn` CLI flag. The memory systems are considered essential
to Ralph's operation and cannot be turned off.

## Key Source Files

| File | Role |
| --- | --- |
| [`src/journal.rs`][journal.rs] | `JournalEntry`, insert, query (recent + FTS), render |
| [`src/knowledge.rs`][knowledge.rs] | `KnowledgeEntry`, discover, match, write, render |
| [`src/acp/prompt.rs`][prompt.rs] | System prompt construction with journal/knowledge context |
| [`src/acp/sigils.rs`][sigils.rs] | `parse_journal_sigil()`, `parse_knowledge_sigils()` |
| [`src/acp/types.rs`][types.rs] | `IterationContext`, `KnowledgeSigil`, `SigilResult` |
| [`src/run_loop.rs`][run_loop.rs] | `build_iteration_context()`, post-iteration journal/knowledge writes |
| [`src/dag/db.rs`][db.rs] | Schema v3: `journal` table, `journal_fts` index, FTS trigger |

[journal.rs]: ../src/journal.rs
[knowledge.rs]: ../src/knowledge.rs
[prompt.rs]: ../src/acp/prompt.rs
[sigils.rs]: ../src/acp/sigils.rs
[types.rs]: ../src/acp/types.rs
[run_loop.rs]: ../src/run_loop.rs
[db.rs]: ../src/dag/db.rs

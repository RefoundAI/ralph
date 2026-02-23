---
title: Journal System
tags: [journal, sqlite, fts5, memory, run-loop]
created_at: "2026-02-18T00:00:00Z"
---

Persistent iteration records in `src/journal.rs`, stored in SQLite with FTS5 full-text search (schema v3).

## Entry Fields

`run_id`, `iteration`, `task_id`, `feature_id`, `outcome` (done/failed/retried/blocked/interrupted), `model`, `duration_secs`, `cost_usd`, `files_modified`, `notes`, `created_at`.

Notes come from the `<journal>` sigil — see [[Sigil Parsing]].

## Smart Selection

`select_journal_entries()` combines two sources:
1. **Recent entries** from current `run_id` (chronological, up to 5)
2. **FTS matches** from prior runs (keyword search, up to 5)

This gives continuity within a run and cross-run learning.

## FTS Query Building

`build_fts_query()`: words >2 chars, cap at 10, OR-joined. FTS5 triggers auto-sync index (see [[Schema Migrations]] for trigger gotcha).

## Token Budget

Rendered within **3000-token budget** (~4 chars/token). Pre-rendered as markdown in [[System Prompt Construction]].

## Write Timing

Journal entries written post-iteration in [[Run Loop Lifecycle]], after task state updates, so they record the final outcome.

See also: [[Knowledge System]], [[System Prompt Construction]], [[Run Loop Lifecycle]], [[Schema Migrations]], [[Sigil Parsing]]

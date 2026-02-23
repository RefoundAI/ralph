---
title: Roam Protocol Bidirectional Linking
tags: [knowledge, links, roam, zettelkasten, graph]
created_at: "2026-02-23T06:12:25.366034+00:00"
---

Knowledge entries support `[[Title]]` references for zettelkasten-style dense linking. Implementation in `src/knowledge.rs`.

## Components

- `extract_links()`: Parses `[[Title]]` references, deduplicates case-insensitively
- `LinkGraph`: `outlinks` + `backlinks` as `HashMap<String, HashSet<String>>` (lowercase keys)
- `build_link_graph()`: Scans all entries, only tracks links to entries that actually exist
- `expand_via_links()`: BFS from tag-matched entries, `max_hops=2`, bonus decays with distance (`base_bonus / hop_number`)

## Rendering

`render_knowledge_context_with_graph()` shows `_Linked from:_` and `_Links to:_` metadata for each entry. The non-graph `render_knowledge_context()` is `#[cfg(test)]` only.

## Lifecycle

Link graph rebuilt each iteration in `build_iteration_context()` ([[Run Loop Lifecycle]]). This means newly written knowledge entries are immediately linkable in the next iteration.

See also: [[Knowledge System]], [[System Prompt Construction]], [[Run Loop Lifecycle]]

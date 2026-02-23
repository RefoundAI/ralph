---
title: Knowledge System
tags: [knowledge, tags, matching, deduplication, memory]
created_at: "2026-02-18T00:00:00Z"
---

Tag-based project knowledge in `src/knowledge.rs`. Markdown files in `.ralph/knowledge/` with YAML frontmatter (`title`, `tags`, optional `feature`, `created_at`).

## Tag-Based Scoring

`match_knowledge_entries()` scores entries against current context:
- +2 per tag matching task title/description words
- +2 per tag matching feature name
- +1 per tag matching recent file path segments

## Deduplication on Write

`write_knowledge_entry()` checks existing entries:
- **Exact title match** → replace body and tags
- **>50% tag overlap + substring title** → merge tags, replace body
- **Otherwise** → create new file

Body truncated to ~500 words, at least 1 tag required.

## Bidirectional Linking

Entries can reference each other via `[[Title]]` syntax — see [[Roam Protocol Bidirectional Linking]]. Link expansion pulls in related entries not directly matched by tags.

## Token Budget

Rendered within **2000-token budget** (~8K chars). Pre-rendered as markdown with backlinks/outlinks metadata in [[System Prompt Construction]].

## Write Path

Knowledge entries from `<knowledge>` sigils are written post-iteration in [[Run Loop Lifecycle]]. See [[Sigil Parsing]] for sigil format.

See also: [[Roam Protocol Bidirectional Linking]], [[Journal System]], [[Sigil Parsing]], [[System Prompt Construction]], [[Run Loop Lifecycle]]

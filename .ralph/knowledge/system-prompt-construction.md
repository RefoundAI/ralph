---
title: System Prompt Construction
tags: [prompt, system-prompt, acp, context, iteration]
created_at: "2026-02-18T00:00:00Z"
---

System prompt assembled in `src/acp/prompt.rs` via `build_prompt_text()`. Single `TextContent` block (ACP has no separate system prompt channel).

## Section Order

1. **Base prompt**: DAG task rules, [[Sigil Parsing]] instructions, tool constraints
2. **Task context**: Assigned task (title, description, parent, completed blockers)
3. **Spec content** (if feature target): Full `spec.md` — see [[Feature Lifecycle]]
4. **Plan content** (if feature target): Full `plan.md`
5. **Retry info** (if retrying): Attempt count, max retries, previous failure reason
6. **Journal context** (if non-empty): Pre-rendered markdown, 3000-token budget — see [[Journal System]]
7. **Knowledge context** (if non-empty): Pre-rendered markdown with link graph, 2000-token budget — see [[Knowledge System]]
8. **Memory section** (always): Sigil format docs, [[Roam Protocol Bidirectional Linking]] instructions

## Adding a New Section

Insert between retry_info and Memory section. Pattern: check if non-empty, push newline, push content.

## Context Pre-Rendering

Journal and knowledge contexts are pre-rendered as markdown strings in `build_iteration_context()` ([[Run Loop Lifecycle]]) and passed verbatim — not JSON.

See also: [[Sigil Parsing]], [[Journal System]], [[Knowledge System]], [[Feature Lifecycle]], [[Run Loop Lifecycle]], [[Roam Protocol Bidirectional Linking]]

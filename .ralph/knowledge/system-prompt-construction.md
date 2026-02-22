---
title: "System prompt construction and context sections"
tags: [prompt, system-prompt, acp, context, iteration]
created_at: "2026-02-18T00:00:00Z"
---

The system prompt is built dynamically in `src/acp/prompt.rs` via `build_prompt_text()`. It assembles optional context sections in this order:

1. **Base prompt**: DAG task assignment rules, sigil instructions, tool constraints
2. **Task context**: Assigned task details (title, description, parent, completed blockers)
3. **Spec content** (if feature target): Full spec.md contents
4. **Plan content** (if feature target): Full plan.md contents
5. **Retry info** (if retrying): "This is retry attempt X of Y" + previous failure reason
6. **Journal context** (if non-empty): Pre-rendered markdown from `journal::render_journal_context()`, 3000-token budget
7. **Knowledge context** (if non-empty): Pre-rendered markdown from `knowledge::render_knowledge_context()`, 2000-token budget
8. **Memory section** (always): Instructions for `<journal>` and `<knowledge>` sigils

Journal and knowledge contexts are pre-rendered as markdown strings in `run_loop.rs::build_iteration_context()` and passed verbatim into the prompt. They are not JSON — just markdown sections.

The prompt is assembled as a single `TextContent` block (ACP has no separate system prompt channel). The function signature is `build_prompt_text(config: &Config, context: &IterationContext) -> String`.

When adding a new optional context section, insert it between retry_info and the Memory section. Follow the pattern: check if non-empty, push a newline, push the content.

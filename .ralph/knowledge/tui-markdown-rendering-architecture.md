---
title: "TUI Markdown Rendering Architecture"
tags: [ui, theme, markdown, tui, view]
created_at: "2026-02-26T04:49:57.071673+00:00"
---

Agent stream markdown rendering in the TUI uses ratatui native `Span`/`Line` styling (not ANSI escapes). The rendering pipeline:

1. `src/ui/state.rs` — `AgentText` events: trims leading whitespace on first chunk, collapses 3+ consecutive newlines into 2. `AgentThinking` events: stored separately, rendered indented with subdued styling.
2. `src/ui/view.rs` — `render_agent_markdown()` parses `state.agent_text` into `Vec<Line<'static>>` with styled spans
3. Supported elements: fenced code blocks (code_block style), headings (heading style, bold), blockquotes (italic, `│` prefix), list bullets (colored bullet/number), horizontal rules (40-char `─` line), inline code/bold/italic/links, sigil bodies (sigil_body style)
4. `parse_inline_markdown()` handles inline elements returning `Vec<Span<'static>>`
5. `IterationDivider` events render as visual separators between iterations in the agent stream

Tool activity uses `ToolLine { name, summary }` struct. `ToolDetail` lines render indented under the most recent tool call. Rendered as: tool_name style + accent arrow + subdued summary.

Theme struct fields: `heading_fg`, `code_span_fg`, `code_block_fg`, `link_fg`, `blockquote_fg`, `list_bullet_fg`, `hr_fg`, `accent_fg`, `tool_name_fg`, `sigil_body_fg`. All support `[ui.colors]` overrides (TOML keys omit the `_fg` suffix, e.g. `heading`, `code_span` — see [[Custom Color Palettes in .ralph.toml]]).

See also: [[Themeable TUI Colour Scheme]], [[Ratatui UI Runtime]], [[UI Event Routing and Plain Fallback]]

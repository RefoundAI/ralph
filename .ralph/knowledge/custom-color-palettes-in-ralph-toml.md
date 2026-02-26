---
title: "Custom Color Palettes in .ralph.toml"
tags: [ui, theme, config, colors, customization]
created_at: "2026-02-26T02:08:48.878430+00:00"
---

Users can define per-token color overrides in `[ui.colors]` section of `.ralph.toml`.

## Config Format
```toml
[ui]
theme = "light"  # base theme (default)

[ui.colors]
border = "#ff5500"    # hex format
title = "magenta"     # named color
```

## Architecture
- `ColorOverrides` struct in `src/ui/theme.rs` — 24 `Option<String>` fields matching Theme tokens
- `parse_color()` handles hex (#rrggbb) and 17 named terminal colors (case-insensitive)
- `ColorOverrides::validate()` checks all set values at config load time
- `ColorOverrides::apply_to()` merges overrides onto a base Theme (maps short names to `_fg`-suffixed Theme fields, e.g. `border` → `border_fg`)
- `init_with_overrides(name, overrides)` replaces `init(name)` at call sites

## Validation
Runs in `load_config()` in `src/project.rs`. Invalid colors produce errors like:
`invalid color for ui.colors.border: unknown color 'neon-pink': expected a hex value like '#ff5500' or a named color...`

## Token Names (24 total)
Core: `background`, `border`, `title`, `status`, `subdued`, `info`, `warn`, `error`, `dim_overlay`, `modal_text`, `input_inactive`, `input_text`, `modal_border`, `cursor_fg`, `cursor_bg`
Markdown rendering: `heading`, `code_span`, `code_block`, `link`, `blockquote`, `list_bullet`, `hr`
Tool activity: `accent`, `tool_name`

## Raw String Gotcha
Tests with TOML containing hex colors need `r##"..."##` not `r#"..."#` because `"#` terminates r# strings.

See also: [[Themeable TUI Colour Scheme]], [[Configuration Layers]]

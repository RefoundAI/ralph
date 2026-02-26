---
title: "Custom Color Palettes in .ralph.toml"
tags: [ui, theme, config, colors, customization]
created_at: "2026-02-26T02:08:48.878430+00:00"
---

Users can define per-token color overrides in `[ui.colors]` section of `.ralph.toml`.

## Config Format
```toml
[ui]
theme = "dark"  # base theme

[ui.colors]
border = "#ff5500"    # hex format
title = "magenta"     # named color
```

## Architecture
- `ColorOverrides` struct in `src/ui/theme.rs` — 14 `Option<String>` fields matching Theme tokens
- `parse_color()` handles hex (#rrggbb) and 17 named terminal colors (case-insensitive)
- `ColorOverrides::validate()` checks all set values at config load time
- `ColorOverrides::apply_to()` merges overrides onto a base Theme
- `init_with_overrides(name, overrides)` replaces `init(name)` at call sites

## Validation
Runs in `load_config()` in `src/project.rs`. Invalid colors produce errors like:
`invalid color for ui.colors.border: unknown color 'neon-pink': expected a hex value like '#ff5500' or a named color...`

## Token Names
`background`, `border`, `title`, `status`, `subdued`, `info`, `warn`, `error`, `dim_overlay`, `modal_text`, `input_inactive`, `modal_border`, `cursor_fg`, `cursor_bg`

## Raw String Gotcha
Tests with TOML containing hex colors need `r##"..."##` not `r#"..."#` because `"#` terminates r# strings.

See also: [[Themeable TUI Colour Scheme]], [[Configuration Layers]]

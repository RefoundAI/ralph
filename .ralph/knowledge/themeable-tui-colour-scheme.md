---
title: "Themeable TUI Colour Scheme"
tags: [ui, theme, config, tui]
created_at: "2026-02-26T01:54:27.641899+00:00"
---

The TUI uses a `Theme` struct in `src/ui/theme.rs` that holds all color tokens. Two built-in themes exist: `light` (default) and `dark`.

## Resolution Order
1. `RALPH_THEME` env var (highest priority)
2. `[ui].theme` in `.ralph.toml` (serde default: `"light"`)
3. Invalid/missing value falls back to `Dark`

## Global State
Active theme stored in `OnceLock<Theme>` — initialized once via `theme::init()` before any rendering. All `theme::*()` accessor functions read from this global.

## Startup Wiring
`theme::init_with_overrides(resolve_theme_name(...), Some(&config.ui.colors))` is called in three places in `main.rs`:
- `handle_feature()` — after `project::discover()`
- `handle_task()` — after `project::discover()`
- `Run` branch — after `project::discover()`, before `ui::start()`

Color overrides come from `[ui.colors]` in `.ralph.toml` — see [[Custom Color Palettes in .ralph.toml]].

## Adding New Tokens
Add field to `Theme` struct, set values in both `Theme::dark()` and `Theme::light()`, add accessor function.

See also: [[Ratatui UI Runtime]], [[Configuration Layers]], [[UI Event Routing and Plain Fallback]], [[Theme Color Addition Checklist]], [[TUI Markdown Rendering Architecture]], [[Custom Color Palettes in .ralph.toml]], [[Event Emission System]]

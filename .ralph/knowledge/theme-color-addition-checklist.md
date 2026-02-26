---
title: "Theme Color Addition Checklist"
tags: [ui, theme, color, input]
created_at: "2026-02-26T04:17:39.349012+00:00"
---

When adding a new color to the TUI theme system, all these locations must be updated:

1. `Theme` struct field in `src/ui/theme.rs`
2. `Theme::dark()` constructor
3. `Theme::light()` constructor
4. `ColorOverrides` struct (with `Option<String>` field)
5. `ColorOverrides::validate()` field list
6. `ColorOverrides::apply_to()` set call
7. Style accessor function (e.g. `pub fn input_text() -> Style`)
8. `color_overrides_all_fields` test

Use `Color::Reset` when you want the terminal's native fg/bg to show through — this is the safest default for text that must be readable on any terminal background.

See also: [[Ratatui UI Runtime]], [[TUI Input Model (v0.8.1+)]]

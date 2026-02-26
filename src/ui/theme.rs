//! Color and style tokens for the ratatui dashboard.
//!
//! Supports light and dark themes. The active theme is resolved at startup
//! from `RALPH_THEME` env var, `[ui].theme` in `.ralph.toml`, or defaults
//! to `light`. All rendering code reads from the resolved theme via the
//! public `theme::*()` accessor functions.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

use crate::ui::event::UiLevel;

/// All color tokens used by the TUI.
#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub border_fg: Color,
    pub title_fg: Color,
    pub status_fg: Color,
    pub subdued_fg: Color,
    pub info_fg: Color,
    pub warn_fg: Color,
    pub error_fg: Color,
    pub dim_overlay_fg: Color,
    pub modal_text_fg: Color,
    pub input_inactive_fg: Color,
    pub modal_border_fg: Color,
    /// Foreground for the cursor block (character under cursor).
    pub cursor_fg: Color,
    /// Background for the cursor block.
    pub cursor_bg: Color,
}

impl Theme {
    /// Dark theme — preserves the original hardcoded scheme.
    pub fn dark() -> Self {
        Self {
            background: Color::Black,
            border_fg: Color::DarkGray,
            title_fg: Color::Cyan,
            status_fg: Color::Green,
            subdued_fg: Color::Rgb(180, 180, 180),
            info_fg: Color::White,
            warn_fg: Color::Yellow,
            error_fg: Color::Red,
            dim_overlay_fg: Color::DarkGray,
            modal_text_fg: Color::White,
            input_inactive_fg: Color::DarkGray,
            modal_border_fg: Color::Cyan,
            cursor_fg: Color::Black,
            cursor_bg: Color::White,
        }
    }

    /// Light theme — designed for light terminal backgrounds.
    pub fn light() -> Self {
        Self {
            background: Color::White,
            border_fg: Color::Rgb(160, 160, 160),
            title_fg: Color::Blue,
            status_fg: Color::Rgb(0, 140, 0),
            subdued_fg: Color::Rgb(80, 80, 80),
            info_fg: Color::Black,
            warn_fg: Color::Rgb(180, 130, 0),
            error_fg: Color::Red,
            dim_overlay_fg: Color::Rgb(180, 180, 180),
            modal_text_fg: Color::Black,
            input_inactive_fg: Color::Rgb(160, 160, 160),
            modal_border_fg: Color::Blue,
            cursor_fg: Color::White,
            cursor_bg: Color::Black,
        }
    }
}

/// Name of the active theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Light,
    Dark,
}

impl ThemeName {
    /// Parse from a string value (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

static ACTIVE_THEME: OnceLock<Theme> = OnceLock::new();

/// Resolve the theme from `RALPH_THEME` env var (overrides config), then
/// the config file value. Falls back to `Light` if neither is set.
pub fn resolve_theme_name(config_theme: &str) -> ThemeName {
    // Env var takes priority.
    if let Ok(val) = std::env::var("RALPH_THEME") {
        if let Some(name) = ThemeName::parse(&val) {
            return name;
        }
    }
    // Config file value.
    ThemeName::parse(config_theme).unwrap_or(ThemeName::Light)
}

/// Initialize the active theme. Call once at startup before any rendering.
/// If called multiple times, subsequent calls are ignored (first write wins).
pub fn init(name: ThemeName) {
    let theme = match name {
        ThemeName::Light => Theme::light(),
        ThemeName::Dark => Theme::dark(),
    };
    let _ = ACTIVE_THEME.set(theme);
}

/// Get the active theme. Falls back to light if not initialized.
fn active() -> &'static Theme {
    ACTIVE_THEME.get_or_init(Theme::light)
}

pub fn background() -> Color {
    active().background
}

pub fn border() -> Style {
    let t = active();
    Style::default().fg(t.border_fg).bg(t.background)
}

pub fn title() -> Style {
    let t = active();
    Style::default()
        .fg(t.title_fg)
        .bg(t.background)
        .add_modifier(Modifier::BOLD)
}

pub fn status() -> Style {
    let t = active();
    Style::default()
        .fg(t.status_fg)
        .bg(t.background)
        .add_modifier(Modifier::BOLD)
}

pub fn subdued() -> Style {
    let t = active();
    Style::default().fg(t.subdued_fg).bg(t.background)
}

pub fn level(level: UiLevel) -> Style {
    let t = active();
    match level {
        UiLevel::Info => Style::default().fg(t.info_fg).bg(t.background),
        UiLevel::Warn => Style::default().fg(t.warn_fg).bg(t.background),
        UiLevel::Error => Style::default()
            .fg(t.error_fg)
            .bg(t.background)
            .add_modifier(Modifier::BOLD),
    }
}

/// Style for the dim overlay behind modals.
pub fn dim_overlay() -> Style {
    let t = active();
    Style::default().fg(t.dim_overlay_fg).bg(t.background)
}

/// Style for modal text content.
pub fn modal_text() -> Style {
    let t = active();
    Style::default()
        .fg(t.modal_text_fg)
        .bg(t.background)
        .add_modifier(Modifier::BOLD)
}

/// Style for the inactive Input pane border and hint text.
pub fn input_inactive() -> Style {
    let t = active();
    Style::default().fg(t.input_inactive_fg).bg(t.background)
}

/// Style for modal borders.
pub fn modal_border() -> Style {
    let t = active();
    Style::default()
        .fg(t.modal_border_fg)
        .bg(t.background)
        .add_modifier(Modifier::BOLD)
}

/// Style for the cursor block (inverted fg/bg).
pub fn cursor() -> Style {
    let t = active();
    Style::default().fg(t.cursor_fg).bg(t.cursor_bg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_name_parse_light() {
        assert_eq!(ThemeName::parse("light"), Some(ThemeName::Light));
        assert_eq!(ThemeName::parse("Light"), Some(ThemeName::Light));
        assert_eq!(ThemeName::parse("LIGHT"), Some(ThemeName::Light));
    }

    #[test]
    fn theme_name_parse_dark() {
        assert_eq!(ThemeName::parse("dark"), Some(ThemeName::Dark));
        assert_eq!(ThemeName::parse("Dark"), Some(ThemeName::Dark));
    }

    #[test]
    fn theme_name_parse_invalid() {
        assert_eq!(ThemeName::parse("nope"), None);
        assert_eq!(ThemeName::parse(""), None);
    }

    #[test]
    fn dark_theme_preserves_original_colors() {
        let t = Theme::dark();
        assert_eq!(t.background, Color::Black);
        assert_eq!(t.title_fg, Color::Cyan);
        assert_eq!(t.border_fg, Color::DarkGray);
    }

    #[test]
    fn light_theme_has_white_background() {
        let t = Theme::light();
        assert_eq!(t.background, Color::White);
        assert_eq!(t.title_fg, Color::Blue);
    }

    #[test]
    fn resolve_falls_back_to_light() {
        // With no env var set, unknown config value falls back to light.
        std::env::remove_var("RALPH_THEME");
        assert_eq!(resolve_theme_name("invalid"), ThemeName::Light);
    }

    #[test]
    fn resolve_uses_config_value() {
        std::env::remove_var("RALPH_THEME");
        assert_eq!(resolve_theme_name("dark"), ThemeName::Dark);
        assert_eq!(resolve_theme_name("light"), ThemeName::Light);
    }

    #[test]
    fn resolve_env_overrides_config() {
        std::env::set_var("RALPH_THEME", "dark");
        assert_eq!(resolve_theme_name("light"), ThemeName::Dark);
        std::env::remove_var("RALPH_THEME");
    }

    #[test]
    fn resolve_env_invalid_falls_through_to_config() {
        std::env::set_var("RALPH_THEME", "nope");
        assert_eq!(resolve_theme_name("dark"), ThemeName::Dark);
        std::env::remove_var("RALPH_THEME");
    }
}

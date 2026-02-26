//! Color and style tokens for the ratatui dashboard.
//!
//! Supports light and dark themes with optional per-token color overrides.
//! The active theme is resolved at startup from `RALPH_THEME` env var,
//! `[ui].theme` in `.ralph.toml`, or defaults to `light`. Users can set
//! individual color overrides in `[ui.colors]` that layer on top of the
//! base theme.
//!
//! All rendering code reads from the resolved theme via the public
//! `theme::*()` accessor functions.

use std::sync::OnceLock;

use anyhow::{bail, Result};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

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
    /// Foreground for user-typed input text.
    /// Defaults to `Color::Reset` so the terminal's native foreground is used,
    /// ensuring readability on both light and dark terminal backgrounds.
    pub input_text_fg: Color,
    /// Foreground for the cursor block (character under cursor).
    pub cursor_fg: Color,
    /// Background for the cursor block.
    pub cursor_bg: Color,
}

impl Theme {
    /// Dark theme — for dark terminal backgrounds (default).
    ///
    /// Uses `Color::Reset` for background so the terminal's native background
    /// shows through. Foreground colors are chosen for good contrast on dark.
    pub fn dark() -> Self {
        Self {
            background: Color::Reset,
            border_fg: Color::Rgb(100, 100, 100),
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
            input_text_fg: Color::Reset,
            cursor_fg: Color::Black,
            cursor_bg: Color::White,
        }
    }

    /// Light theme — for light terminal backgrounds.
    ///
    /// Uses `Color::Reset` for background so the terminal's native background
    /// shows through. Foreground colors are chosen for good contrast on light.
    pub fn light() -> Self {
        Self {
            background: Color::Reset,
            border_fg: Color::Rgb(140, 140, 140),
            title_fg: Color::Blue,
            status_fg: Color::Rgb(0, 140, 0),
            subdued_fg: Color::Rgb(80, 80, 80),
            info_fg: Color::Black,
            warn_fg: Color::Rgb(180, 130, 0),
            error_fg: Color::Red,
            dim_overlay_fg: Color::Rgb(200, 200, 200),
            modal_text_fg: Color::Black,
            input_inactive_fg: Color::Rgb(160, 160, 160),
            modal_border_fg: Color::Blue,
            input_text_fg: Color::Reset,
            cursor_fg: Color::White,
            cursor_bg: Color::Black,
        }
    }
}

/// Parse a color string into a ratatui `Color`.
///
/// Accepts:
/// - Hex values: `#rrggbb` (6-digit, case-insensitive)
/// - Named terminal colors (case-insensitive): `black`, `red`, `green`, `yellow`,
///   `blue`, `magenta`, `cyan`, `white`, `gray`/`grey`, `darkgray`/`darkgrey`,
///   `lightred`, `lightgreen`, `lightyellow`, `lightblue`, `lightmagenta`,
///   `lightcyan`, `reset`
pub fn parse_color(s: &str) -> Result<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            bail!("invalid hex color '{s}': expected 6 hex digits after '#'");
        }
        let r = u8::from_str_radix(&hex[0..2], 16)
            .map_err(|_| anyhow::anyhow!("invalid hex color '{s}': bad red component"))?;
        let g = u8::from_str_radix(&hex[2..4], 16)
            .map_err(|_| anyhow::anyhow!("invalid hex color '{s}': bad green component"))?;
        let b = u8::from_str_radix(&hex[4..6], 16)
            .map_err(|_| anyhow::anyhow!("invalid hex color '{s}': bad blue component"))?;
        return Ok(Color::Rgb(r, g, b));
    }

    match s.to_ascii_lowercase().as_str() {
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "white" => Ok(Color::White),
        "gray" | "grey" => Ok(Color::Gray),
        "darkgray" | "darkgrey" => Ok(Color::DarkGray),
        "lightred" => Ok(Color::LightRed),
        "lightgreen" => Ok(Color::LightGreen),
        "lightyellow" => Ok(Color::LightYellow),
        "lightblue" => Ok(Color::LightBlue),
        "lightmagenta" => Ok(Color::LightMagenta),
        "lightcyan" => Ok(Color::LightCyan),
        "reset" => Ok(Color::Reset),
        _ => bail!(
            "unknown color '{s}': expected a hex value like '#ff5500' or a named color \
             (black, red, green, yellow, blue, magenta, cyan, white, gray, darkgray, \
             lightred, lightgreen, lightyellow, lightblue, lightmagenta, lightcyan, reset)"
        ),
    }
}

/// Per-token color overrides from `[ui.colors]` in `.ralph.toml`.
///
/// Each field is optional. When set, it overrides the corresponding token
/// from the base theme. Unset fields fall back to the base theme's value.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ColorOverrides {
    pub background: Option<String>,
    pub border: Option<String>,
    pub title: Option<String>,
    pub status: Option<String>,
    pub subdued: Option<String>,
    pub info: Option<String>,
    pub warn: Option<String>,
    pub error: Option<String>,
    pub dim_overlay: Option<String>,
    pub modal_text: Option<String>,
    pub input_inactive: Option<String>,
    pub input_text: Option<String>,
    pub modal_border: Option<String>,
    pub cursor_fg: Option<String>,
    pub cursor_bg: Option<String>,
}

impl ColorOverrides {
    /// Validate all color values, returning an error for the first invalid one.
    pub fn validate(&self) -> Result<()> {
        let fields: &[(&str, &Option<String>)] = &[
            ("background", &self.background),
            ("border", &self.border),
            ("title", &self.title),
            ("status", &self.status),
            ("subdued", &self.subdued),
            ("info", &self.info),
            ("warn", &self.warn),
            ("error", &self.error),
            ("dim_overlay", &self.dim_overlay),
            ("modal_text", &self.modal_text),
            ("input_inactive", &self.input_inactive),
            ("input_text", &self.input_text),
            ("modal_border", &self.modal_border),
            ("cursor_fg", &self.cursor_fg),
            ("cursor_bg", &self.cursor_bg),
        ];
        for (name, value) in fields {
            if let Some(v) = value {
                parse_color(v)
                    .map_err(|e| anyhow::anyhow!("invalid color for ui.colors.{name}: {e}"))?;
            }
        }
        Ok(())
    }

    /// Apply overrides to a base theme, returning a new theme with overrides merged in.
    fn apply_to(&self, mut theme: Theme) -> Theme {
        fn set(target: &mut Color, value: &Option<String>) {
            if let Some(v) = value {
                // Already validated, so unwrap is safe here.
                if let Ok(c) = parse_color(v) {
                    *target = c;
                }
            }
        }
        set(&mut theme.background, &self.background);
        set(&mut theme.border_fg, &self.border);
        set(&mut theme.title_fg, &self.title);
        set(&mut theme.status_fg, &self.status);
        set(&mut theme.subdued_fg, &self.subdued);
        set(&mut theme.info_fg, &self.info);
        set(&mut theme.warn_fg, &self.warn);
        set(&mut theme.error_fg, &self.error);
        set(&mut theme.dim_overlay_fg, &self.dim_overlay);
        set(&mut theme.modal_text_fg, &self.modal_text);
        set(&mut theme.input_inactive_fg, &self.input_inactive);
        set(&mut theme.input_text_fg, &self.input_text);
        set(&mut theme.modal_border_fg, &self.modal_border);
        set(&mut theme.cursor_fg, &self.cursor_fg);
        set(&mut theme.cursor_bg, &self.cursor_bg);
        theme
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
    ThemeName::parse(config_theme).unwrap_or(ThemeName::Dark)
}

/// Initialize the active theme with optional color overrides.
///
/// Call once at startup before any rendering. If called multiple times,
/// subsequent calls are ignored (first write wins). Color overrides should
/// be validated with `ColorOverrides::validate()` before calling this.
pub fn init_with_overrides(name: ThemeName, overrides: Option<&ColorOverrides>) {
    let mut theme = match name {
        ThemeName::Light => Theme::light(),
        ThemeName::Dark => Theme::dark(),
    };
    if let Some(ov) = overrides {
        theme = ov.apply_to(theme);
    }
    let _ = ACTIVE_THEME.set(theme);
}

/// Initialize the active theme without color overrides.
/// Convenience wrapper around `init_with_overrides`.
#[allow(dead_code)]
pub fn init(name: ThemeName) {
    init_with_overrides(name, None);
}

/// Get the active theme. Falls back to dark if not initialized.
fn active() -> &'static Theme {
    ACTIVE_THEME.get_or_init(Theme::dark)
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

/// Style for user-typed input text. Uses `Color::Reset` by default so
/// the terminal's native foreground is used, readable on any background.
pub fn input_text() -> Style {
    let t = active();
    Style::default().fg(t.input_text_fg).bg(t.background)
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
    fn dark_theme_uses_reset_background() {
        let t = Theme::dark();
        assert_eq!(t.background, Color::Reset);
        assert_eq!(t.title_fg, Color::Cyan);
        assert_eq!(t.border_fg, Color::Rgb(100, 100, 100));
    }

    #[test]
    fn light_theme_uses_reset_background() {
        let t = Theme::light();
        assert_eq!(t.background, Color::Reset);
        assert_eq!(t.title_fg, Color::Blue);
    }

    #[test]
    fn resolve_falls_back_to_dark() {
        // With no env var set, unknown config value falls back to dark.
        std::env::remove_var("RALPH_THEME");
        assert_eq!(resolve_theme_name("invalid"), ThemeName::Dark);
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

    #[test]
    fn parse_color_hex_valid() {
        assert_eq!(parse_color("#ff5500").unwrap(), Color::Rgb(255, 85, 0));
        assert_eq!(parse_color("#000000").unwrap(), Color::Rgb(0, 0, 0));
        assert_eq!(parse_color("#FFFFFF").unwrap(), Color::Rgb(255, 255, 255));
        assert_eq!(parse_color("#aaBBcc").unwrap(), Color::Rgb(170, 187, 204));
    }

    #[test]
    fn parse_color_hex_invalid() {
        assert!(parse_color("#fff").is_err()); // too short
        assert!(parse_color("#gggggg").is_err()); // invalid hex chars
        assert!(parse_color("#ff550").is_err()); // 5 digits
        assert!(parse_color("#ff55001").is_err()); // 7 digits
    }

    #[test]
    fn parse_color_named() {
        assert_eq!(parse_color("black").unwrap(), Color::Black);
        assert_eq!(parse_color("Red").unwrap(), Color::Red);
        assert_eq!(parse_color("GREEN").unwrap(), Color::Green);
        assert_eq!(parse_color("cyan").unwrap(), Color::Cyan);
        assert_eq!(parse_color("darkgray").unwrap(), Color::DarkGray);
        assert_eq!(parse_color("darkgrey").unwrap(), Color::DarkGray);
        assert_eq!(parse_color("gray").unwrap(), Color::Gray);
        assert_eq!(parse_color("grey").unwrap(), Color::Gray);
        assert_eq!(parse_color("lightblue").unwrap(), Color::LightBlue);
        assert_eq!(parse_color("reset").unwrap(), Color::Reset);
    }

    #[test]
    fn parse_color_unknown_name() {
        let err = parse_color("orange").unwrap_err().to_string();
        assert!(err.contains("unknown color 'orange'"), "got: {err}");
        assert!(err.contains("hex value"), "error should suggest hex format");
    }

    #[test]
    fn parse_color_trims_whitespace() {
        assert_eq!(parse_color("  blue  ").unwrap(), Color::Blue);
        assert_eq!(parse_color("  #ff0000  ").unwrap(), Color::Rgb(255, 0, 0));
    }

    #[test]
    fn color_overrides_validate_valid() {
        let ov = ColorOverrides {
            border: Some("#ff5500".to_string()),
            title: Some("cyan".to_string()),
            ..Default::default()
        };
        assert!(ov.validate().is_ok());
    }

    #[test]
    fn color_overrides_validate_invalid() {
        let ov = ColorOverrides {
            border: Some("not-a-color".to_string()),
            ..Default::default()
        };
        let err = ov.validate().unwrap_err().to_string();
        assert!(err.contains("ui.colors.border"), "got: {err}");
    }

    #[test]
    fn color_overrides_empty_is_noop() {
        let ov = ColorOverrides::default();
        let base = Theme::dark();
        let themed = ov.apply_to(base.clone());
        assert_eq!(themed.border_fg, base.border_fg);
        assert_eq!(themed.title_fg, base.title_fg);
        assert_eq!(themed.background, base.background);
    }

    #[test]
    fn color_overrides_partial_apply() {
        let ov = ColorOverrides {
            border: Some("#ff0000".to_string()),
            title: Some("magenta".to_string()),
            ..Default::default()
        };
        let themed = ov.apply_to(Theme::light());
        assert_eq!(themed.border_fg, Color::Rgb(255, 0, 0));
        assert_eq!(themed.title_fg, Color::Magenta);
        // Unset fields keep the base theme value.
        assert_eq!(themed.status_fg, Theme::light().status_fg);
        assert_eq!(themed.background, Theme::light().background);
    }

    #[test]
    fn color_overrides_all_fields() {
        let ov = ColorOverrides {
            background: Some("#111111".to_string()),
            border: Some("#222222".to_string()),
            title: Some("#333333".to_string()),
            status: Some("#444444".to_string()),
            subdued: Some("#555555".to_string()),
            info: Some("#666666".to_string()),
            warn: Some("#777777".to_string()),
            error: Some("#888888".to_string()),
            dim_overlay: Some("#999999".to_string()),
            modal_text: Some("#aaaaaa".to_string()),
            input_inactive: Some("#bbbbbb".to_string()),
            input_text: Some("#ab1234".to_string()),
            modal_border: Some("#cccccc".to_string()),
            cursor_fg: Some("#dddddd".to_string()),
            cursor_bg: Some("#eeeeee".to_string()),
        };
        assert!(ov.validate().is_ok());
        let themed = ov.apply_to(Theme::dark());
        assert_eq!(themed.background, Color::Rgb(0x11, 0x11, 0x11));
        assert_eq!(themed.border_fg, Color::Rgb(0x22, 0x22, 0x22));
        assert_eq!(themed.cursor_bg, Color::Rgb(0xee, 0xee, 0xee));
    }
}

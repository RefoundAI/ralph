//! Color and style tokens for the ratatui dashboard.
//!
//! Uses explicit foreground + background pairs so the UI is readable
//! regardless of the user's terminal theme (light or dark).

use ratatui::style::{Color, Modifier, Style};

use crate::ui::event::UiLevel;

/// Dark base background used for all panels.
const BG: Color = Color::Black;

pub fn border() -> Style {
    Style::default().fg(Color::DarkGray).bg(BG)
}

pub fn title() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .bg(BG)
        .add_modifier(Modifier::BOLD)
}

pub fn status() -> Style {
    Style::default()
        .fg(Color::Green)
        .bg(BG)
        .add_modifier(Modifier::BOLD)
}

pub fn subdued() -> Style {
    Style::default().fg(Color::Rgb(180, 180, 180)).bg(BG)
}

pub fn level(level: UiLevel) -> Style {
    match level {
        UiLevel::Info => Style::default().fg(Color::White).bg(BG),
        UiLevel::Warn => Style::default().fg(Color::Yellow).bg(BG),
        UiLevel::Error => Style::default()
            .fg(Color::Red)
            .bg(BG)
            .add_modifier(Modifier::BOLD),
    }
}

/// Style for the dim overlay behind modals.
pub fn dim_overlay() -> Style {
    Style::default().fg(Color::DarkGray).bg(BG)
}

/// Style for modal text content.
pub fn modal_text() -> Style {
    Style::default()
        .fg(Color::White)
        .bg(BG)
        .add_modifier(Modifier::BOLD)
}

/// Style for modal borders.
pub fn modal_border() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .bg(BG)
        .add_modifier(Modifier::BOLD)
}

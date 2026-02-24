//! Rendering functions for the ratatui dashboard.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::ui::state::{AppState, UiModal, UiScreen};
use crate::ui::theme;

/// Draw one frame of the UI.
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    // Paint the entire frame black so no terminal background bleeds through.
    let bg = Block::default().style(Style::default().bg(Color::Black));
    frame.render_widget(bg, frame.area());

    match &state.screen {
        UiScreen::Dashboard => render_dashboard(frame, state),
        UiScreen::Explorer {
            title,
            lines,
            scroll,
        } => render_explorer(frame, title, lines, *scroll),
    }

    if let Some(modal) = &state.modal {
        render_modal(frame, modal);
    }
}

fn render_dashboard(frame: &mut Frame<'_>, state: &AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Ralph", theme::title()),
            Span::raw("  "),
            Span::styled(&state.status_line, theme::status()),
        ]),
        Line::from(vec![
            Span::styled(&state.dag_summary, theme::subdued()),
            Span::raw("  "),
            Span::styled(&state.current_task, theme::subdued()),
        ]),
    ])
    .block(
        Block::default()
            .title("Run")
            .borders(Borders::ALL)
            .border_style(theme::border()),
    );
    frame.render_widget(header, root[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(root[1]);

    let logs: Vec<ListItem<'_>> = state
        .logs
        .iter()
        .rev()
        .take(120)
        .map(|line| ListItem::new(Line::styled(line.message.clone(), theme::level(line.level))))
        .collect();
    let logs_panel = List::new(logs).block(
        Block::default()
            .title("Events")
            .borders(Borders::ALL)
            .border_style(theme::border()),
    );
    frame.render_widget(logs_panel, body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(body[1]);

    // Agent Stream: respect user scroll pin, or auto-scroll to bottom.
    // Count wrapped visual lines so auto-scroll reaches the actual bottom.
    let inner_height = right[0].height.saturating_sub(2) as usize; // subtract border
    let inner_width = right[0].width.saturating_sub(2).max(1) as usize; // subtract border
    let total_lines: usize = state
        .agent_text
        .lines()
        .map(|line| {
            let char_count = line.len();
            if char_count == 0 {
                1
            } else {
                (char_count + inner_width - 1) / inner_width
            }
        })
        .sum::<usize>()
        .max(if state.agent_text.is_empty() { 0 } else { 1 });
    let max_offset = total_lines.saturating_sub(inner_height);
    let scroll_offset = match state.agent_scroll {
        Some(pinned) => pinned.min(max_offset),
        None => max_offset, // auto-scroll to bottom
    };
    let scroll_indicator = if state.agent_scroll.is_some() {
        format!("Agent Stream [scroll {}/{}]", scroll_offset, max_offset)
    } else {
        "Agent Stream".to_string()
    };
    let agent = Paragraph::new(state.agent_text.as_str())
        .block(
            Block::default()
                .title(scroll_indicator)
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset as u16, 0))
        .style(theme::subdued());
    frame.render_widget(agent, right[0]);

    let tool_items: Vec<ListItem<'_>> = state
        .tools
        .iter()
        .rev()
        .take(80)
        .map(|line| ListItem::new(Line::styled(line.clone(), theme::subdued())))
        .collect();
    let tools = List::new(tool_items).block(
        Block::default()
            .title("Tool Activity")
            .borders(Borders::ALL)
            .border_style(theme::border()),
    );
    frame.render_widget(tools, right[1]);

    let footer = Paragraph::new(
        "↑/↓ scroll agent stream · End resume auto-scroll · --no-ui for plain output",
    )
    .style(theme::subdued());
    frame.render_widget(footer, root[2]);
}

fn render_explorer(frame: &mut Frame<'_>, title: &str, lines: &[String], scroll: usize) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let body_height = root[0].height.saturating_sub(2) as usize;
    let slice = lines
        .iter()
        .skip(scroll)
        .take(body_height.max(1))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    let body = Paragraph::new(slice)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .style(theme::subdued())
        .wrap(Wrap { trim: false });
    frame.render_widget(body, root[0]);

    let footer = Paragraph::new("Explorer: \u{2191}/\u{2193} scroll, q/Esc/Enter close")
        .style(theme::subdued());
    frame.render_widget(footer, root[1]);
}

fn render_modal(frame: &mut Frame<'_>, modal: &UiModal) {
    // Clear the entire screen first, then paint a dim background so no
    // dashboard text bleeds through around the modal edges.
    let full = frame.area();
    frame.render_widget(Clear, full);
    let dim = Block::default().style(theme::dim_overlay());
    frame.render_widget(dim, full);

    let area = centered_rect(78, 60, full);
    frame.render_widget(Clear, area);

    match modal {
        UiModal::Multiline {
            title,
            hint,
            lines,
            current_line,
        } => {
            // Build structured content with visual differentiation:
            // 1. Hint text (subdued)
            // 2. Separator
            // 3. Input area with prompt markers
            // 4. Hint bar at bottom
            let mut content: Vec<Line<'_>> = Vec::new();

            // Hint text in subdued style
            for hint_line in hint.lines() {
                content.push(Line::from(Span::styled(hint_line, theme::subdued())));
            }
            content.push(Line::from(""));
            content.push(Line::from(Span::styled(
                "─── Input ───────────────────────────────────────────",
                Style::default().fg(Color::DarkGray),
            )));

            // Committed lines with prompt markers
            for line in lines {
                content.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(line.as_str(), theme::modal_text()),
                ]));
            }

            // Current line with cursor marker
            content.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                Span::styled(current_line.as_str(), theme::modal_text()),
                Span::styled("█", Style::default().fg(Color::Cyan)),
            ]));

            content.push(Line::from(""));
            content.push(Line::from(Span::styled(
                "Enter=newline  Empty Enter=submit  Esc=exit  Ctrl+C=interrupt",
                Style::default().fg(Color::DarkGray),
            )));

            let widget = Paragraph::new(content)
                .block(
                    Block::default()
                        .title(title.as_str())
                        .borders(Borders::ALL)
                        .border_style(theme::modal_border()),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(widget, area);

            // Place the real terminal cursor at the typing position.
            // inner area = area minus 1-cell border on each side.
            let inner_x = area.x + 1;
            let inner_y = area.y + 1;
            // The cursor sits after "│ " (2 chars) + current_line length,
            // offset by the number of content lines above the current line.
            let lines_above = hint.lines().count() // hint lines
                + 1  // blank line
                + 1  // separator
                + lines.len(); // committed input lines
            let cursor_col = 2 + current_line.len() as u16;
            let cursor_x = inner_x + cursor_col;
            let cursor_y = inner_y + lines_above as u16;
            // Only set cursor if it fits within the modal area
            if cursor_x < area.x + area.width - 1 && cursor_y < area.y + area.height - 1 {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }
        UiModal::Confirm {
            title,
            prompt,
            default_yes,
        } => {
            let default = if *default_yes { "Y/n" } else { "y/N" };
            let text = format!("{prompt}\n\n[y] yes   [n] no   [Enter] default ({default})");
            let widget = Paragraph::new(text)
                .block(
                    Block::default()
                        .title(title.as_str())
                        .borders(Borders::ALL)
                        .border_style(theme::modal_border()),
                )
                .style(theme::modal_text())
                .wrap(Wrap { trim: false });
            frame.render_widget(widget, area);
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::event::{UiEvent, UiLevel};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn buffer_text(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn dashboard_renders_core_panels() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        state.apply(UiEvent::StatusLine("Iteration 2".to_string()));
        state.apply(UiEvent::Log {
            level: UiLevel::Info,
            message: "hello".to_string(),
        });
        terminal.draw(|f| render(f, &state)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Run"));
        assert!(text.contains("Events"));
        assert!(text.contains("Agent Stream"));
    }

    #[test]
    fn explorer_screen_renders_title() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        state.show_explorer(
            "Task Explorer".to_string(),
            vec!["t-1 done foo".to_string(), "t-2 pending bar".to_string()],
        );
        terminal.draw(|f| render(f, &state)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Task Explorer"));
        assert!(text.contains("Explorer:"));
    }

    #[test]
    fn modal_renders_over_base_screen() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        state.modal = Some(UiModal::Confirm {
            title: "Confirm".to_string(),
            prompt: "Delete?".to_string(),
            default_yes: false,
        });
        terminal.draw(|f| render(f, &state)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Confirm"));
        assert!(text.contains("Delete?"));
    }
}

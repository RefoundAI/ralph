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

    // Left column: Events (60%) over Tool Activity (40%).
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(body[0]);

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
    frame.render_widget(logs_panel, left[0]);

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
    frame.render_widget(tools, left[1]);

    // Right column: Agent Stream (fills remaining) over Input (dynamic height).
    let right_column_height = body[1].height;
    let input_height: u16 = if !state.input_active {
        3 // inactive: border + hint line + border
    } else if state.input_choices.is_none() {
        // Active free-text: hint_lines + committed_lines + 4 (border, current line, hint bar, border)
        let hint_lines = state.input_hint.lines().count().max(1) as u16;
        let committed_lines = state.input_lines.len() as u16;
        let raw = hint_lines + committed_lines + 4;
        let cap = (right_column_height / 4).max(5);
        raw.min(cap)
    } else {
        3 // choice mode: handled in Phase 4
    };
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(input_height)])
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

    render_input_pane(frame, right[1], state);

    let footer_text = if state.input_active && state.input_choices.is_some() {
        "PgUp/PgDn scroll agent stream · ↑/↓ navigate choices · 1-9 quick-select · Esc exit"
    } else {
        "↑/↓ scroll agent stream · End resume auto-scroll · --no-ui for plain output"
    };
    let footer = Paragraph::new(footer_text).style(theme::subdued());
    frame.render_widget(footer, root[2]);
}

/// Render the persistent Input pane in the bottom-right of the dashboard.
fn render_input_pane(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if !state.input_active {
        // Inactive: grayed-out hint text with subdued border.
        let widget = Paragraph::new(state.input_hint.as_str())
            .block(
                Block::default()
                    .title(state.input_title.as_str())
                    .borders(Borders::ALL)
                    .border_style(theme::input_inactive()),
            )
            .style(theme::input_inactive());
        frame.render_widget(widget, area);
    } else if state.input_choices.is_none() {
        // Active free-text: hint, committed lines, current line with cursor, hint bar.
        let mut lines: Vec<Line<'_>> = Vec::new();

        // Hint text in subdued style.
        for hint_line in state.input_hint.lines() {
            lines.push(Line::styled(hint_line.to_string(), theme::subdued()));
        }

        // Committed lines with '│ ' prefix.
        for committed in &state.input_lines {
            lines.push(Line::from(vec![
                Span::styled("│ ", theme::subdued()),
                Span::styled(committed.clone(), theme::modal_text()),
            ]));
        }

        // Current line with '│ ' prefix and '█' cursor block.
        lines.push(Line::from(vec![
            Span::styled("│ ", theme::subdued()),
            Span::styled(state.input_current_line.clone(), theme::modal_text()),
            Span::styled("█", theme::title()),
        ]));

        // Blank line then hint bar.
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "Enter=newline  Empty Enter=submit  Esc=exit  Ctrl+C=interrupt",
            theme::subdued(),
        ));

        let widget = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(state.input_title.as_str())
                    .borders(Borders::ALL)
                    .border_style(theme::modal_border()),
            )
            .style(theme::subdued());
        frame.render_widget(widget, area);

        // Cursor positioning: place terminal cursor at the typing position.
        let pane_inner = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        if pane_inner.width > 0 && pane_inner.height > 0 {
            let hint_lines = state.input_hint.lines().count().max(1) as u16;
            let committed_lines = state.input_lines.len() as u16;
            let cursor_x = pane_inner
                .x
                .saturating_add(2 + state.input_current_line.len() as u16)
                .min(pane_inner.x + pane_inner.width.saturating_sub(1));
            let cursor_y = pane_inner.y + hint_lines + committed_lines;
            if cursor_x >= pane_inner.x
                && cursor_x < pane_inner.x + pane_inner.width
                && cursor_y >= pane_inner.y
                && cursor_y < pane_inner.y + pane_inner.height
            {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }
    } else {
        // Active choice mode: deferred to Phase 4.
        let widget = Paragraph::new(state.input_hint.as_str())
            .block(
                Block::default()
                    .title(state.input_title.as_str())
                    .borders(Borders::ALL)
                    .border_style(theme::modal_border()),
            )
            .style(theme::subdued());
        frame.render_widget(widget, area);
    }
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
        assert!(text.contains("Tool Activity"));
        assert!(text.contains("Input"));
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

    #[test]
    fn input_pane_does_not_panic_on_tiny_frame() {
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();
        terminal.draw(|f| render(f, &state)).unwrap();
    }

    #[test]
    fn dashboard_renders_input_pane_inactive() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();
        terminal.draw(|f| render(f, &state)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Input"), "Input title should appear");
        assert!(
            text.contains("Waiting for agent..."),
            "Inactive hint should appear"
        );
    }

    #[test]
    fn dashboard_renders_input_pane_active_freetext() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        state.input_active = true;
        state.input_title = "Interactive Prompt".to_string();
        state.input_hint = "Type your response".to_string();
        state.input_lines = vec!["hello".to_string(), "world".to_string()];
        state.input_current_line = "typing".to_string();
        terminal.draw(|f| render(f, &state)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        // Committed lines should appear with '│ ' prefix.
        assert!(
            text.contains("│ hello"),
            "First committed line should appear: {text}"
        );
        assert!(
            text.contains("│ world"),
            "Second committed line should appear: {text}"
        );
        // Current line should show with cursor block.
        assert!(
            text.contains("typing█"),
            "Current line with cursor block should appear: {text}"
        );
        // Active border title should appear.
        assert!(
            text.contains("Interactive Prompt"),
            "Active input title should appear: {text}"
        );
    }

    #[test]
    fn dashboard_renders_tool_activity_in_left_column() {
        let width = 100u16;
        let backend = TestBackend::new(width, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        state.apply(UiEvent::ToolActivity("tool_call: Read".to_string()));
        terminal.draw(|f| render(f, &state)).unwrap();
        let buf = terminal.backend().buffer();

        // Find the x-coordinate of "Tool Activity" title in the buffer.
        let title = "Tool Activity";
        let mut found_x = None;
        'outer: for y in 0..buf.area.height {
            for x in 0..buf.area.width.saturating_sub(title.len() as u16) {
                let mut matched = true;
                for (i, ch) in title.chars().enumerate() {
                    if buf[(x + i as u16, y)].symbol().chars().next() != Some(ch) {
                        matched = false;
                        break;
                    }
                }
                if matched {
                    found_x = Some(x);
                    break 'outer;
                }
            }
        }
        let x = found_x.expect("'Tool Activity' title should be present in the buffer");
        // Left column is 42% of terminal width. The title should start within that region.
        let left_column_max = (width as f64 * 0.42) as u16 + 1;
        assert!(
            x < left_column_max,
            "Tool Activity title at x={x} should be within left 42% (max {left_column_max})"
        );
    }
}

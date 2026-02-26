//! Rendering functions for the ratatui dashboard.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::ui::state::{AppState, UiModal, UiScreen};
use crate::ui::theme;

/// Draw one frame of the UI.
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    // Paint the entire frame with the theme background so no terminal background bleeds through.
    let bg = Block::default().style(Style::default().bg(theme::background()));
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
        // Active free-text: hint_lines + wrapped_text_lines + 3 (border, hint bar, border)
        // Calculate available inner width for wrapping estimate.
        let est_inner = body[1].width.saturating_sub(2).max(1) as usize;
        let hint_lines = state.input_hint.lines().count().max(1) as u16;
        let text_visual_lines = count_wrapped_lines(&state.input_text, est_inner).max(1) as u16;
        let raw = hint_lines + text_visual_lines + 3; // borders + hint bar
        let cap = (right_column_height / 2).max(5);
        raw.min(cap)
    } else {
        // Active choice mode: num_choices + 4 (top border, hint line, bottom hint bar, bottom border)
        let num_choices = state.input_choices.as_ref().map(|c| c.len()).unwrap_or(0) as u16;
        let raw = num_choices + 4;
        let cap = (right_column_height * 2 / 5).max(5);
        raw.min(cap)
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
    } else if state.input_active {
        "Enter=submit · Shift+Enter=newline · ↑/↓/←/→ navigate · PgUp/PgDn scroll agent"
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
        // Active free-text: hint, input text with cursor, hint bar.
        let pane_inner = area.inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 1,
        });
        let inner_w = pane_inner.width.max(1) as usize;

        let mut lines: Vec<Line<'_>> = Vec::new();

        // Hint text in subdued style.
        let hint_line_count = state.input_hint.lines().count().max(1);
        for hint_line in state.input_hint.lines() {
            lines.push(Line::styled(hint_line.to_string(), theme::subdued()));
        }

        // Build visual lines by walking through the text with wrapping.
        // We need to compute cursor row/col for terminal cursor positioning.
        let mut cursor_row: u16 = 0;
        let mut cursor_col: u16 = 0;
        let mut visual_row: u16 = 0;

        // Process each logical line of the input text.
        for (line_idx, logical_line) in state.input_text.split('\n').enumerate() {
            let line_start_in_text = if line_idx == 0 {
                0
            } else {
                // Sum of all previous lines + their '\n' chars
                state
                    .input_text
                    .split('\n')
                    .take(line_idx)
                    .map(|l| l.len() + 1)
                    .sum::<usize>()
            };

            // Does the cursor fall within this logical line?
            let cursor_in_line = state.input_cursor >= line_start_in_text
                && state.input_cursor <= line_start_in_text + logical_line.len();

            // Visual wrap this line.
            if logical_line.is_empty() {
                if cursor_in_line {
                    cursor_row = visual_row;
                    cursor_col = 0;
                    lines.push(Line::from(vec![
                        Span::styled(" ", theme::cursor()),
                        Span::raw(""),
                    ]));
                } else {
                    lines.push(Line::from(""));
                }
                visual_row += 1;
            } else {
                // Break logical line into visual rows based on inner_w.
                let mut col = 0;
                let mut row_start = 0;
                let cursor_offset_in_line = if cursor_in_line {
                    state.input_cursor - line_start_in_text
                } else {
                    usize::MAX
                };

                for (byte_idx, _ch) in logical_line.char_indices() {
                    let ch_w = 1; // assume 1 column per char for simplicity
                    if col >= inner_w {
                        // Emit wrapped row.
                        let row_text = &logical_line[row_start..byte_idx];
                        if cursor_in_line
                            && cursor_offset_in_line >= row_start
                            && cursor_offset_in_line < byte_idx
                        {
                            // Cursor is in this row.
                            let local_off = cursor_offset_in_line - row_start;
                            cursor_row = visual_row;
                            cursor_col = local_off as u16;
                            let (pre, post) = row_text.split_at(local_off);
                            let (_, post_rest) = if post.is_empty() {
                                (" ", "")
                            } else {
                                let ch = post.chars().next().unwrap();
                                post.split_at(ch.len_utf8())
                            };
                            lines.push(Line::from(vec![
                                Span::styled(pre.to_string(), theme::modal_text()),
                                Span::styled(
                                    if post.is_empty() {
                                        " ".to_string()
                                    } else {
                                        post.chars().next().unwrap().to_string()
                                    },
                                    theme::cursor(),
                                ),
                                Span::styled(post_rest.to_string(), theme::modal_text()),
                            ]));
                        } else {
                            lines.push(Line::styled(row_text.to_string(), theme::modal_text()));
                        }
                        visual_row += 1;
                        row_start = byte_idx;
                        col = 0;
                    }
                    col += ch_w;
                }

                // Emit the last (or only) row of this logical line.
                let row_text = &logical_line[row_start..];
                if cursor_in_line && cursor_offset_in_line >= row_start {
                    let local_off = cursor_offset_in_line - row_start;
                    cursor_row = visual_row;
                    cursor_col = local_off as u16;
                    if local_off < row_text.len() {
                        let (pre, post) = row_text.split_at(local_off);
                        let cursor_ch = post.chars().next().unwrap();
                        let post_rest = &post[cursor_ch.len_utf8()..];
                        lines.push(Line::from(vec![
                            Span::styled(pre.to_string(), theme::modal_text()),
                            Span::styled(cursor_ch.to_string(), theme::cursor()),
                            Span::styled(post_rest.to_string(), theme::modal_text()),
                        ]));
                    } else {
                        // Cursor at end of line.
                        lines.push(Line::from(vec![
                            Span::styled(row_text.to_string(), theme::modal_text()),
                            Span::styled(" ", theme::cursor()),
                        ]));
                    }
                } else {
                    lines.push(Line::styled(row_text.to_string(), theme::modal_text()));
                }
                visual_row += 1;
            }
        }

        // If text is empty, show cursor on its own.
        if state.input_text.is_empty() {
            cursor_row = 0;
            cursor_col = 0;
            // The empty-text line was already handled above (empty split produces one "").
        }

        // Hint bar.
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "Enter=submit  Shift+Enter=newline  Esc=exit  Ctrl+C=interrupt",
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

        // Position terminal cursor for blinking effect.
        if pane_inner.width > 0 && pane_inner.height > 0 {
            let abs_cursor_y = pane_inner.y + (hint_line_count as u16) + cursor_row;
            let abs_cursor_x = pane_inner.x + cursor_col;
            if abs_cursor_x < pane_inner.x + pane_inner.width
                && abs_cursor_y < pane_inner.y + pane_inner.height
            {
                frame.set_cursor_position((abs_cursor_x, abs_cursor_y));
            }
        }
    } else {
        // Active choice mode: numbered list with highlighted cursor.
        let choices = state.input_choices.as_ref().unwrap();
        let mut lines: Vec<Line<'_>> = Vec::new();

        for (i, choice) in choices.iter().enumerate() {
            let number = i + 1;
            if i == state.input_choice_cursor {
                lines.push(Line::styled(
                    format!("> {number}. {choice}"),
                    theme::title(),
                ));
            } else {
                lines.push(Line::styled(
                    format!("  {number}. {choice}"),
                    theme::subdued(),
                ));
            }
        }

        // Bottom hint bar.
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "1-9 select · ↑/↓ navigate · Enter confirm · type for custom",
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
        // No cursor positioning in choice mode — selection is visual via highlight.
    }
}

/// Count the total visual lines that `text` occupies when wrapped at `width` columns.
fn count_wrapped_lines(text: &str, width: usize) -> usize {
    if text.is_empty() {
        return 1;
    }
    let w = width.max(1);
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                1
            } else {
                (line.len() + w - 1) / w
            }
        })
        .sum()
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
        state.input_text = "hello\nworld\ntyping".to_string();
        state.input_cursor = state.input_text.len(); // cursor at end
        terminal.draw(|f| render(f, &state)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        // All lines should appear in the input.
        assert!(text.contains("hello"), "First line should appear: {text}");
        assert!(text.contains("world"), "Second line should appear: {text}");
        assert!(text.contains("typing"), "Third line should appear: {text}");
        // Active border title should appear.
        assert!(
            text.contains("Interactive Prompt"),
            "Active input title should appear: {text}"
        );
    }

    #[test]
    fn dashboard_renders_input_pane_active_choices() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        state.input_active = true;
        state.input_title = "Choose Option".to_string();
        state.input_hint = "Pick one".to_string();
        state.input_choices = Some(vec!["Option A".to_string(), "Option B".to_string()]);
        state.input_choice_cursor = 1; // Option B is highlighted
        terminal.draw(|f| render(f, &state)).unwrap();
        let text = buffer_text(terminal.backend().buffer());

        // Both choices should appear with numbers.
        assert!(
            text.contains("1. Option A"),
            "First choice should appear: {text}"
        );
        assert!(
            text.contains("2. Option B"),
            "Second choice should appear: {text}"
        );

        // Highlighted choice (Option B at cursor=1) should have '>' prefix.
        assert!(
            text.contains("> 2. Option B"),
            "Highlighted choice should have '>' prefix: {text}"
        );

        // Non-highlighted choice (Option A) should have '  ' prefix (no '>').
        assert!(
            !text.contains("> 1. Option A"),
            "Non-highlighted choice should NOT have '>' prefix: {text}"
        );

        // Border title should appear.
        assert!(
            text.contains("Choose Option"),
            "Choice mode title should appear: {text}"
        );

        // Verify highlighted choice has distinct styling by checking the buffer cells.
        let title_fg = theme::title().fg.unwrap();
        let buf = terminal.backend().buffer();
        let mut highlight_found = false;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width.saturating_sub(1) {
                if buf[(x, y)].symbol() == ">" {
                    // Check that cells in the highlighted row use the title foreground color.
                    let cell = &buf[(x, y)];
                    if cell.fg == title_fg {
                        highlight_found = true;
                    }
                    break;
                }
            }
            if highlight_found {
                break;
            }
        }
        assert!(
            highlight_found,
            "Highlighted choice should use theme::title() foreground color"
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

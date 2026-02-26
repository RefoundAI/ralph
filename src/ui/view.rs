//! Rendering functions for the ratatui dashboard.

use ratatui::prelude::*;
use ratatui::style::Modifier;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::ui::state::{AppState, FrameAreas, UiModal, UiScreen};
use crate::ui::theme;

/// Draw one frame of the UI.
pub fn render(frame: &mut Frame<'_>, state: &AppState, areas: &mut FrameAreas) {
    // Paint the entire frame with the theme background so no terminal background bleeds through.
    let bg = Block::default().style(Style::default().bg(theme::background()));
    frame.render_widget(bg, frame.area());

    match &state.screen {
        UiScreen::Dashboard => render_dashboard(frame, state, areas),
        UiScreen::Explorer {
            title,
            lines,
            scroll,
        } => {
            *areas = FrameAreas::default();
            render_explorer(frame, title, lines, *scroll);
        }
    }

    if let Some(modal) = &state.modal {
        render_modal(frame, modal);
    }
}

fn render_dashboard(frame: &mut Frame<'_>, state: &AppState, areas: &mut FrameAreas) {
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

    // Events panel with scroll support.
    let log_lines: Vec<Line<'_>> = state
        .logs
        .iter()
        .rev()
        .take(120)
        .map(|line| Line::styled(line.message.clone(), theme::level(line.level)))
        .collect();
    let logs_inner_h = left[0].height.saturating_sub(2) as usize;
    let logs_max_offset = log_lines.len().saturating_sub(logs_inner_h);
    let logs_scroll_clamped = state.logs_scroll.min(logs_max_offset);
    let logs_title = if logs_scroll_clamped > 0 {
        format!(
            "Events [scroll {}/{}]",
            logs_scroll_clamped, logs_max_offset
        )
    } else {
        "Events".to_string()
    };
    let logs_panel = Paragraph::new(log_lines)
        .block(
            Block::default()
                .title(logs_title)
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .scroll((logs_scroll_clamped as u16, 0));
    frame.render_widget(logs_panel, left[0]);
    areas.logs = Some(left[0]);

    // Tool Activity panel with scroll support.
    let tool_lines: Vec<Line<'_>> = state
        .tools
        .iter()
        .rev()
        .take(80)
        .map(|tl| {
            if tl.name.is_empty() {
                // Detail line (indented, no tool name).
                Line::from(vec![
                    Span::styled("  ", theme::subdued()),
                    Span::styled(tl.summary.clone(), theme::subdued()),
                ])
            } else {
                let mut spans = vec![
                    Span::styled(tl.name.clone(), theme::tool_name()),
                    Span::styled(" -> ", theme::accent()),
                ];
                if !tl.summary.is_empty() {
                    spans.push(Span::styled(tl.summary.clone(), theme::subdued()));
                }
                Line::from(spans)
            }
        })
        .collect();
    let tools_inner_h = left[1].height.saturating_sub(2) as usize;
    let tools_max_offset = tool_lines.len().saturating_sub(tools_inner_h);
    let tools_scroll_clamped = state.tools_scroll.min(tools_max_offset);
    let tools_title = if tools_scroll_clamped > 0 {
        format!(
            "Tool Activity [scroll {}/{}]",
            tools_scroll_clamped, tools_max_offset
        )
    } else {
        "Tool Activity".to_string()
    };
    let tools_panel = Paragraph::new(tool_lines)
        .block(
            Block::default()
                .title(tools_title)
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .scroll((tools_scroll_clamped as u16, 0));
    frame.render_widget(tools_panel, left[1]);
    areas.tools = Some(left[1]);

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

    // Agent Stream: render markdown-styled lines with scroll support.
    let inner_height = right[0].height.saturating_sub(2) as usize; // subtract border
    let inner_width = right[0].width.saturating_sub(2).max(1) as usize; // subtract border

    let styled_lines = render_agent_markdown(&state.agent_text);

    let total_lines: usize = styled_lines
        .iter()
        .map(|line| {
            let char_count: usize = line.spans.iter().map(|s| s.content.len()).sum();
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
    let agent = Paragraph::new(styled_lines)
        .block(
            Block::default()
                .title(scroll_indicator)
                .borders(Borders::ALL)
                .border_style(theme::border()),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset as u16, 0));
    frame.render_widget(agent, right[0]);
    areas.agent = Some(right[0]);

    render_input_pane(frame, right[1], state);
    areas.input = Some(right[1]);

    let footer_text = if state.input_active && state.input_choices.is_some() {
        "PgUp/PgDn scroll agent · ↑/↓ choices · 1-9 quick-select · Mouse wheel scrolls panels · Esc exit"
    } else if state.input_active {
        "Enter=submit · Shift+Enter=newline · ↑/↓/←/→ navigate · Mouse wheel scrolls panels"
    } else {
        "↑/↓ scroll agent · End auto-scroll · Mouse wheel scrolls panels · --no-ui for plain"
    };
    let footer = Paragraph::new(footer_text).style(theme::subdued());
    frame.render_widget(footer, root[2]);
}

/// Parse agent text as markdown and return styled `Line` objects for ratatui rendering.
fn render_agent_markdown(text: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;

    for raw_line in text.split('\n') {
        let trimmed = raw_line.trim_start();

        // Fenced code block delimiters.
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            lines.push(Line::styled(raw_line.to_string(), theme::code_block()));
            continue;
        }

        // Inside code blocks: render as code_block style, no inline formatting.
        if in_code_block {
            lines.push(Line::styled(raw_line.to_string(), theme::code_block()));
            continue;
        }

        // Horizontal rule: ---, ***, ___
        let hr_trimmed = trimmed.trim();
        if hr_trimmed.len() >= 3
            && (hr_trimmed.chars().all(|c| c == '-')
                || hr_trimmed.chars().all(|c| c == '*')
                || hr_trimmed.chars().all(|c| c == '_'))
        {
            lines.push(Line::styled("─".repeat(40), theme::hr()));
            continue;
        }

        // Headings: # / ## / ###
        if trimmed.starts_with("### ") || trimmed.starts_with("## ") || trimmed.starts_with("# ") {
            lines.push(Line::styled(raw_line.to_string(), theme::heading()));
            continue;
        }

        // Blockquotes: > text
        if trimmed.starts_with("> ") || trimmed == ">" {
            let content = if trimmed.len() > 2 { &trimmed[2..] } else { "" };
            lines.push(Line::from(vec![
                Span::styled("│ ", theme::blockquote()),
                Span::styled(content.to_string(), theme::blockquote()),
            ]));
            continue;
        }

        // List items: - item, * item, + item, or numbered 1. item
        if trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || (trimmed.len() > 2
                && trimmed.as_bytes()[0].is_ascii_digit()
                && trimmed.contains(". "))
        {
            // Find the bullet/number part.
            let leading_ws = &raw_line[..raw_line.len() - trimmed.len()];
            let (bullet, rest) = if trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("+ ")
            {
                (&trimmed[..2], &trimmed[2..])
            } else if let Some(dot_pos) = trimmed.find(". ") {
                (&trimmed[..dot_pos + 2], &trimmed[dot_pos + 2..])
            } else {
                (trimmed, "")
            };

            let mut spans = vec![
                Span::styled(leading_ws.to_string(), theme::subdued()),
                Span::styled(bullet.to_string(), theme::list_bullet()),
            ];
            spans.extend(parse_inline_markdown(rest));
            lines.push(Line::from(spans));
            continue;
        }

        // Normal text: apply inline markdown formatting.
        let spans = parse_inline_markdown(raw_line);
        lines.push(Line::from(spans));
    }

    lines
}

/// Parse inline markdown within a single line, returning styled spans.
///
/// Recognizes: `code`, **bold**, *italic*, [links](url)
fn parse_inline_markdown(line: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();

    while i < len {
        // Backtick: inline code.
        if chars[i] == '`' {
            if let Some(end) = find_closing(&chars, i + 1, '`') {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), theme::subdued()));
                }
                let code: String = chars[i..=end].iter().collect();
                spans.push(Span::styled(code, theme::code_span()));
                i = end + 1;
                continue;
            }
        }

        // Link: [text](url)
        if chars[i] == '[' {
            if let Some((text, url, end_idx)) = parse_markdown_link(&chars, i) {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), theme::subdued()));
                }
                spans.push(Span::styled(format!("[{text}]({url})"), theme::link()));
                i = end_idx;
                continue;
            }
        }

        // Double asterisk: bold.
        if chars[i] == '*' && i + 1 < len && chars[i + 1] == '*' {
            if let Some(end) = find_closing_double(&chars, i + 2, '*', '*') {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), theme::subdued()));
                }
                let bold_text: String = chars[i..end + 2].iter().collect();
                spans.push(Span::styled(
                    bold_text,
                    theme::subdued().add_modifier(Modifier::BOLD),
                ));
                i = end + 2;
                continue;
            }
        }

        // Single asterisk: italic (but not **).
        if chars[i] == '*' && !(i + 1 < len && chars[i + 1] == '*') {
            if let Some(end) = find_closing_single_star(&chars, i + 1) {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), theme::subdued()));
                }
                let italic_text: String = chars[i..=end].iter().collect();
                spans.push(Span::styled(
                    italic_text,
                    theme::subdued().add_modifier(Modifier::ITALIC),
                ));
                i = end + 1;
                continue;
            }
        }

        buf.push(chars[i]);
        i += 1;
    }

    if !buf.is_empty() {
        spans.push(Span::styled(buf, theme::subdued()));
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), theme::subdued()));
    }

    spans
}

/// Find closing char at position > start.
fn find_closing(chars: &[char], start: usize, close: char) -> Option<usize> {
    chars.iter().enumerate().skip(start).find_map(
        |(j, ch)| {
            if *ch == close {
                Some(j)
            } else {
                None
            }
        },
    )
}

/// Find closing double-char pair (e.g. **).
fn find_closing_double(chars: &[char], start: usize, c1: char, c2: char) -> Option<usize> {
    if chars.len() < 2 {
        return None;
    }
    (start..chars.len() - 1).find(|&j| chars[j] == c1 && chars[j + 1] == c2)
}

/// Find a closing single `*` that is not part of `**`.
fn find_closing_single_star(chars: &[char], start: usize) -> Option<usize> {
    for j in start..chars.len() {
        if chars[j] == '*' {
            if j + 1 < chars.len() && chars[j + 1] == '*' {
                continue;
            }
            return Some(j);
        }
    }
    None
}

/// Parse a markdown link `[text](url)` starting at position `i`.
/// Returns (text, url, end_index) where end_index is one past the closing `)`.
fn parse_markdown_link(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    if chars[i] != '[' {
        return None;
    }
    let close_bracket = find_closing(chars, i + 1, ']')?;
    if close_bracket + 1 >= chars.len() || chars[close_bracket + 1] != '(' {
        return None;
    }
    let close_paren = find_closing(chars, close_bracket + 2, ')')?;
    let text: String = chars[i + 1..close_bracket].iter().collect();
    let url: String = chars[close_bracket + 2..close_paren].iter().collect();
    Some((text, url, close_paren + 1))
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
        let inner_h = pane_inner.height as usize;

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
                                Span::styled(pre.to_string(), theme::input_text()),
                                Span::styled(
                                    if post.is_empty() {
                                        " ".to_string()
                                    } else {
                                        post.chars().next().unwrap().to_string()
                                    },
                                    theme::cursor(),
                                ),
                                Span::styled(post_rest.to_string(), theme::input_text()),
                            ]));
                        } else {
                            lines.push(Line::styled(row_text.to_string(), theme::input_text()));
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
                            Span::styled(pre.to_string(), theme::input_text()),
                            Span::styled(cursor_ch.to_string(), theme::cursor()),
                            Span::styled(post_rest.to_string(), theme::input_text()),
                        ]));
                    } else {
                        // Cursor at end of line.
                        lines.push(Line::from(vec![
                            Span::styled(row_text.to_string(), theme::input_text()),
                            Span::styled(" ", theme::cursor()),
                        ]));
                    }
                } else {
                    lines.push(Line::styled(row_text.to_string(), theme::input_text()));
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

        // Compute scroll offset to keep cursor visible.
        // cursor_row is relative to the start of the text lines (after hint lines).
        // Total content rows = hint_line_count + visual_row + 2 (hint bar).
        let total_content_lines = (hint_line_count as u16) + visual_row + 2;
        let abs_cursor_line = (hint_line_count as u16) + cursor_row;
        let scroll_offset = if inner_h == 0 || total_content_lines <= inner_h as u16 {
            // Everything fits, no scroll needed.
            0u16
        } else {
            // Auto-scroll to keep cursor visible.
            let max_scroll = total_content_lines.saturating_sub(inner_h as u16);
            // Use manual scroll if set, but clamp to keep cursor visible.
            let manual = state.input_scroll as u16;
            // Cursor must be within [scroll_offset, scroll_offset + inner_h - 1].
            if abs_cursor_line < manual {
                // Cursor above viewport — scroll up to it.
                abs_cursor_line
            } else if abs_cursor_line >= manual + (inner_h as u16) {
                // Cursor below viewport — scroll down to show it.
                (abs_cursor_line + 1).saturating_sub(inner_h as u16)
            } else {
                // Cursor is visible at current manual scroll position.
                manual
            }
            .min(max_scroll)
        };

        let widget = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(state.input_title.as_str())
                    .borders(Borders::ALL)
                    .border_style(theme::modal_border()),
            )
            .style(theme::subdued())
            .scroll((scroll_offset, 0));
        frame.render_widget(widget, area);

        // Position terminal cursor for blinking effect.
        if pane_inner.width > 0 && pane_inner.height > 0 {
            let abs_cursor_y = pane_inner.y + (hint_line_count as u16) + cursor_row - scroll_offset;
            let abs_cursor_x = pane_inner.x + cursor_col;
            if abs_cursor_x < pane_inner.x + pane_inner.width
                && abs_cursor_y >= pane_inner.y
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
    use crate::ui::event::{ToolLine, UiEvent, UiLevel};
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
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
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
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
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
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Confirm"));
        assert!(text.contains("Delete?"));
    }

    #[test]
    fn input_pane_does_not_panic_on_tiny_frame() {
        let backend = TestBackend::new(1, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
    }

    #[test]
    fn dashboard_renders_input_pane_inactive() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::default();
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
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
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
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
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
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
        state.apply(UiEvent::ToolActivity(ToolLine {
            name: "Read".to_string(),
            summary: "tool_call: Read".to_string(),
        }));
        terminal
            .draw(|f| {
                let mut areas = FrameAreas::default();
                render(f, &state, &mut areas);
            })
            .unwrap();
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

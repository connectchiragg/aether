use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{App, View};
use crate::live::{LiveEngine, TurnMarker};
use crate::model::AgentStatus;
use crate::theme;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    // Show session list view
    if app.view == View::Sessions {
        if let Some(live) = app.engine.live_engine() {
            render_session_list(frame, app, live, area);
            return;
        }
    }

    let agents = app.engine.agents();
    let messages = app.engine.messages();

    if agents.is_empty() {
        render_placeholder(frame, app, area);
        return;
    }

    let agent_count = agents.len();
    let constraints: Vec<Constraint> = (0..agent_count)
        .map(|_| Constraint::Ratio(1, agent_count as u32))
        .collect();

    let columns = Layout::horizontal(constraints).split(area);

    // Store pane column ranges for mouse hit-testing
    app.pane_columns = columns.iter().map(|r| (r.x, r.x + r.width)).collect();

    // Get turn markers for the active session (live mode only)
    let turns: Vec<TurnMarker> = app
        .engine
        .live_engine()
        .and_then(|e| e.active_session())
        .map(|s| s.turns.clone())
        .unwrap_or_default();

    for (i, agent) in agents.iter().enumerate() {
        let is_focused = app.focused_pane == i;
        let border_style = if is_focused {
            theme::focused_border_style()
        } else {
            theme::unfocused_border_style()
        };

        let status_span = match &agent.status {
            AgentStatus::Idle => {
                Span::styled(" ", theme::dim_style())
            }
            AgentStatus::Thinking { dots } => {
                let d = ".".repeat((*dots % 4) + 1);
                Span::styled(
                    format!(" {d} "),
                    Style::default().fg(agent.color),
                )
            }
            AgentStatus::Streaming => Span::styled(
                "  ",
                Style::default()
                    .fg(agent.color)
                    .add_modifier(Modifier::BOLD),
            ),
            AgentStatus::WaitingForInput => {
                Span::styled("  ", Style::default().fg(theme::WARM))
            }
        };

        let title = Line::from(vec![
            Span::styled(
                format!(" {} ", agent.name),
                Style::default()
                    .fg(agent.color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}", agent.role),
                theme::subtle_style(),
            ),
            status_span,
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        let inner = block.inner(columns[i]);
        frame.render_widget(block, columns[i]);

        // Collect messages for this agent (sent or received)
        let agent_messages: Vec<(usize, &crate::model::Message)> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.from == agent.id || m.to == agent.id)
            .collect();

        let mut lines: Vec<Line> = Vec::new();

        for (msg_global_idx, msg) in &agent_messages {
            // Check if a turn separator should be inserted before this message
            for turn in &turns {
                if turn.message_start_idx == *msg_global_idx {
                    let separator_text = if turn.prompt.is_empty() {
                        format!(" Turn {} ", turn.turn_index)
                    } else {
                        let prompt_preview: String =
                            turn.prompt.chars().take(36).collect();
                        let ellipsis = if turn.prompt.len() > 36 { ".." } else { "" };
                        format!(" Turn {}: {prompt_preview}{ellipsis} ", turn.turn_index)
                    };
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        separator_text,
                        Style::default()
                            .fg(theme::SUBTLE)
                            .add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(""));
                }
            }

            let is_sent = msg.from == agent.id;
            let visible = msg.visible_content();

            if visible.is_empty() {
                continue;
            }

            // Determine if this message is from a previous turn (dim it)
            let is_old_turn = if let Some(last_turn) = turns.last() {
                *msg_global_idx < last_turn.message_start_idx
            } else {
                false
            };

            // Direction indicator
            let direction = if is_sent {
                let to_agent = agents.iter().find(|a| a.id == msg.to);
                let to_name = to_agent.map(|a| a.name.clone()).unwrap_or_else(|| msg.to.clone());
                let to_color = if is_old_turn {
                    theme::DIM
                } else {
                    to_agent.map(|a| a.color).unwrap_or(Color::White)
                };
                Line::from(vec![
                    Span::styled(" to ", theme::dim_style()),
                    Span::styled(to_name, Style::default().fg(to_color)),
                ])
            } else {
                let from_agent = agents.iter().find(|a| a.id == msg.from);
                let from_name = from_agent.map(|a| a.name.clone()).unwrap_or_else(|| msg.from.clone());
                let from_color = if is_old_turn {
                    theme::DIM
                } else {
                    from_agent.map(|a| a.color).unwrap_or(Color::White)
                };
                Line::from(vec![
                    Span::styled(" from ", theme::dim_style()),
                    Span::styled(from_name, Style::default().fg(from_color)),
                ])
            };
            lines.push(direction);

            // Message content
            let msg_style = if is_old_turn {
                theme::dim_style()
            } else if is_sent {
                Style::default().fg(agent.color)
            } else {
                Style::default().fg(Color::White)
            };

            for line in visible.lines() {
                lines.push(Line::from(Span::styled(line.to_string(), msg_style)));
            }

            // Streaming cursor
            if !msg.is_fully_revealed() && is_sent {
                lines.push(Line::from(Span::styled(
                    "",
                    Style::default()
                        .fg(agent.color)
                        .add_modifier(Modifier::SLOW_BLINK),
                )));
            }

            lines.push(Line::from(""));
        }

        // Pre-wrap lines at pane width so lines.len() == visual line count
        let wrapped = pre_wrap_lines(lines, inner.width as usize);
        let line_count = wrapped.len() as u16;
        let max_scroll = line_count.saturating_sub(inner.height);
        app.pane_max_scrolls.insert(i, max_scroll);

        let scroll_y = app.pane_scrolls.get(&i).copied().unwrap_or(0).min(max_scroll);
        app.pane_scrolls.insert(i, scroll_y);

        let text = Text::from(wrapped);
        let para = Paragraph::new(text)
            .scroll((scroll_y, 0));

        frame.render_widget(para, inner);
    }
}

fn render_session_list(frame: &mut Frame, app: &App, live: &LiveEngine, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style())
        .title(Line::from(vec![
            Span::styled(
                " sessions ",
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({}) ", live.session_count()),
                theme::subtle_style(),
            ),
        ]));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible: Vec<(usize, &crate::live::SessionState)> = live.active_sessions().collect();

    // Layout: center the list vertically if it fits
    let items_height: u16 = (visible.len() as u16 * 3) + 2;
    let top_pad = if items_height < inner.height {
        (inner.height - items_height) / 3
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    for _ in 0..top_pad {
        lines.push(Line::from(""));
    }

    for (real_idx, session) in &visible {
        let is_selected = *real_idx == app.session_list_cursor;
        let agent_count = session.agents.len();
        let msg_count = session.messages.len();
        let has_activity = !session.agents.is_empty();

        // Cursor and highlight
        let (cursor, name_style, detail_style) = if is_selected {
            (
                Span::styled("  > ", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                Style::default().fg(theme::SUBTLE),
            )
        } else {
            (
                Span::styled("    ", theme::dim_style()),
                Style::default().fg(theme::SUBTLE),
                theme::dim_style(),
            )
        };

        // Status indicator
        let status_indicator = if has_activity {
            Span::styled("● ", Style::default().fg(theme::ACCENT))
        } else {
            Span::styled("○ ", theme::dim_style())
        };

        // Session name line (or rename input)
        let is_renaming = is_selected && app.rename_input.is_some();
        if is_renaming {
            let buf = app.rename_input.as_deref().unwrap_or("");
            lines.push(Line::from(vec![
                cursor,
                status_indicator,
                Span::styled(
                    format!("{buf}▏"),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                cursor,
                status_indicator,
                Span::styled(&session.name, name_style),
            ]));
        }

        // Detail line with stats
        let detail = if agent_count == 0 && msg_count == 0 {
            "      no activity".to_string()
        } else {
            format!("      {agent_count} agents  {msg_count} msgs  {} turns",
                session.turns.len())
        };
        lines.push(Line::from(Span::styled(detail, detail_style)));

        // Separator between items
        lines.push(Line::from(""));
    }

    let text = Text::from(lines);
    let para = Paragraph::new(text);
    frame.render_widget(para, inner);
}

fn render_placeholder(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let center_y = inner.height / 2;
    let pad_area = Layout::vertical([
        Constraint::Length(center_y.saturating_sub(2)),
        Constraint::Min(0),
    ])
    .split(inner);

    let lines = if app.engine.is_live() {
        vec![
            Line::from(Span::styled(
                "waiting for agent activity",
                Style::default().fg(theme::SUBTLE),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "run /tui in your Claude Code session",
                theme::dim_style(),
            )),
        ]
    } else {
        vec![Line::from(Span::styled("no agents", theme::subtle_style()))]
    };

    let para = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(para, pad_area[1]);
}

/// Pre-wrap lines at a given width so that `result.len()` equals the visual line count.
/// Each output Line fits within `width` characters. Spans are split at boundaries.
fn pre_wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return lines;
    }
    let mut out: Vec<Line<'static>> = Vec::new();

    for line in lines {
        // Fast path: measure total width
        let total_width: usize = line.spans.iter().map(|s| s.content.len()).sum();
        if total_width <= width {
            out.push(line);
            continue;
        }

        // Slow path: split spans across multiple lines
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        let mut current_width: usize = 0;

        for span in line.spans {
            let style = span.style;
            let content: &str = &span.content;
            let mut remaining = content;

            while !remaining.is_empty() {
                let space_left = width.saturating_sub(current_width);
                if space_left == 0 {
                    out.push(Line::from(std::mem::take(&mut current_spans)));
                    current_width = 0;
                    continue;
                }

                if remaining.len() <= space_left {
                    current_spans.push(Span::styled(remaining.to_string(), style));
                    current_width += remaining.len();
                    break;
                } else {
                    let (chunk, rest) = remaining.split_at(space_left);
                    current_spans.push(Span::styled(chunk.to_string(), style));
                    out.push(Line::from(std::mem::take(&mut current_spans)));
                    current_width = 0;
                    remaining = rest;
                }
            }
        }

        if !current_spans.is_empty() {
            out.push(Line::from(current_spans));
        }
    }

    out
}

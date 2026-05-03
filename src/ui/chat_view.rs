use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, View};
use crate::live::TurnMarker;
use crate::model::AgentStatus;
use crate::theme;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    // Show session list view
    if app.view == View::Sessions && app.engine.is_live() {
        render_session_list(frame, app, area);
        return;
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
                let to_name = to_agent.map(|a| a.name.as_str()).unwrap_or(&msg.to);
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
                let from_name = from_agent.map(|a| a.name.as_str()).unwrap_or(&msg.from);
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

        // Estimate visual line count using Line::width()
        let pane_width = inner.width.max(1) as usize;
        let visual_height: u16 = lines.iter().map(|line| {
            let w = line.width();
            if w == 0 { 1 } else { ((w.saturating_sub(1)) / pane_width + 1) as u16 }
        }).sum();
        let max_scroll = visual_height.saturating_sub(inner.height) + 20;
        app.pane_max_scrolls.insert(i, max_scroll);

        let scroll_y = app.pane_scrolls.get(&i).copied().unwrap_or(0).min(max_scroll);

        let text = Text::from(lines);
        let para = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0));

        frame.render_widget(para, inner);
    }
}

fn render_session_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let live = app.engine.live_engine().unwrap();
    let session_count = live.session_count();
    let visible: Vec<(usize, String, usize, f64, u64, u64)> = live
        .active_sessions()
        .map(|(i, s)| (
            i,
            s.name.clone(),
            s.usage.turn_count(),
            s.usage.total_cost(),
            s.usage.total_input() + s.usage.total_output(),
            s.last_modified,
        ))
        .collect();

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
                format!("({}) ", session_count),
                theme::subtle_style(),
            ),
        ]));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Each item is 3 lines (name + detail + separator)
    let item_height: u16 = 3;
    let total_items = visible.len() as u16;
    let viewport_items = inner.height / item_height;

    // Find which position in the visible list the cursor is at
    let cursor_pos = visible.iter().position(|(i, ..)| *i == app.session_list_cursor).unwrap_or(0) as u16;

    // Derive scroll from cursor position (keep cursor in viewport)
    if cursor_pos < app.session_list_scroll / item_height {
        app.session_list_scroll = cursor_pos * item_height;
    } else if cursor_pos >= app.session_list_scroll / item_height + viewport_items {
        app.session_list_scroll = (cursor_pos + 1).saturating_sub(viewport_items) * item_height;
    }
    // Clamp scroll to max
    let max_scroll = (total_items * item_height).saturating_sub(inner.height);
    app.session_list_scroll = app.session_list_scroll.min(max_scroll);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut lines: Vec<Line> = Vec::new();

    for (real_idx, name, turn_count, cost, total_tokens, last_modified) in &visible {
        let is_selected = *real_idx == app.session_list_cursor;

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

        // Status indicator: green dot = recently active, dim circle = stale
        let recent = *last_modified > 0 && now.saturating_sub(*last_modified) < 300;
        let status_indicator = if recent {
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
                Span::styled(name.as_str(), name_style),
            ]));
        }

        // Detail line with cost and tokens
        let detail = if *turn_count == 0 {
            String::new()
        } else {
            format!("      {}  {}  {turn_count} turns",
                crate::model::format_cost(*cost),
                crate::model::format_tokens(*total_tokens))
        };
        lines.push(Line::from(Span::styled(detail, detail_style)));

        // Separator between items
        lines.push(Line::from(""));
    }

    let text = Text::from(lines);
    let para = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((app.session_list_scroll, 0));
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

fn text_height_estimate(messages: &[&crate::model::Message], width: u16) -> u16 {
    let mut height: u16 = 0;
    for msg in messages {
        let visible = msg.visible_content();
        if visible.is_empty() {
            continue;
        }
        height += 1;
        for line in visible.lines() {
            let line_width = line.len() as u16;
            height += (line_width / width.max(1)) + 1;
        }
        if !msg.is_fully_revealed() {
            height += 1;
        }
        height += 1;
    }
    height
}

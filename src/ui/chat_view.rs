use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, View};
use crate::live::TurnMarker;
use crate::model::{AgentStatus, TurnOutcome};
use crate::provider::{ProviderKind, ProviderStatus};
use crate::theme;

const SESSION_NAME_MAX_WIDTH: usize = 52;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ProviderActivity {
    NotSetUp,
    Idle,
    Live,
}

fn provider_activity(provider: &ProviderStatus, now: u64) -> ProviderActivity {
    if !provider.enabled {
        ProviderActivity::NotSetUp
    } else if provider.last_activity > 0 && now.saturating_sub(provider.last_activity) < 300 {
        ProviderActivity::Live
    } else {
        ProviderActivity::Idle
    }
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.view == View::Providers && app.engine.is_live() {
        render_provider_list(frame, app, area);
        return;
    }

    // Show session list view
    if app.view == View::Sessions && app.engine.is_live() {
        render_session_list(frame, app, area);
        return;
    }

    let agents = app.engine.agents();
    let messages = app.engine.messages();
    let palette = theme::provider_palette(
        app.engine
            .live_engine()
            .and_then(|live| live.active_provider),
    );

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
            Style::default().fg(palette.accent)
        } else {
            Style::default().fg(palette.dim)
        };

        let status_span = match &agent.status {
            AgentStatus::Idle => Span::styled(" ", Style::default().fg(palette.dim)),
            AgentStatus::Thinking { dots } => {
                let d = ".".repeat((*dots % 4) + 1);
                Span::styled(format!(" {d} "), Style::default().fg(palette.accent))
            }
            AgentStatus::Streaming => Span::styled(
                "  ",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            AgentStatus::WaitingForInput => {
                Span::styled("  ", Style::default().fg(palette.highlight))
            }
        };

        let title = Line::from(vec![
            Span::styled(
                format!(" {} ", agent.name),
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}", agent.role),
                Style::default().fg(palette.subtle),
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
                        let prompt_preview: String = turn.prompt.chars().take(36).collect();
                        let ellipsis = if turn.prompt.len() > 36 { ".." } else { "" };
                        format!(" Turn {}: {prompt_preview}{ellipsis} ", turn.turn_index)
                    };
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        separator_text,
                        Style::default()
                            .fg(palette.subtle)
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
                    palette.dim
                } else {
                    palette.accent
                };
                Line::from(vec![
                    Span::styled(" to ", Style::default().fg(palette.dim)),
                    Span::styled(to_name, Style::default().fg(to_color)),
                ])
            } else {
                let from_agent = agents.iter().find(|a| a.id == msg.from);
                let from_name = from_agent.map(|a| a.name.as_str()).unwrap_or(&msg.from);
                let from_color = if is_old_turn {
                    palette.dim
                } else {
                    palette.text
                };
                Line::from(vec![
                    Span::styled(" from ", Style::default().fg(palette.dim)),
                    Span::styled(from_name, Style::default().fg(from_color)),
                ])
            };
            lines.push(direction);

            // Message content
            let msg_style = if is_old_turn {
                Style::default().fg(palette.dim)
            } else if is_sent {
                Style::default().fg(palette.accent)
            } else {
                Style::default().fg(palette.text)
            };

            for line in visible.lines() {
                lines.push(Line::from(Span::styled(line.to_string(), msg_style)));
            }

            // Streaming cursor
            if !msg.is_fully_revealed() && is_sent {
                lines.push(Line::from(Span::styled(
                    "",
                    Style::default()
                        .fg(palette.accent)
                        .add_modifier(Modifier::SLOW_BLINK),
                )));
            }

            lines.push(Line::from(""));
        }

        // Estimate visual line count using Line::width()
        let pane_width = inner.width.max(1) as usize;
        let visual_height: u16 = lines
            .iter()
            .map(|line| {
                let w = line.width();
                if w == 0 {
                    1
                } else {
                    ((w.saturating_sub(1)) / pane_width + 1) as u16
                }
            })
            .sum();
        let max_scroll = visual_height.saturating_sub(inner.height) + 20;
        app.pane_max_scrolls.insert(i, max_scroll);

        let scroll_y = app
            .pane_scrolls
            .get(&i)
            .copied()
            .unwrap_or(0)
            .min(max_scroll);

        let text = Text::from(lines);
        let para = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0));

        frame.render_widget(para, inner);
    }
}

fn render_provider_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let live = app.engine.live_engine().unwrap();
    let providers = live.provider_statuses();
    let page = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  choose provider",
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {} available", providers.len()),
                theme::subtle_style(),
            ),
        ])),
        page[0],
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let cards = provider_card_areas(page[1], providers.len());
    for (idx, (provider, card_area)) in providers.iter().zip(cards).enumerate() {
        render_provider_card(
            frame,
            provider,
            idx == app.provider_list_cursor,
            provider_activity(provider, now),
            app.tick,
            card_area,
        );
    }
}

fn provider_card_areas(area: Rect, count: usize) -> Vec<Rect> {
    if count == 0 || area.width < 4 || area.height < 4 {
        return Vec::new();
    }
    let content = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };

    if count == 2 && content.width >= 84 {
        let columns = Layout::horizontal([
            Constraint::Ratio(1, 2),
            Constraint::Length(3),
            Constraint::Ratio(1, 2),
        ])
        .split(content);
        vec![columns[0], columns[2]]
    } else if count == 2 {
        let rows = Layout::vertical([
            Constraint::Ratio(1, 2),
            Constraint::Length(1),
            Constraint::Ratio(1, 2),
        ])
        .split(content);
        vec![rows[0], rows[2]]
    } else {
        Layout::vertical(vec![Constraint::Ratio(1, count as u32); count])
            .split(content)
            .to_vec()
    }
}

fn provider_logo(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Claude => &["   ▐▛███▜▌", "  ▝▜█████▛▘", "    ▘▘ ▝▝", "", " CLAUDE CODE"],
        ProviderKind::Codex => &["", ">_  OPENAI CODEX", ""],
    }
}

fn provider_brand_color(kind: ProviderKind) -> Color {
    match kind {
        ProviderKind::Claude => theme::adaptive((217, 119, 87), 173, Color::LightRed),
        // OpenAI's Blossom is always monochrome. Color is reserved for the
        // surrounding Codex telemetry system.
        ProviderKind::Codex => theme::text(),
    }
}

fn provider_surface_color(kind: ProviderKind, selected: bool) -> Color {
    match kind {
        ProviderKind::Claude if selected => theme::adaptive((51, 37, 31), 236, Color::Black),
        ProviderKind::Claude => theme::adaptive((38, 30, 27), 235, Color::Black),
        ProviderKind::Codex if selected => theme::adaptive((28, 35, 34), 235, Color::Black),
        ProviderKind::Codex => theme::adaptive((20, 25, 24), 234, Color::Black),
    }
}

fn provider_live_color(kind: ProviderKind) -> Color {
    match kind {
        ProviderKind::Claude => provider_brand_color(kind),
        ProviderKind::Codex => theme::adaptive((116, 210, 171), 79, Color::LightGreen),
    }
}

fn provider_compact_logo(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Claude => &[" ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝"],
        ProviderKind::Codex => &["", ">_  OPENAI CODEX", ""],
    }
}

fn provider_micro_wordmark(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "CLAUDE CODE",
        ProviderKind::Codex => ">_ OPENAI CODEX",
    }
}

fn render_provider_rail(frame: &mut Frame, area: Rect, brand: Color, selected: bool, tick: u32) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let scan = (tick as usize / 4) % area.width as usize;
    let spans = (0..area.width as usize)
        .map(|x| {
            let glowing = selected && x.abs_diff(scan) <= 1;
            Span::styled(
                if selected { "▀" } else { "─" },
                Style::default().fg(if glowing {
                    theme::text()
                } else if selected {
                    brand
                } else {
                    theme::dim()
                }),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_provider_card(
    frame: &mut Frame,
    provider: &ProviderStatus,
    selected: bool,
    activity: ProviderActivity,
    tick: u32,
    area: Rect,
) {
    let brand = provider_brand_color(provider.kind);
    let border = if selected { brand } else { theme::dim() };
    let activity_color = match activity {
        ProviderActivity::Live if tick % 12 < 8 => provider_live_color(provider.kind),
        ProviderActivity::Live => theme::dim(),
        ProviderActivity::Idle | ProviderActivity::NotSetUp => theme::dim(),
    };
    let activity_symbol = if activity == ProviderActivity::NotSetUp {
        "○"
    } else {
        "●"
    };
    let state = match activity {
        ProviderActivity::Live => "LIVE NOW".to_string(),
        ProviderActivity::Idle => "READY".to_string(),
        ProviderActivity::NotSetUp => provider.state_label().to_ascii_uppercase(),
    };
    let title = Line::from(vec![
        Span::styled(
            if selected { " ◆ " } else { "   " },
            Style::default().fg(brand),
        ),
        Span::styled(
            provider.kind.display_name().to_ascii_uppercase(),
            Style::default()
                .fg(if selected { brand } else { theme::subtle() })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Double
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(provider_surface_color(provider.kind, selected)))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    render_provider_rail(frame, Rect { height: 1, ..inner }, brand, selected, tick);

    let logo_style = if selected {
        Style::default().fg(brand).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::subtle())
    };
    let micro = inner.height < 5;
    let compact = !micro && inner.height < 13;
    let mut lines: Vec<Line<'static>> = if micro {
        vec![Line::from(Span::styled(
            provider_micro_wordmark(provider.kind),
            logo_style,
        ))]
    } else if compact {
        provider_compact_logo(provider.kind)
            .iter()
            .map(|line| Line::from(Span::styled((*line).to_string(), logo_style)))
            .collect()
    } else {
        provider_logo(provider.kind)
            .iter()
            .map(|line| Line::from(Span::styled((*line).to_string(), logo_style)))
            .collect()
    };
    if !micro && !compact {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "─".repeat(inner.width.saturating_sub(8).min(32) as usize),
            theme::dim_style(),
        )));
    }
    let mut status_line = vec![
        Span::styled(
            format!("{activity_symbol} "),
            Style::default().fg(activity_color),
        ),
        Span::styled(
            state,
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if compact || micro {
        status_line.push(Span::styled("  ·  ", theme::dim_style()));
        status_line.push(Span::styled(
            format!("{:02} SESSIONS", provider.session_count),
            Style::default().fg(if selected { brand } else { theme::subtle() }),
        ));
    }
    lines.push(Line::from(status_line));
    if !compact && !micro {
        lines.push(Line::from(Span::styled(
            format!("{:02}  SESSIONS", provider.session_count),
            Style::default().fg(if selected { brand } else { theme::subtle() }),
        )));
    }
    if selected && !micro && !compact {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "SELECTED",
            Style::default().fg(brand).add_modifier(Modifier::BOLD),
        )));
    }

    let available_height = inner.height.saturating_sub(1);
    let content_height = lines.len().min(available_height as usize) as u16;
    let content_area = Rect {
        x: inner.x,
        y: inner
            .y
            .saturating_add(1)
            .saturating_add(available_height.saturating_sub(content_height) / 2),
        width: inner.width,
        height: content_height,
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines)).alignment(Alignment::Center),
        content_area,
    );
}

fn render_session_list(frame: &mut Frame, app: &mut App, area: Rect) {
    struct VisibleSession {
        real_idx: usize,
        project_key: String,
        project_name: String,
        name: String,
        source: String,
        turn_count: usize,
        cost: f64,
        cost_known: bool,
        cost_complete: bool,
        total_tokens: u64,
        last_activity: u64,
        outcome: Option<TurnOutcome>,
    }

    let live = app.engine.live_engine().unwrap();
    let palette = theme::provider_palette(live.active_provider);
    let session_brand = palette.primary;
    let session_count = live.session_count();
    let visible: Vec<VisibleSession> = live
        .active_sessions()
        .map(|(i, s)| VisibleSession {
            real_idx: i,
            project_key: s.project_display_path(),
            project_name: s.project_name(),
            name: s.name.clone(),
            source: s.source.clone(),
            turn_count: s.usage.turn_count(),
            cost: s.usage.total_cost(),
            cost_known: s.usage.cost_is_known(),
            cost_complete: s.usage.cost_is_complete(),
            total_tokens: s.usage.total_input() + s.usage.total_output(),
            last_activity: s.last_activity,
            outcome: s
                .usage
                .turns
                .last()
                .map(|turn| turn.telemetry.outcome.clone()),
        })
        .collect();
    if !visible
        .iter()
        .any(|session| session.real_idx == app.session_list_cursor)
    {
        if let Some(first) = visible.first() {
            app.session_list_cursor = first.real_idx;
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.dim))
        .title(Line::from(vec![
            Span::styled(
                " sessions ",
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({}) ", session_count),
                Style::default().fg(palette.subtle),
            ),
        ]));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    let session_name_width = usize::from(inner.width.saturating_sub(6)).min(SESSION_NAME_MAX_WIDTH);

    let mut total_lines = 0_u16;
    let mut cursor_line = 0_u16;
    let mut previous_project: Option<&str> = None;
    for session in &visible {
        if previous_project != Some(session.project_key.as_str()) {
            total_lines = total_lines.saturating_add(1);
            previous_project = Some(session.project_key.as_str());
        }
        if session.real_idx == app.session_list_cursor {
            cursor_line = total_lines;
        }
        total_lines = total_lines.saturating_add(3);
    }

    let cursor_end = cursor_line.saturating_add(2);
    if cursor_line < app.session_list_scroll {
        app.session_list_scroll = cursor_line;
    } else if cursor_end >= app.session_list_scroll.saturating_add(inner.height) {
        app.session_list_scroll = cursor_end.saturating_add(1).saturating_sub(inner.height);
    }
    let max_scroll = total_lines.saturating_sub(inner.height);
    app.session_list_scroll = app.session_list_scroll.min(max_scroll);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut lines: Vec<Line> = Vec::new();
    let mut current_project: Option<&str> = None;

    for session in &visible {
        if current_project != Some(session.project_key.as_str()) {
            let rule_width = usize::from(inner.width)
                .saturating_sub(UnicodeWidthStr::width(session.project_name.as_str()) + 7);
            lines.push(Line::from(vec![
                Span::styled("  ╭─ ", Style::default().fg(session_brand)),
                Span::styled(
                    session.project_name.as_str(),
                    Style::default()
                        .fg(session_brand)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", "─".repeat(rule_width)),
                    Style::default().fg(palette.dim),
                ),
            ]));
            current_project = Some(session.project_key.as_str());
        }

        let is_selected = session.real_idx == app.session_list_cursor;

        // Cursor and highlight
        let (cursor, name_style, detail_style) = if is_selected {
            (
                Span::styled(
                    " ▌  ",
                    Style::default()
                        .fg(session_brand)
                        .add_modifier(Modifier::BOLD),
                ),
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(palette.subtle),
            )
        } else {
            (
                Span::styled("    ", Style::default().fg(palette.dim)),
                Style::default().fg(palette.subtle),
                Style::default().fg(palette.dim),
            )
        };

        // Failed sessions are distinct from normal recent/stale activity.
        let recent = session.last_activity > 0 && now.saturating_sub(session.last_activity) < 300;
        let failed = session.outcome == Some(TurnOutcome::Failed);
        let status_indicator = if failed {
            Span::styled("× ", Style::default().fg(palette.danger))
        } else if recent {
            Span::styled(
                "● ",
                Style::default().fg(if app.tick % 16 < 11 {
                    palette.accent
                } else {
                    palette.dim
                }),
            )
        } else {
            Span::styled("● ", Style::default().fg(palette.dim))
        };

        let row_style = if is_selected {
            Style::default().bg(palette.surface_high)
        } else {
            Style::default()
        };

        // Session name line (or rename input)
        let is_renaming = is_selected && app.rename_input.is_some();
        if is_renaming {
            let buf = app.rename_input.as_deref().unwrap_or("");
            let remaining =
                usize::from(inner.width).saturating_sub(6 + UnicodeWidthStr::width(buf) + 1);
            lines.push(
                Line::from(vec![
                    cursor,
                    status_indicator,
                    Span::styled(
                        format!("{buf}▏"),
                        Style::default()
                            .fg(palette.text)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" ".repeat(remaining)),
                ])
                .style(row_style),
            );
        } else {
            let display_name = truncate_with_ellipsis(&session.name, session_name_width);
            let remaining = usize::from(inner.width)
                .saturating_sub(6 + UnicodeWidthStr::width(display_name.as_str()));
            lines.push(
                Line::from(vec![
                    cursor,
                    status_indicator,
                    Span::styled(display_name, name_style),
                    Span::raw(" ".repeat(remaining)),
                ])
                .style(row_style),
            );
        }

        // Detail line with cost and tokens
        let detail = if session.turn_count == 0 {
            session.source.clone()
        } else {
            let cost_label = if session.cost_known {
                format!(
                    "est. {}{}",
                    crate::model::format_cost(session.cost),
                    if session.cost_complete {
                        ""
                    } else {
                        " partial"
                    }
                )
            } else {
                "estimate unavailable".to_string()
            };
            format!(
                "{}  {cost_label}  {}  {} turns",
                session.source,
                crate::model::format_tokens(session.total_tokens),
                session.turn_count
            )
        };
        let detail_width = usize::from(inner.width.saturating_sub(6));
        let detail = truncate_with_ellipsis(&detail, detail_width);
        let mut detail_spans = vec![Span::styled("      ", detail_style)];
        if failed {
            detail_spans.push(Span::styled(
                "failed  ",
                Style::default().fg(palette.danger),
            ));
        }
        detail_spans.push(Span::styled(detail, detail_style));
        lines.push(Line::from(detail_spans).style(row_style));

        // Separator between items
        lines.push(Line::from(""));
    }

    let text = Text::from(lines);
    let para = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((app.session_list_scroll, 0));
    frame.render_widget(para, inner);
}

fn truncate_with_ellipsis(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let content_width = max_width - 3;
    let mut width = 0;
    let mut truncated = String::new();
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > content_width {
            break;
        }
        truncated.push(character);
        width += character_width;
    }
    truncated.push_str("...");
    truncated
}

fn render_placeholder(frame: &mut Frame, app: &App, area: Rect) {
    let palette = theme::provider_palette(
        app.engine
            .live_engine()
            .and_then(|live| live.active_provider),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.dim));

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
                Style::default().fg(palette.subtle),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "run a supported coding agent, then refresh this view",
                Style::default().fg(palette.dim),
            )),
        ]
    } else {
        vec![Line::from(Span::styled(
            "no agents",
            Style::default().fg(palette.subtle),
        ))]
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

#[cfg(test)]
mod tests {
    use super::{
        provider_activity, provider_card_areas, provider_compact_logo, provider_logo,
        render_provider_card, truncate_with_ellipsis, ProviderActivity,
    };
    use crate::provider::{ProviderKind, ProviderStatus};
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn session_names_are_truncated_with_three_dots() {
        let result =
            truncate_with_ellipsis("Build several demonstration websites for prospects", 24);

        assert_eq!(result, "Build several demonst...");
        assert!(UnicodeWidthStr::width(result.as_str()) <= 24);
    }

    #[test]
    fn short_session_names_are_not_changed() {
        assert_eq!(truncate_with_ellipsis("Learn SQL", 24), "Learn SQL");
    }

    #[test]
    fn provider_activity_distinguishes_setup_idle_and_live() {
        let mut provider = ProviderStatus {
            kind: ProviderKind::Codex,
            enabled: false,
            available: true,
            session_count: 1,
            last_activity: 990,
        };
        assert_eq!(
            provider_activity(&provider, 1_000),
            ProviderActivity::NotSetUp
        );

        provider.enabled = true;
        assert_eq!(provider_activity(&provider, 1_000), ProviderActivity::Live);
        assert_eq!(provider_activity(&provider, 1_290), ProviderActivity::Idle);
    }

    #[test]
    fn provider_cards_are_side_by_side_when_wide_and_stacked_when_narrow() {
        let wide = provider_card_areas(Rect::new(0, 0, 140, 40), 2);
        assert_eq!(wide.len(), 2);
        assert_eq!(wide[0].y, wide[1].y);
        assert!(wide[0].x < wide[1].x);

        let narrow = provider_card_areas(Rect::new(0, 0, 70, 40), 2);
        assert_eq!(narrow.len(), 2);
        assert_eq!(narrow[0].x, narrow[1].x);
        assert!(narrow[0].y < narrow[1].y);
    }

    #[test]
    fn provider_cards_use_native_terminal_lockups() {
        assert_eq!(
            provider_compact_logo(ProviderKind::Claude),
            &[" ▐▛███▜▌", "▝▜█████▛▘", "  ▘▘ ▝▝"]
        );
        assert!(provider_logo(ProviderKind::Codex)
            .iter()
            .any(|line| line.contains(">_  OPENAI CODEX")));

        for provider in [ProviderKind::Claude, ProviderKind::Codex] {
            assert!(!provider_logo(provider)
                .iter()
                .flat_map(|line| line.chars())
                .any(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch)));
        }
    }

    #[test]
    fn compact_provider_card_keeps_status_and_session_count_visible() {
        let provider = ProviderStatus {
            kind: ProviderKind::Claude,
            enabled: true,
            available: true,
            session_count: 3,
            last_activity: 990,
        };
        let backend = TestBackend::new(76, 7);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_provider_card(
                    frame,
                    &provider,
                    true,
                    ProviderActivity::Live,
                    0,
                    frame.area(),
                )
            })
            .unwrap();

        let output: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(output.contains("CLAUDE CODE"));
        assert!(output.contains("▐▛███▜▌"));
        assert!(output.contains("LIVE NOW"));
        assert!(output.contains("03 SESSIONS"));
    }
}

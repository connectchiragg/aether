use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;
use crate::model::{format_cost, format_tokens, TurnUsage};
use crate::theme;

/// Render the combined graph + detail view for the selected session.
pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let turns: Vec<TurnUsage> = app
        .engine
        .live_engine()
        .and_then(|e| e.sessions.get(e.active_idx))
        .map(|s| s.usage.turns.clone())
        .unwrap_or_default();

    if turns.is_empty() {
        render_empty(frame, area);
        return;
    }

    // Clamp selected dot
    if app.selected_dot >= turns.len() {
        app.selected_dot = turns.len().saturating_sub(1);
    }

    // Split: top graph (40%), bottom detail (60%)
    let chunks = Layout::vertical([
        Constraint::Percentage(40),
        Constraint::Percentage(60),
    ])
    .split(area);

    let selected = app.selected_dot;
    render_graph(frame, &turns, selected, chunks[0]);
    let max_scroll = render_detail(frame, &turns, selected, app, chunks[1]);
    // Clamp scroll
    let cur = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0);
    if cur > max_scroll {
        app.pane_scrolls.insert(usize::MAX, max_scroll);
    }
}

fn render_empty(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style())
        .title(Span::styled(" cost explorer ", Style::default().fg(theme::ACCENT)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let msg = Paragraph::new(Line::from(Span::styled(
        "no usage data yet",
        theme::subtle_style(),
    )))
    .alignment(ratatui::layout::Alignment::Center);

    let pad = Layout::vertical([
        Constraint::Length(inner.height / 2),
        Constraint::Min(0),
    ])
    .split(inner);
    frame.render_widget(msg, pad[1]);
}

fn render_graph(frame: &mut Frame, turns: &[TurnUsage], selected: usize, area: Rect) {
    let total_cost: f64 = turns.iter().map(|t| t.cost).sum();
    let title = Line::from(vec![
        Span::styled(" cost/turn ", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("── {} total ── {} turns ", format_cost(total_cost), turns.len()), theme::subtle_style()),
        Span::styled("── ● ", Style::default().fg(theme::PRIMARY)),
        Span::styled("turn  ", theme::dim_style()),
        Span::styled("◆ ", Style::default().fg(theme::WARM)),
        Span::styled("agents spawned ", theme::dim_style()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style())
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 3 {
        return;
    }

    // Find max cost for Y-axis scaling
    let max_cost = turns.iter().map(|t| t.cost).fold(0.0_f64, f64::max);
    if max_cost == 0.0 {
        return;
    }

    let graph_height = inner.height.saturating_sub(2) as f64; // leave room for x-axis
    let graph_width = inner.width as usize;

    // Determine visible window if too many turns
    let max_dots = graph_width / 3; // each dot needs ~3 chars spacing
    let (start_idx, end_idx) = if turns.len() <= max_dots {
        (0, turns.len())
    } else {
        // Center window around selected dot
        let half = max_dots / 2;
        let start = selected.saturating_sub(half);
        let end = (start + max_dots).min(turns.len());
        let start = end.saturating_sub(max_dots);
        (start, end)
    };

    let visible_turns = &turns[start_idx..end_idx];
    let num_visible = visible_turns.len();

    // Calculate dot positions
    let spacing = if num_visible > 1 {
        (graph_width.saturating_sub(2)) as f64 / (num_visible - 1) as f64
    } else {
        0.0
    };

    // Build the graph lines (bottom to top)
    let mut grid: Vec<Vec<(char, Style)>> = vec![vec![(' ', Style::default()); graph_width]; inner.height as usize];

    // Draw dots and connecting lines
    let mut dot_positions: Vec<(usize, usize)> = Vec::new(); // (x, y) in grid coords

    for (i, turn) in visible_turns.iter().enumerate() {
        let x = if num_visible > 1 {
            (1.0 + i as f64 * spacing) as usize
        } else {
            graph_width / 2
        };
        let y_frac = turn.cost / max_cost;
        let y = (graph_height * (1.0 - y_frac)) as usize + 1; // +1 for top margin
        let x = x.min(graph_width - 1);
        let y = y.min(inner.height as usize - 2);
        dot_positions.push((x, y));
    }

    // Draw connecting lines between dots using step-line:
    // horizontal from dot1 to midpoint x, then vertical, then horizontal to dot2
    let line_style = Style::default().fg(theme::SUBTLE);
    for i in 0..dot_positions.len().saturating_sub(1) {
        let (x1, y1) = dot_positions[i];
        let (x2, y2) = dot_positions[i + 1];
        let mid_x = (x1 + x2) / 2;

        // Horizontal from dot1 to mid_x
        for x in (x1 + 1)..=mid_x {
            if x < graph_width {
                grid[y1][x] = ('─', line_style);
            }
        }
        // Vertical from y1 to y2 at mid_x
        let (ya, yb) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        for y in (ya + 1)..yb {
            if mid_x < graph_width && y < inner.height as usize {
                grid[y][mid_x] = ('│', line_style);
            }
        }
        // Corner pieces
        if mid_x < graph_width && y1 != y2 {
            let corner1 = if y2 > y1 { '╮' } else { '╯' };
            let corner2 = if y2 > y1 { '╰' } else { '╭' };
            grid[y1][mid_x] = (corner1, line_style);
            if y2 < inner.height as usize {
                grid[y2][mid_x] = (corner2, line_style);
            }
        }
        // Horizontal from mid_x to dot2
        for x in (mid_x + 1)..x2 {
            if x < graph_width && y2 < inner.height as usize {
                grid[y2][x] = ('─', line_style);
            }
        }
    }

    // Draw dots on top of lines
    // ◆ = turn with sub-agents spawned, ● = regular turn
    for (i, &(x, y)) in dot_positions.iter().enumerate() {
        let actual_idx = start_idx + i;
        let is_selected = actual_idx == selected;
        let has_agents = !visible_turns[i].agents.is_empty();
        let ch = if has_agents { '◆' } else { '●' };
        let style = if is_selected {
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
        } else if has_agents {
            Style::default().fg(theme::WARM)
        } else {
            Style::default().fg(theme::PRIMARY)
        };
        if x < graph_width && y < inner.height as usize {
            grid[y][x] = (ch, style);
        }
    }

    // X-axis labels (turn numbers) on last row
    let label_row = inner.height as usize - 1;
    for (i, &(x, _)) in dot_positions.iter().enumerate() {
        let actual_idx = start_idx + i;
        let label = format!("{}", actual_idx + 1);
        // Only show every Nth label to avoid overlap
        let show_label = num_visible <= 20 || i % (num_visible / 10).max(1) == 0 || actual_idx == selected;
        if show_label && x + label.len() <= graph_width {
            for (j, ch) in label.chars().enumerate() {
                if x + j < graph_width {
                    let style = if actual_idx == selected {
                        Style::default().fg(theme::ACCENT)
                    } else {
                        theme::dim_style()
                    };
                    grid[label_row][x + j] = (ch, style);
                }
            }
        }
    }

    // Selection indicator (▲) below the selected dot
    if let Some(&(x, y)) = dot_positions.get(selected.saturating_sub(start_idx)) {
        if y + 1 < inner.height as usize - 1 && x < graph_width {
            grid[y + 1][x] = ('▲', Style::default().fg(theme::ACCENT));
        }
    }

    // Convert grid to Lines
    let lines: Vec<Line> = grid.into_iter().map(|row| {
        let spans: Vec<Span> = row.into_iter().map(|(ch, style)| {
            Span::styled(ch.to_string(), style)
        }).collect();
        Line::from(spans)
    }).collect();

    let para = Paragraph::new(Text::from(lines));
    frame.render_widget(para, inner);
}

/// Render the detail panel. Returns max scroll value.
fn render_detail(frame: &mut Frame, turns: &[crate::model::TurnUsage], selected: usize, app: &App, area: Rect) -> u16 {
    let turn = &turns[selected];

    let mut title_spans = vec![
        Span::styled(
            format!(" Turn {} ", selected + 1),
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("── ", theme::dim_style()),
        Span::styled(
            format_cost(turn.cost),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
    ];

    // Show jump input if active
    if let Some(ref buf) = app.graph_jump_input {
        title_spans.push(Span::styled(" ── go to: ", theme::dim_style()));
        title_spans.push(Span::styled(
            format!("{buf}▏"),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ));
    }

    let title = Line::from(title_spans);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style())
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let wrap_width = inner.width.saturating_sub(6) as usize;
    let mut lines: Vec<Line> = Vec::new();

    // Prompt (pre-wrapped)
    lines.push(Line::from(Span::styled("  prompt", theme::dim_style())));
    for chunk in word_wrap(&turn.prompt, wrap_width) {
        lines.push(Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(chunk, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));

    // Cost + context
    lines.push(Line::from(vec![
        Span::styled("  cost    ", theme::dim_style()),
        Span::styled(format_cost(turn.cost), Style::default().fg(theme::ACCENT)),
        Span::styled("     context  ", theme::dim_style()),
        Span::styled(
            format!("↑{} cumulative", format_tokens(turn.cumulative_context)),
            Style::default().fg(theme::PRIMARY),
        ),
    ]));
    lines.push(Line::from(""));

    // Token breakdown
    lines.push(Line::from(vec![
        Span::styled("  tokens  ", theme::dim_style()),
        Span::styled(format!("↑{}", format_tokens(turn.input_tokens)), Style::default().fg(theme::PRIMARY)),
        Span::styled(" in  ", theme::dim_style()),
        Span::styled(format!("↓{}", format_tokens(turn.output_tokens)), Style::default().fg(theme::WARM)),
        Span::styled(" out  ", theme::dim_style()),
        Span::styled(
            format!("cache read: {}  cache write: {}",
                format_tokens(turn.cache_read_tokens),
                format_tokens(turn.cache_write_tokens)),
            theme::subtle_style(),
        ),
    ]));
    lines.push(Line::from(""));

    // Agents section with chat
    if !turn.agents.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  ◆ {} agents spawned", turn.agents.len()),
            Style::default().fg(theme::WARM).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        for agent in &turn.agents {
            // Agent header bar
            lines.push(Line::from(vec![
                Span::styled("  ◆ ", Style::default().fg(theme::WARM).add_modifier(Modifier::BOLD)),
                Span::styled(&agent.name, Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("  {}  ↑{} ↓{}",
                        format_cost(agent.cost),
                        format_tokens(agent.input_tokens),
                        format_tokens(agent.output_tokens)),
                    theme::subtle_style(),
                ),
            ]));
            lines.push(Line::from(""));

            // Request section
            if !agent.prompt.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("    ▸ ", Style::default().fg(theme::ACCENT)),
                    Span::styled("REQUEST", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
                ]));
                lines.push(Line::from(""));
                for chunk in word_wrap(&agent.prompt, wrap_width) {
                    lines.push(Line::from(vec![
                        Span::styled("      ", Style::default()),
                        Span::styled(chunk, Style::default().fg(Color::White)),
                    ]));
                }
                lines.push(Line::from(""));
            }

            // Response section
            if !agent.response_preview.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("    ◂ ", Style::default().fg(theme::PRIMARY)),
                    Span::styled("RESPONSE", Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
                ]));
                lines.push(Line::from(""));
                for chunk in word_wrap(&agent.response_preview, wrap_width) {
                    lines.push(Line::from(vec![
                        Span::styled("      ", Style::default()),
                        Span::styled(chunk, Style::default().fg(theme::SUBTLE)),
                    ]));
                }
                lines.push(Line::from(""));
            }

            // Separator between agents
            let sep = "─".repeat(wrap_width.min(40));
            lines.push(Line::from(Span::styled(format!    ("    {sep}"), theme::dim_style())));
            lines.push(Line::from(""));
        }
    }

    // Context saved insight
    if turn.context_saved > 0 {
        let purple = Color::Rgb(180, 130, 255);
        let insight_text = format!("{} tokens processed by sub-agents outside parent context",
            format_tokens(turn.context_saved));
        lines.push(Line::from(vec![
            Span::styled("  ✦ ", Style::default().fg(purple).add_modifier(Modifier::BOLD)),
            Span::styled("INSIGHT", Style::default().fg(purple).add_modifier(Modifier::BOLD)),
        ]));
        for chunk in word_wrap(&insight_text, wrap_width) {
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(chunk, Style::default().fg(purple)),
            ]));
        }
    }

    // All lines are pre-wrapped, so lines.len() is the exact visual height
    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    let scroll = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0).min(max_scroll);

    let text = Text::from(lines);
    let para = Paragraph::new(text)
        .scroll((scroll, 0));
    frame.render_widget(para, inner);

    max_scroll
}

/// Simple word wrap: split text into lines of at most `width` characters.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    for line in text.lines() {
        if line.len() <= width {
            result.push(line.to_string());
        } else {
            let mut remaining = line;
            while remaining.len() > width {
                // Try to break at a space
                let break_at = remaining[..width]
                    .rfind(' ')
                    .unwrap_or(width);
                let (chunk, rest) = remaining.split_at(break_at);
                result.push(chunk.to_string());
                remaining = rest.trim_start();
            }
            if !remaining.is_empty() {
                result.push(remaining.to_string());
            }
        }
    }
    result
}

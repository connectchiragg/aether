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

const METRIC_NAMES: &[&str] = &["cost", "friction", "hallucination", "confidence", "acceptance", "performance"];
const METRIC_COLORS: &[Color] = &[
    Color::Rgb(220, 60, 60),    // cost - crimson
    Color::Rgb(220, 60, 60),    // friction - crimson
    Color::Rgb(200, 40, 40),    // hallucination - dark crimson
    Color::Rgb(255, 170, 80),   // confidence - warm
    Color::Rgb(220, 60, 60),    // acceptance - crimson
    Color::Rgb(220, 60, 60),    // performance - crimson
];

const PROMPT_PREVIEW_LEN: usize = 300;
const AGENT_PROMPT_PREVIEW_LEN: usize = 200;
const AGENT_RESPONSE_PREVIEW_LEN: usize = 300;

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

    if app.selected_dot >= turns.len() {
        app.selected_dot = turns.len().saturating_sub(1);
    }

    let chunks = Layout::vertical([
        Constraint::Percentage(40),
        Constraint::Percentage(60),
    ])
    .split(area);

    let selected = app.selected_dot;
    render_graph(frame, &turns, selected, app.graph_metric, app.graph_zoom, chunks[0]);
    let max_scroll = render_detail(frame, &turns, selected, app, chunks[1]);
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
    let msg = Paragraph::new(Line::from(Span::styled("no usage data yet", theme::subtle_style())))
        .alignment(ratatui::layout::Alignment::Center);
    let pad = Layout::vertical([Constraint::Length(inner.height / 2), Constraint::Min(0)]).split(inner);
    frame.render_widget(msg, pad[1]);
}

fn metric_value(turn: &TurnUsage, metric: u8) -> f64 {
    match metric {
        0 => turn.cost,
        1 => turn.metrics.as_ref().map(|m| m.friction as f64).unwrap_or(0.0),
        2 => turn.metrics.as_ref().map(|m| m.hallucination as f64).unwrap_or(0.0),
        3 => turn.metrics.as_ref().map(|m| m.confidence as f64).unwrap_or(0.0),
        4 => turn.metrics.as_ref().map(|m| m.acceptance as f64).unwrap_or(0.0),
        5 => turn.metrics.as_ref().map(|m| m.performance as f64).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Crimson gradient: interpolate from ember to deep crimson based on fraction (0.0-1.0)
fn crimson_gradient(frac: f64) -> Color {
    let r = (255.0 - frac * 55.0) as u8;   // 255 → 200
    let g = (90.0 - frac * 60.0) as u8;    // 90 → 30
    let b = (70.0 - frac * 50.0) as u8;    // 70 → 20
    Color::Rgb(r, g, b)
}

fn render_graph(frame: &mut Frame, turns: &[TurnUsage], selected: usize, graph_metric: u8, zoom: i8, area: Rect) {
    let metric_idx = graph_metric as usize % METRIC_NAMES.len();
    let metric_name = METRIC_NAMES[metric_idx];

    let mut title_spans = vec![
        Span::styled(format!(" {} ", metric_name), Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
    ];

    if graph_metric == 0 {
        let total_cost: f64 = turns.iter().map(|t| t.cost).sum();
        title_spans.push(Span::styled(
            format!("── {} total ── {} turns ", format_cost(total_cost), turns.len()),
            theme::subtle_style(),
        ));
    } else {
        title_spans.push(Span::styled(format!("── {} turns ", turns.len()), theme::subtle_style()));
    }

    // Metric selector — show full names
    title_spans.push(Span::styled("── ", theme::dim_style()));
    for (i, name) in METRIC_NAMES.iter().enumerate() {
        if i as u8 == graph_metric {
            title_spans.push(Span::styled(format!("[{}]", name), Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)));
        } else {
            title_spans.push(Span::styled(format!(" {} ", name), theme::dim_style()));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style())
        .title(Line::from(title_spans));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 3 { return; }

    let max_val = if graph_metric == 0 {
        turns.iter().map(|t| t.cost).fold(0.0_f64, f64::max)
    } else {
        1.0
    };
    if max_val == 0.0 { return; }

    let graph_height = inner.height.saturating_sub(2) as f64;
    let graph_width = inner.width as usize;

    // Zoom: positive = fewer dots (zoom in), negative = more dots (zoom out for trends)
    let base_dots = graph_width / 3;
    let max_dots = if zoom >= 0 {
        (base_dots >> zoom as usize).max(3)
    } else {
        // Zoom out: pack more dots, min spacing of 1 char
        (base_dots << (-zoom) as usize).min(graph_width)
    };
    let zoomed_out = zoom < 0;
    let (start_idx, end_idx) = if turns.len() <= max_dots {
        (0, turns.len())
    } else {
        let half = max_dots / 2;
        let start = selected.saturating_sub(half);
        let end = (start + max_dots).min(turns.len());
        let start = end.saturating_sub(max_dots);
        (start, end)
    };

    let visible_turns = &turns[start_idx..end_idx];
    let num_visible = visible_turns.len();

    let max_label_len = format!("{}", end_idx).len();
    let inset = max_label_len / 2 + 1;
    let spacing = if num_visible > 1 {
        (graph_width.saturating_sub(inset * 2)) as f64 / (num_visible - 1) as f64
    } else { 0.0 };

    let mut grid: Vec<Vec<(char, Style)>> = vec![vec![(' ', Style::default()); graph_width]; inner.height as usize];
    let mut dot_positions: Vec<(usize, usize)> = Vec::new();

    for (i, turn) in visible_turns.iter().enumerate() {
        let x = if num_visible > 1 { (inset as f64 + i as f64 * spacing) as usize } else { graph_width / 2 };
        let val = metric_value(turn, graph_metric);
        let y_frac = val / max_val;
        let y = (graph_height * (1.0 - y_frac)) as usize + 1;
        dot_positions.push((x.min(graph_width - 1), y.min(inner.height as usize - 2)));
    }

    // Step-line connections with crimson gradient
    for i in 0..dot_positions.len().saturating_sub(1) {
        let (x1, y1) = dot_positions[i];
        let (x2, y2) = dot_positions[i + 1];
        let mid_x = (x1 + x2) / 2;
        let frac = i as f64 / dot_positions.len().max(1) as f64;
        let line_color = crimson_gradient(frac);
        let ls = Style::default().fg(line_color);

        for x in (x1 + 1)..=mid_x {
            if x < graph_width { grid[y1][x] = ('─', ls); }
        }
        let (ya, yb) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        for y in (ya + 1)..yb {
            if mid_x < graph_width && y < inner.height as usize { grid[y][mid_x] = ('│', ls); }
        }
        if mid_x < graph_width && y1 != y2 {
            grid[y1][mid_x] = (if y2 > y1 { '╮' } else { '╯' }, ls);
            if y2 < inner.height as usize { grid[y2][mid_x] = (if y2 > y1 { '╰' } else { '╭' }, ls); }
        }
        for x in (mid_x + 1)..x2 {
            if x < graph_width && y2 < inner.height as usize { grid[y2][x] = ('─', ls); }
        }
    }

    // Dots — crimson theme
    for (i, &(x, y)) in dot_positions.iter().enumerate() {
        let actual_idx = start_idx + i;
        let is_selected = actual_idx == selected;
        let has_agents = !visible_turns[i].agents.is_empty();
        let ch = if has_agents { '◆' } else { '●' };
        let style = if is_selected {
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
        } else {
            let frac = i as f64 / num_visible.max(1) as f64;
            Style::default().fg(crimson_gradient(frac))
        };
        if x < graph_width && y < inner.height as usize { grid[y][x] = (ch, style); }
    }

    // Labels — skip when zoomed out for clean trend view
    if !zoomed_out {
        let label_row = inner.height as usize - 1;
        let mut occupied: Vec<bool> = vec![false; graph_width + 4];
        let place_label = |grid: &mut Vec<Vec<(char, Style)>>, occupied: &mut Vec<bool>, x: usize, label: &str, style: Style| -> bool {
            let centered = x.saturating_sub(label.len() / 2);
            let label_x = if centered + label.len() > graph_width { graph_width.saturating_sub(label.len()) } else { centered };
            let start = label_x.saturating_sub(1);
            let end = (label_x + label.len() + 1).min(occupied.len());
            if occupied[start..end].iter().any(|&o| o) { return false; }
            for (j, ch) in label.chars().enumerate() {
                if label_x + j < graph_width { grid[label_row][label_x + j] = (ch, style); }
            }
            for p in label_x..label_x + label.len() { if p < occupied.len() { occupied[p] = true; } }
            true
        };
        if let Some(&(x, _)) = dot_positions.get(selected.saturating_sub(start_idx)) {
            place_label(&mut grid, &mut occupied, x, &format!("{}", selected + 1), Style::default().fg(theme::ACCENT));
        }
        for (i, &(x, _)) in dot_positions.iter().enumerate() {
            let actual_idx = start_idx + i;
            if actual_idx == selected { continue; }
            let show = num_visible <= 20 || i % (num_visible / 10).max(1) == 0;
            if show { place_label(&mut grid, &mut occupied, x, &format!("{}", actual_idx + 1), theme::dim_style()); }
        }
    }

    if let Some(&(x, y)) = dot_positions.get(selected.saturating_sub(start_idx)) {
        if y > 0 && x < graph_width { grid[y - 1][x] = ('▼', Style::default().fg(theme::ACCENT)); }
    }

    let lines: Vec<Line> = grid.into_iter().map(|row| {
        Line::from(row.into_iter().map(|(ch, style)| Span::styled(ch.to_string(), style)).collect::<Vec<_>>())
    }).collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_detail(frame: &mut Frame, turns: &[TurnUsage], selected: usize, app: &App, area: Rect) -> u16 {
    let turn = &turns[selected];
    let total_turns = turns.len();

    let mut title_spans = vec![
        Span::styled(format!(" Turn {} ", selected + 1), Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled("── ", theme::dim_style()),
        Span::styled(format_cost(turn.cost), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ];
    if let Some(ref buf) = app.graph_jump_input {
        title_spans.push(Span::styled(" ── go to: ", theme::dim_style()));
        title_spans.push(Span::styled(format!("{buf}▏"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style())
        .title(Line::from(title_spans));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let wrap_width = inner.width.saturating_sub(6) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let section_sep = "─".repeat(wrap_width.min(50));

    // ── PROMPT ──
    let show_full_prompt = app.expanded_view.is_some();
    lines.push(Line::from(vec![
        Span::styled("  ▸ ", Style::default().fg(theme::ACCENT)),
        Span::styled("PROMPT", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
        if !show_full_prompt && turn.prompt.len() > PROMPT_PREVIEW_LEN {
            Span::styled("  (e to expand)", Style::default().fg(theme::DIM))
        } else if show_full_prompt {
            Span::styled("  (e to collapse)", Style::default().fg(theme::DIM))
        } else {
            Span::raw("")
        },
    ]));
    let prompt_text = if show_full_prompt {
        turn.prompt.clone()
    } else {
        let preview: String = turn.prompt.chars().take(PROMPT_PREVIEW_LEN).collect();
        if preview.len() < turn.prompt.len() {
            format!("{}...", preview)
        } else {
            preview
        }
    };
    for chunk in word_wrap(&prompt_text, wrap_width) {
        lines.push(Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(chunk, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));

    // ── RESPONSE (right below prompt) ──
    if !turn.response_text.is_empty() {
        let show_full = app.expanded_view.is_some();
        lines.push(Line::from(vec![
            Span::styled("  ◂ ", Style::default().fg(theme::PRIMARY)),
            Span::styled("RESPONSE", Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
            if !show_full && turn.response_text.len() > AGENT_RESPONSE_PREVIEW_LEN {
                Span::styled("  (e to expand)", Style::default().fg(theme::DIM))
            } else if show_full {
                Span::styled("  (e to collapse)", Style::default().fg(theme::DIM))
            } else { Span::raw("") },
        ]));
        let text = if show_full {
            turn.response_text.clone()
        } else {
            let p: String = turn.response_text.chars().take(AGENT_RESPONSE_PREVIEW_LEN).collect();
            if p.len() < turn.response_text.len() { format!("{}...", p) } else { p }
        };
        for chunk in word_wrap(&text, wrap_width) {
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(chunk, Style::default().fg(theme::SUBTLE)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // ── STATS ──
    lines.push(Line::from(vec![
        Span::styled("  cost    ", theme::dim_style()),
        Span::styled(format_cost(turn.cost), Style::default().fg(theme::ACCENT)),
        Span::styled("     context  ", theme::dim_style()),
        Span::styled(format!("↑{} cumulative", format_tokens(turn.cumulative_context)), Style::default().fg(theme::PRIMARY)),
        if turn.context_saved > 0 {
            Span::styled(format!("  (saved {} tokens via sub-agents)", format_tokens(turn.context_saved)), theme::subtle_style())
        } else {
            Span::raw("")
        },
    ]));
    lines.push(Line::from(vec![
        Span::styled("  tokens  ", theme::dim_style()),
        Span::styled(format!("↑{}", format_tokens(turn.input_tokens)), Style::default().fg(theme::PRIMARY)),
        Span::styled(" in  ", theme::dim_style()),
        Span::styled(format!("↓{}", format_tokens(turn.output_tokens)), Style::default().fg(theme::WARM)),
        Span::styled(" out  ", theme::dim_style()),
        Span::styled(
            format!("cache read: {}  cache write: {}", format_tokens(turn.cache_read_tokens), format_tokens(turn.cache_write_tokens)),
            theme::subtle_style(),
        ),
    ]));
    lines.push(Line::from(""));

    // ── METRICS ──
    lines.push(Line::from(Span::styled(format!("  {section_sep}"), theme::dim_style())));
    if let Some(ref metrics) = turn.metrics {
        let bar_width = wrap_width.saturating_sub(22).min(35);
        let metric_items: &[(&str, f32)] = &[
            ("acceptance ", metrics.acceptance),
            ("performance", metrics.performance),
            ("confidence ", metrics.confidence),
            ("friction   ", metrics.friction),
            ("hallucinate", metrics.hallucination),
        ];

        for (label, value) in metric_items {
            let filled = (*value as f64 * bar_width as f64) as usize;
            let empty = bar_width.saturating_sub(filled);
            // Thin bar with crimson gradient
            let bar_filled: String = (0..filled).map(|j| {
                let _ = j; // all same char
                '▬'
            }).collect();
            let bar_empty: String = "─".repeat(empty);
            let frac = *value as f64;
            let bar_color = crimson_gradient(1.0 - frac); // brighter for higher values
            let pct = format!(" {:.0}%", value * 100.0);

            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", label), Style::default().fg(theme::SUBTLE)),
                Span::styled(bar_filled, Style::default().fg(bar_color)),
                Span::styled(bar_empty, Style::default().fg(theme::DIM)),
                Span::styled(pct, Style::default().fg(theme::SUBTLE)),
            ]));
        }
        lines.push(Line::from(""));

        // Reasoning
        if !metrics.recap.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  ◈ ", Style::default().fg(Color::Rgb(180, 130, 255))),
                Span::styled("REASONING", Style::default().fg(Color::Rgb(180, 130, 255)).add_modifier(Modifier::BOLD)),
            ]));
            for chunk in word_wrap(&metrics.recap, wrap_width) {
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(chunk, Style::default().fg(theme::SUBTLE)),
                ]));
            }
            lines.push(Line::from(""));
        }
    } else if selected >= total_turns.saturating_sub(2) && turn.output_tokens > 0 {
        // Only the last 2 turns can be "analyzing" — everything else is untracked
        let dot_count = ((app.tick / 8) % 4) as usize;
        let dots = ".".repeat(dot_count + 1);
        let phase = (app.tick % 20) as f64 / 20.0 * std::f64::consts::TAU;
        let bright = phase.sin() * 0.5 + 0.5;
        let r = (120.0 + bright * 100.0) as u8;
        let g = (40.0 + bright * 30.0) as u8;
        let b = (40.0 + bright * 20.0) as u8;
        lines.push(Line::from(vec![
            Span::styled("  ◈ ", Style::default().fg(Color::Rgb(r, g, b))),
            Span::styled(format!("analyzing{}", dots), Style::default().fg(Color::Rgb(r, g, b))),
        ]));
        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  ◈ ", Style::default().fg(theme::DIM)),
            Span::styled("metrics not available", Style::default().fg(theme::DIM)),
        ]));
        lines.push(Line::from(""));
    }

    // ── AGENTS ──
    if !turn.agents.is_empty() {
        lines.push(Line::from(Span::styled(format!("  {section_sep}"), theme::dim_style())));
        lines.push(Line::from(Span::styled(
            format!("  ◆ {} agents spawned", turn.agents.len()),
            Style::default().fg(theme::WARM).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        for agent in &turn.agents {
            lines.push(Line::from(vec![
                Span::styled("  ◆ ", Style::default().fg(theme::WARM).add_modifier(Modifier::BOLD)),
                Span::styled(&agent.name, Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("  {}  ↑{} ↓{}", format_cost(agent.cost), format_tokens(agent.input_tokens), format_tokens(agent.output_tokens)),
                    theme::subtle_style(),
                ),
            ]));
            lines.push(Line::from(""));

            let show_full_agent = app.expanded_view.is_some();

            // Request — truncated unless expanded
            if !agent.prompt.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("    ▸ ", Style::default().fg(theme::ACCENT)),
                    Span::styled("REQUEST", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
                    if !show_full_agent && agent.prompt.len() > AGENT_PROMPT_PREVIEW_LEN {
                        Span::styled("  (e to expand)", Style::default().fg(theme::DIM))
                    } else { Span::raw("") },
                ]));
                let text = if show_full_agent {
                    agent.prompt.clone()
                } else {
                    let p: String = agent.prompt.chars().take(AGENT_PROMPT_PREVIEW_LEN).collect();
                    if p.len() < agent.prompt.len() { format!("{}...", p) } else { p }
                };
                for chunk in word_wrap(&text, wrap_width) {
                    lines.push(Line::from(vec![
                        Span::styled("      ", Style::default()),
                        Span::styled(chunk, Style::default().fg(Color::White)),
                    ]));
                }
                lines.push(Line::from(""));
            }

            // Response — truncated unless expanded
            if !agent.response_preview.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("    ◂ ", Style::default().fg(theme::PRIMARY)),
                    Span::styled("RESPONSE", Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD)),
                    if !show_full_agent && agent.response_preview.len() > AGENT_RESPONSE_PREVIEW_LEN {
                        Span::styled("  (e to expand)", Style::default().fg(theme::DIM))
                    } else { Span::raw("") },
                ]));
                let text = if show_full_agent {
                    agent.response_preview.clone()
                } else {
                    let p: String = agent.response_preview.chars().take(AGENT_RESPONSE_PREVIEW_LEN).collect();
                    if p.len() < agent.response_preview.len() { format!("{}...", p) } else { p }
                };
                for chunk in word_wrap(&text, wrap_width) {
                    lines.push(Line::from(vec![
                        Span::styled("      ", Style::default()),
                        Span::styled(chunk, Style::default().fg(theme::SUBTLE)),
                    ]));
                }
                lines.push(Line::from(""));
            }

            lines.push(Line::from(Span::styled(format!("    {}", "─".repeat(wrap_width.min(40))), theme::dim_style())));
            lines.push(Line::from(""));
        }
    }

    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    let scroll = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0).min(max_scroll);
    let para = Paragraph::new(Text::from(lines)).scroll((scroll, 0));
    frame.render_widget(para, inner);
    max_scroll
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![text.to_string()]; }
    let mut result = Vec::new();
    for line in text.lines() {
        if line.len() <= width {
            result.push(line.to_string());
        } else {
            let mut remaining = line;
            while remaining.len() > width {
                let break_at = remaining[..width].rfind(' ').unwrap_or(width);
                let (chunk, rest) = remaining.split_at(break_at);
                result.push(chunk.to_string());
                remaining = rest.trim_start();
            }
            if !remaining.is_empty() { result.push(remaining.to_string()); }
        }
    }
    result
}

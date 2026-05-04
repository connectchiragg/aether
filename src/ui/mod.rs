pub mod chat_view;
pub mod graph_view;

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{App, View};
use crate::theme;

// Boot sequence phases in ticks (50ms each)
const PHASE_LOGO: u16 = 20;     // 1.0s — logo reveal
const PHASE_TEXT: u16 = 14;     // 0.7s — name + tagline
const PHASE_HOLD: u16 = 10;     // 0.5s — hold

const TAGLINE: &str = "see the invisible";

// Eye-Tree of Sauron — braille art generated from image
const EYE_ART: &[&str] = &[
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣆⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣦⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠹⣷⣄⣀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡀⣴⡾⠃⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⡈⣛⡘⠇⠠⢤⡄⠀⠀⠀⠀⣠⠤⠀⠚⢁⡋⢁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⣹⢯⡁⠀⢀⡾⢃⣤⠀⢀⣄⠘⣦⡀⠀⢨⣽⣏⣀⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣄⠉⠉⣿⠀⠀⣴⢏⣠⣼⣷⣶⣶⣾⣥⣈⢻⡆⠀⢠⡏⠉⠁⡄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠐⣿⡤⠀⢹⣷⣴⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣤⣾⠃⠠⣼⡿⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⢠⣄⣶⡄⢰⢒⣶⣾⢿⣻⡯⣡⣾⣿⣿⡿⣿⣿⣿⣶⡩⣽⣻⣿⣶⣖⣲⡄⣴⣦⢠⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠳⣭⣿⠛⣭⢁⣾⣿⣿⢱⣿⣿⣿⣿⡇⣻⣿⣿⣿⣿⠸⣿⣿⣦⢩⡝⢻⣯⣥⠏⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⣠⠀⠀⠀⠀⠀⠀⠀⢀⣴⠶⢮⣭⣈⢿⣿⣿⠸⣿⣿⣿⣿⡇⣼⣿⣿⣿⣿⢠⣿⣿⢟⣨⣽⡶⢶⣤⠀⠀⠀⠀⠀⠀⠀⢠⡄⠀⠀",
    "⢤⣽⣿⣿⡤⠀⠀⠀⠀⠀⠈⠉⠰⡏⢉⢻⣿⣿⣿⣗⠙⢿⣿⣿⣧⣿⣿⣿⠿⢁⣻⣿⣿⣿⢟⡉⡳⠌⠉⠀⠀⠀⠀⠀⠠⣬⣿⣳⣧⠄",
    "⠀⠊⢿⠙⠀⠀⠀⠀⠀⠀⠘⠷⣶⠿⢿⠆⠀⠙⠻⢿⣿⣶⣾⣭⣽⣭⣽⣶⣾⣿⠿⠛⠁⠀⢾⡿⢷⡶⠞⠀⠀⠀⠀⠀⠀⠐⠹⡏⠃⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠉⠻⣿⣿⣿⣿⣿⡿⠛⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⢿⣿⣿⡟⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠘⣿⣿⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⢀⣻⣴⣦⡤⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢤⣴⣴⣇⠀⠀⠀⠀",
    "⠀⠀⠀⠠⠿⡿⣟⠂⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⣿⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠒⣿⣾⠿⠄⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠁⠉⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣼⣿⣿⣦⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠁⠈⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣴⣶⣿⣿⣿⣿⣿⣿⣿⣷⣶⢤⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠠⣄⣦⣾⡀⠀⠀⠈⠈⠻⠆⢿⢩⡟⢿⢹⡇⠰⠛⠀⠁⠀⠀⣘⣦⣶⣤⠄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠐⢺⣷⡿⠦⠀⠀⠀⠀⠀⠀⠈⢈⢷⡞⡈⠁⠀⠀⠀⠀⠀⢀⠼⣿⢿⡓⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠀⠁⠀⠀⠀⠀⠀⠀⠀⠰⢼⣟⣿⠧⠄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠹⠋⠃⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
];

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.view {
        View::Boot => render_boot(frame, app),
        View::Graph => render_graph_view(frame, app),
        _ => render_main(frame, app),
    }
}

fn render_graph_view(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    render_header(frame, app, chunks[0]);
    graph_view::render(frame, app, chunks[1]);
    render_status_bar(frame, app, chunks[2]);
}

fn render_boot(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let t = app.boot_ticks;
    let logo_progress = (t as f32 / PHASE_LOGO as f32).min(1.0);
    let text_progress = if t > PHASE_LOGO {
        ((t - PHASE_LOGO) as f32 / PHASE_TEXT as f32).min(1.0)
    } else {
        0.0
    };

    let art_height = EYE_ART.len() as u16;
    let total_height = art_height + 6;
    let top_pad = area.height.saturating_sub(total_height) / 2;

    let chunks = Layout::vertical([
        Constraint::Length(top_pad),
        Constraint::Length(art_height),
        Constraint::Length(2),
        Constraint::Length(1), // name
        Constraint::Length(1), // tagline
        Constraint::Length(2),
        Constraint::Length(1), // status
        Constraint::Min(0),
    ])
    .split(area);

    // Eye art — lava gradient with breathing pulse after reveal
    let total = EYE_ART.len() as f32;
    let reveal_f = total * logo_progress;
    let lines_to_show = reveal_f.ceil() as usize;
    let glow_line = if lines_to_show > 0 { lines_to_show - 1 } else { 0 };

    // After reveal: one slow scan sweep top to bottom
    let scan_pos = if logo_progress >= 1.0 {
        let ticks_since = (t as f32) - (PHASE_LOGO as f32);
        (ticks_since * 0.03).min(1.2) // single slow sweep, slightly past bottom
    } else {
        -1.0
    };

    let mut art_lines: Vec<Line> = Vec::new();
    for (i, line) in EYE_ART.iter().enumerate() {
        if i < lines_to_show {
            let color = if !theme::is_truecolor() {
                // 256-color gradient with scan-line glow
                let t_pos = i as f32 / total;
                let dist = (t_pos - scan_pos).abs();
                let is_glow = dist < 0.15 && logo_progress >= 1.0;

                if is_glow && dist < 0.05 {
                    // Scan-line center: bright yellow/white
                    Color::Indexed(220) // gold
                } else if is_glow {
                    // Scan-line edge: bright red
                    Color::Indexed(203) // salmon
                } else {
                    // Base gradient: red shades top to bottom
                    match (i * 4 / EYE_ART.len().max(1)) {
                        0 => Color::Indexed(196), // bright red
                        1 => Color::Indexed(160), // medium red
                        2 => Color::Indexed(124), // dark red
                        _ => Color::Indexed(88),  // deep red
                    }
                }
            } else if i == glow_line && logo_progress < 1.0 {
                Color::Rgb(255, 200, 150)
            } else {
                let t_pos = i as f32 / total;
                let base_r = 240.0 - 80.0 * t_pos;
                let base_g = 100.0 - 70.0 * t_pos;
                let base_b = 40.0 - 20.0 * t_pos;

                let dist = (t_pos - scan_pos).abs();
                let glow = if dist < 0.15 { 1.0 - (dist / 0.15) } else { 0.0 };

                let r = (base_r + (255.0 - base_r) * glow).min(255.0) as u8;
                let g = (base_g + (180.0 - base_g) * glow).min(255.0) as u8;
                let b = (base_b + (80.0 - base_b) * glow).min(255.0) as u8;
                Color::Rgb(r, g, b)
            };
            art_lines.push(Line::from(Span::styled(
                *line,
                Style::default().fg(color),
            )));
        } else {
            art_lines.push(Line::from(""));
        }
    }

    let art_widget = Paragraph::new(art_lines).alignment(Alignment::Center);
    frame.render_widget(art_widget, chunks[1]);

    // Name
    if text_progress > 0.0 {
        let name_widget = Paragraph::new(Line::from(Span::styled(
            "A  E  T  H  E  R",
            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(name_widget, chunks[3]);
    }

    // Tagline
    if text_progress > 0.5 {
        let tagline = Paragraph::new(Line::from(Span::styled(
            TAGLINE,
            Style::default().fg(theme::subtle()),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(tagline, chunks[4]);
    }

    // Status
    if logo_progress > 0.2 {
        let dots = ".".repeat(((t / 4) % 4) as usize + 1);
        let status_text = if app.engine.is_live() {
            format!("scanning for sessions{dots}")
        } else {
            format!("loading demo{dots}")
        };
        let status = Paragraph::new(Line::from(Span::styled(
            status_text,
            Style::default().fg(theme::dim()),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(status, chunks[6]);
    }
}

fn render_main(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    render_header(frame, app, chunks[0]);
    chat_view::render(frame, app, chunks[1]);
    render_status_bar(frame, app, chunks[2]);
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    use crate::model::{format_cost, format_tokens};

    let status = app.engine.status_text();

    // Detect truecolor support
    let truecolor = std::env::var("COLORTERM")
        .map(|v| v == "truecolor" || v == "24bit")
        .unwrap_or(false);

    // Pulsing eye: use 2 alternating states for basic terminals, smooth RGB for truecolor
    let pulse_style = if app.paused {
        Style::default().fg(theme::dim())
    } else if truecolor {
        let phase = (app.tick % 30) as f64 / 30.0 * std::f64::consts::TAU;
        let bright = phase.sin() * 0.5 + 0.5;
        let r = (40.0 + bright * 215.0) as u8;
        let g = (10.0 + bright * 50.0) as u8;
        let b = (10.0 + bright * 40.0) as u8;
        Style::default().fg(Color::Rgb(r, g, b))
    } else {
        // Basic terminal: alternate between red and dark red
        if app.tick % 30 < 15 {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    };

    // ── Left: braille eye + "aether" ──
    let mut left_spans: Vec<Span> = vec![
        Span::styled(" ⠑⠽⠑", pulse_style),
        Span::raw(" "),
    ];

    if truecolor {
        // Smooth sweep for truecolor terminals
        let t = app.tick as f64 * 0.02;
        let sweep_pos = t.sin() * 0.5 + 0.5;
        for (i, ch) in "aether".chars().enumerate() {
            let char_pos = i as f64 / 5.0;
            let dist = (char_pos - sweep_pos).abs();
            let glow = (-dist * dist * 12.0).exp();
            let r = (130.0 + glow * 125.0) as u8;
            let g = (40.0 + glow * 50.0) as u8;
            let b = (35.0 + glow * 35.0) as u8;
            left_spans.push(Span::styled(ch.to_string(), Style::default().fg(Color::Rgb(r, g, b)).add_modifier(Modifier::BOLD)));
        }
    } else {
        // Static styled name for basic terminals
        left_spans.push(Span::styled("aether", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
    }

    left_spans.push(Span::styled(" │", Style::default().fg(theme::dim())));

    // ── Center: contextual info ──
    if app.view == View::Sessions {
        let session_count = app.engine.live_engine()
            .map(|l| l.session_count())
            .unwrap_or(0);
        left_spans.push(Span::styled(
            format!(" {} sessions", session_count),
            Style::default().fg(theme::subtle()),
        ));
        left_spans.push(Span::styled(" ─ ", Style::default().fg(theme::dim())));
        let status_style = if status == "watching" {
            Style::default().fg(theme::accent())
        } else {
            Style::default().fg(theme::warm())
        };
        left_spans.push(Span::styled("● ", pulse_style));
        left_spans.push(Span::styled(status, status_style));
    } else if let Some(live) = app.engine.live_engine() {
        let session_name = live.active_session_name();
        let session = live.sessions.get(live.active_idx);
        let session_count = live.session_count();

        // Session position
        left_spans.push(Span::styled(
            format!(" {}/{}", live.active_idx + 1, session_count),
            Style::default().fg(theme::subtle()),
        ));
        left_spans.push(Span::styled(" │", Style::default().fg(theme::dim())));

        // Session name
        let name_display: String = session_name.chars().take(40).collect();
        left_spans.push(Span::styled(
            format!(" {}", name_display),
            Style::default().fg(theme::primary()).add_modifier(Modifier::BOLD),
        ));

        // Session cost & tokens (right side)
        if let Some(s) = session {
            let total_cost = s.usage.total_cost();
            let total_in: u64 = s.usage.turns.iter().map(|t| t.input_tokens).sum();
            let total_out: u64 = s.usage.turns.iter().map(|t| t.output_tokens).sum();
            let turn_count = s.usage.turn_count();

            left_spans.push(Span::styled(" ─ ", Style::default().fg(theme::dim())));
            left_spans.push(Span::styled("● ", pulse_style));
            left_spans.push(Span::styled(
                format!("{}", format_cost(total_cost)),
                Style::default().fg(theme::warm()),
            ));
            left_spans.push(Span::styled(
                format!("  ↑{} ↓{}  {} turns",
                    format_tokens(total_in),
                    format_tokens(total_out),
                    turn_count,
                ),
                Style::default().fg(theme::subtle()),
            ));
        }
    } else {
        left_spans.push(Span::styled(" demo", Style::default().fg(theme::primary())));
        left_spans.push(Span::styled(" ─ ", Style::default().fg(theme::dim())));
        left_spans.push(Span::styled(status, Style::default().fg(theme::accent())));
    }

    if app.paused {
        left_spans.push(Span::styled("  ⏸ paused", Style::default().fg(theme::warm())));
    }

    let title_line = Line::from(left_spans);

    // Render as a styled block with top border accent
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::dim()))
        .title(title_line);

    frame.render_widget(block, area);

    // Draw accent line on top border (truecolor only)
    if truecolor {
        let top_y = area.y;
        let width = area.width.min(8);
        for x in area.x..area.x + width {
            let frac = (x - area.x) as f64 / width as f64;
            let r = (255.0 - frac * 95.0) as u8;
            let g = (90.0 - frac * 60.0) as u8;
            let b = (70.0 - frac * 50.0) as u8;
            let cell = frame.buffer_mut().cell_mut((x, top_y));
            if let Some(cell) = cell {
                cell.set_style(Style::default().fg(Color::Rgb(r, g, b)));
            }
        }
    }
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    // Each entry: (key, description)
    let items: &[(&str, &str)] = if app.rename_input.is_some() {
        &[("enter", "confirm"), ("esc", "cancel")]
    } else if app.view == View::Sessions {
        &[("↑↓", "navigate"), ("enter", "open"), ("r", "rename"), ("q", "quit")]
    } else if app.view == View::Graph {
        if app.graph_jump_input.is_some() {
            &[("0-9", "turn #"), ("enter", "go"), ("esc", "cancel")]
        } else {
            &[
                ("←→", "turns"), ("↑↓", "session"), ("h/l", "first/last turn"),
                ("g", "goto turn"), ("c", "change graph"), ("+/-", "zoom in/out graph"),
                ("e", "expand/collapse"), ("esc", "back"), ("q", "quit"),
            ]
        }
    } else if app.engine.is_live() {
        &[("←→", "focus"), ("↑↓", "scroll"), ("n/p", "session"), ("space", "pause"), ("esc", "back"), ("q", "quit")]
    } else {
        &[("←→", "focus"), ("↑↓", "scroll"), ("space", "pause"), ("r", "reset"), ("q", "quit")]
    };

    let mut spans: Vec<Span> = Vec::new();
    for (i, (key, desc)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(theme::dim())));
        }
        spans.push(Span::styled(format!(" {}", key), Style::default().fg(theme::accent())));
        spans.push(Span::styled(format!(" {}", desc), theme::subtle_style()));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

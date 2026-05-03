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
            let color = if i == glow_line && logo_progress < 1.0 {
                Color::Rgb(255, 200, 150)
            } else {
                // Lava gradient: orange top → red middle → dark crimson bottom
                let t_pos = i as f32 / total;
                let base_r = 240.0 - 80.0 * t_pos;
                let base_g = 100.0 - 70.0 * t_pos;
                let base_b = 40.0 - 20.0 * t_pos;

                // Scan line: soft bright band
                let dist = (t_pos - scan_pos).abs();
                let glow = if dist < 0.15 {
                    1.0 - (dist / 0.15)
                } else {
                    0.0
                };

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
            Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(name_widget, chunks[3]);
    }

    // Tagline
    if text_progress > 0.5 {
        let tagline = Paragraph::new(Line::from(Span::styled(
            TAGLINE,
            Style::default().fg(theme::SUBTLE),
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
            Style::default().fg(theme::DIM),
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
    let agent_count = app.engine.agents().len();
    let status = app.engine.status_text();

    let status_style = match status {
        "watching" => Style::default().fg(theme::ACCENT),
        "waiting for sessions" => Style::default().fg(theme::WARM),
        "done" => Style::default().fg(theme::DIM),
        _ => Style::default().fg(Color::White),
    };

    let pause_indicator = if app.paused { "  paused" } else { "" };

    let mut spans = vec![
        Span::styled(" ◉ ", Style::default().fg(Color::Rgb(200, 30, 30)).add_modifier(Modifier::BOLD)),
        Span::styled("aether ", theme::header_title_style()),
    ];

    if app.view == View::Sessions {
        // Session list view — just show status
        spans.push(Span::styled(status, status_style));
    } else if let Some(live) = app.engine.live_engine() {
        let session_count = live.session_count();
        let session_name = live.active_session_name();

        if session_count > 1 {
            spans.push(Span::styled(
                format!("[{}/{}] ", live.active_idx + 1, session_count),
                Style::default().fg(theme::PRIMARY).add_modifier(Modifier::BOLD),
            ));
        }

        spans.push(Span::styled(
            session_name,
            Style::default().fg(theme::PRIMARY),
        ));
        spans.push(Span::styled("  ", theme::dim_style()));
        spans.push(Span::styled(status, status_style));

        if agent_count > 0 {
            spans.push(Span::styled(
                format!("  {agent_count} agents"),
                theme::subtle_style(),
            ));
        }
    } else {
        spans.push(Span::styled(
            "demo",
            Style::default().fg(theme::PRIMARY),
        ));
        spans.push(Span::styled("  ", theme::dim_style()));
        spans.push(Span::styled(status, status_style));
    }

    spans.push(Span::styled(
        pause_indicator,
        Style::default().fg(theme::WARM),
    ));

    let title_line = Line::from(spans);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::dim_style())
        .title(title_line);

    frame.render_widget(block, area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help = if app.rename_input.is_some() {
        " enter confirm  esc cancel"
    } else if app.view == View::Sessions {
        " q quit  up/down navigate  enter select  r rename"
    } else if app.view == View::Graph {
        if app.graph_jump_input.is_some() {
            " type turn number  enter go  esc cancel"
        } else {
            " q quit  left/right turns  h first  l last  g go to turn  up/down scroll  esc back"
        }
    } else if app.engine.is_live() {
        " q quit  left/right focus  up/down scroll  n/p session  esc back  space pause"
    } else {
        " q quit  left/right focus  up/down scroll  space pause  r reset"
    };

    let spans = help
        .split("  ")
        .enumerate()
        .flat_map(|(i, part)| {
            let mut result = Vec::new();
            if i > 0 {
                result.push(Span::styled("  ", theme::dim_style()));
            }
            if let Some((key, desc)) = part.trim().split_once(' ') {
                result.push(Span::styled(
                    key.to_string(),
                    Style::default().fg(theme::ACCENT),
                ));
                result.push(Span::styled(
                    format!(" {desc}"),
                    theme::subtle_style(),
                ));
            } else {
                result.push(Span::styled(part.to_string(), theme::subtle_style()));
            }
            result
        })
        .collect::<Vec<_>>();

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

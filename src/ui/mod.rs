pub mod chat_view;

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
const REVEAL_TICKS: u16 = 30;  // 1.5s — banner character reveal
const HOLD_TICKS: u16 = 10;    // 0.5s — hold completed banner
const BOOT_DURATION: u16 = REVEAL_TICKS + HOLD_TICKS; // 2.5s total

const BANNER: &[&str] = &[
    "         _   _               ",
    "   __ _ ___| |_| |__   ___ _ __ ",
    "  / _` / _ \\  _| '_ \\ / -_) '_|",
    "  \\__,_\\___/\\__|_| |_|\\___|_|  ",
];

const TAGLINE: &str = "see the invisible";

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.view {
        View::Boot => render_boot(frame, app),
        _ => render_main(frame, app),
    }
}

fn render_boot(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let reveal_progress = (app.boot_ticks as f32 / REVEAL_TICKS as f32).min(1.0);
    let in_hold = app.boot_ticks >= REVEAL_TICKS;

    // Center the banner vertically
    let banner_height = BANNER.len() as u16 + 8;
    let top_pad = area.height.saturating_sub(banner_height) / 2;

    let chunks = Layout::vertical([
        Constraint::Length(top_pad),
        Constraint::Length(BANNER.len() as u16),
        Constraint::Length(2), // spacing
        Constraint::Length(1), // tagline
        Constraint::Length(2), // spacing
        Constraint::Length(1), // status line
        Constraint::Min(0),
    ])
    .split(area);

    // Banner — character-by-character reveal during reveal phase, fully lit during hold
    let total_chars: usize = BANNER.iter().map(|l| l.len()).sum();
    let revealed = if in_hold {
        total_chars
    } else {
        (total_chars as f32 * reveal_progress) as usize
    };
    let mut chars_shown = 0;
    let mut banner_lines: Vec<Line> = Vec::new();

    for line in BANNER {
        let mut spans = Vec::new();
        for ch in line.chars() {
            if chars_shown < revealed {
                // Glow effect: recently revealed chars are brighter
                let style = if in_hold {
                    Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
                } else if chars_shown + 8 >= revealed {
                    // Leading edge — bright white
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
                };
                spans.push(Span::styled(ch.to_string(), style));
            } else {
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(theme::DIM),
                ));
            }
            chars_shown += 1;
        }
        banner_lines.push(Line::from(spans));
    }

    let banner = Paragraph::new(banner_lines).alignment(Alignment::Center);
    frame.render_widget(banner, chunks[1]);

    // Tagline — fades in during hold phase
    if reveal_progress > 0.7 || in_hold {
        let tagline = Paragraph::new(Line::from(Span::styled(
            TAGLINE,
            Style::default().fg(if in_hold { theme::SUBTLE } else { theme::DIM }),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(tagline, chunks[3]);
    }

    // Status line
    if reveal_progress > 0.3 || in_hold {
        let dots = ".".repeat(((app.boot_ticks / 4) % 4) as usize + 1);
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
        frame.render_widget(status, chunks[5]);
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
        Span::styled(" aether ", theme::header_title_style()),
        Span::styled(" ", theme::dim_style()),
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

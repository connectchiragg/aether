use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;

// ── Truecolor detection ────────────────────────────────────
static TRUECOLOR: OnceLock<bool> = OnceLock::new();

pub fn is_truecolor() -> bool {
    *TRUECOLOR.get_or_init(|| {
        std::env::var("COLORTERM")
            .map(|v| v == "truecolor" || v == "24bit")
            .unwrap_or(false)
    })
}

// ── Aether palette ──────────────────────────────────────────
// Deep crimson identity with warm ember accents
// Each color has a truecolor and a basic 16-color fallback

pub fn primary() -> Color {
    if is_truecolor() {
        Color::Rgb(220, 60, 60)
    } else {
        Color::Red
    }
}
pub fn accent() -> Color {
    if is_truecolor() {
        Color::Rgb(255, 90, 70)
    } else {
        Color::LightRed
    }
}
pub fn dim() -> Color {
    if is_truecolor() {
        Color::Rgb(80, 70, 75)
    } else {
        Color::DarkGray
    }
}
pub fn surface() -> Color {
    if is_truecolor() {
        Color::Rgb(40, 36, 38)
    } else {
        Color::Black
    }
}
pub fn subtle() -> Color {
    if is_truecolor() {
        Color::Rgb(120, 100, 105)
    } else {
        Color::Gray
    }
}
pub fn warm() -> Color {
    if is_truecolor() {
        Color::Rgb(255, 170, 80)
    } else {
        Color::Yellow
    }
}

// Legacy constants — used by code that hasn't switched to functions yet
pub const PRIMARY: Color = Color::Rgb(220, 60, 60);
pub const ACCENT: Color = Color::Rgb(255, 90, 70);
pub const DIM: Color = Color::Rgb(80, 70, 75);
pub const SURFACE: Color = Color::Rgb(40, 36, 38);
pub const SUBTLE: Color = Color::Rgb(120, 100, 105);
pub const WARM: Color = Color::Rgb(255, 170, 80);

// Agent color pool — distinct, readable on dark backgrounds
pub const AGENT_COLORS: &[Color] = &[
    Color::Rgb(255, 140, 90),  // ember
    Color::Rgb(140, 200, 100), // soft green
    Color::Rgb(255, 200, 80),  // gold
    Color::Rgb(200, 140, 255), // lavender
    Color::Rgb(100, 180, 255), // sky blue
    Color::Rgb(255, 100, 100), // coral
    Color::Rgb(0, 200, 160),   // mint
    Color::Rgb(255, 160, 200), // pink
];

pub fn status_bar_style() -> Style {
    Style::default().fg(subtle())
}

pub fn focused_border_style() -> Style {
    Style::default().fg(primary())
}

pub fn unfocused_border_style() -> Style {
    Style::default().fg(dim())
}

pub fn header_title_style() -> Style {
    Style::default().fg(accent()).add_modifier(Modifier::BOLD)
}

pub fn dim_style() -> Style {
    Style::default().fg(dim())
}

pub fn subtle_style() -> Style {
    Style::default().fg(subtle())
}

use ratatui::style::{Color, Modifier, Style};

// ── Aether palette ──────────────────────────────────────────
// Ethereal blues and cyans with warm accents
pub const PRIMARY: Color = Color::Rgb(120, 180, 255); // soft blue
pub const ACCENT: Color = Color::Rgb(0, 220, 200); // teal
pub const DIM: Color = Color::Rgb(80, 80, 100); // muted gray-blue
pub const SURFACE: Color = Color::Rgb(40, 42, 54); // dark surface
pub const SUBTLE: Color = Color::Rgb(100, 110, 130); // subtle text
pub const WARM: Color = Color::Rgb(255, 180, 100); // warm amber

// Agent color pool — distinct, readable on dark backgrounds
pub const AGENT_COLORS: &[Color] = &[
    Color::Rgb(0, 220, 200),   // teal
    Color::Rgb(140, 200, 100), // soft green
    Color::Rgb(255, 200, 80),  // gold
    Color::Rgb(200, 140, 255), // lavender
    Color::Rgb(100, 180, 255), // sky blue
    Color::Rgb(255, 130, 130), // coral
    Color::Rgb(0, 200, 160),   // mint
    Color::Rgb(255, 160, 200), // pink
];

pub fn status_bar_style() -> Style {
    Style::default().fg(SUBTLE)
}

pub fn focused_border_style() -> Style {
    Style::default().fg(PRIMARY)
}

pub fn unfocused_border_style() -> Style {
    Style::default().fg(DIM)
}

pub fn header_title_style() -> Style {
    Style::default()
        .fg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

pub fn dim_style() -> Style {
    Style::default().fg(DIM)
}

pub fn subtle_style() -> Style {
    Style::default().fg(SUBTLE)
}

use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;

use crate::provider::ProviderKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorDepth {
    Basic,
    Indexed,
    TrueColor,
}

static COLOR_DEPTH: OnceLock<ColorDepth> = OnceLock::new();

fn color_depth() -> ColorDepth {
    *COLOR_DEPTH.get_or_init(|| {
        let color_term = std::env::var("COLORTERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(color_term.as_str(), "truecolor" | "24bit") {
            return ColorDepth::TrueColor;
        }

        let term = std::env::var("TERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if term.contains("256color")
            || std::env::var_os("WT_SESSION").is_some()
            || std::env::var_os("TERM_PROGRAM").is_some()
        {
            return ColorDepth::Indexed;
        }

        ColorDepth::Basic
    })
}

pub fn is_truecolor() -> bool {
    color_depth() == ColorDepth::TrueColor
}

pub(crate) fn adaptive(rgb: (u8, u8, u8), indexed: u8, basic: Color) -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
        ColorDepth::Indexed => Color::Indexed(indexed),
        ColorDepth::Basic => basic,
    }
}

// Aether's persistent identity uses the darker red from the opening reveal.
// Brighter coral is reserved for focus and live-state accents.

pub fn primary() -> Color {
    adaptive((174, 38, 32), 124, Color::Red)
}
pub fn accent() -> Color {
    adaptive((222, 60, 47), 160, Color::LightRed)
}
pub fn dim() -> Color {
    adaptive((65, 69, 77), 238, Color::DarkGray)
}
pub fn surface() -> Color {
    adaptive((24, 26, 31), 234, Color::Black)
}
pub fn surface_high() -> Color {
    adaptive((32, 35, 42), 236, Color::Black)
}
pub fn subtle() -> Color {
    adaptive((139, 144, 154), 245, Color::Gray)
}
pub fn text() -> Color {
    adaptive((235, 237, 241), 255, Color::White)
}
pub fn warm() -> Color {
    adaptive((244, 190, 79), 221, Color::Yellow)
}
pub fn cool() -> Color {
    adaptive((80, 190, 214), 80, Color::Cyan)
}
pub fn positive() -> Color {
    adaptive((91, 201, 149), 78, Color::Green)
}
pub fn violet() -> Color {
    adaptive((178, 139, 230), 176, Color::Magenta)
}
pub fn rose() -> Color {
    adaptive((235, 111, 151), 204, Color::LightMagenta)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderPalette {
    pub primary: Color,
    pub accent: Color,
    pub highlight: Color,
    pub text: Color,
    pub subtle: Color,
    pub dim: Color,
    pub surface: Color,
    pub surface_high: Color,
    pub danger: Color,
}

pub fn provider_palette(_provider: Option<ProviderKind>) -> ProviderPalette {
    ProviderPalette {
        primary: primary(),
        accent: accent(),
        highlight: warm(),
        text: text(),
        subtle: subtle(),
        dim: dim(),
        surface: surface(),
        surface_high: surface_high(),
        danger: primary(),
    }
}

// Legacy constants — used by code that hasn't switched to functions yet
pub const PRIMARY: Color = Color::Rgb(174, 38, 32);
pub const ACCENT: Color = Color::Rgb(222, 60, 47);
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

pub fn text_style() -> Style {
    Style::default().fg(text())
}

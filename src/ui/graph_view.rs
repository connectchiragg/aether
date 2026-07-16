use rand::{rngs::StdRng, Rng, SeedableRng};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
use crate::model::{
    format_cost, format_duration, format_tokens, pricing_catalog_metadata, pricing_source_at,
    AttributionCategory, AttributionNode, TurnUsage,
};
use crate::provider::ProviderKind;
use crate::theme;
use chrono::{NaiveDate, Utc};

const METRIC_NAMES: &[&str] = &[
    "cost est.",
    "duration",
    "tokens",
    "context",
    "turn complexity",
    "code diff",
];
const DASHBOARD_METRICS: [u8; 6] = [3, 1, 0, 2, 4, 5];
const WIDE_DASHBOARD_WIDTH: u16 = 120;
const MEDIUM_DASHBOARD_WIDTH: u16 = 72;
const CONTEXT_METRIC_INDEX: usize = 3;
const COMPLEXITY_METRIC_INDEX: usize = 4;
const SPIKE_MEDIAN_MULTIPLIER: f64 = 3.0;

const PROMPT_PREVIEW_LEN: usize = 240;
const AGENT_PROMPT_PREVIEW_LEN: usize = 200;
const AGENT_RESPONSE_PREVIEW_LEN: usize = 300;

/// Render the combined graph + detail view for the selected session.
pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let provider = app
        .engine
        .live_engine()
        .and_then(|engine| engine.active_provider);
    let turns: Vec<TurnUsage> = app
        .engine
        .live_engine()
        .and_then(|e| e.sessions.get(e.active_idx))
        .map(|s| s.usage.turns.clone())
        .unwrap_or_default();

    if turns.is_empty() {
        render_empty(frame, provider, area);
        return;
    }

    if app.selected_dot >= turns.len() {
        app.selected_dot = turns.len().saturating_sub(1);
    }

    let selected = app.selected_dot;
    let callout_side = callout_side_for_navigation(app.graph_navigation_direction);
    let columns = dashboard_columns(area.width, area.height);
    let rows = DASHBOARD_METRICS.len().div_ceil(columns);
    let (dashboard_height, _) = dashboard_layout_heights(area.height, rows);
    let telemetry_document = build_telemetry_document(
        &turns[selected],
        selected,
        app,
        area.width.saturating_sub(2),
    );
    let telemetry_height = telemetry_document.height().max(3);
    let attribution_document =
        build_attribution_document(&turns[selected], app, area.width.saturating_sub(2));
    let attribution_height = attribution_document.height().max(3);
    let detail_document =
        build_detail_document(&turns, selected, app, area.width.saturating_sub(2));
    let detail_height = detail_panel_height(
        &detail_document,
        area.width.saturating_sub(2),
        app.expanded_view.is_some(),
    );
    let page_height = telemetry_height
        .saturating_add(dashboard_height)
        .saturating_add(attribution_height)
        .saturating_add(detail_height);
    let mut page = Buffer::empty(Rect::new(0, 0, area.width, page_height));
    let mut window_start = app.graph_window_start;
    render_dashboard(
        &mut page,
        &turns,
        selected,
        provider,
        callout_side,
        &mut window_start,
        Rect::new(
            0,
            telemetry_height.saturating_add(detail_height),
            area.width,
            dashboard_height,
        ),
    );
    render_telemetry_document(
        &mut page,
        telemetry_document,
        Rect::new(0, 0, area.width, telemetry_height),
    );
    render_detail_document(
        &mut page,
        detail_document,
        Rect::new(0, telemetry_height, area.width, detail_height),
    );
    render_attribution_document(
        &mut page,
        attribution_document,
        Rect::new(
            0,
            telemetry_height
                .saturating_add(detail_height)
                .saturating_add(dashboard_height),
            area.width,
            attribution_height,
        ),
    );
    app.graph_window_start = window_start;

    let max_scroll = page_height.saturating_sub(area.height);
    let cur = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0);
    let scroll = cur.min(max_scroll);
    app.pane_scrolls.insert(usize::MAX, scroll);
    app.pane_max_scrolls.insert(usize::MAX, max_scroll);
    blit_page(frame, area, &page, scroll);
}

fn blit_page(frame: &mut Frame, area: Rect, page: &Buffer, scroll: u16) {
    for row in 0..area.height {
        for column in 0..area.width {
            let destination = (area.x + column, area.y + row);
            let source_y = scroll.saturating_add(row);
            let source = page.cell((column, source_y)).cloned().unwrap_or_default();
            if let Some(cell) = frame.buffer_mut().cell_mut(destination) {
                *cell = source;
            }
        }
    }
}

fn dashboard_layout_heights(area_height: u16, rows: usize) -> (u16, u16) {
    let compact_height = area_height < 30;
    let minimum_detail_height = if compact_height { 4 } else { 10 };
    let dashboard_percent = if compact_height { 90 } else { 80 };
    let minimum_panel_height = if compact_height { 16 } else { 20 };
    let dashboard_height = (area_height.saturating_mul(dashboard_percent) / 100)
        .max((rows as u16).saturating_mul(minimum_panel_height))
        .min(area_height.saturating_sub(minimum_detail_height));
    (dashboard_height, minimum_detail_height)
}

fn dashboard_columns(width: u16, height: u16) -> usize {
    if width >= WIDE_DASHBOARD_WIDTH || (width >= 78 && height < 30) {
        3
    } else if width >= MEDIUM_DASHBOARD_WIDTH {
        2
    } else {
        1
    }
}

fn dashboard_panel_areas(area: Rect) -> Vec<Rect> {
    let columns = dashboard_columns(area.width, area.height);
    let rows = DASHBOARD_METRICS.len().div_ceil(columns);
    let row_constraints = vec![Constraint::Ratio(1, rows as u32); rows];
    let row_areas = Layout::vertical(row_constraints).split(area);
    let mut panels = Vec::with_capacity(DASHBOARD_METRICS.len());

    for row in row_areas.iter() {
        let column_constraints = vec![Constraint::Ratio(1, columns as u32); columns];
        let column_areas = Layout::horizontal(column_constraints).split(*row);
        for column in column_areas.iter() {
            if panels.len() == DASHBOARD_METRICS.len() {
                return panels;
            }
            panels.push(*column);
        }
    }

    panels
}

fn shared_turn_window(
    turn_count: usize,
    selected: usize,
    minimum_panel_width: u16,
    current_start: usize,
) -> (usize, usize) {
    let plot_width = minimum_panel_width.saturating_sub(12) as usize;
    let max_dots = (plot_width / 2).clamp(4, 20);

    if turn_count <= max_dots {
        return (0, turn_count);
    }

    let maximum_start = turn_count.saturating_sub(max_dots);
    let mut start = current_start.min(maximum_start);
    if selected < start {
        start = selected;
    } else if selected >= start + max_dots {
        start = selected + 1 - max_dots;
    }
    (start, (start + max_dots).min(turn_count))
}

fn render_dashboard(
    buffer: &mut Buffer,
    turns: &[TurnUsage],
    selected: usize,
    provider: Option<ProviderKind>,
    callout_side: CalloutDirection,
    window_start: &mut usize,
    area: Rect,
) {
    let panels = dashboard_panel_areas(area);
    let minimum_panel_width = panels.iter().map(|panel| panel.width).min().unwrap_or(0);
    let visible_range =
        shared_turn_window(turns.len(), selected, minimum_panel_width, *window_start);
    *window_start = visible_range.0;

    for (metric, panel) in DASHBOARD_METRICS.iter().zip(panels) {
        render_metric_panel(
            buffer,
            turns,
            selected,
            *metric,
            provider,
            callout_side,
            visible_range,
            panel,
        );
    }
}

fn render_empty(frame: &mut Frame, provider: Option<ProviderKind>, area: Rect) {
    let palette = theme::provider_palette(provider);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.dim))
        .title(Span::styled(
            " telemetry explorer ",
            Style::default().fg(palette.primary),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let msg = Paragraph::new(Line::from(Span::styled(
        "no usage data yet",
        Style::default().fg(palette.subtle),
    )))
    .alignment(ratatui::layout::Alignment::Center);
    let pad =
        Layout::vertical([Constraint::Length(inner.height / 2), Constraint::Min(0)]).split(inner);
    frame.render_widget(msg, pad[1]);
}

fn metric_value(turn: &TurnUsage, metric: u8) -> Option<f64> {
    match metric {
        0 if turn.cost_known || turn.cost > 0.0 => Some(turn.cost),
        0 => None,
        1 => turn.telemetry.duration_ms.map(|duration| duration as f64),
        2 => Some((turn.input_tokens + turn.output_tokens) as f64),
        3 => turn.telemetry.context_percent(),
        4 => turn.telemetry.complexity_percent(),
        5 => Some(turn.diff_lines() as f64),
        _ => None,
    }
}

fn metric_series(turns: &[TurnUsage], metric: u8) -> Vec<Option<f64>> {
    let mut last_context = None;

    turns
        .iter()
        .map(|turn| {
            if metric as usize != CONTEXT_METRIC_INDEX {
                return metric_value(turn, metric);
            }

            if let Some(context) = turn.telemetry.context_percent() {
                last_context = Some(context);
                Some(context)
            } else {
                last_context
            }
        })
        .collect()
}

fn format_axis_value(metric: usize, value: f64) -> String {
    match metric {
        0 if value == 0.0 => "$0".to_string(),
        0 if value < 0.01 => format!("${value:.3}"),
        0 => format_cost(value),
        1 => format_duration(value.max(0.0).round() as u64),
        2 => format!("{} tok", format_tokens(value.max(0.0).round() as u64)),
        3 | 4 => format!("{:.0}%", value.clamp(0.0, 100.0)),
        5 => format!("{} lines", value.max(0.0).round() as u64),
        _ => "0".to_string(),
    }
}

fn format_complexity_percent(value: f64) -> String {
    if value > 0.0 && value < 10.0 {
        format!("{value:.1}%")
    } else {
        format!("{value:.0}%")
    }
}

fn cache_ratio(turn: &TurnUsage) -> f64 {
    if turn.cache_read_tokens == 0 {
        return 0.0;
    }
    let denominator = if turn.cache_read_tokens <= turn.input_tokens {
        turn.input_tokens
    } else {
        turn.input_tokens + turn.cache_read_tokens
    };
    turn.cache_read_tokens as f64 / denominator.max(1) as f64
}

fn metric_color(_metric: usize, provider: Option<ProviderKind>) -> Color {
    theme::provider_palette(provider).primary
}

fn draw_continuous_line(
    grid: &mut [Vec<(char, Style)>],
    positions: &[(usize, Option<usize>)],
    plot_left: usize,
    plot_top: usize,
    plot_bottom: usize,
    graph_width: usize,
    style: Style,
) {
    let mut previous = None;

    for &(x, y) in positions {
        let Some(y) = y else {
            previous = None;
            continue;
        };
        let current = (
            x.clamp(plot_left, graph_width.saturating_sub(1)),
            y.clamp(plot_top, plot_bottom),
        );

        let Some((x1, y1)) = previous else {
            grid[current.1][current.0] = ('─', style);
            previous = Some(current);
            continue;
        };
        let (x2, y2) = current;

        if x1 == x2 {
            let (top, bottom) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
            for row in top..=bottom {
                grid[row][x1] = ('│', style);
            }
            previous = Some(current);
            continue;
        }

        let middle = (x1 + x2) / 2;
        if y1 == y2 {
            for column in x1..=x2 {
                grid[y1][column] = ('─', style);
            }
            previous = Some(current);
            continue;
        }

        for column in x1..middle {
            grid[y1][column] = ('─', style);
        }
        grid[y1][middle] = (if y2 > y1 { '╮' } else { '╯' }, style);

        let (top, bottom) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        for row in top + 1..bottom {
            grid[row][middle] = ('│', style);
        }
        grid[y2][middle] = (if y2 > y1 { '╰' } else { '╭' }, style);
        for column in middle + 1..=x2 {
            grid[y2][column] = ('─', style);
        }

        previous = Some(current);
    }
}

fn draw_agent_marker(
    grid: &mut [Vec<(char, Style)>],
    x: usize,
    y: usize,
    plot_left: usize,
    plot_top: usize,
    plot_bottom: usize,
    graph_width: usize,
    style: Style,
) {
    if x >= graph_width || y < plot_top || y > plot_bottom {
        return;
    }

    if graph_width.saturating_sub(plot_left) < 3 {
        grid[y][x] = ('◉', style);
        return;
    }

    let start = x
        .saturating_sub(1)
        .max(plot_left)
        .min(graph_width.saturating_sub(3));
    for (offset, character) in ['╾', '◉', '╼'].into_iter().enumerate() {
        grid[y][start + offset] = (character, style);
    }
}

fn draw_file_lifecycle_marker(
    grid: &mut [Vec<(char, Style)>],
    x: usize,
    y: usize,
    plot_left: usize,
    graph_width: usize,
    created: bool,
    deleted: bool,
    style: Style,
) {
    if x >= graph_width || y >= grid.len() || (!created && !deleted) {
        return;
    }
    if created && deleted && graph_width.saturating_sub(plot_left) >= 2 {
        let start = x
            .saturating_sub(1)
            .max(plot_left)
            .min(graph_width.saturating_sub(2));
        grid[y][start] = ('+', style);
        grid[y][start + 1] = ('x', style);
    } else {
        grid[y][x] = (if created { '+' } else { 'x' }, style);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalloutDirection {
    Left,
    Right,
}

impl CalloutDirection {
    fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

fn callout_side_for_navigation(navigation_direction: i8) -> CalloutDirection {
    if navigation_direction < 0 {
        CalloutDirection::Right
    } else {
        CalloutDirection::Left
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CalloutVariation {
    direction: CalloutDirection,
    horizontal_gap: usize,
    height_percent: usize,
}

fn callout_variation(turn_index: usize, event_kind: u64) -> CalloutVariation {
    let seed = (turn_index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ event_kind.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mut rng = StdRng::seed_from_u64(seed);
    CalloutVariation {
        direction: if rng.gen_bool(0.5) {
            CalloutDirection::Left
        } else {
            CalloutDirection::Right
        },
        horizontal_gap: rng.gen_range(2..=6),
        height_percent: rng.gen_range(15..=85),
    }
}

fn callout_variation_on_side(
    turn_index: usize,
    event_kind: u64,
    direction: CalloutDirection,
) -> CalloutVariation {
    let mut variation = callout_variation(turn_index, event_kind);
    variation.direction = direction;
    variation
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CalloutRect {
    x: i64,
    y: usize,
    width: usize,
    height: usize,
}

impl CalloutRect {
    fn overlaps(self, other: Self) -> bool {
        self.x < other.x + other.width as i64 + 1
            && self.x + self.width as i64 + 1 > other.x
            && self.y < other.y + other.height + 1
            && self.y + self.height + 1 > other.y
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CalloutLayout {
    target_x: i64,
    target_y: usize,
    bubble: CalloutRect,
    inner_width: usize,
    lines: Vec<String>,
    direction: CalloutDirection,
}

fn agent_started_message(count: usize) -> String {
    if count == 1 {
        "1 agent started".to_string()
    } else {
        format!("{count} agents started")
    }
}

fn format_percent_reduction(value: f64) -> String {
    if value > 0.0 && value < 10.0 {
        format!("{value:.1}%")
    } else {
        format!("{value:.0}%")
    }
}

fn compaction_message(before: f64, after: f64) -> String {
    let reduction = (before - after).max(0.0);
    if reduction > 0.0 {
        format!(
            "auto-compact reduced context by {}",
            format_percent_reduction(reduction)
        )
    } else {
        "auto-compact refreshed context".to_string()
    }
}

fn canonical_compaction_range(turn: &TurnUsage) -> Option<(f64, f64)> {
    turn.telemetry
        .context_compaction_ranges()
        .into_iter()
        .max_by(|(before_a, after_a), (before_b, after_b)| {
            let reduction_a = (before_a - after_a).max(0.0);
            let reduction_b = (before_b - after_b).max(0.0);
            reduction_a.total_cmp(&reduction_b)
        })
}

fn compaction_callout(turn: &TurnUsage) -> Option<(String, Option<f64>)> {
    if let Some((before, after)) = canonical_compaction_range(turn) {
        return Some((compaction_message(before, after), Some(after)));
    }
    (turn.telemetry.compactions > 0).then(|| {
        (
            "auto-compact completed".to_string(),
            turn.telemetry.context_percent(),
        )
    })
}

fn event_callout_messages(turn: &TurnUsage) -> Vec<String> {
    let mut messages = Vec::new();
    if !turn.agents.is_empty() {
        messages.push(agent_started_message(turn.agents.len()));
    }

    if let Some((message, _)) = compaction_callout(turn) {
        messages.push(message);
    }
    messages
}

fn nonzero_metric_median(turns: &[TurnUsage], metric: u8) -> Option<f64> {
    let mut values: Vec<f64> = turns
        .iter()
        .filter_map(|turn| metric_value(turn, metric))
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Some(values[middle])
    }
}

fn spike_message(
    turns: &[TurnUsage],
    selected: usize,
    metric: u8,
    label: &str,
    minimum_delta: f64,
) -> Option<String> {
    let value = metric_value(turns.get(selected)?, metric)?;
    let median = nonzero_metric_median(turns, metric)?;
    let ratio = value / median;
    (ratio >= SPIKE_MEDIAN_MULTIPLIER && value - median >= minimum_delta)
        .then(|| format!("{label} was {ratio:.1}x session median"))
}

fn median_spike_message(turns: &[TurnUsage], selected: usize, metric: u8) -> Option<String> {
    let turn = turns.get(selected)?;
    match metric {
        0 => spike_message(turns, selected, metric, "cost", 0.05),
        1 if !matches!(
            turn.telemetry.outcome,
            crate::model::TurnOutcome::Failed | crate::model::TurnOutcome::Aborted
        ) =>
        {
            spike_message(turns, selected, metric, "duration", 15_000.0)
        }
        2 => spike_message(turns, selected, metric, "token use", 5_000.0),
        _ => None,
    }
}

fn code_change_message(turn: &TurnUsage) -> Option<String> {
    let created = turn.files_created();
    let deleted = turn.files_deleted();
    let mut changes = Vec::new();
    if created > 0 {
        changes.push(format!(
            "+ {created} file{} created",
            if created == 1 { "" } else { "s" }
        ));
    }
    if deleted > 0 {
        changes.push(format!(
            "x {deleted} file{} deleted",
            if deleted == 1 { "" } else { "s" }
        ));
    }
    if changes.is_empty() && turn.diff_lines() == 0 {
        return None;
    }
    changes.push(format!(
        "code changed +{} -{}",
        turn.lines_added(),
        turn.lines_removed()
    ));
    Some(changes.join(", "))
}

fn metric_callout(
    turns: &[TurnUsage],
    selected: usize,
    metric: u8,
) -> Option<(String, Option<f64>)> {
    let turn = turns.get(selected)?;
    match metric as usize {
        0 => median_spike_message(turns, selected, metric)
            .map(|message| (message, metric_value(turn, metric))),
        1 => match turn.telemetry.outcome {
            crate::model::TurnOutcome::Failed => {
                Some(("turn failed".to_string(), metric_value(turn, metric)))
            }
            crate::model::TurnOutcome::Aborted => {
                Some(("turn was aborted".to_string(), metric_value(turn, metric)))
            }
            _ => median_spike_message(turns, selected, metric)
                .map(|message| (message, metric_value(turn, metric))),
        },
        2 => median_spike_message(turns, selected, metric)
            .map(|message| (message, metric_value(turn, metric))),
        CONTEXT_METRIC_INDEX => compaction_callout(turn).or_else(|| {
            metric_value(turn, metric)
                .filter(|value| *value >= 80.0)
                .map(|value| (format!("context reached {value:.0}%"), Some(value)))
        }),
        COMPLEXITY_METRIC_INDEX if !turn.agents.is_empty() => Some((
            agent_started_message(turn.agents.len()),
            metric_value(turn, metric),
        )),
        5 => code_change_message(turn).map(|message| (message, metric_value(turn, metric))),
        _ => None,
    }
}

fn put_grid_cell(
    grid: &mut [Vec<(char, Style)>],
    row: usize,
    column: i64,
    plot_left: usize,
    graph_width: usize,
    character: char,
    style: Style,
) {
    if column < plot_left as i64 || column >= graph_width as i64 {
        return;
    }
    if let Some(cell) = grid
        .get_mut(row)
        .and_then(|grid_row| grid_row.get_mut(column as usize))
    {
        *cell = (character, style);
    }
}

fn put_grid_text(
    grid: &mut [Vec<(char, Style)>],
    row: usize,
    column: i64,
    text: &str,
    plot_left: usize,
    graph_width: usize,
    style: Style,
) {
    for (offset, character) in text.chars().enumerate() {
        put_grid_cell(
            grid,
            row,
            column + offset as i64,
            plot_left,
            graph_width,
            character,
            style,
        );
    }
}

fn draw_horizontal_segment(
    grid: &mut [Vec<(char, Style)>],
    row: usize,
    start: i64,
    end: i64,
    clip: (usize, usize),
    character: char,
    style: Style,
) {
    let (plot_left, graph_width) = clip;
    if start > end || graph_width <= plot_left {
        return;
    }
    let visible_start = start.max(plot_left as i64);
    let visible_end = end.min(graph_width as i64 - 1);
    for column in visible_start..=visible_end {
        put_grid_cell(grid, row, column, plot_left, graph_width, character, style);
    }
}

fn callout_lane_positions(
    minimum: usize,
    maximum: usize,
    bubble_height: usize,
    percent: usize,
) -> Vec<usize> {
    if minimum > maximum {
        return Vec::new();
    }
    let preferred = minimum + (maximum - minimum) * percent.min(100) / 100;
    let lane_step = bubble_height + 1;
    let mut positions = Vec::new();
    let mut position = minimum;
    while position <= maximum {
        positions.push(position);
        let Some(next) = position.checked_add(lane_step) else {
            break;
        };
        position = next;
    }
    positions.sort_by_key(|position| position.abs_diff(preferred));
    positions
}

fn layout_timeline_callout(
    target: (i64, usize),
    messages: &[String],
    variation: CalloutVariation,
    plot_bounds: (usize, usize),
    annotation_bounds: (usize, usize),
    timeline_bounds: (i64, i64),
    occupied: &mut Vec<CalloutRect>,
) -> Option<CalloutLayout> {
    let (plot_left, graph_width) = plot_bounds;
    let (annotation_top, annotation_bottom) = annotation_bounds;
    let available_width = graph_width.saturating_sub(plot_left);
    let available_height = annotation_bottom.saturating_sub(annotation_top) + 1;
    if messages.is_empty() || available_width < 14 || available_height < 3 {
        return None;
    }

    let max_inner_width = available_width.saturating_sub(4).min(20);
    let mut lines = Vec::new();
    for message in messages {
        lines.extend(word_wrap(message, max_inner_width));
    }
    if lines.is_empty() {
        return None;
    }

    let inner_width = lines
        .iter()
        .map(|line| line.width())
        .max()
        .unwrap_or(0)
        .max(10)
        .min(max_inner_width);
    let bubble_width = inner_width + 4;
    let bubble_height = lines.len() + 2;
    if bubble_height > available_height {
        return None;
    }

    let (target_x, target_y) = target;
    let (timeline_left, timeline_right) = timeline_bounds;
    let preferred_direction = if target_x + variation.horizontal_gap as i64 + bubble_width as i64
        > timeline_right
    {
        CalloutDirection::Left
    } else if target_x - variation.horizontal_gap as i64 - (bubble_width as i64) < timeline_left {
        CalloutDirection::Right
    } else {
        variation.direction
    };
    let horizontal_preferences = [preferred_direction, preferred_direction.opposite()];
    let mut placement = None;
    let minimum_top = annotation_top;
    let maximum_top = annotation_bottom + 1 - bubble_height;
    let vertical_positions = callout_lane_positions(
        minimum_top,
        maximum_top,
        bubble_height,
        variation.height_percent,
    );

    let maximum_shift =
        (timeline_right.saturating_sub(timeline_left).max(0) as usize).min(bubble_width);
    'placement: for horizontal_shift in 0..=maximum_shift {
        for actual_direction in horizontal_preferences {
            let gap = variation.horizontal_gap + horizontal_shift;
            let bubble_start = match actual_direction {
                CalloutDirection::Left => target_x - bubble_width as i64 - gap as i64,
                CalloutDirection::Right => target_x + gap as i64,
            };
            let bubble_end = bubble_start + bubble_width as i64 - 1;
            if bubble_start < timeline_left || bubble_end > timeline_right {
                continue;
            }
            for bubble_top in vertical_positions.iter().copied() {
                let bounds = CalloutRect {
                    x: bubble_start,
                    y: bubble_top,
                    width: bubble_width,
                    height: bubble_height,
                };
                if occupied.iter().all(|other| !bounds.overlaps(*other)) {
                    placement = Some((bubble_start, bubble_top, actual_direction));
                    break 'placement;
                }
            }
        }
    }

    let (bubble_start, bubble_top, actual_direction) = placement?;
    let bubble = CalloutRect {
        x: bubble_start,
        y: bubble_top,
        width: bubble_width,
        height: bubble_height,
    };
    occupied.push(bubble);
    Some(CalloutLayout {
        target_x,
        target_y,
        bubble,
        inner_width,
        lines,
        direction: actual_direction,
    })
}

fn draw_timeline_callout(
    grid: &mut [Vec<(char, Style)>],
    layout: &CalloutLayout,
    viewport_offset: i64,
    plot_left: usize,
    graph_width: usize,
    border_style: Style,
    text_style: Style,
    marker_style: Style,
) {
    let bubble_x = layout.bubble.x - viewport_offset;
    let target_x = layout.target_x - viewport_offset;
    let viewport_left = plot_left as i64;
    let viewport_right = graph_width.saturating_sub(1) as i64;
    let visibility_halo = layout.bubble.width as i64;
    if target_x < viewport_left - visibility_halo || target_x > viewport_right + visibility_halo {
        return;
    }
    let bubble_right = bubble_x + layout.bubble.width as i64 - 1;
    let bubble_bottom = layout.bubble.y + layout.bubble.height - 1;
    let anchor_x = match layout.direction {
        CalloutDirection::Left => bubble_right - 2,
        CalloutDirection::Right => bubble_x + 2,
    };

    let elbow_row = (layout.target_y + 1).min(layout.bubble.y.saturating_sub(1));
    if anchor_x < target_x {
        put_grid_cell(
            grid,
            elbow_row,
            anchor_x,
            plot_left,
            graph_width,
            '╭',
            border_style,
        );
        draw_horizontal_segment(
            grid,
            elbow_row,
            anchor_x + 1,
            target_x - 1,
            (plot_left, graph_width),
            '─',
            border_style,
        );
        put_grid_cell(
            grid,
            elbow_row,
            target_x,
            plot_left,
            graph_width,
            '╯',
            border_style,
        );
    } else {
        put_grid_cell(
            grid,
            elbow_row,
            anchor_x,
            plot_left,
            graph_width,
            '╮',
            border_style,
        );
        draw_horizontal_segment(
            grid,
            elbow_row,
            target_x + 1,
            anchor_x - 1,
            (plot_left, graph_width),
            '─',
            border_style,
        );
        put_grid_cell(
            grid,
            elbow_row,
            target_x,
            plot_left,
            graph_width,
            '╰',
            border_style,
        );
    }
    if layout.bubble.y > elbow_row + 1 {
        for row in elbow_row + 1..layout.bubble.y {
            put_grid_cell(
                grid,
                row,
                anchor_x,
                plot_left,
                graph_width,
                '│',
                border_style,
            );
        }
    }

    for row in layout.bubble.y..=bubble_bottom {
        draw_horizontal_segment(
            grid,
            row,
            bubble_x,
            bubble_right,
            (plot_left, graph_width),
            ' ',
            text_style,
        );
    }
    put_grid_cell(
        grid,
        layout.bubble.y,
        bubble_x,
        plot_left,
        graph_width,
        '╭',
        border_style,
    );
    draw_horizontal_segment(
        grid,
        layout.bubble.y,
        bubble_x + 1,
        bubble_right - 1,
        (plot_left, graph_width),
        '─',
        border_style,
    );
    put_grid_cell(
        grid,
        layout.bubble.y,
        bubble_right,
        plot_left,
        graph_width,
        '╮',
        border_style,
    );
    put_grid_cell(
        grid,
        layout.bubble.y,
        anchor_x,
        plot_left,
        graph_width,
        '▼',
        border_style,
    );

    for (offset, line) in layout.lines.iter().enumerate() {
        let padding = layout.inner_width.saturating_sub(line.width());
        put_grid_text(
            grid,
            layout.bubble.y + 1 + offset,
            bubble_x,
            &format!("│ {line}{} │", " ".repeat(padding)),
            plot_left,
            graph_width,
            text_style,
        );
        put_grid_cell(
            grid,
            layout.bubble.y + 1 + offset,
            bubble_x,
            plot_left,
            graph_width,
            '│',
            border_style,
        );
        put_grid_cell(
            grid,
            layout.bubble.y + 1 + offset,
            bubble_right,
            plot_left,
            graph_width,
            '│',
            border_style,
        );
        for (column, marker) in lifecycle_marker_columns(line) {
            put_grid_cell(
                grid,
                layout.bubble.y + 1 + offset,
                bubble_x + 2 + column as i64,
                plot_left,
                graph_width,
                marker,
                marker_style,
            );
        }
    }

    put_grid_cell(
        grid,
        bubble_bottom,
        bubble_x,
        plot_left,
        graph_width,
        '╰',
        border_style,
    );
    draw_horizontal_segment(
        grid,
        bubble_bottom,
        bubble_x + 1,
        bubble_right - 1,
        (plot_left, graph_width),
        '─',
        border_style,
    );
    put_grid_cell(
        grid,
        bubble_bottom,
        bubble_right,
        plot_left,
        graph_width,
        '╯',
        border_style,
    );
}

fn lifecycle_marker_columns(line: &str) -> Vec<(usize, char)> {
    let characters: Vec<char> = line.chars().collect();
    let mut display_column = 0;
    let mut markers = Vec::new();
    for (index, character) in characters.iter().copied().enumerate() {
        let previous_is_boundary =
            index == 0 || characters[index - 1].is_whitespace() || characters[index - 1] == ',';
        let next_is_space = characters
            .get(index + 1)
            .is_some_and(|next| next.is_whitespace());
        if matches!(character, '+' | 'x') && previous_is_boundary && next_is_space {
            markers.push((display_column, character));
        }
        display_column += character.width().unwrap_or(0);
    }
    markers
}

fn selected_metric_value(turn: &TurnUsage, metric: u8) -> String {
    match metric as usize {
        0 if turn.cost_known || turn.cost > 0.0 => format!("est. {}", format_cost(turn.cost)),
        0 => "estimate unavailable".to_string(),
        1 => turn
            .telemetry
            .duration_ms
            .map(format_duration)
            .unwrap_or_else(|| {
                if turn.telemetry.outcome == crate::model::TurnOutcome::InProgress {
                    "pending".to_string()
                } else {
                    "not emitted".to_string()
                }
            }),
        2 => format!(
            "{} tok",
            format_tokens(turn.input_tokens + turn.output_tokens)
        ),
        3 => turn
            .telemetry
            .context_percent()
            .map(|value| format!("{value:.0}%"))
            .unwrap_or_else(|| "not emitted".to_string()),
        4 => turn
            .telemetry
            .complexity_percent()
            .map(format_complexity_percent)
            .unwrap_or_else(|| "not emitted".to_string()),
        5 => format!("+{} -{}", turn.lines_added(), turn.lines_removed()),
        _ => "not emitted".to_string(),
    }
}

fn render_metric_panel(
    buffer: &mut Buffer,
    turns: &[TurnUsage],
    selected: usize,
    metric: u8,
    provider: Option<ProviderKind>,
    callout_side: CalloutDirection,
    visible_range: (usize, usize),
    area: Rect,
) {
    let palette = theme::provider_palette(provider);
    let metric_idx = metric as usize % METRIC_NAMES.len();
    let metric_name = METRIC_NAMES[metric_idx];
    let color = metric_color(metric_idx, provider);
    let selected_value = selected_metric_value(&turns[selected], metric);
    let value_style = if metric_idx == 0 {
        Style::default().fg(palette.highlight)
    } else {
        Style::default().fg(palette.text)
    };
    let title_spans = vec![
        Span::styled(
            format!(" {} ", metric_name),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("── ", Style::default().fg(palette.dim)),
        Span::styled(selected_value, value_style.add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.dim))
        .title(Line::from(title_spans));

    let inner = block.inner(area);
    block.render(area, buffer);

    if inner.width < 16 || inner.height < 4 {
        return;
    }

    let uses_percent_scale = matches!(metric_idx, CONTEXT_METRIC_INDEX | COMPLEXITY_METRIC_INDEX);
    let metric_values = metric_series(turns, metric);
    let metric_available = metric_values.iter().any(Option::is_some);
    let raw_max_val = if uses_percent_scale && metric_available {
        100.0
    } else {
        metric_values
            .iter()
            .flatten()
            .copied()
            .fold(0.0_f64, f64::max)
    };
    let flat_graph = raw_max_val == 0.0;
    let max_val = if flat_graph { 1.0 } else { raw_max_val };

    let graph_width = inner.width as usize;
    let graph_height = inner.height as usize;
    let axis_values = if flat_graph {
        vec![0.0]
    } else {
        vec![max_val, max_val / 2.0, 0.0]
    };
    let axis_labels: Vec<String> = if !metric_available {
        vec![if metric_idx == 0 {
            "unavailable".to_string()
        } else {
            "not emitted".to_string()
        }]
    } else {
        axis_values
            .iter()
            .map(|value| format_axis_value(metric_idx, *value))
            .collect()
    };
    let axis_label_width = axis_labels
        .iter()
        .map(|label| label.width())
        .max()
        .unwrap_or(1)
        .min(graph_width.saturating_sub(8));
    let axis_x = axis_label_width + 1;
    let plot_left = axis_x + 1;
    let plot_width = graph_width.saturating_sub(plot_left);
    let x_axis_row = graph_height - 2;
    let label_row = graph_height - 1;
    let plot_top = 1usize;
    let annotation_rows = if graph_height >= 14 {
        (graph_height / 3).clamp(5, 6)
    } else {
        0
    };
    let plot_bottom = x_axis_row.saturating_sub(annotation_rows + 1).max(plot_top);
    let annotation_top = plot_bottom + 1;
    let annotation_bottom = x_axis_row.saturating_sub(1);
    let plot_height = plot_bottom.saturating_sub(plot_top) as f64;

    let (start_idx, end_idx) = visible_range;

    let visible_turns = &turns[start_idx..end_idx];
    let visible_metric_values = &metric_values[start_idx..end_idx];
    let num_visible = visible_turns.len();

    let max_label_len = format!("{}", end_idx).len();
    let inset = max_label_len / 2 + 1;
    let spacing = if num_visible > 1 {
        (plot_width.saturating_sub(inset * 2)) as f64 / (num_visible - 1) as f64
    } else {
        0.0
    };
    let timeline_x = |turn_index: usize| {
        if num_visible > 1 {
            plot_left as i64 + inset as i64 + (turn_index as f64 * spacing).round() as i64
        } else {
            (plot_left + plot_width / 2) as i64
        }
    };
    let viewport_offset = (start_idx as f64 * spacing).round() as i64;
    let viewport_bounds = (
        viewport_offset + plot_left as i64,
        viewport_offset + graph_width.saturating_sub(1) as i64,
    );

    let mut grid: Vec<Vec<(char, Style)>> =
        vec![vec![(' ', Style::default()); graph_width]; graph_height];
    let mut dot_positions: Vec<(usize, Option<usize>)> = Vec::new();

    let axis_style = Style::default().fg(palette.dim);
    for row in 0..x_axis_row {
        grid[row][axis_x] = ('│', axis_style);
    }
    grid[x_axis_row][axis_x] = ('└', axis_style);
    for x in plot_left..graph_width {
        grid[x_axis_row][x] = ('─', axis_style);
    }
    let mut guide_rows = Vec::with_capacity(axis_labels.len());
    for (index, label) in axis_labels.iter().enumerate() {
        let row = if flat_graph {
            plot_top + (plot_bottom - plot_top) / 2
        } else {
            match index {
                0 => plot_top,
                1 => plot_top + (plot_bottom - plot_top) / 2,
                _ => plot_bottom,
            }
        };
        if !guide_rows.contains(&row) {
            guide_rows.push(row);
        }
        for x in plot_left..graph_width {
            grid[row][x] = ('┄', axis_style);
        }
        let label_start = axis_x.saturating_sub(label.width() + 1);
        for (offset, ch) in label.chars().enumerate() {
            if label_start + offset < axis_x {
                grid[row][label_start + offset] = (ch, axis_style);
            }
        }
    }
    let horizontal_label = "turn";
    let horizontal_label_start = axis_x.saturating_sub(horizontal_label.len());
    for (offset, ch) in horizontal_label.chars().enumerate() {
        grid[label_row][horizontal_label_start + offset] = (ch, axis_style);
    }

    let value_to_y = |value: f64| {
        if flat_graph {
            plot_top + (plot_bottom - plot_top) / 2
        } else {
            let y_frac = (value / max_val).clamp(0.0, 1.0);
            plot_top + (plot_height * (1.0 - y_frac)).round() as usize
        }
        .min(plot_bottom)
    };
    for (i, val) in visible_metric_values.iter().enumerate() {
        let x = if num_visible > 1 {
            (timeline_x(start_idx + i) - viewport_offset)
                .clamp(plot_left as i64, graph_width.saturating_sub(1) as i64) as usize
        } else {
            plot_left + plot_width / 2
        };
        dot_positions.push((x.min(graph_width - 1), val.map(value_to_y)));
    }

    let mut callout_layouts = Vec::new();
    if annotation_rows > 0 {
        let mut occupied_callouts = Vec::new();
        if let Some((message, target_value)) = metric_callout(turns, selected, metric) {
            let target_y = target_value
                .map(value_to_y)
                .unwrap_or_else(|| plot_top + (plot_bottom - plot_top) / 2);
            if let Some(layout) = layout_timeline_callout(
                (timeline_x(selected), target_y),
                &[message],
                callout_variation_on_side(selected, 0x100 + metric as u64, callout_side),
                (plot_left, graph_width),
                (annotation_top, annotation_bottom),
                viewport_bounds,
                &mut occupied_callouts,
            ) {
                callout_layouts.push(layout);
            }
        }
    }

    let line_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    draw_continuous_line(
        &mut grid,
        &dot_positions,
        plot_left,
        plot_top,
        plot_bottom,
        graph_width,
        line_style,
    );

    let selected_position = selected
        .checked_sub(start_idx)
        .and_then(|index| dot_positions.get(index))
        .copied();
    if let Some((x, _)) = selected_position {
        let cursor_style = Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD);
        for row in plot_top..=plot_bottom {
            grid[row][x] = ('┊', cursor_style);
        }
        grid[plot_top][x] = (
            '▼',
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        );
    }

    if metric_idx == CONTEXT_METRIC_INDEX {
        let compact_style = Style::default()
            .fg(palette.highlight)
            .add_modifier(Modifier::BOLD);
        for (index, turn) in visible_turns.iter().enumerate() {
            let Some(&(x, _)) = dot_positions.get(index) else {
                continue;
            };
            if let Some((before, after)) = canonical_compaction_range(turn) {
                let before_y = value_to_y(before);
                let after_y = value_to_y(after);
                let (top, bottom) = if before_y <= after_y {
                    (before_y, after_y)
                } else {
                    (after_y, before_y)
                };
                for row in top..=bottom {
                    grid[row][x] = ('┃', compact_style);
                }
                grid[before_y][x] = ('▲', compact_style);
                grid[after_y][x] = ('▼', compact_style);
            }
        }
    }

    let agent_style = Style::default()
        .fg(palette.highlight)
        .add_modifier(Modifier::BOLD);
    if metric_idx == COMPLEXITY_METRIC_INDEX {
        for (index, turn) in visible_turns.iter().enumerate() {
            if turn.agents.is_empty() {
                continue;
            }
            let Some(&(x, Some(y))) = dot_positions.get(index) else {
                continue;
            };
            draw_agent_marker(
                &mut grid,
                x,
                y,
                plot_left,
                plot_top,
                plot_bottom,
                graph_width,
                agent_style,
            );
        }
    }

    let event_marker_style = Style::default()
        .fg(palette.highlight)
        .add_modifier(Modifier::BOLD);
    if metric_idx == 5 {
        for (index, turn) in visible_turns.iter().enumerate() {
            let created = turn.files_created() > 0;
            let deleted = turn.files_deleted() > 0;
            if !created && !deleted {
                continue;
            }
            let Some(&(x, Some(y))) = dot_positions.get(index) else {
                continue;
            };
            draw_file_lifecycle_marker(
                &mut grid,
                x,
                y,
                plot_left,
                graph_width,
                created,
                deleted,
                event_marker_style,
            );
        }
    }
    if metric <= 2 {
        for (index, &(x, y)) in dot_positions.iter().enumerate() {
            if median_spike_message(turns, start_idx + index, metric).is_none() {
                continue;
            }
            let Some(y) = y else {
                continue;
            };
            grid[y][x] = ('●', event_marker_style);
        }
    }

    let selected_has_agents = metric_idx == COMPLEXITY_METRIC_INDEX
        && selected
            .checked_sub(start_idx)
            .and_then(|index| visible_turns.get(index))
            .is_some_and(|turn| !turn.agents.is_empty());
    let selected_has_file_lifecycle = metric_idx == 5
        && selected
            .checked_sub(start_idx)
            .and_then(|index| visible_turns.get(index))
            .is_some_and(|turn| turn.files_created() > 0 || turn.files_deleted() > 0);
    let selected_has_median_spike = median_spike_message(turns, selected, metric).is_some();

    // The curve carries history; only an unbranched active turn needs a point marker.
    if !selected_has_agents && !selected_has_file_lifecycle && !selected_has_median_spike {
        if let Some((x, Some(y))) = selected_position {
            if x < graph_width && y < graph_height {
                grid[y][x] = (
                    '◆',
                    Style::default()
                        .fg(palette.text)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
    }

    if let Some((x, _)) = selected_position {
        grid[x_axis_row][x] = (
            '▲',
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        );
    }

    // Horizontal legend: turn-number labels share the dashboard window.
    {
        let mut occupied: Vec<bool> = vec![false; graph_width + 4];
        let place_label = |grid: &mut Vec<Vec<(char, Style)>>,
                           occupied: &mut Vec<bool>,
                           x: usize,
                           label: &str,
                           style: Style|
         -> bool {
            let centered = x.saturating_sub(label.len() / 2).max(plot_left);
            let label_x = if centered + label.len() > graph_width {
                graph_width.saturating_sub(label.len())
            } else {
                centered
            };
            let start = label_x.saturating_sub(1);
            let end = (label_x + label.len() + 1).min(occupied.len());
            if occupied[start..end].iter().any(|&o| o) {
                return false;
            }
            for (j, ch) in label.chars().enumerate() {
                if label_x + j < graph_width {
                    grid[label_row][label_x + j] = (ch, style);
                }
            }
            for p in label_x..label_x + label.len() {
                if p < occupied.len() {
                    occupied[p] = true;
                }
            }
            true
        };
        if let Some((x, _)) = selected_position {
            place_label(
                &mut grid,
                &mut occupied,
                x,
                &format!("T{}", selected + 1),
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD),
            );
        }
        for (i, &(x, _)) in dot_positions.iter().enumerate() {
            let actual_idx = start_idx + i;
            if actual_idx == selected {
                continue;
            }
            let show = num_visible <= 20 || i % (num_visible / 10).max(1) == 0;
            if show {
                place_label(
                    &mut grid,
                    &mut occupied,
                    x,
                    &format!("{}", actual_idx + 1),
                    Style::default().fg(palette.dim),
                );
            }
        }
    }

    // Explanatory overlays are last so the curve and cursor cannot erase their connector.
    let callout_border_style = Style::default()
        .fg(palette.primary)
        .bg(palette.surface)
        .add_modifier(Modifier::BOLD);
    let callout_text_style = Style::default().fg(palette.text).bg(palette.surface);
    let callout_marker_style = Style::default()
        .fg(palette.highlight)
        .bg(palette.surface)
        .add_modifier(Modifier::BOLD);
    for layout in &callout_layouts {
        draw_timeline_callout(
            &mut grid,
            layout,
            viewport_offset,
            plot_left,
            graph_width,
            callout_border_style,
            callout_text_style,
            callout_marker_style,
        );
    }

    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|(ch, style)| Span::styled(ch.to_string(), style))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    Paragraph::new(Text::from(lines)).render(inner, buffer);
}

struct AttributionDocument {
    title: Line<'static>,
    lines: Vec<Line<'static>>,
    border_style: Style,
}

impl AttributionDocument {
    fn height(&self) -> u16 {
        (self.lines.len() as u16).saturating_add(2)
    }
}

fn build_attribution_document(
    turn: &TurnUsage,
    app: &App,
    inner_width: u16,
) -> AttributionDocument {
    let provider_kind = app
        .engine
        .live_engine()
        .and_then(|engine| engine.sessions.get(engine.active_idx))
        .map(|session| session.provider);
    let palette = theme::provider_palette(provider_kind);
    let root = &turn.attribution.aggregate;

    let title = Line::from(vec![
        Span::styled(
            " input work ",
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("── ", Style::default().fg(palette.dim)),
        Span::styled(
            if root.estimated {
                format!("~{}", format_tokens(root.tokens))
            } else {
                format_tokens(root.tokens)
            },
            Style::default()
                .fg(palette.highlight)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let mut lines = vec![Line::from("")];
    let root_cost = (turn.cost_known || turn.cost > 0.0).then_some(turn.cost);
    let mut visual_root =
        build_attribution_visual_tree_with_economics(root, root_cost, turn.telemetry.duration_ms);
    visual_root.label = attribution_turn_identifier(&turn.prompt);
    let canvas = build_attribution_infographic(&mut visual_root, inner_width as usize, palette);
    lines.extend(canvas.lines);

    AttributionDocument {
        title,
        lines,
        border_style: Style::default().fg(palette.dim),
    }
}

const ATTRIBUTION_NODE_HEIGHT: usize = 5;
const ATTRIBUTION_NODE_GAP: usize = 1;

#[derive(Clone, Debug)]
struct AttributionVisualNode {
    label: String,
    tokens: u64,
    percent_of_root: f64,
    estimated: bool,
    cost: Option<f64>,
    duration_ms: Option<u64>,
    cost_estimated: bool,
    duration_estimated: bool,
    depth: usize,
    y: usize,
    children: Vec<AttributionVisualNode>,
}

struct AttributionCanvas {
    lines: Vec<Line<'static>>,
}

fn build_attribution_visual_tree(root: &AttributionNode) -> AttributionVisualNode {
    build_attribution_visual_tree_with_economics(root, None, None)
}

fn build_attribution_visual_tree_with_economics(
    root: &AttributionNode,
    cost: Option<f64>,
    duration_ms: Option<u64>,
) -> AttributionVisualNode {
    let children = root
        .children
        .iter()
        .map(|category| build_attribution_category_node(category, root.tokens, cost, duration_ms))
        .collect();

    AttributionVisualNode {
        label: root.label.clone(),
        tokens: root.tokens,
        percent_of_root: root.percent_of_root,
        estimated: root.estimated,
        cost,
        duration_ms,
        cost_estimated: cost.is_some(),
        duration_estimated: false,
        depth: 0,
        y: 0,
        children,
    }
}

fn attribution_turn_identifier(prompt: &str) -> String {
    prompt_preview(prompt).0
}

fn prompt_preview(prompt: &str) -> (String, bool) {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return ("User request".to_string(), false);
    }

    let mut preview = normalized
        .chars()
        .take(PROMPT_PREVIEW_LEN)
        .collect::<String>();
    let truncated = normalized.chars().count() > PROMPT_PREVIEW_LEN;
    if truncated {
        preview = preview.trim_end().to_string();
        preview.push_str("...");
    }
    (preview, truncated)
}

fn build_attribution_category_node(
    category: &AttributionNode,
    root_tokens: u64,
    root_cost: Option<f64>,
    root_duration_ms: Option<u64>,
) -> AttributionVisualNode {
    let children = if category.category == Some(AttributionCategory::Compaction)
        || category.children.len() == 1
            && redundant_single_source(category.category, &category.children[0].label)
    {
        Vec::new()
    } else {
        category
            .children
            .iter()
            .map(|source| {
                build_attribution_source_node(
                    source,
                    category.category,
                    root_tokens,
                    root_cost,
                    root_duration_ms,
                )
            })
            .collect()
    };

    AttributionVisualNode {
        label: category.label.clone(),
        tokens: category.tokens,
        percent_of_root: category.percent_of_root,
        estimated: category.estimated,
        cost: allocated_attribution_cost(root_cost, category.tokens, root_tokens),
        duration_ms: allocated_attribution_duration(root_duration_ms, category.tokens, root_tokens),
        cost_estimated: root_cost.is_some(),
        duration_estimated: root_duration_ms.is_some(),
        depth: 1,
        y: 0,
        children,
    }
}

fn redundant_single_source(category: Option<AttributionCategory>, source: &str) -> bool {
    matches!(
        (category, source),
        (Some(AttributionCategory::UserPrompt), "Current prompt")
            | (Some(AttributionCategory::Context), "Prior turns")
            | (
                Some(AttributionCategory::ProviderRuntime),
                "Provider-managed input"
            )
    )
}

fn build_attribution_source_node(
    source: &AttributionNode,
    category: Option<AttributionCategory>,
    root_tokens: u64,
    root_cost: Option<f64>,
    root_duration_ms: Option<u64>,
) -> AttributionVisualNode {
    let count = source_invocation_count(source);
    let source_label = if category == Some(AttributionCategory::Agents) {
        single_agent_purpose(source)
            .map(|purpose| format!("{}: {purpose}", source.label))
            .unwrap_or_else(|| source.label.clone())
    } else {
        source.label.clone()
    };
    let label = if count > 1 {
        format!("{} ({})", source_label, invocation_count_label(count))
    } else {
        source_label
    };
    AttributionVisualNode {
        label,
        tokens: source.tokens,
        percent_of_root: source.percent_of_root,
        estimated: source.estimated,
        cost: allocated_attribution_cost(root_cost, source.tokens, root_tokens),
        duration_ms: allocated_attribution_duration(root_duration_ms, source.tokens, root_tokens),
        cost_estimated: root_cost.is_some(),
        duration_estimated: root_duration_ms.is_some(),
        depth: 2,
        y: 0,
        children: Vec::new(),
    }
}

fn allocated_attribution_cost(cost: Option<f64>, tokens: u64, root_tokens: u64) -> Option<f64> {
    cost.map(|cost| {
        if root_tokens == 0 {
            0.0
        } else {
            cost * tokens as f64 / root_tokens as f64
        }
    })
}

fn allocated_attribution_duration(
    duration_ms: Option<u64>,
    tokens: u64,
    root_tokens: u64,
) -> Option<u64> {
    duration_ms.map(|duration_ms| {
        if root_tokens == 0 {
            return 0;
        }
        let scaled = duration_ms as u128 * tokens as u128;
        ((scaled + root_tokens as u128 / 2) / root_tokens as u128) as u64
    })
}

fn single_agent_purpose(source: &AttributionNode) -> Option<String> {
    let mut calls = source
        .children
        .iter()
        .filter(|invocation| !invocation.label.ends_with(" result"));
    let purpose = calls.next()?.label.as_str();
    if calls.next().is_some() {
        return None;
    }
    Some(
        purpose
            .rsplit_once(" #")
            .filter(|(_, sequence)| sequence.chars().all(|character| character.is_ascii_digit()))
            .map(|(label, _)| label)
            .unwrap_or(purpose)
            .to_string(),
    )
}

fn invocation_count_label(count: usize) -> String {
    format!("{count} times")
}

fn source_invocation_count(source: &AttributionNode) -> usize {
    let calls = source
        .children
        .iter()
        .filter(|invocation| !invocation.label.ends_with(" result"))
        .count();
    if calls == 0 {
        source.children.len()
    } else {
        calls
    }
}

fn build_attribution_infographic(
    root: &mut AttributionVisualNode,
    width: usize,
    palette: theme::ProviderPalette,
) -> AttributionCanvas {
    if width == 0 {
        return AttributionCanvas { lines: Vec::new() };
    }

    let columns = attribution_columns(width);
    let mut next_leaf_y = 2;
    assign_attribution_y(root, &columns, &mut next_leaf_y);
    let height = attribution_tree_bottom(root, &columns)
        .saturating_add(1)
        .max(attribution_node_height(root, &columns) + 2);
    let mut grid = vec![vec![(' ', Style::default()); width]; height];
    let headings = ["total", "category", "source"];
    for (index, heading) in headings.into_iter().enumerate() {
        let (x, column_width) = columns[index];
        let heading = truncate_display_width(heading, column_width);
        let heading_x = x + column_width.saturating_sub(heading.width()) / 2;
        write_attribution_text(
            &mut grid,
            heading_x,
            0,
            &heading,
            Style::default().fg(palette.dim),
            column_width,
        );
    }

    let mut connector_masks = vec![vec![0_u8; width]; height];
    let mut arrows = Vec::new();
    draw_attribution_connectors(root, &columns, &mut connector_masks, &mut arrows);
    let connector_style = Style::default()
        .fg(palette.primary)
        .add_modifier(Modifier::BOLD);
    for (y, row) in connector_masks.into_iter().enumerate() {
        for (x, mask) in row.into_iter().enumerate() {
            if mask != 0 {
                grid[y][x] = (attribution_connector_character(mask), connector_style);
            }
        }
    }
    for (x, y) in arrows {
        if y < grid.len() && x < grid[y].len() {
            grid[y][x] = ('▶', connector_style);
        }
    }

    draw_attribution_nodes(root, &columns, &mut grid, palette);

    let lines = grid
        .into_iter()
        .map(|row| {
            Line::from(
                row.into_iter()
                    .map(|(character, style)| Span::styled(character.to_string(), style))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();

    AttributionCanvas { lines }
}

fn attribution_columns(width: usize) -> [(usize, usize); 3] {
    let mut widths = if width >= 140 {
        [30, 30, 48]
    } else if width >= 100 {
        [26, 28, 40]
    } else if width >= 70 {
        [20, 20, 24]
    } else {
        let available = width.saturating_sub(2).max(3);
        let mut compact = [available / 3; 3];
        for slot in compact.iter_mut().take(available % 3) {
            *slot += 1;
        }
        compact
    };
    let node_width = widths.iter().sum::<usize>();
    if node_width > width {
        widths = [width / 3; 3];
        for slot in widths.iter_mut().take(width % 3) {
            *slot += 1;
        }
    }
    let node_width = widths.iter().sum::<usize>();
    let gap = (width.saturating_sub(node_width) / 2).clamp(1, 24);
    let content_width = node_width.saturating_add(gap * 2);
    let margin = width.saturating_sub(content_width) / 2;

    let mut columns = [(0, 0); 3];
    let mut x = margin;
    for (index, column_width) in widths.into_iter().enumerate() {
        columns[index] = (x.min(width.saturating_sub(1)), column_width.max(1));
        x = x.saturating_add(column_width).saturating_add(gap);
    }
    columns
}

fn attribution_node_height(node: &AttributionVisualNode, columns: &[(usize, usize); 3]) -> usize {
    if node.depth != 0 {
        return ATTRIBUTION_NODE_HEIGHT;
    }

    let width = columns[0].1;
    let inner_width = width.saturating_sub(2).max(1);
    word_wrap(&node.label, inner_width).len().max(1) + 4
}

fn attribution_node_center(node: &AttributionVisualNode, columns: &[(usize, usize); 3]) -> usize {
    node.y + attribution_node_height(node, columns) / 2
}

fn assign_attribution_y(
    node: &mut AttributionVisualNode,
    columns: &[(usize, usize); 3],
    next_leaf_y: &mut usize,
) {
    if node.children.is_empty() {
        node.y = *next_leaf_y;
        *next_leaf_y = next_leaf_y
            .saturating_add(attribution_node_height(node, columns) + ATTRIBUTION_NODE_GAP);
        return;
    }

    for child in &mut node.children {
        assign_attribution_y(child, columns, next_leaf_y);
    }
    let first_center = attribution_node_center(&node.children[0], columns);
    let last_center = attribution_node_center(&node.children[node.children.len() - 1], columns);
    node.y = ((first_center + last_center) / 2)
        .saturating_sub(attribution_node_height(node, columns) / 2);
}

fn attribution_tree_bottom(node: &AttributionVisualNode, columns: &[(usize, usize); 3]) -> usize {
    node.children
        .iter()
        .map(|child| attribution_tree_bottom(child, columns))
        .fold(node.y + attribution_node_height(node, columns), usize::max)
}

fn draw_attribution_connectors(
    node: &AttributionVisualNode,
    columns: &[(usize, usize); 3],
    masks: &mut [Vec<u8>],
    arrows: &mut Vec<(usize, usize)>,
) {
    if node.children.is_empty() || masks.is_empty() {
        return;
    }
    let (parent_x, parent_width) = columns[node.depth.min(2)];
    let from_x = parent_x.saturating_add(parent_width);
    let from_y = attribution_node_center(node, columns);
    let child_depth = node.children[0].depth.min(2);
    let (child_x, _) = columns[child_depth];
    draw_attribution_branch(
        masks,
        arrows,
        from_x,
        from_y,
        child_x,
        node.children
            .iter()
            .map(|child| attribution_node_center(child, columns)),
    );
    for child in &node.children {
        draw_attribution_connectors(child, columns, masks, arrows);
    }
}

fn draw_attribution_branch(
    masks: &mut [Vec<u8>],
    arrows: &mut Vec<(usize, usize)>,
    from_x: usize,
    from_y: usize,
    child_x: usize,
    child_rows: impl Iterator<Item = usize>,
) {
    if masks.is_empty() || from_y >= masks.len() || child_x == 0 {
        return;
    }
    let width = masks[0].len();
    let arrow_x = child_x.saturating_sub(1);
    if from_x >= width || arrow_x >= width {
        return;
    }
    let child_rows = child_rows
        .filter(|row| *row < masks.len())
        .collect::<Vec<_>>();
    if child_rows.is_empty() {
        return;
    }
    arrows.extend(child_rows.iter().map(|row| (arrow_x, *row)));
    if from_x >= arrow_x {
        return;
    }

    let end_x = arrow_x.saturating_sub(1).max(from_x);
    let trunk_x = from_x + end_x.saturating_sub(from_x) / 2;
    let top = *child_rows.iter().min().unwrap_or(&from_y);
    let bottom = *child_rows.iter().max().unwrap_or(&from_y);
    add_attribution_horizontal(masks, from_y, from_x, trunk_x);
    add_attribution_vertical(masks, trunk_x, top.min(from_y), bottom.max(from_y));
    for child_y in child_rows {
        add_attribution_horizontal(masks, child_y, trunk_x, end_x);
    }
}

fn add_attribution_horizontal(masks: &mut [Vec<u8>], y: usize, from: usize, to: usize) {
    let (left, right) = if from <= to { (from, to) } else { (to, from) };
    for x in left..right {
        masks[y][x] |= 0b0010;
        masks[y][x + 1] |= 0b0001;
    }
}

fn add_attribution_vertical(masks: &mut [Vec<u8>], x: usize, from: usize, to: usize) {
    let (top, bottom) = if from <= to { (from, to) } else { (to, from) };
    for y in top..bottom {
        masks[y][x] |= 0b1000;
        masks[y + 1][x] |= 0b0100;
    }
}

fn attribution_connector_character(mask: u8) -> char {
    match mask {
        0b0011 => '─',
        0b1100 => '│',
        0b1010 => '╭',
        0b1001 => '╮',
        0b0110 => '╰',
        0b0101 => '╯',
        0b1110 => '├',
        0b1101 => '┤',
        0b1011 => '┬',
        0b0111 => '┴',
        0b1111 => '┼',
        0b0001 | 0b0010 => '─',
        0b0100 | 0b1000 => '│',
        _ => '┼',
    }
}

fn draw_attribution_nodes(
    node: &AttributionVisualNode,
    columns: &[(usize, usize); 3],
    grid: &mut [Vec<(char, Style)>],
    palette: theme::ProviderPalette,
) {
    let (x, width) = columns[node.depth.min(2)];
    draw_attribution_node(grid, x, node.y, width, node, palette);
    for child in &node.children {
        draw_attribution_nodes(child, columns, grid, palette);
    }
}

fn draw_attribution_node(
    grid: &mut [Vec<(char, Style)>],
    x: usize,
    y: usize,
    width: usize,
    node: &AttributionVisualNode,
    palette: theme::ProviderPalette,
) {
    let columns = [(x, width), (x, width), (x, width)];
    let node_height = attribution_node_height(node, &columns);
    if width < 3 || y + node_height > grid.len() || x + width > grid[0].len() {
        return;
    }
    let is_root = node.depth == 0;
    let border_style = if is_root {
        Style::default()
            .fg(palette.highlight)
            .add_modifier(Modifier::BOLD)
    } else if node.tokens == 0 {
        Style::default().fg(palette.dim)
    } else {
        Style::default()
            .fg(palette.primary)
            .add_modifier(Modifier::BOLD)
    };
    let text_style = if node.tokens == 0 {
        Style::default().fg(palette.dim)
    } else {
        Style::default().fg(palette.text)
    };
    let label_style = if is_root {
        text_style.add_modifier(Modifier::ITALIC)
    } else {
        text_style
    };
    let right = x + width - 1;
    let bottom = y + node_height - 1;
    grid[y][x] = ('╭', border_style);
    grid[y][right] = ('╮', border_style);
    grid[bottom][x] = ('╰', border_style);
    grid[bottom][right] = ('╯', border_style);
    for cell in &mut grid[y][x + 1..right] {
        *cell = ('─', border_style);
    }
    for cell in &mut grid[bottom][x + 1..right] {
        *cell = ('─', border_style);
    }
    for row in grid.iter_mut().take(bottom).skip(y + 1) {
        row[x] = ('│', border_style);
        row[right] = ('│', border_style);
    }

    let inner_width = width.saturating_sub(2);
    let label_lines = if is_root {
        word_wrap(&node.label, inner_width)
    } else {
        vec![truncate_display_width(&node.label, inner_width)]
    };
    for (line_index, label) in label_lines
        .iter()
        .take(node_height.saturating_sub(4))
        .enumerate()
    {
        write_attribution_text(
            grid,
            x + 1,
            y + 1 + line_index,
            label,
            label_style,
            inner_width,
        );
    }

    let metric = attribution_node_metric(node, inner_width);
    write_attribution_text(grid, x + 1, bottom - 2, &metric, text_style, inner_width);
    let economics = attribution_node_economics(node, inner_width);
    write_attribution_text(grid, x + 1, bottom - 1, &economics, text_style, inner_width);
}

fn attribution_node_metric(node: &AttributionVisualNode, inner_width: usize) -> String {
    let token_label = if node.estimated {
        format!("~{}", format_tokens(node.tokens))
    } else {
        format_tokens(node.tokens)
    };
    let percent_label = format_attribution_percent(node.percent_of_root);
    let metric = if inner_width >= 20 {
        format!(
            "{} {} {}",
            attribution_bar(node.percent_of_root, 5),
            token_label,
            percent_label
        )
    } else if inner_width >= 13 {
        format!("{} {}", token_label, percent_label)
    } else {
        token_label
    };
    truncate_display_width(&metric, inner_width)
}

fn attribution_node_economics(node: &AttributionVisualNode, inner_width: usize) -> String {
    let cost = node.cost.map(|cost| {
        let value = if cost == 0.0 {
            "$0".to_string()
        } else {
            format_cost(cost)
        };
        if node.cost_estimated {
            if node.depth == 0 {
                format!("est. {value}")
            } else {
                format!("~{value}")
            }
        } else {
            value
        }
    });
    let duration = node.duration_ms.map(|duration_ms| {
        let value = format_duration(duration_ms);
        if node.duration_estimated {
            format!("~{value}")
        } else {
            value
        }
    });
    let economics = match (cost, duration) {
        (Some(cost), Some(duration)) => format!("{cost}  {duration}"),
        (Some(cost), None) => format!("{cost}  time —"),
        (None, Some(duration)) => format!("cost —  {duration}"),
        (None, None) => "cost —  time —".to_string(),
    };
    truncate_display_width(&economics, inner_width)
}

fn write_attribution_text(
    grid: &mut [Vec<(char, Style)>],
    x: usize,
    y: usize,
    text: &str,
    style: Style,
    max_width: usize,
) {
    if y >= grid.len() || x >= grid[y].len() {
        return;
    }
    let mut column = x;
    let end = x.saturating_add(max_width).min(grid[y].len());
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(1).max(1);
        if column + character_width > end {
            break;
        }
        grid[y][column] = (character, style);
        for continuation in 1..character_width {
            grid[y][column + continuation] = (' ', style);
        }
        column += character_width;
    }
}

fn attribution_bar(percent: f64, width: usize) -> String {
    let filled = ((percent.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn format_attribution_percent(percent: f64) -> String {
    if percent > 0.0 && percent < 0.005 {
        "<0.01%".to_string()
    } else {
        format!("{percent:.2}%")
    }
}

fn truncate_display_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let byte_index = byte_index_for_display_width(text, width - 1);
    format!("{}…", &text[..byte_index])
}

fn render_attribution_document(buffer: &mut Buffer, document: AttributionDocument, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(document.border_style)
        .title(document.title);
    let inner = block.inner(area);
    block.render(area, buffer);
    Paragraph::new(Text::from(document.lines)).render(inner, buffer);
}

struct TelemetryDocument {
    title: Line<'static>,
    lines: Vec<Line<'static>>,
    border_style: Style,
}

impl TelemetryDocument {
    fn height(&self) -> u16 {
        (self.lines.len() as u16).saturating_add(2)
    }
}

fn build_telemetry_document(
    turn: &TurnUsage,
    selected: usize,
    app: &App,
    _inner_width: u16,
) -> TelemetryDocument {
    let provider_kind = app
        .engine
        .live_engine()
        .and_then(|engine| engine.sessions.get(engine.active_idx))
        .map(|session| session.provider);
    let palette = theme::provider_palette(provider_kind);
    let model = turn
        .telemetry
        .model
        .clone()
        .unwrap_or_else(|| "not emitted".to_string());
    let pricing_date = turn
        .timestamp
        .get(..10)
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Utc::now().date_naive());
    let pricing = provider_kind
        .and_then(|provider| pricing_source_at(provider, &model, pricing_date))
        .map(|(label, _)| label);
    let (_, catalog_date) = pricing_catalog_metadata();
    let outcome_style = match turn.telemetry.outcome {
        crate::model::TurnOutcome::Completed => Style::default().fg(palette.primary),
        crate::model::TurnOutcome::InProgress => Style::default().fg(palette.highlight),
        crate::model::TurnOutcome::Aborted | crate::model::TurnOutcome::Failed => {
            Style::default().fg(palette.danger)
        }
    };
    let context_label = turn
        .telemetry
        .context_window
        .map(|window| {
            let used = turn.telemetry.latest_input_tokens.min(window);
            format!(
                "{} / {} ({:.0}%)",
                format_tokens(used),
                format_tokens(window),
                turn.telemetry.context_percent().unwrap_or(0.0)
            )
        })
        .unwrap_or_else(|| "not emitted".to_string());

    let title = Line::from(vec![
        Span::styled(
            " native telemetry ",
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("── ", Style::default().fg(palette.dim)),
        Span::styled(
            format!("Turn {}", selected + 1),
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let lines = vec![
        Line::from(vec![
            Span::styled("  model    ", Style::default().fg(palette.dim)),
            Span::styled(model.clone(), Style::default().fg(palette.text)),
            if let Some(label) = pricing {
                Span::styled(
                    format!("  priced as {label} ({catalog_date})"),
                    Style::default().fg(palette.subtle),
                )
            } else if model == "not emitted" {
                Span::styled("", Style::default().fg(palette.subtle))
            } else {
                Span::styled("  unpriced", Style::default().fg(palette.subtle))
            },
        ]),
        Line::from(vec![
            Span::styled("  outcome  ", Style::default().fg(palette.dim)),
            Span::styled(turn.telemetry.outcome.label(), outcome_style),
            Span::styled("  duration  ", Style::default().fg(palette.dim)),
            Span::styled(
                turn.telemetry
                    .duration_ms
                    .map(format_duration)
                    .unwrap_or_else(|| {
                        if turn.telemetry.outcome == crate::model::TurnOutcome::InProgress {
                            "pending".to_string()
                        } else {
                            "not emitted".to_string()
                        }
                    }),
                Style::default().fg(palette.subtle),
            ),
        ]),
        Line::from(vec![
            Span::styled("  context  ", Style::default().fg(palette.dim)),
            Span::styled(context_label, Style::default().fg(palette.subtle)),
            Span::styled("  cache  ", Style::default().fg(palette.dim)),
            Span::styled(
                format!("{:.0}%", cache_ratio(turn) * 100.0),
                Style::default().fg(palette.subtle),
            ),
            Span::styled("  complexity  ", Style::default().fg(palette.dim)),
            Span::styled(
                turn.telemetry
                    .complexity_percent()
                    .map(|value| {
                        let basis = turn
                            .telemetry
                            .complexity_basis()
                            .unwrap_or("provider effort");
                        format!("{} ({basis})", format_complexity_percent(value))
                    })
                    .unwrap_or_else(|| "not emitted".to_string()),
                Style::default().fg(palette.subtle),
            ),
        ]),
        Line::from(vec![
            Span::styled("  actions  ", Style::default().fg(palette.dim)),
            Span::styled(
                format!(
                    "{} tools  {} patches  {} searches  {} compactions  code +{} -{}",
                    turn.telemetry.tool_calls,
                    turn.telemetry.patches,
                    turn.telemetry.web_searches,
                    turn.telemetry.compactions,
                    turn.lines_added(),
                    turn.lines_removed(),
                ),
                Style::default().fg(palette.subtle),
            ),
        ]),
    ];

    TelemetryDocument {
        title,
        lines,
        border_style: Style::default().fg(palette.dim),
    }
}

fn render_telemetry_document(buffer: &mut Buffer, document: TelemetryDocument, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(document.border_style)
        .title(document.title);
    let inner = block.inner(area);
    block.render(area, buffer);
    Paragraph::new(Text::from(document.lines)).render(inner, buffer);
}

struct DetailDocument<'a> {
    title: Line<'a>,
    lines: Vec<Line<'a>>,
    border_style: Style,
}

impl DetailDocument<'_> {
    fn height(&self) -> u16 {
        (self.lines.len() as u16).saturating_add(2)
    }
}

fn detail_panel_height(document: &DetailDocument<'_>, inner_width: u16, expanded: bool) -> u16 {
    if expanded {
        document.height().max(3)
    } else {
        collapsed_detail_height(inner_width).max(3)
    }
}

fn collapsed_detail_height(inner_width: u16) -> u16 {
    let wrap_width = inner_width.saturating_sub(6).max(1) as usize;
    let prompt_lines = (PROMPT_PREVIEW_LEN + 3).div_ceil(wrap_width);
    let response_lines = (AGENT_RESPONSE_PREVIEW_LEN + 3).div_ceil(wrap_width);
    u16::try_from(
        prompt_lines
            .saturating_add(response_lines)
            .saturating_add(12),
    )
    .unwrap_or(u16::MAX)
}

fn build_detail_document<'a>(
    turns: &'a [TurnUsage],
    selected: usize,
    app: &'a App,
    inner_width: u16,
) -> DetailDocument<'a> {
    let turn = &turns[selected];
    let provider_kind = app
        .engine
        .live_engine()
        .and_then(|engine| engine.sessions.get(engine.active_idx))
        .map(|session| session.provider);
    let palette = theme::provider_palette(provider_kind);

    let mut title_spans = vec![
        Span::styled(
            format!(" Turn {} ", selected + 1),
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("── ", Style::default().fg(palette.dim)),
        Span::styled(
            cost_label(turn),
            Style::default()
                .fg(palette.highlight)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(ref buf) = app.graph_jump_input {
        title_spans.push(Span::styled(
            " ── go to: ",
            Style::default().fg(palette.dim),
        ));
        title_spans.push(Span::styled(
            format!("{buf}▏"),
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let wrap_width = inner_width.saturating_sub(6) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let section_sep = "─".repeat(wrap_width.min(50));

    // ── PROMPT ──
    let show_full_prompt = app.expanded_view.is_some();
    let (prompt_preview, prompt_truncated) = prompt_preview(&turn.prompt);
    lines.push(Line::from(vec![
        Span::styled("  ▸ ", Style::default().fg(palette.primary)),
        Span::styled(
            "PROMPT",
            Style::default()
                .fg(palette.primary)
                .add_modifier(Modifier::BOLD),
        ),
        if !show_full_prompt && prompt_truncated {
            Span::styled("  (e to expand)", Style::default().fg(palette.dim))
        } else if show_full_prompt {
            Span::styled("  (e to collapse)", Style::default().fg(palette.dim))
        } else {
            Span::raw("")
        },
    ]));
    let prompt_text = if show_full_prompt {
        turn.prompt.clone()
    } else {
        prompt_preview
    };
    for chunk in word_wrap(&prompt_text, wrap_width) {
        lines.push(Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(chunk, Style::default().fg(palette.text)),
        ]));
    }
    lines.push(Line::from(""));

    // ── RESPONSE (right below prompt) ──
    if !turn.response_text.is_empty() {
        let show_full = app.expanded_view.is_some();
        lines.push(Line::from(vec![
            Span::styled("  ◂ ", Style::default().fg(palette.accent)),
            Span::styled(
                "RESPONSE",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            if !show_full && turn.response_text.len() > AGENT_RESPONSE_PREVIEW_LEN {
                Span::styled("  (e to expand)", Style::default().fg(palette.dim))
            } else if show_full {
                Span::styled("  (e to collapse)", Style::default().fg(palette.dim))
            } else {
                Span::raw("")
            },
        ]));
        let text = if show_full {
            turn.response_text.clone()
        } else {
            let p: String = turn
                .response_text
                .chars()
                .take(AGENT_RESPONSE_PREVIEW_LEN)
                .collect();
            if p.len() < turn.response_text.len() {
                format!("{}...", p)
            } else {
                p
            }
        };
        for chunk in word_wrap(&text, wrap_width) {
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(chunk, Style::default().fg(palette.subtle)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // ── STATS ──
    lines.push(Line::from(vec![
        Span::styled("  est cost  ", Style::default().fg(palette.dim)),
        Span::styled(cost_label(turn), Style::default().fg(palette.highlight)),
        Span::styled("     context  ", Style::default().fg(palette.dim)),
        Span::styled(
            format!("↑{} cumulative", format_tokens(turn.cumulative_context)),
            Style::default().fg(palette.primary),
        ),
        if turn.context_saved > 0 {
            Span::styled(
                format!(
                    "  (saved {} tokens via sub-agents)",
                    format_tokens(turn.context_saved)
                ),
                Style::default().fg(palette.subtle),
            )
        } else {
            Span::raw("")
        },
    ]));
    lines.push(Line::from(vec![
        Span::styled("  tokens  ", Style::default().fg(palette.dim)),
        Span::styled(
            format!("↑{}", format_tokens(turn.input_tokens)),
            Style::default().fg(palette.primary),
        ),
        Span::styled(" in  ", Style::default().fg(palette.dim)),
        Span::styled(
            format!("↓{}", format_tokens(turn.output_tokens)),
            Style::default().fg(palette.text),
        ),
        Span::styled(" out  ", Style::default().fg(palette.dim)),
        Span::styled(
            format!(
                "cache read: {}  cache write: {}",
                format_tokens(turn.cache_read_tokens),
                format_tokens(turn.cache_write_tokens)
            ),
            Style::default().fg(palette.subtle),
        ),
    ]));
    lines.push(Line::from(""));

    // ── AGENTS ──
    if !turn.agents.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {section_sep}"),
            Style::default().fg(palette.dim),
        )));
        lines.push(Line::from(vec![
            Span::styled(
                if turn.agents.len() == 1 {
                    "  ◆ 1 agent spawned".to_string()
                } else {
                    format!("  ◆ {} agents spawned", turn.agents.len())
                },
                Style::default()
                    .fg(palette.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if app.expanded_view.is_some() {
                    "  (e to collapse)"
                } else {
                    "  (e to expand)"
                },
                Style::default().fg(palette.dim),
            ),
        ]));
        lines.push(Line::from(""));

        if app.expanded_view.is_some() {
            for agent in &turn.agents {
                let outcome_style = match agent.outcome {
                    crate::model::TurnOutcome::Completed => Style::default().fg(palette.primary),
                    crate::model::TurnOutcome::InProgress => Style::default().fg(palette.highlight),
                    crate::model::TurnOutcome::Aborted | crate::model::TurnOutcome::Failed => {
                        Style::default().fg(palette.danger)
                    }
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        "  ◆ ",
                        Style::default()
                            .fg(palette.primary)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        &agent.name,
                        Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}  ", agent.role),
                        Style::default().fg(palette.subtle),
                    ),
                    Span::styled(agent.outcome.label(), outcome_style),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default().fg(palette.dim)),
                    Span::styled(
                        agent.model.as_deref().unwrap_or("model not emitted"),
                        Style::default().fg(palette.subtle),
                    ),
                    Span::styled("  ", Style::default().fg(palette.dim)),
                    Span::styled(
                        agent.duration_ms.map(format_duration).unwrap_or_else(|| {
                            if agent.outcome == crate::model::TurnOutcome::InProgress {
                                "pending".to_string()
                            } else {
                                "duration not emitted".to_string()
                            }
                        }),
                        Style::default().fg(palette.subtle),
                    ),
                    Span::styled(
                        format!(
                            "  {} tools  code +{} -{}",
                            agent.tool_calls, agent.lines_added, agent.lines_removed
                        ),
                        Style::default().fg(palette.subtle),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default().fg(palette.dim)),
                    Span::styled(
                        format!(
                            "{}  ↑{} ↓{}  cache {}",
                            if agent.cost_known || agent.cost > 0.0 {
                                format!("est. {}", format_cost(agent.cost))
                            } else {
                                "estimate unavailable".to_string()
                            },
                            format_tokens(agent.input_tokens),
                            format_tokens(agent.output_tokens),
                            format_tokens(agent.cache_read_tokens),
                        ),
                        Style::default().fg(palette.subtle),
                    ),
                ]));
                lines.push(Line::from(""));

                let show_full_agent = app.expanded_view.is_some();

                // Request — truncated unless expanded
                if !agent.prompt.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("    ▸ ", Style::default().fg(palette.primary)),
                        Span::styled(
                            "REQUEST",
                            Style::default()
                                .fg(palette.primary)
                                .add_modifier(Modifier::BOLD),
                        ),
                        if !show_full_agent && agent.prompt.len() > AGENT_PROMPT_PREVIEW_LEN {
                            Span::styled("  (e to expand)", Style::default().fg(palette.dim))
                        } else {
                            Span::raw("")
                        },
                    ]));
                    let text = if show_full_agent {
                        agent.prompt.clone()
                    } else {
                        let p: String = agent
                            .prompt
                            .chars()
                            .take(AGENT_PROMPT_PREVIEW_LEN)
                            .collect();
                        if p.len() < agent.prompt.len() {
                            format!("{}...", p)
                        } else {
                            p
                        }
                    };
                    for chunk in word_wrap(&text, wrap_width) {
                        lines.push(Line::from(vec![
                            Span::styled("      ", Style::default()),
                            Span::styled(chunk, Style::default().fg(palette.text)),
                        ]));
                    }
                    lines.push(Line::from(""));
                }

                // Response — truncated unless expanded
                if !agent.response_preview.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("    ◂ ", Style::default().fg(palette.accent)),
                        Span::styled(
                            "RESPONSE",
                            Style::default()
                                .fg(palette.accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                        if !show_full_agent
                            && agent.response_preview.len() > AGENT_RESPONSE_PREVIEW_LEN
                        {
                            Span::styled("  (e to expand)", Style::default().fg(palette.dim))
                        } else {
                            Span::raw("")
                        },
                    ]));
                    let text = if show_full_agent {
                        agent.response_preview.clone()
                    } else {
                        let p: String = agent
                            .response_preview
                            .chars()
                            .take(AGENT_RESPONSE_PREVIEW_LEN)
                            .collect();
                        if p.len() < agent.response_preview.len() {
                            format!("{}...", p)
                        } else {
                            p
                        }
                    };
                    for chunk in word_wrap(&text, wrap_width) {
                        lines.push(Line::from(vec![
                            Span::styled("      ", Style::default()),
                            Span::styled(chunk, Style::default().fg(palette.subtle)),
                        ]));
                    }
                    lines.push(Line::from(""));
                }

                lines.push(Line::from(Span::styled(
                    format!("    {}", "─".repeat(wrap_width.min(40))),
                    Style::default().fg(palette.dim),
                )));
                lines.push(Line::from(""));
            }
        }
    }

    DetailDocument {
        title: Line::from(title_spans),
        lines,
        border_style: Style::default().fg(palette.dim),
    }
}

fn render_detail_document(buffer: &mut Buffer, document: DetailDocument<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(document.border_style)
        .title(document.title);
    let inner = block.inner(area);
    block.render(area, buffer);
    Paragraph::new(Text::from(document.lines)).render(inner, buffer);
}

fn cost_label(turn: &TurnUsage) -> String {
    if turn.cost_known || turn.cost > 0.0 {
        format!("est. {}", format_cost(turn.cost))
    } else {
        "estimate unavailable".to_string()
    }
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    for line in text.lines() {
        if line.width() <= width {
            result.push(line.to_string());
        } else {
            let mut remaining = line;
            while remaining.width() > width {
                let limit = byte_index_for_display_width(remaining, width);
                if limit == 0 {
                    let first_char_end = remaining
                        .char_indices()
                        .nth(1)
                        .map(|(idx, _)| idx)
                        .unwrap_or(remaining.len());
                    let (chunk, rest) = remaining.split_at(first_char_end);
                    result.push(chunk.to_string());
                    remaining = rest.trim_start();
                    continue;
                }

                let break_at = if remaining.as_bytes().get(limit) == Some(&b' ') {
                    limit
                } else {
                    remaining[..limit]
                        .rfind(' ')
                        .filter(|idx| *idx > 0)
                        .unwrap_or(limit)
                };
                let (chunk, rest) = remaining.split_at(break_at);
                if !chunk.is_empty() {
                    result.push(chunk.trim_end().to_string());
                }
                remaining = rest.trim_start();
            }
            if !remaining.is_empty() {
                result.push(remaining.to_string());
            }
        }
    }
    result
}

fn byte_index_for_display_width(text: &str, max_width: usize) -> usize {
    let mut display_width = 0;
    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        let char_width = ch.width().unwrap_or(0);
        if display_width + char_width > max_width {
            return end;
        }
        display_width += char_width;
        end = idx + ch.len_utf8();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn turn_with_context(latest_input_tokens: u64, context_window: Option<u64>) -> TurnUsage {
        TurnUsage {
            prompt: String::new(),
            timestamp: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: 0.0,
            agents: Vec::new(),
            cumulative_context: 0,
            context_saved: 0,
            response_text: String::new(),
            cost_known: false,
            telemetry: crate::model::TurnTelemetry {
                latest_input_tokens,
                context_window,
                ..crate::model::TurnTelemetry::default()
            },
            attribution: crate::model::TurnAttribution::default(),
        }
    }

    fn attribution_text(document: &AttributionDocument) -> String {
        lines_text(&document.lines)
    }

    fn lines_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn native_telemetry_is_a_top_level_document_not_turn_detail() {
        let mut turn = turn_with_context(0, None);
        turn.telemetry.model = Some("gpt-test".to_string());
        turn.telemetry.duration_ms = Some(1_250);
        turn.telemetry.tool_calls = 3;
        let turns = vec![turn];
        let app = App::new_mock();

        let telemetry = build_telemetry_document(&turns[0], 0, &app, 118);
        let telemetry_text = lines_text(&telemetry.lines);
        let telemetry_title = lines_text(std::slice::from_ref(&telemetry.title));
        let detail = build_detail_document(&turns, 0, &app, 118);
        let detail_text = lines_text(&detail.lines);

        assert!(telemetry_title.contains("native telemetry"));
        assert!(telemetry_title.contains("Turn 1"));
        assert!(telemetry_text.contains("gpt-test"));
        assert!(telemetry_text.contains("1.2s"));
        assert!(telemetry_text.contains("3 tools"));
        assert!(!detail_text.contains("NATIVE TELEMETRY"));
    }

    #[test]
    fn collapsed_turn_detail_has_a_fixed_height_and_hides_agent_chat() {
        let plain_turns = vec![turn_with_context(0, None)];
        let mut agent_turn = turn_with_context(0, None);
        agent_turn.prompt = "x".repeat(PROMPT_PREVIEW_LEN + 20);
        agent_turn.response_text = "y".repeat(AGENT_RESPONSE_PREVIEW_LEN + 20);
        agent_turn.agents = (0..3).map(test_agent).collect();
        let agent_turns = vec![agent_turn];
        let app = App::new_mock();

        let plain = build_detail_document(&plain_turns, 0, &app, 118);
        let collapsed = build_detail_document(&agent_turns, 0, &app, 118);
        let collapsed_text = lines_text(&collapsed.lines);

        assert_eq!(
            detail_panel_height(&plain, 118, false),
            detail_panel_height(&collapsed, 118, false)
        );
        assert!(collapsed_text.contains("3 agents spawned"));
        assert!(collapsed_text.contains("e to expand"));
        assert!(!collapsed_text.contains("agent 0"));
        assert!(!collapsed_text.contains("REQUEST"));
    }

    #[test]
    fn expanding_turn_detail_reveals_agent_chat() {
        let mut turn = turn_with_context(0, None);
        let mut agent = test_agent(0);
        agent.prompt = "inspect the parser".to_string();
        agent.response_preview = "parser inspected".to_string();
        turn.agents = vec![agent];
        let turns = vec![turn];
        let mut app = App::new_mock();
        app.expanded_view = Some('e');

        let detail = build_detail_document(&turns, 0, &app, 118);
        let text = lines_text(&detail.lines);

        assert!(text.contains("1 agent spawned"));
        assert!(text.contains("e to collapse"));
        assert!(text.contains("agent 0"));
        assert!(text.contains("REQUEST"));
        assert!(detail_panel_height(&detail, 118, true) >= detail.height());
    }

    #[test]
    fn attribution_tree_shows_all_categories_and_exact_aggregate_root() {
        let mut turn = turn_with_context(0, None);
        turn.prompt = "inspect".to_string();
        turn.attribution = crate::model::TurnAttribution::new("inspect", 1_000);
        turn.attribution.observe(
            crate::model::AttributionCategory::ToolsAndMcps,
            "Read",
            "Read README.md #1",
            100,
        );
        turn.attribution.observe(
            crate::model::AttributionCategory::ToolsAndMcps,
            "Write",
            "Write report.md #2",
            80,
        );
        turn.attribution
            .record_parent_request("request-1", 2_000, 500, true);
        let app = App::new_mock();

        let document = build_attribution_document(&turn, &app, 118);
        let text = attribution_text(&document);

        for category in crate::model::AttributionCategory::ALL {
            assert!(
                text.contains(category.label()),
                "missing {}",
                category.label()
            );
        }
        assert!(!text.contains("all work"));
        assert!(text.contains("inspect"));
        assert!(text.contains("2k"));
        assert!(!text.contains("~2k"));
        assert!(text.contains('▶'));
        assert!(text.contains("Read"));
        assert!(!text.contains("Read (1 time)"));
        assert!(!text.contains("Read README.md #1"));
    }

    #[test]
    fn attribution_percentages_keep_two_decimals_for_small_contributions() {
        assert_eq!(format_attribution_percent(0.104), "0.10%");
        assert_eq!(format_attribution_percent(0.005), "0.01%");
        assert_eq!(format_attribution_percent(0.004), "<0.01%");
        assert_eq!(format_attribution_percent(0.0), "0.00%");
    }

    #[test]
    fn attribution_infographic_places_children_to_the_right_with_red_curves() {
        let mut turn = turn_with_context(0, None);
        turn.prompt = "inspect".to_string();
        turn.attribution = crate::model::TurnAttribution::new("inspect", 1_000);
        turn.attribution.observe(
            crate::model::AttributionCategory::ToolsAndMcps,
            "Read",
            "Read README.md #1",
            100,
        );
        turn.attribution.observe(
            crate::model::AttributionCategory::ToolsAndMcps,
            "Write",
            "Write report.md #2",
            80,
        );
        turn.attribution
            .record_parent_request("request-1", 2_000, 0, true);
        let app = App::new_mock();

        let document = build_attribution_document(&turn, &app, 118);
        let rows = document
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let root_x = rows
            .iter()
            .find_map(|row| row.find("inspect"))
            .expect("root box");
        let category_x = rows
            .iter()
            .find_map(|row| row.find("Tools & MCPs"))
            .expect("category box");
        let source_x = rows
            .iter()
            .find_map(|row| row.find("Read"))
            .expect("source box");
        assert!(root_x < category_x && category_x < source_x);

        let red = theme::provider_palette(None).primary;
        assert!(document
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| { span.content == "▶" && span.style.fg == Some(red) }));
        assert!(document
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| {
                matches!(span.content.as_ref(), "╭" | "╮" | "╰" | "╯") && span.style.fg == Some(red)
            }));
        assert!(document
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .all(|span| !matches!(span.content.as_ref(), "┌" | "┐" | "└" | "┘")));
    }

    #[test]
    fn attribution_root_identifies_the_normalized_user_request() {
        let mut turn = turn_with_context(0, None);
        turn.prompt = "  fix the first\n animation   timing  ".to_string();
        turn.attribution = crate::model::TurnAttribution::new(&turn.prompt, 0);
        turn.attribution
            .record_parent_request("request-1", 100, 0, true);
        let app = App::new_mock();

        let document = build_attribution_document(&turn, &app, 118);
        let text = attribution_text(&document);
        let italic_text = document
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .filter(|span| span.style.add_modifier.contains(Modifier::ITALIC))
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("fix the first animation"));
        assert!(italic_text.contains("fix the first animation"));
        assert!(italic_text.contains("timing"));
    }

    #[test]
    fn attribution_root_truncates_only_after_the_high_prompt_limit() {
        let exact = "x".repeat(PROMPT_PREVIEW_LEN);
        let over = format!("{exact}y");

        assert_eq!(attribution_turn_identifier(&exact), exact);
        let truncated = attribution_turn_identifier(&over);
        assert_eq!(truncated.chars().count(), PROMPT_PREVIEW_LEN + 3);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn turn_detail_and_attribution_root_share_the_same_prompt_preview() {
        let prompt = format!("  {}\n tail  ", "word ".repeat(80));
        let (preview, truncated) = prompt_preview(&prompt);

        assert!(truncated);
        assert_eq!(attribution_turn_identifier(&prompt), preview);
        assert!(preview.chars().count() <= PROMPT_PREVIEW_LEN + 3);
        assert!(preview.chars().count() > PROMPT_PREVIEW_LEN);
        assert!(preview.ends_with("..."));
        assert!(!preview.contains('\n'));
    }

    #[test]
    fn attribution_root_branches_round_only_the_outer_edges() {
        let mut turn = turn_with_context(0, None);
        turn.attribution = crate::model::TurnAttribution::new("inspect", 1_000);
        turn.attribution
            .record_parent_request("request-1", 2_000, 0, true);
        let mut visual = build_attribution_visual_tree(&turn.attribution.aggregate);
        let width = 160;
        let columns = attribution_columns(width);
        let mut next_leaf_y = 2;
        assign_attribution_y(&mut visual, &columns, &mut next_leaf_y);
        let height = attribution_tree_bottom(&visual, &columns) + 1;
        let mut masks = vec![vec![0_u8; width]; height];
        let mut arrows = Vec::new();
        draw_attribution_connectors(&visual, &columns, &mut masks, &mut arrows);

        let root_end = columns[0].0 + columns[0].1;
        let category_start = columns[1].0;
        let vertical_lanes = (root_end..category_start)
            .filter(|column| masks.iter().any(|row| row[*column] & 0b1100 != 0))
            .collect::<Vec<_>>();
        assert_eq!(vertical_lanes.len(), 1);
        let rounded_elbows = masks
            .iter()
            .flat_map(|row| &row[root_end..category_start])
            .filter(|mask| matches!(**mask, 0b1010 | 0b1001 | 0b0110 | 0b0101))
            .count();
        assert_eq!(rounded_elbows, 2);
        let square_junctions = masks
            .iter()
            .flat_map(|row| &row[root_end..category_start])
            .filter(|mask| **mask == 0b1111)
            .count();
        assert_eq!(square_junctions, 1);
    }

    #[test]
    fn attribution_root_uses_a_rounded_rectangle() {
        let style = Style::default();
        let mut grid = vec![vec![(' ', style); 30]; 8];
        let root = AttributionVisualNode {
            label: "inspect the renderer".to_string(),
            tokens: 1_000,
            percent_of_root: 100.0,
            estimated: false,
            cost: Some(0.25),
            duration_ms: Some(1_250),
            cost_estimated: true,
            duration_estimated: false,
            depth: 0,
            y: 0,
            children: Vec::new(),
        };
        let palette = theme::provider_palette(None);

        draw_attribution_node(&mut grid, 0, 0, 30, &root, palette);

        let rows = grid
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(character, _)| *character)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rows[0].starts_with("╭─"));
        assert!(rows[0].ends_with("─╮"));
        assert!(rows[1].starts_with('│'));
        assert!(rows[1].ends_with('│'));
        assert!(rows[4].starts_with("╰─"));
        assert!(rows[4].ends_with("─╯"));
    }

    #[test]
    fn attribution_nodes_allocate_cost_and_duration_by_input_share() {
        let mut turn = turn_with_context(0, None);
        turn.attribution = crate::model::TurnAttribution::new("inspect", 0);
        turn.attribution.observe(
            crate::model::AttributionCategory::ToolsAndMcps,
            "Terminal",
            "cargo test #1",
            250,
        );
        turn.attribution
            .record_parent_request("request-1", 1_000, 0, true);
        turn.cost = 2.0;
        turn.cost_known = true;
        turn.telemetry.duration_ms = Some(20_000);

        let visual = build_attribution_visual_tree_with_economics(
            &turn.attribution.aggregate,
            Some(turn.cost),
            turn.telemetry.duration_ms,
        );
        let tools = visual
            .children
            .iter()
            .find(|node| node.label == "Tools & MCPs")
            .expect("tools category");

        assert_eq!(visual.cost, Some(2.0));
        assert_eq!(visual.duration_ms, Some(20_000));
        assert!(!visual.duration_estimated);
        assert_eq!(tools.cost, Some(0.5));
        assert_eq!(tools.duration_ms, Some(5_000));
        assert!(tools.cost_estimated);
        assert!(tools.duration_estimated);
        assert_eq!(tools.children[0].cost, Some(0.5));
        assert_eq!(tools.children[0].duration_ms, Some(5_000));

        let app = App::new_mock();
        let text = attribution_text(&build_attribution_document(&turn, &app, 118));
        assert!(text.contains("est. $2.00  20.0s"));
        assert!(text.contains("~$0.50  ~5.0s"));
    }

    #[test]
    fn attribution_tree_renders_every_source_with_counts_and_hides_invocations() {
        let mut turn = turn_with_context(0, None);
        turn.attribution = crate::model::TurnAttribution::new("inspect", 0);
        for index in 0..7 {
            turn.attribution.observe(
                crate::model::AttributionCategory::ToolsAndMcps,
                format!("Tool {index}"),
                format!("Call {index}"),
                10 + index,
            );
        }
        turn.attribution
            .record_parent_request("request-1", 500, 0, true);
        let app = App::new_mock();
        let document = build_attribution_document(&turn, &app, 100);
        let text = attribution_text(&document);

        assert!(!text.contains("Other ("));
        for index in 0..7 {
            assert!(text.contains(&format!("Tool {index}")));
            assert!(!text.contains(&format!("Tool {index} (1 time)")));
            assert!(!text.contains(&format!("Call {index}")));
        }
    }

    #[test]
    fn attribution_source_count_does_not_count_paired_results_twice() {
        let mut turn = turn_with_context(0, None);
        turn.attribution = crate::model::TurnAttribution::new("inspect", 0);
        for invocation in ["cargo test #1", "cargo test result", "rg #2", "rg result"] {
            turn.attribution.observe(
                crate::model::AttributionCategory::ToolsAndMcps,
                "Terminal",
                invocation,
                10,
            );
        }
        turn.attribution
            .record_parent_request("request-1", 500, 0, true);
        let app = App::new_mock();

        let text = attribution_text(&build_attribution_document(&turn, &app, 100));
        assert!(text.contains("Terminal (2 times)"));
        assert!(!text.contains("cargo test #1"));
    }

    #[test]
    fn attribution_category_hides_only_a_redundant_single_source() {
        let mut turn = turn_with_context(0, None);
        turn.attribution = crate::model::TurnAttribution::new("inspect", 0);
        turn.attribution
            .record_parent_request("request-1", 500, 0, true);

        let visual = build_attribution_visual_tree(&turn.attribution.aggregate);
        let prompt = visual
            .children
            .iter()
            .find(|node| node.label == "User prompt")
            .expect("user prompt category");
        assert!(prompt.children.is_empty());
    }

    #[test]
    fn attribution_compaction_is_always_a_category_only_node() {
        let mut turn = turn_with_context(0, None);
        turn.attribution = crate::model::TurnAttribution::new("continue", 0);
        turn.attribution
            .replace_active_history_estimate(400, 400, "Automatic compaction");
        turn.attribution
            .record_parent_request("request-1", 500, 0, true);

        let visual = build_attribution_visual_tree(&turn.attribution.aggregate);
        let compaction = visual
            .children
            .iter()
            .find(|node| node.label == "Compaction")
            .expect("compaction category");
        assert!(compaction.tokens > 0);
        assert!(compaction.children.is_empty());
    }

    #[test]
    fn attribution_keeps_a_single_agent_name_and_purpose_without_redundant_count() {
        let mut turn = turn_with_context(0, None);
        turn.attribution = crate::model::TurnAttribution::new("inspect", 0);
        turn.attribution.observe(
            crate::model::AttributionCategory::Agents,
            "Godel",
            "Agent instruction",
            10,
        );
        turn.attribution
            .record_parent_request("request-1", 500, 0, true);

        let visual = build_attribution_visual_tree(&turn.attribution.aggregate);
        let agents = visual
            .children
            .iter()
            .find(|node| node.label == "Agents")
            .expect("agents category");
        assert_eq!(agents.children.len(), 1);
        assert_eq!(agents.children[0].label, "Godel: Agent instruction");
    }

    #[test]
    fn attribution_tree_stays_on_the_user_prompt_across_internal_requests() {
        let mut turn = turn_with_context(0, None);
        turn.prompt = "inspect the parser".to_string();
        turn.attribution = crate::model::TurnAttribution::new("inspect", 100);
        turn.attribution
            .record_parent_request("request-1", 200, 0, true);
        turn.attribution.set_next_request_label("After cargo test");
        turn.attribution
            .record_parent_request("request-2", 250, 0, true);
        let app = App::new_mock();

        let document = build_attribution_document(&turn, &app, 42);
        let text = attribution_text(&document);

        assert!(text.contains("inspect the"));
        assert!(!text.contains("After cargo test"));
        assert!(document.lines.iter().all(|line| line.width() <= 42));
    }

    fn test_agent(index: usize) -> crate::model::AgentCost {
        crate::model::AgentCost {
            id: format!("agent-{index}"),
            name: format!("agent {index}"),
            role: "worker".to_string(),
            model: None,
            cost: 0.0,
            cost_known: false,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            outcome: crate::model::TurnOutcome::Completed,
            duration_ms: None,
            tool_calls: 0,
            lines_added: 0,
            lines_removed: 0,
            files_created: 0,
            files_deleted: 0,
            prompt: String::new(),
            response_preview: String::new(),
        }
    }

    #[test]
    fn dashboard_has_six_metrics_in_a_three_by_two_grid() {
        assert_eq!(DASHBOARD_METRICS, [3, 1, 0, 2, 4, 5]);
        assert!(!METRIC_NAMES.contains(&"cache"));
        assert!(!METRIC_NAMES.contains(&"outcome"));

        let panels = dashboard_panel_areas(Rect::new(0, 0, 150, 24));
        assert_eq!(panels.len(), 6);
        assert_eq!(panels[0].y, panels[2].y);
        assert!(panels[3].y > panels[0].y);
        assert_eq!(panels[3].y, panels[5].y);
    }

    #[test]
    fn dashboard_reserves_twice_the_original_normal_height_for_annotations() {
        assert_eq!(dashboard_layout_heights(60, 2), (48, 10));
        assert_eq!(dashboard_layout_heights(24, 2), (20, 4));
    }

    #[test]
    fn provider_dashboards_use_one_aether_data_color() {
        for provider in [ProviderKind::Claude, ProviderKind::Codex] {
            let expected = theme::provider_palette(Some(provider)).primary;
            for metric in 0..METRIC_NAMES.len() {
                assert_eq!(metric_color(metric, Some(provider)), expected);
            }
        }
    }

    #[test]
    fn continuous_line_uses_joined_rounded_segments() {
        let mut grid = vec![vec![(' ', Style::default()); 22]; 8];
        let positions = [(3, Some(4)), (10, Some(1)), (18, Some(5))];
        draw_continuous_line(&mut grid, &positions, 2, 1, 6, 22, Style::default());

        let output: String = grid
            .iter()
            .flatten()
            .map(|(character, _)| character)
            .collect();
        assert!(output.contains('─'));
        assert!(output.contains('│'));
        assert!(output.contains('╯'));
        assert!(output.contains('╭'));
        assert!(!output
            .chars()
            .any(|glyph| ('\u{2800}'..='\u{28ff}').contains(&glyph)));
    }

    #[test]
    fn agent_marker_is_a_compact_robot_face() {
        let mut grid = vec![vec![(' ', Style::default()); 20]; 8];
        let style = Style::default().fg(theme::warm());
        draw_agent_marker(&mut grid, 10, 4, 2, 1, 6, 20, style);

        let row: String = grid[4].iter().map(|(character, _)| character).collect();
        assert!(row.contains("╾◉╼"));
        assert_eq!(grid[4][10].1.fg, Some(theme::warm()));
    }

    #[test]
    fn callout_variation_is_randomized_but_stable_and_bounded() {
        let first = callout_variation(12, 1);
        assert_eq!(first, callout_variation(12, 1));

        let variations: Vec<CalloutVariation> =
            (0..32).map(|turn| callout_variation(turn, 1)).collect();
        let directions: Vec<CalloutDirection> = variations
            .iter()
            .map(|variation| variation.direction)
            .collect();
        assert!(directions.contains(&CalloutDirection::Left));
        assert!(directions.contains(&CalloutDirection::Right));
        assert!(variations.iter().all(|variation| {
            (2..=6).contains(&variation.horizontal_gap)
                && (15..=85).contains(&variation.height_percent)
        }));
        assert!(variations
            .iter()
            .any(|variation| variation.horizontal_gap != variations[0].horizontal_gap));
        assert!(variations
            .iter()
            .any(|variation| variation.height_percent != variations[0].height_percent));
    }

    #[test]
    fn navigation_direction_places_callout_behind_travel() {
        assert_eq!(callout_side_for_navigation(1), CalloutDirection::Left);
        assert_eq!(callout_side_for_navigation(-1), CalloutDirection::Right);
    }

    #[test]
    fn event_callouts_use_static_templates_with_native_dynamic_values() {
        let mut turn = turn_with_context(0, None);
        turn.agents = (0..3).map(test_agent).collect();
        turn.telemetry.observe_context(80_000, Some(100_000));
        turn.telemetry.observe_context(0, Some(100_000));
        turn.telemetry.mark_context_compaction();
        turn.telemetry.observe_context(67_000, Some(100_000));
        turn.telemetry.reasoning_tokens = 3_200;
        turn.telemetry.reasoning_tokens_emitted = true;

        assert_eq!(
            event_callout_messages(&turn),
            vec![
                "3 agents started".to_string(),
                "auto-compact reduced context by 13%".to_string(),
            ]
        );
    }

    #[test]
    fn metric_callouts_surface_only_actionable_selected_turns() {
        let mut turns = vec![
            turn_with_context(20_000, Some(100_000)),
            turn_with_context(30_000, Some(100_000)),
            turn_with_context(40_000, Some(100_000)),
        ];
        for (index, turn) in turns.iter_mut().enumerate() {
            turn.cost_known = true;
            turn.cost = [0.01, 0.02, 0.10][index];
            turn.telemetry.duration_ms = Some([1_000, 2_000, 20_000][index]);
            turn.input_tokens = [100, 200, 8_000][index];
        }
        turns[2].telemetry.lines_added = 30;
        turns[2].telemetry.lines_removed = 8;
        turns[2].telemetry.files_created = 2;
        turns[2].telemetry.files_deleted = 1;

        assert_eq!(
            metric_callout(&turns, 2, 0).unwrap().0,
            "cost was 5.0x session median"
        );
        assert_eq!(
            metric_callout(&turns, 2, 1).unwrap().0,
            "duration was 10.0x session median"
        );
        assert_eq!(
            metric_callout(&turns, 2, 2).unwrap().0,
            "token use was 40.0x session median"
        );
        assert_eq!(
            metric_callout(&turns, 2, 5).unwrap().0,
            "+ 2 files created, x 1 file deleted, code changed +30 -8"
        );
        assert!(metric_callout(&turns, 1, 0).is_none());
        assert!(metric_callout(&turns, 1, 1).is_none());
        assert!(metric_callout(&turns, 1, 2).is_none());
        assert!(metric_callout(&turns, 1, 5).is_none());
    }

    #[test]
    fn file_lifecycle_callout_markers_use_aether_yellow() {
        let mut grid = vec![vec![(' ', Style::default()); 50]; 12];
        let layout = CalloutLayout {
            target_x: 35,
            target_y: 1,
            bubble: CalloutRect {
                x: 5,
                y: 5,
                width: 24,
                height: 4,
            },
            inner_width: 20,
            lines: vec![
                "+ 2 files created".to_string(),
                "x 1 file deleted".to_string(),
            ],
            direction: CalloutDirection::Left,
        };
        let marker_style = Style::default()
            .fg(theme::warm())
            .bg(theme::surface())
            .add_modifier(Modifier::BOLD);

        draw_timeline_callout(
            &mut grid,
            &layout,
            0,
            0,
            50,
            Style::default(),
            Style::default(),
            marker_style,
        );

        assert_eq!(grid[6][7], ('+', marker_style));
        assert_eq!(grid[7][7], ('x', marker_style));
    }

    #[test]
    fn code_diff_graph_marks_created_and_deleted_files() {
        let mut turns = vec![
            turn_with_context(0, None),
            turn_with_context(0, None),
            turn_with_context(0, None),
        ];
        turns[0].telemetry.lines_added = 2;
        turns[0].telemetry.files_created = 1;
        turns[1].telemetry.lines_removed = 3;
        turns[1].telemetry.files_deleted = 1;
        turns[2].telemetry.lines_added = 1;
        turns[2].telemetry.lines_removed = 1;
        turns[2].telemetry.files_created = 1;
        turns[2].telemetry.files_deleted = 1;

        let backend = TestBackend::new(100, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_metric_panel(
                    frame.buffer_mut(),
                    &turns,
                    2,
                    5,
                    Some(ProviderKind::Codex),
                    CalloutDirection::Left,
                    (0, turns.len()),
                    area,
                )
            })
            .unwrap();

        let yellow_symbols: Vec<&str> = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.fg == theme::warm())
            .map(|cell| cell.symbol())
            .collect();
        assert_eq!(
            yellow_symbols
                .iter()
                .filter(|symbol| **symbol == "+")
                .count(),
            2
        );
        assert_eq!(
            yellow_symbols
                .iter()
                .filter(|symbol| **symbol == "x")
                .count(),
            2
        );
    }

    #[test]
    fn median_spike_callouts_get_a_large_graph_point() {
        let mut turns = vec![
            turn_with_context(0, None),
            turn_with_context(0, None),
            turn_with_context(0, None),
        ];
        for (turn, cost) in turns.iter_mut().zip([0.01, 0.02, 0.10]) {
            turn.cost_known = true;
            turn.cost = cost;
        }

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_metric_panel(
                    frame.buffer_mut(),
                    &turns,
                    2,
                    0,
                    Some(ProviderKind::Codex),
                    CalloutDirection::Left,
                    (0, turns.len()),
                    area,
                )
            })
            .unwrap();

        let spike_points: Vec<_> = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.symbol() == "●")
            .collect();
        assert_eq!(spike_points.len(), 1);
        assert_eq!(spike_points[0].fg, theme::warm());
    }

    #[test]
    fn spike_markers_require_exceptional_cost_duration_and_token_values() {
        let mut cost_turns = vec![
            turn_with_context(0, None),
            turn_with_context(0, None),
            turn_with_context(0, None),
        ];
        for (turn, cost) in cost_turns.iter_mut().zip([0.01, 0.02, 0.05]) {
            turn.cost_known = true;
            turn.cost = cost;
        }
        assert!(median_spike_message(&cost_turns, 2, 0).is_none());
        cost_turns[2].cost = 0.10;
        assert!(median_spike_message(&cost_turns, 2, 0).is_some());

        let mut duration_turns = vec![
            turn_with_context(0, None),
            turn_with_context(0, None),
            turn_with_context(0, None),
        ];
        for (turn, duration) in duration_turns.iter_mut().zip([1_000, 2_000, 15_000]) {
            turn.telemetry.duration_ms = Some(duration);
        }
        assert!(median_spike_message(&duration_turns, 2, 1).is_none());
        duration_turns[2].telemetry.duration_ms = Some(20_000);
        assert!(median_spike_message(&duration_turns, 2, 1).is_some());

        let mut token_turns = vec![
            turn_with_context(0, None),
            turn_with_context(0, None),
            turn_with_context(0, None),
        ];
        for (turn, tokens) in token_turns.iter_mut().zip([1_000, 2_000, 6_000]) {
            turn.input_tokens = tokens;
        }
        assert!(median_spike_message(&token_turns, 2, 2).is_none());
        token_turns[2].input_tokens = 8_000;
        assert!(median_spike_message(&token_turns, 2, 2).is_some());
    }

    #[test]
    fn page_blit_scrolls_dashboard_and_detail_as_one_document() {
        let mut page = Buffer::empty(Rect::new(0, 0, 4, 4));
        for (row, text) in ["AAAA", "BBBB", "CCCC", "DDDD"].into_iter().enumerate() {
            page.set_string(0, row as u16, text, Style::default());
        }

        let backend = TestBackend::new(4, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                blit_page(frame, area, &page, 2);
            })
            .unwrap();
        terminal.backend().assert_buffer_lines(["CCCC", "DDDD"]);
    }

    #[test]
    fn rounded_callout_uses_red_box_and_scrolls_with_the_timeline() {
        let mut grid = vec![vec![(' ', Style::default()); 80]; 20];
        let mut occupied = Vec::new();
        let layout = layout_timeline_callout(
            (60, 8),
            &["3 agents started".to_string()],
            CalloutVariation {
                direction: CalloutDirection::Left,
                horizontal_gap: 4,
                height_percent: 50,
            },
            (2, 80),
            (11, 18),
            (2, 79),
            &mut occupied,
        )
        .expect("callout layout");
        let border_style = Style::default().fg(Color::Red).bg(Color::Black);
        let text_style = Style::default().fg(Color::White).bg(Color::Black);
        draw_timeline_callout(
            &mut grid,
            &layout,
            0,
            2,
            80,
            border_style,
            text_style,
            text_style,
        );
        assert_eq!(occupied.len(), 1);
        let bubble = occupied[0];
        assert!(bubble.x >= 2);
        assert!(bubble.y >= 11);
        assert!(bubble.x + bubble.width as i64 <= 80);
        assert!(bubble.y + bubble.height <= 19);

        let output: String = grid
            .iter()
            .flatten()
            .map(|(character, _)| character)
            .collect();
        assert!(output.contains("3 agents started"));
        assert!(output.contains("│ 3 agents started │"));
        assert!(output.contains('▼'));
        assert!(output.contains('╭'));
        assert!(output.contains('╯'));
        assert_eq!(grid[bubble.y][bubble.x as usize].1.fg, Some(Color::Red));
        assert_eq!(grid[bubble.y + 1][bubble.x as usize + 2].1, text_style);

        let original_x = grid[bubble.y]
            .iter()
            .position(|(character, _)| *character == '╭')
            .expect("visible rounded corner");
        let mut shifted_grid = vec![vec![(' ', Style::default()); 80]; 20];
        draw_timeline_callout(
            &mut shifted_grid,
            &layout,
            5,
            2,
            80,
            border_style,
            text_style,
            text_style,
        );
        let shifted_x = shifted_grid[bubble.y]
            .iter()
            .position(|(character, _)| *character == '╭')
            .expect("shifted rounded corner");
        assert_eq!(shifted_x + 5, original_x);
    }

    #[test]
    fn final_turn_callout_is_forced_inward_and_not_truncated() {
        let mut occupied = Vec::new();
        let layout = layout_timeline_callout(
            (95, 8),
            &["3 agents started".to_string()],
            CalloutVariation {
                direction: CalloutDirection::Right,
                horizontal_gap: 4,
                height_percent: 50,
            },
            (2, 100),
            (11, 20),
            (2, 99),
            &mut occupied,
        )
        .expect("edge-aware callout layout");

        assert_eq!(layout.direction, CalloutDirection::Left);
        assert!(layout.bubble.x >= 2);
        assert!(layout.bubble.x + layout.bubble.width as i64 - 1 <= 99);
    }

    #[test]
    fn dense_timeline_callouts_never_overlap() {
        let mut occupied = Vec::new();
        let mut layouts = Vec::new();
        for (index, target_x) in [80, 82, 84, 86].into_iter().enumerate() {
            let layout = layout_timeline_callout(
                (target_x, 8),
                &["auto-compact reduced context by 13%".to_string()],
                CalloutVariation {
                    direction: if index % 2 == 0 {
                        CalloutDirection::Left
                    } else {
                        CalloutDirection::Right
                    },
                    horizontal_gap: 3,
                    height_percent: 50,
                },
                (2, 100),
                (11, 20),
                (2, 219),
                &mut occupied,
            )
            .unwrap_or_else(|| {
                panic!("collision-free callout layout {index}; occupied: {occupied:?}")
            });
            layouts.push(layout);
        }

        for (index, layout) in layouts.iter().enumerate() {
            assert!(layouts[index + 1..]
                .iter()
                .all(|other| !layout.bubble.overlaps(other.bubble)));
        }
    }

    #[test]
    fn one_turn_gets_one_canonical_compaction_callout() {
        let mut turn = turn_with_context(0, None);
        turn.telemetry.observe_context(80_000, Some(100_000));
        turn.telemetry.observe_context(0, Some(100_000));
        assert!(turn.telemetry.mark_context_compaction());
        turn.telemetry.observe_context(67_000, Some(100_000));
        turn.telemetry.observe_context(0, Some(100_000));
        assert!(turn.telemetry.mark_context_compaction());
        turn.telemetry.observe_context(40_000, Some(100_000));

        assert_eq!(turn.telemetry.context_compaction_ranges().len(), 2);
        assert_eq!(
            event_callout_messages(&turn),
            vec!["auto-compact reduced context by 27%".to_string()]
        );
    }

    #[test]
    fn relevant_metric_panels_render_selected_event_callout() {
        let mut turn = turn_with_context(0, None);
        turn.agents = (0..3).map(test_agent).collect();
        turn.telemetry.observe_context(80_000, Some(100_000));
        turn.telemetry.observe_context(0, Some(100_000));
        turn.telemetry.mark_context_compaction();
        turn.telemetry.observe_context(67_000, Some(100_000));
        turn.telemetry.reasoning_tokens = 3_200;
        turn.telemetry.reasoning_tokens_emitted = true;
        let turns = vec![turn];

        for (metric, expected_fragments) in [
            (
                CONTEXT_METRIC_INDEX,
                &["auto-compact", "reduced", "context", "13%"][..],
            ),
            (COMPLEXITY_METRIC_INDEX, &["3 agents started"][..]),
        ] {
            let backend = TestBackend::new(100, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    render_metric_panel(
                        frame.buffer_mut(),
                        &turns,
                        0,
                        metric as u8,
                        Some(ProviderKind::Codex),
                        CalloutDirection::Left,
                        (0, turns.len()),
                        area,
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
            for expected in expected_fragments {
                assert!(output.contains(expected), "missing {expected}");
            }
            if metric == COMPLEXITY_METRIC_INDEX {
                assert!(output.contains("╾◉╼"));
            } else {
                assert!(!output.contains("╾◉╼"));
            }
        }
    }

    #[test]
    fn context_panel_never_renders_more_than_the_selected_turn_callout() {
        let mut turns = Vec::new();
        for after in [67_000, 54_000, 41_000] {
            let mut turn = turn_with_context(0, None);
            turn.telemetry.observe_context(80_000, Some(100_000));
            turn.telemetry.observe_context(0, Some(100_000));
            turn.telemetry.mark_context_compaction();
            turn.telemetry.observe_context(after, Some(100_000));
            turns.push(turn);
        }
        turns.push(turn_with_context(39_000, Some(100_000)));

        for (selected, expected_boxes) in [(1, 1), (3, 0)] {
            let backend = TestBackend::new(100, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    render_metric_panel(
                        frame.buffer_mut(),
                        &turns,
                        selected,
                        CONTEXT_METRIC_INDEX as u8,
                        Some(ProviderKind::Codex),
                        CalloutDirection::Left,
                        (0, turns.len()),
                        area,
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
            assert_eq!(output.matches("auto-compact").count(), expected_boxes);
        }
    }

    #[test]
    fn selected_turn_has_cursor_point_and_top_marker() {
        let turns = vec![
            turn_with_context(64_000, Some(256_000)),
            turn_with_context(128_000, Some(256_000)),
            turn_with_context(192_000, Some(256_000)),
        ];
        let backend = TestBackend::new(70, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_metric_panel(
                    frame.buffer_mut(),
                    &turns,
                    1,
                    CONTEXT_METRIC_INDEX as u8,
                    Some(ProviderKind::Codex),
                    CalloutDirection::Left,
                    (0, turns.len()),
                    area,
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
        assert!(output.contains('┊'));
        assert!(output.contains('◆'));
        assert!(output.contains('▼'));
        assert!(output.contains('▲'));
        assert!(output.contains("T2"));
        assert!(output.chars().filter(|character| *character == '┄').count() >= 50);
        assert!(!output.contains('●'));
        assert!(!output
            .chars()
            .any(|glyph| ('\u{2800}'..='\u{28ff}').contains(&glyph)));

        let point = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .find(|cell| cell.symbol() == "◆")
            .expect("selected point");
        assert_eq!(point.bg, Color::Reset);
        assert!(terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .all(|cell| cell.bg == Color::Reset));
    }

    #[test]
    fn compact_dashboard_keeps_percent_axis_labels_on_separate_rows() {
        let mut turns = vec![
            turn_with_context(64_000, Some(256_000)),
            turn_with_context(128_000, Some(256_000)),
            turn_with_context(192_000, Some(256_000)),
        ];
        for turn in &mut turns {
            turn.telemetry.reasoning_tokens = 8_000;
            turn.telemetry.reasoning_tokens_emitted = true;
        }
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut window_start = 0;
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_dashboard(
                    frame.buffer_mut(),
                    &turns,
                    1,
                    Some(ProviderKind::Codex),
                    CalloutDirection::Left,
                    &mut window_start,
                    area,
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
        assert!(output.contains("100%"));
        assert!(!output.contains("150%"));
    }

    #[test]
    fn dashboard_layout_adapts_without_creating_independent_timelines() {
        assert_eq!(dashboard_columns(150, 24), 3);
        assert_eq!(dashboard_columns(90, 30), 2);
        assert_eq!(dashboard_columns(80, 20), 3);
        assert_eq!(dashboard_columns(60, 50), 1);

        assert_eq!(dashboard_panel_areas(Rect::new(0, 0, 90, 30)).len(), 6);
        assert_eq!(dashboard_panel_areas(Rect::new(0, 0, 60, 50)).len(), 6);
        assert_eq!(shared_turn_window(40, 20, 50, 11), (11, 30));
        assert_eq!(shared_turn_window(40, 21, 50, 11), (11, 30));
        assert_eq!(shared_turn_window(40, 30, 50, 11), (12, 31));
    }

    #[test]
    fn dashboard_renders_all_six_metrics_at_each_responsive_breakpoint() {
        let turns = vec![turn_with_context(64_000, Some(256_000))];

        for (width, height) in [(150, 24), (90, 30), (60, 46)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut window_start = 0;
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    render_dashboard(
                        frame.buffer_mut(),
                        &turns,
                        0,
                        Some(ProviderKind::Codex),
                        CalloutDirection::Left,
                        &mut window_start,
                        area,
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
            for name in [
                "context",
                "duration",
                "cost est.",
                "tokens",
                "turn complexity",
                "code diff",
            ] {
                assert!(output.contains(name), "missing {name} at {width}x{height}");
            }
        }
    }

    #[test]
    fn graph_axis_values_include_metric_units() {
        assert_eq!(format_axis_value(0, 3.05), "$3.05");
        assert_eq!(format_axis_value(1, 125_000.0), "2m 05s");
        assert_eq!(format_axis_value(2, 1_500_000.0), "1.5M tok");
        assert_eq!(format_axis_value(3, 50.0), "50%");
        assert_eq!(format_axis_value(4, 50.0), "50%");
        assert_eq!(format_axis_value(5, 18.0), "18 lines");
        assert_eq!(format_complexity_percent(6.25), "6.2%");
    }

    #[test]
    fn active_turn_duration_is_pending_until_codex_emits_it() {
        let mut turn = turn_with_context(0, None);
        assert_eq!(selected_metric_value(&turn, 1), "pending");

        turn.telemetry.outcome = crate::model::TurnOutcome::Completed;
        assert_eq!(selected_metric_value(&turn, 1), "not emitted");
    }

    #[test]
    fn context_graph_carries_missing_samples_but_preserves_emitted_resets() {
        let turns = vec![
            turn_with_context(64_000, Some(256_000)),
            turn_with_context(0, None),
            turn_with_context(0, Some(256_000)),
        ];

        assert_eq!(
            metric_series(&turns, 3),
            vec![Some(25.0), Some(25.0), Some(0.0)]
        );
    }

    #[test]
    fn word_wrap_handles_curly_quotes_at_boundary() {
        let wrapped = word_wrap(
            "Codex turns have unknown cost, and the graph treats all-zero cost as “nothing to draw”.",
            58,
        );

        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|line| line.width() <= 58));
        assert!(wrapped.iter().any(|line| line.contains("“nothing")));
    }

    #[test]
    fn word_wrap_keeps_a_full_width_word_before_a_boundary_space() {
        assert_eq!(
            word_wrap("auto-compact reduced context by 13%", 20),
            vec!["auto-compact reduced", "context by 13%"]
        );
    }

    #[test]
    fn word_wrap_splits_long_unicode_words_on_char_boundaries() {
        let wrapped = word_wrap("prefix “abcdefghijklmnopqrstuvwxyz” suffix", 10);

        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().all(|line| line.width() <= 10));
        assert!(wrapped.iter().any(|line| line.contains('”')));
    }
}

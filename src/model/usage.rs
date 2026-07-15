/// Token usage and cost tracking per session.
use crate::provider::ProviderKind;
use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Default, Clone)]
pub struct UsageStats {
    pub turns: Vec<TurnUsage>,
}

#[derive(Clone)]
pub struct TurnUsage {
    pub prompt: String,
    pub timestamp: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost: f64,
    pub agents: Vec<AgentCost>,
    /// Cumulative input context at this turn (running total)
    pub cumulative_context: u64,
    /// Tokens processed by sub-agents (context that didn't enter parent)
    pub context_saved: u64,
    /// Raw assistant response text.
    pub response_text: String,
    /// Whether Aether could map this provider/model token usage to a known price.
    pub cost_known: bool,
    /// Exact operational signals emitted by the provider runtime.
    pub telemetry: TurnTelemetry,
}

impl TurnUsage {
    pub fn lines_added(&self) -> u64 {
        self.telemetry.lines_added
            + self
                .agents
                .iter()
                .map(|agent| agent.lines_added)
                .sum::<u64>()
    }

    pub fn lines_removed(&self) -> u64 {
        self.telemetry.lines_removed
            + self
                .agents
                .iter()
                .map(|agent| agent.lines_removed)
                .sum::<u64>()
    }

    pub fn diff_lines(&self) -> u64 {
        self.lines_added().saturating_add(self.lines_removed())
    }

    pub fn files_created(&self) -> u32 {
        self.agents
            .iter()
            .fold(self.telemetry.files_created, |total, agent| {
                total.saturating_add(agent.files_created)
            })
    }

    pub fn files_deleted(&self) -> u32 {
        self.agents
            .iter()
            .fold(self.telemetry.files_deleted, |total, agent| {
                total.saturating_add(agent.files_deleted)
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum TurnOutcome {
    #[default]
    InProgress,
    Completed,
    Aborted,
    Failed,
}

impl TurnOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::InProgress => "in progress",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }

    pub fn score(&self) -> f64 {
        match self {
            Self::Completed => 1.0,
            Self::InProgress => 0.5,
            Self::Aborted | Self::Failed => 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextSample {
    pub used_tokens: u64,
    pub context_window: u64,
    pub compacted: bool,
}

impl ContextSample {
    pub fn percent(&self) -> f64 {
        self.used_tokens.min(self.context_window) as f64 / self.context_window.max(1) as f64 * 100.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct TurnTelemetry {
    pub model: Option<String>,
    /// Input context reported for the latest model request in this turn.
    pub latest_input_tokens: u64,
    pub reasoning_tokens: u64,
    pub reasoning_tokens_emitted: bool,
    /// Inclusive output tokens for Claude responses that contain a thinking block.
    /// Used only when Claude Code omits the exact thinking-token breakdown.
    pub complexity_proxy_tokens: u64,
    pub complexity_proxy_emitted: bool,
    pub context_window: Option<u64>,
    /// Native request-level context samples retained for compaction visualization.
    pub context_samples: Vec<ContextSample>,
    /// Deduplicates paired compaction records when no context sample is available.
    pub compaction_record_pending: bool,
    pub duration_ms: Option<u64>,
    pub outcome: TurnOutcome,
    pub tool_calls: u32,
    pub patches: u32,
    pub web_searches: u32,
    pub compactions: u32,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub files_created: u32,
    pub files_deleted: u32,
}

impl TurnTelemetry {
    pub fn observe_context(&mut self, latest_input_tokens: u64, context_window: Option<u64>) {
        self.latest_input_tokens = latest_input_tokens;
        if context_window.is_some() {
            self.context_window = context_window;
        }
        self.compaction_record_pending = false;

        let Some(window) = self.context_window else {
            return;
        };
        let sample = ContextSample {
            used_tokens: latest_input_tokens.min(window),
            context_window: window,
            compacted: false,
        };
        if self.context_samples.last().is_some_and(|last| {
            last.used_tokens == sample.used_tokens && last.context_window == sample.context_window
        }) {
            return;
        }
        self.context_samples.push(sample);
    }

    /// Record one physical compaction, deduplicating Codex's paired event records.
    pub fn mark_context_compaction(&mut self) -> bool {
        if let Some(sample) = self.context_samples.last_mut() {
            if sample.compacted {
                return false;
            }
            sample.compacted = true;
        } else if self.compaction_record_pending {
            return false;
        } else {
            self.compaction_record_pending = true;
        }
        self.compactions += 1;
        true
    }

    /// Native pre/post percentages for each compaction observed in this turn.
    pub fn context_compaction_ranges(&self) -> Vec<(f64, f64)> {
        self.context_samples
            .iter()
            .enumerate()
            .filter(|(_, sample)| sample.compacted)
            .filter_map(|(index, sample)| {
                let before = self.context_samples[..index]
                    .iter()
                    .rev()
                    .find(|candidate| candidate.used_tokens > 0)
                    .map(ContextSample::percent)?;
                let after = self.context_samples[index + 1..]
                    .iter()
                    .find(|candidate| candidate.used_tokens > 0)
                    .map(ContextSample::percent)
                    .unwrap_or_else(|| sample.percent());
                Some((before, after))
            })
            .collect()
    }

    /// Percentage of the provider-reported context window used by the latest request.
    pub fn context_percent(&self) -> Option<f64> {
        self.context_window.map(|window| {
            self.latest_input_tokens.min(window) as f64 / window.max(1) as f64 * 100.0
        })
    }

    /// Convert native reasoning usage to a stable 0-100 turn-complexity scale.
    pub fn complexity_percent(&self) -> Option<f64> {
        const MAX_COMPLEXITY_REASONING_TOKENS: f64 = 16_000.0;

        if !self.reasoning_tokens_emitted && !self.complexity_proxy_emitted {
            return None;
        }
        let tokens = self
            .reasoning_tokens
            .saturating_add(self.complexity_proxy_tokens);

        Some((tokens as f64 / MAX_COMPLEXITY_REASONING_TOKENS * 100.0).clamp(0.0, 100.0))
    }

    pub fn complexity_basis(&self) -> Option<&'static str> {
        if self.reasoning_tokens_emitted && self.complexity_proxy_emitted {
            Some("reasoning + thinking-output proxy")
        } else if self.reasoning_tokens_emitted {
            Some("reasoning tokens")
        } else if self.complexity_proxy_emitted {
            Some("thinking-output proxy")
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentCost {
    /// Stable provider-native ID for this nested run.
    pub id: String,
    pub name: String,
    pub role: String,
    pub model: Option<String>,
    pub cost: f64,
    pub cost_known: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub outcome: TurnOutcome,
    pub duration_ms: Option<u64>,
    pub tool_calls: u32,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub files_created: u32,
    pub files_deleted: u32,
    /// The initial prompt given to the agent
    pub prompt: String,
    /// Key text snippets from agent responses
    pub response_preview: String,
}

impl UsageStats {
    pub fn total_cost(&self) -> f64 {
        self.turns
            .iter()
            .map(|turn| turn.cost + turn.agents.iter().map(|agent| agent.cost).sum::<f64>())
            .sum()
    }

    pub fn cost_is_known(&self) -> bool {
        self.turns.iter().any(|turn| {
            turn.cost_known
                || turn.cost > 0.0
                || turn
                    .agents
                    .iter()
                    .any(|agent| agent.cost_known || agent.cost > 0.0)
        })
    }

    pub fn cost_is_complete(&self) -> bool {
        !self.turns.is_empty()
            && self.turns.iter().all(|turn| {
                (turn.cost_known || turn.cost > 0.0)
                    && turn
                        .agents
                        .iter()
                        .all(|agent| agent.cost_known || agent.cost > 0.0)
            })
    }

    pub fn total_input(&self) -> u64 {
        self.turns
            .iter()
            .map(|turn| {
                turn.input_tokens
                    + turn
                        .agents
                        .iter()
                        .map(|agent| agent.input_tokens)
                        .sum::<u64>()
            })
            .sum()
    }

    pub fn total_output(&self) -> u64 {
        self.turns
            .iter()
            .map(|turn| {
                turn.output_tokens
                    + turn
                        .agents
                        .iter()
                        .map(|agent| agent.output_tokens)
                        .sum::<u64>()
            })
            .sum()
    }

    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }
}

#[derive(Clone, Copy)]
struct TokenRates {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_write_1h: f64,
    cache_read: f64,
    cache_read_is_input_subset: bool,
}

#[derive(Deserialize)]
struct PricingCatalog {
    schema_version: u32,
    updated_at: String,
    models: Vec<ModelPricing>,
}

#[derive(Deserialize)]
struct ModelPricing {
    provider: String,
    label: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    patterns: Vec<String>,
    input: f64,
    output: f64,
    cache_write: f64,
    #[serde(default)]
    cache_write_1h: Option<f64>,
    cache_read: f64,
    cache_read_is_input_subset: bool,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    effective_from: Option<String>,
    #[serde(default)]
    effective_until: Option<String>,
    #[serde(default)]
    long_context: Option<LongContextPricing>,
    source: String,
}

#[derive(Clone, Copy, Deserialize)]
struct LongContextPricing {
    threshold: u64,
    input_multiplier: f64,
    output_multiplier: f64,
}

impl ModelPricing {
    fn matches(&self, provider: &str, model: &str, today: NaiveDate) -> bool {
        if self.provider != provider || !self.active_on(today) {
            return false;
        }
        self.aliases.iter().any(|alias| model == alias)
            || self.patterns.iter().any(|pattern| model.contains(pattern))
    }

    fn active_on(&self, date: NaiveDate) -> bool {
        let starts = self
            .effective_from
            .as_deref()
            .and_then(parse_catalog_date)
            .map(|start| date >= start)
            .unwrap_or(true);
        let ends = self
            .effective_until
            .as_deref()
            .and_then(parse_catalog_date)
            .map(|end| date <= end)
            .unwrap_or(true);
        starts && ends
    }

    fn rates(&self, input_tokens: u64) -> TokenRates {
        let mut rates = TokenRates {
            input: self.input,
            output: self.output,
            cache_write: self.cache_write,
            cache_write_1h: self.cache_write_1h.unwrap_or(self.cache_write),
            cache_read: self.cache_read,
            cache_read_is_input_subset: self.cache_read_is_input_subset,
        };
        if let Some(long_context) = self
            .long_context
            .filter(|pricing| input_tokens > pricing.threshold)
        {
            rates.input *= long_context.input_multiplier;
            rates.cache_read *= long_context.input_multiplier;
            rates.cache_write *= long_context.input_multiplier;
            rates.cache_write_1h *= long_context.input_multiplier;
            rates.output *= long_context.output_multiplier;
        }
        rates
    }
}

fn parse_catalog_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

fn pricing_catalog() -> &'static PricingCatalog {
    static CATALOG: OnceLock<PricingCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("pricing.json"))
            .expect("embedded pricing catalog must be valid")
    })
}

fn pricing_for(
    provider: ProviderKind,
    model: &str,
    pricing_date: NaiveDate,
) -> Option<&'static ModelPricing> {
    let provider_id = match provider {
        ProviderKind::Claude => "anthropic",
        ProviderKind::Codex => "openai",
    };
    let normalized = model.trim().to_ascii_lowercase();
    pricing_catalog()
        .models
        .iter()
        .find(|pricing| pricing.matches(provider_id, &normalized, pricing_date))
}

impl TokenRates {
    fn cost(
        self,
        input: u64,
        output: u64,
        cache_write: u64,
        cache_write_1h: u64,
        cache_read: u64,
    ) -> f64 {
        let billable_input = if self.cache_read_is_input_subset {
            input.saturating_sub(cache_read)
        } else {
            input
        };

        (billable_input as f64 * self.input
            + output as f64 * self.output
            + cache_write as f64 * self.cache_write
            + cache_write_1h as f64 * self.cache_write_1h
            + cache_read as f64 * self.cache_read)
            / 1_000_000.0
    }
}

/// Calculate USD cost for provider-specific token accounting.
pub fn compute_provider_cost(
    provider: ProviderKind,
    model: &str,
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
) -> Option<f64> {
    compute_provider_cost_at(
        provider,
        model,
        input,
        output,
        cache_write,
        cache_read,
        Utc::now().date_naive(),
    )
}

/// Calculate a token-only API cost estimate using prices effective on the usage date.
pub fn compute_provider_cost_at(
    provider: ProviderKind,
    model: &str,
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
    pricing_date: NaiveDate,
) -> Option<f64> {
    let pricing = pricing_for(provider, model, pricing_date)?;
    let rates = pricing.rates(input);
    Some(rates.cost(input, output, cache_write, 0, cache_read))
}

/// Calculate a token-only API cost estimate with provider-emitted cache TTLs.
pub fn compute_provider_cost_with_cache_ttl_at(
    provider: ProviderKind,
    model: &str,
    input: u64,
    output: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
    cache_read: u64,
    pricing_date: NaiveDate,
) -> Option<f64> {
    let pricing = pricing_for(provider, model, pricing_date)?;
    let rates = pricing.rates(input);
    Some(rates.cost(input, output, cache_write_5m, cache_write_1h, cache_read))
}

pub fn pricing_catalog_metadata() -> (u32, &'static str) {
    let catalog = pricing_catalog();
    (catalog.schema_version, catalog.updated_at.as_str())
}

pub fn pricing_source(provider: ProviderKind, model: &str) -> Option<(&'static str, &'static str)> {
    pricing_source_at(provider, model, Utc::now().date_naive())
}

pub fn pricing_source_at(
    provider: ProviderKind,
    model: &str,
    pricing_date: NaiveDate,
) -> Option<(&'static str, &'static str)> {
    pricing_for(provider, model, pricing_date)
        .map(|pricing| (pricing.label.as_str(), pricing.source.as_str()))
}

/// Return the cataloged model context window when the provider transcript omits it.
pub fn model_context_window_at(
    provider: ProviderKind,
    model: &str,
    date: NaiveDate,
) -> Option<u64> {
    pricing_for(provider, model, date)?.context_window
}

/// Format a token count for display (e.g., 1200 -> "1.2k", 1500000 -> "1.5M")
pub fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

/// Format a dollar amount for display
pub fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        "<1¢".to_string()
    } else if cost < 10.0 {
        format!("${:.2}", cost)
    } else {
        format!("${:.1}", cost)
    }
}

pub fn format_duration(duration_ms: u64) -> String {
    let total_seconds = duration_ms / 1000;
    if total_seconds < 60 {
        format!("{:.1}s", duration_ms as f64 / 1000.0)
    } else {
        format!("{}m {:02}s", total_seconds / 60, total_seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_claude_cost_with_prompt_cache_rates() {
        let cost = compute_provider_cost(
            ProviderKind::Claude,
            "claude-sonnet-4-6",
            1000,
            100,
            50,
            200,
        )
        .unwrap();

        let expected = (1000.0 * 3.0 + 100.0 * 15.0 + 50.0 * 3.75 + 200.0 * 0.30) / 1_000_000.0;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn computes_claude_one_hour_cache_writes_at_emitted_ttl_rate() {
        let cost = compute_provider_cost_with_cache_ttl_at(
            ProviderKind::Claude,
            "claude-fable-5",
            1_000,
            100,
            50,
            200,
            300,
            NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
        )
        .unwrap();

        let expected = (1_000.0 * 10.0 + 100.0 * 50.0 + 50.0 * 12.5 + 200.0 * 20.0 + 300.0 * 1.0)
            / 1_000_000.0;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn computes_openai_cost_with_cached_input_subset() {
        let cost =
            compute_provider_cost(ProviderKind::Codex, "gpt-5.5", 1000, 100, 0, 200).unwrap();

        let expected = (800.0 * 5.0 + 200.0 * 0.50 + 100.0 * 30.0) / 1_000_000.0;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn leaves_unknown_models_unpriced() {
        assert!(
            compute_provider_cost(ProviderKind::Codex, "codex-auto-review", 1000, 100, 0, 0)
                .is_none()
        );
    }

    #[test]
    fn computes_gpt_5_6_sol_cost_with_cached_input() {
        let cost =
            compute_provider_cost(ProviderKind::Codex, "gpt-5.6-sol", 1000, 100, 0, 200).unwrap();

        let expected = (800.0 * 5.0 + 200.0 * 0.50 + 100.0 * 30.0) / 1_000_000.0;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn applies_long_context_pricing_only_to_models_that_define_it() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap();
        let sol = compute_provider_cost_at(
            ProviderKind::Codex,
            "gpt-5.6-sol",
            300_000,
            1_000,
            0,
            50_000,
            date,
        )
        .unwrap();
        let mini = compute_provider_cost_at(
            ProviderKind::Codex,
            "gpt-5.4-mini",
            300_000,
            1_000,
            0,
            50_000,
            date,
        )
        .unwrap();

        let expected_sol = (250_000.0 * 10.0 + 50_000.0 * 1.0 + 1_000.0 * 45.0) / 1_000_000.0;
        let expected_mini = (250_000.0 * 0.75 + 50_000.0 * 0.075 + 1_000.0 * 4.5) / 1_000_000.0;
        assert!((sol - expected_sol).abs() < f64::EPSILON);
        assert!((mini - expected_mini).abs() < f64::EPSILON);
    }

    #[test]
    fn uses_date_effective_pricing_for_historical_turns() {
        let introductory = compute_provider_cost_at(
            ProviderKind::Claude,
            "claude-sonnet-5",
            1_000,
            100,
            0,
            0,
            NaiveDate::from_ymd_opt(2026, 8, 31).unwrap(),
        )
        .unwrap();
        let standard = compute_provider_cost_at(
            ProviderKind::Claude,
            "claude-sonnet-5",
            1_000,
            100,
            0,
            0,
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
        )
        .unwrap();

        assert!((introductory - 0.003).abs() < f64::EPSILON);
        assert!((standard - 0.0045).abs() < f64::EPSILON);
    }

    #[test]
    fn catalog_exposes_version_and_model_specific_source() {
        assert_eq!(pricing_catalog_metadata(), (3, "2026-07-15"));
        let source = pricing_source_at(
            ProviderKind::Codex,
            "gpt-5.5-pro-2026-05-01",
            NaiveDate::from_ymd_opt(2026, 7, 14).unwrap(),
        )
        .unwrap();
        assert_eq!(source.0, "GPT-5.5 Pro");
        assert_eq!(
            source.1,
            "https://developers.openai.com/api/docs/models/gpt-5.5-pro"
        );
    }

    #[test]
    fn formats_subminute_and_multiminute_durations() {
        assert_eq!(format_duration(1250), "1.2s");
        assert_eq!(format_duration(125_000), "2m 05s");
    }

    #[test]
    fn converts_reasoning_tokens_to_absolute_complexity_percent() {
        let telemetry = TurnTelemetry {
            reasoning_tokens: 4_000,
            reasoning_tokens_emitted: true,
            ..TurnTelemetry::default()
        };
        assert_eq!(telemetry.complexity_percent(), Some(25.0));

        let capped = TurnTelemetry {
            reasoning_tokens: 32_000,
            reasoning_tokens_emitted: true,
            ..TurnTelemetry::default()
        };
        assert_eq!(capped.complexity_percent(), Some(100.0));
        assert_eq!(TurnTelemetry::default().complexity_percent(), None);

        let proxy = TurnTelemetry {
            complexity_proxy_tokens: 800,
            complexity_proxy_emitted: true,
            ..TurnTelemetry::default()
        };
        assert_eq!(proxy.complexity_percent(), Some(5.0));
        assert_eq!(proxy.complexity_basis(), Some("thinking-output proxy"));
    }

    #[test]
    fn catalog_supplies_provider_context_windows_when_transcripts_omit_them() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
        assert_eq!(
            model_context_window_at(ProviderKind::Claude, "claude-opus-4-8", date),
            Some(1_000_000)
        );
        assert_eq!(
            model_context_window_at(ProviderKind::Claude, "claude-opus-4-5", date),
            Some(200_000)
        );
        assert_eq!(
            model_context_window_at(ProviderKind::Claude, "claude-sonnet-4-6", date),
            Some(1_000_000)
        );
        assert_eq!(
            model_context_window_at(ProviderKind::Claude, "claude-haiku-4-5", date),
            Some(200_000)
        );
    }

    #[test]
    fn converts_latest_input_to_context_percent() {
        let telemetry = TurnTelemetry {
            latest_input_tokens: 64_000,
            context_window: Some(256_000),
            ..TurnTelemetry::default()
        };
        assert_eq!(telemetry.context_percent(), Some(25.0));

        let capped = TurnTelemetry {
            latest_input_tokens: 300_000,
            context_window: Some(256_000),
            ..TurnTelemetry::default()
        };
        assert_eq!(capped.context_percent(), Some(100.0));
        assert_eq!(TurnTelemetry::default().context_percent(), None);
    }

    #[test]
    fn preserves_native_context_compaction_range_and_deduplicates_records() {
        let mut telemetry = TurnTelemetry::default();
        telemetry.observe_context(229_899, Some(258_400));
        telemetry.observe_context(0, Some(258_400));
        assert!(telemetry.mark_context_compaction());
        assert!(!telemetry.mark_context_compaction());
        telemetry.observe_context(25_330, Some(258_400));
        telemetry.observe_context(97_978, Some(258_400));

        assert_eq!(telemetry.compactions, 1);
        let ranges = telemetry.context_compaction_ranges();
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].0 - 88.970_201).abs() < 0.001);
        assert!((ranges[0].1 - 9.802_632).abs() < 0.001);
        assert!((telemetry.context_percent().unwrap() - 37.917_183).abs() < 0.001);
    }
}

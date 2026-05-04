/// Token usage and cost tracking per session.

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
    /// Per-turn quality metrics from Haiku analysis
    pub metrics: Option<TurnMetrics>,
    /// Raw assistant response text (for Haiku analysis)
    pub response_text: String,
}

#[derive(Clone, Debug)]
pub struct TurnMetrics {
    /// Was this turn a correction/friction point? (0.0 = smooth, 1.0 = high friction)
    pub friction: f32,
    /// Likelihood of hallucinated content (0.0 = grounded, 1.0 = hallucinated)
    pub hallucination: f32,
    /// Agent's apparent confidence in its work (0.0 = uncertain, 1.0 = confident)
    pub confidence: f32,
    /// How well the agent accepted/followed user intent (0.0 = ignored, 1.0 = perfect)
    pub acceptance: f32,
    /// Quality of output/deliverable (0.0 = poor, 1.0 = excellent)
    pub performance: f32,
    /// Rolling recap for next turn's context
    pub recap: String,
}

#[derive(Clone)]
pub struct AgentCost {
    pub name: String,
    pub cost: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The initial prompt given to the agent
    pub prompt: String,
    /// Key text snippets from agent responses
    pub response_preview: String,
}

impl UsageStats {
    pub fn total_cost(&self) -> f64 {
        self.turns.iter().map(|t| t.cost).sum()
    }

    pub fn total_input(&self) -> u64 {
        self.turns.iter().map(|t| t.input_tokens).sum()
    }

    pub fn total_output(&self) -> u64 {
        self.turns.iter().map(|t| t.output_tokens).sum()
    }

    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }
}

/// Calculate cost in USD for given token counts and model.
pub fn compute_cost(model: &str, input: u64, output: u64, cache_write: u64, cache_read: u64) -> f64 {
    let (in_rate, out_rate, cw_rate, cr_rate) = match model {
        m if m.contains("opus") => (15.0, 75.0, 18.75, 1.875),
        m if m.contains("sonnet") => (3.0, 15.0, 3.75, 0.375),
        m if m.contains("haiku") => (0.80, 4.0, 1.0, 0.08),
        _ => (3.0, 15.0, 3.75, 0.375), // default to sonnet pricing
    };
    (input as f64 * in_rate + output as f64 * out_rate
        + cache_write as f64 * cw_rate + cache_read as f64 * cr_rate)
        / 1_000_000.0
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

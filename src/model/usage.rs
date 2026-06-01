/// Token usage and cost tracking per session.
use crate::provider::ProviderKind;

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
    /// Whether Aether could map this provider/model token usage to a known price.
    pub cost_known: bool,
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

    pub fn cost_is_known(&self) -> bool {
        self.turns.iter().any(|t| t.cost_known || t.cost > 0.0)
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

#[derive(Clone, Copy)]
struct TokenRates {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
    cache_read_is_input_subset: bool,
}

impl TokenRates {
    fn cost(self, input: u64, output: u64, cache_write: u64, cache_read: u64) -> f64 {
        let billable_input = if self.cache_read_is_input_subset {
            input.saturating_sub(cache_read)
        } else {
            input
        };

        (billable_input as f64 * self.input
            + output as f64 * self.output
            + cache_write as f64 * self.cache_write
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
    let rates = match provider {
        ProviderKind::Claude => claude_rates(model),
        ProviderKind::Codex => openai_rates(model, input),
    }?;
    Some(rates.cost(input, output, cache_write, cache_read))
}

fn claude_rates(model: &str) -> Option<TokenRates> {
    let normalized = model.to_ascii_lowercase();
    let (input, output) = if normalized.contains("opus-4-8")
        || normalized.contains("opus-4-7")
        || normalized.contains("opus-4-6")
        || normalized.contains("opus-4-5")
    {
        (5.0, 25.0)
    } else if normalized.contains("opus") {
        (15.0, 75.0)
    } else if normalized.contains("sonnet") {
        (3.0, 15.0)
    } else if normalized.contains("haiku-4-5") {
        (1.0, 5.0)
    } else if normalized.contains("haiku") {
        (0.80, 4.0)
    } else {
        return None;
    };

    Some(TokenRates {
        input,
        output,
        cache_write: input * 1.25,
        cache_read: input * 0.1,
        cache_read_is_input_subset: false,
    })
}

fn openai_rates(model: &str, input_tokens: u64) -> Option<TokenRates> {
    let normalized = model.to_ascii_lowercase();
    let mut rates = if normalized.contains("gpt-5.5") {
        TokenRates {
            input: 5.0,
            cache_read: 0.50,
            output: 30.0,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-5.4-mini") {
        TokenRates {
            input: 0.75,
            cache_read: 0.075,
            output: 4.50,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-5.4") {
        TokenRates {
            input: 2.50,
            cache_read: 0.25,
            output: 15.0,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-5.3-codex")
        || normalized.contains("gpt-5.2-codex")
        || normalized.contains("gpt-5.2")
    {
        TokenRates {
            input: 1.75,
            cache_read: 0.175,
            output: 14.0,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-5.1")
        || normalized.contains("gpt-5-codex")
        || normalized.contains("gpt-5-chat")
        || normalized == "gpt-5"
    {
        TokenRates {
            input: 1.25,
            cache_read: 0.125,
            output: 10.0,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-5-mini") {
        TokenRates {
            input: 0.25,
            cache_read: 0.025,
            output: 2.0,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-5-nano") {
        TokenRates {
            input: 0.05,
            cache_read: 0.005,
            output: 0.40,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-4.1-mini") {
        TokenRates {
            input: 0.40,
            cache_read: 0.10,
            output: 1.60,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-4.1-nano") {
        TokenRates {
            input: 0.10,
            cache_read: 0.025,
            output: 0.40,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-4.1") {
        TokenRates {
            input: 2.0,
            cache_read: 0.50,
            output: 8.0,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-4o-mini") {
        TokenRates {
            input: 0.15,
            cache_read: 0.075,
            output: 0.60,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else if normalized.contains("gpt-4o") {
        TokenRates {
            input: 2.50,
            cache_read: 1.25,
            output: 10.0,
            cache_write: 0.0,
            cache_read_is_input_subset: true,
        }
    } else {
        return None;
    };

    if (normalized.contains("gpt-5.5") || normalized.contains("gpt-5.4")) && input_tokens > 272_000
    {
        rates.input *= 2.0;
        rates.cache_read *= 2.0;
        rates.output *= 1.5;
    }

    Some(rates)
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
}

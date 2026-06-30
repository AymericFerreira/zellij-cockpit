//! Built-in Claude model price table.
//!
//! Rates are USD per 1M tokens, verified against the Claude pricing reference
//! (2026-06). Cache-write is 1.25x input (5-minute TTL), cache-read is 0.1x
//! input. **Update `RATES` here when prices change.**

use crate::claude::parse::Usage;

/// Per-token prices (already divided down from the per-1M table).
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

/// (input, output) USD per 1M tokens, matched by substring of the model id.
const RATES: &[(&str, f64, f64)] = &[
    ("opus", 5.0, 25.0),
    ("sonnet", 3.0, 15.0),
    ("haiku", 1.0, 5.0),
    ("fable", 10.0, 50.0),
    ("mythos", 10.0, 50.0),
];

/// Default to Opus-tier pricing for unknown models so cost is never silently 0.
const DEFAULT: (f64, f64) = (5.0, 25.0);

pub fn price_for(model: &str) -> Price {
    let m = model.to_ascii_lowercase();
    let (input_1m, output_1m) = RATES
        .iter()
        .find(|(key, _, _)| m.contains(key))
        .map(|&(_, i, o)| (i, o))
        .unwrap_or(DEFAULT);

    Price {
        input: input_1m / 1e6,
        output: output_1m / 1e6,
        cache_write: input_1m * 1.25 / 1e6,
        cache_read: input_1m * 0.1 / 1e6,
    }
}

/// Cost in USD for one assistant turn.
pub fn cost(model: &str, u: &Usage) -> f64 {
    let p = price_for(model);
    u.input_tokens as f64 * p.input
        + u.output_tokens as f64 * p.output
        + u.cache_creation_input_tokens as f64 * p.cache_write
        + u.cache_read_input_tokens as f64 * p.cache_read
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_cost_matches_table() {
        let u = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        // 1M input @ $5 + 1M output @ $25 = $30
        assert!((cost("claude-opus-4-8", &u) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn cache_multipliers_applied() {
        let u = Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: 1_000_000, // 1.25x $5 = $6.25
            cache_read_input_tokens: 1_000_000,     // 0.1x $5 = $0.50
        };
        assert!((cost("claude-opus-4-8", &u) - 6.75).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_uses_default() {
        let u = Usage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        assert!((cost("some-future-model", &u) - 5.0).abs() < 1e-9);
    }
}

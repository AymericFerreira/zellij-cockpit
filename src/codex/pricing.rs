//! Built-in OpenAI model price table for Codex usage.
//!
//! Rates are USD per 1M tokens. Cached input is billed at 0.1x input (OpenAI's
//! standard cached-input discount). **Update `RATES` here when prices change**,
//! and verify the model ids your Codex CLI actually records.

use crate::codex::parse::CodexTokens;

struct Price {
    input: f64,
    cached: f64,
    output: f64,
}

/// (model-id substring, input $/1M, output $/1M).
const RATES: &[(&str, f64, f64)] = &[
    ("gpt-5", 1.25, 10.0), // also matches gpt-5-codex
    ("o4-mini", 1.10, 4.40),
    ("o3", 2.0, 8.0),
    ("gpt-4.1", 2.0, 8.0),
    ("gpt-4o", 2.5, 10.0),
];

/// Default to gpt-5 family pricing for unknown models.
const DEFAULT: (f64, f64) = (1.25, 10.0);

fn price_for(model: &str) -> Price {
    let m = model.to_ascii_lowercase();
    let (input_1m, output_1m) = RATES
        .iter()
        .find(|(key, _, _)| m.contains(key))
        .map(|&(_, i, o)| (i, o))
        .unwrap_or(DEFAULT);
    Price {
        input: input_1m / 1e6,
        cached: input_1m * 0.1 / 1e6,
        output: output_1m / 1e6,
    }
}

/// Cost in USD. `input_tokens` is the full prompt count; `cached_input_tokens`
/// is the (discounted) subset of it, so non-cached input = input - cached.
pub fn cost(model: &str, t: &CodexTokens) -> f64 {
    let p = price_for(model);
    let uncached_input = t.input.saturating_sub(t.cached_input);
    uncached_input as f64 * p.input
        + t.cached_input as f64 * p.cached
        + (t.output + t.reasoning) as f64 * p.output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt5_cost() {
        let t = CodexTokens {
            input: 1_000_000,
            cached_input: 0,
            output: 1_000_000,
            reasoning: 0,
        };
        // 1M input @ $1.25 + 1M output @ $10 = $11.25
        assert!((cost("gpt-5-codex", &t) - 11.25).abs() < 1e-9);
    }

    #[test]
    fn cached_input_discounted() {
        let t = CodexTokens {
            input: 1_000_000,
            cached_input: 1_000_000, // all cached -> 0.1x
            output: 0,
            reasoning: 0,
        };
        assert!((cost("gpt-5", &t) - 0.125).abs() < 1e-9);
    }
}

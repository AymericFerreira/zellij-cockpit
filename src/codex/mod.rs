//! Codex CLI usage engine. Native-only.
//!
//! Scans the OpenAI Codex CLI session rollouts under `~/.codex/sessions/**`,
//! prices each turn against a built-in OpenAI model table, and aggregates via
//! the shared `usage` engine.
//!
//! NOTE: the exact rollout JSON schema is parsed defensively at known shallow
//! paths because it varies across Codex versions; verify against a real
//! `rollout-*.jsonl` if numbers look off.

pub mod parse;
pub mod pricing;

use chrono::{Duration as ChronoDuration, Local};

pub fn current_usage() -> crate::types::ProviderUsage {
    let now = Local::now();
    let (entries, rate_limit) = parse::scan_recent(ChronoDuration::hours(24));
    let mut usage = crate::usage::summarize(entries, now, ChronoDuration::hours(5));

    // Codex records its real rate-limit window (used %, exact reset time). Prefer
    // it over the token-timestamp heuristic — but only while it's actually open,
    // and never fabricate `present`: the chip shows only when there's real usage
    // (set by `summarize`), so a stale snapshot can't conjure a phantom window.
    if let Some(rl) = rate_limit {
        let remaining_secs = rl.resets_at - now.timestamp();
        if remaining_secs > 0 {
            usage.block_active = true;
            usage.block_elapsed_frac = (rl.used_percent / 100.0).clamp(0.0, 1.0);
            usage.block_remaining_min = remaining_secs as f64 / 60.0;
        }
    }

    usage
}

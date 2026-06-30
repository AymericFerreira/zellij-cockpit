//! The JSON contract between the native helper and the WASM plugin.
//!
//! The helper prints one `Metrics` line; the plugin deserializes it and renders.
//! Keep these types free of native-only dependencies (no chrono, etc.) so they
//! compile for the WASM target too.

use serde::{Deserialize, Serialize};

/// One snapshot of everything the bar displays.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Metrics {
    /// Overall CPU utilization, 0..100.
    pub cpu: f32,
    /// Used memory in bytes (total - available).
    pub mem_used: u64,
    /// Total memory in bytes.
    pub mem_total: u64,
    /// Claude Code usage derived from `~/.claude` logs.
    pub claude: ClaudeUsage,
}

/// Claude Code spend, both for the calendar day and the active 5-hour window.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct ClaudeUsage {
    /// Total cost (USD) across all projects since local midnight.
    pub today_cost: f64,
    /// Total tokens (input + output + cache) since local midnight.
    pub today_tokens: u64,
    /// Whether there is an open 5-hour rate-limit block right now.
    pub block_active: bool,
    /// Cost (USD) accrued in the active block.
    pub block_cost: f64,
    /// Tokens accrued in the active block.
    pub block_tokens: u64,
    /// Fraction (0..1) of the 5-hour window that has elapsed.
    pub block_elapsed_frac: f64,
    /// Token burn rate in the active block, tokens/minute.
    pub block_burn_per_min: f64,
}

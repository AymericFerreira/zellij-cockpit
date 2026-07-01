//! The JSON contract between the native helper and the WASM plugin.
//!
//! The helper prints one `Metrics` line; the plugin deserializes it and renders.
//! Keep these types free of native-only dependencies (no chrono, etc.) so they
//! compile for the WASM target too. `#[serde(default)]` lets the plugin tolerate
//! a helper built at a different version (missing fields fall back to defaults).

use serde::{Deserialize, Serialize};

/// One snapshot of everything the bar displays.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(default)]
pub struct Metrics {
    /// Overall CPU utilization, 0..100.
    pub cpu: f32,
    /// Used memory in bytes.
    pub mem_used: u64,
    /// Total memory in bytes.
    pub mem_total: u64,
    /// Claude Code usage (from `~/.claude`).
    pub claude: ProviderUsage,
    /// Codex CLI usage (from `~/.codex`).
    pub codex: ProviderUsage,
}

/// Per-coding-agent usage: calendar-day totals plus the active rate-limit window.
/// Shared shape for Claude and Codex (and any future provider).
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(default)]
pub struct ProviderUsage {
    /// Whether any usage was found at all (controls whether the bar shows it).
    pub present: bool,
    /// Total cost (USD) since local midnight.
    pub today_cost: f64,
    /// Total tokens since local midnight.
    pub today_tokens: u64,
    /// Whether there is an open rate-limit window right now.
    pub block_active: bool,
    /// Cost (USD) accrued in the active window.
    pub block_cost: f64,
    /// Tokens accrued in the active window.
    pub block_tokens: u64,
    /// Fraction (0..1) of the window that has elapsed.
    pub block_elapsed_frac: f64,
    /// Minutes remaining until the active window resets.
    pub block_remaining_min: f64,
}

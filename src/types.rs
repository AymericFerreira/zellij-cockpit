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
    /// Used swap in bytes.
    pub swap_used: u64,
    /// Total swap in bytes (macOS grows this file dynamically, so it moves).
    pub swap_total: u64,
    /// macOS memory pressure, 0..100. `None` where the platform has no such
    /// notion, in which case the bar falls back to coloring by used/total.
    pub mem_pressure: Option<f32>,
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
    /// Fraction (0..1) driving the window bar.
    ///
    /// Its meaning depends on `block_is_quota`, and the two are not
    /// interchangeable: a window can be 72% elapsed while only 12% of the quota
    /// is spent. Read `block_is_quota` before showing this as a percentage.
    pub block_elapsed_frac: f64,
    /// True when `block_elapsed_frac` is real quota used, as reported by the
    /// provider. False when it is merely how much of the window has elapsed,
    /// which is a guess about time and says nothing about quota.
    pub block_is_quota: bool,
    /// Minutes remaining until the active window resets.
    pub block_remaining_min: f64,
}

//! Claude Code usage engine. Native-only.
//!
//! Scans `~/.claude/projects/**/*.jsonl`, prices each assistant turn against a
//! built-in model table, and aggregates via the shared `usage` engine (today +
//! the active 5-hour rate-limit window).

pub mod live;
pub mod parse;
pub mod pricing;

use chrono::{Duration as ChronoDuration, Local};

/// Compute current Claude usage from the local logs alone. Only files touched in
/// the last 24h are read, which covers both "today" and the active block while
/// keeping the scan cheap.
///
/// The rate-limit window this produces is an *estimate*: it measures how much of
/// the window has elapsed, which is not how much quota you have spent. Overlay
/// [`apply_live_window`] to replace it with the real thing.
pub fn current_usage() -> crate::types::ProviderUsage {
    let now = Local::now();
    let entries = parse::scan_recent(ChronoDuration::hours(24));
    crate::usage::summarize(entries, now, ChronoDuration::hours(5))
}

/// Replace the estimated rate-limit window with the real one from the server.
///
/// Only the window is replaced. Cost and tokens still come from the logs: the
/// server reports utilization against your plan, not dollars.
pub fn apply_live_window(usage: &mut crate::types::ProviderUsage, window: live::Window, now: i64) {
    // A window at 0% utilization is not an open window - nothing has been spent in
    // it, and its `resets_at` is a placeholder five hours out rather than a real
    // deadline. Counting it would put a phantom "0%, 5h left" chip on the bar of a
    // machine that has done nothing.
    if window.used_frac <= 0.0 {
        return;
    }
    usage.block_active = true;
    usage.block_is_quota = true;
    usage.block_elapsed_frac = window.used_frac;
    usage.block_remaining_min = window.remaining_min(now);
    // Real usage on the account, so the chip must show even when the local logs
    // are empty (fresh machine, or work done from another client).
    usage.present = true;
}

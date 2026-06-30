//! Claude Code usage engine. Native-only.
//!
//! Scans `~/.claude/projects/**/*.jsonl`, prices each assistant turn against a
//! built-in model table, and aggregates via the shared `usage` engine (today +
//! the active 5-hour rate-limit window).

pub mod parse;
pub mod pricing;

use chrono::{Duration as ChronoDuration, Local};

/// Compute current Claude usage. Only files touched in the last 24h are read,
/// which covers both "today" and the active block while keeping the scan cheap.
pub fn current_usage() -> crate::types::ProviderUsage {
    let now = Local::now();
    let entries = parse::scan_recent(ChronoDuration::hours(24));
    crate::usage::summarize(entries, now, ChronoDuration::hours(5))
}

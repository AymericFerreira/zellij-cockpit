//! Self-contained Claude Code usage engine. Native-only.
//!
//! Scans `~/.claude/projects/**/*.jsonl`, prices each assistant turn against a
//! built-in model table, and aggregates into today's total plus the active
//! 5-hour rate-limit window.

pub mod block;
pub mod parse;
pub mod pricing;

use chrono::{Duration as ChronoDuration, Local};

/// Compute current Claude usage. Only files touched in the last 24h are read,
/// which covers both "today" and the active 5-hour block while keeping the scan
/// cheap even with many projects.
pub fn current_usage() -> crate::types::ClaudeUsage {
    let now = Local::now();
    let mut entries = parse::scan_recent(ChronoDuration::hours(24));
    block::summarize(&mut entries, now)
}

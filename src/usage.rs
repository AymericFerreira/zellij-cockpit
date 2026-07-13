//! Provider-agnostic usage aggregation + log scanning. Native-only.
//!
//! Each provider (Claude, Codex, …) parses its own logs into `Entry` values
//! (timestamp + tokens + already-priced cost). `summarize` then computes the
//! calendar-day total and the active rate-limit window the same way for all of
//! them. The window length is a parameter so each provider can set its own.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, SystemTime};

use chrono::{DateTime, Duration as ChronoDuration, Local};
use walkdir::WalkDir;

use crate::types::ProviderUsage;

/// One priced, timestamped unit of usage (a turn, or a whole session).
pub struct Entry {
    pub timestamp: DateTime<Local>,
    pub tokens: u64,
    pub cost: f64,
    /// Optional key to drop duplicate rows (same turn seen in multiple files).
    pub dedup_key: Option<String>,
}

/// Walk `~/<subdir>/**` and hand each `*.jsonl` modified within `window` to
/// `handle` (path + buffered reader). Shared by every provider's scanner.
pub fn scan_recent_files<F>(subdir: &[&str], window: ChronoDuration, mut handle: F)
where
    F: FnMut(&Path, BufReader<File>),
{
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let mut root = home;
    for part in subdir {
        root = root.join(part);
    }

    let cutoff = SystemTime::now()
        .checked_sub(StdDuration::from_secs(window.num_seconds().max(0) as u64))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(meta) = entry.metadata()
            && let Ok(modified) = meta.modified()
            && modified < cutoff
        {
            continue;
        }
        if let Ok(file) = File::open(path) {
            handle(path, BufReader::new(file));
        }
    }
}

/// Aggregate entries into today's totals and the active `window` block.
///
/// A block is anchored at its first entry's timestamp; a new block starts after
/// a gap longer than `window` or once `window` has elapsed since the anchor. The
/// active block is the most recent one, if `now` still falls inside it.
pub fn summarize(
    mut entries: Vec<Entry>,
    now: DateTime<Local>,
    window: ChronoDuration,
) -> ProviderUsage {
    let mut usage = ProviderUsage::default();
    if entries.is_empty() {
        return usage;
    }
    usage.present = true;

    entries.sort_by_key(|e| e.timestamp);
    let mut seen = HashSet::new();
    entries.retain(|e| match &e.dedup_key {
        Some(k) => seen.insert(k.clone()),
        None => true,
    });

    let today = now.date_naive();
    for e in &entries {
        if e.timestamp.date_naive() == today {
            usage.today_cost += e.cost;
            usage.today_tokens += e.tokens;
        }
    }

    let mut block_start: Option<DateTime<Local>> = None;
    let mut last_ts: Option<DateTime<Local>> = None;
    let mut block_tokens = 0u64;
    let mut block_cost = 0.0;

    for e in &entries {
        let starts_new = match (block_start, last_ts) {
            (Some(bs), Some(lt)) => !(e.timestamp - bs < window && e.timestamp - lt < window),
            _ => true,
        };
        if starts_new {
            // Anchor at the actual first-activity time (not floored to the hour),
            // so an entry just under `window` old isn't wrongly split into a new
            // block and a live window isn't dropped near the boundary.
            block_start = Some(e.timestamp);
            block_tokens = 0;
            block_cost = 0.0;
        }
        block_tokens += e.tokens;
        block_cost += e.cost;
        last_ts = Some(e.timestamp);
    }

    if let Some(bs) = block_start
        && now >= bs
        && now - bs < window
    {
        let elapsed = now - bs;
        usage.block_active = true;
        usage.block_cost = block_cost;
        usage.block_tokens = block_tokens;
        usage.block_elapsed_frac =
            (elapsed.num_seconds() as f64 / window.num_seconds() as f64).clamp(0.0, 1.0);
        usage.block_remaining_min = ((window - elapsed).num_seconds().max(0) as f64) / 60.0;
    }

    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ts: DateTime<Local>, tokens: u64, cost: f64) -> Entry {
        Entry {
            timestamp: ts,
            tokens,
            cost,
            dedup_key: None,
        }
    }

    #[test]
    fn empty_is_absent() {
        let u = summarize(vec![], Local::now(), ChronoDuration::hours(5));
        assert!(!u.present);
        assert!(!u.block_active);
    }

    #[test]
    fn recent_activity_is_an_active_block() {
        let now = Local::now();
        let entries = vec![
            e(now - ChronoDuration::hours(1), 100, 1.0),
            e(now - ChronoDuration::minutes(10), 200, 2.0),
        ];
        let u = summarize(entries, now, ChronoDuration::hours(5));
        assert!(u.present && u.block_active);
        assert_eq!(u.block_tokens, 300);
        assert!((u.block_cost - 3.0).abs() < 1e-9);
        assert!(u.block_remaining_min > 0.0 && u.block_remaining_min <= 300.0);
    }

    #[test]
    fn stale_activity_has_no_active_block() {
        let now = Local::now();
        let u = summarize(
            vec![e(now - ChronoDuration::hours(9), 100, 1.0)],
            now,
            ChronoDuration::hours(5),
        );
        assert!(u.present);
        assert!(!u.block_active);
    }

    #[test]
    fn first_activity_anchor_keeps_a_4h_old_turn_in_the_active_window() {
        // Two turns 4h01m apart with now just after; previously hour-flooring
        // could split these and drop the older turn from the active block.
        let now = Local::now();
        let entries = vec![
            e(now - ChronoDuration::minutes(241), 100, 1.0),
            e(now - ChronoDuration::minutes(1), 200, 2.0),
        ];
        let u = summarize(entries, now, ChronoDuration::hours(5));
        assert!(u.block_active);
        assert_eq!(u.block_tokens, 300); // both turns counted
    }
}

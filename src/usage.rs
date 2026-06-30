//! Provider-agnostic usage aggregation. Native-only.
//!
//! Each provider (Claude, Codex, …) parses its own logs into `Entry` values
//! (timestamp + tokens + already-priced cost). `summarize` then computes the
//! calendar-day total and the active rate-limit window the same way for all of
//! them. The window length is a parameter so each provider can set its own.

use std::collections::HashSet;

use chrono::{DateTime, Duration as ChronoDuration, Local, Timelike};

use crate::types::ProviderUsage;

/// One priced, timestamped unit of usage (a turn, or a whole session).
pub struct Entry {
    pub timestamp: DateTime<Local>,
    pub tokens: u64,
    pub cost: f64,
    /// Optional key to drop duplicate rows (same turn seen in multiple files).
    pub dedup_key: Option<String>,
}

/// Aggregate entries into today's totals and the active `window` block.
///
/// Blocks are anchored at the hour of their first activity; a new block starts
/// after a gap longer than `window` or once `window` has elapsed. The active
/// block is the most recent one, if `now` still falls inside it.
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
            block_start = Some(floor_to_hour(e.timestamp));
            block_tokens = 0;
            block_cost = 0.0;
        }
        block_tokens += e.tokens;
        block_cost += e.cost;
        last_ts = Some(e.timestamp);
    }

    if let Some(bs) = block_start {
        if now >= bs && now - bs < window {
            let elapsed = now - bs;
            let elapsed_min = elapsed.num_seconds() as f64 / 60.0;
            usage.block_active = true;
            usage.block_cost = block_cost;
            usage.block_tokens = block_tokens;
            usage.block_elapsed_frac =
                (elapsed.num_seconds() as f64 / window.num_seconds() as f64).clamp(0.0, 1.0);
            usage.block_remaining_min = ((window - elapsed).num_seconds().max(0) as f64) / 60.0;
            usage.block_burn_per_min = if elapsed_min > 0.0 {
                block_tokens as f64 / elapsed_min
            } else {
                0.0
            };
        }
    }

    usage
}

fn floor_to_hour(dt: DateTime<Local>) -> DateTime<Local> {
    dt.with_minute(0)
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(dt)
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
}

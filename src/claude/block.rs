//! Aggregate parsed entries into today's total and the active 5-hour block.
//!
//! The 5-hour block logic mirrors ccusage: blocks are anchored at the hour of
//! their first activity; a new block starts when an entry is more than 5h after
//! the block start, or more than 5h after the previous entry (inactivity gap).
//! The "active" block is the most recent one, if `now` still falls inside it.

use std::collections::HashSet;

use chrono::{DateTime, Duration as ChronoDuration, Local, Timelike};

use crate::claude::{parse::Entry, pricing};
use crate::types::ClaudeUsage;

pub fn summarize(entries: &mut Vec<Entry>, now: DateTime<Local>) -> ClaudeUsage {
    entries.sort_by_key(|e| e.timestamp);

    // Drop duplicate rows (same message/request appearing in multiple files).
    let mut seen = HashSet::new();
    entries.retain(|e| match &e.dedup_key {
        Some(k) => seen.insert(k.clone()),
        None => true,
    });

    let block = ChronoDuration::hours(5);
    let today = now.date_naive();

    let mut usage = ClaudeUsage::default();

    // --- Today's total ---
    for e in entries.iter() {
        if e.timestamp.date_naive() == today {
            usage.today_cost += pricing::cost(&e.model, &e.usage);
            usage.today_tokens += e.usage.total();
        }
    }

    // --- Active 5-hour block ---
    // Walk forward, resetting the running block whenever a boundary is crossed.
    // After the loop the accumulators hold the most recent block.
    let mut block_start: Option<DateTime<Local>> = None;
    let mut last_ts: Option<DateTime<Local>> = None;
    let mut block_tokens = 0u64;
    let mut block_cost = 0.0;

    for e in entries.iter() {
        let starts_new = match (block_start, last_ts) {
            (Some(bs), Some(lt)) => {
                !(e.timestamp - bs < block && e.timestamp - lt < block)
            }
            _ => true,
        };
        if starts_new {
            block_start = Some(floor_to_hour(e.timestamp));
            block_tokens = 0;
            block_cost = 0.0;
        }
        block_tokens += e.usage.total();
        block_cost += pricing::cost(&e.model, &e.usage);
        last_ts = Some(e.timestamp);
    }

    if let Some(bs) = block_start {
        if now >= bs && now - bs < block {
            let elapsed = now - bs;
            let elapsed_min = elapsed.num_seconds() as f64 / 60.0;
            usage.block_active = true;
            usage.block_cost = block_cost;
            usage.block_tokens = block_tokens;
            usage.block_elapsed_frac =
                (elapsed.num_seconds() as f64 / block.num_seconds() as f64).clamp(0.0, 1.0);
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
    use crate::claude::parse::Usage;

    fn entry(ts: DateTime<Local>, tokens: u64) -> Entry {
        Entry {
            timestamp: ts,
            model: "claude-opus-4-8".to_string(),
            usage: Usage {
                input_tokens: tokens,
                ..Default::default()
            },
            dedup_key: None,
        }
    }

    #[test]
    fn active_block_accumulates_recent_entries() {
        let now = Local::now();
        let mut entries = vec![
            entry(now - ChronoDuration::hours(2), 100),
            entry(now - ChronoDuration::minutes(30), 200),
        ];
        let u = summarize(&mut entries, now);
        assert!(u.block_active);
        assert_eq!(u.block_tokens, 300);
        assert!(u.block_elapsed_frac > 0.0 && u.block_elapsed_frac <= 1.0);
    }

    #[test]
    fn old_only_activity_yields_no_active_block() {
        let now = Local::now();
        let mut entries = vec![entry(now - ChronoDuration::hours(8), 100)];
        let u = summarize(&mut entries, now);
        assert!(!u.block_active);
    }

    #[test]
    fn duplicates_are_dropped() {
        let now = Local::now();
        let mut e1 = entry(now - ChronoDuration::minutes(10), 100);
        e1.dedup_key = Some("dup".to_string());
        let mut e2 = entry(now - ChronoDuration::minutes(5), 100);
        e2.dedup_key = Some("dup".to_string());
        let mut entries = vec![e1, e2];
        let u = summarize(&mut entries, now);
        assert_eq!(u.block_tokens, 100);
    }
}

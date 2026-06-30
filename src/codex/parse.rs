//! Parse OpenAI Codex CLI session rollouts into priced usage entries.
//!
//! Codex writes one rollout `*.jsonl` per session under `~/.codex/sessions/`.
//! The schema differs across versions, so we search each line recursively for
//! the keys we need rather than assuming a fixed path:
//!   * a `model` string (tracked across lines as the current model),
//!   * `last_token_usage` (per-turn deltas, preferred — summable), and
//!   * `total_token_usage` (cumulative — used as a per-file fallback).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration as StdDuration, SystemTime};

use chrono::{DateTime, Duration as ChronoDuration, Local};
use serde_json::Value;
use walkdir::WalkDir;

use crate::codex::pricing;
use crate::usage::Entry;

/// Token counts from a Codex token-usage object. `input` is the full prompt
/// count; `cached_input` is the discounted subset of it.
#[derive(Default, Clone)]
pub struct CodexTokens {
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl CodexTokens {
    /// Tokens to display (cached is a subset of input, so not added separately).
    pub fn display_total(&self) -> u64 {
        self.input + self.output + self.reasoning
    }
    fn is_zero(&self) -> bool {
        self.input == 0 && self.output == 0 && self.reasoning == 0
    }
}

/// The latest primary rate-limit snapshot Codex recorded (the real 5h window).
#[derive(Clone)]
pub struct RateLimit {
    /// Percent of the window's quota used (0..100).
    pub used_percent: f64,
    /// Unix timestamp when the window resets.
    pub resets_at: i64,
    /// When this snapshot was recorded (to keep the newest).
    pub at: DateTime<Local>,
}

/// Scan returns priced usage entries plus the newest rate-limit snapshot.
pub fn scan_recent(window: ChronoDuration) -> (Vec<Entry>, Option<RateLimit>) {
    let mut out = Vec::new();
    let mut latest_rl: Option<RateLimit> = None;

    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return (out, latest_rl);
    };
    let root = home.join(".codex").join("sessions");

    let cutoff = SystemTime::now()
        .checked_sub(StdDuration::from_secs(window.num_seconds().max(0) as u64))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    continue;
                }
            }
        }
        if let Ok(file) = File::open(path) {
            parse_file(BufReader::new(file), &mut out, &mut latest_rl);
        }
    }

    (out, latest_rl)
}

fn parse_file<R: BufRead>(reader: R, out: &mut Vec<Entry>, latest_rl: &mut Option<RateLimit>) {
    let mut current_model = String::new();
    let mut produced_delta = false;
    // Fallback when a file has no per-turn deltas: the largest cumulative total.
    let mut max_total: Option<(CodexTokens, DateTime<Local>, String)> = None;

    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if let Some(m) = find_key(&v, "model").and_then(Value::as_str) {
            if !m.is_empty() {
                current_model = m.to_string();
            }
        }

        let ts = find_key(&v, "timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Local));
        let Some(ts) = ts else { continue };

        // Codex records the real rate-limit window on each token_count event;
        // keep the most recent primary snapshot.
        if let Some(primary) = find_key(&v, "rate_limits").and_then(|r| r.get("primary")) {
            let used = primary.get("used_percent").and_then(Value::as_f64);
            let resets = primary.get("resets_at").and_then(Value::as_i64);
            if let (Some(used_percent), Some(resets_at)) = (used, resets) {
                let newer = latest_rl.as_ref().map(|r| ts > r.at).unwrap_or(true);
                if newer {
                    *latest_rl = Some(RateLimit {
                        used_percent,
                        resets_at,
                        at: ts,
                    });
                }
            }
        }

        if let Some(obj) = find_key(&v, "last_token_usage") {
            let t = extract(obj);
            if !t.is_zero() {
                out.push(Entry {
                    timestamp: ts,
                    tokens: t.display_total(),
                    cost: pricing::cost(&current_model, &t),
                    dedup_key: None,
                });
                produced_delta = true;
            }
        }

        if let Some(obj) = find_key(&v, "total_token_usage") {
            let t = extract(obj);
            let better = max_total
                .as_ref()
                .map(|(mt, _, _)| t.display_total() > mt.display_total())
                .unwrap_or(true);
            if better {
                max_total = Some((t, ts, current_model.clone()));
            }
        }
    }

    if !produced_delta {
        if let Some((t, ts, model)) = max_total {
            if !t.is_zero() {
                out.push(Entry {
                    timestamp: ts,
                    tokens: t.display_total(),
                    cost: pricing::cost(&model, &t),
                    dedup_key: None,
                });
            }
        }
    }
}

fn extract(obj: &Value) -> CodexTokens {
    let f = |k: &str| obj.get(k).and_then(Value::as_u64).unwrap_or(0);
    CodexTokens {
        input: f("input_tokens"),
        cached_input: f("cached_input_tokens"),
        output: f("output_tokens"),
        reasoning: f("reasoning_output_tokens"),
    }
}

/// First depth-first match of `key` anywhere in the JSON value.
fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                return Some(found);
            }
            map.values().find_map(|val| find_key(val, key))
        }
        Value::Array(arr) => arr.iter().find_map(|val| find_key(val, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_per_turn_deltas() {
        let lines = concat!(
            r#"{"timestamp":"2026-06-30T09:00:00Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-30T09:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":500,"reasoning_output_tokens":0,"total_tokens":1500},"total_token_usage":{"input_tokens":1000,"output_tokens":500,"total_tokens":1500}}}}"#,
        );
        let mut out = Vec::new();
        parse_file(Cursor::new(lines), &mut out, &mut None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tokens, 1500);
        assert!(out[0].cost > 0.0);
    }

    #[test]
    fn falls_back_to_cumulative_total() {
        let lines = concat!(
            r#"{"timestamp":"2026-06-30T09:00:00Z","payload":{"model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-30T09:00:05Z","payload":{"total_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}"#,
            "\n",
            r#"{"timestamp":"2026-06-30T09:01:00Z","payload":{"total_token_usage":{"input_tokens":300,"output_tokens":120,"total_tokens":420}}}"#,
        );
        let mut out = Vec::new();
        parse_file(Cursor::new(lines), &mut out, &mut None);
        assert_eq!(out.len(), 1); // one entry from the largest cumulative total
        assert_eq!(out[0].tokens, 420);
    }
}

//! Parse OpenAI Codex CLI session rollouts into priced usage entries.
//!
//! Codex writes one rollout `*.jsonl` per session under `~/.codex/sessions/`.
//! Each line is `{"timestamp": ..., "type": ..., "payload": {...}}`. We read the
//! fields at their known shallow paths (rather than a recursive search) so a
//! stray nested `model` can't clobber pricing, and so each line is walked once:
//!   * `payload.model` — the active model (tracked across lines),
//!   * `payload.info.last_token_usage` — per-turn deltas (preferred, summable),
//!   * `payload.info.total_token_usage` — cumulative (per-file fallback + dedup),
//!   * `payload.rate_limits.primary` — the real rate-limit window.

use std::io::BufRead;
use std::path::Path;

use chrono::{DateTime, Duration as ChronoDuration, Local};
use serde_json::Value;

use crate::codex::pricing;
use crate::usage::{self, Entry};

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

/// The latest primary rate-limit snapshot Codex recorded (the real window).
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

    usage::scan_recent_files(&[".codex", "sessions"], window, |path, reader| {
        parse_file(reader, &session_id(path), &mut out, &mut latest_rl);
    });

    (out, latest_rl)
}

/// The session id is the rollout filename stem (a uuid) — used to scope the
/// dedup key so a duplicated event within a session can't be double-counted.
fn session_id(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

fn parse_file<R: BufRead>(
    reader: R,
    session: &str,
    out: &mut Vec<Entry>,
    latest_rl: &mut Option<RateLimit>,
) {
    let mut current_model = String::new();
    let mut produced_delta = false;
    // Fallback when a file has no per-turn deltas: the largest cumulative total.
    let mut max_total: Option<(CodexTokens, DateTime<Local>, String, u64)> = None;

    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = v.get("payload");

        if let Some(m) = payload
            .and_then(|p| p.get("model"))
            .or_else(|| v.get("model"))
            .and_then(Value::as_str)
            && !m.is_empty()
        {
            current_model = m.to_string();
        }

        let Some(ts) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Local))
        else {
            continue;
        };

        let info = payload.and_then(|p| p.get("info"));

        // Real rate-limit window (kept: newest primary snapshot).
        if let Some(primary) = payload
            .and_then(|p| p.get("rate_limits"))
            .and_then(|r| r.get("primary"))
            && let (Some(used_percent), Some(resets_at)) = (
                primary.get("used_percent").and_then(Value::as_f64),
                primary.get("resets_at").and_then(Value::as_i64),
            )
        {
            let newer = latest_rl.as_ref().map(|r| ts > r.at).unwrap_or(true);
            if newer {
                *latest_rl = Some(RateLimit {
                    used_percent,
                    resets_at,
                    at: ts,
                });
            }
        }

        // Cumulative total on this line — monotonic per session, so it doubles
        // as a stable per-event dedup key and the fallback selector.
        let cumulative = info
            .and_then(|i| i.get("total_token_usage"))
            .and_then(|t| t.get("total_tokens"))
            .and_then(Value::as_u64);

        if let Some(obj) = info.and_then(|i| i.get("last_token_usage")) {
            let t = extract(obj);
            if !t.is_zero() {
                out.push(Entry {
                    timestamp: ts,
                    tokens: t.display_total(),
                    cost: pricing::cost(&current_model, &t),
                    dedup_key: cumulative.map(|c| format!("{session}:{c}")),
                });
                produced_delta = true;
            }
        }

        if let Some(obj) = info.and_then(|i| i.get("total_token_usage")) {
            let t = extract(obj);
            let better = max_total
                .as_ref()
                .map(|(mt, _, _, _)| t.display_total() > mt.display_total())
                .unwrap_or(true);
            if better {
                max_total = Some((t, ts, current_model.clone(), cumulative.unwrap_or(0)));
            }
        }
    }

    if !produced_delta
        && let Some((t, ts, model, cumulative)) = max_total
        && !t.is_zero()
    {
        out.push(Entry {
            timestamp: ts,
            tokens: t.display_total(),
            cost: pricing::cost(&model, &t),
            dedup_key: Some(format!("{session}:{cumulative}")),
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_per_turn_deltas_with_real_shape() {
        let lines = concat!(
            r#"{"timestamp":"2026-06-30T09:00:00Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-30T09:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":500,"reasoning_output_tokens":0,"total_tokens":1500},"total_token_usage":{"input_tokens":1000,"output_tokens":500,"total_tokens":1500}},"rate_limits":{"primary":{"used_percent":9.0,"window_minutes":300,"resets_at":1782862364}}}}"#,
        );
        let mut out = Vec::new();
        let mut rl = None;
        parse_file(Cursor::new(lines), "sess1", &mut out, &mut rl);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tokens, 1500);
        assert!(out[0].cost > 0.0);
        assert_eq!(out[0].dedup_key.as_deref(), Some("sess1:1500"));
        assert!(rl.is_some());
        assert_eq!(rl.unwrap().resets_at, 1782862364);
    }

    #[test]
    fn a_stray_nested_model_does_not_clobber_pricing() {
        // A model embedded deep in an unrelated object must NOT change current_model.
        let lines = concat!(
            r#"{"timestamp":"2026-06-30T09:00:00Z","payload":{"model":"gpt-5-codex"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-30T09:00:05Z","payload":{"response":{"deep":{"model":"gpt-4o"}},"info":{"last_token_usage":{"input_tokens":1000,"output_tokens":0,"total_tokens":1000}}}}"#,
        );
        let mut out = Vec::new();
        let mut rl = None;
        parse_file(Cursor::new(lines), "s", &mut out, &mut rl);
        assert_eq!(out.len(), 1);
        // gpt-5-codex pricing (gpt-5 row), not gpt-4o: 1000 input @ $1.25/M.
        assert!((out[0].cost - 0.00125).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_cumulative_total() {
        let lines = concat!(
            r#"{"timestamp":"2026-06-30T09:00:00Z","payload":{"model":"gpt-5"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-30T09:00:05Z","payload":{"info":{"total_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}}"#,
            "\n",
            r#"{"timestamp":"2026-06-30T09:01:00Z","payload":{"info":{"total_token_usage":{"input_tokens":300,"output_tokens":120,"total_tokens":420}}}}"#,
        );
        let mut out = Vec::new();
        let mut rl = None;
        parse_file(Cursor::new(lines), "s", &mut out, &mut rl);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tokens, 420);
    }
}

//! Read and parse Claude Code JSONL transcripts.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration as StdDuration, SystemTime};

use chrono::{DateTime, Duration as ChronoDuration, Local};
use serde_json::Value;
use walkdir::WalkDir;

/// Token counts from one assistant turn's `message.usage`.
#[derive(Debug, Default, Clone)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl Usage {
    /// All tokens counted toward "usage" (matches ccusage's total).
    pub fn total(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }
}

/// One priced, timestamped assistant turn.
#[derive(Debug, Clone)]
pub struct Entry {
    pub timestamp: DateTime<Local>,
    pub model: String,
    pub usage: Usage,
    /// `message.id:requestId` when available — used to drop duplicate rows.
    pub dedup_key: Option<String>,
}

/// Walk `~/.claude/projects` and parse every `*.jsonl` modified within `window`.
pub fn scan_recent(window: ChronoDuration) -> Vec<Entry> {
    let mut out = Vec::new();

    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return out;
    };
    let root = home.join(".claude").join("projects");

    let cutoff = SystemTime::now()
        .checked_sub(StdDuration::from_secs(window.num_seconds().max(0) as u64))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // Skip files untouched in the window; today + the active block can only
        // live in recently-written files.
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    continue;
                }
            }
        }
        let Ok(file) = File::open(path) else { continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some(e) = parse_line(&line) {
                out.push(e);
            }
        }
    }

    out
}

/// Parse one JSONL line into an `Entry`, or `None` if it isn't a usage-bearing
/// assistant turn. Tolerant of schema drift — missing fields are skipped.
fn parse_line(line: &str) -> Option<Entry> {
    let v: Value = serde_json::from_str(line).ok()?;
    let msg = v.get("message")?;
    let usage_v = msg.get("usage")?;

    let field = |k: &str| usage_v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let usage = Usage {
        input_tokens: field("input_tokens"),
        output_tokens: field("output_tokens"),
        cache_creation_input_tokens: field("cache_creation_input_tokens"),
        cache_read_input_tokens: field("cache_read_input_tokens"),
    };
    if usage.total() == 0 {
        return None;
    }

    let ts_str = v.get("timestamp").and_then(Value::as_str)?;
    let timestamp = DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&Local);

    let model = msg
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let id = msg.get("id").and_then(Value::as_str).unwrap_or("");
    let req = v.get("requestId").and_then(Value::as_str).unwrap_or("");
    let dedup_key = if id.is_empty() && req.is_empty() {
        None
    } else {
        Some(format!("{id}:{req}"))
    };

    Some(Entry {
        timestamp,
        model,
        usage,
        dedup_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_realistic_line() {
        let line = r#"{"type":"assistant","requestId":"req_1","timestamp":"2026-06-30T09:00:00.000Z","message":{"id":"msg_1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}}"#;
        let e = parse_line(line).expect("should parse");
        assert_eq!(e.model, "claude-opus-4-8");
        assert_eq!(e.usage.total(), 180);
        assert_eq!(e.dedup_key.as_deref(), Some("msg_1:req_1"));
    }

    #[test]
    fn skips_lines_without_usage() {
        assert!(parse_line(r#"{"type":"user","message":{"content":"hi"}}"#).is_none());
        assert!(parse_line("not json").is_none());
        assert!(parse_line(r#"{"message":{"usage":{"input_tokens":0,"output_tokens":0}}}"#).is_none());
    }
}

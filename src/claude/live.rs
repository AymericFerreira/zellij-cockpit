//! Real Claude rate-limit usage, read from the same endpoint `/usage` uses.
//!
//! The log scan can only ever *estimate* the rate-limit window: it knows when you
//! were active, not how much quota Anthropic thinks you burned. Those are
//! different quantities and they disagree wildly - a window can be 72% elapsed
//! while you have spent 12% of it.
//!
//! So ask the server. `GET /api/oauth/usage` returns the utilization and reset
//! time that Claude Code's own `/usage` command shows, authenticated with the
//! OAuth token Claude Code already stored on this machine (macOS Keychain, or
//! `~/.claude/.credentials.json` elsewhere). The token is read, used, and
//! dropped - never logged, never written anywhere by us.
//!
//! The endpoint is internal to Claude Code, undocumented, and **rate limited**.
//! Every step is therefore best-effort: no credentials, no network, an expired
//! token, a 429, or a changed payload all yield `None`, and the caller falls back
//! to the estimate. Callers must cache and back off - see the helper.

use std::time::Duration;

use serde::Deserialize;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Keep the bar responsive: a hung network must never stall a render tick.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The rate-limit window as the server sees it.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    /// Fraction of the quota used, 0..1.
    pub used_frac: f64,
    /// When the window resets, as a Unix timestamp.
    ///
    /// Absolute rather than a duration, so a window cached for a minute still
    /// counts down truthfully instead of freezing at the value it was fetched
    /// with.
    pub resets_at: i64,
}

impl Window {
    pub fn remaining_min(&self, now: i64) -> f64 {
        (self.resets_at - now).max(0) as f64 / 60.0
    }
}

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<RawWindow>,
}

#[derive(Deserialize)]
struct RawWindow {
    /// Percent of quota used, 0..100.
    utilization: Option<f64>,
    /// RFC 3339 timestamp.
    resets_at: Option<String>,
}

impl RawWindow {
    fn into_window(self) -> Option<Window> {
        let used_frac = (self.utilization? / 100.0).clamp(0.0, 1.0);
        let resets_at = chrono::DateTime::parse_from_rfc3339(self.resets_at.as_deref()?).ok()?;
        Some(Window {
            used_frac,
            resets_at: resets_at.timestamp(),
        })
    }
}

/// Why a live read failed. Worth distinguishing: being rate limited is a request
/// to stop asking, and deserves a much longer wait than a laptop that is merely
/// offline for a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// HTTP 429. Observed to persist for a long time once tripped.
    RateLimited,
    /// No credentials, no network, expired token, or a payload we don't know.
    Unavailable,
}

/// Fetch the real 5-hour session window.
pub fn fetch_session() -> Result<Window, FetchError> {
    let token = access_token().ok_or(FetchError::Unavailable)?;

    let response = ureq::get(USAGE_URL)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .call();

    let mut response = match response {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(429)) => return Err(FetchError::RateLimited),
        Err(_) => return Err(FetchError::Unavailable),
    };

    response
        .body_mut()
        .read_json::<UsageResponse>()
        .ok()
        .and_then(|body| body.five_hour?.into_window())
        .ok_or(FetchError::Unavailable)
}

/// The OAuth access token Claude Code stored when you logged in.
///
/// We never refresh it: refreshing is Claude Code's job, and it rewrites the
/// token in place as you use it. An expired token simply means a failed request
/// and a fall back to the estimate.
fn access_token() -> Option<String> {
    let raw = raw_credentials()?;
    let creds: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let token = creds
        .get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()?
        .trim()
        .to_string();
    (!token.is_empty()).then_some(token)
}

/// macOS keeps the credentials in the login Keychain.
#[cfg(target_os = "macos")]
fn raw_credentials() -> Option<String> {
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Everything else (Linux, WSL) keeps them in a file.
#[cfg(not(target_os = "macos"))]
fn raw_credentials() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    std::fs::read_to_string(std::path::PathBuf::from(home).join(".claude/.credentials.json")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<Window> {
        serde_json::from_str::<UsageResponse>(json)
            .unwrap()
            .five_hour?
            .into_window()
    }

    #[test]
    fn reads_utilization_and_reset() {
        let w = parse(r#"{"five_hour":{"utilization":14.0,"resets_at":"2026-07-13T15:10:00Z"}}"#)
            .expect("window");
        assert!((w.used_frac - 0.14).abs() < 1e-9);
        assert_eq!(w.resets_at, 1783955400);
    }

    #[test]
    fn counts_down_from_the_absolute_reset_time() {
        let w = parse(r#"{"five_hour":{"utilization":50.0,"resets_at":"2026-07-13T15:10:00Z"}}"#)
            .unwrap();
        // 90 minutes before the reset.
        assert!((w.remaining_min(1783955400 - 5400) - 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_past_reset_clamps_to_zero_rather_than_going_negative() {
        let w = parse(r#"{"five_hour":{"utilization":50.0,"resets_at":"2026-07-13T15:10:00Z"}}"#)
            .unwrap();
        assert_eq!(w.remaining_min(1783955400 + 600), 0.0);
    }

    #[test]
    fn extra_payload_fields_are_ignored() {
        // The endpoint is undocumented and grows fields; parsing must not be
        // brittle to that.
        let w = parse(
            r#"{"five_hour":{"utilization":7.0,"resets_at":"2026-07-13T15:10:00Z",
                 "limit_dollars":null},"seven_day":{"utilization":2.0},"nimbus_quill":null}"#,
        )
        .unwrap();
        assert!((w.used_frac - 0.07).abs() < 1e-9);
    }

    #[test]
    fn missing_or_malformed_fields_yield_no_window() {
        assert!(parse(r#"{"five_hour":{"utilization":null,"resets_at":null}}"#).is_none());
        assert!(parse(r#"{"five_hour":{"utilization":10.0,"resets_at":"nonsense"}}"#).is_none());
        assert!(parse(r#"{"five_hour":null}"#).is_none());
    }
}

//! cockpit-helper: collect system + per-provider usage and print one JSON line.
//!
//! Invoked by the plugin on a timer. Short-lived (no daemon, no lock files).
//! CPU needs two `sysinfo` reads ~300ms apart; everything more expensive than
//! that is cached to `~/.cache/zellij-cockpit/usage.json` so each tick stays
//! cheap - the log scans, and Claude's real rate-limit window.
//!
//! The two caches have different clocks on purpose. Log scans are local and only
//! cost disk, so they refresh every 30s. The live rate-limit lookup is a network
//! request against an endpoint that *rate limits* us, so it refreshes far more
//! slowly and backs off hard when it fails: a bar that ticks every 3s must not
//! turn into a hundred requests an hour.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sysinfo::System;

use zellij_cockpit::system::{CpuUsage, MemUsage};
use zellij_cockpit::types::{Metrics, ProviderUsage};
use zellij_cockpit::{claude, codex};

/// How long a local log scan stays fresh.
const LOGS_TTL: i64 = 30;
/// How long a successful live rate-limit read stays fresh.
///
/// Deliberately slow. The endpoint is rate limited, and observed limits are both
/// strict and slow to clear - a burst of requests locked us out for well over ten
/// minutes. Quota also moves slowly, and the countdown to the reset stays exact
/// between fetches because we cache the absolute reset time rather than a
/// duration. So there is nothing to gain from polling hard and a working bar to
/// lose. Five minutes is 12 requests an hour, however fast the bar ticks.
const LIVE_TTL: i64 = 300;
/// After a failed live read (offline, expired token, 429), wait at least this
/// long before trying again, doubling per consecutive failure up to
/// [`LIVE_BACKOFF_MAX`]. Retrying into a rate limit is how you stay rate limited.
const LIVE_BACKOFF_BASE: i64 = 300;
const LIVE_BACKOFF_MAX: i64 = 3600;
/// Never show a live window older than this: better an honest estimate than a
/// stale quota presented as current.
const LIVE_STALE: i64 = 1200;

/// What we cache between ticks. System metrics are absent because they're cheap
/// enough to compute fresh every time.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct CachedUsage {
    claude: ProviderUsage,
    codex: ProviderUsage,
    /// When the log scans above were taken.
    logs_at: i64,

    /// Last good live window: quota used, and when it resets (absolute).
    live_used_frac: Option<f64>,
    live_resets_at: Option<i64>,
    /// When that live window was fetched.
    live_at: i64,
    /// Earliest time we may call the endpoint again.
    live_retry_at: i64,
    /// Consecutive failed live reads, which lengthen the backoff.
    live_failures: u32,
}

impl CachedUsage {
    /// A cache stamped after `now` means the clock moved backwards (suspend, NTP
    /// correction). Its timestamps are meaningless, so it must be thrown away
    /// rather than treated as fresh until the clock catches up.
    fn is_from_the_future(&self, now: i64) -> bool {
        self.logs_at > now || self.live_at > now
    }
}

/// Back off further with each consecutive failure, so a long rate-limit lockout
/// or an offline laptop settles into hourly retries instead of grinding away.
fn backoff_secs(failures: u32) -> i64 {
    LIVE_BACKOFF_BASE
        .saturating_mul(1i64 << failures.min(8))
        .min(LIVE_BACKOFF_MAX)
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "doctor") {
        std::process::exit(run_doctor());
    }
    // Opt out of the one thing here that touches the network or credentials.
    let live = !args.iter().any(|a| a == "--no-live");

    let mut sys = System::new();

    let mut cpu = CpuUsage::default();
    cpu.sample(&mut sys);
    std::thread::sleep(Duration::from_millis(300));
    cpu.read(&mut sys);

    let mut mem = MemUsage::default();
    mem.update(&mut sys);

    let usage = cached_usage(live);

    let metrics = Metrics {
        cpu: cpu.total,
        mem_used: mem.used,
        mem_total: mem.total,
        swap_used: mem.swap_used,
        swap_total: mem.swap_total,
        mem_pressure: mem.pressure,
        claude: usage.claude,
        codex: usage.codex,
    };

    println!("{}", serde_json::to_string(&metrics).unwrap_or_default());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            CheckStatus::Ok => "OK",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
        }
    }
}

struct DoctorCheck {
    status: CheckStatus,
    name: &'static str,
    message: String,
}

impl DoctorCheck {
    fn ok(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Ok,
            name,
            message: message.into(),
        }
    }

    fn warn(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Warn,
            name,
            message: message.into(),
        }
    }

    fn fail(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Fail,
            name,
            message: message.into(),
        }
    }
}

fn run_doctor() -> i32 {
    let checks = doctor_checks();
    println!("zellij-cockpit doctor");
    for check in &checks {
        println!(
            "{:<4} {:<18} {}",
            check.status.label(),
            check.name,
            check.message
        );
    }

    if checks.iter().any(|c| c.status == CheckStatus::Fail) {
        1
    } else {
        0
    }
}

fn doctor_checks() -> Vec<DoctorCheck> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut checks = vec![
        check_current_helper(),
        check_cache_writable(&cache_path()),
        check_command_on_path("zellij"),
        check_memory(),
        check_claude_live(),
    ];

    match &home {
        Some(home) => {
            checks.push(check_optional_dir(
                "claude logs",
                &home.join(".claude").join("projects"),
            ));
            checks.push(check_optional_dir(
                "codex logs",
                &home.join(".codex").join("sessions"),
            ));
            checks.push(check_optional_file(
                "claude settings",
                &home.join(".claude").join("settings.json"),
            ));
            let plugin_dir = home.join(".config").join("zellij").join("plugins");
            checks.push(check_optional_file(
                "plugin wasm",
                &plugin_dir.join("zellij-cockpit.wasm"),
            ));
            checks.push(check_optional_file(
                "installed helper",
                &plugin_dir.join("cockpit-helper"),
            ));
        }
        None => checks.push(DoctorCheck::warn(
            "home",
            "HOME is not set; install path and agent log checks were skipped",
        )),
    }

    checks
}

fn check_current_helper() -> DoctorCheck {
    match std::env::current_exe() {
        Ok(path) if path.exists() => {
            DoctorCheck::ok("helper", format!("running {}", path.display()))
        }
        Ok(path) => DoctorCheck::fail("helper", format!("{} does not exist", path.display())),
        Err(err) => DoctorCheck::fail("helper", format!("could not resolve current exe: {err}")),
    }
}

/// Say whether the Claude window is the real quota or a local estimate. This is
/// the check to read when the bar's percentage disagrees with `/usage`.
fn check_claude_live() -> DoctorCheck {
    match zellij_cockpit::claude::live::fetch_session() {
        Some(w) => DoctorCheck::ok(
            "claude quota",
            format!(
                "live: 5h window {:.0}% used, resets in {:.0}m",
                w.used_frac * 100.0,
                w.remaining_min(now_epoch())
            ),
        ),
        None => DoctorCheck::warn(
            "claude quota",
            "no live quota (not logged in, offline, or token expired); \
             the window falls back to a time-elapsed estimate and shows no percentage",
        ),
    }
}

/// Report what the bar will actually display for memory, and which signal colors
/// it. Memory is the one metric whose meaning is platform-dependent, so this is
/// the check to read when the MEM or SWAP segment looks wrong.
fn check_memory() -> DoctorCheck {
    let mut sys = System::new();
    let mut mem = MemUsage::default();
    mem.update(&mut sys);

    if mem.total == 0 {
        return DoctorCheck::fail("memory", "total memory reported as 0");
    }

    let gb = |bytes: u64| bytes as f64 / 1e9;
    let used_pct = mem.used as f64 / mem.total as f64 * 100.0;

    let swap = if mem.swap_total == 0 {
        "no swap configured (SWAP segment hidden)".to_string()
    } else {
        format!(
            "swap {:.1}/{:.1}G used",
            gb(mem.swap_used),
            gb(mem.swap_total)
        )
    };

    // Where pressure exists it colors MEM; elsewhere used/total does, which is
    // the right signal on Linux because `used` there already excludes the
    // reclaimable page cache.
    let coloring = match mem.pressure {
        Some(p) => format!("pressure {p:.0}% (colors MEM)"),
        None => "no pressure signal on this platform; MEM colored by used/total".to_string(),
    };

    DoctorCheck::ok(
        "memory",
        format!(
            "{:.1}/{:.1}G used ({used_pct:.0}%), {swap}, {coloring}",
            gb(mem.used),
            gb(mem.total)
        ),
    )
}

fn check_cache_writable(path: &Path) -> DoctorCheck {
    let Some(dir) = path.parent() else {
        return DoctorCheck::fail("cache", "cache path has no parent directory");
    };
    if let Err(err) = fs::create_dir_all(dir) {
        return DoctorCheck::fail(
            "cache",
            format!("could not create {}: {err}", dir.display()),
        );
    }

    let probe = dir.join(".doctor-write-test");
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            DoctorCheck::ok("cache", format!("{} is writable", dir.display()))
        }
        Err(err) => DoctorCheck::fail("cache", format!("{} is not writable: {err}", dir.display())),
    }
}

fn check_command_on_path(command: &str) -> DoctorCheck {
    if command_on_path(command) {
        DoctorCheck::ok("zellij", format!("{command} found on PATH"))
    } else {
        DoctorCheck::warn(
            "zellij",
            format!("{command} was not found on PATH; hooks may not be able to pipe attention"),
        )
    }
}

fn command_on_path(command: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        command_candidates(command)
            .iter()
            .any(|c| dir.join(c).is_file())
    })
}

fn command_candidates(command: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        let mut candidates = vec![command.to_string()];
        candidates.extend(
            pathext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| format!("{command}{ext}")),
        );
        candidates
    }
    #[cfg(not(windows))]
    {
        vec![command.to_string()]
    }
}

fn check_optional_dir(name: &'static str, path: &Path) -> DoctorCheck {
    if path.is_dir() {
        DoctorCheck::ok(name, format!("{} exists", path.display()))
    } else {
        DoctorCheck::warn(name, format!("{} was not found", path.display()))
    }
}

fn check_optional_file(name: &'static str, path: &Path) -> DoctorCheck {
    if path.is_file() {
        DoctorCheck::ok(name, format!("{} exists", path.display()))
    } else {
        DoctorCheck::warn(name, format!("{} was not found", path.display()))
    }
}

fn cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("zellij-cockpit").join("usage.json")
}

/// Write the cache via a temp file and a rename, so a reader never sees a
/// half-written one.
///
/// Helpers overlap in practice: every zellij session running the bar spawns one
/// every few seconds, all against this one file. A torn read parses as garbage,
/// which we would treat as "no cache" and answer by refetching - turning a
/// harmless race into extra requests against a rate-limited endpoint. `rename` is
/// atomic within a filesystem, so a reader sees either the old file or the new
/// one. The temp name carries the pid so two writers cannot collide.
fn write_cache_atomically(path: &Path, cache: &CachedUsage) {
    let Some(dir) = path.parent() else { return };
    let _ = fs::create_dir_all(dir);

    let Ok(serialized) = serde_json::to_string(cache) else {
        return;
    };
    let tmp = dir.join(format!("usage.json.{}.tmp", std::process::id()));
    if fs::write(&tmp, serialized).is_ok() && fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Provider usage for this tick: refresh whichever half of the cache is stale,
/// then overlay the real rate-limit window on Claude's estimate.
fn cached_usage(live: bool) -> CachedUsage {
    let path = cache_path();
    let now = now_epoch();

    let mut cache = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<CachedUsage>(&s).ok())
        .unwrap_or_default();

    let mut dirty = false;

    if cache.is_from_the_future(now) {
        cache = CachedUsage::default();
        dirty = true;
    }

    if now - cache.logs_at >= LOGS_TTL {
        cache.claude = claude::current_usage();
        cache.codex = codex::current_usage();
        cache.logs_at = now;
        dirty = true;
    }

    if live {
        // Refetch once the cached window goes stale, once it has actually reset,
        // or if we have never had one - but never before the backoff expires.
        let reset_passed = cache.live_resets_at.is_some_and(|r| now >= r);
        let expired = now - cache.live_at >= LIVE_TTL;
        if (expired || reset_passed) && now >= cache.live_retry_at {
            match claude::live::fetch_session() {
                Some(window) => {
                    cache.live_used_frac = Some(window.used_frac);
                    cache.live_resets_at = Some(window.resets_at);
                    cache.live_at = now;
                    cache.live_retry_at = 0;
                    cache.live_failures = 0;
                }
                None => {
                    cache.live_retry_at = now + backoff_secs(cache.live_failures);
                    cache.live_failures = cache.live_failures.saturating_add(1);
                }
            }
            dirty = true;
        }
    }

    // Persist *before* overlaying. The cache holds what the logs said; the live
    // window is layered on top only for this tick's output. Baking the overlay
    // into the cache would leave the real quota frozen in it, and we would go on
    // presenting a stale number as current long after the live read went bad.
    if dirty {
        write_cache_atomically(&path, &cache);
    }

    // Only overlay a window we still believe: fresh enough, and not already reset.
    // Otherwise Claude keeps the log-based estimate, which the bar renders without
    // a percentage precisely because it is not quota.
    if live
        && let (Some(used_frac), Some(resets_at)) = (cache.live_used_frac, cache.live_resets_at)
        && now - cache.live_at < LIVE_STALE
        && now < resets_at
    {
        let window = claude::live::Window {
            used_frac,
            resets_at,
        };
        claude::apply_live_window(&mut cache.claude, window, now);
    }

    cache
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("zellij-cockpit-{name}-{}", std::process::id()));
        path
    }

    #[test]
    fn writable_cache_check_passes_for_temp_dir() {
        let path = temp_path("cache").join("usage.json");
        let check = check_cache_writable(&path);
        assert_eq!(check.status, CheckStatus::Ok);
    }

    #[test]
    fn optional_file_warns_when_missing() {
        let check = check_optional_file("missing", &temp_path("missing-file"));
        assert_eq!(check.status, CheckStatus::Warn);
    }

    #[test]
    fn command_candidates_include_plain_command() {
        assert!(command_candidates("zellij").iter().any(|c| c == "zellij"));
    }

    #[test]
    fn cache_write_is_atomic_and_leaves_no_temp_file() {
        let dir = temp_path("atomic");
        let path = dir.join("usage.json");
        let cache = CachedUsage {
            live_used_frac: Some(0.42),
            ..Default::default()
        };
        write_cache_atomically(&path, &cache);

        let read: CachedUsage =
            serde_json::from_str(&fs::read_to_string(&path).expect("cache written")).unwrap();
        assert_eq!(read.live_used_frac, Some(0.42));

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file was left behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backoff_doubles_per_failure_and_is_capped() {
        assert_eq!(backoff_secs(0), LIVE_BACKOFF_BASE);
        assert_eq!(backoff_secs(1), LIVE_BACKOFF_BASE * 2);
        assert_eq!(backoff_secs(2), LIVE_BACKOFF_BASE * 4);
        // Never grows without bound, and never overflows however long we fail.
        assert_eq!(backoff_secs(4), LIVE_BACKOFF_MAX);
        assert_eq!(backoff_secs(u32::MAX), LIVE_BACKOFF_MAX);
    }

    #[test]
    fn a_cache_stamped_in_the_future_is_not_trusted() {
        // A backwards clock jump (suspend, NTP correction) would otherwise pin the
        // cache as "fresh" arbitrarily far into the future and freeze the bar.
        let now = 1_000_000;
        assert!(!CachedUsage::default().is_from_the_future(now));
        assert!(
            CachedUsage {
                logs_at: now + 10_000,
                ..Default::default()
            }
            .is_from_the_future(now)
        );
        assert!(
            CachedUsage {
                live_at: now + 10_000,
                ..Default::default()
            }
            .is_from_the_future(now)
        );
    }
}

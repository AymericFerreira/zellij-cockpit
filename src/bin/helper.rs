//! cockpit-helper: collect system + per-provider usage and print one JSON line.
//!
//! Invoked by the plugin on a timer. Short-lived (no daemon, no lock files).
//! CPU needs two `sysinfo` reads ~300ms apart; the per-provider log scans are
//! cached to `~/.cache/zellij-cockpit/usage.json` and recomputed at most every
//! 30s so each tick stays cheap.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sysinfo::System;

use zellij_cockpit::system::{CpuUsage, MemUsage};
use zellij_cockpit::types::{Metrics, ProviderUsage};
use zellij_cockpit::{claude, codex};

const USAGE_CACHE_TTL: Duration = Duration::from_secs(30);

/// What we cache between ticks: both providers' usage (system metrics are always
/// computed fresh since they're cheap).
#[derive(Serialize, Deserialize, Default)]
struct CachedUsage {
    claude: ProviderUsage,
    codex: ProviderUsage,
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("doctor") {
        std::process::exit(run_doctor());
    }

    let mut sys = System::new();

    let mut cpu = CpuUsage::default();
    cpu.sample(&mut sys);
    std::thread::sleep(Duration::from_millis(300));
    cpu.read(&mut sys);

    let mut mem = MemUsage::default();
    mem.update(&mut sys);

    let usage = cached_usage();

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
    let mut checks = Vec::new();

    checks.push(check_current_helper());
    checks.push(check_cache_writable(&cache_path()));
    checks.push(check_command_on_path("zellij"));

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

/// Return cached provider usage if fresh, else recompute and refresh the cache.
fn cached_usage() -> CachedUsage {
    let path = cache_path();

    if let Ok(meta) = fs::metadata(&path) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .map(|age| age < USAGE_CACHE_TTL)
            .unwrap_or(false);
        if fresh
            && let Ok(s) = fs::read_to_string(&path)
            && let Ok(usage) = serde_json::from_str::<CachedUsage>(&s)
        {
            return usage;
        }
    }

    let usage = CachedUsage {
        claude: claude::current_usage(),
        codex: codex::current_usage(),
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(&usage) {
        let _ = fs::write(&path, s);
    }
    usage
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
}

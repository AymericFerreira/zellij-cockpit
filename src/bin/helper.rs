//! cockpit-helper: collect system + per-provider usage and print one JSON line.
//!
//! Invoked by the plugin on a timer. Short-lived (no daemon, no lock files).
//! CPU needs two `sysinfo` reads ~300ms apart; the per-provider log scans are
//! cached to `~/.cache/zellij-cockpit/usage.json` and recomputed at most every
//! 30s so each tick stays cheap.

use std::fs;
use std::path::PathBuf;
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
        claude: usage.claude,
        codex: usage.codex,
    };

    println!("{}", serde_json::to_string(&metrics).unwrap_or_default());
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
        if fresh {
            if let Ok(s) = fs::read_to_string(&path) {
                if let Ok(usage) = serde_json::from_str::<CachedUsage>(&s) {
                    return usage;
                }
            }
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

use sysinfo::System;

/// Physical memory, swap, and (on macOS) memory pressure.
///
/// We read `used_memory()` directly rather than `total - available_memory()`:
/// on macOS, sysinfo's `available_memory()` reports 0, which would make used
/// equal total. `used_memory()` is reliable across platforms.
#[derive(Default)]
pub struct MemUsage {
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// macOS memory pressure, 0..100. `None` on other platforms.
    pub pressure: Option<f32>,
}

impl MemUsage {
    pub fn update(&mut self, sys: &mut System) {
        sys.refresh_memory();
        self.total = sys.total_memory();
        self.used = sys.used_memory();
        self.swap_total = sys.total_swap();
        self.swap_used = sys.used_swap();
        self.pressure = memory_pressure(self.total);
    }
}

/// macOS memory pressure, the number Activity Monitor graphs.
///
/// Used memory is a poor health signal on macOS: the kernel keeps file caches
/// and compressible pages resident, so "used" sits near total on a healthy
/// machine. Pressure instead measures the memory the kernel *cannot* reclaim
/// cheaply - wired pages plus pages the compressor is already holding - as a
/// fraction of physical memory.
#[cfg(target_os = "macos")]
fn memory_pressure(total: u64) -> Option<f32> {
    let out = std::process::Command::new("vm_stat").output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    parse_vm_stat_pressure(&text, total)
}

#[cfg(not(target_os = "macos"))]
fn memory_pressure(_total: u64) -> Option<f32> {
    None
}

/// Parse `vm_stat` output into a pressure percentage.
///
/// The page size comes from the header line, so the arithmetic stays correct on
/// both 4K (Intel) and 16K (Apple silicon) pages.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_vm_stat_pressure(text: &str, total: u64) -> Option<f32> {
    if total == 0 {
        return None;
    }

    let mut page_size: Option<u64> = None;
    let mut wired: Option<u64> = None;
    let mut compressed: Option<u64> = None;

    for line in text.lines() {
        if page_size.is_none()
            && let Some(rest) = line.split("page size of ").nth(1)
        {
            page_size = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok());
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches('.').parse::<u64>().ok();
        match key.trim() {
            "Pages wired down" => wired = value,
            "Pages occupied by compressor" => compressed = value,
            _ => {}
        }
    }

    let unreclaimable = wired?.checked_add(compressed?)? * page_size?;
    Some((unreclaimable as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                                4096.
Pages active:                            300000.
Pages inactive:                          200000.
Pages speculative:                        10000.
Pages throttled:                              0.
Pages wired down:                        200000.
Pages purgeable:                           5000.
\"Translation faults\":                 123456789.
Pages occupied by compressor:            100000.
Swapins:                                      0.
Swapouts:                                     0.
";

    #[test]
    fn mem_update_is_sane() {
        let mut sys = System::new();
        let mut usage = MemUsage::default();
        usage.update(&mut sys);
        assert!(usage.total > 0, "total memory should be nonzero");
        assert!(usage.used > 0, "used memory should be nonzero");
        assert!(usage.used <= usage.total, "used cannot exceed total");
        assert!(usage.swap_used <= usage.swap_total.max(usage.swap_used));
        if let Some(p) = usage.pressure {
            assert!((0.0..=100.0).contains(&p), "pressure out of range: {p}");
        }
    }

    #[test]
    fn parses_pressure_from_vm_stat() {
        // (200000 wired + 100000 compressed) * 16384 = 4.915 GB of 16 GB.
        let pressure = parse_vm_stat_pressure(SAMPLE, 16 * 1024 * 1024 * 1024).unwrap();
        assert!(
            (pressure - 28.6).abs() < 0.2,
            "unexpected pressure: {pressure}"
        );
    }

    #[test]
    fn pressure_is_none_when_fields_are_missing() {
        assert!(parse_vm_stat_pressure("Pages free: 10.", 16_000_000_000).is_none());
        assert!(parse_vm_stat_pressure(SAMPLE, 0).is_none());
    }
}

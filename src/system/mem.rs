use sysinfo::System;

/// Physical memory totals.
///
/// We read `used_memory()` directly rather than `total - available_memory()`:
/// on macOS, sysinfo's `available_memory()` reports 0, which would make used
/// equal total. `used_memory()` is reliable across platforms.
#[derive(Default)]
pub struct MemUsage {
    pub total: u64,
    pub used: u64,
}

impl MemUsage {
    pub fn update(&mut self, sys: &mut System) {
        sys.refresh_memory();
        self.total = sys.total_memory();
        self.used = sys.used_memory();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_update_is_sane() {
        let mut sys = System::new();
        let mut usage = MemUsage::default();
        usage.update(&mut sys);
        assert!(usage.total > 0, "total memory should be nonzero");
        assert!(usage.used > 0, "used memory should be nonzero");
        assert!(usage.used <= usage.total, "used cannot exceed total");
    }
}

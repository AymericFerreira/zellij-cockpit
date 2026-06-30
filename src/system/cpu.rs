use sysinfo::System;

/// Overall CPU utilization. `sysinfo` needs two reads separated by a short
/// interval (~200ms+) to compute a percentage, so call `sample`, sleep, `read`.
#[derive(Default)]
pub struct CpuUsage {
    pub total: f32,
}

impl CpuUsage {
    pub fn sample(&mut self, sys: &mut System) {
        sys.refresh_cpu_all();
    }

    pub fn read(&mut self, sys: &mut System) {
        sys.refresh_cpu_all();
        self.total = sys.global_cpu_usage();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cpu_read_is_a_valid_percentage() {
        let mut sys = System::new();
        let mut usage = CpuUsage::default();
        usage.sample(&mut sys);
        std::thread::sleep(Duration::from_millis(250));
        usage.read(&mut sys);
        assert!(
            (0.0..=100.0).contains(&usage.total),
            "cpu usage {} out of range",
            usage.total
        );
    }
}

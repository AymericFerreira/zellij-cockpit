//! Cross-platform system metrics via `sysinfo`. Native-only.

pub mod cpu;
pub mod mem;

pub use cpu::CpuUsage;
pub use mem::MemUsage;

//! User-facing display configuration shared by the plugin and tests.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Compact,
    Balanced,
    Full,
}

impl Preset {
    fn parse(value: Option<&String>) -> Self {
        match value.map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s == "compact" => Preset::Compact,
            Some(s) if s == "full" => Preset::Full,
            _ => Preset::Balanced,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayConfig {
    pub preset: Preset,
    pub show_cpu: bool,
    pub show_mem: bool,
    pub show_swap: bool,
    /// Show the memory-pressure percentage next to MEM (it always drives the
    /// MEM color, whether or not the number itself is shown).
    pub show_pressure: bool,
    pub show_claude: bool,
    pub show_codex: bool,
    pub show_cost: bool,
    pub show_tokens: bool,
    pub show_window: bool,
    pub show_percent: bool,
    pub show_provider_labels: bool,
    pub glyphs: Glyphs,
    pub thresholds: Thresholds,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self::from_map(&BTreeMap::new())
    }
}

impl DisplayConfig {
    pub fn from_map(config: &BTreeMap<String, String>) -> Self {
        let preset = Preset::parse(config.get("preset"));
        let mut display = match preset {
            Preset::Compact => Self {
                preset,
                show_cpu: true,
                show_mem: true,
                show_swap: true,
                show_pressure: false,
                show_claude: true,
                show_codex: true,
                show_cost: false,
                show_tokens: false,
                show_window: true,
                show_percent: true,
                show_provider_labels: true,
                glyphs: Glyphs::default(),
                thresholds: Thresholds::default(),
            },
            Preset::Balanced => Self {
                preset,
                show_cpu: true,
                show_mem: true,
                show_swap: true,
                show_pressure: false,
                show_claude: true,
                show_codex: true,
                show_cost: true,
                show_tokens: true,
                show_window: true,
                show_percent: true,
                show_provider_labels: true,
                glyphs: Glyphs::default(),
                thresholds: Thresholds::default(),
            },
            Preset::Full => Self {
                preset,
                show_cpu: true,
                show_mem: true,
                show_swap: true,
                show_pressure: true,
                show_claude: true,
                show_codex: true,
                show_cost: true,
                show_tokens: true,
                show_window: true,
                show_percent: true,
                show_provider_labels: true,
                glyphs: Glyphs::default(),
                thresholds: Thresholds::default(),
            },
        };

        display.show_cpu = bool_key(config, "cpu", display.show_cpu);
        display.show_mem = bool_key(config, "mem", display.show_mem);
        display.show_swap = bool_key(config, "swap", display.show_swap);
        display.show_pressure = bool_key(config, "pressure", display.show_pressure);
        display.show_claude = bool_key(config, "claude", display.show_claude);
        display.show_codex = bool_key(config, "codex", display.show_codex);
        display.show_cost = bool_key(config, "cost", display.show_cost);
        display.show_tokens = bool_key(config, "tokens", display.show_tokens);
        display.show_window = bool_key(config, "window", display.show_window);
        display.show_percent = bool_key(config, "percent", display.show_percent);
        display.show_provider_labels =
            bool_key(config, "provider_labels", display.show_provider_labels);

        let ascii = bool_key(config, "ascii", false);
        display.glyphs = Glyphs::from_map(config, ascii);
        display.thresholds = Thresholds::from_map(config);
        display
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyphs {
    pub working: String,
    pub waiting: String,
    pub done: String,
}

impl Default for Glyphs {
    fn default() -> Self {
        Self {
            working: "◐".to_string(),
            waiting: "●".to_string(),
            done: "✓".to_string(),
        }
    }
}

impl Glyphs {
    fn from_map(config: &BTreeMap<String, String>, ascii: bool) -> Self {
        let defaults = if ascii {
            Self {
                working: "~".to_string(),
                waiting: "!".to_string(),
                done: "+".to_string(),
            }
        } else {
            Self::default()
        };
        Self {
            working: config
                .get("glyph_working")
                .cloned()
                .unwrap_or(defaults.working),
            waiting: config
                .get("glyph_waiting")
                .cloned()
                .unwrap_or(defaults.waiting),
            done: config.get("glyph_done").cloned().unwrap_or(defaults.done),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Thresholds {
    pub cpu_warn: f64,
    pub cpu_crit: f64,
    pub mem_warn: f64,
    pub mem_crit: f64,
    /// Memory-pressure percentages (macOS). Apple turns Activity Monitor's graph
    /// yellow well before the machine is in trouble, so these sit lower than the
    /// used/total thresholds they replace.
    pub pressure_warn: f64,
    pub pressure_crit: f64,
    /// Swap is graded in absolute gigabytes: on macOS the swap file grows on
    /// demand, so "percent of total swap" says nothing useful.
    pub swap_warn_gb: f64,
    pub swap_crit_gb: f64,
    pub window_warn: f64,
    pub window_crit: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            cpu_warn: 50.0,
            cpu_crit: 80.0,
            mem_warn: 60.0,
            mem_crit: 80.0,
            pressure_warn: 50.0,
            pressure_crit: 75.0,
            swap_warn_gb: 1.0,
            swap_crit_gb: 4.0,
            window_warn: 60.0,
            window_crit: 80.0,
        }
    }
}

impl Thresholds {
    fn from_map(config: &BTreeMap<String, String>) -> Self {
        let defaults = Self::default();
        Self {
            cpu_warn: f64_key(config, "cpu_warn", defaults.cpu_warn),
            cpu_crit: f64_key(config, "cpu_crit", defaults.cpu_crit),
            mem_warn: f64_key(config, "mem_warn", defaults.mem_warn),
            mem_crit: f64_key(config, "mem_crit", defaults.mem_crit),
            pressure_warn: f64_key(config, "pressure_warn", defaults.pressure_warn),
            pressure_crit: f64_key(config, "pressure_crit", defaults.pressure_crit),
            swap_warn_gb: f64_key(config, "swap_warn_gb", defaults.swap_warn_gb),
            swap_crit_gb: f64_key(config, "swap_crit_gb", defaults.swap_crit_gb),
            window_warn: f64_key(config, "window_warn", defaults.window_warn),
            window_crit: f64_key(config, "window_crit", defaults.window_crit),
        }
    }
}

fn bool_key(config: &BTreeMap<String, String>, key: &str, default: bool) -> bool {
    config
        .get(key)
        .and_then(|s| parse_bool(s))
        .unwrap_or(default)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" | "" => Some(false),
        _ => None,
    }
}

fn f64_key(config: &BTreeMap<String, String>, key: &str, default: f64) -> f64 {
    config
        .get(key)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_to_balanced() {
        let cfg = DisplayConfig::default();
        assert_eq!(cfg.preset, Preset::Balanced);
        assert!(cfg.show_cpu);
        assert!(cfg.show_mem);
        assert!(cfg.show_cost);
        assert!(cfg.show_tokens);
        assert!(cfg.show_window);
        assert!(cfg.show_percent);
    }

    #[test]
    fn compact_hides_cost_and_tokens() {
        let cfg = DisplayConfig::from_map(&map(&[("preset", "compact")]));
        assert_eq!(cfg.preset, Preset::Compact);
        assert!(!cfg.show_cost);
        assert!(!cfg.show_tokens);
        assert!(cfg.show_window);
        assert!(cfg.show_provider_labels);
    }

    #[test]
    fn explicit_values_override_presets() {
        let cfg = DisplayConfig::from_map(&map(&[
            ("preset", "compact"),
            ("cost", "yes"),
            ("tokens", "1"),
            ("cpu", "off"),
        ]));
        assert!(cfg.show_cost);
        assert!(cfg.show_tokens);
        assert!(!cfg.show_cpu);
    }

    #[test]
    fn ascii_changes_default_glyphs_but_not_explicit_glyphs() {
        let cfg = DisplayConfig::from_map(&map(&[("ascii", "true"), ("glyph_waiting", "WAIT")]));
        assert_eq!(cfg.glyphs.working, "~");
        assert_eq!(cfg.glyphs.waiting, "WAIT");
        assert_eq!(cfg.glyphs.done, "+");
    }

    #[test]
    fn only_full_shows_the_pressure_number_but_swap_is_always_on() {
        for (preset, wants_pressure) in [("compact", false), ("balanced", false), ("full", true)] {
            let cfg = DisplayConfig::from_map(&map(&[("preset", preset)]));
            assert!(cfg.show_swap, "{preset} should show swap");
            assert_eq!(cfg.show_pressure, wants_pressure, "preset {preset}");
        }
    }

    #[test]
    fn swap_and_pressure_toggles_override_presets() {
        let cfg = DisplayConfig::from_map(&map(&[
            ("preset", "full"),
            ("swap", "off"),
            ("pressure", "no"),
        ]));
        assert!(!cfg.show_swap);
        assert!(!cfg.show_pressure);
    }

    #[test]
    fn parses_pressure_and_swap_thresholds() {
        let cfg = DisplayConfig::from_map(&map(&[("pressure_warn", "40"), ("swap_crit_gb", "8")]));
        assert_eq!(cfg.thresholds.pressure_warn, 40.0);
        assert_eq!(cfg.thresholds.pressure_crit, 75.0);
        assert_eq!(cfg.thresholds.swap_crit_gb, 8.0);
        assert_eq!(cfg.thresholds.swap_warn_gb, 1.0);
    }

    #[test]
    fn parses_thresholds() {
        let cfg = DisplayConfig::from_map(&map(&[("cpu_warn", "40"), ("window_crit", "90.5")]));
        assert_eq!(cfg.thresholds.cpu_warn, 40.0);
        assert_eq!(cfg.thresholds.window_crit, 90.5);
        assert_eq!(cfg.thresholds.mem_warn, 60.0);
    }
}

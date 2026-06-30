//! zellij-cockpit WASM plugin: renders the full top bar.
//!
//! Layout (one row): `<tabs + attention icons>  …pad…  <CPU  MEM  Claude  5h>`
//!
//! Data flows:
//!   * metrics (pull): a Timer fires `cockpit-helper` via run_command; its JSON
//!     stdout arrives as RunCommandResult and is rendered.
//!   * attention (push): Claude Code hooks send `zellij pipe --name
//!     "cockpit::attention::<state>::$ZELLIJ_PANE_ID"`; the name is parsed here,
//!     the pane is mapped to its tab, and that tab gets an icon until focused.

use std::collections::BTreeMap;

use colored::Colorize;
use zellij_tile::prelude::*;

use zellij_cockpit::types::Metrics;

/// Per-tab attention state, set by Claude Code hooks.
#[derive(Clone, Copy)]
enum Attn {
    Working,
    Waiting,
    Done,
}

impl Attn {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "working" => Some(Attn::Working),
            "waiting" => Some(Attn::Waiting),
            "done" => Some(Attn::Done),
            _ => None,
        }
    }

    fn glyph(self) -> String {
        match self {
            Attn::Working => format!("{}", "◐".yellow()),
            Attn::Waiting => format!("{}", "●".bright_red()),
            Attn::Done => format!("{}", "✓".bright_green()),
        }
    }
}

#[derive(Default)]
struct State {
    metrics: Metrics,
    tabs: Vec<TabInfo>,
    /// tab position -> attention state.
    attention: BTreeMap<usize, Attn>,
    /// terminal pane id -> tab position (from PaneManifest).
    pane_to_tab: BTreeMap<u32, usize>,
    interval: f64,
    /// sh -c argument that runs the helper; default resolves $HOME at runtime.
    helper_cmd: String,
    got_perms: bool,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        colored::control::set_override(true);

        self.interval = configuration
            .get("interval")
            .and_then(|s| s.parse().ok())
            .unwrap_or(3.0);

        let helper_path = configuration
            .get("helper")
            .cloned()
            .unwrap_or_else(|| "$HOME/.config/zellij/plugins/cockpit-helper".to_string());
        self.helper_cmd = format!("exec \"{helper_path}\"");

        request_permission(&[PermissionType::RunCommands]);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::Timer,
            EventType::RunCommandResult,
            EventType::TabUpdate,
            EventType::PaneUpdate,
        ]);
        set_selectable(false);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(_) => {
                self.got_perms = true;
                self.fetch();
                set_timeout(self.interval);
                false
            }
            Event::Timer(_) => {
                self.fetch();
                set_timeout(self.interval);
                false
            }
            Event::RunCommandResult(_code, stdout, _stderr, _ctx) => {
                if let Ok(text) = String::from_utf8(stdout) {
                    if let Ok(metrics) = serde_json::from_str::<Metrics>(text.trim()) {
                        self.metrics = metrics;
                        return true;
                    }
                }
                false
            }
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                // Clear attention on the focused tab — if you're looking at it,
                // it no longer needs to flag for your attention.
                if let Some(active) = self.tabs.iter().find(|t| t.active) {
                    self.attention.remove(&active.position);
                }
                true
            }
            Event::PaneUpdate(manifest) => {
                self.pane_to_tab.clear();
                for (tab_pos, panes) in manifest.panes {
                    for pane in panes {
                        if !pane.is_plugin {
                            self.pane_to_tab.insert(pane.id, tab_pos);
                        }
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        // Hooks broadcast via `zellij pipe --name`, so the payload is the name.
        let Some(rest) = message.name.strip_prefix("cockpit::attention::") else {
            return false;
        };
        let mut parts = rest.split("::");
        let (Some(state), Some(pane_id)) = (parts.next(), parts.next()) else {
            return false;
        };
        let (Some(attn), Ok(pane_id)) = (Attn::from_str(state), pane_id.trim().parse::<u32>())
        else {
            return false;
        };
        let Some(&tab) = self.pane_to_tab.get(&pane_id) else {
            return false;
        };

        // Don't flag a tab you're already looking at.
        let is_active = self
            .tabs
            .iter()
            .find(|t| t.position == tab)
            .map(|t| t.active)
            .unwrap_or(false);
        if is_active {
            return false;
        }

        self.attention.insert(tab, attn);
        true
    }

    fn render(&mut self, _rows: usize, cols: usize) {
        if cols == 0 {
            return;
        }

        let left = self.render_tabs();
        let left_w = visible_len(&left);
        let sep = format!(" {} ", "|".bright_black());

        // Fit the right-hand metrics, dropping segments right-to-left when tight.
        let mut segments = self.metric_segments();
        loop {
            if segments.is_empty() {
                print!("{left}");
                return;
            }
            let right = segments.join(&sep);
            let right_w = visible_len(&right);
            if left_w + 1 + right_w <= cols {
                let pad = cols.saturating_sub(left_w + right_w);
                print!("{left}{}{right}", " ".repeat(pad));
                return;
            }
            segments.pop();
        }
    }
}

impl State {
    fn fetch(&self) {
        if !self.got_perms {
            return;
        }
        run_command(&["sh", "-c", &self.helper_cmd], BTreeMap::new());
    }

    fn render_tabs(&self) -> String {
        let parts: Vec<String> = self
            .tabs
            .iter()
            .map(|tab| {
                let label = format!("{} {}", tab.position + 1, tab.name);
                let styled = if tab.active {
                    format!("{}", label.bold().bright_green())
                } else {
                    format!("{}", label.bright_black())
                };
                match self.attention.get(&tab.position) {
                    Some(a) => format!("{styled} {}", a.glyph()),
                    None => styled,
                }
            })
            .collect();
        parts.join("  ")
    }

    fn metric_segments(&self) -> Vec<String> {
        let m = &self.metrics;
        let mut segs = Vec::new();

        // CPU
        let cpu = m.cpu as f64;
        segs.push(format!(
            "{} {}",
            "CPU".bright_black(),
            usage_color(format!("{cpu:.0}%"), cpu, 50.0, 80.0)
        ));

        // MEM
        let used_g = m.mem_used as f64 / 1e9;
        let total_g = m.mem_total as f64 / 1e9;
        let mem_pct = if m.mem_total > 0 {
            m.mem_used as f64 / m.mem_total as f64 * 100.0
        } else {
            0.0
        };
        segs.push(format!(
            "{} {}",
            "MEM".bright_black(),
            usage_color(format!("{used_g:.1}/{total_g:.0}G"), mem_pct, 60.0, 80.0)
        ));

        // Claude today
        let c = &m.claude;
        segs.push(format!(
            "{} {} · {}",
            "Claude".bright_black(),
            format!("${:.2}", c.today_cost).bright_green(),
            human_tokens(c.today_tokens)
        ));

        // 5-hour block
        if c.block_active {
            let pct = c.block_elapsed_frac * 100.0;
            segs.push(format!(
                "{} {} {:.0}%",
                "5h".bright_black(),
                usage_color(bar5(c.block_elapsed_frac), pct, 60.0, 85.0),
                pct
            ));
        }

        segs
    }
}

/// Count visible columns, skipping ANSI escape sequences (`\x1b[ … letter`).
fn visible_len(s: &str) -> usize {
    let mut count = 0usize;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            count += 1;
        }
    }
    count
}

fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// A 5-cell progress bar for a 0..1 fraction.
fn bar5(frac: f64) -> String {
    let filled = (frac.clamp(0.0, 1.0) * 5.0).round() as usize;
    (0..5)
        .map(|i| if i < filled { '▓' } else { '░' })
        .collect()
}

fn usage_color(text: String, pct: f64, warn: f64, crit: f64) -> colored::ColoredString {
    if pct >= crit {
        text.bright_red()
    } else if pct >= warn {
        text.yellow()
    } else {
        text.bright_green()
    }
}

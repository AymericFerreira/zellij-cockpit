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

use zellij_cockpit::types::{Metrics, ProviderUsage};

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
    /// Which providers' usage to display (config: `claude` / `codex`).
    show_claude: bool,
    show_codex: bool,
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

        // Per-provider toggles (default on). Set `claude false` / `codex false`
        // in the plugin's KDL config to hide one. Only explicit false-y spellings
        // hide a provider; anything else (or absent) stays on.
        let flag = |key: &str| {
            configuration
                .get(key)
                .map(|s| {
                    !matches!(
                        s.trim().to_ascii_lowercase().as_str(),
                        "false" | "0" | "no" | "off" | ""
                    )
                })
                .unwrap_or(true)
        };
        self.show_claude = flag("claude");
        self.show_codex = flag("codex");

        // RunCommands: spawn the helper. ReadApplicationState: receive
        // TabUpdate/PaneUpdate (without it we never learn the tabs or which
        // pane is in which tab, so tab names and attention mapping break).
        request_permission(&[
            PermissionType::RunCommands,
            PermissionType::ReadApplicationState,
        ]);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::Timer,
            EventType::RunCommandResult,
            EventType::TabUpdate,
            EventType::PaneUpdate,
        ]);
        // NOTE: do not call set_selectable(false) here. The permission prompt is
        // answered by focusing this pane and pressing `y`; a non-selectable pane
        // can't be focused. We mark it non-selectable only after the grant.
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.got_perms = true;
                // Now that the grant is handled, drop out of the focus rotation
                // so the 1-row bar doesn't steal focus during normal use.
                set_selectable(false);
                self.fetch();
                set_timeout(self.interval);
                false
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.got_perms = false;
                set_selectable(false);
                false
            }
            Event::Timer(_) => {
                self.fetch();
                set_timeout(self.interval);
                false
            }
            Event::RunCommandResult(_code, stdout, _stderr, _ctx) => {
                if let Ok(text) = String::from_utf8(stdout)
                    && let Ok(metrics) = serde_json::from_str::<Metrics>(text.trim())
                {
                    self.metrics = metrics;
                    return true;
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
        // Use an absolute interpreter path: zellij spawns the command with
        // tokio::process::Command::new(argv[0]), and a bare "sh" can ENOENT
        // depending on how zellij itself was launched. $HOME is inherited from
        // the zellij server's env, so it still expands inside the shell.
        run_command(&["/bin/sh", "-c", &self.helper_cmd], BTreeMap::new());
    }

    fn render_tabs(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for tab in &self.tabs {
            if tab.active {
                // Active tab: a clear highlighted chip. Attention is auto-cleared
                // on the focused tab, so the active tab never carries an icon.
                parts.push(format!(
                    "{}",
                    format!(" {} ", tab.name).black().on_green().bold()
                ));
            } else if let Some(a) = self.attention.get(&tab.position) {
                parts.push(format!(" {} {} ", tab.name.bold(), a.glyph()));
            } else {
                parts.push(format!(" {} ", tab.name.bold()));
            }
        }
        parts.join("")
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

        // Per-provider usage (each only if enabled and it actually has data).
        if self.show_claude {
            segs.extend(provider_segments("Claude", &m.claude));
        }
        if self.show_codex {
            segs.extend(provider_segments("Codex", &m.codex));
        }

        segs
    }
}

/// Build the segments for one provider: `<label> $cost · tokens` and, if a
/// window is active, `5h <bar> <time> left`.
fn provider_segments(label: &str, u: &ProviderUsage) -> Vec<String> {
    let mut segs = Vec::new();
    if !u.present {
        return segs;
    }

    segs.push(format!(
        "{} {} · {}",
        label.bright_black(),
        format!("${:.2}", u.today_cost).bright_green(),
        human_tokens(u.today_tokens)
    ));

    // Bar + percent show how full the window is (time elapsed for Claude, real
    // quota used for Codex); the text shows time until it resets. The percent is
    // color-coded so it's obvious when you're close to the limit (red ≥ 80%).
    if u.block_active {
        let pct = u.block_elapsed_frac * 100.0;
        segs.push(format!(
            "{} {} {} {}",
            "5h".bright_black(),
            usage_color(bar5(u.block_elapsed_frac), pct, 60.0, 80.0),
            usage_color(format!("{pct:.0}%"), pct, 60.0, 80.0),
            format!("{} left", human_duration(u.block_remaining_min)).bright_black()
        ));
    }

    segs
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

/// Format minutes as "Xh Ym" / "Ym" for the time-until-reset display.
fn human_duration(minutes: f64) -> String {
    let total = minutes.max(0.0).round() as u64;
    let h = total / 60;
    let m = total % 60;
    if h > 0 {
        format!("{h}h{m:02}m")
    } else {
        format!("{m}m")
    }
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
    (0..5).map(|i| if i < filled { '▓' } else { '░' }).collect()
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

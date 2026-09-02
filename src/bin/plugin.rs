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
//!   * activity (push): shell hooks send `zellij pipe --name
//!     "cockpit::activity::<start|end>::$ZELLIJ_PANE_ID::<era>::<seq>"` around
//!     every foreground command, so a tab can say "a command is running here".

use std::collections::BTreeMap;

use colored::Colorize;
use zellij_tile::prelude::*;

use zellij_cockpit::activity::{self, Tracker};
use zellij_cockpit::bar::{Attn, TabView, render_tabs, visible_len};
use zellij_cockpit::config::DisplayConfig;
use zellij_cockpit::types::{Metrics, ProviderUsage};

#[derive(Default)]
struct State {
    metrics: Metrics,
    tabs: Vec<TabInfo>,
    /// tab position -> attention state.
    attention: BTreeMap<usize, Attn>,
    /// terminal pane id -> tab position (from PaneManifest).
    pane_to_tab: BTreeMap<u32, usize>,
    /// Which panes are running a foreground shell command.
    activity: Tracker,
    interval: f64,
    /// sh -c argument that runs the helper; default resolves $HOME at runtime.
    helper_cmd: String,
    got_perms: bool,
    display: DisplayConfig,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        colored::control::set_override(true);

        self.interval = configuration
            .get("interval")
            .and_then(|s| s.parse().ok())
            .unwrap_or(3.0);

        self.display = DisplayConfig::from_map(&configuration);

        let helper_path = configuration
            .get("helper")
            .cloned()
            .unwrap_or_else(|| "$HOME/.config/zellij/plugins/cockpit-helper".to_string());
        // `live "false"` has to reach the helper: it is the helper, not the
        // plugin, that reads credentials and talks to the network.
        let offline = if self.display.live { "" } else { " --no-live" };
        self.helper_cmd = format!("exec \"{helper_path}\"{offline}");

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
                // A command still running one tick later is worth showing.
                self.activity.tick()
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
                // A pane that closed mid-command must not leave its tab marked.
                let known = &self.pane_to_tab;
                self.activity.retain_panes(|pane| known.contains_key(&pane));
                false
            }
            _ => false,
        }
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        if let Some(rest) = message.name.strip_prefix("cockpit::activity::") {
            return self.activity_pipe(rest);
        }
        // Hooks broadcast via `zellij pipe --name`, so the payload is the name.
        let Some(rest) = message.name.strip_prefix("cockpit::attention::") else {
            return false;
        };
        let mut parts = rest.split("::");
        let (Some(state), Some(pane_id)) = (parts.next(), parts.next()) else {
            return false;
        };
        let (Some(attn), Ok(pane_id)) = (Attn::parse(state), pane_id.trim().parse::<u32>()) else {
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
    /// Hand a `cockpit::activity::…` message to the tracker.
    fn activity_pipe(&mut self, rest: &str) -> bool {
        match activity::Message::parse(rest) {
            Some(message) => self.activity.apply(message),
            None => false,
        }
    }

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
        let tabs: Vec<TabView> = self
            .tabs
            .iter()
            .map(|tab| TabView {
                position: tab.position,
                name: tab.name.clone(),
                active: tab.active,
            })
            .collect();
        render_tabs(
            &tabs,
            &self.attention,
            &self.activity,
            &self.pane_to_tab,
            &self.display,
        )
    }

    fn metric_segments(&self) -> Vec<String> {
        let m = &self.metrics;
        let mut segs = Vec::new();

        let thresholds = &self.display.thresholds;

        if self.display.show_cpu {
            let cpu = m.cpu as f64;
            segs.push(format!(
                "{} {}",
                "CPU".bright_black(),
                usage_color(
                    format!("{cpu:.0}%"),
                    cpu,
                    thresholds.cpu_warn,
                    thresholds.cpu_crit
                )
            ));
        }

        if self.display.show_mem {
            let used_g = m.mem_used as f64 / 1e9;
            let total_g = m.mem_total as f64 / 1e9;
            let used_pct = if m.mem_total > 0 {
                m.mem_used as f64 / m.mem_total as f64 * 100.0
            } else {
                0.0
            };

            // Where pressure is available (macOS) it decides the color: used
            // memory sits near total on a healthy Mac because the kernel keeps
            // reclaimable pages resident, so coloring by used/total would show
            // red all day. Elsewhere, used/total is the best signal we have.
            let (level, warn, crit) = match m.mem_pressure {
                Some(p) => (p as f64, thresholds.pressure_warn, thresholds.pressure_crit),
                None => (used_pct, thresholds.mem_warn, thresholds.mem_crit),
            };

            let mut mem_parts = vec![
                format!("{}", "MEM".bright_black()),
                format!(
                    "{}",
                    usage_color(format!("{used_g:.1}/{total_g:.0}G"), level, warn, crit)
                ),
            ];
            // Pressure gets the bar+percent idiom the rate-limit window uses, so
            // it reads as its own quantity: a bare "66%" next to "15.5/19G" would
            // look like the used-memory percentage, which is the exact confusion
            // pressure exists to clear up.
            if self.display.show_pressure
                && let Some(p) = m.mem_pressure
            {
                let frac = f64::from(p) / 100.0;
                mem_parts.push(format!("{}", usage_color(bar5(frac), level, warn, crit)));
                mem_parts.push(format!(
                    "{}",
                    usage_color(format!("{p:.0}%"), level, warn, crit)
                ));
            }
            segs.push(mem_parts.join(" "));
        }

        // Swap is the signal that memory is actually hurting: a Mac happily runs
        // with memory "full", but sustained swap means paging to disk.
        if self.display.show_swap && m.swap_total > 0 {
            let used_g = m.swap_used as f64 / 1e9;
            segs.push(format!(
                "{} {}",
                "SWAP".bright_black(),
                usage_color(
                    human_bytes(m.swap_used),
                    used_g,
                    thresholds.swap_warn_gb,
                    thresholds.swap_crit_gb
                )
            ));
        }

        // Per-provider usage (each only if enabled and it actually has data).
        if self.display.show_claude {
            segs.extend(provider_segments("Claude", &m.claude, &self.display));
        }
        if self.display.show_codex {
            segs.extend(provider_segments("Codex", &m.codex, &self.display));
        }

        segs
    }
}

/// Build the segments for one provider from the configured display toggles.
fn provider_segments(label: &str, u: &ProviderUsage, display: &DisplayConfig) -> Vec<String> {
    let mut segs = Vec::new();
    if !u.present {
        return segs;
    }

    let mut usage_parts = Vec::new();
    if display.show_provider_labels {
        usage_parts.push(format!("{}", label.bright_black()));
    }
    if display.show_cost {
        usage_parts.push(format!(
            "{}",
            format!("${:.2}", u.today_cost).bright_green()
        ));
    }
    if display.show_tokens {
        usage_parts.push(human_tokens(u.today_tokens));
    }
    let has_usage_segment = usage_parts.len() > usize::from(display.show_provider_labels);
    if has_usage_segment {
        segs.push(usage_parts.join(" · "));
    }

    // The rate-limit window. When the provider tells us the real quota used, the
    // bar and percent mean exactly that, color-coded so it's obvious when you're
    // close to the limit.
    //
    // When it doesn't, all we know locally is how much of the window has
    // *elapsed* - which says nothing about how much quota is left. So we show no
    // percentage at all and dim the bar: an estimate rendered like a quota reads
    // as "72% used" when the truth might be 12%. The reset time stays, because
    // that part of the estimate is sound.
    if display.show_window && u.block_active {
        let thresholds = &display.thresholds;
        let pct = u.block_elapsed_frac * 100.0;
        let mut window_parts = Vec::new();
        if display.show_provider_labels && !has_usage_segment {
            window_parts.push(format!("{}", label.bright_black()));
        }
        window_parts.push(format!("{}", "5h".bright_black()));

        if u.block_is_quota {
            window_parts.push(format!(
                "{}",
                usage_color(
                    bar5(u.block_elapsed_frac),
                    pct,
                    thresholds.window_warn,
                    thresholds.window_crit
                )
            ));
            if display.show_percent {
                window_parts.push(format!(
                    "{}",
                    usage_color(
                        format!("{pct:.0}%"),
                        pct,
                        thresholds.window_warn,
                        thresholds.window_crit
                    )
                ));
            }
        } else {
            window_parts.push(format!("{}", bar5(u.block_elapsed_frac).bright_black()));
        }

        window_parts.push(format!(
            "{}",
            format!("{} left", human_duration(u.block_remaining_min)).bright_black()
        ));
        segs.push(window_parts.join(" "));
    }

    segs
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

/// Byte counts for the swap segment: "0", "512M", "4.2G".
fn human_bytes(n: u64) -> String {
    let g = n as f64 / 1e9;
    if g >= 1.0 {
        format!("{g:.1}G")
    } else if n >= 1_000_000 {
        format!("{:.0}M", n as f64 / 1e6)
    } else if n == 0 {
        "0".to_string()
    } else {
        format!("{:.0}K", n as f64 / 1e3)
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

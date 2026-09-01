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

use zellij_cockpit::config::{DisplayConfig, Glyphs};
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

    fn glyph(self, glyphs: &Glyphs) -> String {
        match self {
            Attn::Working => format!("{}", glyphs.working.yellow()),
            Attn::Waiting => format!("{}", glyphs.waiting.bright_red()),
            Attn::Done => format!("{}", glyphs.done.bright_green()),
        }
    }
}

/// A pane running a foreground shell command, as reported by the shell hooks.
#[derive(Clone, Copy)]
struct Activity {
    /// Which shell is talking: its start time. A pane has one shell at a time,
    /// so a different era simply means a new shell, and whatever it says is the
    /// truth - we never compare eras. Comparing them would let one bad value
    /// (a clock jump, a stray test message) silence the pane for good.
    era: u64,
    /// That shell's command counter. The hooks fire in the background, so pipes
    /// from the same shell can arrive out of order; an older seq is dropped.
    seq: u64,
    running: bool,
    /// Only shown once it has survived a Timer tick. Commands that finish
    /// inside one tick never draw, so the bar doesn't flicker on every `ls`.
    shown: bool,
}

#[derive(Default)]
struct State {
    metrics: Metrics,
    tabs: Vec<TabInfo>,
    /// tab position -> attention state.
    attention: BTreeMap<usize, Attn>,
    /// terminal pane id -> tab position (from PaneManifest).
    pane_to_tab: BTreeMap<u32, usize>,
    /// terminal pane id -> foreground-command state, set by the shell hooks.
    activity: BTreeMap<u32, Activity>,
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
                let mut changed = false;
                for act in self.activity.values_mut() {
                    if act.running && !act.shown {
                        act.shown = true;
                        changed = true;
                    }
                }
                changed
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
                self.activity
                    .retain(|id, _| self.pane_to_tab.contains_key(id));
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
    /// Handle `<start|end>::<pane_id>::<era>::<seq>` from the shell hooks, or
    /// `reset`, which drops every marker (an escape hatch for a stuck bar).
    fn activity_pipe(&mut self, rest: &str) -> bool {
        if rest.trim() == "reset" {
            let had_marker = self.activity.values().any(|a| a.running && a.shown);
            self.activity.clear();
            return had_marker;
        }

        let mut parts = rest.split("::");
        let (Some(state), Some(pane_id), Some(era), Some(seq)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        let (Ok(pane_id), Ok(era), Ok(seq)) = (
            pane_id.trim().parse::<u32>(),
            era.trim().parse::<u64>(),
            seq.trim().parse::<u64>(),
        ) else {
            return false;
        };
        let running = match state {
            "start" => true,
            "end" => false,
            _ => return false,
        };

        // Within one shell, a late pipe from an older command must not
        // resurrect it. Across shells, the newcomer always wins.
        let previous = self.activity.get(&pane_id).copied();
        if previous.is_some_and(|p| p.era == era && seq < p.seq) {
            return false;
        }
        let was_shown = previous.is_some_and(|p| p.running && p.shown);
        self.activity.insert(
            pane_id,
            Activity {
                era,
                seq,
                running,
                shown: false,
            },
        );
        // Starting draws nothing yet (the Timer decides); ending has to repaint
        // if the marker was up.
        was_shown && !running
    }

    /// Is a command running in any pane of this tab?
    fn tab_is_busy(&self, tab: usize) -> bool {
        self.activity
            .iter()
            .any(|(pane, act)| act.running && act.shown && self.pane_to_tab.get(pane) == Some(&tab))
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
        let mut parts: Vec<String> = Vec::new();
        for tab in &self.tabs {
            // Unlike attention, a running command still matters on the tab you
            // are looking at: it says the command has not finished yet.
            let busy = self.display.show_activity && self.tab_is_busy(tab.position);
            let running = &self.display.glyphs.running;
            if tab.active {
                // Active tab: a clear highlighted chip. Attention is auto-cleared
                // on the focused tab, so the active tab never carries an icon.
                let label = if busy {
                    format!(" {} {running} ", tab.name)
                } else {
                    format!(" {} ", tab.name)
                };
                parts.push(format!("{}", label.black().on_green().bold()));
            } else {
                let mut chip = format!(" {}", tab.name.bold());
                if busy {
                    chip.push_str(&format!(" {}", running.bright_cyan()));
                }
                if let Some(attn) = self.attention.get(&tab.position) {
                    chip.push_str(&format!(" {}", attn.glyph(&self.display.glyphs)));
                }
                chip.push(' ');
                parts.push(chip);
            }
        }
        parts.join("")
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

//! Drawing the left-hand side of the bar: tab names and their markers.
//!
//! Kept free of `zellij_tile` so it builds and tests natively. The plugin maps
//! zellij's `TabInfo` onto [`TabView`] and hands the result here.

use std::collections::BTreeMap;

use colored::Colorize;

use crate::activity::Tracker;
use crate::config::{DisplayConfig, Glyphs};

/// What the bar needs to know about one tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabView {
    pub position: usize,
    pub name: String,
    pub active: bool,
}

/// Per-tab attention state, set by Claude Code hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attn {
    Working,
    Waiting,
    Done,
}

impl Attn {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "working" => Some(Attn::Working),
            "waiting" => Some(Attn::Waiting),
            "done" => Some(Attn::Done),
            _ => None,
        }
    }

    pub fn glyph(self, glyphs: &Glyphs) -> String {
        match self {
            Attn::Working => format!("{}", glyphs.working.yellow()),
            Attn::Waiting => format!("{}", glyphs.waiting.bright_red()),
            Attn::Done => format!("{}", glyphs.done.bright_green()),
        }
    }
}

/// Render the tab strip.
///
/// Attention is cleared on the focused tab elsewhere, so the active tab never
/// carries an attention icon. The running-command marker is different: it still
/// matters on the tab you are looking at, because it says the command has not
/// finished yet.
pub fn render_tabs(
    tabs: &[TabView],
    attention: &BTreeMap<usize, Attn>,
    activity: &Tracker,
    pane_to_tab: &BTreeMap<u32, usize>,
    display: &DisplayConfig,
) -> String {
    let running = &display.glyphs.running;
    let mut parts: Vec<String> = Vec::new();
    for tab in tabs {
        let busy = display.show_activity && activity.tab_is_busy(tab.position, pane_to_tab);
        if tab.active {
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
            if let Some(attn) = attention.get(&tab.position) {
                chip.push_str(&format!(" {}", attn.glyph(&display.glyphs)));
            }
            chip.push(' ');
            parts.push(chip);
        }
    }
    parts.join("")
}

/// Count visible columns, skipping ANSI escape sequences (`\x1b[ … letter`).
pub fn visible_len(s: &str) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Message;

    /// Render without colors, so a test reads the text the way a user sees it.
    fn plain(
        tabs: &[TabView],
        attention: &BTreeMap<usize, Attn>,
        activity: &Tracker,
        display: &DisplayConfig,
    ) -> String {
        colored::control::set_override(false);
        render_tabs(tabs, attention, activity, &panes(), display)
    }

    /// Pane 7 lives on tab 0, pane 8 on tab 1.
    fn panes() -> BTreeMap<u32, usize> {
        BTreeMap::from([(7, 0), (8, 1)])
    }

    fn tabs() -> Vec<TabView> {
        vec![
            TabView {
                position: 0,
                name: "edit".into(),
                active: false,
            },
            TabView {
                position: 1,
                name: "build".into(),
                active: true,
            },
        ]
    }

    /// A tracker with a command drawn in `pane`.
    fn running_in(pane: u32) -> Tracker {
        let mut tracker = Tracker::default();
        tracker.apply(Message::Command {
            pane,
            era: 100,
            seq: 1,
            running: true,
        });
        tracker.tick();
        tracker
    }

    fn config(pairs: &[(&str, &str)]) -> DisplayConfig {
        DisplayConfig::from_map(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn an_idle_bar_is_just_the_tab_names() {
        let out = plain(&tabs(), &BTreeMap::new(), &Tracker::default(), &config(&[]));
        assert_eq!(out, " edit  build ");
    }

    #[test]
    fn a_busy_inactive_tab_gets_the_marker() {
        let out = plain(&tabs(), &BTreeMap::new(), &running_in(7), &config(&[]));
        assert_eq!(out, " edit ▶  build ");
    }

    #[test]
    fn the_marker_shows_on_the_tab_you_are_looking_at() {
        // The whole point: attention hides itself on the focused tab, but a
        // running command still has not finished.
        let out = plain(&tabs(), &BTreeMap::new(), &running_in(8), &config(&[]));
        assert_eq!(out, " edit  build ▶ ");
    }

    #[test]
    fn a_tab_can_carry_both_a_marker_and_an_attention_icon() {
        let attention = BTreeMap::from([(0, Attn::Waiting)]);
        let out = plain(&tabs(), &attention, &running_in(7), &config(&[]));
        assert_eq!(out, " edit ▶ ●  build ");
    }

    #[test]
    fn attention_alone_still_renders() {
        let attention = BTreeMap::from([(0, Attn::Done)]);
        let out = plain(&tabs(), &attention, &Tracker::default(), &config(&[]));
        assert_eq!(out, " edit ✓  build ");
    }

    #[test]
    fn turning_activity_off_hides_the_marker_everywhere() {
        let display = config(&[("activity", "false")]);
        assert_eq!(
            plain(&tabs(), &BTreeMap::new(), &running_in(7), &display),
            " edit  build "
        );
        assert_eq!(
            plain(&tabs(), &BTreeMap::new(), &running_in(8), &display),
            " edit  build "
        );
    }

    #[test]
    fn ascii_and_custom_glyphs_reach_the_marker() {
        assert_eq!(
            plain(
                &tabs(),
                &BTreeMap::new(),
                &running_in(7),
                &config(&[("ascii", "true")])
            ),
            " edit >  build "
        );
        assert_eq!(
            plain(
                &tabs(),
                &BTreeMap::new(),
                &running_in(7),
                &config(&[("glyph_running", "RUN")])
            ),
            " edit RUN  build "
        );
    }

    #[test]
    fn attention_on_a_tab_with_no_panes_is_still_drawn() {
        // Attention is keyed by tab, activity by pane: a tab whose panes we
        // have not seen yet must still show its icon.
        let attention = BTreeMap::from([(0, Attn::Working)]);
        let out = plain(&tabs(), &attention, &Tracker::default(), &config(&[]));
        assert_eq!(out, " edit ◐  build ");
    }

    #[test]
    fn visible_len_ignores_color_codes() {
        colored::control::set_override(true);
        let colored_text = format!("{}", "edit".bold());
        assert!(colored_text.len() > 4, "should carry escape codes");
        assert_eq!(visible_len(&colored_text), 4);
        colored::control::set_override(false);
    }

    #[test]
    fn visible_len_counts_the_marker_as_one_column() {
        assert_eq!(visible_len(" edit ▶ "), 8);
    }
}

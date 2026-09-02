//! Which panes are running a foreground shell command.
//!
//! The shell hooks (`assets/cockpit-shell.sh`) pipe a message around every
//! command. This module owns what those messages mean; the plugin only feeds
//! them in and asks which tabs are busy.

use std::collections::BTreeMap;

/// One pane's foreground-command state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Activity {
    /// Which shell is talking: its start time. A pane has one shell at a time,
    /// so a different era simply means a new shell, and whatever it says is the
    /// truth - eras are never compared. Ranking them would let one bad value (a
    /// clock jump, a stray message) silence the pane for good.
    pub era: u64,
    /// That shell's command counter. The hooks fire in the background, so pipes
    /// from the same shell can arrive out of order; an older seq is dropped.
    pub seq: u64,
    pub running: bool,
    /// Only drawn once it has survived a refresh. Commands that finish inside
    /// one tick never draw, so the bar doesn't flicker on every `ls`.
    pub shown: bool,
}

/// A parsed `cockpit::activity::…` message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// `<start|end>::<pane_id>::<era>::<seq>`
    Command {
        pane: u32,
        era: u64,
        seq: u64,
        running: bool,
    },
    /// `reset` - drop every marker. The escape hatch for a stuck bar.
    Reset,
}

impl Message {
    /// Parse what follows `cockpit::activity::` in the pipe name.
    pub fn parse(rest: &str) -> Option<Self> {
        if rest.trim() == "reset" {
            return Some(Message::Reset);
        }
        let mut parts = rest.split("::");
        let (state, pane, era, seq) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let running = match state {
            "start" => true,
            "end" => false,
            _ => return None,
        };
        Some(Message::Command {
            pane: pane.trim().parse().ok()?,
            era: era.trim().parse().ok()?,
            seq: seq.trim().parse().ok()?,
            running,
        })
    }
}

/// Every pane the shell hooks have told us about.
#[derive(Default, Debug)]
pub struct Tracker {
    panes: BTreeMap<u32, Activity>,
}

impl Tracker {
    /// Apply a message. Returns whether the drawn bar has to change now.
    pub fn apply(&mut self, message: Message) -> bool {
        let (pane, era, seq, running) = match message {
            Message::Reset => {
                let had_marker = self.panes.values().any(|a| a.running && a.shown);
                self.panes.clear();
                return had_marker;
            }
            Message::Command {
                pane,
                era,
                seq,
                running,
            } => (pane, era, seq, running),
        };

        // Within one shell, a late pipe from an older command must not
        // resurrect it. Across shells, the newcomer always wins.
        let previous = self.panes.get(&pane).copied();
        if previous.is_some_and(|p| p.era == era && seq < p.seq) {
            return false;
        }
        let was_shown = previous.is_some_and(|p| p.running && p.shown);
        self.panes.insert(
            pane,
            Activity {
                era,
                seq,
                running,
                shown: false,
            },
        );
        // Starting draws nothing yet (`tick` decides); ending has to repaint if
        // the marker was up.
        was_shown && !running
    }

    /// One refresh went by. A command still running is now worth drawing.
    /// Returns whether anything became visible.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        for activity in self.panes.values_mut() {
            if activity.running && !activity.shown {
                activity.shown = true;
                changed = true;
            }
        }
        changed
    }

    /// Forget panes that no longer exist, so a pane closed mid-command does not
    /// leave its tab marked.
    pub fn retain_panes(&mut self, exists: impl Fn(u32) -> bool) {
        self.panes.retain(|pane, _| exists(*pane));
    }

    /// Is a command running in any pane of this tab?
    pub fn tab_is_busy(&self, tab: usize, pane_to_tab: &BTreeMap<u32, usize>) -> bool {
        self.panes
            .iter()
            .any(|(pane, a)| a.running && a.shown && pane_to_tab.get(pane) == Some(&tab))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(pane: u32, era: u64, seq: u64, running: bool) -> Message {
        Message::Command {
            pane,
            era,
            seq,
            running,
        }
    }

    fn one_pane_on_tab_0() -> BTreeMap<u32, usize> {
        BTreeMap::from([(7, 0)])
    }

    fn busy(tracker: &Tracker) -> bool {
        tracker.tab_is_busy(0, &one_pane_on_tab_0())
    }

    #[test]
    fn parses_both_message_shapes() {
        assert_eq!(
            Message::parse("start::7::100::3"),
            Some(cmd(7, 100, 3, true))
        );
        assert_eq!(
            Message::parse("end::7::100::4"),
            Some(cmd(7, 100, 4, false))
        );
        assert_eq!(Message::parse("reset"), Some(Message::Reset));
    }

    #[test]
    fn rejects_malformed_messages() {
        for bad in [
            "start::7::100",      // no seq: the pre-era format
            "sleep::7::100::3",   // not a state
            "start::abc::100::3", // pane is not a number
            "start::7::era::3",   // era is not a number
            "start::7::100::seq", // seq is not a number
            "start::-1::100::3",  // pane is not unsigned
            "",                   // nothing at all
            "resetting::7::1::1", // reset is exact, not a prefix
        ] {
            assert_eq!(Message::parse(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_command_draws_only_after_it_survives_a_refresh() {
        let mut tracker = Tracker::default();
        assert!(!tracker.apply(cmd(7, 100, 1, true)));
        assert!(!busy(&tracker), "must not draw before the first tick");
        assert!(tracker.tick());
        assert!(busy(&tracker));
    }

    #[test]
    fn a_command_that_ends_within_one_tick_never_draws() {
        let mut tracker = Tracker::default();
        tracker.apply(cmd(7, 100, 1, true));
        assert!(!tracker.apply(cmd(7, 100, 2, false)));
        assert!(!tracker.tick(), "nothing is running, nothing to reveal");
        assert!(!busy(&tracker));
    }

    #[test]
    fn ending_a_drawn_command_clears_the_marker_and_repaints() {
        let mut tracker = Tracker::default();
        tracker.apply(cmd(7, 100, 1, true));
        tracker.tick();
        assert!(tracker.apply(cmd(7, 100, 2, false)), "must repaint");
        assert!(!busy(&tracker));
    }

    #[test]
    fn an_older_message_from_the_same_shell_is_dropped() {
        // The hooks fire in the background, so start(1) can land after end(2).
        let mut tracker = Tracker::default();
        tracker.apply(cmd(7, 100, 2, false));
        tracker.apply(cmd(7, 100, 1, true));
        tracker.tick();
        assert!(
            !busy(&tracker),
            "a late start must not resurrect the command"
        );
    }

    #[test]
    fn a_new_shell_wins_however_its_era_compares() {
        // This is the bug that silenced a pane: one far-future value used to
        // make every later message look old.
        for stray_era in [u64::MAX, 9_999_999_999, 0] {
            let mut tracker = Tracker::default();
            tracker.apply(cmd(7, stray_era, u64::MAX, false));
            tracker.apply(cmd(7, 100, 1, true));
            tracker.tick();
            assert!(busy(&tracker), "stray era {stray_era} silenced the pane");
        }
    }

    #[test]
    fn reset_clears_every_marker_and_repaints_only_when_one_was_up() {
        let mut tracker = Tracker::default();
        assert!(!tracker.apply(Message::Reset), "nothing was drawn");
        tracker.apply(cmd(7, 100, 1, true));
        tracker.tick();
        assert!(tracker.apply(Message::Reset));
        assert!(!busy(&tracker));
        // A shell that keeps talking after a reset is believed again.
        tracker.apply(cmd(7, 100, 2, true));
        tracker.tick();
        assert!(busy(&tracker));
    }

    #[test]
    fn a_pane_that_closes_mid_command_drops_its_marker() {
        let mut tracker = Tracker::default();
        tracker.apply(cmd(7, 100, 1, true));
        tracker.tick();
        assert!(busy(&tracker));
        tracker.retain_panes(|pane| pane != 7);
        assert!(!busy(&tracker));
    }

    #[test]
    fn only_the_tab_holding_the_pane_is_busy() {
        let mut tracker = Tracker::default();
        let panes = BTreeMap::from([(7, 0), (8, 1)]);
        tracker.apply(cmd(8, 100, 1, true));
        tracker.tick();
        assert!(!tracker.tab_is_busy(0, &panes));
        assert!(tracker.tab_is_busy(1, &panes));
    }

    #[test]
    fn a_pane_the_bar_does_not_know_yet_marks_nothing() {
        let mut tracker = Tracker::default();
        tracker.apply(cmd(99, 100, 1, true));
        tracker.tick();
        assert!(!busy(&tracker));
    }
}

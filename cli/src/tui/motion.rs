//! Time for animation, kept apart from the tick every cadence is counted in.
//!
//! The loop sets the clock before every reduce and every draw; panes read it
//! to know how far along a movement is. A [`Motion`] is live, slowed for
//! capturing a frame mid-movement, or settled. Settled is what a test gets
//! and what `HEDOS_MOTION=off` asks for: every movement reads as finished,
//! so a frame is always its final one. The clock still advances when
//! settled, since an elapsed time on screen ("thinking · 1.2s") is
//! information, not motion.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How long one spinner frame shows: twelve frames a second.
const SPIN_FRAME_MS: u64 = 83;
/// How much longer every movement takes when slowed.
const SLOW_FACTOR: u64 = 10;

/// Whether things move, and how fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Live,
    Slow,
    Settled,
}

/// The animation clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Motion {
    mode: Mode,
    now_ms: u64,
    wall_ms: i64,
}

impl Motion {
    /// A clock on which every movement has already finished.
    pub(crate) fn settled() -> Self {
        Self {
            mode: Mode::Settled,
            now_ms: 0,
            wall_ms: 0,
        }
    }

    /// The clock `HEDOS_MOTION` asks for: `off`, `0`, `false` or `no`
    /// settle everything, `slow` stretches every movement tenfold, anything
    /// else (or nothing) is live.
    pub(crate) fn from_env() -> Self {
        Self::from_value(std::env::var("HEDOS_MOTION").ok().as_deref())
    }

    /// The clock a `HEDOS_MOTION` of `value` asks for, without reading the
    /// environment.
    #[cfg(test)]
    pub(crate) fn from_env_value_for_tests(value: Option<&str>) -> Self {
        Self::from_value(value)
    }

    fn from_value(value: Option<&str>) -> Self {
        let mode = match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("off" | "0" | "false" | "no") => Mode::Settled,
            Some("slow") => Mode::Slow,
            _ => Mode::Live,
        };
        Self {
            mode,
            now_ms: 0,
            wall_ms: 0,
        }
    }

    /// Set the clock to `now_ms` milliseconds since the screen first opened.
    pub(crate) fn set(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// Real milliseconds since the screen first opened, slowed or not, for
    /// what is information rather than motion: how long a reply has taken.
    pub(crate) fn real_ms(&self) -> u64 {
        self.now_ms
    }

    /// Set the clock on the wall, in Unix milliseconds.
    pub(crate) fn set_wall(&mut self, wall_ms: i64) {
        self.wall_ms = wall_ms;
    }

    /// The clock on the wall, in Unix milliseconds; zero until the loop sets
    /// it.
    pub(crate) fn wall_ms(&self) -> i64 {
        self.wall_ms
    }

    /// The animation clock in milliseconds since the screen first opened:
    /// real time, or a tenth of it when slowed. Every movement's start is
    /// stamped on this clock, so a slowed movement starts late as well as
    /// running long.
    pub(crate) fn now_ms(&self) -> u64 {
        match self.mode {
            Mode::Slow => self.now_ms / SLOW_FACTOR,
            Mode::Live | Mode::Settled => self.now_ms,
        }
    }

    /// How long ago `at_ms` on the animation clock was, in seconds; `None`
    /// when settled, where every movement has finished.
    pub(crate) fn age(&self, at_ms: u64) -> Option<f32> {
        // Divided in f64 and narrowed after, so a screen left open for days
        // keeps its movements smooth.
        let elapsed = (self.now_ms().saturating_sub(at_ms) as f64 / 1000.0) as f32;
        match self.mode {
            Mode::Live | Mode::Slow => Some(elapsed),
            Mode::Settled => None,
        }
    }

    /// How far through a movement of `duration_ms` that began at `at_ms`
    /// the clock is, from 0 to 1; 1 when settled.
    pub(crate) fn progress(&self, at_ms: u64, duration_ms: u64) -> f32 {
        match self.age(at_ms) {
            Some(age) => (age * 1000.0 / duration_ms.max(1) as f32).clamp(0.0, 1.0),
            None => 1.0,
        }
    }

    /// [`Self::progress`] eased out, fast then settling.
    pub(crate) fn eased(&self, at_ms: u64, duration_ms: u64) -> f32 {
        ease_out_cubic(self.progress(at_ms, duration_ms))
    }

    /// `text` as it reads `per_ms` into each character's arrival from
    /// `at_ms`: each one comes in as `.`, then `:`, then itself, the next
    /// one a step behind. A character still on its way is flagged, so it
    /// can be drawn quieter; settled, the text arrives whole.
    pub(crate) fn reveal(&self, text: &str, at_ms: u64, per_ms: u64) -> Vec<(String, bool)> {
        let Some(age) = self.age(at_ms) else {
            return vec![(text.to_owned(), false)];
        };
        let step = age * 1000.0 / per_ms.max(1) as f32;
        text.graphemes(true)
            .enumerate()
            .map(|(index, grapheme)| {
                let since = step - index as f32;
                if grapheme.trim().is_empty() || since >= 2.0 {
                    (grapheme.to_owned(), false)
                } else if since < 0.0 {
                    (" ".repeat(grapheme.width()), false)
                } else if since < 1.0 {
                    (".".to_owned(), true)
                } else {
                    (":".to_owned(), true)
                }
            })
            .collect()
    }

    /// Whether anything moves at all.
    pub(crate) fn is_live(&self) -> bool {
        self.mode != Mode::Settled
    }

    /// `duration` as this clock runs it: stretched when slowed.
    pub(crate) fn scaled(&self, duration: Duration) -> Duration {
        match self.mode {
            Mode::Slow => duration * SLOW_FACTOR as u32,
            Mode::Live | Mode::Settled => duration,
        }
    }

    /// Which spinner frame shows. Live, it turns twelve times a second; settled,
    /// it turns on `ticks`, so a wait still shows activity with motion off.
    pub(crate) fn spin_frame(&self, ticks: u64) -> u64 {
        match self.mode {
            Mode::Live | Mode::Slow => self.now_ms() / SPIN_FRAME_MS,
            Mode::Settled => ticks,
        }
    }

    /// How often a spinner needs a new frame.
    pub(crate) fn spin_interval(&self) -> Duration {
        self.scaled(Duration::from_millis(SPIN_FRAME_MS))
    }
}

/// A figure that eases to a new value instead of jumping: it starts from
/// zero at `first_at_ms` the first time it is read, and from whatever it
/// showed when its target changes after that. Read while drawing, so it
/// keeps its own state in a cell.
#[derive(Debug, Default)]
pub(crate) struct Eased(Cell<Option<Tween>>);

#[derive(Debug, Clone, Copy)]
struct Tween {
    from: f64,
    to: f64,
    at_ms: u64,
}

impl Eased {
    /// The figure to show for `target` now, moving over `duration_ms`.
    pub(crate) fn value(
        &self,
        target: f64,
        first_at_ms: u64,
        duration_ms: u64,
        motion: &Motion,
    ) -> f64 {
        let shown = |tween: Tween| {
            tween.from + (tween.to - tween.from) * f64::from(motion.eased(tween.at_ms, duration_ms))
        };
        let tween = match self.0.get() {
            None => Tween {
                from: 0.0,
                to: target,
                at_ms: first_at_ms,
            },
            Some(tween) if tween.to != target => Tween {
                from: shown(tween),
                to: target,
                at_ms: motion.now_ms(),
            },
            Some(tween) => tween,
        };
        self.0.set(Some(tween));
        shown(tween)
    }

    /// Whether the figure is still moving toward its target; one never shown
    /// is not moving.
    pub(crate) fn moving(&self, duration_ms: u64, motion: &Motion) -> bool {
        self.0
            .get()
            .is_some_and(|tween| motion.progress(tween.at_ms, duration_ms) < 1.0)
    }
}

/// A set of figures that ease as [`Eased`] does, keyed, for things that
/// come and go: a model warmed grows in from nothing, one unloaded shrinks
/// away before it leaves the set.
#[derive(Debug, Default)]
pub(crate) struct EasedSet(RefCell<Vec<Keyed>>, Cell<bool>);

#[derive(Debug)]
struct Keyed {
    key: String,
    label: String,
    figure: Eased,
    present: bool,
}

impl EasedSet {
    /// The `(label, value)` to show for `items` (key, label, target), in
    /// their order, followed by any key that has left and is still easing
    /// to nothing.
    pub(crate) fn values(
        &self,
        items: &[(String, String, f64)],
        duration_ms: u64,
        motion: &Motion,
    ) -> Vec<(String, f64)> {
        let mut entries = self.0.borrow_mut();
        // Only the first read is the screen opening; a set emptied later
        // and filled again grows from the moment it fills.
        let first = !self.1.replace(true);
        for entry in entries.iter_mut() {
            entry.present = false;
        }
        for (key, label, _) in items {
            match entries.iter_mut().find(|entry| &entry.key == key) {
                Some(entry) => {
                    entry.present = true;
                    entry.label.clone_from(label);
                }
                None => entries.push(Keyed {
                    key: key.clone(),
                    label: label.clone(),
                    figure: Eased::default(),
                    present: true,
                }),
            }
        }
        // What is there when the screen opens grows in with the launch;
        // what arrives later grows from the moment it does.
        let start = if first { 0 } else { motion.now_ms() };
        let mut shown = Vec::new();
        for (key, _, target) in items {
            if let Some(entry) = entries.iter().find(|entry| &entry.key == key) {
                let value = entry.figure.value(*target, start, duration_ms, motion);
                shown.push((entry.label.clone(), value));
            }
        }
        entries.retain(|entry| {
            entry.present
                || entry.figure.value(0.0, start, duration_ms, motion) > 0.0
                    && entry.figure.moving(duration_ms, motion)
        });
        for entry in entries.iter().filter(|entry| !entry.present) {
            let value = entry.figure.value(0.0, start, duration_ms, motion);
            shown.push((entry.label.clone(), value));
        }
        shown
    }

    /// Whether any figure in the set is still easing.
    pub(crate) fn moving(&self, duration_ms: u64, motion: &Motion) -> bool {
        self.0
            .borrow()
            .iter()
            .any(|entry| entry.figure.moving(duration_ms, motion))
    }
}

/// Fast at first, settling at the end.
pub(crate) fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

/// Two sines of unrelated frequency averaged, so a drift takes minutes to
/// come back to where it started.
pub(crate) fn drift(t: f32, a: f32, b: f32) -> f32 {
    ((t * a).sin() + (t * b).sin()) * 0.5
}

impl Default for Motion {
    fn default() -> Self {
        Self::settled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_picks_the_mode() {
        for off in ["off", "0", "false", "NO", " off "] {
            assert!(!Motion::from_value(Some(off)).is_live(), "{off}");
        }
        assert_eq!(Motion::from_value(Some("slow")).mode, Mode::Slow);
        assert_eq!(Motion::from_value(None).mode, Mode::Live);
        assert_eq!(Motion::from_value(Some("anything")).mode, Mode::Live);
        assert!(!Motion::settled().is_live());
    }

    #[test]
    fn the_spinner_turns_on_the_clock_when_live_and_on_the_tick_when_settled() {
        let mut live = Motion::from_value(None);
        live.set(SPIN_FRAME_MS * 3 + 1);
        assert_eq!(live.spin_frame(99), 3);
        let mut slow = Motion::from_value(Some("slow"));
        slow.set(SPIN_FRAME_MS * 3 + 1);
        assert_eq!(slow.spin_frame(99), 0);
        let mut settled = Motion::settled();
        settled.set(SPIN_FRAME_MS * 3);
        assert_eq!(settled.spin_frame(7), 7);
    }

    fn live_at(now_ms: u64) -> Motion {
        let mut motion = Motion::from_value(None);
        motion.set(now_ms);
        motion
    }

    #[test]
    fn progress_runs_from_zero_to_one_and_settled_is_done() {
        assert_eq!(live_at(100).progress(100, 200), 0.0);
        assert_eq!(live_at(200).progress(100, 200), 0.5);
        assert_eq!(live_at(900).progress(100, 200), 1.0);
        assert_eq!(Motion::settled().progress(100, 200), 1.0);
        assert!((live_at(200).eased(100, 200) - 0.875).abs() < 1e-6);
        let mut slow = Motion::from_value(Some("slow"));
        slow.set(2000);
        assert_eq!(
            slow.progress(100, 200),
            0.5,
            "a tenth of 2000 ms is 100 ms into the movement"
        );
    }

    #[test]
    fn a_reveal_arrives_as_a_dot_a_colon_and_the_letter() {
        let read = |now| -> String {
            live_at(now)
                .reveal("ab", 0, 14)
                .into_iter()
                .map(|(text, _)| text)
                .collect()
        };
        assert_eq!(read(0), ". ");
        assert_eq!(read(14), ":.");
        assert_eq!(read(28), "a:");
        assert_eq!(read(42), "ab");
        let ramps = live_at(14).reveal("ab", 0, 14);
        assert!(ramps.iter().all(|(_, ramp)| *ramp));
        assert_eq!(
            Motion::settled().reveal("ab", 0, 14),
            vec![("ab".to_owned(), false)]
        );
        let spaced: String = live_at(0)
            .reveal("a b", 0, 14)
            .into_iter()
            .map(|(text, _)| text)
            .collect();
        assert_eq!(spaced, ".  ");
    }

    #[test]
    fn an_eased_figure_counts_up_then_eases_to_a_change() {
        let figure = Eased::default();
        assert_eq!(figure.value(10.0, 100, 200, &live_at(100)), 0.0);
        assert_eq!(figure.value(10.0, 100, 200, &live_at(300)), 10.0);
        assert!(!figure.moving(200, &live_at(300)));
        assert_eq!(figure.value(20.0, 100, 200, &live_at(400)), 10.0);
        assert!(figure.moving(200, &live_at(400)));
        assert_eq!(figure.value(20.0, 100, 200, &live_at(600)), 20.0);
        let settled = Eased::default();
        assert_eq!(settled.value(7.0, 100, 200, &Motion::settled()), 7.0);
    }

    #[test]
    fn a_keyed_figure_grows_in_and_shrinks_away() {
        let set = EasedSet::default();
        let item = |key: &str, value| (key.to_owned(), key.to_owned(), value);
        assert_eq!(
            set.values(&[item("a", 4.0)], 100, &live_at(0)),
            vec![("a".to_owned(), 0.0)]
        );
        assert_eq!(
            set.values(&[item("a", 4.0)], 100, &live_at(200)),
            vec![("a".to_owned(), 4.0)]
        );
        let grown = set.values(&[item("a", 4.0), item("b", 2.0)], 100, &live_at(300));
        assert_eq!(grown, vec![("a".to_owned(), 4.0), ("b".to_owned(), 0.0)]);
        let leaving = set.values(&[item("b", 2.0)], 100, &live_at(500));
        assert_eq!(leaving[0], ("b".to_owned(), 2.0));
        assert_eq!(leaving[1], ("a".to_owned(), 4.0), "a starts shrinking now");
        assert!(set.moving(100, &live_at(550)));
        let gone = set.values(&[item("b", 2.0)], 100, &live_at(700));
        assert_eq!(gone, vec![("b".to_owned(), 2.0)], "a has shrunk away");
        let settled = EasedSet::default();
        assert_eq!(
            settled.values(&[item("a", 4.0)], 100, &Motion::settled()),
            vec![("a".to_owned(), 4.0)]
        );
    }

    #[test]
    fn slowed_motion_stretches_every_duration() {
        let slow = Motion::from_value(Some("slow"));
        assert_eq!(
            slow.spin_interval(),
            Duration::from_millis(SPIN_FRAME_MS * 10)
        );
        let live = Motion::from_value(None);
        assert_eq!(live.spin_interval(), Duration::from_millis(SPIN_FRAME_MS));
    }
}

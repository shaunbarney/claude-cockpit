//! Frame clock, spinners, and change-flash tracking.
//!
//! The dashboard deliberately does no work when nothing is happening — the run
//! loop redraws on a data revision bump or on input. That keeps an idle cockpit
//! free, but it also means the UI never *shows* change: a job that flips from
//! `working` to `blocked` just silently has different text next frame.
//!
//! This module supplies the two pieces that fix it without giving up the idle
//! property: a monotonic frame counter for anything that animates, and a
//! [`Flashes`] set that remembers which rows changed recently so they can be
//! highlighted as they settle. Both report whether they are currently animating,
//! so the run loop can pick a fast tick only while something is actually moving.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Frame period while something is animating (~12.5 fps — smooth enough for a
/// spinner and a fade, cheap enough to be invisible in a CPU graph).
pub const FRAME: Duration = Duration::from_millis(80);

/// How long a changed row stays highlighted.
pub const FLASH: Duration = Duration::from_millis(1600);

/// Braille spinner frames, used while a manual refresh is in flight.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A soft "breathing" pulse used for live indicators.
pub const PULSE: [&str; 8] = ["·", "•", "●", "◉", "●", "•", "·", " "];

/// Monotonic clock driving every animation. One instance lives on `App`.
#[derive(Debug, Clone)]
pub struct Clock {
    start: Instant,
}

impl Default for Clock {
    fn default() -> Self {
        Clock {
            start: Instant::now(),
        }
    }
}

impl Clock {
    /// Frames elapsed since start, at [`FRAME`] resolution.
    pub fn frame(&self) -> u64 {
        (self.start.elapsed().as_millis() / FRAME.as_millis()) as u64
    }

    /// The current spinner glyph.
    pub fn spinner(&self) -> &'static str {
        SPINNER[(self.frame() as usize) % SPINNER.len()]
    }

    /// The current pulse glyph, at half spinner speed so it reads as breathing
    /// rather than spinning.
    pub fn pulse(&self) -> &'static str {
        PULSE[((self.frame() / 2) as usize) % PULSE.len()]
    }
}

/// Rows that changed recently, keyed by a stable row identity.
///
/// `mark` records a change; `intensity` reports how fresh it is on a 1.0 → 0.0
/// ramp so the renderer can fade the highlight out instead of snapping it off.
#[derive(Debug, Default, Clone)]
pub struct Flashes {
    seen: HashMap<String, Instant>,
}

impl Flashes {
    /// Record that `key` just changed.
    pub fn mark(&mut self, key: &str) {
        self.seen.insert(key.to_string(), Instant::now());
    }

    /// Freshness of `key` on a 1.0 (just changed) → 0.0 (settled) ramp.
    pub fn intensity(&self, key: &str) -> f32 {
        let Some(at) = self.seen.get(key) else {
            return 0.0;
        };
        let e = at.elapsed();
        if e >= FLASH {
            return 0.0;
        }
        1.0 - (e.as_millis() as f32 / FLASH.as_millis() as f32)
    }

    /// True while any flash is still fading — the run loop uses this to decide
    /// whether it needs to keep ticking.
    pub fn animating(&self) -> bool {
        self.seen.values().any(|at| at.elapsed() < FLASH)
    }

    /// Drop settled entries so the map cannot grow without bound over a long
    /// session. Called opportunistically from the run loop.
    pub fn gc(&mut self) {
        self.seen.retain(|_, at| at.elapsed() < FLASH);
    }

    /// Number of entries still being tracked (settled or not).
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// True when nothing is being tracked at all.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_and_pulse_cycle() {
        let c = Clock::default();
        // Both must return one of their own frames, whatever the wall clock.
        assert!(SPINNER.contains(&c.spinner()));
        assert!(PULSE.contains(&c.pulse()));
    }

    #[test]
    fn unmarked_key_has_no_intensity() {
        let f = Flashes::default();
        assert_eq!(f.intensity("nope"), 0.0);
        assert!(!f.animating());
    }

    #[test]
    fn marked_key_starts_hot_and_animates() {
        let mut f = Flashes::default();
        f.mark("job:1");
        assert!(f.intensity("job:1") > 0.9);
        assert!(f.animating());
        assert_eq!(f.intensity("job:2"), 0.0);
    }

    #[test]
    fn gc_keeps_live_entries() {
        let mut f = Flashes::default();
        f.mark("a");
        f.gc();
        assert_eq!(f.len(), 1);
    }
}

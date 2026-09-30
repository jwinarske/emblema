//! Counting the vertical blanks a frame loop did not land on.
//!
//! A frame rate on its own says nothing about pacing. A loop presenting sixty
//! frames a second to a sixty-hertz display is doing the right thing; a loop
//! presenting sixty while the display refreshes a hundred and twenty is missing
//! every other blank and looks identical from a frame counter. What separates them
//! is the sequence number the kernel reports with each completed flip, which is the
//! display's own count of blanks since the pipeline came up.
//!
//! So this counts in blanks, not in seconds, and the arithmetic is integer. There is
//! no threshold to calibrate per board and no clock to trust. Three reasons that
//! matters rather than one:
//!
//! - [`crate::Mode::frame_nanos`] returns zero where the driver reports no refresh
//!   rate, so a rule of the shape "an interval longer than two frames is a miss"
//!   classifies every interval as a miss on such a driver -- silently, and in the
//!   direction that invents a problem.
//! - The refresh a mode reports is rounded to whole hertz, so it is about one per
//!   cent out on the 59.94 Hz modes HDMI is full of. Fine for a wait budget, useless
//!   for deciding whether an interval held one blank or two.
//! - The event loop polls every five hundred microseconds rather than sleeping on the
//!   descriptor, so a time measured in userspace carries an arbitrary fraction of a
//!   millisecond that has nothing to do with the display.
//!
//! Kernel timestamps are still recorded, for one job: a cross-check. Flips times the
//! period should account for the span they arrived over, and if it does not then the
//! sequence numbers are not what this thinks they are. They are never mixed with
//! `Instant`, which has no epoch to share.
//!
//! What a single miss count cannot tell you is whether there was headroom. A loop that
//! waits for the flip it committed before committing again has one commit outstanding
//! whatever the ring depth, so a deep ring hides a slow frame instead of missing a
//! blank. Zero misses at depth three is consistent with a frame taking almost the whole
//! period and with one taking a tenth of it. Two things separate those, which is why a
//! miss count is reported beside the ring depth rather than on its own: the offscreen
//! figure `cargo xtask bench` measures, and the same run at depth two, where there is
//! no spare buffer to hide behind. `docs/on-a-board.md` has both for a Pi 5.

use std::time::Duration;

/// Flips discarded before counting starts.
///
/// The first flip follows the commit that set the mode, which enables the CRTC --
/// and a blank counter means nothing until the thing counting blanks is running. The
/// commit that set it is also the one the target deliberately waits for on the CPU,
/// so the frame after it started late through no fault of the loop.
///
/// Five, which is what `xtask`'s bench discards before it starts timing, for the
/// same reason and deliberately the same number.
pub const WARMUP: u32 = 5;

/// A gap this large is a broken counter rather than a slow loop.
///
/// At sixty hertz a thousand blanks is sixteen seconds. A loop that stalled that
/// long has failed in a way a miss count does not describe, and a counter that
/// jumped that far has more likely restarted or is not a blank counter at all.
/// Reported as unusable rather than as nine hundred and ninety-nine misses.
const SANITY: u32 = 1000;

/// What a run of completed flips says about the blanks it did not land on.
///
/// Fed one call per completed flip and asked afterwards. Holds no display types on
/// purpose: the arithmetic is the part worth testing without a card, and a table of
/// sequence numbers tests it on any machine.
#[derive(Debug, Clone, Default)]
pub struct Pacing {
    warmup_left: u32,
    first: Option<(u32, Duration)>,
    last: Option<(u32, Duration)>,
    flips: u64,
    usable: bool,
}

impl Pacing {
    /// A ledger that has seen nothing.
    pub fn new() -> Self {
        Self {
            warmup_left: WARMUP,
            first: None,
            last: None,
            flips: 0,
            usable: true,
        }
    }

    /// Record a completed flip: the blank it landed on, and when the kernel said so.
    pub fn observe(&mut self, sequence: u32, at: Duration) {
        if self.warmup_left > 0 {
            self.warmup_left -= 1;
            return;
        }
        let Some((previous, _)) = self.last else {
            // The first flip after warm-up is where counting starts from. It is a
            // reference point and not yet a sample: there is no interval behind it
            // to judge.
            self.first = Some((sequence, at));
            self.last = Some((sequence, at));
            self.flips = 1;
            return;
        };

        // Wrapping, because the kernel's counter is a `u32` and a long run reaches
        // the end of one. A gap is still a gap across that boundary.
        let gap = sequence.wrapping_sub(previous);
        if gap == 0 || gap >= SANITY {
            // A counter that did not advance is not reporting blanks, and one that
            // jumped further than a stall explains has restarted or is something
            // else. Either way the total below would be a number with no meaning,
            // so the ledger says so instead of producing one.
            self.usable = false;
        }
        self.last = Some((sequence, at));
        self.flips += 1;
    }

    /// Begin again, keeping nothing.
    ///
    /// For a reconfigure, where the mode may change and the driver may restart its
    /// counter, so nothing before it is comparable with anything after. Warm-up
    /// starts over because the commit that follows sets the mode again.
    ///
    /// Nothing calls this yet. `KmsOutput` never reports a reconfigure -- hotplug is
    /// not detected -- so the branch is written for the shape of the problem rather
    /// than for a caller that exists.
    pub fn restart(&mut self) {
        *self = Self::new();
    }

    /// Flips counted, warm-up excluded.
    pub fn flips(&self) -> u64 {
        self.flips
    }

    /// Blanks between the first counted flip and the last.
    pub fn elapsed_vblanks(&self) -> u64 {
        match (self.first, self.last) {
            (Some((first, _)), Some((last, _))) => u64::from(last.wrapping_sub(first)),
            _ => 0,
        }
    }

    /// Blanks at which the display latched nothing new while the loop was trying.
    ///
    /// The blanks that passed, less the intervals that were filled. A flip landing
    /// on the very next blank fills its interval and misses nothing, which is why
    /// this subtracts one per interval rather than one per flip.
    ///
    /// `None` where the counter cannot be trusted; see [`Self::usable`].
    pub fn missed(&self) -> Option<u64> {
        if !self.usable || self.flips < 2 {
            return None;
        }
        Some(self.elapsed_vblanks() - (self.flips - 1))
    }

    /// Whether the sequence numbers behaved like a blank counter.
    ///
    /// False where one did not advance between flips, or advanced further than a
    /// stall explains. A caller reporting a run should say it skipped rather than
    /// print a total this refused.
    pub fn usable(&self) -> bool {
        self.usable && self.flips >= 2
    }

    /// How long the counted flips arrived over, by the kernel's clock.
    ///
    /// For the cross-check rather than for the rate: flips times the period should
    /// account for this, and a disagreement means the sequence numbers are not
    /// measuring what they appear to.
    pub fn span(&self) -> Duration {
        match (self.first, self.last) {
            (Some((_, from)), Some((_, to))) => to.saturating_sub(from),
            _ => Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a run of sequence numbers, one blank apart in time unless stated.
    fn run(sequences: &[u32]) -> Pacing {
        let mut pacing = Pacing::new();
        for (i, sequence) in sequences.iter().enumerate() {
            pacing.observe(*sequence, Duration::from_micros(16_666 * i as u64));
        }
        pacing
    }

    /// Warm-up is discarded, and the flip after it is a reference rather than a
    /// sample.
    #[test]
    fn counting_starts_after_the_warmup_and_the_first_flip_is_the_baseline() {
        // Five warm-up flips and nothing else: nothing counted, nothing to say.
        let pacing = run(&[1, 2, 3, 4, 5]);
        assert_eq!(pacing.flips(), 0);
        assert_eq!(
            pacing.missed(),
            None,
            "a run with no samples reports no total"
        );
        assert!(!pacing.usable());

        // One more, which is the baseline. Still no interval behind it.
        let pacing = run(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(pacing.flips(), 1);
        assert_eq!(pacing.elapsed_vblanks(), 0);
        assert_eq!(pacing.missed(), None, "one flip is no interval");
    }

    /// Every flip on the next blank misses nothing.
    #[test]
    fn a_flip_on_every_blank_misses_none() {
        let pacing = run(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(pacing.flips(), 5, "ten flips less five of warm-up");
        assert_eq!(pacing.elapsed_vblanks(), 4);
        assert_eq!(pacing.missed(), Some(0));
        assert!(pacing.usable());
    }

    /// A gap of three blanks is two missed, not three.
    ///
    /// The interval itself is filled by the flip that ends it. Counting the gap
    /// rather than the gap less one is the arithmetic mistake this pins.
    #[test]
    fn a_gap_counts_the_blanks_that_latched_nothing() {
        // Warm-up 1-5, baseline 6, then 9: blanks 7 and 8 latched nothing.
        let pacing = run(&[1, 2, 3, 4, 5, 6, 9]);
        assert_eq!(pacing.flips(), 2);
        assert_eq!(pacing.elapsed_vblanks(), 3);
        assert_eq!(pacing.missed(), Some(2));

        // Two gaps of one blank each, in a longer run.
        let pacing = run(&[1, 2, 3, 4, 5, 6, 8, 9, 11]);
        assert_eq!(pacing.flips(), 4);
        assert_eq!(pacing.elapsed_vblanks(), 5);
        assert_eq!(pacing.missed(), Some(2));
    }

    /// The counter is a `u32` and a long run reaches the end of one.
    #[test]
    fn the_count_survives_the_counter_wrapping() {
        let pacing = run(&[1, 2, 3, 4, 5, u32::MAX - 1, u32::MAX, 0, 1]);
        assert_eq!(pacing.flips(), 4);
        assert_eq!(pacing.elapsed_vblanks(), 3, "three blanks across the wrap");
        assert_eq!(pacing.missed(), Some(0));
        assert!(pacing.usable());
    }

    /// A counter that does not advance is not reporting blanks.
    #[test]
    fn a_sequence_that_repeats_is_refused_rather_than_counted() {
        let pacing = run(&[1, 2, 3, 4, 5, 6, 6, 7]);
        assert!(!pacing.usable(), "a repeated sequence was accepted");
        assert_eq!(
            pacing.missed(),
            None,
            "a refused ledger produced a total anyway"
        );
    }

    /// A jump further than a stall explains is refused too.
    #[test]
    fn a_jump_past_the_sanity_bound_is_refused() {
        let pacing = run(&[1, 2, 3, 4, 5, 6, 6 + SANITY]);
        assert!(!pacing.usable());
        assert_eq!(pacing.missed(), None);

        // One blank inside the bound is a stall, not a broken counter, and is
        // counted -- which is what says the bound is a bound and not a ceiling on
        // everything.
        let pacing = run(&[1, 2, 3, 4, 5, 6, 5 + SANITY]);
        assert!(pacing.usable());
        assert_eq!(pacing.missed(), Some(u64::from(SANITY) - 2));
    }

    /// Restarting keeps nothing, warm-up included.
    #[test]
    fn a_restart_keeps_nothing() {
        let mut pacing = run(&[1, 2, 3, 4, 5, 6, 9]);
        assert_eq!(pacing.missed(), Some(2));
        pacing.restart();
        assert_eq!(pacing.flips(), 0);
        assert_eq!(pacing.missed(), None);
        assert_eq!(pacing.span(), Duration::ZERO);
        assert!(!pacing.usable(), "a restarted ledger reported a total");
    }

    /// The span is the kernel's, for the cross-check.
    #[test]
    fn the_span_covers_the_counted_flips_only() {
        let pacing = run(&[1, 2, 3, 4, 5, 6, 7, 8]);
        // Flips at indices 5, 6 and 7 of the feed, so two intervals of one blank.
        assert_eq!(pacing.flips(), 3);
        assert_eq!(pacing.span(), Duration::from_micros(16_666 * 2));
    }
}

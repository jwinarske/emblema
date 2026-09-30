//! Cutting a path into the pieces a dash pattern leaves behind.
//!
//! # Why this is a path transform rather than a stroke option
//!
//! A dashed stroke is a stroke of a different path. Cutting first and stroking
//! the pieces gives every dash its own caps and gives the joins that survive
//! the cut the same treatment they would have had, which is what a reader
//! expects and what SVG and PostScript specify. The alternative — teaching the
//! stroker to skip — would have to decide what a join means when one of its two
//! segments is not drawn, and there is no answer to that which is not simply
//! this one arrived at more expensively.
//!
//! It also means dashing composes with everything already here for free: the
//! result is a path, so it tessellates, transforms, clips and antialiases by
//! the same code as any other.
//!
//! # What a pattern means
//!
//! Lengths alternate drawn and skipped, starting drawn, and repeat. An odd
//! number of them repeats to an even one, so `[5]` is five on and five off —
//! the SVG rule, and the one that makes a single number mean something obvious
//! rather than nothing. The phase advances the starting position into the
//! pattern, which is how a marching-ants animation is made without rebuilding
//! anything.
//!
//! Measurement is along the flattened path, so a dash on a curve is as accurate
//! as the flattening tolerance makes it. That is the same accuracy the stroke
//! itself has, so a dash cannot be more wrong than the line it lies on.

use crate::flatten::flatten;
use crate::path::{Path, PathBuilder};
use glam::Vec2;

/// A dash pattern: alternating drawn and skipped lengths, and where to start.
#[derive(Debug, Clone, PartialEq)]
pub struct Dash {
    /// Alternating drawn and skipped lengths, starting with a drawn one.
    ///
    /// In the same space the path is in, so a pattern travels through the
    /// canvas transform with the geometry rather than staying fixed on screen.
    pub intervals: Vec<f32>,
    /// How far into the pattern the path starts.
    ///
    /// Wrapped into one period, so any value is meaningful and an animation can
    /// simply keep adding to it.
    pub phase: f32,
}

impl Dash {
    pub fn new(intervals: Vec<f32>, phase: f32) -> Self {
        Self { intervals, phase }
    }

    /// Whether this pattern describes something that can be walked.
    ///
    /// A pattern of nothing, of negative lengths, or of lengths that sum to
    /// zero has no period to advance through, and walking one would not
    /// terminate. Such a pattern is treated as no pattern at all: the path is
    /// drawn whole, which is the one behavior that cannot surprise anybody who
    /// got here by computing an interval that came out zero.
    pub fn is_usable(&self) -> bool {
        !self.intervals.is_empty()
            && self.intervals.iter().all(|v| v.is_finite() && *v >= 0.0)
            && self.intervals.iter().sum::<f32>() > 0.0
            && self.phase.is_finite()
    }

    /// The intervals as an even-length cycle, which is what walking wants.
    fn cycle(&self) -> Vec<f32> {
        if self.intervals.len() % 2 == 0 {
            self.intervals.clone()
        } else {
            // `[5]` becomes `[5, 5]`, and `[4, 2, 1]` becomes `[4, 2, 1, 4, 2,
            // 1]`: repeating the whole list is what makes an odd pattern
            // alternate rather than stall on one phase.
            let mut doubled = self.intervals.clone();
            doubled.extend_from_slice(&self.intervals);
            doubled
        }
    }
}

/// Cut `path` into the drawn pieces of `dash`.
///
/// `tolerance` is the flattening tolerance in the path's own space, the same
/// one the tessellator would use, so the dashes lie on the curve as closely as
/// the curve itself is drawn.
///
/// An unusable pattern returns the path unchanged rather than nothing: a caller
/// whose interval arithmetic produced a zero gets an undashed line, which is
/// visibly wrong in a way that leads back to the pattern, where an empty result
/// is invisible and leads nowhere.
pub fn dash_path(path: &Path, dash: &Dash, tolerance: f32) -> Path {
    if !dash.is_usable() {
        return path.clone();
    }
    let cycle = dash.cycle();
    let period: f32 = cycle.iter().sum();
    // A pattern finer than the curve is flattened to cannot be drawn as dashes:
    // every on and off together falls inside one line segment of the polyline
    // below, so there is nothing for them to land on distinctly. Returned
    // unchanged on the same terms as an unusable pattern, and for a sharper
    // reason than tidiness -- `walk` counts intervals rather than distance, so a
    // period of `f32::MIN_POSITIVE` over a hundred-unit path asks for about ten
    // to the fortieth of them, and the position it accumulates them into stops
    // advancing long before that: at around two times ten to the minus
    // thirty-first, adding the interval to it is below the last bit of an `f32`
    // and the loop stops making progress while still emitting geometry every
    // turn. That was not slow, it did not finish, and it exhausted memory trying.
    //
    // Worth being straight about what this costs. A sub-resolution pattern with
    // an even duty cycle would ideally read as a line at half coverage, and this
    // draws it solid. The difference is a shade on something no caller can see
    // the shape of, and the alternative was a hang.
    // Stated positively and then negated, because `!(period > tolerance)` is the
    // negated comparison clippy refuses on a partially ordered type -- and it
    // refuses it for the reason that matters here: the two can be incomparable. A
    // `tolerance` of NaN makes `resolvable` false and returns the path unchanged,
    // which is the safe direction, where reading the comparison the other way
    // round would let it through.
    //
    // `period` has to be checked finite here and not merely large, and the reason
    // is that `is_usable` does not check *this* sum. It adds up the intervals as
    // the caller gave them; `cycle` doubles an odd-length pattern so that it
    // alternates, so the total the walk runs against can be twice the one that was
    // validated -- and `[f32::MAX, 118.0, 370.0]` sums to `f32::MAX`, which is
    // finite and positive, then doubles to infinity. `phase.rem_euclid(inf)` is
    // infinity, and the loop that normalizes the phase below subtracts intervals
    // from it forever.
    let resolvable = period.is_finite() && period > tolerance;
    if !resolvable {
        return path.clone();
    }

    let mut builder = PathBuilder::new().with_fill_rule(path.fill_rule());
    for polyline in flatten(path, tolerance) {
        if polyline.len() < 2 {
            continue;
        }
        walk(&polyline, &cycle, period, dash.phase, &mut builder);
    }
    builder.build()
}

/// Walk one polyline, emitting the drawn runs.
///
/// The state is a position in the pattern rather than a distance traveled,
/// because that is what has to survive from one segment to the next: a dash
/// crossing a vertex is one dash, and rebuilding the phase from total distance
/// at each vertex would accumulate the error of every segment before it.
fn walk(points: &[Vec2], cycle: &[f32], period: f32, phase: f32, out: &mut PathBuilder) {
    // Where in the cycle the path begins, as an index and a remainder.
    let mut remaining = phase.rem_euclid(period);
    let mut index = 0usize;
    while remaining >= cycle[index] {
        // Guarded rather than trusted. The comment here used to say that a cycle
        // of positive total length cannot spin forever and that `is_usable`
        // guarantees it before we arrive, and both halves were wrong: `is_usable`
        // sums the intervals as given while this walks the doubled cycle, and an
        // interval smaller than the last bit of `remaining` leaves it where it was
        // however many times it is subtracted. `dash_path` now refuses a
        // non-finite period, which is the case that made this loop immortal, and
        // this is what makes the loop's own termination not depend on that.
        let next = remaining - cycle[index];
        if next >= remaining {
            break;
        }
        remaining = next;
        index = (index + 1) % cycle.len();
    }
    // How much of the current interval is left to travel. Floored at zero because
    // the loop above can leave `remaining` larger than the interval it stopped on,
    // and a negative `left` would send the walk below backwards.
    let mut left = (cycle[index] - remaining).max(0.0);
    // Even indices are drawn, odd are skipped.
    let mut drawing = index % 2 == 0;
    let mut pen_down = false;

    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let segment = to - from;
        let length = segment.length();
        if !length.is_finite() || length <= 0.0 {
            continue;
        }
        let mut traveled = 0.0f32;
        while length - traveled > left {
            // The interval ends inside this segment.
            //
            // Guarded against an interval too small to move the position it is
            // added to. The check above cannot be relied on for this: it compares
            // the period against the tolerance, and a caller reaching `walk`
            // through another route, or a tolerance small enough to admit a
            // period this fine, would arrive here anyway. Termination should not
            // depend on either. Breaking leaves the rest of the segment to the
            // code below, which draws it as one run.
            let next = traveled + left;
            if next <= traveled {
                break;
            }
            traveled = next;
            let at = from + segment * (traveled / length);
            if drawing {
                if !pen_down {
                    out.move_to(from + segment * ((traveled - left) / length));
                }
                out.line_to(at);
                pen_down = false;
            }
            index = (index + 1) % cycle.len();
            drawing = !drawing;
            left = cycle[index];
            // A zero-length interval would leave `left` at zero and spin here.
            // The pattern's total is positive, so at most one interval in a row
            // can be zero, and stepping past it makes progress.
            if left <= 0.0 {
                index = (index + 1) % cycle.len();
                drawing = !drawing;
                left = cycle[index];
            }
        }
        // The rest of the segment lies inside the current interval.
        let rest = length - traveled;
        if drawing {
            if !pen_down {
                out.move_to(from + segment * (traveled / length));
                pen_down = true;
            }
            out.line_to(to);
        } else {
            pen_down = false;
        }
        left -= rest;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A horizontal line of the given length, starting at the origin.
    fn line(length: f32) -> Path {
        let mut builder = PathBuilder::new();
        builder.move_to(Vec2::ZERO);
        builder.line_to(Vec2::new(length, 0.0));
        builder.build()
    }

    /// Total length of every segment in a path of straight lines.
    fn drawn_length(path: &Path) -> f32 {
        flatten(path, 0.25)
            .iter()
            .flat_map(|line| line.windows(2))
            .map(|pair| (pair[1] - pair[0]).length())
            .sum()
    }

    #[test]
    fn a_pattern_draws_half_of_an_even_line() {
        // Ten on, ten off, across a hundred: five dashes of ten.
        let dashed = dash_path(&line(100.0), &Dash::new(vec![10.0, 10.0], 0.0), 0.25);
        assert!(
            (drawn_length(&dashed) - 50.0).abs() < 0.01,
            "drew {} of a hundred",
            drawn_length(&dashed)
        );
        assert_eq!(flatten(&dashed, 0.25).len(), 5, "expected five dashes");
    }

    #[test]
    fn an_odd_pattern_repeats_to_alternate() {
        // `[10]` means ten on and ten off, not ten on forever. Without the
        // doubling this draws the whole line, which is the bug the SVG rule
        // exists to prevent.
        let dashed = dash_path(&line(100.0), &Dash::new(vec![10.0], 0.0), 0.25);
        assert!(
            (drawn_length(&dashed) - 50.0).abs() < 0.01,
            "drew {}",
            drawn_length(&dashed)
        );
    }

    #[test]
    fn the_phase_moves_the_pattern_along_the_line() {
        // Started ten in, the first gap is where the first dash was, so the
        // line begins with a gap and ends with one more dash-worth drawn at the
        // far end. The total stays half either way; what changes is where.
        let plain = dash_path(&line(100.0), &Dash::new(vec![10.0, 10.0], 0.0), 0.25);
        let shifted = dash_path(&line(100.0), &Dash::new(vec![10.0, 10.0], 10.0), 0.25);
        let first_of = |p: &Path| flatten(p, 0.25)[0][0].x;
        assert!(first_of(&plain) < 0.01, "unshifted should start at zero");
        assert!(
            (first_of(&shifted) - 10.0).abs() < 0.01,
            "a phase of ten should start ten along, not at {}",
            first_of(&shifted)
        );
    }

    #[test]
    fn a_phase_beyond_one_period_wraps() {
        // Any phase is meaningful, so an animation can keep adding to it
        // without ever reaching a value that behaves differently.
        let once = dash_path(&line(100.0), &Dash::new(vec![10.0, 10.0], 5.0), 0.25);
        let again = dash_path(&line(100.0), &Dash::new(vec![10.0, 10.0], 25.0), 0.25);
        assert_eq!(
            flatten(&once, 0.25).len(),
            flatten(&again, 0.25).len(),
            "a phase one period further should repeat"
        );
    }

    #[test]
    fn a_dash_crossing_a_corner_stays_one_dash() {
        // Two segments meeting at a right angle, with a dash long enough to
        // span the join. Cutting per segment instead of carrying the phase
        // across would end the dash at the corner and start another, which
        // shows as a break exactly where a stroke is most visible.
        let mut builder = PathBuilder::new();
        builder.move_to(Vec2::ZERO);
        builder.line_to(Vec2::new(10.0, 0.0));
        builder.line_to(Vec2::new(10.0, 10.0));
        let dashed = dash_path(&builder.build(), &Dash::new(vec![30.0, 5.0], 0.0), 0.25);
        assert_eq!(
            flatten(&dashed, 0.25).len(),
            1,
            "the dash was cut at the corner"
        );
        assert!((drawn_length(&dashed) - 20.0).abs() < 0.01);
    }

    #[test]
    fn an_unusable_pattern_draws_the_path_whole() {
        // Each of these has no period to advance through. Drawing the line
        // undashed is visibly wrong in a way that leads back to the pattern;
        // drawing nothing is invisible and leads nowhere.
        for intervals in [vec![], vec![0.0, 0.0], vec![-4.0, 2.0], vec![f32::NAN]] {
            let dashed = dash_path(&line(100.0), &Dash::new(intervals.clone(), 0.0), 0.25);
            assert!(
                (drawn_length(&dashed) - 100.0).abs() < 0.01,
                "{intervals:?} should draw the whole line, drew {}",
                drawn_length(&dashed)
            );
        }
    }

    #[test]
    fn a_zero_length_interval_inside_a_usable_pattern_terminates() {
        // Reachable: a caller computing intervals can produce a zero among
        // nonzero ones, and the walk has to step over it rather than stand on
        // it. This test exists to fail by hanging rather than by asserting.
        let dashed = dash_path(
            &line(100.0),
            &Dash::new(vec![10.0, 0.0, 5.0, 5.0], 0.0),
            0.25,
        );
        assert!(drawn_length(&dashed) > 0.0);
        assert!(drawn_length(&dashed) < 100.0);
    }

    #[test]
    fn a_pattern_longer_than_the_path_draws_what_it_reaches() {
        // The first dash covers everything, so the line is drawn whole -- and
        // the opposite, where the path begins inside a gap that outlasts it,
        // draws nothing at all.
        let covered = dash_path(&line(10.0), &Dash::new(vec![100.0, 100.0], 0.0), 0.25);
        assert!((drawn_length(&covered) - 10.0).abs() < 0.01);

        let skipped = dash_path(&line(10.0), &Dash::new(vec![100.0, 100.0], 100.0), 0.25);
        assert_eq!(drawn_length(&skipped), 0.0, "a line inside a gap drew");
    }

    #[test]
    fn a_curve_is_dashed_along_its_length_rather_than_its_chord() {
        // A quarter circle of radius ten has an arc length of about 15.7,
        // against a chord of about 14.1. Dashing it half on and half off should
        // draw about half the arc, which is enough to tell the two apart.
        let mut builder = PathBuilder::new();
        builder.move_to(Vec2::new(10.0, 0.0));
        builder.cubic_to(
            Vec2::new(10.0, 5.523),
            Vec2::new(5.523, 10.0),
            Vec2::new(0.0, 10.0),
        );
        let arc = std::f32::consts::FRAC_PI_2 * 10.0;
        let dashed = dash_path(&builder.build(), &Dash::new(vec![1.0, 1.0], 0.0), 0.05);
        let drawn = drawn_length(&dashed);
        assert!(
            (drawn - arc / 2.0).abs() < 0.5,
            "drew {drawn} of an arc of {arc}"
        );
    }

    /// A pattern too fine to resolve is drawn solid rather than not at all.
    ///
    /// `is_usable` admits it: the intervals are finite, positive, and their sum is
    /// greater than zero. What it cannot see is how that sum compares with the
    /// path, and `walk` counts intervals rather than distance -- so a period of
    /// `f32::MIN_POSITIVE` over a hundred-unit line asks for about ten to the
    /// fortieth dashes. It never got that far. The position the intervals
    /// accumulate into stops advancing at around `2e-31`, where adding one is
    /// below the last bit of an `f32`, and from there the loop emitted geometry
    /// forever without moving. Memory ran out.
    ///
    /// Found by generating a `Paint` field by field, which is how a `Dash` this
    /// small is built at all.
    #[test]
    fn a_pattern_finer_than_the_tolerance_is_left_solid() {
        let path = line(100.0);
        for interval in [f32::MIN_POSITIVE, 1e-30, 1e-20, 1e-9] {
            let dash = Dash::new(vec![interval, interval], 0.0);
            assert!(
                dash.is_usable(),
                "an interval of {interval:e} is what this test is about, and \
                 `is_usable` is expected to admit it"
            );
            // Reaching the assertion at all is most of the point.
            let dashed = dash_path(&path, &dash, 0.1);
            assert_eq!(
                dashed.verbs().len(),
                path.verbs().len(),
                "an interval of {interval:e} is finer than the tolerance, so the \
                 path should come back as it went in"
            );
        }

        // And a pattern the tolerance can resolve is still dashed, so the guard
        // above did not turn dashing off for everything.
        let dashed = dash_path(&path, &Dash::new(vec![5.0, 5.0], 0.0), 0.1);
        assert!(
            dashed.verbs().len() > line(100.0).verbs().len(),
            "a five-unit dash over a hundred units produced no extra verbs"
        );
    }

    /// An odd-length pattern whose doubled cycle overflows is refused.
    ///
    /// `is_usable` sums the intervals as the caller gave them, and `cycle` doubles
    /// an odd-length pattern so that it alternates -- so the total the walk runs
    /// against is twice the one that was validated. `[f32::MAX, 118.0, 370.0]`
    /// sums to `f32::MAX`, which is finite and positive and passes every check
    /// `is_usable` makes, and doubles to infinity.
    ///
    /// Then `phase.rem_euclid(inf)` is infinity, and the loop that normalizes the
    /// phase subtracted intervals from it forever: infinity less `f32::MAX` is
    /// still infinity, so the index cycled and the condition never turned false.
    /// The comment on that loop had claimed `is_usable` ruled this out.
    ///
    /// Found by generating a `Paint` field by field, and only with a random seed:
    /// a fixed one had passed this file's own properties several times over.
    #[test]
    fn an_odd_pattern_whose_doubled_cycle_overflows_is_refused() {
        let path = line(100.0);
        // Odd length, so `cycle` doubles it, and a sum that doubles past what an
        // `f32` holds.
        let dash = Dash::new(vec![f32::MAX, 118.494_29, 370.615_72], -1e20);
        assert!(
            dash.is_usable(),
            "the intervals are finite, non-negative and sum above zero, which is \
             the whole of what `is_usable` asks -- this test is about what it does \
             not ask"
        );
        assert!(
            !dash.cycle().iter().sum::<f32>().is_finite(),
            "this case is only interesting while the doubled cycle overflows"
        );
        // Reaching the assertion at all is the point: this did not return.
        let dashed = dash_path(&path, &dash, 0.1);
        assert_eq!(
            dashed.verbs().len(),
            path.verbs().len(),
            "a pattern whose cycle does not sum to a length should come back \
             undashed"
        );
    }

    /// An interval below the last bit of the phase does not stall the walk.
    ///
    /// The other half of the same loop, and the half that does not depend on
    /// `dash_path` having refused anything: a large phase against a tiny first
    /// interval leaves `remaining` where it was however often it is subtracted.
    #[test]
    fn a_phase_far_past_a_tiny_interval_still_settles() {
        let path = line(100.0);
        // Period is 500-ish, so `dash_path` admits it, and the first interval is
        // far below the last bit of the phase inside it.
        let dash = Dash::new(vec![f32::MIN_POSITIVE, 500.0], 499.999_97);
        assert!(dash.is_usable());
        let _ = dash_path(&path, &dash, 0.1);
    }
}

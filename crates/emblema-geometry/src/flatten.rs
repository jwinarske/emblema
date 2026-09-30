//! Adaptive Bezier flattening.
//!
//! Curves are subdivided into line segments before tessellation. The question
//! is how many segments, and the answer has to be transform-aware: tolerance
//! is a screen-space quantity, so a path scaled up by a factor of ten needs
//! roughly three times the segments to stay within the same visual error.
//! Flattening at record time with the identity transform and reusing the
//! result under a larger scale is what produces visibly faceted curves.
//!
//! Segment counts come from Wang's formula, which bounds the error of
//! uniformly subdividing a Bezier curve using the magnitude of its second
//! differences. It is a closed-form estimate rather than a recursive
//! subdivision test, so the count is known before any work is done and the
//! output buffer can be sized once.

use crate::path::{Path, Verb};
use glam::Vec2;

/// Default flattening tolerance in device pixels.
///
/// A quarter pixel is below the threshold of visibility for curve facets while
/// keeping segment counts modest. Text at small sizes uses a tighter value.
pub const DEFAULT_TOLERANCE: f32 = 0.25;

/// Upper bound on segments for a single curve.
///
/// Wang's formula grows without limit as tolerance approaches zero or control
/// points diverge, and a pathological path should degrade rather than exhaust
/// memory. Hitting this cap means the curve is being drawn far outside any
/// sane viewport.
pub const MAX_SEGMENTS: u32 = 1000;

/// Segment count for a quadratic curve at a given tolerance.
///
/// Wang's formula for degree `n` bounds the error of `N` uniform subdivisions
/// by `n(n-1)·M / (8N²)`, where `M` is the largest second difference. Solving
/// for `N` at degree 2 gives `sqrt(M / (4·tol))`.
pub fn quad_segment_count(p0: Vec2, p1: Vec2, p2: Vec2, tolerance: f32) -> u32 {
    let second_diff = second_difference_magnitude(p0 - p1 * 2.0 + p2);
    segment_count(second_diff / (4.0 * tolerance.max(f32::MIN_POSITIVE)))
}

/// Segment count for a cubic curve at a given tolerance.
///
/// At degree 3 the same bound gives `sqrt(3·M / (4·tol))`, where `M` is the
/// larger of the two second differences.
pub fn cubic_segment_count(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, tolerance: f32) -> u32 {
    let d0 = second_difference_magnitude(p0 - p1 * 2.0 + p2);
    let d1 = second_difference_magnitude(p1 - p2 * 2.0 + p3);
    let m = d0.max(d1);
    segment_count(3.0 * m / (4.0 * tolerance.max(f32::MIN_POSITIVE)))
}

/// An upper bound on the length of a second-difference vector.
///
/// Deliberately not `Vec2::length`, which squares its components: control
/// points far outside the viewport overflow f32 in the square and produce
/// infinity, which then reads as a degenerate curve and collapses a very large
/// curve to a single segment — a straight line where a curve belongs.
///
/// The Chebyshev norm needs no squaring, and scaling it by `sqrt(2)` bounds
/// the Euclidean norm from above, so the resulting segment count errs toward
/// over-subdivision rather than under.
/// NaN is propagated rather than converted to infinity, so that garbage input
/// degrades to one segment instead of being subdivided maximally into a
/// thousand NaN points.
fn second_difference_magnitude(v: Vec2) -> f32 {
    if v.is_nan() {
        return f32::NAN;
    }
    v.abs().max_element() * std::f32::consts::SQRT_2
}

/// Shared tail of both formulas: take the root, round up, clamp.
fn segment_count(squared: f32) -> u32 {
    if squared.is_nan() || squared <= 0.0 {
        // A degenerate curve — coincident control points, or non-finite input
        // from a bad transform — is a single segment, not an error. There is
        // no curve to approximate.
        return 1;
    }
    if squared.is_infinite() {
        // Genuinely enormous rather than degenerate. Subdivide maximally; the
        // clamp keeps it bounded.
        return MAX_SEGMENTS;
    }
    (squared.sqrt().ceil() as u32).clamp(1, MAX_SEGMENTS)
}

/// Scale a tolerance into curve space given the transform's largest scale
/// factor.
///
/// Tolerance is specified in device pixels. A curve that will be scaled up
/// must be flattened more finely in its own space to land within tolerance
/// after transformation, so the tolerance shrinks by the scale factor.
pub fn tolerance_for_scale(tolerance: f32, max_scale: f32) -> f32 {
    if max_scale <= 0.0 || !max_scale.is_finite() {
        return tolerance;
    }
    tolerance / max_scale
}

/// Evaluate a quadratic Bezier at `t`.
pub fn eval_quad(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    p0 * (mt * mt) + p1 * (2.0 * mt * t) + p2 * (t * t)
}

/// Evaluate a cubic Bezier at `t`.
pub fn eval_cubic(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    let mt2 = mt * mt;
    let t2 = t * t;
    p0 * (mt2 * mt) + p1 * (3.0 * mt2 * t) + p2 * (3.0 * mt * t2) + p3 * (t2 * t)
}

/// Flatten a path into polylines, one per subpath.
///
/// Every emitted polyline starts at the subpath's first point and ends at its
/// last, exactly — endpoints are copied rather than evaluated, so a closed
/// subpath's ends coincide bit-for-bit and tessellation does not see a
/// hairline gap from float error.
pub fn flatten(path: &Path, tolerance: f32) -> Vec<Vec<Vec2>> {
    let mut out: Vec<Vec<Vec2>> = Vec::new();
    let mut current: Vec<Vec2> = Vec::new();
    let mut pen = Vec2::ZERO;
    let mut subpath_start = Vec2::ZERO;

    for (verb, points) in path.segments() {
        match verb {
            Verb::MoveTo => {
                if current.len() > 1 {
                    out.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                pen = points[0];
                subpath_start = pen;
                current.push(pen);
            }
            Verb::LineTo => {
                pen = points[0];
                current.push(pen);
            }
            Verb::QuadTo => {
                let (ctrl, to) = (points[0], points[1]);
                let n = quad_segment_count(pen, ctrl, to, tolerance);
                for i in 1..n {
                    let t = i as f32 / n as f32;
                    current.push(eval_quad(pen, ctrl, to, t));
                }
                current.push(to);
                pen = to;
            }
            Verb::CubicTo => {
                let (c0, c1, to) = (points[0], points[1], points[2]);
                let n = cubic_segment_count(pen, c0, c1, to, tolerance);
                for i in 1..n {
                    let t = i as f32 / n as f32;
                    current.push(eval_cubic(pen, c0, c1, to, t));
                }
                current.push(to);
                pen = to;
            }
            Verb::Close => {
                if current.first() != Some(&subpath_start) || current.len() < 2 {
                    // Nothing meaningful to close.
                } else if current.last() != Some(&subpath_start) {
                    current.push(subpath_start);
                }
                pen = subpath_start;
            }
        }
    }
    if current.len() > 1 {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::PathBuilder;

    /// Closing a contour that holds only a `move_to` produces no contour.
    ///
    /// Upstream keeps a scene on this -- a path that walks three points, moves
    /// back to the first and closes -- with a comment saying the close must not
    /// add a second nearly-empty contour. The scene cannot be mirrored in the
    /// catalog, because a `Shape` states its contours all-open or all-closed
    /// and this path is one of each; but the invariant it guards is a question
    /// about the builder rather than about a picture, and this is where it can
    /// be asked directly.
    ///
    /// The verbs are asserted alongside the count so the test cannot pass for
    /// the wrong reason. Without them, a builder that quietly dropped the
    /// trailing `move_to` would satisfy the count while no longer exercising
    /// the case, and the test would go on passing having stopped meaning
    /// anything.
    #[test]
    fn a_close_after_a_lone_move_to_adds_no_contour() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 400.0))
            .line_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(400.0, 0.0))
            .move_to(Vec2::new(0.0, 400.0));
        b.close();
        let path = b.build();

        assert_eq!(
            path.verbs(),
            [
                Verb::MoveTo,
                Verb::LineTo,
                Verb::LineTo,
                Verb::MoveTo,
                Verb::Close
            ],
            "the builder should still be holding the case this is about"
        );

        let contours = flatten(&path, 0.1);
        assert_eq!(
            contours.len(),
            1,
            "the move_to and close describe a contour with no extent, which is \
             nothing to walk; got {contours:?}"
        );
        assert_eq!(
            contours[0],
            [
                Vec2::new(0.0, 400.0),
                Vec2::new(0.0, 0.0),
                Vec2::new(400.0, 0.0)
            ]
        );
    }

    #[test]
    fn a_straight_curve_needs_one_segment() {
        // Collinear control points have zero second difference, so no
        // subdivision is warranted regardless of how tight the tolerance is.
        let n = quad_segment_count(
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            0.001,
        );
        assert_eq!(n, 1);
    }

    #[test]
    fn tighter_tolerance_never_reduces_segment_count() {
        let (p0, p1, p2) = (
            Vec2::new(0.0, 0.0),
            Vec2::new(50.0, 100.0),
            Vec2::new(100.0, 0.0),
        );
        let mut previous = 0;
        for tol in [4.0, 2.0, 1.0, 0.5, 0.25, 0.1] {
            let n = quad_segment_count(p0, p1, p2, tol);
            assert!(
                n >= previous,
                "tolerance {tol} gave {n} segments, less than the looser {previous}"
            );
            previous = n;
        }
    }

    #[test]
    fn cubics_need_at_least_as_many_segments_as_the_matching_quad() {
        // Same curvature scale, higher degree: the degree-3 coefficient is
        // larger, so the cubic must not come out coarser.
        let (p0, p1, p2) = (
            Vec2::new(0.0, 0.0),
            Vec2::new(50.0, 100.0),
            Vec2::new(100.0, 0.0),
        );
        let quad = quad_segment_count(p0, p1, p2, 0.25);
        let cubic = cubic_segment_count(p0, p1, p1, p2, 0.25);
        assert!(cubic >= quad, "cubic {cubic} < quad {quad}");
    }

    #[test]
    fn degenerate_input_yields_one_segment_rather_than_an_error() {
        let z = Vec2::ZERO;
        assert_eq!(quad_segment_count(z, z, z, 0.25), 1);
        assert_eq!(cubic_segment_count(z, z, z, z, 0.25), 1);
        // A non-finite control point, as a bad transform can produce.
        let nan = Vec2::new(f32::NAN, 0.0);
        assert_eq!(quad_segment_count(z, nan, z, 0.25), 1);
    }

    #[test]
    fn segment_count_is_capped_for_pathological_input() {
        let far = Vec2::new(1e30, 1e30);
        let n = cubic_segment_count(Vec2::ZERO, far, -far, Vec2::ZERO, 1e-6);
        assert_eq!(n, MAX_SEGMENTS);
    }

    #[test]
    fn scaling_up_tightens_tolerance_proportionally() {
        // A path drawn at 10x must flatten ten times finer in its own space to
        // land within the same device-space error.
        assert_eq!(tolerance_for_scale(0.25, 10.0), 0.025);
        // Degenerate scales leave tolerance alone rather than producing
        // infinities.
        assert_eq!(tolerance_for_scale(0.25, 0.0), 0.25);
        assert_eq!(tolerance_for_scale(0.25, f32::NAN), 0.25);
    }

    #[test]
    fn flattening_preserves_exact_endpoints() {
        let start = Vec2::new(3.0, 7.0);
        let end = Vec2::new(11.0, 2.0);
        let mut b = PathBuilder::new();
        b.move_to(start)
            .cubic_to(Vec2::new(5.0, 20.0), Vec2::new(9.0, -6.0), end);
        let lines = flatten(&b.build(), DEFAULT_TOLERANCE);

        assert_eq!(lines.len(), 1);
        // Copied, not evaluated: an evaluated endpoint would differ in the low
        // bits and leave a hairline gap at a closed subpath's seam.
        assert_eq!(lines[0].first(), Some(&start));
        assert_eq!(lines[0].last(), Some(&end));
    }

    #[test]
    fn closing_makes_the_last_point_equal_the_first() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(10.0, 0.0))
            .line_to(Vec2::new(10.0, 10.0))
            .close();
        let lines = flatten(&b.build(), DEFAULT_TOLERANCE);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].first(), lines[0].last());
    }

    #[test]
    fn each_subpath_becomes_its_own_polyline() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(1.0, 0.0))
            .move_to(Vec2::new(5.0, 5.0))
            .line_to(Vec2::new(6.0, 5.0))
            .line_to(Vec2::new(6.0, 6.0));
        let lines = flatten(&b.build(), DEFAULT_TOLERANCE);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[1].len(), 3);
    }

    #[test]
    fn a_lone_move_produces_no_polyline() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(4.0, 4.0));
        // A subpath with no extent has nothing to fill or stroke, so emitting
        // a one-point polyline would only give tessellation a degenerate case
        // to handle.
        assert!(flatten(&b.build(), DEFAULT_TOLERANCE).is_empty());
    }

    #[test]
    fn flattened_points_stay_within_the_control_hull() {
        let p0 = Vec2::new(0.0, 0.0);
        let c0 = Vec2::new(0.0, 100.0);
        let c1 = Vec2::new(100.0, 100.0);
        let p3 = Vec2::new(100.0, 0.0);
        let mut b = PathBuilder::new();
        b.move_to(p0).cubic_to(c0, c1, p3);
        let path = b.build();
        let bounds = path.bounds();

        for line in flatten(&path, DEFAULT_TOLERANCE) {
            for p in line {
                // A Bezier lies within the convex hull of its control points,
                // so a sample outside the bounding box means the evaluator is
                // wrong.
                assert!(bounds.contains(p), "{p:?} escaped {bounds:?}");
            }
        }
    }
}

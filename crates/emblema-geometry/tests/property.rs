//! Property tests for flattening invariants.
//!
//! These check the claims the tessellator relies on, over generated input
//! rather than chosen examples. Coordinates are bounded to a range a real
//! viewport might see: the degenerate and overflow cases have their own unit
//! tests, and mixing them in here would only rediscover those.

use emblema_geometry::flatten::{
    cubic_segment_count, eval_cubic, eval_quad, quad_segment_count, tolerance_for_scale,
    DEFAULT_TOLERANCE, MAX_SEGMENTS,
};
use emblema_geometry::tessellate::{covered_area, polygon_area, Tessellator};
use emblema_geometry::{flatten, max_scale, PathBuilder};
use glam::{Affine2, Vec2};
use proptest::prelude::*;

/// Coordinates within a generous but finite viewport.
fn coord() -> impl Strategy<Value = f32> {
    -10_000.0f32..10_000.0f32
}

fn point() -> impl Strategy<Value = Vec2> {
    (coord(), coord()).prop_map(|(x, y)| Vec2::new(x, y))
}

/// Tolerances spanning the useful range, from coarse to text-quality.
fn tolerance() -> impl Strategy<Value = f32> {
    0.01f32..4.0f32
}

/// Coordinates for the tests that also scale, small enough that the product
/// stays where `f32` resolves a fraction of a pixel. The full range at a scale
/// of fifty reaches half a million, where the float spacing is a twentieth of a
/// pixel and a quarter-pixel bound measures the arithmetic.
fn small_coord() -> impl Strategy<Value = f32> {
    -1_000.0f32..1_000.0f32
}

fn small_point() -> impl Strategy<Value = Vec2> {
    (small_coord(), small_coord()).prop_map(|(x, y)| Vec2::new(x, y))
}

/// The largest singular value of an affine's linear part, exactly: the
/// reference [`max_scale`] estimates. From the eigenvalues of `M^T M`, whose
/// trace is the squared Frobenius norm.
///
/// In the test rather than the library because `max_scale` exists to avoid
/// computing it per draw.
fn largest_singular_value(transform: &Affine2) -> f32 {
    let m = transform.matrix2;
    let frobenius_squared = m.x_axis.length_squared() + m.y_axis.length_squared();
    let determinant = m.determinant();
    let discriminant =
        (frobenius_squared * frobenius_squared - 4.0 * determinant * determinant).max(0.0);
    ((frobenius_squared + discriminant.sqrt()) * 0.5).sqrt()
}

/// Distance from a point to a line segment.
fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let length_squared = ab.length_squared();
    if length_squared <= 0.0 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / length_squared).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// The furthest the curve strays from the polyline approximating it.
///
/// Sampled rather than solved -- the distance from a cubic to a polyline has no
/// closed form. Sampling the *curve* is the direction that matters: a dropped
/// segment leaves a curve point with nothing near it.
fn worst_deviation(curve: impl Fn(f32) -> Vec2, polyline: &[Vec2]) -> f32 {
    const SAMPLES: usize = 512;
    let mut worst = 0.0f32;
    for i in 0..=SAMPLES {
        let t = i as f32 / SAMPLES as f32;
        let p = curve(t);
        let mut nearest = f32::INFINITY;
        for pair in polyline.windows(2) {
            nearest = nearest.min(distance_to_segment(p, pair[0], pair[1]));
        }
        worst = worst.max(nearest);
    }
    worst
}

/// Float slack for a distance at these coordinates: the magnitude's own
/// resolution, so the allowance does not depend on the generated scale.
fn float_slack(points: &[Vec2]) -> f32 {
    let magnitude = points
        .iter()
        .map(|p| p.abs().max_element())
        .fold(0.0f32, f32::max);
    (magnitude * f32::EPSILON * 16.0).max(1e-3)
}

proptest! {
    /// A Bezier lies within the convex hull of its control points, so every
    /// flattened sample must fall inside their bounding box. A violation means
    /// the evaluator is wrong.
    #[test]
    fn flattened_samples_stay_within_the_control_hull(
        p0 in point(), c0 in point(), c1 in point(), p3 in point(),
        tol in tolerance(),
    ) {
        let mut b = PathBuilder::new();
        b.move_to(p0).cubic_to(c0, c1, p3);
        let path = b.build();
        let bounds = path.bounds();

        for line in flatten(&path, tol) {
            for p in line {
                // A small epsilon absorbs rounding in the evaluation, not
                // errors of substance.
                prop_assert!(
                    p.x >= bounds.min.x - 0.01 && p.x <= bounds.max.x + 0.01
                        && p.y >= bounds.min.y - 0.01 && p.y <= bounds.max.y + 0.01,
                    "sample {p:?} escaped {bounds:?}"
                );
            }
        }
    }

    /// Endpoints are copied rather than evaluated, so they must survive
    /// flattening bit-for-bit. Anything less leaves hairline gaps where
    /// subpaths meet.
    #[test]
    fn endpoints_survive_flattening_exactly(
        p0 in point(), c0 in point(), c1 in point(), p3 in point(),
        tol in tolerance(),
    ) {
        let mut b = PathBuilder::new();
        b.move_to(p0).cubic_to(c0, c1, p3);
        let lines = flatten(&b.build(), tol);

        prop_assert_eq!(lines.len(), 1);
        prop_assert_eq!(lines[0].first(), Some(&p0));
        prop_assert_eq!(lines[0].last(), Some(&p3));
    }

    /// Refining tolerance must never produce a coarser approximation.
    #[test]
    fn segment_count_is_monotonic_in_tolerance(
        p0 in point(), p1 in point(), p2 in point(),
        coarse in 0.5f32..4.0f32,
        factor in 1.0f32..20.0f32,
    ) {
        let fine = coarse / factor;
        prop_assert!(
            quad_segment_count(p0, p1, p2, fine) >= quad_segment_count(p0, p1, p2, coarse)
        );
        prop_assert!(
            cubic_segment_count(p0, p1, p1, p2, fine)
                >= cubic_segment_count(p0, p1, p1, p2, coarse)
        );
    }

    /// Flattening must land within its tolerance of the curve.
    ///
    /// The three properties above are about where the samples are, not how
    /// many, so a flattener undercounting its segments satisfies all of them.
    ///
    /// Skipped at `MAX_SEGMENTS`, where the cap and not the tolerance decides.
    #[test]
    fn flattening_lands_within_tolerance_of_the_curve(
        p0 in point(), c0 in point(), c1 in point(), p3 in point(),
        tol in tolerance(),
    ) {
        prop_assume!(cubic_segment_count(p0, c0, c1, p3, tol) < MAX_SEGMENTS);
        let mut b = PathBuilder::new();
        b.move_to(p0).cubic_to(c0, c1, p3);
        let lines = flatten(&b.build(), tol);
        prop_assert_eq!(lines.len(), 1);

        let worst = worst_deviation(|t| eval_cubic(p0, c0, c1, p3, t), &lines[0]);
        let slack = float_slack(&[p0, c0, c1, p3]);
        prop_assert!(
            worst <= tol + slack,
            "flattening at tolerance {} left the curve by {} (slack {})",
            tol, worst, slack
        );
    }

    /// And the same bound after the transform, which is the space the tolerance
    /// is in: the renderer shrinks it by the largest scale, flattens in path
    /// space, then transforms.
    ///
    /// A similarity, which is the family where `max_scale` is exact. The
    /// skewed case is below.
    #[test]
    fn flattening_lands_within_tolerance_after_a_similarity(
        p0 in small_point(), c0 in small_point(), c1 in small_point(), p3 in small_point(),
        tol in tolerance(),
        scale in 0.05f32..50.0,
        angle in 0.0f32..std::f32::consts::TAU,
        translate in small_point(),
    ) {
        let transform = Affine2::from_translation(translate)
            * Affine2::from_angle(angle)
            * Affine2::from_scale(Vec2::splat(scale));
        let path_tolerance = tolerance_for_scale(tol, max_scale(&transform));
        prop_assume!(cubic_segment_count(p0, c0, c1, p3, path_tolerance) < MAX_SEGMENTS);

        let mut b = PathBuilder::new();
        b.move_to(p0).cubic_to(c0, c1, p3);
        let lines = flatten(&b.build(), path_tolerance);
        prop_assert_eq!(lines.len(), 1);

        // Both sides into device space: the polyline the renderer would draw,
        // and the curve it is meant to approximate.
        let device: Vec<Vec2> = lines[0].iter().map(|p| transform.transform_point2(*p)).collect();
        let worst = worst_deviation(
            |t| transform.transform_point2(eval_cubic(p0, c0, c1, p3, t)),
            &device,
        );
        let slack = float_slack(&device);
        prop_assert!(
            worst <= tol + slack,
            "at scale {} the flattening left the curve by {} in device space, \
             past a tolerance of {} (slack {})",
            scale, worst, tol, slack
        );
    }

    /// `max_scale` bounds the true largest singular value from below, by at
    /// most `sqrt(2)`. Its own doc comment says so; this checks it, because the
    /// bound above rests on it.
    ///
    /// Both halves matter: above the true value only wastes segments, while
    /// more than `sqrt(2)` below leaves a curve outside its tolerance.
    #[test]
    fn max_scale_is_within_root_two_below_the_true_largest_stretch(
        a in -50.0f32..50.0, b in -50.0f32..50.0,
        c in -50.0f32..50.0, d in -50.0f32..50.0,
    ) {
        let transform = Affine2::from_cols(Vec2::new(a, b), Vec2::new(c, d), Vec2::ZERO);
        let estimate = max_scale(&transform);
        let truth = largest_singular_value(&transform);
        let slack = truth * 1e-4 + 1e-4;
        prop_assert!(
            estimate <= truth + slack,
            "max_scale {estimate} exceeded the true largest stretch {truth}"
        );
        prop_assert!(
            estimate * std::f32::consts::SQRT_2 + slack >= truth,
            "max_scale {estimate} fell more than root two below {truth}"
        );
    }

    /// Under a skew the estimate is low, so the device-space error may exceed
    /// the tolerance by up to `sqrt(2)` -- the accepted cost of not
    /// decomposing per draw. A quarter pixel can show as a third. Exceeding
    /// the factor is the defect.
    #[test]
    fn a_skew_costs_no_more_than_the_stated_factor(
        p0 in small_point(), c0 in small_point(), c1 in small_point(), p3 in small_point(),
        tol in tolerance(),
        sx in 0.1f32..20.0, sy in 0.1f32..20.0,
        skew in -4.0f32..4.0,
        angle in 0.0f32..std::f32::consts::TAU,
    ) {
        let mut linear = Affine2::from_angle(angle) * Affine2::from_scale(Vec2::new(sx, sy));
        linear.matrix2.y_axis.x += skew;
        let path_tolerance = tolerance_for_scale(tol, max_scale(&linear));
        prop_assume!(cubic_segment_count(p0, c0, c1, p3, path_tolerance) < MAX_SEGMENTS);

        let mut b = PathBuilder::new();
        b.move_to(p0).cubic_to(c0, c1, p3);
        let lines = flatten(&b.build(), path_tolerance);
        prop_assert_eq!(lines.len(), 1);

        let device: Vec<Vec2> = lines[0].iter().map(|p| linear.transform_point2(*p)).collect();
        let worst = worst_deviation(
            |t| linear.transform_point2(eval_cubic(p0, c0, c1, p3, t)),
            &device,
        );
        let allowed = tol * std::f32::consts::SQRT_2 + float_slack(&device);
        prop_assert!(
            worst <= allowed,
            "a skew left the curve by {} in device space, past the {} this is allowed",
            worst, allowed
        );
    }

    /// Curve evaluation must reproduce the endpoints exactly at the parameter
    /// bounds, since the flattener relies on it when sampling interior points.
    #[test]
    fn curves_interpolate_their_endpoints(
        p0 in point(), c0 in point(), c1 in point(), p3 in point(),
    ) {
        prop_assert_eq!(eval_quad(p0, c0, p3, 0.0), p0);
        prop_assert_eq!(eval_quad(p0, c0, p3, 1.0), p3);
        prop_assert_eq!(eval_cubic(p0, c0, c1, p3, 0.0), p0);
        prop_assert_eq!(eval_cubic(p0, c0, c1, p3, 1.0), p3);
    }

    /// Closing a subpath must make its ends coincide exactly, whatever the
    /// geometry, so the tessellator sees a genuinely closed loop.
    #[test]
    fn closed_subpaths_have_coincident_ends(
        p0 in point(), p1 in point(), p2 in point(),
    ) {
        let mut b = PathBuilder::new();
        b.move_to(p0).line_to(p1).line_to(p2).close();
        let lines = flatten(&b.build(), DEFAULT_TOLERANCE);

        prop_assert_eq!(lines.len(), 1);
        prop_assert_eq!(lines[0].first(), lines[0].last());
    }

    /// Tessellating a convex polygon must cover its area and no more. This is
    /// the assertion that catches a fan applied to something that is not
    /// actually convex, which would paint outside the path.
    #[test]
    fn convex_tessellation_covers_exactly_the_polygon_area(
        n in 3usize..24,
        rx in 1.0f32..100.0f32,
        ry in 1.0f32..100.0f32,
        cx in -50.0f32..50.0f32,
        cy in -50.0f32..50.0f32,
    ) {
        // Sampling an ellipse at evenly spaced angles is convex for any axis
        // lengths. Varying the radius per vertex instead would not be:
        // alternating long and short radii produces a star, which is concave,
        // and the test would then be asserting the wrong thing.
        let center = Vec2::new(cx, cy);
        let pts: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                center + Vec2::new(a.cos() * rx, a.sin() * ry)
            })
            .collect();

        let mut b = PathBuilder::new();
        b.move_to(pts[0]);
        for p in &pts[1..] {
            b.line_to(*p);
        }
        b.close();

        let mut t = Tessellator::new();
        let buffers = t.fill(&b.build(), DEFAULT_TOLERANCE);
        prop_assert!(buffers.is_well_formed());

        let expected = polygon_area(&pts);
        let actual = covered_area(buffers);
        prop_assert!(
            (actual - expected).abs() <= expected * 0.01 + 0.01,
            "covered {actual}, polygon is {expected}"
        );
    }

    /// Whatever the input, index buffers must stay in bounds and whole. A
    /// malformed buffer is an out-of-range read on the GPU, not a visual
    /// artifact.
    #[test]
    fn tessellation_always_produces_well_formed_buffers(
        pts in prop::collection::vec(point(), 3..40),
    ) {
        let mut b = PathBuilder::new();
        b.move_to(pts[0]);
        for p in &pts[1..] {
            b.line_to(*p);
        }
        b.close();

        let mut t = Tessellator::new();
        prop_assert!(t.fill(&b.build(), DEFAULT_TOLERANCE).is_well_formed());
    }

    /// Every verb's point count must match what the builder pushed, or the
    /// parallel buffers desynchronize and every later segment reads the wrong
    /// points.
    #[test]
    fn verb_and_point_buffers_stay_in_step(
        pts in prop::collection::vec(point(), 1..32),
    ) {
        let mut b = PathBuilder::new();
        b.move_to(pts[0]);
        for chunk in pts[1..].chunks(3) {
            match chunk.len() {
                3 => { b.cubic_to(chunk[0], chunk[1], chunk[2]); }
                2 => { b.quad_to(chunk[0], chunk[1]); }
                _ => { b.line_to(chunk[0]); }
            }
        }
        let path = b.build();

        let declared: usize = path.verbs().iter().map(|v| v.point_count()).sum();
        prop_assert_eq!(declared, path.points().len());
        // The walk must consume the buffer exactly, neither panicking nor
        // leaving a tail.
        prop_assert_eq!(path.segments().count(), path.verbs().len());
    }
}

/// What the ear-clipping fast path accepts must fill the same as the general route.
///
/// `ear_fill`'s verification is the only thing standing between a wrong fill and the
/// framebuffer, and the hand-written cases beside it do not prove it: a bowtie, a
/// pentagram and an hourglass are all rejected by the *triangle count*, because
/// `earcutr` bails early on invalid input and returns fewer than `n - 2`. The
/// edge-parity half of the check caught none of them.
///
/// So the property is stated against the route it replaces. Contours are generated on
/// a small integer lattice, where self-intersection is common rather than rare -- most
/// of these are not simple polygons. For every one the fast path *accepts*, the area
/// it fills must equal what lyon fills under the non-zero rule, which is the thing the
/// renderer would otherwise have drawn.
mod ear_clipping_agrees {
    use emblema_geometry::path::{FillRule, Path};
    use emblema_geometry::tessellate::Tessellator;
    use glam::Vec2;
    use proptest::prelude::*;

    fn area(vertices: &[Vec2], indices: &[u32]) -> f32 {
        indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (
                    vertices[t[0] as usize],
                    vertices[t[1] as usize],
                    vertices[t[2] as usize],
                );
                ((b - a).perp_dot(c - a) * 0.5).abs()
            })
            .sum()
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

        #[test]
        fn an_accepted_contour_fills_what_lyon_fills(
            points in prop::collection::vec((0i32..12, 0i32..12), 3..14),
        ) {
            let contour: Vec<Vec2> = points
                .iter()
                .map(|&(x, y)| Vec2::new(x as f32 * 10.0, y as f32 * 10.0))
                .collect();

            let mut builder = Path::builder().with_fill_rule(FillRule::NonZero);
            builder.move_to(contour[0]);
            for p in &contour[1..] {
                builder.line_to(*p);
            }
            builder.close();
            let path = builder.build();

            let mut tess = Tessellator::new();
            let filled = tess.fill(&path, 0.25).clone();

            // Whichever route ran, the fill is addressable and finite.
            for index in &filled.indices {
                prop_assert!((*index as usize) < filled.vertices.len());
            }
            for v in &filled.vertices {
                prop_assert!(v.is_finite());
            }

            // Either fast path writes exactly the contour's own vertices and `n - 2`
            // triangles, and lyon adds vertices of its own, so this says a fast path
            // ran without saying which. That is the right granularity: the fan and the
            // ear clipper both owe the general route the same area. What pins that ear
            // clipping is reached at all is a unit test, `tessellate.rs`'s
            // `fill_sends_a_concave_contour_to_ear_clipping`.
            let n = contour.len();
            let took_fast_path = filled.vertices.len() == n
                && filled.indices.len() == (n - 2) * 3
                && filled.vertices == contour;
            if took_fast_path {
                let mut general = Tessellator::new();
                let lyon = general.fill_general(&path, 0.25).clone();
                let (fast, slow) = (area(&filled.vertices, &filled.indices), area(&lyon.vertices, &lyon.indices));
                prop_assert!(
                    (fast - slow).abs() <= 0.01 * slow.max(1.0),
                    "ear clipping accepted a contour it fills differently: {fast} against {slow} for {contour:?}"
                );
            }
        }
    }
}

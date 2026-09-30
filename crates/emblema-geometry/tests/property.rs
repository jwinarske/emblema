//! Property tests for flattening invariants.
//!
//! These check the claims the tessellator relies on, over generated input
//! rather than chosen examples. Coordinates are bounded to a range a real
//! viewport might see: the degenerate and overflow cases have their own unit
//! tests, and mixing them in here would only rediscover those.

use emblema_geometry::flatten::{
    cubic_segment_count, eval_cubic, eval_quad, quad_segment_count, DEFAULT_TOLERANCE,
};
use emblema_geometry::tessellate::{covered_area, polygon_area, Tessellator};
use emblema_geometry::{flatten, PathBuilder};
use glam::Vec2;
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

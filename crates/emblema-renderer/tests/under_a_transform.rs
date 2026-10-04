//! Stroking and filling through the renderer, under a generated transform.
//!
//! `emblema-geometry`'s generated tests stop at the tessellator, which works in
//! path space and never sees a transform. The transform enters on either side:
//! ahead, deciding the flattening tolerance, and behind, carrying vertices into
//! clip space. Neither half was generated -- this crate had no generated tests.
//!
//! Two claims, the pair `hostile.rs` uses one rung down. Given anything at all,
//! the buffers come back addressable; an index past the end is a read the
//! driver performs on this process's behalf. Given input a caller would write,
//! every clip-space vertex is finite, and there is nothing to blame for one
//! that is not.
//!
//! Where a property's subject is the transform, the path stays tame. A hostile
//! path is refused at the boundary and comes back empty, which left 3 cases in
//! 256 with any geometry in them and passed while checking nothing.
//!
//! Clip-space positions are undivided -- the rasterizer needs `w` to clip
//! against the plane where it vanishes -- so "finite" is about all three
//! components.

use emblema_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
use emblema_geometry::{Path, PathBuilder, Transform2D};
use emblema_hal::Extent2D;
use emblema_renderer::{Renderer, TOLERANCE};
use glam::{Affine2, Vec2};
use proptest::prelude::*;

const TARGET: Extent2D = Extent2D {
    width: 256,
    height: 256,
};

/// Coordinates a caller would write: inside the tessellation range.
fn tame_coord() -> impl Strategy<Value = f32> + Clone {
    -2_000.0f32..2_000.0f32
}

/// And coordinates nobody meant -- the same set `hostile.rs` uses.
fn hostile_coord() -> impl Strategy<Value = f32> + Clone {
    prop_oneof![
        8 => tame_coord(),
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::NEG_INFINITY),
        1 => Just(0.0f32),
        1 => Just(f32::MIN_POSITIVE),
        1 => Just(f32::MAX),
        1 => Just(f32::MIN),
        1 => Just(1e30f32),
    ]
}

/// A transform built from parts, the way a scene graph builds one. Nine free
/// entries would spend most cases on matrices no caller can produce.
fn transform(coord: impl Strategy<Value = f32> + Clone) -> impl Strategy<Value = Transform2D> {
    (
        (coord.clone(), coord.clone()),
        0.0f32..std::f32::consts::TAU,
        coord.clone(),
        (coord.clone(), coord),
    )
        .prop_map(|((sx, sy), angle, skew, (tx, ty))| {
            let mut affine = Affine2::from_angle(angle) * Affine2::from_scale(Vec2::new(sx, sy));
            affine.matrix2.y_axis.x += skew;
            affine.translation = Vec2::new(tx, ty);
            Transform2D::from(affine)
        })
}

/// Scales covering both ends of the division that derives the tolerance: a
/// large one drives it toward the denormals, a small one past the size of the
/// path, where a curve becomes one segment.
fn scale() -> impl Strategy<Value = f32> {
    prop_oneof![
        6 => 1e-3f32..1e3,
        1 => Just(0.0f32),
        1 => Just(f32::MIN_POSITIVE),
        1 => Just(1e20f32),
        1 => Just(f32::MAX),
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
    ]
}

/// Widths that are mostly real, with the degenerate ones mixed in. A width of
/// NaN or zero produces no geometry, and the subject here is the transform.
fn hostile_width() -> impl Strategy<Value = f32> + Clone {
    prop_oneof![
        8 => 0.1f32..100.0,
        1 => Just(0.0f32),
        1 => Just(-1.0f32),
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::MAX),
        1 => Just(f32::MIN_POSITIVE),
    ]
}

fn stroke_style(width: impl Strategy<Value = f32>) -> impl Strategy<Value = StrokeStyle> {
    (
        width,
        prop_oneof![
            Just(LineCap::Butt),
            Just(LineCap::Round),
            Just(LineCap::Square)
        ],
        prop_oneof![
            Just(LineJoin::Miter),
            Just(LineJoin::Round),
            Just(LineJoin::Bevel)
        ],
        0.0f32..8.0,
    )
        .prop_map(|(width, cap, join, miter_limit)| StrokeStyle {
            width,
            cap,
            join,
            miter_limit,
        })
}

/// A path with a curve in it, which is the part a tolerance acts on. A
/// polyline would make every tolerance here equivalent.
fn path(coord: impl Strategy<Value = f32> + Clone) -> impl Strategy<Value = Path> {
    let point = (coord.clone(), coord).prop_map(|(x, y)| Vec2::new(x, y));
    prop::collection::vec(point, 2..10).prop_map(|points| {
        let mut b = PathBuilder::new();
        b.move_to(points[0]);
        for window in points[1..].chunks(3) {
            match window {
                [a, b2, c] => {
                    b.cubic_to(*a, *b2, *c);
                }
                [a, b2] => {
                    b.quad_to(*a, *b2);
                }
                [a] => {
                    b.line_to(*a);
                }
                _ => unreachable!("chunks(3) yields one, two or three"),
            }
        }
        b.close();
        b.build()
    })
}

fn renderer() -> Renderer {
    let mut renderer = Renderer::new();
    renderer.begin_frame(TARGET, TOLERANCE);
    renderer
}

proptest! {
    /// Whatever the transform, a stroke comes back addressable.
    #[test]
    fn a_stroke_under_any_transform_leaves_addressable_buffers(
        path in path(tame_coord()),
        style in stroke_style(hostile_width()),
        transform in transform(hostile_coord()),
    ) {
        let mut renderer = renderer();
        let geometry = renderer.stroke_path(&path, &style, None, transform);
        let vertices = geometry.vertices.len();
        for &index in geometry.indices {
            prop_assert!(
                (index as usize) < vertices,
                "a stroke under {transform:?} named vertex {index} of {vertices}"
            );
        }
    }

    /// And a fill, which takes the same two steps around a different middle.
    #[test]
    fn a_fill_under_any_transform_leaves_addressable_buffers(
        path in path(tame_coord()),
        transform in transform(hostile_coord()),
    ) {
        let mut renderer = renderer();
        let geometry = renderer.fill_path(&path, transform);
        let vertices = geometry.vertices.len();
        for &index in geometry.indices {
            prop_assert!(
                (index as usize) < vertices,
                "a fill under {transform:?} named vertex {index} of {vertices}"
            );
        }
    }

    /// A scale alone, since a composed transform reaches a given scale rarely
    /// and the scale is what feeds `tolerance_for_scale`.
    #[test]
    fn any_scale_leaves_addressable_buffers(
        path in path(tame_coord()),
        style in stroke_style(0.0f32..100.0),
        sx in scale(),
        sy in scale(),
    ) {
        let mut renderer = renderer();
        let transform = Transform2D::from(Affine2::from_scale(Vec2::new(sx, sy)));
        let geometry = renderer.stroke_path(&path, &style, None, transform);
        let vertices = geometry.vertices.len();
        for &index in geometry.indices {
            prop_assert!(
                (index as usize) < vertices,
                "a stroke at scale ({sx}, {sy}) named vertex {index} of {vertices}"
            );
        }
    }

    /// Input a caller would write must reach clip space finite. The three
    /// above allow garbage out for garbage in; this one does not.
    #[test]
    fn tame_input_reaches_clip_space_finite(
        path in path(tame_coord()),
        style in stroke_style(0.1f32..40.0),
        transform in transform(-50.0f32..50.0),
    ) {
        prop_assume!(transform.is_finite());
        let mut renderer = renderer();
        let geometry = renderer.stroke_path(&path, &style, None, transform);
        for v in geometry.vertices {
            prop_assert!(
                v.is_finite(),
                "a stroke under {transform:?} put {v:?} in clip space"
            );
        }
    }

    /// And the same for a fill.
    #[test]
    fn tame_input_fills_to_finite_clip_space(
        path in path(tame_coord()),
        transform in transform(-50.0f32..50.0),
    ) {
        prop_assume!(transform.is_finite());
        let mut renderer = renderer();
        let geometry = renderer.fill_path(&path, transform);
        for v in geometry.vertices {
            prop_assert!(
                v.is_finite(),
                "a fill under {transform:?} put {v:?} in clip space"
            );
        }
    }
}

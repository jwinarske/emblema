//! Occlusion culling changes no pixel, over generated scenes.
//!
//! Culling is defined as removing only what nothing can see, so the same scene
//! recorded with it and without it must render to the same bytes. That is one
//! property, it holds for every scene, and it is exactly what a wrong predicate
//! breaks.
//!
//! `emblema`'s `culling_hidden_pixels_changes_no_pixel` asserts it over eight
//! scenes written by hand, one per case someone thought of. The case that
//! actually shipped a wrong picture was not among them: a mesh with translucent
//! per-vertex colors satisfied `BatchDraw::occludes`, because the material it
//! carries is an opaque `Material::Solid` and the colors are on the vertices.
//! It was found by reading `docs/non-parity.md` §19, which predicted the
//! unsoundness for a path that did not exist yet, and `covered` had been
//! refusing the unindexed form by accident -- so an indexed quad was a wrong
//! picture by 128 of 255 through the public API.
//!
//! Generating the scenes is what makes the next one of those arrive as a test
//! failure instead. Measured against two mutations, each reverted:
//!
//! | mutation | caught by |
//! |---|---|
//! | `occludes` stops reading the vertex colors, which is the shipped defect | 125 levels |
//! | `covered_device_bounds` rounds outward instead of inward | 255 levels |
//!
//! **One predicate it cannot reach, and the reason is worth writing down.**
//! `splits_safely` refuses to split a translucent draw, and that guards a driver
//! that does not honor a scissor exactly -- `cull_occluded` says so at the call:
//! more than one piece means overlapping writes, so the draw has to survive
//! being written twice. On a conformant driver the pieces tile the region and
//! each fragment is written once, so letting a translucent draw split changes no
//! pixel and this comparison stays green. Mutating that arm is invisible here by
//! construction, not by a gap in the generation -- the generator does reach the
//! route, 218 draws in 256 scenes, 168 of them translucent. The lavapipe defect
//! it protects against is probed directly in `golden.rs` and in `public_api.rs`.
//!
//! What is left that this does falsify: `occludes`, `covered`, `remainder`,
//! `Material::is_opaque` and `Material::needs_screen_derivatives` are each
//! conservative in a direction, and each one's wrong answer is a pixel.
//!
//! # What is generated, and what is fixed
//!
//! Scenes of several draws that *overlap*, because culling only does anything
//! where one draw covers another. A generator producing scattered shapes would
//! spend its cases on recordings culling does not touch, so the geometry is
//! drawn from a small set of overlapping rectangles and meshes and what varies
//! is the material, the alpha, the blend, the winding and the indexing -- which
//! is where the predicates live.
//!
//! Pinned to the software reference, like `golden.rs`, because this is a
//! property of the renderer rather than of a device: both sides of the
//! comparison run on the same driver, so a driver's own quirks cancel.

use emblema_core::VertexMode;
use emblema_hal::BlendMode;
use emblema_hal::PixelFormat;
use emblema_hal_vulkan::validation::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use emblema_testkit::scene::{Fill, MeshSpec, Stop};
use emblema_testkit::{render_scene_culled_into, Culling, Item, Node, Scene, Shape, Transform};
use proptest::prelude::*;

/// Alphas that straddle the predicates' only numeric boundary.
///
/// One exactly, since that is the test every opacity predicate here makes, and
/// values either side of it. A uniform sample would reach one almost never, and
/// one is the case that decides whether a draw may be moved at all.
fn alpha() -> impl Strategy<Value = f32> {
    prop_oneof![
        4 => Just(1.0f32),
        2 => 0.2f32..0.99,
        1 => Just(0.0f32),
        1 => Just(0.999f32),
    ]
}

fn color() -> impl Strategy<Value = [f32; 4]> {
    (0.0f32..1.0, 0.0f32..1.0, 0.0f32..1.0, alpha())
        .prop_map(|(r, g, b, a)| [r * a, g * a, b * a, a])
}

/// Blends the predicates treat differently: two that pass an opaque source
/// through, and two that read the destination.
fn blend() -> impl Strategy<Value = BlendMode> {
    prop_oneof![
        3 => Just(BlendMode::SrcOver),
        2 => Just(BlendMode::Src),
        1 => Just(BlendMode::Multiply),
        1 => Just(BlendMode::DstIn),
    ]
}

/// A rectangle from a small set of overlapping ones.
///
/// Snapped to halves rather than to integers: a draw whose edge falls between
/// pixels is the case where a scissor's rounding and a rasterizer's coverage can
/// disagree, and `covered_device_bounds` rounds inward for exactly that reason.
fn rect() -> impl Strategy<Value = Shape> {
    (0u32..6, 0u32..6, 1u32..7, 1u32..7).prop_map(|(x, y, w, h)| Shape::Rect {
        min: [x as f32 * 15.5, y as f32 * 15.5],
        max: [
            x as f32 * 15.5 + w as f32 * 16.0,
            y as f32 * 15.5 + h as f32 * 16.0,
        ],
    })
}

/// A fill for a given rectangle: a flat color, or one of two gradients.
///
/// The gradient's endpoints are derived from the rectangle rather than fixed,
/// because that decides the *route*: endpoints on the shape's own edges take
/// the vertex-interpolated path and anything else takes the fragment walk. A
/// fixed pair reached the first almost never, which left the predicates that
/// path depends on -- `splits_safely` admitting `Material::VertexGradient` --
/// generated but never exercised. Mutating that arm went unnoticed until the
/// endpoints followed the shape.
fn fill_for(shape: &Shape) -> impl Strategy<Value = Fill> {
    let Shape::Rect { min, max } = *shape else {
        unreachable!("rect() yields a rectangle")
    };
    prop_oneof![
        3 => color().prop_map(Fill::Solid),
        // On the edges: the interpolated route.
        2 => (color(), color()).prop_map(move |(a, b)| Fill::LinearGradient {
            start: [min[0], min[1]],
            end: [max[0], min[1]],
            stops: vec![Stop::new(a, 0.0), Stop::new(b, 1.0)],
            tile: emblema_hal::TileMode::Clamp,
        }),
        // Diagonal: the fragment walk, over the same shape.
        1 => (color(), color()).prop_map(move |(a, b)| Fill::LinearGradient {
            start: [min[0], min[1]],
            end: [max[0], max[1]],
            stops: vec![Stop::new(a, 0.0), Stop::new(b, 1.0)],
            tile: emblema_hal::TileMode::Clamp,
        }),
    ]
}

/// A quad as a mesh, indexed or not, with or without per-vertex colors.
///
/// Both forms on purpose. `BatchDraw::covered` wants four distinct vertices
/// against six indices, so the unindexed quad has six distinct ones and is
/// refused -- which is what kept the translucent-mesh defect latent, and is why
/// a generator that produced only one of the two forms would have missed it.
fn mesh() -> impl Strategy<Value = MeshSpec> {
    (
        rect(),
        prop::collection::vec(color(), 4..5),
        any::<bool>(),
        any::<bool>(),
        blend(),
    )
        .prop_map(|(shape, colors, indexed, colored, blend)| {
            let Shape::Rect { min, max } = shape else {
                unreachable!("rect() yields a rectangle")
            };
            let corners = vec![
                [min[0], min[1]],
                [max[0], min[1]],
                [max[0], max[1]],
                [min[0], max[1]],
            ];
            let (positions, indices) = if indexed {
                (corners, vec![0, 1, 2, 0, 2, 3])
            } else {
                // The same quad with every vertex its own.
                (
                    vec![
                        corners[0], corners[1], corners[2], corners[0], corners[2], corners[3],
                    ],
                    Vec::new(),
                )
            };
            let colors = if colored {
                positions
                    .iter()
                    .enumerate()
                    .map(|(i, _)| colors[i % colors.len()])
                    .collect()
            } else {
                Vec::new()
            };
            MeshSpec {
                mode: VertexMode::Triangles,
                positions,
                colors,
                texture_coords: Vec::new(),
                indices,
                // Opaque white, which is the pairing that reads as an occluder
                // while the vertex colors decide what is actually painted.
                fill: Fill::Solid([1.0, 1.0, 1.0, 1.0]),
                tint_blend: BlendMode::Modulate,
                blend,
                transform: Transform::default(),
                image_filter: emblema_core::ImageFilter::None,
                mask_blur: emblema_testkit::scene::MaskBlur::default(),
            }
        })
}

fn node() -> impl Strategy<Value = Node> {
    prop_oneof![
        3 => rect().prop_flat_map(|shape| {
            (Just(shape.clone()), fill_for(&shape), blend())
        }).prop_map(|(shape, fill, blend)| {
            Node::Draw(Box::new(Item::filled(shape, fill).with_blend(blend)))
        }),
        2 => mesh().prop_map(|m| Node::Mesh(Box::new(m))),
    ]
}

/// Several overlapping draws, which is the only shape culling acts on.
fn scene() -> impl Strategy<Value = Scene> {
    prop::collection::vec(node(), 2..6)
        .prop_map(|items| Scene::tree("culling", items).with_samples(1))
}

fn device() -> Option<Validated> {
    match Validated::new(DevicePreference::Software) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no software reference ({e})");
            None
        }
    }
}

proptest! {
    // Each case is two recordings and two renders, so the count is lower than a
    // recording-only sweep and `PROPTEST_CASES` raises it for a deliberate one.
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Culling removes only what nothing can see, so it changes no pixel.
    #[test]
    fn culling_changes_no_pixel_of_a_generated_scene(scene in scene()) {
        let Some(mut ctx) = device() else { return Ok(()) };

        let culled = render_scene_culled_into::<VulkanHal>(
            &mut ctx, &scene, PixelFormat::Rgba8Unorm, Culling::Applied,
        );
        let whole = render_scene_culled_into::<VulkanHal>(
            &mut ctx, &scene, PixelFormat::Rgba8Unorm, Culling::Kept,
        );
        // A scene this device declines is declined both ways, and a refusal is
        // an answer. What would not be is one side drawing and the other not.
        let (culled, whole) = match (culled, whole) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(_), Err(_)) => return Ok(()),
            (a, b) => {
                prop_assert!(
                    false,
                    "culling decided whether the scene renders at all: {:?} against {:?}",
                    a.err(), b.err()
                );
                unreachable!()
            }
        };

        prop_assert_eq!(culled.pixels.len(), whole.pixels.len());
        // Exact. A split draw is the same fragments with a scissor around them,
        // which is what the arithmetic predicts and what `public_api.rs`'s
        // hand-written version of this asserts on three rasterizers.
        let mut worst = 0i32;
        let mut at = 0usize;
        for (i, (a, b)) in culled.pixels.iter().zip(whole.pixels.iter()).enumerate() {
            let d = (*a as i32 - *b as i32).abs();
            if d > worst {
                worst = d;
                at = i;
            }
        }
        prop_assert_eq!(
            worst, 0,
            "culling changed a pixel by {} levels, at byte {} of {}",
            worst, at, culled.pixels.len()
        );
    }
}

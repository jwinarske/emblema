//! Generated scenes rendered on both backends.
//!
//! `cross_backend.rs` renders the corpus and the catalog on Vulkan and GLES and
//! requires them to agree. Every scene in both is written by hand, so what the
//! comparison covers is the shapes someone thought to write down.
//!
//! The two backends are not two configurations of one implementation. Vulkan
//! encodes a batch into a command buffer; GLES replays it against a global state
//! machine, and reaches advanced blending through an extension with a barrier
//! before each draw. One WGSL source is translated per backend, so a
//! translation that diverged on one of them shows up as a disagreement here --
//! which is the thing `emblema-shaders`' snapshot test says in its own words it
//! cannot see, since it compares the two translations against stored copies of
//! themselves.
//!
//! Generating the scenes widens that net to shapes nobody wrote: a draw whose
//! blend and tint disagree, a mesh indexed one way rather than another, a
//! gradient that takes the interpolated route beside one that does not.
//!
//! # Tolerance
//!
//! `Scene::tolerance()`, the same figure the corpus comparison uses, derived
//! from what the scene does rather than chosen for this test. A generated scene
//! that needed more than a hand-written one would be asserting less than what
//! already runs -- and if one does exceed it, the question is which scene,
//! which is what the failure prints.

use emblema_core::VertexMode;
use emblema_hal::BlendMode;
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesHal};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use emblema_testkit::scene::{Fill, MeshSpec, Stop};
use emblema_testkit::{accepts, compare, render_scene, Item, Node, Scene, Shape, Transform};
use proptest::prelude::*;

/// Alphas either side of the one boundary the renderer's predicates test.
fn alpha() -> impl Strategy<Value = f32> {
    prop_oneof![
        3 => Just(1.0f32),
        2 => 0.2f32..0.95,
    ]
}

fn color() -> impl Strategy<Value = [f32; 4]> {
    (0.0f32..1.0, 0.0f32..1.0, 0.0f32..1.0, alpha()).prop_map(|(r, g, b, a)| [r, g, b, a])
}

/// Whether both backends here offer advanced blending.
///
/// Probed once, because it decides what the generator may produce. A scene
/// naming an advanced mode on a device without the capability is not a
/// comparison -- `Scene::supported_by` refuses it -- and generating them anyway
/// is what made this test abort: at 512 cases it hit 355 successes against 1024
/// rejects and proptest gave up, because this workstation's Vulkan device has no
/// advanced blending while its GLES context does, and four of the six modes
/// below are advanced.
///
/// So the capability is read rather than assumed, and the modes follow it: where
/// both offer advanced blending the advanced path is covered, and where one does
/// not the comparison keeps every case and says what it gave up.
///
/// **Neither environment measured offers it on both sides**, and the first
/// version of this comment claimed CI did. Measured 2026-10-04: this
/// workstation's Mesa 26.2.3 gives `advanced_blend` false on the Vulkan device
/// `Auto` selects, true on lavapipe and true on GLES; CI's Mesa 25.2.8 gives
/// false on both Vulkan preferences and true on GLES, so lavapipe gained the
/// capability between those versions. The pair this test uses -- `Auto` and
/// GLES, matching `cross_backend.rs` -- therefore omits advanced modes in both
/// places today. Comparing them across backends stays the corpus's job, where
/// the catalog's advanced-blend scenes are held wherever a device reports the
/// capability and named in the skip census where it does not.
fn advanced_blending_everywhere() -> bool {
    static ANSWER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ANSWER.get_or_init(|| {
        let vulkan = Validated::new(DevicePreference::Auto)
            .map(|ctx| ctx.capabilities().advanced_blend)
            .unwrap_or(false);
        let gles = GlesValidated::new(DisplayTarget::Surfaceless)
            .map(|ctx| ctx.capabilities().advanced_blend)
            .unwrap_or(false);
        if !(vulkan && gles) {
            eprintln!(
                "generating no advanced blend modes: vulkan has them = {vulkan}, \
                 gles has them = {gles}"
            );
        }
        vulkan && gles
    })
}

/// Modes that reach different machinery on GLES: Porter-Duff ones are fixed
/// function, the advanced ones go through `GL_KHR_blend_equation_advanced` with
/// a blend-qualified copy of the fragment stage and a barrier.
fn blend() -> BoxedStrategy<BlendMode> {
    let porter_duff = prop_oneof![3 => Just(BlendMode::SrcOver), 1 => Just(BlendMode::Src)];
    if !advanced_blending_everywhere() {
        return porter_duff.boxed();
    }
    prop_oneof![
        3 => Just(BlendMode::SrcOver),
        1 => Just(BlendMode::Src),
        1 => Just(BlendMode::Multiply),
        1 => Just(BlendMode::Screen),
        1 => Just(BlendMode::Overlay),
        1 => Just(BlendMode::Difference),
    ]
    .boxed()
}

/// A rectangle on integer coordinates, which is the boundary of what two
/// backends can be held to.
///
/// A shape's edge between pixels is where the backends are free to differ, at
/// either sample count, and for two different reasons. Aliased, a pixel is in
/// or out by where its *center* falls, so an edge within float noise of a center
/// is decided by the last bit of the vertex transform. Multisampled, the
/// coverage is the fraction of sample positions inside the shape -- and those
/// positions are implementation-defined, so neither specification requires the
/// two to agree.
///
/// Both measured in `a_fractional_edge_is_where_the_backends_may_differ`: an
/// edge two thousandths of a pixel from a center flips the full fill color over
/// 32 pixels aliased, and a fractional edge at four samples differs by about an
/// eighth of the color along it. Integer coordinates put every edge half a pixel
/// from the nearest center and on a sample grid boundary, so coverage is zero or
/// one whatever the pattern.
///
/// What is given up is antialiased edge agreement, which `scene.tolerance()`'s
/// multisampled profile is what handles in the corpus. What is kept is
/// everything this file is actually about: state, shading, blending, materials,
/// meshes and which route a fill took.
fn rect() -> impl Strategy<Value = ([f32; 2], [f32; 2])> {
    (2u32..40, 2u32..40, 16u32..60, 16u32..60)
        .prop_map(|(x, y, w, h)| ([x as f32, y as f32], [(x + w) as f32, (y + h) as f32]))
}

/// A fill derived from the shape, so a gradient's endpoints decide its route.
///
/// Both routes on purpose: endpoints on the shape's edges take the
/// vertex-interpolated path and a diagonal takes the fragment walk, and the two
/// are translated differently per backend only in the second case. A fixed pair
/// of endpoints would reach one of them almost never -- which is a mistake this
/// generator already made once, in `culling.rs`.
fn fill_for(min: [f32; 2], max: [f32; 2]) -> impl Strategy<Value = Fill> {
    prop_oneof![
        3 => color().prop_map(Fill::Solid),
        2 => (color(), color(), alpha()).prop_map(move |(a, b, alpha)| {
            let with = |c: [f32; 4]| [c[0], c[1], c[2], alpha];
            Fill::LinearGradient {
                start: [min[0], min[1]],
                end: [max[0], min[1]],
                stops: vec![Stop::new(with(a), 0.0), Stop::new(with(b), 1.0)],
                tile: emblema_hal::TileMode::Clamp,
            }
        }),
        2 => (color(), color()).prop_map(move |(a, b)| Fill::LinearGradient {
            start: [min[0], min[1]],
            end: [max[0], max[1]],
            stops: vec![Stop::new(a, 0.0), Stop::new(b, 1.0)],
            tile: emblema_hal::TileMode::Clamp,
        }),
    ]
}

/// A quad as a mesh, indexed or not, colored or not.
fn mesh() -> impl Strategy<Value = MeshSpec> {
    (rect(), color(), any::<bool>(), any::<bool>(), blend()).prop_map(
        |((min, max), color, indexed, colored, blend)| {
            let corners = [
                [min[0], min[1]],
                [max[0], min[1]],
                [max[0], max[1]],
                [min[0], max[1]],
            ];
            let (positions, indices) = if indexed {
                (corners.to_vec(), vec![0, 1, 2, 0, 2, 3])
            } else {
                (
                    vec![
                        corners[0], corners[1], corners[2], corners[0], corners[2], corners[3],
                    ],
                    Vec::new(),
                )
            };
            let colors = if colored {
                vec![color; positions.len()]
            } else {
                Vec::new()
            };
            MeshSpec {
                mode: VertexMode::Triangles,
                positions,
                colors,
                texture_coords: Vec::new(),
                indices,
                fill: Fill::Solid([1.0, 1.0, 1.0, 1.0]),
                tint_blend: BlendMode::Modulate,
                blend,
                transform: Transform::default(),
                image_filter: emblema_core::ImageFilter::None,
                mask_blur: emblema_testkit::scene::MaskBlur::default(),
            }
        },
    )
}

fn node() -> impl Strategy<Value = Node> {
    prop_oneof![
        3 => rect()
            .prop_flat_map(|(min, max)| (Just((min, max)), fill_for(min, max), blend()))
            .prop_map(|((min, max), fill, blend)| {
                Node::Draw(Box::new(
                    Item::filled(Shape::Rect { min, max }, fill).with_blend(blend),
                ))
            }),
        2 => rect()
            .prop_flat_map(|(min, max)| (Just((min, max)), fill_for(min, max), blend()))
            .prop_map(|((min, max), fill, blend)| {
                Node::Draw(Box::new(
                    Item::filled(
                        Shape::RoundedRect {
                            min,
                            max,
                            radius: 6.0,
                        },
                        fill,
                    )
                    .with_blend(blend),
                ))
            }),
        2 => mesh().prop_map(|m| Node::Mesh(Box::new(m))),
    ]
}

/// Several overlapping draws, and a sample count.
///
/// Multisampled as well as not, since the pass's sample count is state each
/// backend establishes its own way -- a resolve attachment on one, a
/// framebuffer with a renderbuffer on the other.
fn scene() -> impl Strategy<Value = Scene> {
    (
        prop::collection::vec(node(), 1..5),
        prop_oneof![3 => Just(1u32), 1 => Just(4u32)],
    )
        .prop_map(|(items, samples)| {
            Scene::tree("cross-backend-generated", items).with_samples(samples)
        })
}

struct Backends {
    vulkan: Validated,
    gles: GlesValidated,
}

fn backends() -> Option<Backends> {
    let vulkan = match Validated::new(DevicePreference::Auto) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            return None;
        }
    };
    let gles = match GlesValidated::new(DisplayTarget::Surfaceless) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no GLES context ({e})");
            return None;
        }
    };
    Some(Backends { vulkan, gles })
}

proptest! {
    // Each case is two contexts rendering the same scene, which is dearer than
    // a single-device property. `PROPTEST_CASES` raises it for a sweep.
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// The two backends agree on a generated scene, to the tolerance the scene
    /// itself derives.
    #[test]
    fn the_backends_agree_on_a_generated_scene(scene in scene()) {
        let Some(mut ctx) = backends() else { return Ok(()) };

        // A scene neither backend claims is not a disagreement. Asked of both,
        // because a capability one has and the other does not is exactly what
        // this comparison must not absorb silently.
        prop_assume!(
            scene.supported_by(ctx.vulkan.capabilities())
                && scene.supported_by(ctx.gles.capabilities())
        );

        let from_vulkan = render_scene::<VulkanHal>(&mut ctx.vulkan, &scene);
        let from_gles = render_scene::<GlesHal>(&mut ctx.gles, &scene);
        let (from_vulkan, from_gles) = match (from_vulkan, from_gles) {
            (Ok(a), Ok(b)) => (a, b),
            // Both refusing is an answer. One refusing where the other drew is
            // not, and it is the asymmetry this test exists to find.
            (Err(_), Err(_)) => return Ok(()),
            (a, b) => {
                prop_assert!(
                    false,
                    "the backends disagreed about whether the scene renders at all: \
                     vulkan {:?}, gles {:?}",
                    a.err(),
                    b.err()
                );
                unreachable!()
            }
        };

        let difference = compare(&from_vulkan, &from_gles).expect("same size");
        let tolerance = scene.tolerance();
        prop_assert!(
            accepts(&difference, tolerance),
            "the backends disagree: {}",
            difference.describe(tolerance)
        );
    }
}

/// Integer edges agree; fractional ones are not required to.
///
/// A shape's edge between pixels is free to differ, at either sample count and
/// for two separate reasons. Aliased, a pixel is in or out by where its center
/// falls, so an edge within float noise of a center turns on the last bit of the
/// vertex transform. Multisampled, coverage is the fraction of sample positions
/// inside the shape, and those positions are implementation-defined -- neither
/// specification requires two backends to place them alike.
///
/// So this asserts only the direction that is required, and *reports* the other.
/// The first version asserted that the fractional cases do differ and failed in
/// CI, where lavapipe and llvmpipe agree on both: a permitted difference is not
/// a required one, and `docs/architecture.md`'s own note says CI's drivers are
/// not this workstation's. Measured here, where they do differ: the aliased edge
/// two thousandths of a pixel from a center flips the whole fill color over 32
/// pixels, and the multisampled fractional edge differs by 33 of 255 along it.
///
/// What is asserted is what `rect`'s integer coordinates rest on, and that holds
/// wherever both backends run.
#[test]
fn integer_edges_agree_and_fractional_ones_need_not() {
    let Some(mut ctx) = backends() else { return };
    let measure = |min: [f32; 2], max: [f32; 2], samples: u32, ctx: &mut Backends| -> (u8, usize) {
        let scene = Scene::tree(
            "edge",
            vec![Node::Draw(Box::new(Item::filled(
                Shape::Rect { min, max },
                Fill::Solid([0.0, 0.289_746, 0.513_394, 1.0]),
            )))],
        )
        .with_samples(samples);
        let a = render_scene::<VulkanHal>(&mut ctx.vulkan, &scene).expect("vulkan");
        let b = render_scene::<GlesHal>(&mut ctx.gles, &scene).expect("gles");
        let d = compare(&a, &b).expect("same size");
        (d.max_delta, d.differing)
    };

    // The generator's own case, at both sample counts. This is the assertion.
    for samples in [1, 4] {
        assert_eq!(
            measure([10.0, 2.0], [26.0, 18.0], samples, &mut ctx),
            (0, 0),
            "an integer edge is half a pixel from either center, at {samples} sample(s)"
        );
    }

    // And aliased, a quarter of a pixel is not a tie either, which is why the
    // snapping is about ties rather than about fractions.
    assert_eq!(
        measure([9.75, 2.0], [25.75, 18.0], 1, &mut ctx),
        (0, 0),
        "a quarter of a pixel is not a tie"
    );

    // Reported, not asserted: whether these differ is the driver pair's to
    // decide, and the generator avoids them either way.
    let (aliased, aliased_pixels) = measure([9.501_682, 2.0], [25.501_682, 18.0], 1, &mut ctx);
    let (multisampled, multisampled_pixels) =
        measure([2.0, 14.126_62], [51.153_47, 62.126_797], 4, &mut ctx);
    eprintln!(
        "a tied aliased edge differs by {aliased} over {aliased_pixels} pixels; \
         a fractional multisampled edge by {multisampled} over {multisampled_pixels}"
    );
}

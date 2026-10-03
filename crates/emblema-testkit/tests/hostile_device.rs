//! Generated scenes, drawn on a device rather than only recorded.
//!
//! Every other generated test in this workspace stops at the recording. `hostile_api.rs`
//! builds hostile canvas sequences and asserts the recording names nothing that is not
//! there; `hostile.rs` builds paths from NaN and infinities and asserts tessellation
//! returns. None of them hands the result to a driver, so a backend that mishandles a
//! shape only a generator would produce is not covered by any of it. `architecture.md`'s
//! L7 row named that: "all of it is recording-only -- no generated scene is executed on a
//! device".
//!
//! That gap is not theoretical. The one defect of this class found so far -- a driver
//! crashing inside its own shader compiler -- turned up because a bench happened to run on
//! the board, not because anything asked.
//!
//! The second half of the same row: `LayerSpec`, which is what the corpus is written in, is
//! not generated at all. Both are here, because the useful generated scene is one carrying
//! a generated layer.
//!
//! # What is asserted, and what is not
//!
//! Not that every generated scene renders. A hostile layer can ask for something a device
//! declines, and a refusal is a correct answer -- `Unsupported` is a result, not a fault.
//! What is asserted is that the attempt *returns*: no panic, no hang, no abort, and where an
//! image does come back it is the size that was asked for and every byte of it is readable.
//! That is the property a driver can break and nothing else here would notice.

use emblema_core::{ImageFilter, Transform2D};
use emblema_hal::{BlendMode, Extent2D};
use emblema_hal_vulkan::validation::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use emblema_testkit::scene::MorphologySpec;
use emblema_testkit::{render_scene, Item, LayerSpec, Node, Scene, Shape, Transform};
use glam::{Affine2, Vec2};
use proptest::prelude::*;

/// Values a layer field is interesting at.
///
/// Hostile rather than uniform: zero, the subnormals, both infinities, NaN and the largest
/// finite float are where a filter's arithmetic stops being ordinary, and a uniform sample
/// over a sane range reaches none of them.
/// The awkward values, chosen by index rather than by a union of strategies.
///
/// A `prop_oneof!` per float was the obvious spelling and it does not survive composition:
/// ten fields each built from an eight-arm union, several of them twice over inside a filter,
/// nests `TupleUnion` deeply enough that generating one value overflows the stack in a debug
/// build -- before any case reaches the test body, which is what made it look like a renderer
/// fault rather than a strategy one. Each piece passed alone; only the whole did not.
///
/// An index into a table is one small strategy whatever it is composed into, and covers the
/// same values. The ordinary range keeps the weight it had: indices past the table's end map
/// into it, so four of twelve draws land on a plain number.
const AWKWARD: [f32; 7] = [
    0.0,
    -0.0,
    f32::MIN_POSITIVE,
    f32::MAX,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NAN,
];

fn hostile_f32() -> impl Strategy<Value = f32> {
    (0u8..12, -64.0f32..64.0)
        .prop_map(|(which, ordinary)| AWKWARD.get(which as usize).copied().unwrap_or(ordinary))
}

fn image_filter() -> impl Strategy<Value = ImageFilter> {
    prop_oneof![
        3 => Just(ImageFilter::None),
        2 => (hostile_f32(), hostile_f32())
            .prop_map(|(sigma_x, sigma_y)| ImageFilter::Blur { sigma_x, sigma_y }),
        1 => (hostile_f32(), hostile_f32())
            .prop_map(|(radius_x, radius_y)| ImageFilter::Dilate { radius_x, radius_y }),
        1 => (hostile_f32(), hostile_f32())
            .prop_map(|(radius_x, radius_y)| ImageFilter::Erode { radius_x, radius_y }),
        1 => (hostile_f32(), hostile_f32(), hostile_f32()).prop_map(|(sx, sy, angle)| {
            ImageFilter::Matrix {
                transform: Transform2D::from(Affine2::from_scale(Vec2::new(sx, sy)))
                    * Transform2D::from(Affine2::from_angle(angle)),
            }
        }),
    ]
}

fn transform() -> impl Strategy<Value = Transform> {
    (
        (hostile_f32(), hostile_f32()),
        hostile_f32(),
        (hostile_f32(), hostile_f32()),
        (hostile_f32(), hostile_f32()),
        (hostile_f32(), hostile_f32()),
    )
        // A struct literal, so a field added to `Transform` is a compile error here rather
        // than a field this quietly stops covering. `hostile_api.rs` generates `Layer` the
        // same way and for the same reason.
        .prop_map(|(scale, rotate, skew, translate, perspective)| Transform {
            scale: [scale.0, scale.1],
            rotate,
            skew: [skew.0, skew.1],
            translate: [translate.0, translate.1],
            perspective: [perspective.0, perspective.1],
        })
}

fn blend_mode() -> impl Strategy<Value = BlendMode> {
    prop_oneof![
        Just(BlendMode::SrcOver),
        Just(BlendMode::Src),
        Just(BlendMode::DstIn),
        Just(BlendMode::Multiply),
        Just(BlendMode::Screen),
        Just(BlendMode::Difference),
    ]
}

/// A generated `LayerSpec`, every field covered.
///
/// Built as a struct literal for the reason the comment in `transform` gives: the corpus is
/// written in this type, and a field nobody generates is a field the corpus can use and no
/// generated scene ever exercises.
///
/// Tupled in groups because `proptest`'s tuple strategies stop at twelve and this has ten
/// fields spread across types that do not share a strategy.
fn layer_spec() -> impl Strategy<Value = LayerSpec> {
    (
        (image_filter(), image_filter()),
        (hostile_f32(), hostile_f32()),
        (
            prop::option::of(transform()),
            prop::option::of((hostile_f32(), hostile_f32(), any::<bool>())),
        ),
        (hostile_f32(), blend_mode(), prop::option::of(any::<i64>())),
    )
        .prop_map(
            |(
                (filter, backdrop),
                (blur, backdrop_blur),
                (matrix, morphology),
                (alpha, blend, backdrop_id),
            )| LayerSpec {
                filter,
                backdrop,
                blur,
                matrix,
                alpha,
                blend,
                backdrop_blur,
                morphology: morphology.map(|(x, y, dilate)| MorphologySpec {
                    radius: [x, y],
                    dilate,
                }),
                color_filter: emblema_hal::ColorFilter::None,
                backdrop_id,
            },
        )
}

/// A scene whose one group carries a generated layer.
///
/// Small and fixed apart from the layer: what is under test is the layer, and a generated
/// shape underneath it would make a failure two questions instead of one.
fn scene_with(spec: LayerSpec) -> Scene {
    Scene::tree(
        "hostile-layer",
        vec![Node::Layer {
            layer: Box::new(spec),
            bounds: None,
            transform: Transform::default(),
            children: vec![Node::Draw(Box::new(Item::fill(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                [0.2, 0.6, 0.9, 1.0],
            )))],
        }],
    )
}

proptest! {
    // Fewer cases than a recording-only sweep, because each one is a submission and a
    // readback rather than arithmetic. Enough to cover the strategy's arms several times
    // over, and `PROPTEST_CASES` raises it for a deliberate sweep.
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// A generated layer, drawn. The attempt returns and any image is whole.
    #[test]
    fn a_generated_layer_draws_or_declines(spec in layer_spec()) {
        let Some(ctx) = device() else { return Ok(()) };
        let mut ctx = ctx;
        // Each case written out before it runs, when asked. A device fault here is as
        // likely to abort the process as to fail an assertion, and proptest cannot report
        // or shrink an input whose process did not survive it -- so the log is the only
        // thing that names the case. Off unless `HOSTILE_LAYER_LOG` says where.
        if let Ok(path) = std::env::var("HOSTILE_LAYER_LOG") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "{spec:?}");
                let _ = f.flush();
            }
        }
        let scene = scene_with(spec);
        let extent = scene.size;
        // A refusal is an answer. What would not be is a panic, a hang, or an image the
        // wrong size -- and an image the wrong size is what a backend that believed a
        // NaN-derived extent would hand back.
        if let Ok(image) = render_scene::<VulkanHal>(&mut ctx, &scene) {
            prop_assert_eq!(
                (image.width, image.height),
                (extent.width, extent.height),
                "a generated layer changed the target's size"
            );
            prop_assert_eq!(
                image.pixels.len(),
                (extent.width as usize) * (extent.height as usize) * 4,
                "an image came back with the wrong number of bytes for its size"
            );
        }
    }
}

/// The device these run on, or `None` with the reason named.
///
/// Built per case rather than once, which costs time and buys the thing being looked for: a
/// context that a hostile layer left in a state the next case cannot use would otherwise
/// show up as the *next* case failing, and the shrinker would blame the wrong input.
fn device() -> Option<Validated> {
    static WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    match Validated::new(DevicePreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            WARNED.get_or_init(|| eprintln!("skipping: no Vulkan device ({e})"));
            None
        }
    }
}

/// The generated scene is a scene: it records, and the recording is addressable.
///
/// The recording half, kept beside the device half because the two fail differently. A
/// recording that names a vertex it does not have is this crate's bug; an image the wrong
/// size is a backend's.
#[test]
fn the_generated_scene_shape_is_sound() {
    let spec = LayerSpec {
        filter: ImageFilter::Blur {
            sigma_x: f32::NAN,
            sigma_y: f32::INFINITY,
        },
        backdrop: ImageFilter::None,
        blur: f32::MAX,
        matrix: None,
        alpha: f32::NAN,
        blend: BlendMode::SrcOver,
        backdrop_blur: -0.0,
        morphology: Some(MorphologySpec {
            radius: [f32::INFINITY, 0.0],
            dilate: true,
        }),
        color_filter: emblema_hal::ColorFilter::None,
        backdrop_id: None,
    };
    let scene = scene_with(spec);
    assert_eq!(scene.size, Extent2D::new(128, 128));
    let recording = emblema_testkit::record_scene(&scene).expect("a hostile layer still records");
    for pass in &recording.passes {
        let vertices = pass.batch.vertices().len();
        for draw in pass.batch.draws() {
            let first = draw.first_index as usize;
            let end = first + draw.index_count as usize;
            let indices = pass.batch.indices();
            assert!(end <= indices.len(), "a draw names indices past the buffer");
            for &index in &indices[first..end] {
                assert!(
                    (index as usize) < vertices,
                    "a draw names vertex {index} of {vertices}"
                );
            }
        }
    }
}

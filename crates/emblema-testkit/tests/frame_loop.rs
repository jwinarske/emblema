//! A generated scene renders the same after other frames have run.
//!
//! Every device test in this workspace but one renders a scene into a fresh
//! context, which is the single arrangement in which state a previous frame
//! left behind cannot be seen. A frame loop is the opposite: one context
//! renders scene after scene, and whatever one frame leaves set is what the
//! next inherits.
//!
//! `cross_backend.rs`'s `a_scene_renders_the_same_after_an_unrelated_frame`
//! asserts this, and its comment records the failure that put it there: a clip
//! left the scissor enabled, and the multisample resolve -- a blit, and blits
//! are scissored -- copied only the part of the frame the previous draw could
//! touch. Its subject and its pollutants are both corpus scenes, filtered to
//! the ones whose names begin `clip-` or `layer-`, so what it covers is the
//! state the corpus happens to set.
//!
//! This generates both sides. The subject is rendered, then one to three other
//! scenes, then the subject again, and the two renders must be identical to the
//! byte. Nothing about that is a tolerance question: it is the same scene, the
//! same device, the same driver, so a difference is state.
//!
//! # Both backends, and why GLES is the one to watch
//!
//! Vulkan records state into a command buffer per pass. GLES sets it on a
//! global context and the next draw inherits whatever was not reset -- scissor,
//! blend equation, the bound framebuffer, the advanced-blend barrier. So the
//! generator leans on the things that get *enabled*: clips, layers,
//! multisampling, and the blend modes that reach the extension path.
//!
//! # What it catches, measured
//!
//! Removing *both* of the GLES backend's scissor resets -- the one at a pass's
//! start and the one before the resolve blit -- fails this in 16,128 pixels.
//! Removing either alone does not, and that is the tree being doubly defended
//! rather than this being weak: either reset on its own leaves the scissor off
//! by the time the next frame draws.
//!
//! **What it cannot catch is a frame that corrupts itself.** The resolve guard's
//! own comment describes the historical bug as the resolve copying only the part
//! of the frame the last draw could touch -- and that happens *within* one
//! render, so both of this test's renders carry it identically and the
//! comparison cancels. That defect belongs to `cross_backend.rs` and the
//! goldens, which compare against something other than the same scene on the
//! same device. Here the subject is compared with itself, so what is left is
//! exactly state carried *between* frames, which is the thing no other test
//! here varies.

use emblema_core::MaskBlurStyle;
use emblema_hal::{BlendMode, Hal, HalContext};
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesHal};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use emblema_testkit::scene::Fill;
use emblema_testkit::{render_scene, Item, LayerSpec, Node, Scene, Shape};
use proptest::prelude::*;

fn color() -> impl Strategy<Value = [f32; 4]> {
    (
        0.0f32..1.0,
        0.0f32..1.0,
        0.0f32..1.0,
        prop_oneof![
            3 => Just(1.0f32),
            1 => 0.2f32..0.95,
        ],
    )
        .prop_map(|(r, g, b, a)| [r, g, b, a])
}

/// Integer coordinates, for the reason `cross_backend_generated.rs` measures:
/// a shape's edge between pixels is where two rasterizers are free to differ.
///
/// Here it matters less -- both renders are on the same device -- but a subject
/// whose own edges are unstable would make a failure two questions instead of
/// one.
fn rect() -> impl Strategy<Value = ([f32; 2], [f32; 2])> {
    (2u32..40, 2u32..40, 16u32..60, 16u32..60)
        .prop_map(|(x, y, w, h)| ([x as f32, y as f32], [(x + w) as f32, (y + h) as f32]))
}

/// Blends that reach different machinery: fixed function, and the advanced
/// modes that GLES serves through an extension with a barrier before each draw.
fn blend() -> impl Strategy<Value = BlendMode> {
    prop_oneof![
        3 => Just(BlendMode::SrcOver),
        1 => Just(BlendMode::Src),
        1 => Just(BlendMode::Multiply),
        1 => Just(BlendMode::Overlay),
        1 => Just(BlendMode::Difference),
    ]
}

/// A draw that may carry a clip, a clip-out, a mask blur or a stroke.
///
/// Weighted toward clips because that is the state whose leak is on record.
fn draw() -> impl Strategy<Value = Node> {
    (
        rect(),
        color(),
        blend(),
        prop::option::of(rect()),
        prop::option::of(rect()),
        prop_oneof![4 => Just(0.0f32), 1 => 1.0f32..6.0],
        prop_oneof![
            Just(MaskBlurStyle::Normal),
            Just(MaskBlurStyle::Solid),
            Just(MaskBlurStyle::Outer),
        ],
    )
        .prop_map(
            |((min, max), color, blend, clip, clip_out, mask_blur, style)| {
                let mut item =
                    Item::filled(Shape::Rect { min, max }, Fill::Solid(color)).with_blend(blend);
                if let Some((lo, hi)) = clip {
                    item = item.with_clip([lo[0], lo[1], hi[0], hi[1]]);
                }
                if let Some((lo, hi)) = clip_out {
                    item = item.with_clip_out([lo[0], lo[1], hi[0], hi[1]]);
                }
                if mask_blur > 0.0 {
                    item = item.with_mask_blur(mask_blur).with_mask_blur_style(style);
                }
                Node::Draw(Box::new(item))
            },
        )
}

/// A group, which is a pass of its own and a framebuffer the backend has to
/// bind and unbind.
fn layer() -> impl Strategy<Value = Node> {
    (
        prop::collection::vec(draw(), 1..3),
        0.3f32..1.0,
        blend(),
        prop_oneof![3 => Just(0.0f32), 1 => 1.0f32..5.0],
    )
        .prop_map(|(children, alpha, blend, blur)| Node::Layer {
            layer: Box::new(LayerSpec::opacity(alpha).with_blend(blend).with_blur(blur)),
            bounds: None,
            transform: emblema_testkit::Transform::default(),
            children,
        })
}

fn node() -> impl Strategy<Value = Node> {
    prop_oneof![3 => draw(), 1 => layer()]
}

fn scene(name: &'static str) -> impl Strategy<Value = Scene> {
    (
        prop::collection::vec(node(), 1..4),
        prop_oneof![2 => Just(1u32), 1 => Just(4u32)],
    )
        .prop_map(move |(items, samples)| Scene::tree(name, items).with_samples(samples))
}

/// Render the subject, then the others, then the subject again.
///
/// Returns the number of differing pixels, or `None` if the device declined the
/// subject -- which is an answer, and the same one both times.
fn repeat<H: Hal>(ctx: &mut H::Context, subject: &Scene, others: &[Scene]) -> Option<usize>
where
    H::Context: HalContext<Hal = H>,
{
    if !subject.supported_by(ctx.capabilities()) {
        return None;
    }
    let first = render_scene::<H>(ctx, subject).ok()?;
    for other in others {
        if other.supported_by(ctx.capabilities()) {
            // A refusal is fine: what matters is that it ran and left whatever
            // it left.
            let _ = render_scene::<H>(ctx, other);
        }
    }
    let again = render_scene::<H>(ctx, subject).ok()?;
    Some(
        first
            .pixels
            .chunks_exact(4)
            .zip(again.pixels.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count(),
    )
}

proptest! {
    // Each case is up to five renders on each of two backends.
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// The same scene, rendered twice in one context with other frames between,
    /// gives the same bytes on either backend.
    #[test]
    fn a_generated_scene_survives_other_frames(
        subject in scene("subject"),
        others in prop::collection::vec(scene("other"), 1..4),
    ) {
        if let Ok(mut vulkan) = Validated::new(DevicePreference::Software) {
            if let Some(differing) = repeat::<VulkanHal>(&mut vulkan, &subject, &others) {
                prop_assert_eq!(
                    differing, 0,
                    "vulkan: the subject rendered differently after {} other frame(s), \
                     in {} pixel(s)",
                    others.len(), differing
                );
            }
        }
        if let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) {
            if let Some(differing) = repeat::<GlesHal>(&mut gles, &subject, &others) {
                prop_assert_eq!(
                    differing, 0,
                    "gles: the subject rendered differently after {} other frame(s), \
                     in {} pixel(s)",
                    others.len(), differing
                );
            }
        }
    }
}

/// The comparison is not vacuous: a context does render these scenes.
///
/// Beside the property because every arm of that one is conditional -- no
/// device, an unsupported scene and a refused render all return without
/// asserting, which is correct and is also how it could go quiet.
#[test]
fn the_frame_loop_actually_renders() {
    let subject = Scene::tree(
        "subject",
        vec![Node::Draw(Box::new(
            Item::filled(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [72.0, 56.0],
                },
                Fill::Solid([0.2, 0.5, 0.9, 1.0]),
            )
            .with_clip([16.0, 16.0, 64.0, 48.0]),
        ))],
    )
    .with_samples(4);
    let other = Scene::tree(
        "other",
        vec![Node::Draw(Box::new(
            Item::filled(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [32.0, 32.0],
                },
                Fill::Solid([1.0, 0.0, 0.0, 1.0]),
            )
            .with_clip([0.0, 0.0, 16.0, 16.0]),
        ))],
    );

    let mut ran = 0usize;
    if let Ok(mut vulkan) = Validated::new(DevicePreference::Software) {
        let differing = repeat::<VulkanHal>(&mut vulkan, &subject, std::slice::from_ref(&other))
            .expect("the software reference renders a clipped rectangle");
        assert_eq!(differing, 0, "vulkan left state behind");
        ran += 1;
    }
    if let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let differing = repeat::<GlesHal>(&mut gles, &subject, std::slice::from_ref(&other))
            .expect("gles renders a clipped rectangle");
        assert_eq!(differing, 0, "gles left state behind");
        ran += 1;
    }
    assert!(
        ran > 0,
        "no backend was available, so the property above asserted nothing either"
    );
}

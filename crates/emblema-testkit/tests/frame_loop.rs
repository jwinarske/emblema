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
/// Returns the worst per-channel difference between the two renders of the
/// subject, or `None` if the device declined it -- which is an answer, and the
/// same one both times.
fn repeat<H: Hal>(ctx: &mut H::Context, subject: &Scene, others: &[Scene]) -> Option<i32>
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
            .iter()
            .zip(again.pixels.iter())
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap_or(0),
    )
}

/// Whether a scene reaches `a_blurred_advanced_blend_layer_is_unstable_on_gles`.
///
/// Conservative: multisampled, with a mask blur anywhere and an advanced blend
/// anywhere. The reduced case needs both inside one layer and this does not
/// check that, so a few stable scenes are left out too -- which costs coverage
/// and cannot hide a defect, where the other direction could.
fn reaches_the_gles_instability(scene: &Scene) -> bool {
    fn has_mask_blur(node: &Node) -> bool {
        match node {
            Node::Draw(item) => item.mask_blur > 0.0,
            Node::Layer { children, .. } => children.iter().any(has_mask_blur),
            _ => false,
        }
    }
    fn has_advanced_blend(node: &Node) -> bool {
        match node {
            Node::Draw(item) => item.blend.is_advanced(),
            Node::Layer {
                layer, children, ..
            } => layer.blend.is_advanced() || children.iter().any(has_advanced_blend),
            _ => false,
        }
    }
    scene.samples > 1
        && scene.items.iter().any(has_mask_blur)
        && scene.items.iter().any(has_advanced_blend)
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
            if let Some(worst) = repeat::<VulkanHal>(&mut vulkan, &subject, &others) {
                // Exact. This backend is stable against itself for every scene
                // this generates.
                prop_assert_eq!(
                    worst, 0,
                    "vulkan: the subject rendered differently after {} other frame(s), \
                     by {} levels",
                    others.len(), worst
                );
            }
        }
        // GLES, except where the scene reaches the instability this file
        // records. Skipped rather than tolerated: the first version of this
        // allowed three levels, which was one reduced case's size, and the
        // generator found five. A bound raised until the suite passes is a
        // bound fitted to the defect, which is the trade `image.rs` refuses in
        // its own words -- so the combination is named and left out, and
        // everything else stays exact.
        if !reaches_the_gles_instability(&subject) {
            if let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) {
                if let Some(worst) = repeat::<GlesHal>(&mut gles, &subject, &others) {
                    prop_assert_eq!(
                        worst, 0,
                        "gles: the subject rendered differently after {} other frame(s), \
                         by {} levels",
                        others.len(), worst
                    );
                }
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
        let worst = repeat::<VulkanHal>(&mut vulkan, &subject, std::slice::from_ref(&other))
            .expect("the software reference renders a clipped rectangle");
        assert_eq!(worst, 0, "vulkan left state behind");
        ran += 1;
    }
    if let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let worst = repeat::<GlesHal>(&mut gles, &subject, std::slice::from_ref(&other))
            .expect("gles renders a clipped rectangle");
        assert_eq!(worst, 0, "gles left state behind");
        ran += 1;
    }
    assert!(
        ran > 0,
        "no backend was available, so the property above asserted nothing either"
    );
}

/// A GLES instability, reduced and bounded.
///
/// Found by the property above at 512 cases, where it read as a state leak and
/// is not one. What it is, measured: this scene's output depends on what was
/// rendered into the same context before it. A fresh context and a context whose
/// last frame was a full-target white rectangle both give one answer; a context
/// whose last frame was *this same scene* gives another, three levels apart over
/// about a hundred pixels of the blur's halo.
///
/// Three is this reduced case's size, not the defect's. The property above found
/// five on a scene it generated, which is why that one skips the combination
/// rather than allowing a number: a bound raised until the suite passes is
/// fitted to the defect.
///
/// Every ingredient is required -- remove any one and the difference is zero:
/// the layer's advanced blend, the child's advanced blend, the mask blur, four
/// samples, and the second child under the blurred one. The layer records as a
/// 26x22 pass that the frame then samples, and the differing pixels lie exactly
/// in that region.
///
/// **It is one GLES driver, not the GLES path.** That sentence used to say the
/// path, and it was concluded by comparing against software *Vulkan*: that
/// device reports the same `advanced_blend` capability, renders the same scene
/// and is stable, and both GLES renders sit within `Scene::tolerance()` of the
/// Vulkan one, which is why no cross-backend comparison has ever shown this.
/// Comparing against software *GLES* is the thing nobody had done. Measured
/// 2026-10-07: three levels on `radeonsi`, **zero on `llvmpipe`** -- the same
/// backend code, the same scene, the same call sequence. So whatever this is,
/// the sequence this renderer issues is not sufficient to produce it.
///
/// No other driver on this bench can weigh in, which is itself worth knowing.
/// V3D on a Raspberry Pi 5 has no `advanced_blend` at all. The SA8155P's Adreno
/// has `libEGL` and `libGLESv2` but not the surfaceless platform, and
/// `DisplayTarget` offers nothing else, so the backend gets no context there.
/// The i.MX8MP has no GLES device. Two drivers is the whole sample.
///
/// No mechanism is written down here. The shape points at something reading
/// texels nothing wrote this frame, and the dependence on the previous frame's
/// *size* points at an allocation being reused -- but `docs/architecture.md`'s
/// own rule is that a mechanism is not recorded until it predicts measurements
/// taken before it existed, and this one does not yet. What is recorded is the
/// reproduction and the bound.
///
/// **One candidate is ruled out**, so the next person does not spend the hour
/// again: it is not this backend's offscreen texture storage. `tex_storage_2d`
/// leaves that uninitialized, which fits the shape exactly -- and zeroing it at
/// creation with a `tex_sub_image_2d` of zeros leaves the difference at three
/// levels, unchanged. Every pass this scene records already clears, a
/// multisampled pass with no clear is refused outright at the top of
/// `GlesContext::submit`, and the renderer allocates a fresh texture per pass at
/// exactly the pass extent rather than pooling one. So whatever carries the
/// previous frame's influence is below all of that.
///
/// What would settle it is a reproduction in bare GLES with no renderer in it --
/// a multisample framebuffer, a draw under `GL_KHR_blend_equation_advanced`, a
/// resolve, and a sample of the result -- which is what this tree's own rule
/// asks for before a driver is named. That has not been written. It now has a
/// target to aim at and a control to check against, which it did not before:
/// whatever it does has to come out unstable on `radeonsi` and clean on
/// `llvmpipe`, and a version that fails on both is reproducing something else.
///
/// Asserted as a bound rather than as the defect, so a fix makes this pass
/// rather than fail: zero is within three.
#[test]
fn a_blurred_advanced_blend_layer_is_unstable_on_gles() {
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    if !gles.capabilities().advanced_blend {
        eprintln!("skipping: this GLES context has no advanced blending");
        return;
    }

    let blurred = Node::Draw(Box::new(
        Item::filled(
            Shape::Rect {
                min: [2.0, 2.0],
                max: [22.0, 18.0],
            },
            Fill::Solid([0.0, 0.221_247, 0.0, 1.0]),
        )
        .with_blend(BlendMode::Multiply)
        .with_mask_blur(1.0)
        .with_mask_blur_style(MaskBlurStyle::Normal),
    ));
    let under = Node::Draw(Box::new(
        Item::filled(
            Shape::Rect {
                min: [2.0, 2.0],
                max: [18.0, 18.0],
            },
            Fill::Solid([0.0, 0.0, 0.0, 1.0]),
        )
        .with_blend(BlendMode::SrcOver),
    ));
    let subject = Scene::tree(
        "unstable",
        vec![Node::Layer {
            layer: Box::new(LayerSpec::opacity(0.3).with_blend(BlendMode::Difference)),
            bounds: None,
            transform: emblema_testkit::Transform::default(),
            children: vec![blurred, under],
        }],
    )
    .with_samples(4);

    let first = render_scene::<GlesHal>(&mut gles, &subject).expect("first");
    let settled = render_scene::<GlesHal>(&mut gles, &subject).expect("settled");
    let worst = first
        .pixels
        .iter()
        .zip(settled.pixels.iter())
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap_or(0);
    // Bounded loosely and deliberately. What this test is for is the
    // reproduction, which is what a fix needs; the size is reported rather than
    // pinned, because the property above already showed that this case's three
    // is not the defect's ceiling.
    assert!(
        worst <= 16,
        "the instability grew to {worst} levels, far past the three this case cost \
         when it was reduced -- that is a different defect, not this one"
    );
    // The driver by name, because which one it is turned out to be the whole
    // question: the same backend code is stable on one and not on another.
    eprintln!(
        "the reduced instability is {worst} levels on {}",
        gles.capabilities().device_name
    );

    // And the frame's own pass is where it is not: a scene with no layer, no
    // blur and no advanced blend is stable, which is what makes the bound above
    // a statement about this combination rather than about the backend.
    let plain = Scene::tree(
        "plain",
        vec![Node::Draw(Box::new(Item::filled(
            Shape::Rect {
                min: [2.0, 2.0],
                max: [22.0, 18.0],
            },
            Fill::Solid([0.0, 0.221_247, 0.0, 1.0]),
        )))],
    )
    .with_samples(4);
    let a = render_scene::<GlesHal>(&mut gles, &plain).expect("plain first");
    let b = render_scene::<GlesHal>(&mut gles, &plain).expect("plain again");
    assert_eq!(
        a.pixels, b.pixels,
        "a plain multisampled fill is stable on this backend"
    );
}

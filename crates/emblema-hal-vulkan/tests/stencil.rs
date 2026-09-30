//! Stencil clipping at the Vulkan backend, checked against the validation
//! layer as well as against pixels.
//!
//! The stencil path is the first thing here that attaches something other than
//! a color image, and most of the ways it can be wrong — a pipeline built
//! against an incompatible render pass, a reference nobody set, an attachment
//! count that disagrees with the clear values — produce either a plausible
//! picture or none at all. The layer names them, so every test asserts the log
//! is clean and not merely that the result looked right.

use emblema_hal::{
    Batch, BlendMode, ClipState, ColorFilter, Extent2D, Material, PassDescriptor, PixelFormat,
    Scissor, TextureDescriptor,
};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

const SIZE: Extent2D = Extent2D {
    width: 32,
    height: 32,
};
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];
const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

fn context() -> Option<VulkanContext> {
    match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
        ..Default::default()
    }) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

/// A rectangle in clip space, from one given in pixels with the origin at the
/// top-left corner.
///
/// Y is negated because clip space here follows the WGSL convention and runs
/// upward, so the top row of the image sits at positive one. Getting this
/// backwards mirrors the rectangle vertically, which is why every rectangle in
/// this file is deliberately off-center: it showed up as a shifted origin with
/// the correct height rather than as anything obviously wrong.
fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> [[f32; 2]; 4] {
    let to_clip = |x: f32, y: f32| {
        [
            x / SIZE.width as f32 * 2.0 - 1.0,
            1.0 - y / SIZE.height as f32 * 2.0,
        ]
    };
    [
        to_clip(x0, y0),
        to_clip(x1, y0),
        to_clip(x1, y1),
        to_clip(x0, y1),
    ]
}

const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

fn render(ctx: &mut VulkanContext, batch: &Batch, samples: u32) -> Vec<u8> {
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("texture");
    ctx.submit_batch(
        &mut target,
        batch,
        PassDescriptor::clear(BACKGROUND).with_samples(samples),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
    pixels
}

fn lit(pixels: &[u8], x: u32, y: u32) -> bool {
    pixels[((y * SIZE.width + x) * 4) as usize] > 128
}

/// The rectangle of pixels the draw actually reached.
fn bounds(pixels: &[u8]) -> Option<Scissor> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            if lit(pixels, x, y) {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then(|| Scissor::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

#[test]
fn a_stencil_clip_confines_content_to_the_clip_geometry() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    // Build a clip out of an off-center rectangle, then fill the whole target
    // through it. Off-center so a mirrored or transposed axis is a different
    // picture rather than the same one.
    batch
        .push_with(
            &rect(6.0, 4.0, 20.0, 25.0),
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(0),
        )
        .expect("clip");
    batch
        .push_with(
            &FULL,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::content(1),
        )
        .expect("content");

    let pixels = render(&mut ctx, &batch, 1);
    assert_eq!(bounds(&pixels), Some(Scissor::new(6, 4, 14, 21)));
}

#[test]
fn a_clip_draw_writes_no_color_of_its_own() {
    let Some(mut ctx) = context() else { return };
    // The clip geometry is white on a black background, so if it wrote color it
    // would be plainly visible. Nothing follows it, so the target must come
    // back exactly as it was cleared.
    let mut batch = Batch::new();
    batch
        .push_with(
            &rect(6.0, 4.0, 20.0, 25.0),
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(0),
        )
        .expect("clip");

    let pixels = render(&mut ctx, &batch, 1);
    assert_eq!(bounds(&pixels), None, "the clip geometry painted itself");
}

#[test]
fn nested_clips_intersect() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    // Two overlapping clips, each narrowing one step further. Content at depth
    // two survives only where both admitted it; a stencil that replaced rather
    // than stepped forward would leave the second rectangle whole.
    batch
        .push_with(
            &rect(4.0, 4.0, 24.0, 20.0),
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(0),
        )
        .expect("outer");
    batch
        .push_with(
            &rect(12.0, 10.0, 30.0, 28.0),
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(1),
        )
        .expect("inner");
    batch
        .push_with(
            &FULL,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::content(2),
        )
        .expect("content");

    let pixels = render(&mut ctx, &batch, 1);
    assert_eq!(
        bounds(&pixels),
        Some(Scissor::new(12, 10, 12, 10)),
        "the two clips did not intersect"
    );
}

#[test]
fn widening_undoes_a_clip_for_what_follows() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    // Narrow, draw nothing, widen back, then fill. The fill is at depth zero
    // again and must reach the whole target; a stencil left stepped forward
    // would confine it to the clip that was supposed to have been undone.
    let clip = rect(4.0, 4.0, 16.0, 16.0);
    batch
        .push_with(
            &clip,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(0),
        )
        .expect("narrow");
    batch
        .push_with(
            &clip,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::widen(1),
        )
        .expect("widen");
    batch
        .push_with(
            &FULL,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::content(0),
        )
        .expect("content");

    let pixels = render(&mut ctx, &batch, 1);
    assert_eq!(
        bounds(&pixels),
        Some(Scissor::covering(SIZE)),
        "widening did not restore the previous clip"
    );
}

#[test]
fn a_scissor_and_a_stencil_clip_both_apply() {
    let Some(mut ctx) = context() else { return };
    // The two are independent tests and compose by intersection. This is what
    // lets an axis-aligned clip keep using the scissor even once a stencil is
    // already in play, which is the reason the scissor path is not a
    // stepping stone.
    let mut batch = Batch::new();
    batch
        .push_with(
            &rect(4.0, 4.0, 24.0, 24.0),
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(0),
        )
        .expect("clip");
    batch
        .push_with(
            &FULL,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            Some(Scissor::new(10, 0, 30, 12)),
            ClipState::content(1),
        )
        .expect("content");

    let pixels = render(&mut ctx, &batch, 1);
    // The stencil allows x in 4..24 and y in 4..24; the scissor allows x from
    // 10 and y below 12. The overlap is the only region that may be drawn.
    assert_eq!(bounds(&pixels), Some(Scissor::new(10, 4, 14, 8)));
}

#[test]
fn a_batch_that_clips_nothing_still_renders() {
    let Some(mut ctx) = context() else { return };
    // No stencil attachment is created at all in this case, and the pipelines
    // built for it are keyed differently from the clipping ones. This is the
    // path every existing scene takes, so it is worth asserting directly
    // rather than inferring from the rest of the suite.
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, Material::solid(WHITE), BlendMode::Src)
        .expect("content");
    assert!(!batch.uses_stencil());

    let pixels = render(&mut ctx, &batch, 1);
    assert_eq!(bounds(&pixels), Some(Scissor::covering(SIZE)));
}

#[test]
fn a_stencil_clip_works_multisampled() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    // The stencil attachment has to carry the same sample count as the color
    // one, and a mismatch is a render pass the layer rejects rather than
    // something visible. Beyond validity this is what antialiases a clip edge:
    // with a per-sample stencil, a boundary crossing a pixel admits some of its
    // samples and not others.
    let mut batch = Batch::new();
    batch
        .push_with(
            &rect(6.0, 4.0, 20.0, 25.0),
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(0),
        )
        .expect("clip");
    batch
        .push_with(
            &FULL,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::content(1),
        )
        .expect("content");

    let pixels = render(&mut ctx, &batch, 4);
    assert_eq!(bounds(&pixels), Some(Scissor::new(6, 4, 14, 21)));
}

#[test]
fn a_clip_stack_deeper_than_the_stencil_is_refused() {
    let Some(mut ctx) = context() else { return };
    // Past the eight bits every device is required to offer, the value wraps to
    // zero and the clip admits everything it was meant to exclude. That is a
    // wrong picture rather than an error, so it is refused up front.
    let mut batch = Batch::new();
    batch
        .push_with(
            &FULL,
            &QUAD,
            Material::solid(WHITE),
            ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(255),
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("texture");
    let result = ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(BACKGROUND));
    ctx.destroy_texture(target);
    assert!(
        matches!(result, Err(emblema_hal::Error::LimitExceeded { .. })),
        "a clip stack past the stencil's range was accepted: {result:?}"
    );
}

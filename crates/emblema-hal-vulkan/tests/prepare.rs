//! Pipelines built ahead of the first frame.
//!
//! The claim `prepare_pipelines` makes is that the draws a frame starts with
//! find their pipelines already built, so the first frame compiles nothing.
//! The count of pipelines the context holds is how that is seen: a draw that
//! needed a new one would add to it. The pictures are checked too, since a
//! pipeline built against one pass and used with another is only correct if
//! the two passes are compatible, and a mismatch there is a picture, or a
//! validation error, rather than a failure to build.

use emblema_hal::{
    Batch, BlendMode, ClipState, ColorFilter, Extent2D, Material, PassDescriptor, PixelFormat,
    TextureDescriptor,
};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, ValidationSeverity, VulkanContext};

const SIZE: Extent2D = Extent2D::new(32, 32);
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const INNER: [[f32; 2]; 4] = [[-0.5, -0.25], [0.25, -0.25], [0.25, 0.75], [-0.5, 0.75]];
const HALF_RED: [f32; 4] = [1.0, 0.0, 0.0, 0.5];

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

/// A translucent fill: source-over, no clip.
fn plain() -> Batch {
    let mut batch = Batch::new();
    batch
        .push(&INNER, &QUAD, Material::solid(HALF_RED), BlendMode::SrcOver)
        .expect("push");
    batch
}

/// A clip narrowed, filled through, and widened back: every stencil role.
fn clipped() -> Batch {
    let mut batch = Batch::new();
    let mut push = |verts: &[[f32; 2]; 4], stencil| {
        batch
            .push_with(
                verts,
                &QUAD,
                Material::solid(HALF_RED),
                ColorFilter::None,
                BlendMode::SrcOver,
                None,
                stencil,
            )
            .expect("push");
    };
    push(&INNER, ClipState::narrow(0));
    push(&FULL, ClipState::content(1));
    push(&INNER, ClipState::widen(1));
    batch
}

fn render(ctx: &mut VulkanContext, batch: &Batch, samples: u32) -> Vec<u8> {
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("texture");
    ctx.submit_batch(
        &mut target,
        batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]).with_samples(samples),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
    pixels
}

/// Every frame the tests draw, at one sample and at four.
fn frames(ctx: &mut VulkanContext) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for samples in [1, 4] {
        out.push(render(ctx, &plain(), samples));
        out.push(render(ctx, &clipped(), samples));
    }
    out
}

#[test]
fn a_prepared_context_builds_nothing_for_a_first_frame() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: no four-sample rendering");
        return;
    }
    assert_eq!(ctx.pipeline_count(), 0);
    ctx.prepare_pipelines(PixelFormat::Rgba8Unorm, 4)
        .expect("prepare");
    // Source-over at one and four samples: one with no stencil, three with.
    assert_eq!(ctx.pipeline_count(), 8);
    frames(&mut ctx);
    assert_eq!(
        ctx.pipeline_count(),
        8,
        "a first frame built a pipeline prepare_pipelines should have"
    );
}

#[test]
fn a_prepared_context_draws_what_an_unprepared_one_does() {
    let Some(mut cold) = context() else { return };
    let Some(mut warm) = context() else { return };
    if !cold.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: no four-sample rendering");
        return;
    }
    warm.prepare_pipelines(PixelFormat::Rgba8Unorm, 4)
        .expect("prepare");
    assert_eq!(frames(&mut warm), frames(&mut cold));
}

#[test]
fn preparing_again_builds_nothing_more() {
    let Some(mut ctx) = context() else { return };
    ctx.prepare_pipelines(PixelFormat::Rgba8Unorm, 1)
        .expect("prepare");
    // One sample only: one with no stencil, three with.
    assert_eq!(ctx.pipeline_count(), 4);
    ctx.prepare_pipelines(PixelFormat::Rgba8Unorm, 1)
        .expect("prepare again");
    assert_eq!(ctx.pipeline_count(), 4);
}

#[test]
fn an_unusable_sample_count_is_refused_and_builds_nothing() {
    let Some(mut ctx) = context() else { return };
    assert!(ctx.prepare_pipelines(PixelFormat::Rgba8Unorm, 3).is_err());
    assert!(ctx.prepare_pipelines(PixelFormat::Rgba8Unorm, 128).is_err());
    assert_eq!(ctx.pipeline_count(), 0);
}

/// No validation message at or above a warning, the context's whole life.
fn assert_clean(ctx: &VulkanContext) {
    let bad: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity >= ValidationSeverity::Warning)
        .collect();
    assert!(bad.is_empty(), "validation: {bad:?}");
}

#[test]
fn what_the_driver_compiled_is_taken_by_a_later_context() {
    let Some(mut first) = context() else { return };
    assert!(
        first.pipeline_cache_data().expect("data").is_empty(),
        "a context that built nothing has nothing to keep"
    );
    first
        .prepare_pipelines(PixelFormat::Rgba8Unorm, 1)
        .expect("prepare");
    let saved = first.pipeline_cache_data().expect("data");
    assert!(!saved.is_empty());
    let drawn = render(&mut first, &plain(), 1);
    assert_clean(&first);

    let Some(mut second) = context() else { return };
    let taken = second.load_pipeline_cache(&saved).expect("load");
    // A driver that keeps nothing says so with less than a header (lavapipe
    // gives 20 bytes), and that is not a cache to hand back to it.
    assert_eq!(taken, saved.len() >= 32, "{} bytes", saved.len());
    // Loaded is not built: the pipelines are still made, from what was kept.
    assert_eq!(second.pipeline_count(), 0);
    assert_eq!(render(&mut second, &plain(), 1), drawn);
    assert_clean(&second);
}

#[test]
fn what_is_not_this_device_s_cache_is_left_alone() {
    let Some(mut first) = context() else { return };
    first
        .prepare_pipelines(PixelFormat::Rgba8Unorm, 1)
        .expect("prepare");
    let saved = first.pipeline_cache_data().expect("data");
    let drawn = render(&mut first, &plain(), 1);

    if saved.len() < 32 {
        eprintln!("skipping: this driver keeps no pipeline cache");
        return;
    }
    let Some(mut ctx) = context() else { return };
    assert!(!ctx.load_pipeline_cache(&[]).expect("empty"));
    assert!(!ctx.load_pipeline_cache(&[0xff; 31]).expect("short"));
    assert!(!ctx.load_pipeline_cache(&[0xff; 4096]).expect("noise"));
    // Another build of the driver: its UUID differs.
    let mut other = saved.clone();
    other[16] ^= 0xff;
    assert!(!ctx.load_pipeline_cache(&other).expect("another driver"));
    // Another device.
    let mut other = saved;
    other[12] ^= 0xff;
    assert!(!ctx.load_pipeline_cache(&other).expect("another device"));
    // Nothing refused has been handed to the driver, and it draws as before.
    assert_eq!(render(&mut ctx, &plain(), 1), drawn);
    assert_clean(&ctx);
}

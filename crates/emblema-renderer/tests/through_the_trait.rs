//! Rendering a scene without naming a backend.
//!
//! Every other test reaches for the Vulkan context's inherent methods. This
//! one goes through `HalContext` alone, so it fails to compile if the trait
//! omits something a renderer genuinely needs. That is the check the trait
//! exists for: a backend-agnostic layer that can only be written against a
//! concrete backend has an abstraction that does not abstract.

use emblema_geometry::{Path, PathBuilder};
use emblema_hal::{
    Batch, BlendMode, Extent2D, Hal, HalContext, HalTexture, PassDescriptor, PixelFormat,
    TextureDescriptor,
};
use emblema_hal_vulkan::{VulkanContext, VulkanHal};
use emblema_renderer::{Paint, Renderer, TOLERANCE};
use glam::{Affine2, Vec2};

const SIZE: u32 = 64;
const TARGET: Extent2D = Extent2D {
    width: SIZE,
    height: SIZE,
};

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Path {
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(x0, y0))
        .line_to(Vec2::new(x1, y0))
        .line_to(Vec2::new(x1, y1))
        .line_to(Vec2::new(x0, y1))
        .close();
    b.build()
}

/// Draw a scene using only what the HAL trait exposes.
///
/// Generic over the backend and never mentioning Vulkan, so anything missing
/// from the trait is a compile error here rather than a discovery when a second
/// backend is written.
fn render_scene<H: Hal>(
    ctx: &mut H::Context,
    shapes: &[(Path, [f32; 4], BlendMode)],
) -> (Vec<u8>, Extent2D)
where
    H::Context: HalContext<Hal = H>,
{
    let mut renderer = Renderer::new();
    renderer.begin_frame(TARGET, TOLERANCE);
    let mut batch = Batch::new();
    for (path, color, blend) in shapes {
        renderer
            .fill_into(
                &mut batch,
                path,
                Affine2::IDENTITY,
                &Paint::solid(*color).with_blend(*blend),
            )
            .expect("tessellate");
    }

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(
            TARGET,
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");

    // The target must describe itself through the trait: a renderer sizing its
    // projection needs this without knowing which backend produced it.
    assert_eq!(target.extent(), TARGET);
    assert_eq!(target.format(), PixelFormat::Rgba8Unorm);

    ctx.submit_batch(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    let extent = target.extent();
    ctx.destroy_texture(target);
    (pixels, extent)
}

fn context() -> Option<emblema_hal_vulkan::Validated> {
    match emblema_hal_vulkan::Validated::new(emblema_hal_vulkan::DevicePreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn a_scene_renders_through_the_trait_alone() {
    let Some(mut ctx) = context() else { return };
    let shapes = vec![
        (
            rect(0.0, 0.0, 32.0, 64.0),
            [1.0, 0.0, 0.0, 1.0],
            BlendMode::Src,
        ),
        (
            rect(32.0, 0.0, 64.0, 64.0),
            [0.0, 1.0, 0.0, 1.0],
            BlendMode::Src,
        ),
    ];
    let (pixels, extent) = render_scene::<VulkanHal>(&mut ctx, &shapes);

    assert_eq!(extent, TARGET);
    assert_eq!(pixel(&pixels, 8, 32), [255, 0, 0, 255], "left shape");
    assert_eq!(pixel(&pixels, 56, 32), [0, 255, 0, 255], "right shape");
}

#[test]
fn the_backend_names_itself_for_reports() {
    // Report fingerprints and per-driver tolerance tables are keyed on this,
    // so it has to be reachable without naming the backend.
    assert_eq!(VulkanHal::NAME, "vulkan");
}

#[test]
fn capabilities_are_reachable_through_the_trait() {
    let Some(mut ctx) = context() else { return };
    // Branching on capabilities is the rule everywhere above the HAL, which
    // only works if the trait exposes them.
    let caps = ctx.capabilities().clone();
    assert!(caps.max_texture_size >= 4096);
    assert!(caps.sample_counts.supports(1));

    // And an oversized allocation must fail through the trait too, rather than
    // only through the concrete API.
    let too_big = TextureDescriptor::offscreen(
        Extent2D::new(caps.max_texture_size + 1, 16),
        PixelFormat::Rgba8Unorm,
    );
    assert!(HalContext::create_texture(&mut *ctx, &too_big).is_err());
}

#[test]
fn translucent_shapes_compose_through_the_trait() {
    let Some(mut ctx) = context() else { return };
    let shapes = vec![
        (
            rect(0.0, 0.0, 64.0, 64.0),
            [0.0, 0.0, 1.0, 1.0],
            BlendMode::Src,
        ),
        (
            rect(0.0, 0.0, 64.0, 64.0),
            [1.0, 0.0, 0.0, 0.5],
            BlendMode::SrcOver,
        ),
    ];
    let (pixels, _) = render_scene::<VulkanHal>(&mut ctx, &shapes);
    // Half-alpha red over opaque blue, both reaching the backend in one batch.
    let got = pixel(&pixels, 32, 32);
    for (channel, want) in got.iter().zip([128u8, 0, 128, 255]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 1,
            "got {got:?}, expected about [128, 0, 128, 255]"
        );
    }
}

#[test]
fn a_scene_can_be_rendered_antialiased_through_the_trait() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }

    // A triangle with a shallow edge, in user coordinates.
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(0.0, 64.0))
        .line_to(Vec2::new(64.0, 64.0))
        .line_to(Vec2::new(0.0, 20.0))
        .close();
    let path = b.build();

    let render = |ctx: &mut VulkanContext, samples: u32| -> Vec<u8> {
        let mut renderer = Renderer::new();
        renderer.begin_frame(TARGET, TOLERANCE);
        let mut batch = Batch::new();
        renderer
            .fill_into(
                &mut batch,
                &path,
                Affine2::IDENTITY,
                &Paint::solid([1.0, 1.0, 1.0, 1.0]).with_blend(BlendMode::Src),
            )
            .expect("tessellate");

        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(
                TARGET,
                PixelFormat::Rgba8Unorm,
            ))
            .expect("texture");
        ctx.submit_batch(
            &mut target,
            &batch,
            PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]).with_samples(samples),
        )
        .expect("submit");
        let pixels = ctx.read_texture(&mut target).expect("readback");
        ctx.destroy_texture(target);
        pixels
    };

    let partial = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 0 && p[0] < 255)
            .count()
    };

    // Antialiasing needs no renderer plumbing: geometry is independent of how
    // the pass samples it, so a caller opts in purely through the descriptor.
    assert_eq!(partial(&render(&mut ctx, 1)), 0, "aliased");
    assert!(
        partial(&render(&mut ctx, 4)) > 16,
        "antialiased edge should have intermediate coverage"
    );
}

//! Texture sampling, checked against the validation layer.
//!
//! Descriptors are where this can go wrong without the picture saying so: a set
//! bound to the wrong layout, an image read in a layout a shader cannot read,
//! a binding a pipeline declares and nothing supplies. Several of those produce
//! a plausible image on one driver and garbage on the next, so the layer's
//! verdict matters at least as much as the pixels do.

use emblema_hal::{
    Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat, TextureDescriptor, TileMode,
};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

/// Clip space to a texture spanning the whole target, as the shader reads it.
///
/// `scale` is half the target's size over the texture's: one where a texture
/// covers the target at its own size, a half where it covers twice that. The
/// paint's origin is inside the matrix rather than packed beside it, so this is
/// the whole mapping.
fn across_the_target(scale: f32) -> [f32; 12] {
    [
        scale, 0.0, 0.0, 0.0, //
        0.0, -scale, 0.0, 0.0, //
        scale, scale, 1.0, 0.0,
    ]
}

const SIZE: Extent2D = Extent2D {
    width: 16,
    height: 16,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

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

fn assert_clean(ctx: &VulkanContext, what: &str) {
    if !ctx.validation_active() {
        eprintln!("skipping validation assertions: no layer installed");
        return;
    }
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "{what}: validation errors: {errors:?}");
}

fn image_material() -> Material {
    Material::Image {
        to_local: across_the_target(0.5),
        slot: 0,
        alpha: 1.0,
        tile: TileMode::Clamp,
        sampling: emblema_hal::Sampling::Linear,
        source: [0.0, 0.0, 1.0, 1.0],
        tint: [1.0, 1.0, 1.0, 1.0],
    }
}

#[test]
fn sampling_an_uploaded_texture_is_valid() {
    let Some(mut ctx) = context() else { return };
    let mut source = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(2, 2),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("source");
    ctx.write_texture(&mut source, &[255u8; 16])
        .expect("upload");

    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, image_material(), BlendMode::Src)
        .expect("push");
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0; 4]),
        &[&source],
    )
    .expect("submit");
    ctx.destroy_texture(target);
    ctx.destroy_texture(source);
    assert_clean(&ctx, "sampling an uploaded texture");
}

#[test]
fn sampling_a_rendered_target_is_valid() {
    let Some(mut ctx) = context() else { return };
    // A render target arrives in a color-attachment layout, which a shader
    // cannot read. This is the case the save layer will be built on, and the
    // transition it needs is invisible in the output on a device that happens
    // to tolerate the wrong layout.
    let mut layer = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("layer");
    let mut fill = Batch::new();
    fill.push(
        &FULL,
        &QUAD,
        Material::solid([1.0, 0.0, 0.0, 1.0]),
        BlendMode::Src,
    )
    .expect("push");
    ctx.submit_batch(&mut layer, &fill, PassDescriptor::clear([0.0; 4]))
        .expect("fill the layer");

    let mut composite = Batch::new();
    composite
        .push(&FULL, &QUAD, image_material(), BlendMode::SrcOver)
        .expect("push");
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &composite,
        PassDescriptor::clear([0.0; 4]),
        &[&layer],
    )
    .expect("composite");
    ctx.destroy_texture(target);
    ctx.destroy_texture(layer);
    assert_clean(&ctx, "sampling a rendered target");
}

#[test]
fn a_batch_that_samples_nothing_still_binds_a_valid_descriptor() {
    let Some(mut ctx) = context() else { return };
    // The shader declares the texture whatever the paint is, so a solid fill
    // still needs something bound. Leaving it unbound is invalid rather than
    // merely wasteful, and this is the path every other test in the suite
    // takes -- so it is worth asserting directly.
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([0.0, 1.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch(&mut target, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("submit");
    ctx.destroy_texture(target);
    assert_clean(&ctx, "a batch that samples nothing");
}

#[test]
fn sampling_several_textures_in_one_batch_is_valid() {
    let Some(mut ctx) = context() else { return };
    // Two slots means two descriptor sets and a rebind between the draws, which
    // is where a set allocated from a pool sized for one would fail.
    let mut textures = Vec::new();
    for _ in 0..2 {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(
                Extent2D::new(2, 2),
                PixelFormat::Rgba8Unorm,
            ))
            .expect("texture");
        ctx.write_texture(&mut texture, &[255u8; 16])
            .expect("upload");
        textures.push(texture);
    }

    let mut batch = Batch::new();
    for slot in 0..2u32 {
        let mut material = image_material();
        if let Material::Image { slot: s, .. } = &mut material {
            *s = slot;
        }
        batch
            .push(&FULL, &QUAD, material, BlendMode::SrcOver)
            .expect("push");
    }
    // And a solid draw between them, so the placeholder is bound in the middle
    // of a run that also binds real textures.
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([0.0, 0.0, 1.0, 0.5]),
            BlendMode::SrcOver,
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    let refs: Vec<_> = textures.iter().collect();
    ctx.submit_batch_textured(&mut target, &batch, PassDescriptor::clear([0.0; 4]), &refs)
        .expect("submit");
    ctx.destroy_texture(target);
    for texture in textures {
        ctx.destroy_texture(texture);
    }
    assert_clean(&ctx, "several textures in one batch");
}

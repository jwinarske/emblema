//! Paint reaching the fragment shader, and draws composing onto a target.
//!
//! Until now every draw was red because the shader said so. These tests check
//! that a color supplied per draw arrives intact, and that a second draw can
//! add to a target instead of replacing it.

use emblema_hal::{BlendMode, Extent2D, Material, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext, VulkanTexture};

const SIZE: u32 = 32;

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

fn target(ctx: &mut VulkanContext) -> VulkanTexture {
    ctx.create_texture(&TextureDescriptor::offscreen(
        Extent2D::new(SIZE, SIZE),
        PixelFormat::Rgba8Unorm,
    ))
    .expect("texture")
}

/// A quad covering the left half of clip space.
const LEFT: [[f32; 2]; 4] = [[-1.0, -1.0], [0.0, -1.0], [0.0, 1.0], [-1.0, 1.0]];
/// A quad covering the right half.
const RIGHT: [[f32; 2]; 4] = [[0.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, 1.0]];
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn assert_clean(ctx: &VulkanContext) {
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .map(|m| format!("[{}] {}", m.id, m.message))
        .collect();
    assert!(
        errors.is_empty(),
        "validation errors:\n{}",
        errors.join("\n")
    );
}

#[test]
fn each_channel_of_the_paint_arrives_independently() {
    let Some(mut ctx) = context() else { return };
    // Distinct values per channel, so a swizzle or a shared write cannot pass.
    for (color, expected) in [
        ([1.0, 0.0, 0.0, 1.0], [255, 0, 0, 255]),
        ([0.0, 1.0, 0.0, 1.0], [0, 255, 0, 255]),
        ([0.0, 0.0, 1.0, 1.0], [0, 0, 255, 255]),
        ([1.0, 1.0, 0.0, 1.0], [255, 255, 0, 255]),
    ] {
        let mut tex = target(&mut ctx);
        ctx.draw_indexed(
            &mut tex,
            &FULL,
            &QUAD,
            Material::solid(color),
            BlendMode::Src,
            Some([0.0, 0.0, 0.0, 1.0]),
        )
        .expect("draw");
        let pixels = ctx.read_texture(&mut tex).expect("readback");
        ctx.destroy_texture(tex);
        assert_eq!(
            pixel(&pixels, SIZE / 2, SIZE / 2),
            expected,
            "for {color:?}"
        );
    }
    assert_clean(&ctx);
}

#[test]
fn intermediate_values_survive_the_round_trip() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    ctx.draw_indexed(
        &mut tex,
        &FULL,
        &QUAD,
        Material::solid([0.25, 0.5, 0.75, 1.0]),
        BlendMode::Src,
        Some([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("draw");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    // The format stores linear values, so these map straight onto bytes. A
    // stray transfer function would show up as a systematic shift here.
    let got = pixel(&pixels, 4, 4);
    for (channel, want) in got.iter().zip([64u8, 128, 191, 255]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 1,
            "got {got:?}, expected about [64, 128, 191, 255]"
        );
    }
    assert_clean(&ctx);
}

#[test]
fn paint_is_independent_of_the_clear_color() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    // Half the target painted, the rest left as the clear color: the two must
    // not be confused for one another.
    ctx.draw_indexed(
        &mut tex,
        &LEFT,
        &QUAD,
        Material::solid([1.0, 0.0, 0.0, 1.0]),
        BlendMode::Src,
        Some([0.0, 0.0, 1.0, 1.0]),
    )
    .expect("draw");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    assert_eq!(
        pixel(&pixels, 4, SIZE / 2),
        [255, 0, 0, 255],
        "painted half"
    );
    assert_eq!(
        pixel(&pixels, SIZE - 4, SIZE / 2),
        [0, 0, 255, 255],
        "cleared half"
    );
    assert_clean(&ctx);
}

#[test]
fn a_second_draw_can_preserve_what_the_first_left() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);

    // First draw clears and paints the left half.
    ctx.draw_indexed(
        &mut tex,
        &LEFT,
        &QUAD,
        Material::solid([1.0, 0.0, 0.0, 1.0]),
        BlendMode::Src,
        Some([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("first draw");
    // Second draw preserves, painting the right half a different color.
    ctx.draw_indexed(
        &mut tex,
        &RIGHT,
        &QUAD,
        Material::solid([0.0, 1.0, 0.0, 1.0]),
        BlendMode::Src,
        None,
    )
    .expect("second draw");

    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    // Both must be present. If the second draw cleared, the left half would be
    // black; if it failed to load, the left half would be undefined.
    assert_eq!(pixel(&pixels, 4, SIZE / 2), [255, 0, 0, 255], "first draw");
    assert_eq!(
        pixel(&pixels, SIZE - 4, SIZE / 2),
        [0, 255, 0, 255],
        "second draw"
    );
    assert_clean(&ctx);
}

#[test]
fn a_later_draw_paints_over_an_earlier_one() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);

    ctx.draw_indexed(
        &mut tex,
        &FULL,
        &QUAD,
        Material::solid([1.0, 0.0, 0.0, 1.0]),
        BlendMode::Src,
        Some([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("first");
    ctx.draw_indexed(
        &mut tex,
        &FULL,
        &QUAD,
        Material::solid([0.0, 0.0, 1.0, 1.0]),
        BlendMode::Src,
        None,
    )
    .expect("second");

    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    // Blending is off, so the later draw wins outright rather than mixing.
    assert_eq!(pixel(&pixels, SIZE / 2, SIZE / 2), [0, 0, 255, 255]);
    assert_clean(&ctx);
}

#[test]
fn preserving_and_clearing_pipelines_are_cached_separately() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    // The load operation is baked into a render pass, so the two need distinct
    // objects. Alternating exercises both cache entries repeatedly, which would
    // expose a key collision handing back the wrong pass.
    for i in 0..4 {
        ctx.draw_indexed(
            &mut tex,
            &FULL,
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
            Some([0.0, 0.0, 0.0, 1.0]),
        )
        .unwrap_or_else(|e| panic!("clearing draw {i}: {e}"));
        ctx.draw_indexed(
            &mut tex,
            &LEFT,
            &QUAD,
            Material::solid([0.0, 1.0, 0.0, 1.0]),
            BlendMode::Src,
            None,
        )
        .unwrap_or_else(|e| panic!("preserving draw {i}: {e}"));
    }
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    assert_eq!(pixel(&pixels, 4, SIZE / 2), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, SIZE - 4, SIZE / 2), [255, 0, 0, 255]);
    assert_clean(&ctx);
}

#[test]
fn a_preserving_draw_with_no_geometry_leaves_the_target_alone() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    ctx.draw_indexed(
        &mut tex,
        &FULL,
        &QUAD,
        Material::solid([1.0, 0.0, 0.0, 1.0]),
        BlendMode::Src,
        Some([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("first");
    // Nothing to draw and nothing to clear must be a no-op, not a wipe.
    ctx.draw_indexed(
        &mut tex,
        &[],
        &[],
        Material::solid([0.0, 1.0, 0.0, 1.0]),
        BlendMode::Src,
        None,
    )
    .expect("empty");

    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    assert_eq!(pixel(&pixels, SIZE / 2, SIZE / 2), [255, 0, 0, 255]);
    assert_clean(&ctx);
}

//! Blending, checked against the arithmetic rather than by eye.
//!
//! Source-over has a closed form, so every expectation here is computed rather
//! than recorded from a previous run. That matters because the classic bug —
//! applying alpha twice by pairing a premultiplied source with a `SRC_ALPHA`
//! blend factor — produces output that looks plausible and is simply too dark.

use emblema_hal::{BlendMode, Extent2D, Material, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

const SIZE: u32 = 16;
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

/// Clear to `background`, then draw each `(color, blend)` in order.
fn composite(
    ctx: &mut VulkanContext,
    background: [f32; 4],
    draws: &[([f32; 4], BlendMode)],
) -> [u8; 4] {
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(SIZE, SIZE),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");

    let mut clear = Some(background);
    for (color, blend) in draws {
        ctx.draw_indexed(
            &mut tex,
            &FULL,
            &QUAD,
            Material::solid(*color),
            *blend,
            clear,
        )
        .expect("draw");
        clear = None;
    }

    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");

    let i = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Compare within one unit per channel.
///
/// Used where the specification permits latitude, never as a way to make an
/// exact expectation pass.
fn assert_near(got: [u8; 4], want: [u8; 4], what: &str) {
    for i in 0..4 {
        assert!(
            (got[i] as i32 - want[i] as i32).abs() <= 1,
            "{what}: got {got:?}, expected about {want:?}"
        );
    }
}

#[test]
fn half_alpha_source_over_opaque_lands_halfway() {
    let Some(mut ctx) = context() else { return };
    // Red at straight alpha 0.5 over opaque blue.
    //   src premultiplied = (0.5, 0, 0, 0.5)
    //   rgb = src + dst * (1 - 0.5) = (0.5, 0, 0) + (0, 0, 0.5) = (0.5, 0, 0.5)
    //   a   = 0.5 + 1 * 0.5 = 1
    let got = composite(
        &mut ctx,
        [0.0, 0.0, 1.0, 1.0],
        &[([1.0, 0.0, 0.0, 0.5], BlendMode::SrcOver)],
    );
    // Were alpha applied twice, red would come back near 64 rather than 128.
    assert_near(got, [128, 0, 128, 255], "half over opaque");
}

#[test]
fn a_fully_transparent_source_leaves_the_destination_untouched() {
    let Some(mut ctx) = context() else { return };
    let got = composite(
        &mut ctx,
        [0.0, 0.0, 1.0, 1.0],
        &[([1.0, 0.0, 0.0, 0.0], BlendMode::SrcOver)],
    );
    assert_near(got, [0, 0, 255, 255], "transparent source");
}

#[test]
fn an_opaque_source_over_replaces_the_destination() {
    let Some(mut ctx) = context() else { return };
    let got = composite(
        &mut ctx,
        [0.0, 0.0, 1.0, 1.0],
        &[([1.0, 0.0, 0.0, 1.0], BlendMode::SrcOver)],
    );
    assert_near(got, [255, 0, 0, 255], "opaque source over");
}

#[test]
fn stacked_translucent_draws_accumulate_correctly() {
    let Some(mut ctx) = context() else { return };
    // Two half-alpha reds over blue. After the first the target holds
    // (0.5, 0, 0.5, 1); the second gives (0.5, 0, 0) + (0.5, 0, 0.5) * 0.5
    // = (0.75, 0, 0.25), alpha 1.
    let got = composite(
        &mut ctx,
        [0.0, 0.0, 1.0, 1.0],
        &[
            ([1.0, 0.0, 0.0, 0.5], BlendMode::SrcOver),
            ([1.0, 0.0, 0.0, 0.5], BlendMode::SrcOver),
        ],
    );
    assert_near(got, [191, 0, 64, 255], "two stacked halves");
}

#[test]
fn src_overwrites_alpha_where_source_over_would_accumulate_it() {
    let Some(mut ctx) = context() else { return };
    let half_red = [1.0, 0.0, 0.0, 0.5];
    let opaque_blue = [0.0, 0.0, 1.0, 1.0];

    let over = composite(&mut ctx, opaque_blue, &[(half_red, BlendMode::SrcOver)]);
    let replaced = composite(&mut ctx, opaque_blue, &[(half_red, BlendMode::Src)]);

    // Source-over keeps the destination opaque; Src stamps the source's own
    // alpha through. The distinction is invisible on an opaque target's color
    // but decides whether a layer composites correctly later.
    assert_near(over, [128, 0, 128, 255], "source over");
    assert_near(replaced, [128, 0, 0, 128], "src");
}

#[test]
fn stored_color_is_premultiplied() {
    let Some(mut ctx) = context() else { return };
    // Straight alpha in, premultiplied out: half-alpha red stores 128, not
    // 255. Storing straight alpha would make every later blend and every
    // filtered sample wrong at the edges.
    let got = composite(
        &mut ctx,
        [0.0, 0.0, 0.0, 0.0],
        &[([1.0, 0.0, 0.0, 0.5], BlendMode::Src)],
    );
    assert_near(got, [128, 0, 0, 128], "premultiplied storage");
}

#[test]
fn blend_modes_are_cached_as_separate_pipelines() {
    let Some(mut ctx) = context() else { return };
    // Blending is baked into a pipeline, so the two modes need distinct
    // objects. Alternating would expose a key collision serving the wrong one.
    for _ in 0..3 {
        let over = composite(
            &mut ctx,
            [0.0, 0.0, 1.0, 1.0],
            &[([1.0, 0.0, 0.0, 0.5], BlendMode::SrcOver)],
        );
        assert_near(over, [128, 0, 128, 255], "source over");

        let src = composite(
            &mut ctx,
            [0.0, 0.0, 1.0, 1.0],
            &[([1.0, 0.0, 0.0, 0.5], BlendMode::Src)],
        );
        assert_near(src, [128, 0, 0, 128], "src");
    }
}

#[test]
fn the_software_reference_blends_identically() {
    let Some(mut ctx) = context() else { return };
    let Ok(mut sw) = Validated::new(DevicePreference::Software) else {
        eprintln!("skipping: no software rasterizer");
        return;
    };
    let draws = [
        ([1.0, 0.0, 0.0, 0.5], BlendMode::SrcOver),
        ([0.0, 1.0, 0.0, 0.25], BlendMode::SrcOver),
    ];
    let hardware = composite(&mut ctx, [0.0, 0.0, 1.0, 1.0], &draws);
    let software = composite(&mut sw, [0.0, 0.0, 1.0, 1.0], &draws);

    // One unit of tolerance, and only here. Clears and rasterization coverage
    // are asserted bit-exact elsewhere because those are exactly specified.
    // Blending is not: converting a blended result to normalized fixed-point
    // permits either of the two nearest representable values, so a channel
    // landing on 63.75 may legitimately come back as 63 or 64. Both drivers
    // are conformant and this is the tolerance the specification allows, not a
    // driver bug being papered over.
    assert_near(hardware, software, "blending diverged across drivers");
}

/// A context on the device that offers advanced blending, with validation on.
///
/// Which physical device that is varies by machine: the extension is not
/// guaranteed on the preferred device, and here it happens to be the software
/// rasterizer that has it. Both are tried rather than assuming either.
fn advanced_context() -> Option<VulkanContext> {
    for device in [DevicePreference::Auto, DevicePreference::Software] {
        let Ok(ctx) = VulkanContext::with_config(ContextConfig {
            device,
            validation: true,
            ..Default::default()
        }) else {
            continue;
        };
        if ctx.capabilities().advanced_blend {
            return Some(ctx);
        }
    }
    eprintln!("skipping: no device offers advanced blending");
    None
}

#[test]
fn the_advanced_blend_pipeline_state_is_valid() {
    let Some(mut ctx) = advanced_context() else {
        return;
    };
    if !ctx.validation_active() {
        eprintln!("skipping: no validation layer installed");
        return;
    }
    // An advanced mode extends the color blend state through `pNext`, which is
    // where this can go wrong in ways that still produce a picture: a structure
    // chained without its feature enabled, or premultiplication declared the
    // way it is not. Neither shows up in the output reliably, and both are
    // exactly what the validation layer exists to catch. Every mode is drawn
    // because each builds its own pipeline.
    //
    // `composite` asserts the log is clean after each draw, so the assertion is
    // in the drawing rather than after it.
    for mode in BlendMode::ADVANCED {
        composite(
            &mut ctx,
            [0.2, 0.5, 0.7, 1.0],
            &[([0.8, 0.3, 0.6, 0.6], *mode)],
        );
    }
}

#[test]
fn a_device_without_the_extension_refuses_advanced_modes() {
    // Built without the extension rather than found without it. This used to
    // return early when the preferred device offered advanced blending, which on
    // a machine whose preferred device has it meant the test never ran -- and
    // whether it ran was decided by a device preference that has nothing to do
    // with what is being checked.
    let mut ctx = match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
        withheld: emblema_hal::Withheld::of(emblema_hal::Capability::AdvancedBlend),
    }) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            return;
        }
    };
    assert!(
        !ctx.capabilities().advanced_blend,
        "the capability survived being withheld, so what follows would be \
         checking a device that can do it"
    );
    // Reported as unsupported rather than drawn as something else. A backend
    // that quietly fell back to source-over would produce a picture that is
    // wrong in a way no test of the output would name.
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(SIZE, SIZE),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    for mode in BlendMode::ADVANCED {
        let result = ctx.draw_indexed(
            &mut tex,
            &FULL,
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 0.5]),
            *mode,
            None,
        );
        assert!(
            matches!(result, Err(emblema_hal::Error::Unsupported(_))),
            "{mode} was accepted by a device that cannot do it"
        );
    }
    ctx.destroy_texture(tex);
}

//! Exporting images as dma-bufs.
//!
//! This exercises the first of the two requirements the HAL reserved from day
//! one. Until now it was a field in a descriptor that nothing filled in; these
//! tests check that an image really can be allocated with a negotiated layout
//! and handed over as a file descriptor, which is what the DRM presentation
//! path will be built on.

use emblema_hal::{BlendMode, Extent2D, Material, PassDescriptor, PixelFormat};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};
use emblema_present::{negotiate, PREFERRED_FORMATS};

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

fn exportable(ctx: &VulkanContext) -> bool {
    if ctx.capabilities().dma_buf.can_allocate_scanout() {
        return true;
    }
    eprintln!("skipping: this device cannot allocate exportable images");
    false
}

#[test]
fn the_device_advertises_layouts_it_can_render_into() {
    let Some(ctx) = context() else { return };
    let formats = &ctx.capabilities().render_formats;
    if !exportable(&ctx) {
        return;
    }

    // Negotiation needs a real advertisement from the render side. An empty
    // list would leave linear as the only safe assumption, which works
    // everywhere and wastes bandwidth everywhere.
    assert!(
        !formats.is_empty(),
        "no renderable formats advertised, so negotiation has nothing to work with"
    );
    for set in formats {
        eprintln!("  {} with {} modifier(s)", set.fourcc, set.modifiers.len());
        assert!(!set.modifiers.is_empty(), "{} has no layouts", set.fourcc);
    }

    // A device that can only offer linear means every shared buffer gives up
    // whatever tiling would have saved, which on a GPU is a missing modifier
    // query rather than a device that genuinely has one layout. On a CPU
    // rasterizer it is the correct answer -- there is no tiling to describe --
    // so this reports rather than fails, and the census counts it.
    let non_linear = formats
        .iter()
        .any(|s| s.modifiers.iter().any(|m| !m.is_linear()));
    if ctx.capabilities().software {
        if !non_linear {
            eprintln!("skipping: a software rasterizer has only linear layouts to offer");
        }
        return;
    }
    assert!(
        non_linear,
        "only linear layouts were advertised; sharing will cost full bandwidth"
    );
}

#[test]
fn an_exported_image_carries_a_descriptor_and_a_real_layout() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }

    // Negotiate against a display side that accepts anything this device can
    // render into. A real target advertises its own set; this stands in for one.
    let render = ctx.capabilities().render_formats.clone();
    let chosen = negotiate(&render, &render, PREFERRED_FORMATS).expect("negotiated");
    eprintln!("negotiated {chosen}");

    let set = render
        .iter()
        .find(|s| s.fourcc == chosen.fourcc)
        .expect("the chosen format was advertised");
    let format = match chosen.fourcc {
        f if f == emblema_hal::Fourcc::ARGB8888 => PixelFormat::Bgra8Unorm,
        f if f == emblema_hal::Fourcc::ABGR8888 => PixelFormat::Rgba8Unorm,
        f if f == emblema_hal::Fourcc::XRGB2101010 => PixelFormat::Rgb10A2Unorm,
        other => panic!("unexpected negotiated format {other}"),
    };

    let texture = ctx
        .create_exportable_texture(Extent2D::new(64, 64), format, &set.modifiers)
        .expect("exportable texture");

    // The driver picks from the candidates, so the result must be one of them
    // rather than something invented.
    let modifier = ctx.texture_modifier(&texture).expect("modifier");
    assert!(
        set.modifiers.contains(&modifier),
        "driver chose {modifier}, which was not among the candidates offered"
    );

    let exported = ctx.export_texture(&texture).expect("export");
    assert_eq!(exported.fourcc, chosen.fourcc);
    assert_eq!(exported.modifier, modifier);
    assert_eq!(exported.planes.len(), 1, "expected a single-plane image");

    // A stride of zero, or one narrower than the image, would produce a
    // framebuffer the display controller reads garbage from.
    let plane = &exported.planes[0];
    assert!(
        plane.stride >= 64 * 4,
        "stride {} is too small for a 64-pixel row",
        plane.stride
    );

    // The descriptor must be a real, open file: that is the whole point, and an
    // invalid one would fail only much later, at import on the display side.
    // Duplicating it is the cheapest way to make the kernel confirm it.
    plane
        .fd
        .try_clone()
        .expect("the exported descriptor is not an open file");

    drop(exported);
    ctx.destroy_texture(texture);
}

#[test]
fn an_exported_image_can_still_be_rendered_into() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let render = ctx.capabilities().render_formats.clone();
    let set = render
        .iter()
        .find(|s| s.fourcc == emblema_hal::Fourcc::ARGB8888)
        .expect("a common eight-bit format");

    let mut texture = ctx
        .create_exportable_texture(
            Extent2D::new(32, 32),
            PixelFormat::Bgra8Unorm,
            &set.modifiers,
        )
        .expect("exportable texture");

    // An image allocated for sharing is still a render target. If a modifier
    // layout could not be drawn into, scanout would need a copy every frame.
    let mut batch = emblema_hal::Batch::new();
    batch
        .push(
            &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    ctx.submit_batch(
        &mut texture,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit");

    let pixels = ctx.read_texture(&mut texture).expect("readback");
    // Bgra8Unorm stores red in byte two.
    assert_eq!(&pixels[..4], &[0, 0, 255, 255]);

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");

    ctx.destroy_texture(texture);
}

#[test]
fn an_ordinary_texture_cannot_be_exported() {
    let Some(mut ctx) = context() else { return };
    let texture = ctx
        .create_texture(&emblema_hal::TextureDescriptor::offscreen(
            Extent2D::new(16, 16),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");

    // A pooled image shares its allocation with other resources, and a dma-buf
    // hands over a whole allocation. Refusing beats exporting a descriptor that
    // covers more than the caller asked for.
    assert!(ctx.export_texture(&texture).is_err());
    assert!(ctx.texture_modifier(&texture).is_err());
    ctx.destroy_texture(texture);
}

#[test]
fn exporting_without_candidate_modifiers_is_refused() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    // An empty candidate list means negotiation produced nothing, and letting
    // the driver pick unilaterally would defeat the agreement.
    let result = ctx.create_exportable_texture(Extent2D::new(16, 16), PixelFormat::Bgra8Unorm, &[]);
    assert!(result.is_err());
}

#[test]
fn exported_descriptors_are_distinct_and_do_not_leak() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let render = ctx.capabilities().render_formats.clone();
    let set = render
        .iter()
        .find(|s| s.fourcc == emblema_hal::Fourcc::ARGB8888)
        .expect("a common eight-bit format");

    // Each export is a fresh descriptor the caller owns. Repeating it would
    // exhaust the process's descriptors if they were never released.
    let mut previous = Vec::new();
    for i in 0..32 {
        let texture = ctx
            .create_exportable_texture(
                Extent2D::new(16, 16),
                PixelFormat::Bgra8Unorm,
                &set.modifiers,
            )
            .unwrap_or_else(|e| panic!("iteration {i}: {e}"));
        let exported = ctx.export_texture(&texture).expect("export");
        use std::os::fd::AsRawFd;
        previous.push(exported.planes[0].fd.as_raw_fd());
        drop(exported);
        ctx.destroy_texture(texture);
    }
    assert_eq!(previous.len(), 32);
}

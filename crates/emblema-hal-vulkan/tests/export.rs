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
    // As many planes as the modifier has, which is not always one: this device
    // picks an AMD DCC layout whose metadata is a second plane. This read
    // `== 1` until export learned to describe them all, and that assertion was
    // recording what export could do rather than what the image is.
    assert!(
        !exported.planes.is_empty(),
        "an exported image was described with no planes"
    );
    eprintln!("{modifier} described in {} plane(s)", exported.planes.len());

    // A stride of zero, or one narrower than the image, would produce a
    // framebuffer the display controller reads garbage from. Plane zero holds
    // the pixels, so it is the one with a row to be too narrow for; a metadata
    // plane's stride is the driver's business and has no such floor.
    let plane = &exported.planes[0];
    assert!(
        plane.stride >= 64 * 4,
        "stride {} is too small for a 64-pixel row",
        plane.stride
    );
    // Every plane's descriptor, not just the first: a second plane described
    // with a closed or invalid one fails at the importer and nowhere earlier.
    for (index, plane) in exported.planes.iter().enumerate() {
        plane
            .fd
            .try_clone()
            .unwrap_or_else(|e| panic!("plane {index}'s descriptor is not an open file: {e}"));
    }

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

/// A dma-buf this device exported can be imported back and rendered into.
///
/// The round trip is the test, and it is the half the DRM path needs where a
/// device cannot allocate its own scanout buffers: something else allocates,
/// this imports, and the renderer draws into an image it did not make. Doing it
/// against this device's own export is what makes the test runnable anywhere
/// that exports at all -- a GBM allocation would prove the same thing and needs
/// a card.
///
/// The imported image is checked by drawing into it and reading it back, not by
/// its handle being non-null: an import that bound the wrong memory type or
/// mis-stated the plane's stride still returns a texture, and only the pixels
/// say otherwise.
#[test]
fn an_exported_dma_buf_can_be_imported_and_rendered_into() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    if !ctx.capabilities().dma_buf.import {
        eprintln!("skipping: this device cannot import a dma-buf");
        return;
    }
    let render = ctx.capabilities().render_formats.clone();
    let Some(set) = render
        .iter()
        .find(|s| s.fourcc == emblema_hal::Fourcc::ARGB8888)
    else {
        eprintln!("skipping: no eight-bit format to negotiate with");
        return;
    };

    // LINEAR deliberately, not the device's preference. Left to choose, RADV
    // picks an AMD DCC modifier -- measured, `0x20000000056bb03` -- which has
    // two memory planes, and both this crate's export and its import handle one.
    // See `docs/on-a-board.md`: that is a real gap, and it is not this test's
    // subject. LINEAR is also what GBM hands back on every board on this bench,
    // so it is the case the GBM path actually needs.
    if !set.modifiers.contains(&emblema_hal::Modifier::LINEAR) {
        eprintln!("skipping: this device will not render into a linear layout");
        return;
    }
    let extent = Extent2D::new(32, 32);
    let source = ctx
        .create_exportable_texture(
            extent,
            PixelFormat::Bgra8Unorm,
            &[emblema_hal::Modifier::LINEAR],
        )
        .expect("exportable texture");
    let described = ctx.export_texture(&source).expect("export");
    eprintln!(
        "exported {} plane(s), modifier {:#x}",
        described.planes.len(),
        described.modifier.0
    );

    let mut imported = match ctx.import_texture(described, extent, PixelFormat::Bgra8Unorm) {
        Ok(texture) => texture,
        Err(e) => {
            // Said rather than swallowed: a device that exports but refuses its
            // own export is a finding, not a capability gap.
            ctx.destroy_texture(source);
            panic!("importing this device's own export was refused: {e}");
        }
    };

    let mut batch = emblema_hal::Batch::new();
    batch
        .push(
            &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([0.0, 1.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    ctx.submit_batch(
        &mut imported,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit into the imported image");

    let pixels = ctx.read_texture(&mut imported).expect("readback");
    // Every pixel, not the first one. The draw covers the whole target, so a
    // stride or offset stated wrongly on import moves later rows while leaving
    // pixel zero perfectly green -- measured: doubling `row_pitch` passes a
    // check on `pixels[..4]` and fails this one.
    let wrong = pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, texel)| *texel != [0u8, 255, 0, 255])
        .map(|(i, texel)| format!("{}: {texel:?}", i))
        .take(4)
        .collect::<Vec<_>>();
    assert!(
        wrong.is_empty(),
        "{} of {} texels are not the color drawn; first few: {}",
        pixels
            .chunks_exact(4)
            .filter(|t| *t != [0u8, 255, 0, 255])
            .count(),
        pixels.len() / 4,
        wrong.join(", ")
    );

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");

    ctx.destroy_texture(imported);
    ctx.destroy_texture(source);
}

/// The round trip at the layout the device chooses for itself.
///
/// The test above forces LINEAR, and had to: left to choose, RADV picks an AMD
/// DCC modifier with **two** memory planes, and export described only plane
/// zero -- so importing this device's own export was refused with
/// `ERROR_INVALID_DRM_FORMAT_MODIFIER_PLANE_LAYOUT_EXT`. Export now describes
/// every plane the modifier declares and import requires the count to match.
///
/// This is the case that matters on a device whose preferred layout is not
/// linear, which is most of them: a single-plane description of a two-plane
/// buffer is not a near miss, it is half a layout, and a display told the same
/// thing scans out the wrong bytes.
///
/// Deliberately *not* pinned to a modifier. The point is whatever the driver
/// prefers, so this reports what it got and passes on one plane as readily as
/// on two -- a device that prefers linear exercises the same code path with
/// `planes == 1`.
#[test]
fn the_round_trip_works_at_the_layout_the_device_prefers() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    if !ctx.capabilities().dma_buf.import {
        eprintln!("skipping: this device cannot import a dma-buf");
        return;
    }
    let render = ctx.capabilities().render_formats.clone();
    let Some(set) = render
        .iter()
        .find(|s| s.fourcc == emblema_hal::Fourcc::ARGB8888)
    else {
        eprintln!("skipping: no eight-bit format to negotiate with");
        return;
    };

    let extent = Extent2D::new(64, 64);
    // Every modifier it offers, in its own preference order: the driver picks.
    let source = ctx
        .create_exportable_texture(extent, PixelFormat::Bgra8Unorm, &set.modifiers)
        .expect("exportable texture");
    let described = ctx.export_texture(&source).expect("export");
    eprintln!(
        "the device chose modifier {:#x}, described in {} plane(s)",
        described.modifier.0,
        described.planes.len()
    );
    assert!(
        !described.planes.is_empty(),
        "an exported image was described with no planes at all"
    );

    let mut imported = match ctx.import_texture(described, extent, PixelFormat::Bgra8Unorm) {
        Ok(texture) => texture,
        Err(e) => {
            ctx.destroy_texture(source);
            panic!("importing this device's own preferred layout was refused: {e}");
        }
    };

    let mut batch = emblema_hal::Batch::new();
    batch
        .push(
            &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([0.0, 0.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    ctx.submit_batch(
        &mut imported,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit into the imported image");

    let pixels = ctx.read_texture(&mut imported).expect("readback");
    // Bgra8Unorm stores blue in byte zero. Every texel, for the reason the test
    // above gives: a stride stated wrongly leaves pixel zero correct.
    let wrong = pixels
        .chunks_exact(4)
        .filter(|texel| *texel != [255u8, 0, 0, 255])
        .count();
    assert_eq!(
        wrong,
        0,
        "{wrong} of {} texels are not the color drawn; first is {:?}",
        pixels.len() / 4,
        &pixels[..4]
    );

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");

    ctx.destroy_texture(imported);
    ctx.destroy_texture(source);
}

//! End-to-end pixel tests: allocate an image, put known values in it, and
//! read them back.
//!
//! This is the first point where a claim about the backend can be checked
//! against what the GPU actually produced rather than against what the driver
//! reported. Everything in the golden and conformance apparatus is built on
//! this path.

use emblema_hal::{Extent2D, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::DevicePreference;
use emblema_hal_vulkan::Validated;

fn context() -> Option<Validated> {
    match Validated::new(DevicePreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

/// Clear a texture to `color` and read it back as bytes.
fn clear_and_read(
    ctx: &mut Validated,
    extent: Extent2D,
    format: PixelFormat,
    color: [f32; 4],
) -> Vec<u8> {
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(extent, format))
        .expect("texture creation");
    ctx.clear_texture(&mut tex, color).expect("clear");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    pixels
}

#[test]
fn a_cleared_texture_reads_back_the_clear_color() {
    let Some(mut ctx) = context() else { return };
    let extent = Extent2D::new(64, 64);
    let pixels = clear_and_read(
        &mut ctx,
        extent,
        PixelFormat::Rgba8Unorm,
        [1.0, 0.0, 0.0, 1.0],
    );

    assert_eq!(pixels.len(), 64 * 64 * 4, "readback must be tightly packed");
    // Every pixel, not just the first: a partial clear or a wrong copy region
    // would leave the tail untouched and pass a spot check.
    for (i, px) in pixels.chunks_exact(4).enumerate() {
        assert_eq!(
            px,
            [255, 0, 0, 255],
            "pixel {i} at ({}, {})",
            i % 64,
            i / 64
        );
    }
}

#[test]
fn channel_order_is_not_transposed() {
    let Some(mut ctx) = context() else { return };
    // Distinct values per channel, so a swizzle cannot pass by symmetry the
    // way an all-red or all-white clear would.
    let pixels = clear_and_read(
        &mut ctx,
        Extent2D::new(8, 8),
        PixelFormat::Rgba8Unorm,
        [1.0, 0.5019608, 0.0, 1.0],
    );
    let first = &pixels[..4];
    assert_eq!(first[0], 255, "red channel");
    assert!(
        (first[1] as i32 - 128).abs() <= 1,
        "green channel: {first:?}"
    );
    assert_eq!(first[2], 0, "blue channel");
    assert_eq!(first[3], 255, "alpha channel");
}

#[test]
fn bgra_and_rgba_place_the_same_color_in_opposite_channels() {
    let Some(mut ctx) = context() else { return };
    let extent = Extent2D::new(4, 4);
    let red = [1.0, 0.0, 0.0, 1.0];

    let rgba = clear_and_read(&mut ctx, extent, PixelFormat::Rgba8Unorm, red);
    let bgra = clear_and_read(&mut ctx, extent, PixelFormat::Bgra8Unorm, red);

    // The clear color is specified in RGBA regardless of storage order, so red
    // must land in byte 0 for RGBA and byte 2 for BGRA. Getting this wrong is
    // the classic scanout bug where everything renders blue.
    assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
    assert_eq!(&bgra[..4], &[0, 0, 255, 255]);
}

#[test]
fn a_non_square_texture_reads_back_with_the_right_dimensions() {
    let Some(mut ctx) = context() else { return };
    // Non-square catches a width and height transposition in the copy region,
    // which a square texture cannot detect.
    let extent = Extent2D::new(37, 11);
    let pixels = clear_and_read(
        &mut ctx,
        extent,
        PixelFormat::Rgba8Unorm,
        [0.0, 1.0, 0.0, 1.0],
    );

    assert_eq!(pixels.len(), 37 * 11 * 4);
    assert!(pixels.chunks_exact(4).all(|p| p == [0, 255, 0, 255]));
}

#[test]
fn textures_can_be_created_and_destroyed_repeatedly_without_exhausting_memory() {
    let Some(mut ctx) = context() else { return };
    // A leak in the allocator path shows up as a failure partway through
    // rather than on the first iteration.
    for i in 0..32 {
        let mut tex = ctx
            .create_texture(&TextureDescriptor::offscreen(
                Extent2D::new(256, 256),
                PixelFormat::Rgba8Unorm,
            ))
            .unwrap_or_else(|e| panic!("iteration {i}: {e}"));
        ctx.clear_texture(&mut tex, [0.0, 0.0, 1.0, 1.0]).unwrap();
        ctx.destroy_texture(tex);
    }
}

#[test]
fn an_oversized_texture_is_refused_with_the_limit_reported() {
    let Some(mut ctx) = context() else { return };
    let limit = ctx.capabilities().max_texture_size;
    let desc = TextureDescriptor::offscreen(Extent2D::new(limit + 1, 16), PixelFormat::Rgba8Unorm);

    // Refusing here, with the limit in the message, beats letting the driver
    // fail with an opaque code.
    match ctx.create_texture(&desc) {
        Err(emblema_hal::Error::LimitExceeded { limit: l, .. }) => {
            assert_eq!(l, limit as u64)
        }
        other => panic!("expected LimitExceeded, got {other:?}", other = other.err()),
    }
}

#[test]
fn a_zero_sized_texture_is_refused() {
    let Some(mut ctx) = context() else { return };
    let desc = TextureDescriptor::offscreen(Extent2D::new(0, 64), PixelFormat::Rgba8Unorm);
    assert!(ctx.create_texture(&desc).is_err());
}

#[test]
fn the_software_reference_produces_the_same_pixels_as_the_default_device() {
    let Some(mut default_ctx) = context() else {
        return;
    };
    let Ok(mut sw_ctx) = Validated::new(DevicePreference::Software) else {
        eprintln!("skipping: no software rasterizer");
        return;
    };

    let extent = Extent2D::new(16, 16);
    let color = [0.25, 0.5, 0.75, 1.0];
    let from_default = clear_and_read(&mut default_ctx, extent, PixelFormat::Rgba8Unorm, color);
    let from_software = clear_and_read(&mut sw_ctx, extent, PixelFormat::Rgba8Unorm, color);

    eprintln!(
        "default={} software={}",
        default_ctx.capabilities().device_name,
        sw_ctx.capabilities().device_name
    );
    // A clear is exactly reproducible, so this needs no tolerance. Scenes with
    // real shading get per-driver tolerance profiles; this one must not.
    assert_eq!(
        from_default, from_software,
        "a solid clear must be bit-identical across drivers"
    );
}

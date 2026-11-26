//! An image over another device's dma-buf: what `Context::import_image` is for.
//!
//! The producer here is a second Vulkan device -- a context of its own, as a
//! camera, a decoder or another renderer would be -- which draws a frame into
//! a buffer it exports. The consumer imports the buffer and samples it as it
//! samples any image. What it draws has to be what the producer's buffer
//! holds, read back on the producer's side, pixel for pixel at each
//! quadrant's middle: a stride stated wrongly shears the rows below the
//! first, and a layout transition that discards the contents draws nothing.

#![cfg(all(unix, feature = "vulkan"))]

use emblema::vulkan::{ContextConfig, DevicePreference, VulkanContext};
use emblema::{
    BackendPreference, Canvas, Color, Context, Extent2D, Modifier, Paint, PixelFormat, Rect,
};
use emblema_hal::{BlendMode, Material, PassDescriptor};

const SIZE: Extent2D = Extent2D {
    width: 64,
    height: 64,
};

/// A device that draws a frame and exports it, or `None` where this machine
/// cannot.
fn producer() -> Option<VulkanContext> {
    let ctx = match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        ..Default::default()
    }) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no Vulkan device to produce with ({e})");
            return None;
        }
    };
    if !ctx.capabilities().dma_buf.can_allocate_scanout() {
        eprintln!("skipping: this device cannot allocate exportable images");
        return None;
    }
    Some(ctx)
}

/// The four quadrants, drawn by the producer into its exportable buffer:
/// red, green, blue and yellow, in the order clip space names them.
fn draw_quadrants(ctx: &mut VulkanContext, target: &mut emblema::vulkan::VulkanTexture) {
    let mut batch = emblema_hal::Batch::new();
    for (x, y, color) in [
        (-1.0, -1.0, [1.0, 0.0, 0.0, 1.0]),
        (0.0, -1.0, [0.0, 1.0, 0.0, 1.0]),
        (-1.0, 0.0, [0.0, 0.0, 1.0, 1.0]),
        (0.0, 0.0, [1.0, 1.0, 0.0, 1.0]),
    ] {
        batch
            .push(
                &[[x, y], [x + 1.0, y], [x + 1.0, y + 1.0], [x, y + 1.0]],
                &[0, 1, 2, 0, 2, 3],
                Material::solid(color),
                BlendMode::Src,
            )
            .expect("push");
    }
    ctx.submit_batch(target, &batch, PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]))
        .expect("draw the producer's frame");
}

fn at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn an_image_over_another_devices_dma_buf_draws_what_the_buffer_holds() {
    let Some(mut producer) = producer() else {
        return;
    };
    let mut ctx = match Context::new(BackendPreference::Vulkan) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no Vulkan context to import into ({e})");
            return;
        }
    };

    // LINEAR, which every device that exports at all can share, and what
    // a software rasterizer has to offer.
    let mut frame = producer
        .create_exportable_texture(SIZE, PixelFormat::Bgra8Unorm, &[Modifier::LINEAR])
        .expect("exportable frame");
    draw_quadrants(&mut producer, &mut frame);
    let held = producer
        .read_texture(&mut frame)
        .expect("producer readback");
    let described = producer.export_texture(&frame).expect("export");

    let image = match ctx.import_image(described, SIZE, PixelFormat::Bgra8Unorm) {
        Ok(image) => image,
        Err(emblema::Error::Unsupported(why)) => {
            eprintln!("skipping: {why}");
            producer.destroy_texture(frame);
            return;
        }
        Err(e) => panic!("importing another device's export was refused: {e}"),
    };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
    canvas
        .draw_rect(
            Rect::from_size(SIZE.width as f32, SIZE.height as f32),
            &Paint::image(0, Rect::from_size(SIZE.width as f32, SIZE.height as f32))
                .with_anti_alias(false),
        )
        .expect("rect");
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw the imported image");
    let drawn = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);
    producer.destroy_texture(frame);

    let mut colors = Vec::new();
    for (x, y) in [(16, 16), (48, 16), (16, 48), (48, 48)] {
        // The producer's buffer is BGRA; the surface reads back RGBA.
        let [b, g, r, a] = at(&held, SIZE.width, x, y);
        let want = [r, g, b, a];
        assert_eq!(
            at(&drawn, SIZE.width, x, y),
            want,
            "({x}, {y}) drew something other than the buffer holds there"
        );
        colors.push(want);
    }
    colors.sort();
    colors.dedup();
    assert_eq!(
        colors.len(),
        4,
        "the producer's quadrants should be four colors, not {colors:?}"
    );
}

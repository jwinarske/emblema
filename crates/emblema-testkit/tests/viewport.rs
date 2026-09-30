//! A pass whose clip space is larger than the target it lands in.
//!
//! A layer records its draws before anything is known about how much of the
//! target they cover, and `docs/architecture.md` records that a recording is
//! tessellated geometry rather than a command list — so by the time the extent
//! could be narrowed, every vertex and every material is already in the clip
//! space of the target that was aimed at. Narrowing the *target* instead, and
//! sliding the viewport so the wanted region lands on it, leaves all of that
//! alone. These say the two backends actually do that.

use emblema_core::{Canvas, Color, Extent2D, Paint, Rect};
use emblema_hal::{PassDescriptor, PassViewport, PixelFormat, TextureDescriptor};
use emblema_hal_gles::DisplayTarget;
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_vulkan::DevicePreference;
use emblema_hal_vulkan::Validated;

/// The space the geometry is recorded against.
const RECORDED: Extent2D = Extent2D::new(64, 64);
/// The target it is cropped into: the bottom-right quarter.
const CROPPED: Extent2D = Extent2D::new(32, 32);

/// A red square covering device (32,32)..(48,48) of a 64x64 space.
///
/// Placed in the quarter the crop keeps, and not filling it, so that a pass
/// which *scaled* rather than cropped puts it somewhere else: at half the
/// recorded size the same square would land at (16,16)..(24,24) instead of at
/// the origin, which the assertions below tell apart.
fn recorded() -> emblema_core::Recording {
    let mut canvas = Canvas::new(RECORDED);
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 48.0, 48.0),
            &Paint::fill(Color::srgb(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("a square");
    canvas.finish()
}

fn cropping() -> PassDescriptor {
    PassDescriptor {
        clear: Some([0.0, 0.0, 0.0, 1.0]),
        samples: 1,
        // The recorded extent is kept; the offset slides its origin off the
        // top-left of the target so that device (32,32) lands at (0,0).
        viewport: Some(PassViewport {
            offset: [-32.0, -32.0],
            extent: RECORDED,
        }),
    }
}

fn at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * CROPPED.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[track_caller]
fn check(pixels: &[u8], backend: &str) {
    // The square's own corner, which the crop puts at the target's origin.
    assert_eq!(
        at(pixels, 1, 1),
        [255, 0, 0, 255],
        "{backend} did not put the recorded (33,33) at the target's origin"
    );
    assert_eq!(
        at(pixels, 14, 14),
        [255, 0, 0, 255],
        "{backend} lost the far corner of the square"
    );
    // Just past it, which is background. A pass that scaled the recorded space
    // down to the target instead would still be inside the square here.
    assert_eq!(
        at(pixels, 20, 20),
        [0, 0, 0, 255],
        "{backend} scaled the recorded space instead of cropping it"
    );
}

#[test]
fn a_viewport_crops_the_recorded_space_rather_than_scaling_it() {
    let recording = recorded();
    let batch = &recording.passes[0].batch;
    let mut ran = 0;

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(
                CROPPED,
                PixelFormat::Rgba8Unorm,
            ))
            .expect("target");
        ctx.submit_batch(&mut target, batch, cropping())
            .expect("submit");
        let pixels = ctx.read_texture(&mut target).expect("readback");
        ctx.destroy_texture(target);
        check(&pixels, "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(
                CROPPED,
                PixelFormat::Rgba8Unorm,
            ))
            .expect("target");
        ctx.submit_batch(&mut target, batch, cropping())
            .expect("submit");
        let pixels = ctx.read_texture(&mut target).expect("readback");
        ctx.destroy_texture(target);
        check(&pixels, "gles");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

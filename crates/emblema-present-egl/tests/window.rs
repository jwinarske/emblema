//! Presenting to an EGL surface, against one with no window behind it.
//!
//! The claim these have to establish is one no test here can look at: whether a
//! window system would show the frame the right way up. So they establish the
//! property that decides it instead — a presented frame is the vertical mirror
//! of the frame as rendered — and pin the reason, which is that everything this
//! renderer draws puts the image's top row at a framebuffer's row zero while a
//! window system reads row zero as the bottom of what it shows.
//!
//! A scene symmetric top to bottom would satisfy that claim without meaning
//! anything, so nothing drawn here is.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it. A failed assumption in a test should stop the
// run; the workspace denies these because a *library* must not.
#![allow(clippy::panic)]

use emblema_hal::{Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat};
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesContext};
use emblema_present::PresentTarget;
use emblema_present_egl::{create_offscreen_surface, destroy_surface, WindowTarget};

const SIZE: Extent2D = Extent2D {
    width: 32,
    height: 24,
};
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A rectangle in clip space, from one given in pixels with a top-left origin.
///
/// Y is negated because clip space runs upward where an image's rows run down.
fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> [[f32; 2]; 4] {
    let to_clip = |x: f32, y: f32| {
        [
            x / SIZE.width as f32 * 2.0 - 1.0,
            1.0 - y / SIZE.height as f32 * 2.0,
        ]
    };
    [
        to_clip(x0, y0),
        to_clip(x1, y0),
        to_clip(x1, y1),
        to_clip(x0, y1),
    ]
}

/// A band across the top of the frame, and nothing else.
///
/// Deliberately not symmetric top to bottom: a mirror of this is a different
/// image, which is the entire point.
fn top_band() -> Batch {
    let mut batch = Batch::new();
    batch
        .push(
            &rect(0.0, 0.0, SIZE.width as f32, 6.0),
            &QUAD,
            Material::solid([1.0, 1.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    batch
}

fn setup() -> Option<(GlesValidated, khronos_egl::Surface)> {
    let ctx = match GlesValidated::new(DisplayTarget::Surfaceless) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no GLES context ({e})");
            return None;
        }
    };
    match create_offscreen_surface(&ctx, SIZE) {
        Ok(surface) => Some((ctx, surface)),
        Err(e) => {
            eprintln!("skipping: no offscreen surface ({e})");
            None
        }
    }
}

fn with_target(body: impl FnOnce(&mut GlesContext, &mut WindowTarget)) {
    let Some((mut ctx, surface)) = setup() else {
        return;
    };
    let mut target = match WindowTarget::new(&mut ctx, surface, SIZE, PixelFormat::Rgba8Unorm) {
        Ok(target) => target,
        Err(e) => {
            let _ = destroy_surface(&ctx, surface);
            panic!("window target: {e}");
        }
    };
    body(&mut ctx, &mut target);
    target.destroy(&mut ctx);
    destroy_surface(&ctx, surface).expect("destroy surface");
}

/// Whether a row of the image is lit.
fn row_is_lit(pixels: &[u8], row: u32) -> bool {
    let at = ((row * SIZE.width) * 4) as usize;
    pixels[at] > 128
}

#[test]
fn a_presented_frame_is_the_vertical_mirror_of_what_was_rendered() {
    with_target(|ctx, target| {
        // Rendered with a band across the top, in the renderer's own
        // orientation. What the window system is handed must be the mirror of
        // that, because it reads row zero as the bottom of what it shows.
        let image = target.acquire(ctx).expect("acquire");
        ctx.submit_batch(
            image,
            &top_band(),
            PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        )
        .expect("submit");

        let rendered = ctx.read_texture(image).expect("read the rendered frame");
        target.present(ctx).expect("present");
        // `read_presented` returns the window's buffer in the renderer's
        // orientation, undoing the same flip presenting applied — so a correct
        // present makes these two equal, and a missing flip makes them
        // mirrored.
        let presented = target.read_presented(ctx).expect("read the window buffer");

        assert!(
            row_is_lit(&rendered, 1),
            "the band was not drawn at the top of the rendered frame"
        );
        assert!(
            !row_is_lit(&rendered, SIZE.height - 2),
            "the rendered frame is symmetric and proves nothing"
        );
        assert_eq!(
            rendered, presented,
            "the presented frame is not the rendered one flipped into the window's orientation"
        );
    });
}

#[test]
fn the_flip_is_vertical_and_leaves_the_horizontal_axis_alone() {
    with_target(|ctx, target| {
        // A band down the left rather than across the top. Presenting must not
        // move it: exchanging the destination's columns instead of the
        // source's rows, or reversing both, would.
        let mut batch = Batch::new();
        batch
            .push(
                &rect(0.0, 0.0, 8.0, SIZE.height as f32),
                &QUAD,
                Material::solid([1.0, 1.0, 1.0, 1.0]),
                BlendMode::Src,
            )
            .expect("push");

        let image = target.acquire(ctx).expect("acquire");
        ctx.submit_batch(image, &batch, PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]))
            .expect("submit");
        let rendered = ctx.read_texture(image).expect("read");
        target.present(ctx).expect("present");
        let presented = target.read_presented(ctx).expect("read the window buffer");

        let lit = |pixels: &[u8], x: u32| {
            let at = ((SIZE.height / 2 * SIZE.width + x) * 4) as usize;
            pixels[at] > 128
        };
        assert!(
            lit(&rendered, 2) && !lit(&rendered, SIZE.width - 3),
            "left band"
        );
        assert_eq!(
            lit(&presented, 2),
            lit(&rendered, 2),
            "presenting moved the band horizontally"
        );
        assert_eq!(
            lit(&presented, SIZE.width - 3),
            lit(&rendered, SIZE.width - 3),
            "presenting moved the band horizontally"
        );
    });
}

#[test]
fn presenting_advances_the_frame_count() {
    with_target(|ctx, target| {
        assert_eq!(target.presented_frames(), 0);
        for expected in 1..=4u64 {
            let image = target.acquire(ctx).expect("acquire");
            ctx.submit_batch(image, &top_band(), PassDescriptor::clear([0.0; 4]))
                .expect("submit");
            target.present(ctx).expect("present");
            assert_eq!(target.presented_frames(), expected);
        }
    });
}

#[test]
fn an_unbalanced_acquire_or_present_is_refused() {
    with_target(|ctx, target| {
        assert!(target.present(ctx).is_err(), "present without acquire");
        let _ = target.acquire(ctx).expect("acquire");
        assert!(
            target.acquire(ctx).is_err(),
            "two frames were acquired at once"
        );
    });
}

#[test]
fn reconfiguring_changes_the_extent_and_keeps_the_target_usable() {
    with_target(|ctx, target| {
        // A window resize. The surface itself is the caller's to resize; what
        // this owns is the offscreen target frames are drawn into, and it has
        // to follow.
        let bigger = Extent2D::new(SIZE.width, SIZE.height * 2);
        target.reconfigure(ctx, bigger).expect("reconfigure");
        assert_eq!(target.extent(), bigger);

        let image = target.acquire(ctx).expect("acquire after reconfigure");
        assert_eq!(image.extent(), bigger);
        let mut batch = Batch::new();
        batch
            .push(
                &rect(0.0, 0.0, 4.0, 4.0),
                &QUAD,
                Material::solid([0.0, 1.0, 0.0, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        ctx.submit_batch(image, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
    });
}

#[test]
fn a_clip_left_from_the_last_draw_does_not_truncate_the_presented_frame() {
    with_target(|ctx, target| {
        // A blit is subject to the scissor test, so a clip the last draw left
        // enabled would present only the part of the frame that draw could
        // touch. The rest would hold whatever the window buffer held before,
        // which is a partial frame rather than an error.
        let image = target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push_clipped(
                &rect(0.0, 0.0, SIZE.width as f32, SIZE.height as f32),
                &QUAD,
                Material::solid([1.0, 1.0, 1.0, 1.0]),
                BlendMode::Src,
                Some(emblema_hal::Scissor::new(0, 0, 4, 4)),
            )
            .expect("push");
        ctx.submit_batch(image, &batch, PassDescriptor::clear([0.2, 0.2, 0.2, 1.0]))
            .expect("submit");
        let rendered = ctx.read_texture(image).expect("read");
        target.present(ctx).expect("present");
        let presented = target.read_presented(ctx).expect("read the window buffer");

        // Every pixel arrived, including the ones the clip excluded from the
        // draw but not from the frame.
        assert_eq!(rendered, presented, "the presented frame was truncated");
    });
}

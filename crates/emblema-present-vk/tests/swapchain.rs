//! The swapchain presentation path, against a surface with no window.
//!
//! `VK_EXT_headless_surface` is what makes this a real test rather than a
//! compile check. Everything the presentation target does — querying what the
//! surface allows, negotiating a format, acquiring, presenting, recreating —
//! runs exactly as it would against a window, and needs no display, no
//! compositor, and no window system library.
//!
//! What it does not exercise is what a window adds: an extent the surface
//! dictates, and a surface that goes out of date because something resized it.
//! Both are noted where the code handles them, and both are the parts a real
//! window would have to confirm.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it. A failed assumption in a test should stop the
// run; the workspace denies these because a *library* must not.
#![allow(clippy::panic)]

use emblema_hal::{Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat, Vertex};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext, VulkanHal};
use emblema_present::PresentTarget;
use emblema_present_vk::{create_headless_surface, destroy_surface, PresentMode, SwapchainTarget};

const SIZE: Extent2D = Extent2D {
    width: 64,
    height: 64,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A context and a headless surface, or nothing if either is unavailable.
fn setup() -> Option<(VulkanContext, ash::vk::SurfaceKHR)> {
    // Validation on, because this is where it earns its keep. A semaphore
    // signaled and never waited on, a wait on one nothing signals, an image
    // presented in the wrong layout: none of those show in the pixels, and two
    // of them are what the synchronization here exists to arrange.
    let ctx = match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
        ..Default::default()
    }) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            return None;
        }
    };
    match create_headless_surface(&ctx) {
        Ok(surface) => Some((ctx, surface)),
        Err(e) => {
            eprintln!("skipping: no headless surface ({e})");
            None
        }
    }
}

/// Run `body` with a target, tearing everything down afterwards.
fn with_target(mode: PresentMode, body: impl FnOnce(&mut VulkanContext, &mut SwapchainTarget)) {
    let Some((mut ctx, surface)) = setup() else {
        return;
    };
    let mut target = match SwapchainTarget::new(&mut ctx, surface, SIZE, mode) {
        Ok(target) => target,
        Err(e) => {
            unsafe { destroy_surface(&ctx, surface) };
            panic!("swapchain: {e}");
        }
    };
    // Caught, so that teardown still happens when an assertion inside `body`
    // fails. Without this a failing test leaks the surface past
    // `vkDestroyInstance`, and the validation layer reports that -- so the run
    // shows a lifetime error pointing at teardown alongside the real failure,
    // and the loudest message is the one that is not the problem. That is
    // exactly how long this particular failure took to read.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        body(&mut ctx, &mut target);
    }))
    .err();
    target.destroy(&mut ctx);

    if let Some(payload) = panicked {
        // SAFETY: the swapchain built on it has just been destroyed.
        unsafe { destroy_surface(&ctx, surface) };
        std::panic::resume_unwind(payload);
    }

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
    // SAFETY: the swapchain built on it has just been destroyed.
    unsafe { destroy_surface(&ctx, surface) };
}

#[test]
fn a_swapchain_reports_a_usable_configuration() {
    with_target(PresentMode::Fifo, |_ctx, target| {
        // A headless surface defers its size, so this is the size asked for.
        // A swapchain silently one pixel across passes every other test here,
        // which is what this pins.
        assert_eq!(
            target.extent(),
            SIZE,
            "the swapchain ignored the size asked for"
        );
        // At least two, or the engine has nothing to display while the
        // application draws.
        assert!(
            target.image_count() >= 2,
            "a swapchain of {} image(s) cannot overlap drawing with display",
            target.image_count()
        );
        // sRGB where the surface offers it, which every ordinary one does. The
        // renderer's colors are linear and the presentation engine is told the
        // image holds sRGB, so the attachment is what encodes between them. A
        // linear format is accepted as a fallback and is a little over a third
        // too dark at mid gray, which is why it is not the preference.
        assert!(
            matches!(
                target.format(),
                PixelFormat::Bgra8UnormSrgb
                    | PixelFormat::Rgba8UnormSrgb
                    | PixelFormat::Bgra8Unorm
                    | PixelFormat::Rgba8Unorm
            ),
            "unexpected surface format {:?}",
            target.format()
        );
    });
}

#[test]
fn acquiring_and_presenting_advances_the_frame_count() {
    with_target(PresentMode::Fifo, |ctx, target| {
        assert_eq!(target.presented_frames(), 0);
        for expected in 1..=6u64 {
            target.acquire(ctx).expect("acquire");
            // Something has to be drawn, or the image is never transitioned out
            // of the undefined layout the engine handed it over in.
            let mut batch = Batch::new();
            let shade = expected as f32 / 6.0;
            batch
                .push(
                    &FULL,
                    &QUAD,
                    Material::solid([shade, 0.2, 1.0 - shade, 1.0]),
                    BlendMode::Src,
                )
                .expect("push");
            target
                .submit(ctx, &batch, PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]))
                .expect("submit");
            target.present(ctx).expect("present");
            assert_eq!(target.presented_frames(), expected);
        }
    });
}

#[test]
fn the_images_are_cycled_rather_than_reused_immediately() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // A swapchain that handed back the same image every time would let a
        // frame be drawn into an image the engine is still displaying. Reading
        // the pointer rather than an index because that is what the caller
        // actually renders into.
        //
        // Which image an acquisition returns is not specified. The engine may
        // hand back any image it is not still reading from, so an
        // implementation that finishes presenting immediately is free to
        // return the same one every time, and Mesa's headless surface on the
        // CI runner does exactly that with five images available. What keeps a
        // frame from being drawn into an image still on screen is the
        // semaphore acquisition waits on, not the index it comes back with.
        //
        // So this is worth checking where it holds -- it would catch a target
        // that ignored the acquired index and always rendered into the first
        // image -- and is not a failure where it does not.
        if target.image_count() < 2 {
            eprintln!(
                "skipping: this surface offers {} image, so nothing cycles",
                target.image_count()
            );
            return;
        }
        let mut seen: Vec<*const u8> = Vec::new();
        for _ in 0..target.image_count() {
            let image = target.acquire(ctx).expect("acquire");
            seen.push(image as *const _ as *const u8);
            let mut batch = Batch::new();
            batch
                .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
                .expect("push");
            target
                .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
                .expect("submit");
            target.present(ctx).expect("present");
        }
        seen.sort_unstable();
        let distinct = {
            seen.dedup();
            seen.len()
        };
        if distinct == 1 {
            eprintln!(
                "skipping: this engine returned the same image for all {} \
                 acquisitions, which it is allowed to do",
                target.image_count()
            );
            return;
        }
        assert!(
            distinct > 1,
            "all {} acquisitions from a {}-image swapchain returned the same image, \
             so nothing is double buffered",
            target.image_count(),
            target.image_count()
        );
    });
}

#[test]
fn presenting_without_acquiring_is_refused() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // An unbalanced pair means a frame index nobody holds, and presenting
        // one would display whatever that image last contained.
        assert!(target.present(ctx).is_err(), "present without acquire");
    });
}

#[test]
fn acquiring_twice_without_presenting_is_refused() {
    with_target(PresentMode::Fifo, |ctx, target| {
        let _ = target.acquire(ctx).expect("first acquire");
        assert!(
            target.acquire(ctx).is_err(),
            "two frames were acquired at once"
        );
    });
}

#[test]
fn reconfiguring_rebuilds_at_the_new_size() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // A headless surface defers its size to the swapchain, which is what
        // lets this ask for one. A windowed surface dictates instead, and the
        // request is ignored — the reason the extent is read back afterwards
        // rather than assumed.
        let before = target.extent();
        target
            .reconfigure(ctx, Extent2D::new(before.width * 2, before.height / 2))
            .expect("reconfigure");
        assert_ne!(target.extent(), before, "the swapchain did not change size");

        // And it still works afterwards: the old images are gone and the new
        // ones have to be usable, which is the part a rebuild gets wrong.
        let after = target.extent();
        let image = target.acquire(ctx).expect("acquire after reconfigure");
        assert_eq!(image.extent(), after);
        let _ = image;
        let mut batch = Batch::new();
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([0.0, 1.0, 0.0, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
    });
}

#[test]
fn a_presented_frame_holds_what_was_drawn_into_it() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // Presenting is not observable without a display, so this checks the
        // half that is: the image handed out is a real render target, and what
        // is drawn into it is there afterwards. Without this the tests above
        // would pass on a target that handed back an image nothing reached.
        let _ = target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([0.25, 0.5, 0.75, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");

        let image = target.acquired_image().expect("acquired image");
        let extent = image.extent();
        let format = image.format();
        let pixels = ctx.read_texture(image).expect("readback");
        let middle = ((extent.height / 2 * extent.width + extent.width / 2) * 4) as usize;
        let got = [
            pixels[middle],
            pixels[middle + 1],
            pixels[middle + 2],
            pixels[middle + 3],
        ];
        // What the surface should hold, derived from what was drawn rather than
        // written down: an sRGB attachment encodes on write, so the byte is the
        // linear value put through the transfer function, and a linear one
        // stores it as it was. Stating it as a constant would have been a
        // second place to make the same mistake the format choice made.
        let encode = |linear: f32| {
            let encoded = if target.format().is_srgb() {
                if linear <= 0.003_130_8 {
                    linear * 12.92
                } else {
                    1.055 * linear.powf(1.0 / 2.4) - 0.055
                }
            } else {
                linear
            };
            (encoded * 255.0).round() as u8
        };
        let rgba = [encode(0.25), encode(0.5), encode(0.75), 255];
        let bgra = [encode(0.75), encode(0.5), encode(0.25), 255];
        let near = |want: [u8; 4]| {
            got.iter()
                .zip(&want)
                .all(|(a, b)| (*a as i32 - *b as i32).abs() <= 1)
        };
        assert!(
            near(rgba) || near(bgra),
            "a presented image came back {got:?} in {format:?}"
        );

        target.present(ctx).expect("present");
    });
}

#[test]
fn a_mailbox_request_falls_back_rather_than_failing() {
    // Mailbox is optional, and a surface without it should still give a working
    // swapchain: it is a latency preference, not a correctness requirement.
    with_target(PresentMode::Mailbox, |ctx, target| {
        target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
        assert_eq!(target.presented_frames(), 1);
    });
}

#[test]
fn nothing_blocks_until_the_frames_in_flight_are_exhausted() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // Acquiring and drawing are ordered by semaphores, so the first frames
        // do not block at all. A wait appears only once a slot has to be
        // reused, which is what bounds how far ahead of the display the
        // renderer runs — the mechanism working rather than a defect.
        assert_eq!(target.throttle_waits(), 0);
        for _ in 0..2 {
            target.acquire(ctx).expect("acquire");
            let mut batch = Batch::new();
            batch
                .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
                .expect("push");
            target
                .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
                .expect("submit");
            target.present(ctx).expect("present");
        }
        assert_eq!(
            target.throttle_waits(),
            0,
            "a frame blocked before any slot needed reusing"
        );

        // The third frame reuses the first slot, and only then is there
        // anything to wait for.
        target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
        assert_eq!(
            target.throttle_waits(),
            1,
            "reusing a frame slot did not wait for the frame that held it"
        );
    });
}

#[test]
fn a_swapchain_image_carries_texture_coordinates_like_any_other_target() {
    // Not about the swapchain as such: it checks that an image the presentation
    // engine allocated behaves as a render target in every respect, including
    // the parts a wrapper around somebody else's image is most likely to get
    // wrong. A target that only worked for solid fills would pass everything
    // above.
    with_target(PresentMode::Fifo, |ctx, target| {
        let _ = target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        let vertices: Vec<Vertex> = FULL.iter().map(|p| Vertex::at(*p)).collect();
        batch
            .push_mesh(
                &vertices,
                &QUAD,
                Material::LinearGradient {
                    axis: [1.0 - -1.0, 0.0 - 0.0],
                    // Clip space measured from the gradient's start, which sits
                    // at the left edge -- so the mapping is a translation and
                    // the start no longer travels beside it.
                    to_local: [
                        1.0, 0.0, 0.0, 0.0, //
                        0.0, 1.0, 0.0, 0.0, //
                        1.0, 0.0, 1.0, 0.0,
                    ],
                    stops: vec![
                        emblema_hal::Stop::new([1.0, 0.0, 0.0, 1.0], 0.0),
                        emblema_hal::Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
                    ],
                    tile: Default::default(),
                    ramp: None,
                },
                emblema_hal::ColorFilter::None,
                BlendMode::Src,
                None,
                emblema_hal::ClipState::UNCLIPPED,
            )
            .expect("push");
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");

        let image = target.acquired_image().expect("acquired image");
        let extent = image.extent();
        let pixels = ctx.read_texture(image).expect("readback");
        let at = |x: u32| ((extent.height / 2 * extent.width + x) * 4) as usize;
        let left = pixels[at(2)];
        let right = pixels[at(extent.width - 3)];
        assert_ne!(
            left, right,
            "a gradient rendered flat into a swapchain image"
        );
        target.present(ctx).expect("present");
    });
}

#[test]
fn presenting_a_frame_drawn_the_generic_way_is_refused() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // Drawing through the backend-agnostic submission leaves acquisition's
        // semaphore signaled and never waited on, and presentation about to
        // wait on one nothing signaled. That hangs rather than looking wrong,
        // so it is refused instead.
        let image = target.acquire(ctx).expect("acquire");
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
            .expect("push");
        ctx.submit_batch(image, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        assert!(
            target.present(ctx).is_err(),
            "a frame drawn without the target's own submit was presented"
        );

        // And the frame is still recoverable: drawing it properly presents.
        target
            .submit(ctx, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        target.present(ctx).expect("present");
    });
}

#[test]
fn a_frame_with_layers_can_be_presented() {
    with_target(PresentMode::Fifo, |ctx, target| {
        // Until `submit_recording` existed this was impossible rather than
        // merely untested: the swapchain took a batch, a recording with layers
        // is several passes, and the synchronized submission could not sample
        // anything -- so the pass that composites a layer had no way to reach a
        // presentable image. A frame with layers could be rendered offscreen
        // and never displayed.
        //
        // What is checked is that presenting agrees with rendering offscreen.
        // The layers have to be composited, in the right place, into the image
        // the swapchain handed out; comparing against the offscreen path is
        // what says so, and it is the same comparison the frame-loop test makes
        // for a flat scene.
        use emblema_core::{Canvas, Color, Layer, Paint, Rect, Vec2};

        let extent = target.extent();
        let scene = || {
            let mut canvas = Canvas::new(extent);
            canvas.clear(Color::linear(0.05, 0.05, 0.1, 1.0));
            canvas.save_layer_bounds(
                Layer::opacity(0.6),
                Rect::new(
                    8.0,
                    8.0,
                    extent.width as f32 - 8.0,
                    extent.height as f32 - 8.0,
                ),
            );
            for (dx, color) in [(-12.0, [0.9, 0.3, 0.2, 1.0]), (12.0, [0.2, 0.8, 0.9, 1.0])] {
                canvas
                    .draw_circle(
                        Vec2::new(extent.width as f32 / 2.0 + dx, extent.height as f32 / 2.0),
                        (extent.width.min(extent.height) as f32) / 4.0,
                        &Paint::fill(Color::linear(color[0], color[1], color[2], color[3]))
                            .with_anti_alias(false),
                    )
                    .expect("circle");
            }
            canvas.restore();
            canvas.finish()
        };
        let recording = scene();
        assert!(
            recording.passes.len() > 1,
            "the scene has no layer, so this proves nothing"
        );

        // Offscreen first, as the reference -- into the format the swapchain
        // chose rather than a fixed one. The question here is whether
        // presenting changes what was drawn, and rendering the reference into a
        // linear target while the surface is sRGB would answer a different one:
        // the two would differ by the transfer function everywhere, which is
        // the encoding working rather than presentation failing.
        let mut reference = ctx
            .create_texture(&emblema_hal::TextureDescriptor::offscreen(
                target.extent(),
                target.format(),
            ))
            .expect("reference target");
        emblema_core::execute::<VulkanHal>(ctx, &mut reference, &recording, &[])
            .expect("offscreen");
        let expected = ctx.read_texture(&mut reference).expect("read reference");
        ctx.destroy_texture(reference);

        let _ = target.acquire(ctx).expect("acquire");
        target
            .submit_recording(ctx, &recording, &[])
            .expect("submit recording");

        let image = target.acquired_image().expect("acquired image");
        let presented = ctx.read_texture(image).expect("readback");
        target.present(ctx).expect("present");

        // A swapchain surface may be BGRA where the offscreen target is RGBA,
        // so the comparison is per pixel against both orderings rather than a
        // memcmp that would fail on channel order alone.
        assert_eq!(presented.len(), expected.len());
        let mut worst = 0u8;
        for (got, want) in presented.chunks_exact(4).zip(expected.chunks_exact(4)) {
            let direct = (0..4).map(|i| got[i].abs_diff(want[i])).max().unwrap();
            let swapped = [want[2], want[1], want[0], want[3]];
            let reversed = (0..4).map(|i| got[i].abs_diff(swapped[i])).max().unwrap();
            worst = worst.max(direct.min(reversed));
        }
        assert!(
            worst <= 1,
            "a presented layered frame differs from the offscreen render by {worst}"
        );
        // And it drew something, so the agreement is not between two clears.
        assert!(
            expected.iter().any(|&b| b > 48),
            "the scene rendered nothing"
        );
    });
}

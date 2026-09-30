//! The facade has to be able to reach a window.
//!
//! `present-wsi` was in the default feature set and gated nothing for as long
//! as it existed: no `cfg` referenced it and the crate implementing it was not
//! a dependency. The facade's default build therefore advertised a
//! presentation path it had no route to, and the only caller that wanted a
//! window -- the playground -- reached past the facade to
//! `emblema-present-vk` directly, which is the symptom that was visible the
//! whole time.
//!
//! Everything here goes through `emblema` alone. That is the point: a test
//! that named `emblema_present_vk` would pass with the feature still dead.

#![cfg(feature = "present-wsi")]

use emblema::present::PresentTarget;
use emblema::{BackendPreference, Context, Extent2D};

const SIZE: Extent2D = Extent2D {
    width: 64,
    height: 64,
};

#[test]
fn a_swapchain_can_be_reached_through_the_facade_alone() {
    // The context the renderer draws with is the one the swapchain must be
    // built on -- a second context would be a second device, and images from
    // one are not presentable on the other. So this takes the facade's own
    // context apart rather than making its own, which is what the public
    // variants are for.
    let mut ctx = match Context::new(BackendPreference::Auto) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no context ({e})");
            return;
        }
    };
    let Context::Vulkan(ref mut vk) = ctx else {
        eprintln!("skipping: WSI here is Vulkan's, and this context is not");
        return;
    };

    // `VK_EXT_headless_surface` stands in for a window system, which a test
    // has no business requiring. Where it is missing there is nothing to
    // present to and the test says so rather than failing.
    let surface = match emblema::wsi::create_headless_surface(vk) {
        Ok(surface) => surface,
        Err(e) => {
            eprintln!("skipping: no headless surface ({e})");
            return;
        }
    };

    let target =
        emblema::wsi::SwapchainTarget::new(vk, surface, SIZE, emblema::wsi::PresentMode::Fifo);
    match target {
        Ok(mut target) => {
            // The image the renderer will actually draw into, asked of a real
            // driver's real format list rather than of a fabricated one.
            //
            // `choose_format` has a unit test, and it passed for as long as it
            // preferred an sRGB surface -- it was asserting the preference, not
            // whether the preference was right. This asks the question the unit
            // test cannot: of everything this device offered, is the one that
            // was taken a format the pipeline can write correctly?
            //
            // It would have failed for the whole of the time the swapchain
            // preferred sRGB, which was every commit until the pipeline stopped
            // carrying light and nothing in eight hundred tests noticed.
            // Acquired, not merely asked for: the first attempt here read the
            // image without acquiring a frame, got "no frame is acquired", and
            // reported a skip -- so the test passed having checked nothing,
            // which is the failure it exists to catch, in itself.
            let acquired = target.acquire(vk).map(|image| image.format());
            match acquired {
                Ok(format) => {
                    assert!(
                        format.is_drawable(),
                        "the swapchain acquired a {format:?}, which encodes on \
                         write -- so everything drawn through it would be \
                         encoded twice and come back over a third too bright"
                    );
                }
                Err(e) => panic!("a swapchain that built should acquire: {e}"),
            }

            // Explicitly, not by dropping it: tearing a swapchain down needs
            // the context that built it, which `Drop` does not have. Letting it
            // fall out of scope leaves the swapchain and its images behind, and
            // the validation layer reports it twice over -- once for a surface
            // destroyed while a swapchain still refers to it, and again for
            // every object outliving the device.
            target.destroy(vk);
            // SAFETY: the swapchain built on it has just been destroyed.
            unsafe { emblema::wsi::destroy_surface(vk, surface) };
        }
        Err(e) => {
            // SAFETY: as above; the target was never created.
            unsafe { emblema::wsi::destroy_surface(vk, surface) };
            panic!("a swapchain reached through the facade should build: {e}");
        }
    }
}

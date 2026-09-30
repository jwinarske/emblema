//! Validation is asserted, not eyeballed.
//!
//! A device created invalid can pass an entire suite while appearing to work,
//! so these tests turn the layer on and require the log to come back empty.
//! They skip where the layer is not installed, since a machine without the SDK
//! should still get a green run.

use emblema_hal::{Extent2D, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

fn validated(device: DevicePreference) -> Option<VulkanContext> {
    let ctx = VulkanContext::with_config(ContextConfig {
        device,
        validation: true,
        ..Default::default()
    })
    .ok()?;
    if !ctx.validation_active() {
        eprintln!("skipping: validation layer not installed");
        return None;
    }
    Some(ctx)
}

fn assert_clean(ctx: &VulkanContext, what: &str) {
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "{what} produced {} validation error(s):\n{}",
        errors.len(),
        errors
            .iter()
            .map(|e| format!("  [{}] {}", e.id, e.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn device_creation_is_validation_clean() {
    let Some(ctx) = validated(DevicePreference::Auto) else {
        return;
    };
    // This is the test that would have caught the missing image-format-list
    // dependency, which every pixel test passed straight through.
    assert_clean(&ctx, "device creation");
    assert!(ctx.validation_clean());
}

#[test]
fn allocate_clear_and_read_is_validation_clean() {
    let Some(mut ctx) = validated(DevicePreference::Auto) else {
        return;
    };
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(32, 32),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    ctx.clear_texture(&mut tex, [0.0, 1.0, 1.0, 1.0])
        .expect("clear");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    assert_eq!(&pixels[..4], &[0, 255, 255, 255]);
    // Layout transitions and copy regions are exactly what the layer checks,
    // so a clean log here is worth more than the pixel assertion above.
    assert_clean(&ctx, "allocate, clear and read");
}

#[test]
fn the_software_reference_is_validation_clean() {
    let Some(mut ctx) = validated(DevicePreference::Software) else {
        return;
    };
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(16, 16),
            PixelFormat::Bgra8Unorm,
        ))
        .expect("texture");
    ctx.clear_texture(&mut tex, [1.0, 0.0, 0.0, 1.0])
        .expect("clear");
    ctx.destroy_texture(tex);
    assert_clean(&ctx, "software reference");
}

#[test]
fn repeated_context_cycles_stay_clean() {
    if validated(DevicePreference::Auto).is_none() {
        return;
    }
    // Teardown ordering bugs tend to report on the second or third cycle.
    for i in 0..3 {
        let ctx = validated(DevicePreference::Auto).expect("layer was present");
        assert_clean(&ctx, &format!("cycle {i}"));
    }
}

#[test]
fn synchronization_validation_is_switched_on() {
    // Core validation checks that each call is well formed. It does not check
    // that one access is ordered against the next -- a missing barrier, a read
    // of an image the GPU has not finished writing -- which is a separate
    // feature the layer does not enable by default. It is the one that matters
    // most here, because this renderer synchronizes explicitly rather than
    // through a driver that hides it, and because a missing barrier renders
    // correctly on the device it was written on and wrongly elsewhere.
    //
    // So every clean validation log in this suite means one of two quite
    // different things depending on this, and nothing else would notice if it
    // stopped being requested.
    //
    // What this asserts is that it was requested and the extension carrying the
    // request was there, not that the layer honored it -- there is no way to
    // ask the layer what it enabled. The behavioral evidence is elsewhere and
    // is strong: turning this on immediately reported a real hazard in the
    // swapchain path, which is the change that added this test. A probe that
    // constructed a hazard on purpose was written first and then removed,
    // because every hazard reachable through this HAL is now prevented by the
    // fix -- which is the outcome wanted, and leaves nothing to provoke.
    let Ok(ctx) = VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
        ..Default::default()
    }) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    if !ctx.validation_active() {
        eprintln!("skipping: the validation layer is unavailable");
        return;
    }
    assert!(
        ctx.sync_validation_active(),
        "the validation layer is on but synchronization validation is not, so a \
         clean log in this suite says only that every call was well formed"
    );
}

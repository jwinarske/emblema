//! Device bring-up against whatever drivers this machine has.
//!
//! These tests need a Vulkan loader and at least one device. Where neither
//! exists they skip rather than fail: a developer without a GPU stack should
//! still get a green `cargo test`, and the lanes that must have Vulkan enforce
//! it by running on images that provide it.

use emblema_hal::Capability;
use emblema_hal_vulkan::DevicePreference;
use emblema_hal_vulkan::Validated;

/// Create a context, or `None` if this machine has no usable Vulkan.
fn context(preference: DevicePreference) -> Option<Validated> {
    match Validated::new(preference) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

#[test]
fn a_context_reports_coherent_capabilities() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };
    let caps = ctx.capabilities();
    eprintln!("device: {} ({})", caps.device_name, caps.driver_name);

    assert!(
        !caps.device_name.is_empty(),
        "a device that cannot name itself makes every report fingerprint useless"
    );
    assert!(
        caps.max_texture_size >= 4096,
        "got {}",
        caps.max_texture_size
    );

    // Single-sampled rendering is universally supported; a device reporting
    // otherwise has a mask decoded wrong.
    assert!(caps.sample_counts.supports(1));
    assert!(caps.sample_counts.max() >= 1);
    assert!(caps.sample_counts.max().is_power_of_two());
}

#[test]
fn capability_flags_are_internally_consistent() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };
    let caps = ctx.capabilities();

    // Self-allocated scanout requires both export and explicit modifiers.
    // Reporting the composite true while a component is false would send the
    // presentation layer down a path the driver cannot support.
    if caps.dma_buf.can_allocate_scanout() {
        assert!(caps.dma_buf.export);
        assert!(caps.dma_buf.modifiers);
    }

    // The extension backing each flag must actually have been enabled, or the
    // flag is a promise the device cannot keep.
    // Export is a semaphore operation and import is a fence one, so the two
    // flags come from different extensions rather than one covering both.
    assert_eq!(
        caps.sync.export_sync_file,
        ctx.has_extension("VK_KHR_external_semaphore_fd")
    );
    assert_eq!(
        caps.sync.import_sync_file,
        ctx.has_extension("VK_KHR_external_fence_fd")
    );
    assert_eq!(
        caps.dma_buf.modifiers,
        ctx.has_extension("VK_EXT_image_drm_format_modifier")
    );
}

/// Withholding advanced blending produces a device that has not got the
/// extension, rather than one that has it and says otherwise.
///
/// This is the assertion the mechanism exists to earn. `Capabilities` already has
/// unit tests for the refusal itself, against a hand-built value, so a test here
/// that only checked the capability reads false would prove nothing those do not.
/// What cannot be reached from a synthesized `Capabilities` is whether the *device*
/// was built without the thing -- and that is what makes the refusal the real one
/// rather than a flag disagreeing with its driver.
///
/// It is also the guard against a later simplification. Clearing the field after
/// detection would satisfy every other assertion in this file and would leave
/// `enabled_extensions` saying the device has an extension the capability denies.
/// Some paths read that set instead of the capability, so the two must agree.
#[test]
fn withholding_advanced_blend_leaves_the_extension_out_of_the_device() {
    const EXTENSION: &str = "VK_EXT_blend_operation_advanced";

    // Whichever device here has the thing, rather than whichever one is
    // preferred. Asking only `Auto` is how the test this mechanism replaces came
    // to run nowhere: on this machine the preferred device reports no advanced
    // blending and the software one does, so a search finds a device to withhold
    // from where a preference finds none. The same mistake, caught in the test
    // written to fix it.
    let capable = [DevicePreference::Auto, DevicePreference::Software]
        .into_iter()
        .find(|preference| {
            Validated::new(*preference)
                .map(|ctx| {
                    let has = ctx.capabilities().advanced_blend;
                    // The positive control, and not decoration: without it a
                    // later reading cannot tell a working restriction from a
                    // device that never had the extension.
                    assert!(
                        !has || ctx.has_extension(EXTENSION),
                        "a device reports advanced blending without the extension"
                    );
                    has
                })
                .unwrap_or(false)
        });
    let Some(preference) = capable else {
        eprintln!("skipping: no device here has advanced blending to withhold");
        return;
    };

    let restricted = Validated::without(preference, Capability::AdvancedBlend)
        .expect("a device that was just created can be created again");
    assert!(
        !restricted.capabilities().advanced_blend,
        "advanced blending survived being withheld"
    );
    assert!(
        !restricted.has_extension(EXTENSION),
        "the capability reads false while the extension is still enabled, so the \
         device and what it says about itself disagree"
    );
}

/// A restricted device still reports flags that match its own extensions.
///
/// The same three pairs the test above this file's middle asserts for an ordinary
/// device, re-run against a restricted one. That makes the argument for taking the
/// extension out -- rather than overruling the answer afterwards -- an executable
/// claim rather than a paragraph in a commit message.
#[test]
fn a_restricted_device_reports_flags_that_match_its_extensions() {
    let Ok(ctx) = Validated::without(DevicePreference::Auto, Capability::AdvancedBlend) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    // Withholding something the device never had is a no-op, which is exactly
    // what makes this safe to run on any device: the pairs below hold either way.
    let caps = ctx.capabilities();

    assert_eq!(
        caps.sync.export_sync_file,
        ctx.has_extension("VK_KHR_external_semaphore_fd")
    );
    assert_eq!(
        caps.sync.import_sync_file,
        ctx.has_extension("VK_KHR_external_fence_fd")
    );
    assert_eq!(
        caps.dma_buf.modifiers,
        ctx.has_extension("VK_EXT_image_drm_format_modifier")
    );

    // And the withholding reached nothing it was not asked to reach. A
    // subtraction that took a dependency with it would show up here.
    assert!(
        ctx.has_extension("VK_KHR_swapchain") || caps.device_name.contains("llvmpipe"),
        "withholding advanced blending cost the swapchain extension"
    );
}

#[test]
fn scanout_and_explicit_sync_are_reported_independently() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };
    let caps = ctx.capabilities();
    eprintln!(
        "scanout={} explicit_sync={} import={} export={} modifiers={}",
        caps.supports_scanout(),
        caps.sync.supports_explicit_scanout(),
        caps.dma_buf.import,
        caps.dma_buf.export,
        caps.dma_buf.modifiers,
    );

    // A device can be scanout-capable without fence export; that combination
    // selects the CPU-wait fallback rather than disabling the DRM path, so
    // neither flag may be derived from the other. What says so is the pair of
    // extension checks in the test above, since the two flags come from
    // different extensions.
    //
    // This used to close with an assertion that could not fail.
    // `supports_scanout` is `can_allocate_scanout() || import`, so guarding it
    // with `if !supports_scanout()` and then asserting `!can_allocate_scanout()`
    // asserts the second half of a disjunction already known to be false --
    // entailed by the definition, on any device, whatever the driver does. It
    // read as coverage of the relationship and was none, and the relationship it
    // appeared to cover is held by `import_only_devices_still_support_scanout` in
    // `emblema_hal::capabilities`, which needs no device: import alone makes the
    // composite true while `can_allocate_scanout` stays false, which is the one
    // direction that is not entailed.
    //
    // So what is left here is a report rather than a check, and the print above is
    // the point of it -- what this machine's device actually offers, named in the
    // output where a later reader of a board run can see it. Said plainly because
    // a test whose body is one `eprintln!` looks like something half-written
    // unless it says it is not.
}

#[test]
fn the_software_rasterizer_is_selectable_as_the_reference_device() {
    match Validated::new(DevicePreference::Software) {
        Ok(ctx) => {
            let caps = ctx.capabilities();
            eprintln!("software device: {}", caps.device_name);
            // The golden corpus is compared against this device, so it has to
            // be usable for real rendering, not merely enumerable.
            assert!(caps.max_texture_size >= 4096);
            assert!(caps.sample_counts.supports(1));
        }
        Err(e) => eprintln!("skipping: no software rasterizer ({e})"),
    }
}

#[test]
fn an_out_of_range_device_index_is_an_error_rather_than_a_panic() {
    // Device indices come from configuration and command lines, so an invalid
    // one must surface as an error the caller can report.
    let result = Validated::new(DevicePreference::Index(9999));
    assert!(result.is_err());
}

#[test]
fn contexts_can_be_created_and_dropped_repeatedly() {
    if context(DevicePreference::Auto).is_none() {
        return;
    }
    // Teardown order is instance-after-device; getting it wrong tends to show
    // up as a crash on the second cycle rather than the first.
    for _ in 0..3 {
        let ctx = Validated::new(DevicePreference::Auto).expect("device was available");
        assert!(!ctx.capabilities().device_name.is_empty());
    }
}

#[test]
fn enabled_extensions_carry_their_dependencies() {
    let Some(ctx) = context(DevicePreference::Auto) else {
        return;
    };

    // The specification requires every dependency of an enabled extension to
    // be enabled too, and a device created without them is invalid even though
    // it appears to work. This only shows up under a validation layer, so the
    // invariant is asserted directly here instead.
    if ctx.has_extension("VK_EXT_image_drm_format_modifier") {
        assert!(
            ctx.has_extension("VK_KHR_image_format_list"),
            "the modifier extension requires VK_KHR_image_format_list on a 1.1 baseline"
        );
    }
    if ctx.has_extension("VK_EXT_external_memory_dma_buf") {
        assert!(
            ctx.has_extension("VK_KHR_external_memory_fd"),
            "dma-buf external memory requires the fd extension"
        );
    }
}

/// A device asked for by UUID is the device that comes back.
///
/// The round trip is the test: learn a UUID from a context, ask for that UUID, and the second context
/// must report the same one. A selector that ignored the preference and fell through to `Auto` would
/// pass on a single-device machine and fail on any other, so the device count is reported and the
/// multi-device case is where this has teeth -- `vkEnumeratePhysicalDevices` on this desktop answers
/// two.
#[test]
fn a_device_asked_for_by_uuid_is_the_one_returned() {
    let Some(first) = context(DevicePreference::Auto) else {
        return;
    };
    let wanted = first.device_uuid();
    assert_ne!(
        wanted, [0u8; 16],
        "a real device reported no uuid, so the properties2 chain did not reach it"
    );
    eprintln!("auto picked {wanted:02x?}");

    let Some(again) = context(DevicePreference::Uuid(wanted)) else {
        panic!("the uuid just reported was refused");
    };
    assert_eq!(
        again.device_uuid(),
        wanted,
        "asking for a uuid returned a different device"
    );
    assert_eq!(
        again.capabilities().device_name,
        first.capabilities().device_name,
        "the same uuid named two different devices"
    );
}

/// Every device enumerated has a distinct UUID, and each can be asked for by name.
///
/// The part that makes the test above mean something on a machine with more than one device: if two
/// devices shared a UUID, or if asking by UUID always answered the same one, this notices. Walks the
/// enumeration by index -- which is exactly the unstable thing `Uuid` exists to replace, and is the
/// right tool for *discovering* what is present.
#[test]
fn each_device_can_be_named_by_its_own_uuid() {
    let mut seen: Vec<[u8; 16]> = Vec::new();
    for index in 0..8 {
        let Ok(ctx) = emblema_hal_vulkan::Validated::new(DevicePreference::Index(index)) else {
            break;
        };
        let uuid = ctx.device_uuid();
        let name = ctx.capabilities().device_name.clone();
        drop(ctx);

        // Asked for by its own uuid, it must come back -- and be the same device.
        let Ok(again) = emblema_hal_vulkan::Validated::new(DevicePreference::Uuid(uuid)) else {
            panic!("device {index} ({name}) would not answer to its own uuid {uuid:02x?}");
        };
        assert_eq!(again.device_uuid(), uuid, "device {index} ({name})");
        seen.push(uuid);
    }
    if seen.is_empty() {
        eprintln!("skipping: no usable Vulkan device");
        return;
    }
    eprintln!("{} device(s): {seen:02x?}", seen.len());

    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seen.len(),
        "two devices reported the same uuid, so a uuid does not name one"
    );
}

/// A UUID no device has is an error, not a fallback.
///
/// The whole point of asking for a particular GPU is that the wrong one does not work. Handing back
/// another would fail later at a dma-buf import, somewhere with nothing to say about why -- so the
/// refusal belongs here, and it names what is present so the caller can see what it should have asked
/// for.
#[test]
fn an_unknown_uuid_is_refused_rather_than_substituted() {
    if context(DevicePreference::Auto).is_none() {
        return;
    }
    // Not any real device's: a UUID is 16 bytes and this is all ones.
    let absent = [0xffu8; 16];
    match emblema_hal_vulkan::Validated::new(DevicePreference::Uuid(absent)) {
        Ok(ctx) => panic!(
            "an absent uuid was substituted with {}",
            ctx.capabilities().device_name
        ),
        Err(why) => {
            let said = format!("{why}");
            assert!(
                said.contains("uuid"),
                "the refusal does not mention the uuid: {said}"
            );
            // The message lists what is present, which is what a caller needs to correct the call.
            assert!(
                said.contains("present"),
                "the refusal does not say what was available: {said}"
            );
            eprintln!("refused as expected: {said}");
        }
    }
}

/// A render node asked for by device number is the device that comes back, where one is reported.
///
/// Skips where the driver has no `VK_EXT_physical_device_drm` -- lavapipe does not -- because then
/// there is no node to ask for and nothing to assert. That is the honest shape: the extension is how
/// this is read, so a driver without it can never match, and the refusal below covers the case where
/// a caller asks anyway.
#[test]
fn a_device_asked_for_by_render_node_is_the_one_returned() {
    let Some(first) = context(DevicePreference::Auto) else {
        return;
    };
    let Some((major, minor)) = first.render_node() else {
        eprintln!("skipping: this device reports no DRM render node");
        return;
    };
    eprintln!("auto picked render node {major}:{minor}");

    let Some(again) = context(DevicePreference::RenderNode { major, minor }) else {
        panic!("the render node just reported was refused");
    };
    assert_eq!(again.render_node(), Some((major, minor)));
    assert_eq!(
        again.device_uuid(),
        first.device_uuid(),
        "the same render node named two different devices"
    );
}

/// A render node no device has is an error, not a fallback.
#[test]
fn an_unknown_render_node_is_refused() {
    if context(DevicePreference::Auto).is_none() {
        return;
    }
    // No DRM device is numbered this: minors run from 0 and majors are allocated low.
    match emblema_hal_vulkan::Validated::new(DevicePreference::RenderNode {
        major: 9999,
        minor: 9999,
    }) {
        Ok(ctx) => panic!(
            "an absent render node was substituted with {}",
            ctx.capabilities().device_name
        ),
        Err(why) => {
            let said = format!("{why}");
            assert!(
                said.contains("render node"),
                "the refusal does not mention the render node: {said}"
            );
            eprintln!("refused as expected: {said}");
        }
    }
}

/// Render node `0:0` matches nothing, which is the trap in reading a chained struct.
///
/// `VkPhysicalDeviceDrmPropertiesEXT` is only filled by a driver that supports
/// `VK_EXT_physical_device_drm`. Chaining it onto a driver that does not leaves it at its default, and
/// a default of zero reads as render node `0:0` -- so a caller asking for `0:0` would be handed
/// whichever device ignored the struct. The extension is checked by name first, and this is what says
/// so.
///
/// `0:0` is not a real DRM node either: minors are allocated from 0 within a major, and major 0 is not
/// DRM's.
#[test]
fn render_node_zero_matches_nothing() {
    if context(DevicePreference::Auto).is_none() {
        return;
    }
    match emblema_hal_vulkan::Validated::new(DevicePreference::RenderNode { major: 0, minor: 0 }) {
        Ok(ctx) => panic!(
            "render node 0:0 matched {}, so a driver without the extension was read as zero",
            ctx.capabilities().device_name
        ),
        Err(why) => eprintln!("refused as expected: {why}"),
    }
}

//! GLES context bring-up against whatever EGL this machine provides.
//!
//! Skips where EGL or a surfaceless platform is unavailable, so a machine
//! without a graphics stack still gets a green run, while lanes that must have
//! GLES enforce it through the image they run on.

use emblema_hal::Capability;
use emblema_hal_gles::DisplayTarget;
use emblema_hal_gles::Validated as GlesValidated;

fn context() -> Option<GlesValidated> {
    match GlesValidated::new(DisplayTarget::Surfaceless) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable GLES context ({e})");
            None
        }
    }
}

#[test]
fn a_context_reports_coherent_capabilities() {
    let Some(ctx) = context() else { return };
    let caps = ctx.capabilities();
    eprintln!("device: {} ({})", caps.device_name, caps.driver_name);

    assert!(
        !caps.device_name.is_empty(),
        "a device that cannot name itself makes every report fingerprint useless"
    );
    // GLES 3.0 guarantees at least 2048; anything less means the context came
    // up below the floor this project targets.
    assert!(
        caps.max_texture_size >= 2048,
        "got {}",
        caps.max_texture_size
    );
    assert!(caps.sample_counts.supports(1));
    assert!(caps.sample_counts.max().is_power_of_two());
}

#[test]
fn the_context_is_at_least_gles_three() {
    let Some(ctx) = context() else { return };
    let version = &ctx.capabilities().driver_name;
    // GLES 2.0 is permanently out of scope, so a 2.x context is a failure to
    // start rather than a degraded mode.
    assert!(
        version.contains("OpenGL ES 3"),
        "expected an ES 3 context, got {version}"
    );
}

#[test]
fn sample_counts_are_powers_of_two_and_may_have_gaps() {
    let Some(ctx) = context() else { return };
    let counts = ctx.capabilities().sample_counts;
    // This used to require a contiguous run, on the belief that GLES reports
    // only a maximum and that the mask below it is safe to synthesize. It is
    // not: `GetInternalformativ` reports the counts a format actually supports,
    // and llvmpipe answers 8 and 4 for `RGBA8` with no 2. So a gap is a real
    // device answer rather than a broken query, and requiring contiguity meant
    // requiring the wrong thing on a conformant driver.
    //
    // What remains true is the shape. Anything else -- which counts, how many
    // -- is the device's to say, and pinning it here would only re-encode the
    // assumption this replaced.
    let max = counts.max();
    assert!(
        counts.supports(1),
        "single-sampled is always available and was not reported"
    );
    assert!(max.is_power_of_two(), "maximum {max} is not a power of two");
    for n in [1u32, 2, 4, 8, 16, 32] {
        if counts.supports(n) {
            assert!(n <= max, "{n}x is supported but above the maximum {max}");
        }
    }
    assert!(!counts.supports(max * 2), "{}x above the maximum", max * 2);
}

#[test]
fn capability_flags_match_the_extensions_actually_present() {
    let Some(ctx) = context() else { return };
    let caps = ctx.capabilities();

    // Import and export are separate extensions here, unlike Vulkan where one
    // handle type covers both. A flag that did not match its extension would be
    // a promise the driver cannot keep.
    assert_eq!(
        caps.dma_buf.import,
        ctx.has_egl_extension("EGL_EXT_image_dma_buf_import")
    );
    assert_eq!(
        caps.dma_buf.export,
        ctx.has_egl_extension("EGL_MESA_image_dma_buf_export")
    );
    // Export follows its extension, now that there is a fence to export. It
    // did not for a while: `Hal::Fence` was `std::convert::Infallible`, so the
    // flag promised a caller something it could obtain nothing to perform, and
    // this assertion was inverted to pin it false until the fence existed.
    assert_eq!(
        caps.sync.export_sync_file,
        ctx.has_egl_extension("EGL_ANDROID_native_fence_sync")
    );
    // Import does not follow anything, because it is not implemented: a sync
    // built *from* a descriptor needs a constructor taking one, and there is
    // none. This is the same shape as the assertion above used to be, and it
    // should become the extension check when that constructor exists.
    assert!(
        !caps.sync.import_sync_file,
        "importing a sync_file is advertised while nothing can consume one"
    );

    eprintln!(
        "scanout={} explicit_sync={} import={} export={} modifiers={}",
        caps.supports_scanout(),
        caps.sync.supports_explicit_scanout(),
        caps.dma_buf.import,
        caps.dma_buf.export,
        caps.dma_buf.modifiers,
    );
}

#[test]
fn gl_calls_work_on_the_current_context() {
    use glow::HasContext;
    let Some(ctx) = context() else { return };
    let gl = ctx.raw_gl();

    // Proves the loader resolved real entry points rather than nulls, which
    // would otherwise surface as a crash at the first draw rather than here.
    // SAFETY: the context is current on this thread for its whole lifetime.
    unsafe {
        assert_eq!(gl.get_error(), glow::NO_ERROR);
        let viewport = gl.get_parameter_i32(glow::MAX_VIEWPORT_DIMS);
        assert!(
            viewport > 0,
            "max viewport dimension came back as {viewport}"
        );
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
}

#[test]
fn contexts_can_be_created_and_dropped_repeatedly() {
    if context().is_none() {
        return;
    }
    // Teardown unbinds before destroying; getting that wrong tends to leak the
    // display and show up on a later cycle rather than the first.
    for i in 0..3 {
        let ctx = GlesValidated::new(DisplayTarget::Surfaceless)
            .unwrap_or_else(|e| panic!("cycle {i}: {e}"));
        assert!(!ctx.capabilities().device_name.is_empty());
    }
}

/// A deferred submission hands back a fence that signals, and exports.
///
/// The three things `HalFence` promises, against real work rather than against
/// an empty stream: an unsignaled fence eventually signals, a wait reports
/// which of the two happened, and where the driver has the native extension
/// the fence dups out to a descriptor something outside this process could be
/// given.
///
/// The point of drawing first is that a fence on an empty command stream is
/// signaled immediately on most drivers and would pass this while proving
/// nothing about ordering.
#[test]
fn a_deferred_submission_hands_back_a_fence_that_signals() {
    use emblema_hal::{HalContext, HalFence, PassDescriptor, PixelFormat, TextureDescriptor};
    use emblema_hal_gles::{DisplayTarget, GlesContext};

    let Ok(mut ctx) = GlesContext::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    let caps = ctx.capabilities().clone();
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(
            emblema_hal::Extent2D::new(256, 256),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("target");

    let batch = emblema_hal::Batch::default();
    let fence = match ctx.submit_batch_deferred_textured(
        &mut target,
        &batch,
        PassDescriptor::clear([0.2, 0.4, 1.0, 1.0]),
        &[],
    ) {
        Ok(f) => f,
        Err(e) => panic!("deferred submission should be available now: {e}"),
    };

    // Signals within a second. A clear of a small target is microseconds of
    // work, so a second is a hang rather than a slow machine.
    assert!(
        fence.wait(std::time::Duration::from_secs(1)).expect("wait"),
        "the fence did not signal within a second"
    );
    assert!(
        fence.is_signaled().expect("is_signaled"),
        "a fence that satisfied a wait must report itself signaled"
    );

    // And the export, where the driver offers it. Checked against the
    // capability rather than assumed, which is the contract `HalFence`
    // documents: a caller branches on `export_sync_file` first.
    #[cfg(unix)]
    {
        let exported = fence.export_sync_file();
        if caps.sync.export_sync_file {
            let fd = exported.expect("the capability says this exports");
            use std::os::fd::AsRawFd;
            assert!(fd.as_raw_fd() >= 0, "a descriptor should be valid");
        } else {
            assert!(
                exported.is_err(),
                "a fence must refuse to export where the capability says it cannot"
            );
        }
    }

    ctx.destroy_texture(target);
}

/// Withholding advanced blending leaves the extensions out of the context.
///
/// The Vulkan backend's test of this name says what the assertion is for and why a
/// capability reading false is not enough on its own: the extension set is kept on
/// the context and other code reads it, so the two have to agree.
///
/// What is not observable from here, said rather than left as a gap: the
/// blend-qualified program variants are built lazily from the capability, and
/// nothing exposes whether one was linked. So this asserts the reachable
/// consequences -- the capability is false, both extensions are gone, and the debug
/// log is clean -- and the program half is covered where a draw is refused.
#[test]
fn withholding_advanced_blend_leaves_the_extensions_out_of_the_context() {
    let Some(ctx) = context() else {
        return;
    };
    if !ctx.capabilities().advanced_blend {
        eprintln!("skipping: this driver has no advanced blending to withhold");
        return;
    }
    assert!(
        ctx.has_gl_extension("GL_KHR_blend_equation_advanced"),
        "the driver reports advanced blending without the extension behind it"
    );
    drop(ctx);

    let Ok(restricted) =
        GlesValidated::without(DisplayTarget::Surfaceless, Capability::AdvancedBlend)
    else {
        eprintln!("skipping: no GLES context");
        return;
    };
    assert!(
        !restricted.capabilities().advanced_blend,
        "advanced blending survived being withheld"
    );
    // Both names, though only the first bites everywhere. The driver this was
    // written against has the advanced equations and not the coherent variant, so
    // the second assertion is vacuously true here and dropping that name from the
    // map cannot be caught on this machine -- checked, rather than assumed. The
    // first one is load-bearing: withholding only the coherent name fails this.
    for name in [
        "GL_KHR_blend_equation_advanced",
        "GL_KHR_blend_equation_advanced_coherent",
    ] {
        assert!(
            !restricted.has_gl_extension(name),
            "{name} is still present while the capability it backs reads false"
        );
    }
}

/// A restricted context still reports flags that match its own extensions.
///
/// The pairs from `capability_flags_match_the_extensions_actually_present`, re-run
/// against a restricted context. Withholding something reaches the two capabilities
/// named and nothing else, and these are the fields that would show it if it did.
#[test]
fn a_restricted_context_reports_flags_that_match_its_extensions() {
    let Ok(ctx) = GlesValidated::without(DisplayTarget::Surfaceless, Capability::AdvancedBlend)
    else {
        eprintln!("skipping: no GLES context");
        return;
    };
    let caps = ctx.capabilities();

    assert_eq!(
        caps.dma_buf.export,
        ctx.has_egl_extension("EGL_MESA_image_dma_buf_export")
    );
    assert_eq!(
        caps.sync.export_sync_file,
        ctx.has_egl_extension("EGL_ANDROID_native_fence_sync")
    );
    assert!(!caps.sync.import_sync_file);
}

/// The GBM display target reaches the same driver as the surfaceless one.
///
/// The point of the variant is vendor stacks -- a Vivante or an Adreno ships
/// `libgbm` and advertises `EGL_KHR_platform_gbm` while having no surfaceless
/// *platform*, so this is the difference between their GLES being testable and
/// being reported absent. Neither of those is here, which is exactly why this
/// test exists: Mesa has both platforms, so it is where the two can be asked
/// the same question and required to give the same answer.
///
/// What is asserted is the part a board cannot check for us. That the context
/// comes up at all, that it is the same device, and that it reports the same
/// capabilities -- a display target decides how a handle is obtained and
/// nothing else, so anything differing here is this code and not the driver.
#[test]
fn a_gbm_display_reaches_the_same_device_as_a_surfaceless_one() {
    let Some(surfaceless) = context() else { return };
    let gbm = match GlesValidated::new(DisplayTarget::Gbm) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no GBM display here ({e})");
            return;
        }
    };

    let (a, b) = (surfaceless.capabilities(), gbm.capabilities());
    eprintln!(
        "surfaceless: {} ({}); gbm: {} ({})",
        a.device_name, a.driver_name, b.device_name, b.driver_name
    );
    assert_eq!(
        a.device_name, b.device_name,
        "the two display targets found different devices"
    );
    assert_eq!(
        a.advanced_blend, b.advanced_blend,
        "advanced blending differs by display target, which it cannot"
    );
    assert_eq!(
        a.max_texture_size, b.max_texture_size,
        "the texture limit differs by display target, which it cannot"
    );
    assert_eq!(
        a.sample_counts, b.sample_counts,
        "the sample counts differ by display target, which they cannot"
    );
}

/// A GBM context renders, and renders the same thing.
///
/// Capabilities agreeing is cheap; the display target could still have left
/// the context unable to draw. The same scene through both must come back
/// byte-identical, since the only difference between them is where the display
/// handle came from.
#[test]
fn a_gbm_context_renders_what_a_surfaceless_one_does() {
    use emblema_hal::{Batch, BlendMode, Material, PassDescriptor, PixelFormat};
    use emblema_hal::{Extent2D, TextureDescriptor};

    let Some(mut surfaceless) = context() else {
        return;
    };
    let Ok(mut gbm) = GlesValidated::new(DisplayTarget::Gbm) else {
        eprintln!("skipping: no GBM display here");
        return;
    };

    const SIZE: Extent2D = Extent2D {
        width: 32,
        height: 32,
    };
    let mut batch = Batch::new();
    batch
        .push(
            &[[-0.8, -0.8], [0.6, -0.8], [0.6, 0.4], [-0.8, 0.4]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([0.2, 0.7, 0.3, 1.0]),
            BlendMode::SrcOver,
        )
        .expect("push");

    let render = |ctx: &mut GlesValidated| {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
            .expect("offscreen target");
        ctx.submit_batch(&mut target, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("submit");
        let pixels = ctx.read_texture(&mut target).expect("read back");
        ctx.destroy_texture(target);
        pixels
    };

    assert_eq!(
        render(&mut surfaceless),
        render(&mut gbm),
        "the same scene differs between display targets"
    );
}

//! The scanout path against a real KMS device.
//!
//! Everything the recording stand-in cannot answer: whether a display
//! controller accepts the buffers this renderer exports, whether an atomic
//! commit with those properties is valid, and whether the flip events come back.
//!
//! It needs a card this process can become master of. A compositor holds
//! master on any card driving a display, so in practice that means the virtual
//! KMS driver — which provides atomic modesetting and vblank with no display
//! behind it. `cargo xtask drm` says whether a machine has one; where it does
//! not, these skip and say so rather than passing quietly.

use emblema_hal::{Batch, BlendMode, Material, PassDescriptor, PixelFormat};
use emblema_hal_vulkan::{DevicePreference, Validated};
use emblema_present::negotiate::negotiate;
use emblema_present_drm::output::{CommitRequest, DmaBufPlanes, OutputEvent, ScanoutOutput};
use emblema_present_drm::KmsOutput;

const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

mod common;

/// A card this process can drive, held for as long as the guard lives.
fn output() -> Option<(KmsOutput, common::CardGuard)> {
    let guard = common::take_the_card();
    let mut refused = Vec::new();

    // A named card, for a machine with more than one. Both Raspberry Pi 4 and
    // Pi 5 carry a render device and one or two display controllers as separate
    // card nodes, and the loop below takes whichever the directory lists first
    // -- which on a Pi 5 is the DSI controller and not the HDMI one. That is
    // fine for "can this drive a display" and useless for "can *this* display
    // controller take what the renderer exported", which is the question a
    // board is worth running on to answer.
    if let Ok(path) = std::env::var("EMBLEMA_DRM_CARD") {
        return match KmsOutput::open(&path) {
            Ok(output) => Some((output, guard)),
            Err(e) => {
                eprintln!("skipping: {path} was named but cannot be driven ({e})");
                None
            }
        };
    }

    for entry in std::fs::read_dir("/dev/dri").ok()?.flatten() {
        let name = entry.file_name().into_string().ok()?;
        if !name.starts_with("card") {
            continue;
        }
        let path = entry.path().to_string_lossy().into_owned();
        match KmsOutput::open(&path) {
            Ok(output) => return Some((output, guard)),
            Err(e) => refused.push(format!("{path}: {e}")),
        }
    }
    eprintln!(
        "skipping: no card this process can drive ({})",
        refused.join("; ")
    );
    None
}

fn context() -> Option<Validated> {
    match Validated::new(DevicePreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            None
        }
    }
}

#[test]
fn a_rendered_frame_reaches_a_real_display_controller() {
    let (Some((mut output, _card)), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let mode = output.mode();
    eprintln!(
        "driving {} at {}x{}",
        output.path(),
        mode.extent.width,
        mode.extent.height
    );

    // Negotiation against what this plane advertises, rather than a layout
    // chosen in advance. Linear is a valid answer here: vkms accepts nothing
    // else, and that being reported rather than assumed is the point.
    let render = ctx.capabilities().render_formats.clone();
    if render.is_empty() {
        // Not a negotiation that failed: a render device that advertises no
        // format and modifier at all, which PanVK on an RK3566 does. There is
        // nothing to intersect with the plane's list.
        eprintln!("skipping: this render device advertises no formats to negotiate with");
        return;
    }
    let agreed = negotiate(
        &render,
        output.supported_formats(),
        &[
            emblema_hal::Fourcc::ARGB8888,
            emblema_hal::Fourcc::XRGB8888,
            emblema_hal::Fourcc::ABGR8888,
        ],
    )
    .expect("the render device and this plane share no layout");
    eprintln!("agreed on {:?} with {:?}", agreed.fourcc, agreed.modifier);

    // Allocate an image the display can scan out, draw into it, and export it.
    let format = PixelFormat::Bgra8Unorm;
    let mut target = ctx
        .create_exportable_texture(mode.extent, format, &[agreed.modifier])
        .expect("an exportable image at the negotiated layout");

    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([0.2, 0.6, 0.9, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    ctx.submit_batch(&mut target, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("render into the scanout buffer");

    let exported = ctx.export_texture(&target).expect("dma-buf export");
    let planes = DmaBufPlanes {
        planes: exported.planes,
        fourcc: agreed.fourcc,
        modifier: agreed.modifier,
        extent: mode.extent,
    };

    // The whole point: a buffer this renderer allocated, drew into and
    // exported, accepted by a display controller as a framebuffer.
    let fb = output
        .import_dmabuf(planes)
        .expect("the display controller refused the exported buffer");

    // First commit sets the mode; a flip alone would have nothing to flip onto.
    output
        .commit(CommitRequest {
            fb,
            in_fence_fd: None,
            allow_modeset: true,
        })
        .expect("atomic commit");

    // vkms drives vblank off a timer, so the flip lands within a frame. Waiting
    // rather than polling once is what distinguishes an event that arrives from
    // one that never does.
    let events = output
        .wait_for_event(500_000_000)
        .expect("waiting for the flip");
    assert!(
        events.contains(&OutputEvent::FlipComplete { fb }),
        "no flip-complete arrived for the committed frame: {events:?}"
    );

    output.release_framebuffer(fb);
    ctx.destroy_texture(target);
}

#[test]
fn several_frames_flip_in_turn() {
    let (Some((mut output, _card)), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let mode = output.mode();
    if ctx.capabilities().render_formats.is_empty() {
        eprintln!("skipping: this render device advertises no formats to negotiate with");
        return;
    }
    let agreed = negotiate(
        &ctx.capabilities().render_formats.clone(),
        output.supported_formats(),
        &[emblema_hal::Fourcc::ARGB8888, emblema_hal::Fourcc::XRGB8888],
    )
    .expect("negotiation");

    // Two buffers, alternating, which is the arrangement a frame loop uses: one
    // on screen while the next is drawn. A single buffer would be drawn into
    // while the controller was reading it.
    let mut slots = Vec::new();
    for _ in 0..2 {
        let mut texture = ctx
            .create_exportable_texture(mode.extent, PixelFormat::Bgra8Unorm, &[agreed.modifier])
            .expect("exportable image");
        let mut batch = Batch::new();
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([0.9, 0.3, 0.1, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        ctx.submit_batch(&mut texture, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("render");
        let exported = ctx.export_texture(&texture).expect("export");
        let fb = output
            .import_dmabuf(DmaBufPlanes {
                planes: exported.planes,
                fourcc: agreed.fourcc,
                modifier: agreed.modifier,
                extent: mode.extent,
            })
            .expect("import");
        slots.push((texture, fb));
    }

    for frame in 0..6 {
        let (_, fb) = slots[frame % slots.len()];
        output
            .commit(CommitRequest {
                fb,
                in_fence_fd: None,
                // Only the first commit may modeset; asking every frame turns a
                // flip into something far more expensive.
                allow_modeset: frame == 0,
            })
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        let events = output
            .wait_for_event(500_000_000)
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        assert!(
            events.contains(&OutputEvent::FlipComplete { fb }),
            "frame {frame} never flipped: {events:?}"
        );
    }

    for (texture, fb) in slots {
        output.release_framebuffer(fb);
        ctx.destroy_texture(texture);
    }
}

#[test]
fn committing_a_framebuffer_this_output_did_not_import_is_refused() {
    let Some((mut output, _card)) = output() else {
        return;
    };
    // A handle from somewhere else names a framebuffer the kernel would reject
    // or, worse, one belonging to another import. Refusing before the commit
    // keeps that from being an atomic failure nobody can read.
    let result = output.commit(CommitRequest {
        fb: emblema_present_drm::FbHandle(0xdead_beef),
        in_fence_fd: None,
        allow_modeset: false,
    });
    assert!(result.is_err(), "an unknown framebuffer was committed");
}

#[test]
fn the_scanout_target_drives_a_real_display_controller() {
    // The whole stack, with nothing standing in: the presentation target's ring
    // and fence accounting, driving a real KMS device, rendering with the HAL.
    // Every test above this one exercises a piece; this is the one that says
    // the pieces fit.
    use emblema_hal_vulkan::VulkanHal;
    use emblema_present::PresentTarget;
    use emblema_present_drm::DrmScanoutTarget;

    let (Some((output, _card)), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let mode = output.mode();
    eprintln!(
        "{}x{} at {} mHz",
        mode.extent.width, mode.extent.height, mode.refresh_mhz
    );

    // Three buffers: one on screen, one being flipped to, one being drawn.
    let mut target = match DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3) {
        Ok(target) => target,
        // A device that cannot allocate its own scanout buffers has not
        // failed this -- it has not got the thing this drives. Rockchip on a
        // Radxa Zero 3 says exactly that and wants the GBM path. Only
        // `Unsupported` skips; every other error still fails, so a defect
        // cannot hide here.
        Err(emblema_hal::Error::Unsupported(what)) => {
            eprintln!("skipping: {what}");
            return;
        }
        Err(e) => panic!("building the scanout target: {e}"),
    };

    for frame in 0..8u32 {
        let image = target.acquire(&mut ctx).expect("acquire");
        let mut batch = Batch::new();
        let t = frame as f32 / 8.0;
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([t, 0.4, 1.0 - t, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        // Deferred every frame, so every commit has a fence to hand the
        // kernel. The target withholds it on the one commit that also
        // modesets, which is the case vkms never completes a flip for.
        let fence = ctx
            .submit_batch_deferred(image, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("deferred submission");
        target
            .set_frame_fence(fence)
            .expect("attach the render fence");
        target
            .present(&mut ctx)
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
    }

    // Every frame reached the controller, and the ring did not stall waiting
    // for one that never flipped. The CPU-wait count is what says whether the
    // fence rode the commit or the frame loop had to block first.
    // Exactly one: the first frame, whose commit also set the mode. Every
    // frame after it handed its fence to the kernel and blocked on nothing,
    // which is the property the DRM path exists for and the one a count of
    // zero-or-eight would fail to distinguish.
    assert_eq!(
        target.cpu_waits(),
        1,
        "expected a stall only on the modesetting frame"
    );
    eprintln!("ring depth {}", target.ring_depth());
    target.destroy(&mut ctx);
}

/// Count how many open descriptors this process holds.
///
/// The dma-buf and sync_file exchange on this path is all descriptors, so a count
/// that climbs over a run is the shape a leak takes here.
fn open_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// A long run neither leaks nor loses track of the blanks it landed on.
///
/// Sixty frames rather than eight, because what this is about is what does *not*
/// change over a run: the framebuffers the output holds, the descriptors this
/// process holds, and the count of commits that had to wait on the CPU.
///
/// **No deadline is asserted here and none can be.** The suite that runs on a board
/// is a debug build -- `docs/on-a-board.md` cross-builds without `--release` -- so a
/// frame here is an unoptimized frame measured against a real sixteen-millisecond
/// budget, and an assertion about missed blanks would be an assertion about the
/// optimizer. The number belongs to the release-built example, and this test's job
/// is that the ledger and the bookkeeping are sound wherever it runs.
///
/// What it does check about pacing is the one thing that can be wrong rather than
/// merely slow: that the blanks the ledger counted account for the time the flips
/// arrived over. A ledger reporting more flips than the display could have shown in
/// that span is reading something other than vertical blanks, and no amount of
/// arithmetic self-consistency would reveal it -- `missed` is *defined* from the
/// sequence numbers, so comparing it back against them proves nothing.
#[test]
fn a_long_run_neither_leaks_nor_loses_blanks() {
    use emblema_hal_vulkan::VulkanHal;
    use emblema_present::PresentTarget;
    use emblema_present_drm::DrmScanoutTarget;

    const FRAMES: u32 = 60;

    let (Some((output, _card)), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let period = output.exact_frame_nanos();
    let mut target = match DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3) {
        Ok(target) => target,
        // A device that cannot allocate its own scanout buffers has not
        // failed this -- it has not got the thing this drives. Rockchip on a
        // Radxa Zero 3 says exactly that and wants the GBM path. Only
        // `Unsupported` skips; every other error still fails, so a defect
        // cannot hide here.
        Err(emblema_hal::Error::Unsupported(what)) => {
            eprintln!("skipping: {what}");
            return;
        }
        Err(e) => panic!("building the scanout target: {e}"),
    };

    // Taken after the ring is built, so the buffers it imports on purpose are not
    // counted as growth.
    let framebuffers = target.output().framebuffer_count();
    let ring_depth = target.ring_depth();
    let descriptors = open_descriptors();

    for frame in 0..FRAMES {
        let image = target.acquire(&mut ctx).expect("acquire");
        let mut batch = Batch::new();
        let t = (frame % 8) as f32 / 8.0;
        batch
            .push(
                &FULL,
                &QUAD,
                Material::solid([t, 0.4, 1.0 - t, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
        let fence = ctx
            .submit_batch_deferred(image, &batch, PassDescriptor::clear([0.0; 4]))
            .expect("deferred submission");
        target
            .set_frame_fence(fence)
            .expect("attach the render fence");
        target
            .present(&mut ctx)
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
    }

    let pacing = target.output().pacing();
    eprintln!(
        "{FRAMES} frames: {} flips over {} blanks, missed {:?}, cpu waits {}, ring {}",
        pacing.flips(),
        pacing.elapsed_vblanks(),
        pacing.missed(),
        target.cpu_waits(),
        ring_depth,
    );

    assert!(
        pacing.flips() > 0,
        "sixty frames produced no counted flip, so the ledger saw nothing"
    );

    // The cross-check. Flips cannot have arrived faster than the display refreshes,
    // so the span has to cover the blanks between them. A tenth of slack because
    // the kernel's timestamp is the flip's and the first counted flip may follow a
    // longer interval than the ones after it.
    if pacing.usable() {
        if let Some(period) = period {
            let expected = period * pacing.elapsed_vblanks();
            let span = pacing.span().as_nanos() as u64;
            assert!(
                span * 10 >= expected * 9,
                "{} flips over {} blanks arrived in {span} ns, which is less than \
                 the {expected} ns those blanks take -- the sequence numbers are \
                 not counting vertical blanks",
                pacing.flips(),
                pacing.elapsed_vblanks()
            );
        }
    } else {
        eprintln!(
            "skipping the blank cross-check: this driver's flip sequence did not advance usably"
        );
    }

    assert_eq!(
        target.output().framebuffer_count(),
        framebuffers,
        "the output holds more framebuffers than it started the run with"
    );
    assert_eq!(
        target.ring_depth(),
        ring_depth,
        "the ring changed depth under a steady loop"
    );
    assert_eq!(
        target.cpu_waits(),
        1,
        "a commit past the modesetting one had to wait on the CPU"
    );

    target.destroy(&mut ctx);

    // After teardown, so the ring's own descriptors are gone and what is left is
    // what the loop failed to release.
    let after = open_descriptors();
    assert!(
        after <= descriptors,
        "sixty frames left {} open descriptors behind",
        after - descriptors
    );
}

#[test]
fn a_frame_with_layers_reaches_a_real_display_controller() {
    // The same whole-stack check as above, for a frame that composites a layer.
    // That path is different in kind rather than in degree: the recording is
    // several passes, the layer ones render into targets of their own, and the
    // pass the controller scans out is the one that samples them. Nothing in
    // the flat case exercises a deferred submission that samples anything, and
    // this is the only place where getting it wrong means a display shows it.
    use emblema_core::{Canvas, Color, Layer, Paint, Rect, Vec2};
    use emblema_hal_vulkan::VulkanHal;
    use emblema_present::PresentTarget;
    use emblema_present_drm::DrmScanoutTarget;

    // Validation on, unlike the flat loop next to it. This is the only test
    // where a deferred submission samples anything, so it is the only one where
    // a descriptor pool freed while the GPU still reads it, or a layer target
    // released before the commit it feeds has flipped, is possible at all --
    // and none of that shows in a picture nobody reads back.
    let Some((output, _card)) = output() else {
        return;
    };
    let Some(mut ctx) = context() else {
        return;
    };
    let mut target = match DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3) {
        Ok(target) => target,
        // A device that cannot allocate its own scanout buffers has not
        // failed this -- it has not got the thing this drives. Rockchip on a
        // Radxa Zero 3 says exactly that and wants the GBM path. Only
        // `Unsupported` skips; every other error still fails, so a defect
        // cannot hide here.
        Err(emblema_hal::Error::Unsupported(what)) => {
            eprintln!("skipping: {what}");
            return;
        }
        Err(e) => panic!("building the scanout target: {e}"),
    };
    let extent = target.extent();

    let scene = |t: f32| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::linear(0.03, 0.03, 0.08, 1.0));
        let side = extent.width.min(extent.height) as f32;
        let region = Rect::new(
            extent.width as f32 * 0.25,
            extent.height as f32 * 0.25,
            extent.width as f32 * 0.75,
            extent.height as f32 * 0.75,
        );
        // Bounded, so the layer target is smaller than the scanout buffer and
        // sits at an offset inside it -- the case where a mapping that ignored
        // the target's origin would put the composite in the wrong place.
        canvas.save_layer_bounds(Layer::opacity(0.6), region);
        for (dx, color) in [(-0.06, [0.9, 0.3, 0.2]), (0.06, [0.2, 0.7, 0.9])] {
            canvas
                .draw_circle(
                    Vec2::new(extent.width as f32 * (0.5 + dx), extent.height as f32 * 0.5),
                    side * (0.12 + 0.02 * t),
                    &Paint::fill(Color::linear(color[0], color[1], color[2], 1.0))
                        .with_anti_alias(false),
                )
                .expect("circle");
        }
        canvas.restore();
        canvas.finish()
    };
    assert!(
        scene(0.0).passes.len() > 1,
        "the scene has no layer, so this proves nothing"
    );

    for frame in 0..8u32 {
        let recording = scene(frame as f32 / 8.0);
        let _ = target.acquire(&mut ctx).expect("acquire");
        target
            .submit_recording(&mut ctx, &recording, &[])
            .unwrap_or_else(|e| panic!("frame {frame}: submitting a recording: {e}"));
        target
            .present(&mut ctx)
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
    }

    // The same property the flat loop has, and the one the layers could have
    // broken: every commit but the modesetting first hands its fence to the
    // kernel rather than stalling the CPU. A layer pass that had to be waited
    // on separately, or a fence that could not be exported because the
    // submission sampled something, would show up here as eight.
    assert_eq!(
        target.cpu_waits(),
        1,
        "expected a stall only on the modesetting frame"
    );
    target.destroy(&mut ctx);

    // After teardown, so that a layer target still alive when the device goes
    // down is reported as the leak it is rather than passing unnoticed.
    if ctx.validation_active() {
        let errors: Vec<_> = ctx
            .validation_messages()
            .into_iter()
            .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
            .collect();
        assert!(errors.is_empty(), "validation errors: {errors:?}");
    } else {
        eprintln!("skipping: the validation layer is unavailable, so lifetimes went unchecked");
    }
}

/// A storm of resizes against a real display controller.
///
/// A resize here is a modeset: the plane is told to read a rectangle the size
/// of the mode, so changing the buffer size without changing the mode makes the
/// kernel refuse the whole atomic request. That is what this used to do --
/// `reconfigure` accepted any extent, rebuilt the ring at it and left the mode
/// alone, and the next `present` came back `ENOSPC` with nothing naming why.
///
/// Storming it is what a single resize cannot answer. Each round releases every
/// framebuffer the controller holds and gives it new ones, so a framebuffer
/// destroyed without its buffer objects closed, or a descriptor left behind by
/// the export, accumulates into something a before-and-after of one resize
/// would not show.
///
/// Three numbers are held across the storm: the framebuffers the output keeps,
/// the descriptors this process keeps, and the count of commits that had to
/// wait on the CPU -- which should be exactly one per modeset, the frame whose
/// commit sets the mode and therefore withholds its fence.
#[test]
fn a_storm_of_resizes_neither_leaks_nor_breaks_the_commit() {
    use emblema_hal_vulkan::VulkanHal;
    use emblema_present::PresentTarget;
    use emblema_present_drm::DrmScanoutTarget;

    let (Some((output, _card)), Some(mut ctx)) = (output(), context()) else {
        return;
    };

    // Distinct sizes, smallest first, at most three of them. A connector often
    // lists the same size at several refresh rates, and resizing between two of
    // those is not a resize. Smallest first and capped because the point is the
    // number of rebuilds, not their area: a 4096x2160 ring is a hundred
    // megabytes to allocate, export and import, several times over.
    //
    // The size the target will open at goes first by construction, not by
    // hoping it survives the sort: the opening resize is then the no-op it
    // would be anyway, and the count of modesets below follows from the rounds
    // rather than from which modes this particular connector happens to list.
    let settled = output.mode().extent;
    let mut others: Vec<emblema_hal::Extent2D> = Vec::new();
    for mode in output.modes() {
        if mode.extent != settled && !others.contains(&mode.extent) {
            others.push(mode.extent);
        }
    }
    others.sort_by_key(|e| (e.width as u64) * (e.height as u64));
    others.truncate(2);
    if others.is_empty() {
        eprintln!("skipping: this connector offers one size, so there is nothing to resize to");
        return;
    }
    let mut extents = vec![settled];
    extents.extend(others);
    eprintln!("storming {extents:?}");

    const DEPTH: usize = 3;
    const ROUNDS: usize = 4;
    let mut target = match DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, DEPTH) {
        Ok(target) => target,
        // A device that cannot allocate its own scanout buffers has not
        // failed this -- it has not got the thing this drives. Rockchip on a
        // Radxa Zero 3 says exactly that and wants the GBM path. Only
        // `Unsupported` skips; every other error still fails, so a defect
        // cannot hide here.
        Err(emblema_hal::Error::Unsupported(what)) => {
            eprintln!("skipping: {what}");
            return;
        }
        Err(e) => panic!("building the scanout target: {e}"),
    };
    let descriptors = open_descriptors();

    let mut modesets = 0usize;
    for round in 0..ROUNDS {
        for extent in &extents {
            let changed = target.extent() != *extent;
            target
                .reconfigure(&mut ctx, *extent)
                .unwrap_or_else(|e| panic!("round {round} resize to {extent:?}: {e}"));
            if changed {
                modesets += 1;
            }
            assert_eq!(target.extent(), *extent, "round {round}");
            assert_eq!(target.ring_depth(), DEPTH, "round {round}");

            // Two frames: the first sets the mode, the second flips onto it.
            for which in 0..2 {
                let image = target.acquire(&mut ctx).expect("acquire");
                let mut batch = Batch::new();
                batch
                    .push(
                        &FULL,
                        &QUAD,
                        Material::solid([0.2, 0.6, 0.9, 1.0]),
                        BlendMode::Src,
                    )
                    .expect("push");
                let fence = ctx
                    .submit_batch_deferred(image, &batch, PassDescriptor::clear([0.0; 4]))
                    .expect("deferred submission");
                target.set_frame_fence(fence).expect("attach the fence");
                target
                    .present(&mut ctx)
                    .unwrap_or_else(|e| panic!("round {round}, frame {which} at {extent:?}: {e}"));
            }

            assert_eq!(
                target.output().framebuffer_count(),
                DEPTH,
                "round {round} at {extent:?}: the controller is holding framebuffers from an \
                 earlier size"
            );
        }
    }

    assert_eq!(
        open_descriptors(),
        descriptors,
        "descriptors climbed across {ROUNDS} rounds of resizes"
    );
    // One per modeset and no more: the opening commit and each resize's first
    // frame. A count above this means a frame blocked on the CPU that should
    // have handed its fence to the kernel.
    assert_eq!(
        target.cpu_waits(),
        modesets as u64 + 1,
        "expected a stall only on each modesetting frame"
    );
    target.destroy(&mut ctx);
}

/// GBM allocates a scanout buffer and the renderer draws into it.
///
/// Behind the `gbm` feature, which is off by default: `drmkit-gbm` declares a
/// higher `rust-version` than this workspace does, and the MSRV a published
/// crate states is a promise to its callers rather than a detail. The feature
/// is what a caller who wants this path opts into, along with the newer
/// toolchain it needs.
///
/// The path this crate named from the start and could not take: where a device
/// cannot allocate its own scanout buffers, something else allocates and the
/// renderer imports. Both halves are exercised here against a real card --
/// `drmkit-gbm` allocating on the display's own descriptor, and
/// `import_texture` making it a render target -- because either alone proves
/// nothing about the other.
///
/// The display is served too, from a second descriptor onto the same
/// allocation. That is the arrangement a ring needs: the renderer and the
/// display each hold their own descriptor, and `gbm_bo_get_fd` hands out a new
/// one per call.
///
/// Checked by the pixels rather than by the handles. An allocation whose stride
/// was mis-stated, or an import bound to the wrong memory, still yields a
/// texture and a framebuffer handle.
#[test]
#[cfg(feature = "gbm")]
fn gbm_allocates_a_buffer_the_renderer_can_draw_into() {
    use emblema_present_drm::gbm::GbmAllocator;

    let Some((mut output, _guard)) = output() else {
        return;
    };
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    if !ctx.capabilities().dma_buf.import {
        eprintln!("skipping: this device cannot import a dma-buf");
        return;
    }
    let Some(fd) = output.device_fd() else {
        eprintln!("skipping: this output has no DRM device to allocate on");
        return;
    };
    let allocator = match GbmAllocator::open(fd) {
        Ok(allocator) => allocator,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };

    let formats = output.supported_formats().to_vec();
    let Some(set) = formats.iter().find(|s| {
        s.fourcc == emblema_hal::Fourcc::XRGB8888 || s.fourcc == emblema_hal::Fourcc::ARGB8888
    }) else {
        eprintln!("skipping: this display offers no eight-bit format");
        return;
    };
    let extent = output.mode().extent;

    let allocated = match allocator.allocate(extent, set.fourcc, &set.modifiers) {
        Ok(allocated) => allocated,
        Err(e) => {
            eprintln!("skipping: GBM would not allocate for this display ({e})");
            return;
        }
    };
    eprintln!(
        "gbm allocated {}x{} as {} at modifier {:#x}",
        extent.width,
        extent.height,
        allocated.fourcc(),
        allocated.modifier().0
    );

    // The renderer's descriptor.
    let described = allocated.describe().expect("a descriptor for the renderer");
    let format = match allocated.fourcc() {
        f if f == emblema_hal::Fourcc::ARGB8888 || f == emblema_hal::Fourcc::XRGB8888 => {
            PixelFormat::Bgra8Unorm
        }
        other => panic!("unexpected format {other}"),
    };
    let mut texture = match ctx.import_texture(described, extent, format) {
        Ok(texture) => texture,
        Err(e) => {
            // Reported rather than skipped: a GBM buffer this renderer cannot
            // import is the whole path failing, not a capability gap.
            panic!("importing a GBM buffer was refused: {e}");
        }
    };

    let mut batch = Batch::new();
    batch
        .push(
            &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([0.0, 0.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    ctx.submit_batch(
        &mut texture,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit into the GBM buffer");

    let pixels = ctx.read_texture(&mut texture).expect("readback");
    let wrong = pixels
        .chunks_exact(4)
        .filter(|texel| texel[0] != 255 || texel[1] != 0 || texel[2] != 0)
        .count();
    assert_eq!(
        wrong,
        0,
        "{wrong} of {} texels in the GBM buffer are not the blue that was drawn; first is {:?}",
        pixels.len() / 4,
        &pixels[..4]
    );

    // And the display's own descriptor onto the same allocation.
    let for_display = allocated.describe().expect("a second descriptor");
    // Whether this display ever offered what came back. A driver may answer a
    // constrained request with a layout outside the list -- measured on two
    // boards -- and a display refusing a layout it never advertised is not a
    // defect.
    let advertised = formats
        .iter()
        .any(|s| s.fourcc == allocated.fourcc() && s.modifiers.contains(&allocated.modifier()));
    match output.import_dmabuf(DmaBufPlanes {
        planes: for_display.planes,
        fourcc: for_display.fourcc,
        modifier: for_display.modifier,
        extent,
    }) {
        Ok(fb) => {
            eprintln!("the display took the same allocation as a framebuffer");
            output.release_framebuffer(fb);
        }
        Err(e) if !advertised => eprintln!(
            "skipping the display half: it does not advertise {} at {:#x} ({e})",
            allocated.fourcc(),
            allocated.modifier().0
        ),
        // Asserted rather than skipped, and this is the load-bearing half. The
        // kernel validates a stride against the buffer's size, so it is the one
        // check here independent of the descriptor this test wrote: rendering
        // into the imported image and reading it back uses that same layout
        // both ways, so a stride stated wrongly cancels out and the pixels come
        // back perfect. Measured -- doubling the stride passes the readback and
        // fails here with `add_planar_framebuffer: Invalid argument`.
        Err(e) => panic!(
            "the display refused a buffer at {} and {:#x}, both of which it advertises: {e}",
            allocated.fourcc(),
            allocated.modifier().0
        ),
    }

    ctx.destroy_texture(texture);
}

/// A GBM-allocated ring presents, and survives a resize.
///
/// The path `DrmScanoutTarget::new` has refused into since it was written. The
/// previous test proved one GBM buffer can be allocated, imported and handed to
/// the display; this proves a *ring* of them works as a target — acquire, draw,
/// present, repeatedly, which is where the slot's ownership of its buffer
/// matters.
///
/// The resize is half the point. `reconfigure` releases the ring and builds it
/// again, and a GBM ring has to rebuild through GBM: dispatching to the
/// renderer-allocated path would ask a device that cannot allocate scanout
/// buffers to do exactly that, and the failure would arrive at the `present`
/// after the call that caused it. That is the shape of the defect writing the
/// resize storm found in the first place, so it is checked rather than assumed.
///
/// Run on any card, including one whose renderer *can* allocate. Comparing the
/// two paths on one machine is how one of them being wrong gets noticed.
#[test]
#[cfg(feature = "gbm")]
fn a_gbm_allocated_ring_presents_and_survives_a_resize() {
    use emblema_hal_vulkan::VulkanHal;
    use emblema_present::PresentTarget;
    use emblema_present_drm::DrmScanoutTarget;

    const DEPTH: usize = 3;

    let (Some((output, _card)), Some(mut ctx)) = (output(), context()) else {
        return;
    };
    let settled = output.mode().extent;
    let other = output
        .modes()
        .into_iter()
        .map(|mode| mode.extent)
        .find(|extent| *extent != settled);

    let mut target = match DrmScanoutTarget::<VulkanHal, _>::with_gbm(&mut ctx, output, DEPTH) {
        Ok(target) => target,
        // Only `Unsupported` skips: no card behind the output, or a renderer
        // that cannot import. Every other error still fails, so a defect in the
        // GBM path cannot hide here.
        Err(emblema_hal::Error::Unsupported(what)) => {
            eprintln!("skipping: {what}");
            return;
        }
        Err(e) => panic!("building a GBM scanout target: {e}"),
    };
    assert_eq!(target.ring_depth(), DEPTH, "the ring was not built in full");
    // On a device that can allocate its own scanout buffers -- this workstation
    // can -- the two paths present and resize identically, so everything below
    // would pass on a target that quietly took the renderer-allocated one. This
    // is the one assertion that says which path is under test. What it still
    // cannot say is which builder filled the slots; the run that distinguishes
    // that is a device where `new` refuses, which is a board and not here.
    assert!(
        target.allocates_through_gbm(),
        "with_gbm produced a target that does not allocate through GBM"
    );
    eprintln!("gbm ring of {DEPTH} at {settled:?}");

    // A macro rather than a closure: the closure needed the target's full type
    // written out to infer, and the second parameter of `DrmScanoutTarget` is
    // the output's, which this test never names.
    macro_rules! present_four {
        ($what:expr) => {
            for frame in 0..4u32 {
                let image = target.acquire(&mut ctx).expect("acquire");
                let mut batch = Batch::new();
                let t = frame as f32 / 4.0;
                batch
                    .push(
                        &FULL,
                        &QUAD,
                        Material::solid([t, 0.5, 1.0 - t, 1.0]),
                        BlendMode::Src,
                    )
                    .expect("push");
                let fence = ctx
                    .submit_batch_deferred(image, &batch, PassDescriptor::clear([0.0; 4]))
                    .expect("deferred submission");
                target.set_frame_fence(fence).expect("attach the fence");
                target
                    .present(&mut ctx)
                    .unwrap_or_else(|e| panic!("{}, frame {frame}: {e}", $what));
            }
        };
    }

    present_four!("at the settled mode");
    assert_eq!(
        target.output().framebuffer_count(),
        DEPTH,
        "the controller is holding more framebuffers than the ring has"
    );

    let Some(other) = other else {
        eprintln!("skipping the resize: this connector offers one extent");
        return;
    };
    eprintln!("resizing to {other:?}");
    target
        .reconfigure(&mut ctx, other)
        .unwrap_or_else(|e| panic!("reconfiguring a GBM ring to {other:?}: {e}"));
    assert_eq!(
        target.extent(),
        other,
        "the target did not take the new extent"
    );
    assert_eq!(
        target.ring_depth(),
        DEPTH,
        "the ring was not rebuilt in full"
    );

    // The present after the resize is the one that caught the original defect:
    // `reconfigure` returned `Ok` and the next commit came back `ENOSPC`.
    present_four!("after the resize");
    assert_eq!(
        target.output().framebuffer_count(),
        DEPTH,
        "framebuffers from the earlier size are still held"
    );

    // Back to where it started, which does two jobs. It exercises the GBM
    // dispatch in the shrinking direction as well as the growing one, and it
    // drains: `reconfigure` releases the ring, and releasing waits each slot's
    // fence. Without it the last frames submitted at the larger mode are still
    // executing when this returns, and the validation layer reports a pipeline
    // and a render pass destroyed while a command buffer still refers to them.
    // Measured, which is how this line came to be here.
    target
        .reconfigure(&mut ctx, settled)
        .unwrap_or_else(|e| panic!("reconfiguring back to {settled:?}: {e}"));
    assert_eq!(target.extent(), settled, "the target did not shrink back");
    assert_eq!(
        target.ring_depth(),
        DEPTH,
        "the ring was not rebuilt in full"
    );

    // `PresentTarget::destroy`, as every other target test in this file ends.
    // It waits each slot's fence, releases the framebuffers and destroys the
    // textures -- and on this path it drops the GBM allocations last, after
    // both the view the renderer held and the one the display held are gone.
    // Without it the ring's three imported images and their memory outlive the
    // device, which the validation layer reports at `vkDestroyDevice`. That is
    // how the ordering above was confirmed rather than assumed.
    target.destroy(&mut ctx);
}

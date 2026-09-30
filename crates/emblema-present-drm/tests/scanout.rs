//! The scanout frame loop, driven against a stand-in display.
//!
//! No KMS device is involved. That is deliberate rather than a limitation: the
//! parts of this path most likely to be wrong are the ring accounting and the
//! fence plumbing, and neither needs real hardware to get wrong. A display that
//! records what it was asked to do makes those assertions direct — that a
//! buffer still on screen is never handed back, that a commit carries the
//! render-done signal, that acquiring waits for a flip rather than spinning.
//!
//! What this cannot check is whether a real display controller accepts the
//! buffers. That is what the VKMS lane and the board rack are for.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it. A failed assumption in a test should stop the
// run; the workspace denies these because a *library* must not.
#![allow(clippy::unwrap_used)]

use emblema_hal::{
    Batch, BlendMode, Extent2D, FormatModifierSet, Fourcc, Material, Modifier, PassDescriptor,
    Result,
};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext, VulkanHal};
use emblema_present::PresentTarget;
use emblema_present_drm::{
    output::{CommitRequest, DmaBufPlanes, OutputEvent, ScanoutOutput},
    DrmScanoutTarget, FbHandle, Mode,
};
use std::sync::{Arc, Mutex};

/// What a commit carried, recorded for inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedCommit {
    fb: FbHandle,
    had_fence: bool,
    allow_modeset: bool,
}

#[derive(Debug, Default)]
struct Recording {
    commits: Vec<RecordedCommit>,
    imported: Vec<FbHandle>,
    released: Vec<FbHandle>,
    waits: usize,
}

/// A display that records what it was asked to do and flips on demand.
struct FakeOutput {
    mode: Mode,
    formats: Vec<FormatModifierSet>,
    next_fb: u64,
    log: Arc<Mutex<Recording>>,
    pending: Vec<OutputEvent>,
    /// When true, a commit immediately queues its own flip completion, which is
    /// what a display doing its job looks like.
    auto_flip: bool,
}

impl FakeOutput {
    fn new(formats: Vec<FormatModifierSet>) -> (Self, Arc<Mutex<Recording>>) {
        let log = Arc::new(Mutex::new(Recording::default()));
        (
            Self {
                mode: Mode {
                    extent: Extent2D::new(64, 64),
                    refresh_mhz: 60_000,
                },
                formats,
                next_fb: 1,
                log: Arc::clone(&log),
                pending: Vec::new(),
                auto_flip: true,
            },
            log,
        )
    }
}

impl ScanoutOutput for FakeOutput {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn supported_formats(&self) -> &[FormatModifierSet] {
        &self.formats
    }

    fn import_dmabuf(&mut self, buffer: DmaBufPlanes) -> Result<FbHandle> {
        // A real import fails on a mismatched modifier, so the test double
        // checks the buffer it was handed carries one it advertised.
        let advertised = self
            .formats
            .iter()
            .find(|s| s.fourcc == buffer.fourcc)
            .expect("imported a format that was never advertised");
        assert!(
            advertised.modifiers.contains(&buffer.modifier),
            "imported modifier {} was never advertised",
            buffer.modifier
        );

        let fb = FbHandle(self.next_fb);
        self.next_fb += 1;
        self.log.lock().unwrap().imported.push(fb);
        Ok(fb)
    }

    fn release_framebuffer(&mut self, fb: FbHandle) {
        self.log.lock().unwrap().released.push(fb);
    }

    fn commit(&mut self, request: CommitRequest) -> Result<()> {
        self.log.lock().unwrap().commits.push(RecordedCommit {
            fb: request.fb,
            #[cfg(unix)]
            had_fence: request.in_fence_fd.is_some(),
            #[cfg(not(unix))]
            had_fence: false,
            allow_modeset: request.allow_modeset,
        });
        if self.auto_flip {
            self.pending
                .push(OutputEvent::FlipComplete { fb: request.fb });
        }
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<OutputEvent> {
        std::mem::take(&mut self.pending)
    }

    fn wait_for_event(&mut self, _timeout_nanos: u64) -> Result<Vec<OutputEvent>> {
        self.log.lock().unwrap().waits += 1;
        Ok(std::mem::take(&mut self.pending))
    }
}

fn context() -> Option<VulkanContext> {
    // Validation on, because these drive the deferred submission path and the
    // fences that gate it, and that is where a resource freed while the GPU is
    // still reading it shows up. Nothing about such a bug reaches the pixels:
    // the frame renders correctly right up until the memory is reused.
    match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
        ..Default::default()
    }) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

/// Fail if the validation layer reported anything.
fn assert_validation_clean(ctx: &VulkanContext) {
    if !ctx.validation_active() {
        return;
    }
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
}

/// Formats the display accepts: whatever the device can render into.
fn display_formats(ctx: &VulkanContext) -> Vec<FormatModifierSet> {
    ctx.capabilities().render_formats.clone()
}

fn scene() -> Batch {
    let mut batch = Batch::new();
    batch
        .push(
            &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    batch
}

/// Render and present one frame, as a real loop would.
fn frame(
    ctx: &mut VulkanContext,
    target: &mut DrmScanoutTarget<VulkanHal, FakeOutput>,
    batch: &Batch,
) -> Result<()> {
    let image = target.acquire(ctx)?;
    let fence = ctx.submit_batch_deferred(image, batch, PassDescriptor::clear([0.0; 4]))?;
    target.set_frame_fence(fence)?;
    target.present(ctx)
}

fn exportable(ctx: &VulkanContext) -> bool {
    if ctx.capabilities().dma_buf.can_allocate_scanout() {
        return true;
    }
    eprintln!("skipping: this device cannot allocate its own scanout buffers");
    false
}

#[test]
fn a_ring_is_built_from_exported_buffers() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, log) = FakeOutput::new(display_formats(&ctx));
    let target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3).expect("scanout target");

    assert_eq!(target.ring_depth(), 3);
    // Every buffer is exported once at startup and imported once, rather than
    // per frame. Doing it per frame would put an allocation and an import in
    // the middle of the frame loop.
    assert_eq!(log.lock().unwrap().imported.len(), 3);

    target.destroy(&mut ctx);
    assert_eq!(log.lock().unwrap().released.len(), 3, "framebuffers leaked");
    assert_validation_clean(&ctx);
}

#[test]
fn the_first_commit_allows_a_modeset_and_later_ones_do_not() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3).expect("scanout target");
    let batch = scene();

    for _ in 0..4 {
        frame(&mut ctx, &mut target, &batch).expect("frame");
    }

    let recorded = log.lock().unwrap();
    // A modeset is far more expensive than a flip. Requesting one every frame
    // would work and would be slow in a way nothing about the output reveals.
    assert!(recorded.commits[0].allow_modeset, "first commit");
    assert!(
        recorded.commits[1..].iter().all(|c| !c.allow_modeset),
        "later commits should not ask for a modeset"
    );
    drop(recorded);
    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn every_commit_carries_the_render_done_signal() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    if !ctx.capabilities().sync.export_sync_file {
        eprintln!("skipping: this device cannot export a sync_file");
        return;
    }
    let (output, log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3).expect("scanout target");
    let batch = scene();

    for _ in 0..4 {
        frame(&mut ctx, &mut target, &batch).expect("frame");
    }

    let recorded = log.lock().unwrap();
    let with_fence = recorded.commits.iter().filter(|c| c.had_fence).count();
    // Committing without a fence means the CPU waited first, which is correct
    // and costs a frame of latency. On hardware that can export, it should not
    // be happening at all, and the counter is what makes that visible rather
    // than merely slow.
    assert!(
        with_fence > 0,
        "no commit carried a sync_file, so every frame blocked the CPU first"
    );
    eprintln!(
        "  {with_fence} of {} commits carried a fence, {} CPU waits",
        recorded.commits.len(),
        target.cpu_waits()
    );
    drop(recorded);
    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn a_buffer_still_on_screen_is_never_handed_back() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (mut output, log) = FakeOutput::new(display_formats(&ctx));
    // No automatic flips: nothing ever leaves the screen. A display controller
    // takes one flip at a time, so the loop stalls at the second frame's
    // commit rather than getting two in and stalling on the third acquire —
    // which is what this test expected before the target learned to wait for
    // the outstanding flip, and what a real controller refuses outright.
    output.auto_flip = false;
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2).expect("scanout target");
    // This display is never going to flip, which the test arranged, so there is
    // nothing to learn from waiting the full default for it.
    target.set_flip_timeout(std::time::Duration::from_millis(50));
    let batch = scene();

    frame(&mut ctx, &mut target, &batch).expect("first frame");

    // The first frame is committed and has not flipped away. Committing again
    // would be refused by a real controller, and reusing its buffer would
    // tear, so the loop must stall instead.
    let stalled = frame(&mut ctx, &mut target, &batch);
    let error = stalled.expect_err("a second frame was committed while the first had not flipped");
    // Named, not merely reported. A frame loop can stall on a fence that never
    // signals, a flip that never lands, or a slot that never comes free, and
    // one message for all three means working out which timeout could have
    // elapsed in the time the run took -- which is what a flake in this file
    // once cost.
    let text = error.to_string();
    assert!(
        text.contains("flip"),
        "the error should say what it waited for, got: {text}"
    );
    assert!(
        log.lock().unwrap().waits > 0,
        "acquire spun instead of waiting"
    );

    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn a_flip_frees_the_buffer_it_replaced() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, _log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2).expect("scanout target");
    let batch = scene();

    // With flips arriving, a two-deep ring sustains an indefinite loop. If a
    // completed flip did not free the buffer it replaced, this would stall on
    // the third frame.
    for i in 0..16 {
        frame(&mut ctx, &mut target, &batch).unwrap_or_else(|e| panic!("frame {i}: {e}"));
    }

    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn presenting_without_acquiring_is_refused() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    let (output, _log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2).expect("scanout target");

    // Committing a buffer nobody rendered into would put whatever it last held
    // on screen.
    assert!(target.present(&mut ctx).is_err());
    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn negotiation_runs_against_what_the_display_advertises() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    // A display that shares nothing with the renderer must fail to build a
    // target, rather than quietly picking something the plane cannot scan out.
    let incompatible = vec![FormatModifierSet::new(
        Fourcc::new(*b"NV12"),
        vec![Modifier::LINEAR],
    )];
    let (output, _log) = FakeOutput::new(incompatible);
    let result = DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 2);
    assert!(result.is_err(), "a target was built with no shared format");
}

#[test]
fn negotiation_against_a_real_plane_agrees_on_a_layout() {
    // Until now both halves of format negotiation came from the same place in
    // a test: the render side from a device, and the display side from a list
    // the test wrote. This is the first time the display's half is what a
    // display actually advertises, which is the only way to learn that the two
    // agree on hardware rather than on paper.
    //
    // Reading a plane's formats needs no master, so this runs on an ordinary
    // desktop with a compositor driving the card.
    use emblema_present_drm::device::DrmDevice;

    let Some(ctx) = context() else { return };
    let Some(card) = std::fs::read_dir("/dev/dri")
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card"))
        })
    else {
        eprintln!("skipping: no card node on this machine");
        return;
    };
    let device = match DrmDevice::open(&card.to_string_lossy()) {
        Ok(device) => device,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let scanout = device.scanout_formats().expect("plane formats");
    if scanout.is_empty() {
        eprintln!("skipping: this plane advertises nothing");
        return;
    }

    let render = display_formats(&ctx);
    let preferred = [Fourcc::ARGB8888, Fourcc::XRGB8888, Fourcc::ABGR8888];
    let agreed = emblema_present::negotiate::negotiate(&render, &scanout, &preferred)
        .expect("the render device and this plane share no layout at all");

    // Agreeing linear is a valid outcome and a slow one, and on hardware where
    // both sides advertise tiled layouts it means the modifier lists were
    // intersected wrongly rather than that nothing better existed.
    let both_have_tiled = render
        .iter()
        .any(|s| s.modifiers.iter().any(|m| !m.is_linear()))
        && scanout
            .iter()
            .any(|s| s.modifiers.iter().any(|m| !m.is_linear()));
    if both_have_tiled {
        assert!(
            !agreed.is_linear(),
            "both sides advertise tiled layouts and negotiation settled for linear: {agreed:?}"
        );
    }
    assert_validation_clean(&ctx);
}

#[test]
fn a_frame_with_layers_reaches_the_scanout_buffer() {
    let Some(mut ctx) = context() else { return };
    if !exportable(&ctx) {
        return;
    }
    // A recording with layers is several passes, and the one that composites
    // them is the one that has to land in the buffer the display scans out.
    // Submitting a batch cannot express that, so before `submit_recording` a
    // frame with layers could not be scanned out at all -- the same gap the
    // swapchain had, in the path that has no window to fall back on.
    //
    // What is checked is that the loop still holds together around it: the
    // commit carries the render-done signal, buffers are cycled rather than
    // reused while in flight, and nothing is leaked. That the composite lands
    // in the right pixels is the swapchain's comparison against an offscreen
    // render; here the buffer is scanned out rather than read back.
    use emblema_core::{Canvas, Color, Layer, Paint, Rect, Vec2};

    let (output, log) = FakeOutput::new(display_formats(&ctx));
    let mut target =
        DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3).expect("scanout target");
    let extent = target.extent();

    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::linear(0.05, 0.05, 0.1, 1.0));
    canvas.save_layer_bounds(
        Layer::opacity(0.6),
        Rect::new(
            4.0,
            4.0,
            extent.width as f32 / 2.0,
            extent.height as f32 / 2.0,
        ),
    );
    canvas
        .draw_circle(
            Vec2::new(extent.width as f32 / 4.0, extent.height as f32 / 4.0),
            (extent.width.min(extent.height) as f32) / 8.0,
            &Paint::fill(Color::linear(0.9, 0.3, 0.2, 1.0)).with_anti_alias(false),
        )
        .expect("circle");
    canvas.restore();
    let recording = canvas.finish();
    assert!(
        recording.passes.len() > 1,
        "the scene has no layer, so this proves nothing"
    );

    // More frames than the ring is deep, so slots are recycled and the layer
    // targets a recycled slot was holding are released rather than accumulated.
    for _ in 0..5 {
        let _ = target.acquire(&mut ctx).expect("acquire");
        target
            .submit_recording(&mut ctx, &recording, &[])
            .expect("submit recording");
        target.present(&mut ctx).expect("present");
    }

    {
        let recorded = log.lock().unwrap();
        assert_eq!(recorded.commits.len(), 5, "one commit per frame");
        // The commits after the modeset carry the fence, which is the whole
        // point of the deferred path: the kernel waits for the render rather
        // than the CPU doing it.
        assert!(
            recorded.commits[1..].iter().all(|c| c.had_fence),
            "a commit after the modeset went without the render-done signal: {:?}",
            recorded.commits
        );
    }

    target.destroy(&mut ctx);
    assert_eq!(log.lock().unwrap().released.len(), 3, "framebuffers leaked");
    assert_validation_clean(&ctx);
}

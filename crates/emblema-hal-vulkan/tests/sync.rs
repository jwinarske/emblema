//! Exporting GPU completion as a sync_file.
//!
//! The second of the two requirements the HAL reserved from day one. The DRM
//! presentation path attaches this descriptor to an atomic commit so the kernel
//! latches the page flip when rendering finishes, instead of the CPU waiting
//! and then committing. Without it the frame loop blocks in the middle, which
//! is a full frame of latency rather than a correctness problem — the kind of
//! regression that is easy to introduce and hard to notice.

use emblema_hal::{Batch, BlendMode, Extent2D, HalFence, Material, PassDescriptor, PixelFormat};
use emblema_hal::{TextureDescriptor, FRAME_WAIT_TIMEOUT};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

fn context() -> Option<VulkanContext> {
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

fn scene() -> Batch {
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    batch
}

fn target(ctx: &mut VulkanContext) -> emblema_hal_vulkan::VulkanTexture {
    ctx.create_texture(&TextureDescriptor::offscreen(
        Extent2D::new(64, 64),
        PixelFormat::Rgba8Unorm,
    ))
    .expect("texture")
}

fn assert_clean(ctx: &VulkanContext) {
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
}

#[test]
fn a_deferred_submission_returns_a_fence_that_eventually_signals() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    let batch = scene();

    let fence = ctx
        .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("deferred submit");

    // Waiting is still available and is what a caller without fence export
    // falls back to. A timeout here would mean work was never submitted.
    assert!(
        fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"),
        "the fence never signaled"
    );
    assert!(fence.is_signaled().expect("status"));

    ctx.retire_fence(fence);
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn a_fence_exports_a_sync_file_before_the_work_completes() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sync.export_sync_file {
        eprintln!("skipping: this device cannot export a sync_file");
        return;
    }
    let mut tex = target(&mut ctx);
    let batch = scene();

    let fence = ctx
        .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("deferred submit");
    assert!(fence.is_exportable());

    // Exported without waiting first: that is the entire point. Waiting and
    // then exporting would produce a descriptor that is already signaled and
    // defeat the purpose of handing it to the kernel.
    match fence.export_sync_file() {
        Ok(fd) => {
            // A real, open descriptor. An invalid one fails only much later,
            // when the atomic commit rejects it.
            fd.try_clone()
                .expect("the exported sync_file is not an open descriptor");
        }
        Err(e) => {
            // The work may already have finished, in which case there is
            // nothing left to wait on and the kernel represents that as no
            // descriptor at all. That is a legitimate outcome for a trivial
            // scene on a fast device.
            let text = e.to_string();
            assert!(
                text.contains("already signaled"),
                "unexpected export failure: {text}"
            );
        }
    }

    assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(fence);
    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn an_exported_sync_file_is_independent_of_the_fence_it_came_from() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sync.export_sync_file {
        return;
    }
    let mut tex = target(&mut ctx);
    let batch = scene();

    let fence = ctx
        .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("deferred submit");
    let exported = fence.export_sync_file().ok();

    assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(fence);

    // The descriptor outlives the fence, which is what handing it to another
    // process or to the kernel requires. Retiring the fence must not have
    // closed it.
    if let Some(fd) = exported {
        fd.try_clone()
            .expect("the sync_file was closed when the fence was retired");
    }

    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn repeated_deferred_submissions_do_not_exhaust_resources() {
    let Some(mut ctx) = context() else { return };
    let mut tex = target(&mut ctx);
    let batch = scene();

    // Each submission holds a command buffer, a framebuffer, a view, and the
    // geometry buffers it is still reading. A frame loop does this every frame,
    // so anything not released on retire accumulates until something fails.
    for i in 0..64 {
        let fence = ctx
            .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
            .unwrap_or_else(|e| panic!("iteration {i}: {e}"));
        let _ = fence.export_sync_file();
        assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
        ctx.retire_fence(fence);
    }

    let pixels = ctx.read_texture(&mut tex).expect("readback");
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn deferred_and_waiting_submission_produce_the_same_pixels() {
    let Some(mut ctx) = context() else { return };
    let batch = scene();

    let mut deferred_target = target(&mut ctx);
    let fence = ctx
        .submit_batch_deferred(
            &mut deferred_target,
            &batch,
            PassDescriptor::clear([0.0, 0.0, 1.0, 1.0]),
        )
        .expect("deferred submit");
    assert!(fence.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(fence);
    let deferred = ctx.read_texture(&mut deferred_target).expect("readback");
    ctx.destroy_texture(deferred_target);

    let mut waiting_target = target(&mut ctx);
    ctx.submit_batch(
        &mut waiting_target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 1.0, 1.0]),
    )
    .expect("submit");
    let waiting = ctx.read_texture(&mut waiting_target).expect("readback");
    ctx.destroy_texture(waiting_target);

    // Whether the caller waits is a scheduling decision, not a rendering one.
    assert_eq!(deferred, waiting, "deferring changed what was rendered");
    assert_clean(&ctx);
}

#[test]
fn a_deferred_multisampled_pass_is_refused_rather_than_leaking() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        return;
    }
    let mut tex = target(&mut ctx);
    let batch = scene();

    // The transient multisample buffer would have to outlive the submission
    // too. Refusing is better than submitting and freeing it underneath the
    // GPU, which corrupts intermittently rather than failing.
    let result = ctx.submit_batch_deferred(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0; 4]).with_samples(4),
    );
    assert!(result.is_err());

    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

#[test]
fn retiring_one_frame_does_not_free_another_frame_s_resources() {
    let Some(mut ctx) = context() else { return };
    let batch = scene();

    // Every other test here retires a fence before submitting again, so only
    // one submission is ever outstanding. A frame loop that runs two frames
    // deep does not, and that is a different thing: the resources a submission
    // is reading have to belong to *that* submission rather than to a list the
    // context drains whenever any fence retires.
    //
    // This is the shape that broke. Buffers were retained on the context, so
    // retiring the first frame released the second frame's geometry while the
    // GPU was still reading it — which renders correctly right up until the
    // memory is reused, and is reported by nothing but the validation layer.
    let mut first_target = target(&mut ctx);
    let mut second_target = target(&mut ctx);

    let first = ctx
        .submit_batch_deferred(&mut first_target, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("first submission");
    let second = ctx
        .submit_batch_deferred(&mut second_target, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("second submission");

    // Retired in order, with the second still in flight when the first goes.
    assert!(first.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(first);
    assert!(second.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(second);

    // Both frames drew what they were asked to, which is what says the geometry
    // survived long enough to be read.
    for tex in [&mut first_target, &mut second_target] {
        let pixels = ctx.read_texture(tex).expect("readback");
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }
    ctx.destroy_texture(first_target);
    ctx.destroy_texture(second_target);
    assert_clean(&ctx);
}

#[test]
fn frames_may_be_retired_out_of_the_order_they_were_submitted() {
    let Some(mut ctx) = context() else { return };
    let batch = scene();
    // Nothing requires a caller to retire in submission order, and a target
    // holding a slot per frame retires whichever slot comes round next. This
    // pins that the API allows it and stays clean.
    //
    // It is the weaker of the two: run against the shared release list this
    // replaced, it passed, because by the time the newer frame completes the
    // older one has as well. The test above is the one that reproduces that
    // defect, and this one is here because the ordering it permits is worth
    // stating rather than because it catches anything on its own.
    let mut first_target = target(&mut ctx);
    let mut second_target = target(&mut ctx);

    let first = ctx
        .submit_batch_deferred(&mut first_target, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("first submission");
    let second = ctx
        .submit_batch_deferred(&mut second_target, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("second submission");

    assert!(second.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(second);
    assert!(first.wait(FRAME_WAIT_TIMEOUT).expect("wait"));
    ctx.retire_fence(first);

    for tex in [&mut first_target, &mut second_target] {
        let pixels = ctx.read_texture(tex).expect("readback");
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }
    ctx.destroy_texture(first_target);
    ctx.destroy_texture(second_target);
    assert_clean(&ctx);
}

#[test]
fn an_exported_sync_file_actually_signals() {
    let Some(mut ctx) = context() else { return };
    // Everything else here checks that a sync_file can be exported and that it
    // outlives the fence it came from. Nothing checked the property the whole
    // export exists for: that it *signals* when the work completes.
    //
    // That gap matters because a display commit carrying one waits on it. A
    // sync_file that never signals produces a flip that never lands — a frame
    // loop that stops, with nothing in the rendered output to say why.
    let batch = scene();
    let mut tex = target(&mut ctx);
    let fence = ctx
        .submit_batch_deferred(&mut tex, &batch, PassDescriptor::clear([0.0; 4]))
        .expect("deferred submission");

    let Ok(fd) = fence.export_sync_file() else {
        eprintln!("skipping: this device cannot export a sync_file");
        ctx.retire_fence(fence);
        ctx.destroy_texture(tex);
        return;
    };

    // A signaled sync_file becomes readable. Polling is how anything else
    // waiting on one finds out, including the kernel when it latches a flip.
    let borrowed = std::os::fd::AsFd::as_fd(&fd);
    let mut fds = [rustix::event::PollFd::new(
        &borrowed,
        rustix::event::PollFlags::IN,
    )];
    let ready = rustix::event::poll(
        &mut fds,
        Some(&rustix::event::Timespec {
            tv_sec: 2,
            tv_nsec: 0,
        }),
    )
    .expect("poll");
    assert!(
        ready > 0,
        "the exported sync_file never signaled, so anything waiting on it \
         — a display commit above all — would wait forever"
    );

    ctx.retire_fence(fence);
    ctx.destroy_texture(tex);
    assert_clean(&ctx);
}

//! Correcting a texture's tracked layout after writing it outside emblema.
//!
//! # What this is for
//!
//! `VulkanTexture` tracks the layout its image is in, and emblema's next transition names that value
//! as `oldLayout`. A texture written by work emblema did not submit -- a raw `ash` pass on its own
//! device, which is exactly what `tessella_emblema` does -- keeps whatever emblema last recorded. For
//! a texture emblema created and nothing else touched, that is `UNDEFINED`.
//!
//! `oldLayout = UNDEFINED` tells the driver the contents do not matter. It is allowed to throw them
//! away, and on a tiler it does, because the barrier is where it decides whether to load the tile
//! from memory at all.
//!
//! # Where this test can fail
//!
//! **On a tiler.** On llvmpipe and most immediate-mode desktop drivers there is nothing to discard and
//! the bug is invisible: the test passes with and without the fix. It is written to be run on a
//! Raspberry Pi 5's v3d -- see `docs/on-a-board.md` -- and the control below is what says which
//! situation the run is in, rather than leaving a green result to be misread as a proof.

use ash::vk;
use emblema_hal::{Extent2D, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::{DevicePreference, Validated};

/// Fills `image` with `color` using raw `ash` on emblema's own device, and returns the layout the
/// image is left in.
///
/// Deliberately not through any emblema call: the point is work emblema does not know about.
/// `cmd_clear_color_image` rather than a staging buffer and a copy, because the test needs known
/// contents in the image and nothing else -- a host-visible allocation would be sixty lines that
/// cannot fail in an interesting way.
fn fill_outside_emblema(ctx: &Validated, image: vk::Image, color: [f32; 4]) -> vk::ImageLayout {
    let device = ctx.raw_device();
    let family = ctx.queue_family_index();

    // SAFETY: every handle below comes from this device and is destroyed before returning; the
    // submission is waited on, so nothing outlives the command buffer that records it.
    unsafe {
        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                None,
            )
            .expect("command pool");
        let buffers = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .expect("command buffer");
        let cmd = buffers[0];

        device
            .begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .expect("begin");

        let whole = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);

        // Into a layout a clear can write. From UNDEFINED, which is honest -- nothing has written it
        // yet, so there is nothing for this barrier to preserve.
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .image(image)
                .subresource_range(whole)],
        );

        device.cmd_clear_color_image(
            cmd,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &vk::ClearColorValue { float32: color },
            &[whole],
        );

        device.end_command_buffer(cmd).expect("end");

        let recorded = [cmd];
        let submits = [vk::SubmitInfo::default().command_buffers(&recorded)];
        device
            .queue_submit(ctx.raw_queue(), &submits, vk::Fence::null())
            .expect("submit");
        // Waited on, so the fill is complete before anything is asserted about it. The contract on
        // `assume_layout` is that the outside work has been submitted; this test makes it stronger
        // than it needs to be so a failure cannot be a race.
        device.device_wait_idle().expect("idle");

        device.free_command_buffers(pool, &[cmd]);
        device.destroy_command_pool(pool, None);
    }

    // Where the fill left it, which is what emblema has to be told.
    vk::ImageLayout::TRANSFER_DST_OPTIMAL
}

/// What emblema does with the texture after the outside fill.
#[derive(Clone, Copy, PartialEq)]
enum Then {
    /// Read it straight back, which is a transfer.
    Read,
    /// Run an empty **preserving** render pass over it first, then read.
    ///
    /// This is where a tiler decides whether to load the tile from memory at all:
    /// `PassDescriptor::preserve` builds the pass with `LOAD_OP_LOAD`, and emblema transitions the
    /// target into `COLOR_ATTACHMENT_OPTIMAL` from whatever layout it has recorded. A transfer-based
    /// readback never makes that decision, which is why the `Read` case cannot provoke the bug on any
    /// driver measured so far.
    PreserveThenRead,
}

/// Fills a texture outside emblema and reads it back, choosing what emblema does in between and
/// whether the layout is corrected first.
fn fill_then(ctx: &mut Validated, correct_the_layout: bool, then: Then) -> Vec<u8> {
    let extent = Extent2D::new(32, 32);
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            extent,
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture creation");

    // Blue, with a distinct alpha: a channel-order mistake cannot look like a discard.
    let left_in = fill_outside_emblema(ctx, tex.raw_image(), [0.0, 0.0, 1.0, 1.0]);
    if correct_the_layout {
        tex.assume_layout(left_in);
    }

    if then == Then::PreserveThenRead {
        // An empty batch, so nothing is drawn and the only thing under test is whether the fill
        // survived the pass. A preserving pass that drew would confuse "the tile was loaded" with
        // "something was painted over it".
        let batch = emblema_hal::Batch::new();
        ctx.submit_batch(&mut tex, &batch, emblema_hal::PassDescriptor::preserve())
            .expect("a preserving pass");
    }

    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    pixels
}

/// Every device this machine has, by enumeration index.
///
/// By index rather than `Auto`, and not because an index is a good way to name a device -- it is the
/// right way to *enumerate* one. A tiler is the only place this test can fail, and on a Pi 5 `Auto`
/// may well answer llvmpipe: v3d and llvmpipe are `OTHER` and `CPU`, so neither outranks the other by
/// device type and the winner is whichever came first. Walking them all removes the question.
fn every_device() -> Vec<(usize, Validated)> {
    let mut found = Vec::new();
    for index in 0..8 {
        match Validated::new(DevicePreference::Index(index)) {
            Ok(ctx) => found.push((index, ctx)),
            Err(_) => break,
        }
    }
    if found.is_empty() {
        eprintln!("skipping: no usable Vulkan device");
    }
    found
}

/// A texture written outside emblema reads back what was written, once the layout is corrected.
///
/// The assertion that matters, and it must hold on **every** device. `read_texture` transitions from
/// the layout emblema has recorded, so without the correction that is `UNDEFINED` and a tiler is free
/// to discard the fill.
#[test]
fn a_texture_written_outside_emblema_survives_a_corrected_layout() {
    for (index, mut ctx) in every_device() {
        let name = ctx.capabilities().device_name.clone();
        for then in [Then::Read, Then::PreserveThenRead] {
            let what = match then {
                Then::Read => "read",
                Then::PreserveThenRead => "preserving pass, then read",
            };
            let pixels = fill_then(&mut ctx, true, then);
            assert_eq!(pixels.len(), 32 * 32 * 4, "device {index} ({name})");
            // Every pixel, because a discard that happened to spare one tile would pass a spot check.
            for (at, px) in pixels.chunks_exact(4).enumerate() {
                assert_eq!(
                    px,
                    [0, 0, 255, 255],
                    "device {index} ({name}), {what}, pixel {at} at ({}, {}) -- the fill did not \
                     survive the transition",
                    at % 32,
                    at / 32
                );
            }
            eprintln!("device {index} ({name}), {what}: the fill survived, as it must");
        }
    }
}

/// What the same run does *without* the correction, per device, reported rather than asserted.
///
/// This cannot be an assertion in either direction. On a tiler the fill is discarded and the pixels
/// come back cleared; on llvmpipe or RADV they come back intact, because there was never a tile to
/// drop and `UNDEFINED` cost nothing. Asserting the first fails on every desktop; asserting the second
/// fails on the hardware this is for.
///
/// So it prints which happened, per device, and those lines are what tell a reader whether the test
/// above proved anything. A green suite on a desktop proves the API exists. A green suite with a line
/// here saying the fill was discarded proves the API is load-bearing -- and that line is the whole
/// reason this test is run on a board.
#[test]
fn without_the_correction_the_outcome_is_the_drivers_choice() {
    let mut discarded_somewhere = false;
    for (index, mut ctx) in every_device() {
        let name = ctx.capabilities().device_name.clone();
        for then in [Then::Read, Then::PreserveThenRead] {
            let what = match then {
                Then::Read => "read",
                Then::PreserveThenRead => "preserving pass, then read",
            };
            let pixels = fill_then(&mut ctx, false, then);
            let intact = pixels.chunks_exact(4).all(|px| px == [0, 0, 255, 255]);
            if intact {
                eprintln!(
                    "device {index} ({name}), {what}: the fill survived an UNDEFINED oldLayout -- \
                     this path does not exercise the fix here"
                );
            } else {
                discarded_somewhere = true;
                eprintln!(
                    "device {index} ({name}), {what}: the fill was DISCARDED without assume_layout \
                     (first pixel {:?}) -- this is the case the fix is for",
                    &pixels[..4]
                );
            }
        }
    }
    if !discarded_somewhere {
        eprintln!(
            "no device here discards on UNDEFINED, so this run does not demonstrate the bug; see \
             docs/on-a-board.md for a tiler that does"
        );
    }
}

//! The blocks device memory is taken in.
//!
//! What a context holds after its first texture is the size of a block, not
//! of the texture: the measure here is what was taken from the driver. A
//! version of the allocator dropped the sizes it was given and made 256 MiB
//! and 64 MiB blocks whatever was asked for, which nothing noticed until a
//! driver charged for every page of them.

use emblema_hal::{Extent2D, MemoryBlocks, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::{ContextConfig, VulkanContext};

const MIB: u64 = 1 << 20;

fn context(blocks: Option<MemoryBlocks>) -> Option<VulkanContext> {
    let config = ContextConfig {
        validation: true,
        ..Default::default()
    };
    let made = match blocks {
        Some(blocks) => VulkanContext::with_memory(config, blocks),
        None => VulkanContext::with_config(config),
    };
    match made {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

/// A texture made and written: one device allocation, one host one.
fn reserved_after_a_write(ctx: &mut VulkanContext) -> u64 {
    assert_eq!(ctx.memory_reserved(), 0, "nothing is taken ahead of use");
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(64, 64),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    ctx.write_texture(&mut tex, &[0x7f; 64 * 64 * 4])
        .expect("write");
    let reserved = ctx.memory_reserved();
    assert_eq!(
        ctx.read_texture(&mut tex).expect("readback"),
        vec![0x7f; 64 * 64 * 4]
    );
    ctx.destroy_texture(tex);
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
    reserved
}

#[test]
fn a_context_takes_memory_in_the_blocks_it_was_given() {
    let Some(mut ctx) = context(Some(MemoryBlocks {
        device: 16 * MIB,
        host: 4 * MIB,
    })) else {
        return;
    };
    let reserved = reserved_after_a_write(&mut ctx);
    // A device where every memory type is host-visible takes both from host
    // blocks; either way nothing near the default's 320 MiB.
    assert!(
        (4 * MIB..=20 * MIB).contains(&reserved),
        "{} MiB reserved",
        reserved / MIB
    );
}

#[test]
fn the_default_blocks_are_as_they_were() {
    assert_eq!(
        MemoryBlocks::default(),
        MemoryBlocks {
            device: 256 * MIB,
            host: 64 * MIB
        }
    );
    let Some(mut ctx) = context(None) else {
        return;
    };
    let reserved = reserved_after_a_write(&mut ctx);
    assert!(
        (64 * MIB..=320 * MIB).contains(&reserved),
        "{} MiB reserved",
        reserved / MIB
    );
}

#[test]
fn a_block_size_out_of_range_is_held_to_it() {
    let Some(mut ctx) = context(Some(MemoryBlocks { device: 1, host: 1 })) else {
        return;
    };
    let reserved = reserved_after_a_write(&mut ctx);
    assert!(
        (4 * MIB..=8 * MIB).contains(&reserved),
        "{} MiB reserved",
        reserved / MIB
    );
}

/// A texture larger than a host block and smaller than a device one is placed
/// in a device block, whatever kinds of memory the device has. With one kind,
/// all of it mapped by the host, the allocator sizes every block as a host
/// one, and the texture had an allocation of its own: 8 MiB reserved here,
/// and on a board a frame that waited for it.
#[test]
fn a_texture_is_placed_in_a_device_block_on_any_device() {
    let Some(mut ctx) = context(Some(MemoryBlocks {
        device: 32 * MIB,
        host: 4 * MIB,
    })) else {
        return;
    };
    let tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(2048, 1024),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    assert_eq!(ctx.memory_reserved() / MIB, 32);
    ctx.destroy_texture(tex);
}

/// What is in use and the most that has been, by kind: a texture is the
/// device's, the staging its write goes through the host's, and a peak
/// outlasts what made it.
#[test]
fn what_is_in_use_and_its_peak_are_counted_by_kind() {
    let Some(mut ctx) = context(None) else {
        return;
    };
    // A context holds something of its own by now on some paths; what a
    // texture adds is what is measured.
    let before = ctx.memory_used();
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(256, 256),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    let held = ctx.memory_used();
    let texture = held.device - before.device;
    assert!(texture >= 256 * 256 * 4, "{texture} bytes for the texture");
    assert_eq!(held.host, before.host, "a texture is not the host's");

    ctx.write_texture(&mut tex, &vec![0x7f; 256 * 256 * 4])
        .expect("write");
    // The staging is given back when the write returns, and was counted.
    assert_eq!(ctx.memory_used(), held);
    let peak = ctx.memory_peak();
    assert!(
        peak.host >= before.host + 256 * 256 * 4,
        "{} bytes of host memory at the most",
        peak.host
    );
    assert!(peak.device >= held.device);

    ctx.destroy_texture(tex);
    assert_eq!(ctx.memory_used(), before);
    assert_eq!(ctx.memory_peak(), peak, "a peak is not lowered by a free");
    assert!(ctx.memory_reserved() >= ctx.memory_used().device);
}

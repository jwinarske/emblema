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

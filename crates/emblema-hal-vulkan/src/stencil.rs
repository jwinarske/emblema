//! The transient stencil attachment clipping is built on.
//!
//! Allocated per submission rather than cached, matching the multisample
//! buffer: both are written during a pass and never read again, and both will
//! move into the frame's resource pool once one exists. Caching either now
//! would mean writing invalidation rules before anything needs them.
//!
//! # Why not a `VulkanTexture`
//!
//! Every other image in this backend goes through `create_texture`, which is
//! keyed by [`emblema_hal::PixelFormat`]. A stencil format is not one of
//! those and should not become one: `PixelFormat` describes what a caller can
//! render into and read back, and nothing above the HAL can do either with a
//! stencil buffer. Widening it to carry a format only this file allocates would
//! put a variant every backend must handle into the public surface.

use crate::device::VulkanContext;
use crate::render::sample_flags;
use crate::resource::{backend_err, WithDetail};
use ash::vk;
use emblema_hal::{Error, Extent2D, Result};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

/// A stencil attachment and the memory behind it.
pub struct StencilBuffer {
    pub image: vk::Image,
    pub view: vk::ImageView,
    allocation: Option<Allocation>,
}

impl StencilBuffer {
    /// Release the image, its view, and its memory.
    ///
    /// Explicit rather than a `Drop` implementation because freeing needs the
    /// device and the allocator, and threading those into a destructor would
    /// mean storing a handle to the context inside every buffer.
    pub fn destroy(mut self, ctx: &mut VulkanContext) {
        let device = ctx.raw_device().clone();
        // SAFETY: the caller has given up the buffer, and the submission that
        // used it was waited on before this runs.
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(allocation) = self.allocation.take() {
            let _ = ctx.allocator_mut().free(allocation);
        }
    }
}

/// A stencil format this device can use as a depth/stencil attachment.
///
/// `S8_UINT` first because it carries no depth component and so costs the least
/// memory and bandwidth, but it is optional and several drivers do not offer
/// it. The two combined formats after it are the ones the specification
/// requires at least one of, so this cannot come back empty on a conformant
/// device.
pub fn stencil_format(ctx: &VulkanContext) -> Result<vk::Format> {
    let candidates = [
        vk::Format::S8_UINT,
        vk::Format::D24_UNORM_S8_UINT,
        vk::Format::D32_SFLOAT_S8_UINT,
    ];
    for format in candidates {
        let properties = unsafe {
            ctx.raw_instance()
                .get_physical_device_format_properties(ctx.raw_physical_device(), format)
        };
        if properties
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
        {
            return Ok(format);
        }
    }
    Err(Error::Unsupported(
        "no stencil attachment format; this device cannot clip by a path",
    ))
}

/// Allocate a stencil attachment matching a target.
///
/// The sample count must match the color attachment it is used beside, which is
/// what makes a clip edge antialiase along with the shape it confines: with
/// multisampling the stencil is per sample, so a clip boundary crossing a pixel
/// admits some of its samples and not others.
pub fn create(
    ctx: &mut VulkanContext,
    extent: Extent2D,
    samples: u32,
    format: vk::Format,
) -> Result<StencilBuffer> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(sample_flags(samples))
        .tiling(vk::ImageTiling::OPTIMAL)
        // Transient tells a tiler this never needs to reach main memory, which
        // is the whole point of discarding it at the end of the pass.
        .usage(
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT,
        )
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);

    let device = ctx.raw_device().clone();
    let image =
        unsafe { device.create_image(&info, None) }.map_err(|e| backend_err("create_image", e))?;

    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let allocation = match ctx.allocator_mut().allocate(&AllocationCreateDesc {
        name: "stencil",
        requirements,
        location: MemoryLocation::GpuOnly,
        linear: false,
        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
    }) {
        Ok(allocation) => allocation,
        Err(e) => {
            unsafe { device.destroy_image(image, None) };
            return Err(Error::OutOfMemory {
                what: "stencil memory",
            }
            .with_detail(e));
        }
    };

    if let Err(e) =
        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()) }
    {
        let _ = ctx.allocator_mut().free(allocation);
        unsafe { device.destroy_image(image, None) };
        return Err(backend_err("bind_image_memory", e));
    }

    // The view names only the stencil aspect even where the format carries a
    // depth component too, since nothing here reads or writes depth.
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect_mask(format))
                .level_count(1)
                .layer_count(1),
        );
    match unsafe { device.create_image_view(&view_info, None) } {
        Ok(view) => Ok(StencilBuffer {
            image,
            view,
            allocation: Some(allocation),
        }),
        Err(e) => {
            let _ = ctx.allocator_mut().free(allocation);
            unsafe { device.destroy_image(image, None) };
            Err(backend_err("create_image_view", e))
        }
    }
}

/// The aspects a format actually carries.
///
/// A combined format's view must name both aspects even though only the stencil
/// one is used; naming just the stencil aspect of a depth/stencil image is
/// invalid for an attachment view.
pub fn aspect_mask(format: vk::Format) -> vk::ImageAspectFlags {
    match format {
        vk::Format::S8_UINT => vk::ImageAspectFlags::STENCIL,
        _ => vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
    }
}

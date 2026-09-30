//! Exporting images as dma-bufs.
//!
//! This is the first of the two requirements the HAL reserved from day one, and
//! the point where it stops being a reservation. The DRM presentation path
//! needs images the display controller can scan out, which means an image whose
//! memory layout was agreed with the display side and whose memory can be
//! handed over as a file descriptor.
//!
//! Two things have to be true at once, and both are set at creation rather than
//! discovered later. The image must be created with an explicit format modifier
//! chosen from what the device offers, because an optimally-tiled image has a
//! layout only the GPU understands. And its memory must be allocated as
//! exportable, because a normal allocation cannot be turned into a dma-buf after
//! the fact.

use crate::device::VulkanContext;
use crate::resource::{backend_err, vk_format, TextureMemory, VulkanTexture};
use ash::vk;
use emblema_hal::{
    Error, Extent2D, FormatModifierSet, Modifier, PixelFormat, Result, TextureUsage,
};

/// Ask the device which layouts it can use for a format.
///
/// This is the render side's half of format negotiation. Without it the only
/// safe assumption is linear, which works everywhere and wastes bandwidth
/// everywhere.
pub fn query_format_modifiers(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: PixelFormat,
) -> Option<FormatModifierSet> {
    let fourcc = format.fourcc()?;

    // Two-pass query: the first call reports how many entries there are, the
    // second fills them in.
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the chained structure is initialized and outlives the call.
    unsafe {
        instance.get_physical_device_format_properties2(
            physical_device,
            vk_format(format),
            &mut properties,
        );
    }

    let count = list.drm_format_modifier_count as usize;
    if count == 0 {
        return None;
    }
    let mut entries = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
    list.p_drm_format_modifier_properties = entries.as_mut_ptr();
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the buffer is sized by the count the first call reported.
    unsafe {
        instance.get_physical_device_format_properties2(
            physical_device,
            vk_format(format),
            &mut properties,
        );
    }

    let modifiers: Vec<Modifier> = entries
        .iter()
        .take(count)
        // Only layouts that can actually be rendered into are useful here; the
        // device also reports ones it can merely sample from.
        .filter(|e| {
            e.drm_format_modifier_tiling_features
                .contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT)
        })
        .map(|e| Modifier(e.drm_format_modifier))
        .collect();

    if modifiers.is_empty() {
        return None;
    }
    Some(FormatModifierSet::new(fourcc, modifiers))
}

impl VulkanContext {
    /// Allocate an image that can be exported as a dma-buf.
    ///
    /// `modifiers` are the layouts the other side will accept, in preference
    /// order; the driver picks one it can also use. Passing the negotiated set
    /// rather than a single modifier is what lets the driver choose the best
    /// layout both sides share.
    pub fn create_exportable_texture(
        &mut self,
        extent: Extent2D,
        format: PixelFormat,
        modifiers: &[Modifier],
    ) -> Result<VulkanTexture> {
        if !self.capabilities().dma_buf.can_allocate_scanout() {
            return Err(Error::Unsupported(
                "exporting images requires dma-buf export and explicit modifiers",
            ));
        }
        if modifiers.is_empty() {
            return Err(Error::Unsupported("no candidate modifiers were offered"));
        }
        if !self.capabilities().can_allocate(extent) {
            return Err(Error::LimitExceeded {
                what: "texture dimension",
                requested: extent.width.max(extent.height) as u64,
                limit: self.capabilities().max_texture_size as u64,
            });
        }

        let device = self.raw_device().clone();
        let raw_modifiers: Vec<u64> = modifiers.iter().map(|m| m.0).collect();

        let mut modifier_list = vk::ImageDrmFormatModifierListCreateInfoEXT::default()
            .drm_format_modifiers(&raw_modifiers);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format(format))
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            // Not OPTIMAL: the layout is the negotiated modifier, which is the
            // whole point. An optimally-tiled image has a layout only this GPU
            // understands and cannot be shared.
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut modifier_list)
            .push_next(&mut external);

        // SAFETY: the chained structures outlive the call, and every object is
        // destroyed on the failure paths below.
        let image = unsafe { device.create_image(&info, None) }
            .map_err(|e| backend_err("create_image (exportable)", e))?;

        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let memory_type = match self.find_memory_type(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        ) {
            Some(index) => index,
            None => {
                unsafe { device.destroy_image(image, None) };
                return Err(Error::Backend {
                    backend: "vulkan",
                    detail: "no device-local memory type accepts an exportable image".into(),
                });
            }
        };

        // Dedicated rather than suballocated: a dma-buf hands over a whole
        // allocation, so an image sharing memory with others cannot be exported
        // without exporting them too.
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut dedicated)
            .push_next(&mut export);

        let memory = match unsafe { device.allocate_memory(&allocate, None) } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe { device.destroy_image(image, None) };
                return Err(backend_err("allocate_memory (exportable)", e));
            }
        };

        if let Err(e) = unsafe { device.bind_image_memory(image, memory, 0) } {
            unsafe {
                device.free_memory(memory, None);
                device.destroy_image(image, None);
            }
            return Err(backend_err("bind_image_memory (exportable)", e));
        }

        Ok(VulkanTexture {
            image,
            memory: TextureMemory::Dedicated(memory),
            extent,
            format,
            layout: std::cell::Cell::new(vk::ImageLayout::UNDEFINED),
            // An exportable image is scanned out, not sampled, and a modifier
            // negotiated with another device says nothing about a chain.
            mip_levels: 1,
            usage: TextureUsage {
                render_target: true,
                transfer: true,
                scanout: true,
                ..TextureUsage::default()
            },
        })
    }

    /// The layout the driver actually chose for an exportable image.
    pub fn texture_modifier(&self, texture: &VulkanTexture) -> Result<Modifier> {
        if !texture.usage.scanout {
            return Err(Error::Unsupported(
                "only an exportable image has a negotiated modifier",
            ));
        }
        let loader = ash::ext::image_drm_format_modifier::Device::new(
            self.raw_instance(),
            self.raw_device(),
        );
        let mut properties = vk::ImageDrmFormatModifierPropertiesEXT::default();
        // SAFETY: the image was created with DRM_FORMAT_MODIFIER_EXT tiling,
        // which is what this query requires.
        unsafe {
            loader
                .get_image_drm_format_modifier_properties(texture.image, &mut properties)
                .map_err(|e| backend_err("get_image_drm_format_modifier_properties", e))?;
        }
        Ok(Modifier(properties.drm_format_modifier))
    }

    /// Export an image's memory as a dma-buf.
    ///
    /// The returned descriptor carries everything the display side needs to
    /// build a framebuffer: the file descriptor, the format, the layout the
    /// driver chose, and the plane's offset and stride.
    #[cfg(unix)]
    pub fn export_texture(
        &self,
        texture: &VulkanTexture,
    ) -> Result<emblema_hal::ExternalImageDesc> {
        use std::os::fd::FromRawFd;

        if !texture.usage.scanout {
            return Err(Error::Unsupported(
                "only an image created for export can be exported",
            ));
        }
        let fourcc = texture.format.fourcc().ok_or(Error::Unsupported(
            "this format has no scanout representation",
        ))?;
        let modifier = self.texture_modifier(texture)?;

        let memory = match &texture.memory {
            TextureMemory::Dedicated(memory) => *memory,
            TextureMemory::Pooled(_) => {
                return Err(Error::Unsupported(
                    "a suballocated image cannot be exported on its own",
                ))
            }
            // A swapchain image's memory belongs to the presentation engine,
            // which never offered a handle to it.
            TextureMemory::Borrowed => {
                return Err(Error::Unsupported(
                    "an image this backend did not allocate cannot be exported",
                ))
            }
        };

        let loader =
            ash::khr::external_memory_fd::Device::new(self.raw_instance(), self.raw_device());
        let info = vk::MemoryGetFdInfoKHR::default()
            .memory(memory)
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        // SAFETY: the memory was allocated with a matching export handle type.
        let raw =
            unsafe { loader.get_memory_fd(&info) }.map_err(|e| backend_err("get_memory_fd", e))?;

        // Ownership transfers to the caller here: the driver hands over a new
        // descriptor, and closing it is the importer's job.
        // SAFETY: the driver returned a fresh, owned descriptor.
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };

        let subresource =
            vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT);
        // SAFETY: a modifier image's plane layout is queried this way.
        let layout = unsafe {
            self.raw_device()
                .get_image_subresource_layout(texture.image, subresource)
        };

        Ok(emblema_hal::ExternalImageDesc {
            planes: vec![emblema_hal::DmaBufPlane {
                fd,
                offset: layout.offset as u32,
                stride: layout.row_pitch as u32,
            }],
            fourcc,
            modifier,
        })
    }

    fn find_memory_type(&self, type_bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        // SAFETY: the physical device outlives this context.
        let properties = unsafe {
            self.raw_instance()
                .get_physical_device_memory_properties(self.raw_physical_device())
        };
        (0..properties.memory_type_count).find(|i| {
            let usable = type_bits & (1 << i) != 0;
            usable
                && properties.memory_types[*i as usize]
                    .property_flags
                    .contains(flags)
        })
    }
}

/// The formats this device can render into and export, for negotiation.
pub fn render_formats(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Vec<FormatModifierSet> {
    [
        PixelFormat::Bgra8Unorm,
        PixelFormat::Rgba8Unorm,
        PixelFormat::Rgb10A2Unorm,
    ]
    .into_iter()
    .filter_map(|format| query_format_modifiers(instance, physical_device, format))
    .collect()
}

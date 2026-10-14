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
/// Whether this format, at this modifier, can be exported as a dma-buf.
///
/// Both halves of the answer, because four devices between them need both and
/// a check of either one alone is wrong on two of them. Measured 2026-10-08,
/// at the usage [`create_exportable_texture`] asks for:
///
/// | device | features | compatible handle types | |
/// |---|---|---|---|
/// | RADV | `EXPORTABLE \| IMPORTABLE` | `OPAQUE_FD \| DMA_BUF` | keep |
/// | V3D | `EXPORTABLE \| IMPORTABLE` | `OPAQUE_FD \| DMA_BUF` | keep |
/// | llvmpipe, Mesa 26 | `EXPORTABLE \| IMPORTABLE` | `OPAQUE_FD \| DMA_BUF` | keep |
/// | llvmpipe, Mesa 19 | **`IMPORTABLE`** | `DMA_BUF` | drop |
/// | Vivante GC7000UL | `EXPORTABLE \| IMPORTABLE` | **`OPAQUE_FD`** | drop |
///
/// The last two are why. An older llvmpipe lists dma-buf as compatible and
/// cannot export at all; a Vivante says it can export and lists a different
/// handle type. `exportFromImportedHandleTypes` is a third field and answers a
/// third question -- it is `DMA_BUF` on the Vivante -- so it is not the one to
/// read.
///
/// The usage matters and is the one the allocation uses: a device may export a
/// layout it can sample and refuse the same layout as a color attachment.
///
/// [`create_exportable_texture`]: crate::VulkanContext::create_exportable_texture
fn exports_as_dma_buf(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: PixelFormat,
    modifier: u64,
) -> bool {
    let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(vk_format(format))
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
        .push_next(&mut external)
        .push_next(&mut modifier_info);

    let mut external_properties = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external_properties);
    // SAFETY: every chained structure outlives the call.
    let answered = unsafe {
        instance.get_physical_device_image_format_properties2(
            physical_device,
            &info,
            &mut properties,
        )
    }
    .is_ok();

    let external = external_properties.external_memory_properties;
    answered
        && external
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE)
        && external
            .compatible_handle_types
            .contains(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
}

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
        // And only ones the device will hand out as a dma-buf. Rendering into
        // a layout and exporting it are separate questions; this asked only
        // the first, so a device that advertised a modifier and then refused
        // the handle type got an image built anyway.
        .filter(|e| exports_as_dma_buf(instance, physical_device, format, e.drm_format_modifier))
        .map(|e| Modifier(e.drm_format_modifier))
        .collect();

    if modifiers.is_empty() {
        return None;
    }
    Some(FormatModifierSet::new(fourcc, modifiers))
}

/// How many memory planes a format has at a given modifier.
///
/// Not a property of the format alone: AMD's DCC modifiers put the metadata in
/// a second plane, so the same `ARGB8888` is one plane linear and two tiled.
/// `VkDrmFormatModifierPropertiesEXT` carries this and the modifier query above
/// discarded it, which is why export described only plane zero for as long as it
/// did.
fn plane_count(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: PixelFormat,
    modifier: Modifier,
) -> Option<u32> {
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the physical device outlives the call, and the chain is a
    // properties query with no preconditions.
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
    entries
        .iter()
        .take(count)
        .find(|e| e.drm_format_modifier == modifier.0)
        .map(|e| e.drm_format_modifier_plane_count)
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

    /// Import a dma-buf as an image the renderer can draw into.
    ///
    /// The mirror of [`Self::create_exportable_texture`], and the half the DRM
    /// path needs where this device cannot allocate its own scanout buffers:
    /// GBM allocates, this imports, and the renderer draws into something it
    /// did not make.
    ///
    /// Three things differ from allocating. The modifier is *explicit* rather
    /// than a list, because the buffer already has a layout and the driver is
    /// being told it rather than asked. The memory is imported rather than
    /// allocated, so the memory type comes from what the descriptor itself
    /// permits intersected with what the image requires, not from a property
    /// search. And the allocation is dedicated for the same reason the export
    /// side is: a dma-buf is a whole allocation.
    #[cfg(unix)]
    pub fn import_texture(
        &mut self,
        mut image: emblema_hal::ExternalImageDesc,
        extent: Extent2D,
        format: PixelFormat,
    ) -> Result<VulkanTexture> {
        use std::os::fd::IntoRawFd as _;

        if !self.capabilities().dma_buf.import {
            return Err(Error::Unsupported("dma-buf import"));
        }
        // A linear buffer can be imported without the modifier extension, by
        // creating the image with LINEAR tiling instead of stating a layout.
        // That matters more than it looks: the devices that actually need this
        // path are the ones that cannot allocate scanout buffers, and on a
        // Radxa Zero 3 that device is a lavapipe with no
        // `VK_EXT_image_drm_format_modifier` at all -- so requiring the
        // extension made the GBM path unreachable on the hardware it exists
        // for. Measured there.
        //
        // The catch is that plain LINEAR cannot *state* a stride: Vulkan
        // computes its own row pitch, and if the allocator chose a different
        // one the import is silently wrong. So the layout is read back below
        // and the import refused where it disagrees.
        let plain_linear = !self.capabilities().dma_buf.modifiers;
        if plain_linear && image.modifier != Modifier::LINEAR {
            return Err(Error::Unsupported(
                "importing a tiled dma-buf requires explicit format modifiers",
            ));
        }
        if image.modifier == Modifier::INVALID {
            // A buffer whose layout nobody stated cannot be imported with an
            // explicit modifier, and guessing is how an importer ends up
            // reading tiled memory as linear.
            return Err(Error::Unsupported(
                "importing a dma-buf whose modifier is INVALID",
            ));
        }
        // Copied out rather than borrowed, because the descriptors are released
        // below and that needs `image.planes` by value.
        let Some((first_offset, first_stride)) = image.planes.first().map(|p| (p.offset, p.stride))
        else {
            return Err(Error::Unsupported("importing a dma-buf with no planes"));
        };
        // Every plane the modifier declares must be described, and the count
        // has to match: Vulkan refuses an explicit modifier whose plane count
        // disagrees with the layouts handed in, which is where this used to
        // fail against this device's own export.
        let declared = plane_count(
            self.raw_instance(),
            self.raw_physical_device(),
            format,
            image.modifier,
        )
        .unwrap_or(1)
        .max(1) as usize;
        if image.planes.len() != declared {
            return Err(Error::Backend {
                backend: "vulkan",
                detail: format!(
                    "this modifier declares {declared} memory plane(s) and {} were described",
                    image.planes.len()
                ),
            });
        }
        if !self.capabilities().can_allocate(extent) {
            return Err(Error::LimitExceeded {
                what: "texture dimension",
                requested: extent.width.max(extent.height) as u64,
                limit: self.capabilities().max_texture_size as u64,
            });
        }

        let device = self.raw_device().clone();
        let layouts: Vec<vk::SubresourceLayout> = image
            .planes
            .iter()
            .map(|p| {
                vk::SubresourceLayout::default()
                    .offset(u64::from(p.offset))
                    .row_pitch(u64::from(p.stride))
            })
            .collect();

        // Ownership of every descriptor is released here, before anything is
        // created, rather than after a successful import.
        //
        // The specification says a failed import does not transfer ownership,
        // so the obvious shape is to keep the `OwnedFd`s and release them once
        // `vkAllocateMemory` has succeeded. That aborts in practice. Measured on
        // a Radxa Zero 3's lavapipe: a *failed* import left the descriptor
        // closed, and dropping the `OwnedFd` on the error path killed the
        // process with "IO Safety violation: owned file descriptor already
        // closed". Leaking one descriptor down a path that is already returning
        // an error is the lesser bug, and it cannot abort.
        let descriptors: Vec<std::os::fd::RawFd> = std::mem::take(&mut image.planes)
            .into_iter()
            .map(|p| p.fd.into_raw_fd())
            .collect();
        let Some(first) = descriptors.first().copied() else {
            return Err(Error::Unsupported("importing a dma-buf with no planes"));
        };
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(image.modifier.0)
            .plane_layouts(&layouts);
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
            .tiling(if plain_linear {
                vk::ImageTiling::LINEAR
            } else {
                vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT
            })
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        let info = if plain_linear {
            info
        } else {
            info.push_next(&mut explicit)
        };

        // SAFETY: the chained structures outlive the call, and every object is
        // destroyed on the failure paths below.
        let vk_image = unsafe { device.create_image(&info, None) }
            .map_err(|e| backend_err("create_image (imported)", e))?;

        // On the plain-LINEAR path the stride was never stated, so check that
        // the one Vulkan chose is the one the buffer actually has. A disagreement
        // here is not a near miss: every row after the first would be read from
        // the wrong offset, and the picture would look like a shear rather than
        // like an error.
        if plain_linear {
            let subresource =
                vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::COLOR);
            // SAFETY: a LINEAR image's layout is queried this way.
            let chosen = unsafe { device.get_image_subresource_layout(vk_image, subresource) };
            if chosen.row_pitch != u64::from(first_stride) || first_offset != 0 {
                unsafe { device.destroy_image(vk_image, None) };
                return Err(Error::Backend {
                    backend: "vulkan",
                    detail: format!(
                        "a linear import cannot state a layout, and this device would read \
                         the buffer at stride {} offset 0 where it was allocated at stride {} \
                         offset {}",
                        chosen.row_pitch, first_stride, first_offset
                    ),
                });
            }
        }

        // What the descriptor permits, rather than a property search: an
        // imported allocation's memory type is decided by the buffer, and
        // asking for DEVICE_LOCAL here would be asking the wrong question.
        let loader = ash::khr::external_memory_fd::Device::new(self.raw_instance(), &device);
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        // SAFETY: the descriptor is open and is a dma-buf, which is the handle
        // type named.
        if let Err(e) = unsafe {
            loader.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                first,
                &mut fd_properties,
            )
        } {
            unsafe { device.destroy_image(vk_image, None) };
            return Err(backend_err("get_memory_fd_properties", e));
        }
        let requirements = unsafe { device.get_image_memory_requirements(vk_image) };
        let permitted = requirements.memory_type_bits & fd_properties.memory_type_bits;
        let Some(memory_type) = (0..32).find(|i| permitted & (1 << i) != 0) else {
            unsafe { device.destroy_image(vk_image, None) };
            return Err(Error::Backend {
                backend: "vulkan",
                detail: "no memory type is accepted by both the image and the dma-buf".into(),
            });
        };

        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(vk_image);
        // Vulkan takes the descriptor on success and leaves it alone on
        // failure, so it is handed over by raw value here and released from
        // `OwnedFd` only once the call has succeeded.
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(first);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut dedicated)
            .push_next(&mut import);

        let memory = match unsafe { device.allocate_memory(&allocate, None) } {
            Ok(memory) => memory,
            Err(e) => {
                unsafe { device.destroy_image(vk_image, None) };
                return Err(backend_err("allocate_memory (imported)", e));
            }
        };
        if let Err(e) = unsafe { device.bind_image_memory(vk_image, memory, 0) } {
            unsafe {
                device.free_memory(memory, None);
                device.destroy_image(vk_image, None);
            }
            return Err(backend_err("bind_image_memory (imported)", e));
        }

        Ok(VulkanTexture {
            image: vk_image,
            memory: TextureMemory::Dedicated(memory),
            extent,
            format,
            layout: std::cell::Cell::new(vk::ImageLayout::UNDEFINED),
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

        // Every plane the modifier declares, not just the first. A DCC
        // modifier puts its metadata in a second plane, and describing only
        // plane zero hands over half a layout -- an importer then refuses with
        // ERROR_INVALID_DRM_FORMAT_MODIFIER_PLANE_LAYOUT_EXT, and a display
        // told the same thing would scan out the wrong bytes.
        let planes = plane_count(
            self.raw_instance(),
            self.raw_physical_device(),
            texture.format,
            modifier,
        )
        .unwrap_or(1)
        .max(1);
        let aspects = [
            vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
            vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
            vk::ImageAspectFlags::MEMORY_PLANE_2_EXT,
            vk::ImageAspectFlags::MEMORY_PLANE_3_EXT,
        ];
        let Some(aspects) = aspects.get(..planes as usize) else {
            return Err(Error::Backend {
                backend: "vulkan",
                detail: format!(
                    "this modifier declares {planes} memory planes, past the four \
                                 a dma-buf describes"
                ),
            });
        };

        let mut described = Vec::with_capacity(aspects.len());
        for (index, aspect) in aspects.iter().enumerate() {
            let subresource = vk::ImageSubresource::default().aspect_mask(*aspect);
            // SAFETY: a modifier image's plane layout is queried this way, and
            // the aspect is one the modifier declares.
            let layout = unsafe {
                self.raw_device()
                    .get_image_subresource_layout(texture.image, subresource)
            };
            // One descriptor per plane, all onto the same allocation: the
            // planes live at offsets within one dma-buf, and an importer takes
            // ownership of each descriptor it is handed. The first is the one
            // the driver returned; the rest are duplicates of it.
            let fd = if index == 0 {
                fd.try_clone().map_err(|e| Error::Backend {
                    backend: "vulkan",
                    detail: format!("duplicating the exported descriptor: {e}"),
                })?
            } else {
                fd.try_clone().map_err(|e| Error::Backend {
                    backend: "vulkan",
                    detail: format!("duplicating the exported descriptor for plane {index}: {e}"),
                })?
            };
            described.push(emblema_hal::DmaBufPlane {
                fd,
                offset: layout.offset as u32,
                stride: layout.row_pitch as u32,
            });
        }

        Ok(emblema_hal::ExternalImageDesc {
            planes: described,
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

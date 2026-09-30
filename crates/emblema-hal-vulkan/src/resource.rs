//! Images, their memory, and getting pixels back to the CPU.
//!
//! Readback exists for the test apparatus rather than for the frame loop. The
//! entire golden and conformance corpus renders to an offscreen target and
//! compares the result, so this path is what turns "the device reported a
//! capability" into "the device produced these pixels".

use crate::device::VulkanContext;
use ash::vk;
use emblema_hal::{Error, Extent2D, PixelFormat, Result, TextureDescriptor, TextureUsage};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

/// An image, its backing memory, and the state needed to transition it.
///
/// Ownership is explicit: a texture must be handed back to
/// [`VulkanContext::destroy_texture`]. Automatic retirement belongs to the
/// fence waiter, which tracks GPU completion and cannot be built until there
/// is asynchronous work to track.
pub struct VulkanTexture {
    pub(crate) image: vk::Image,
    pub(crate) memory: TextureMemory,
    pub(crate) extent: Extent2D,
    pub(crate) format: PixelFormat,
    /// The layout the image is currently in, so the next operation knows what
    /// to transition from.
    /// The layout the image is currently in.
    ///
    /// Behind a cell because this tracks device-side state, not anything Rust
    /// aliasing rules are about: a texture being *sampled* is borrowed shared,
    /// and getting it into a readable layout is a transition the recorder has
    /// to make and record. Requiring a unique borrow for that would mean a
    /// batch could sample only one texture at a time.
    pub(crate) layout: std::cell::Cell<vk::ImageLayout>,
    pub(crate) usage: TextureUsage,
    /// How many mip levels the image was allocated with, the image included.
    pub(crate) mip_levels: u32,
}

/// Where a texture's memory came from.
///
/// The distinction matters only at export: a dma-buf hands over a whole
/// allocation, so an image sharing one with other resources cannot be exported
/// without exporting them too.
pub(crate) enum TextureMemory {
    /// Suballocated from a pool. The common case, and not exportable.
    Pooled(Allocation),
    /// A whole allocation of its own, which is what export requires.
    Dedicated(vk::DeviceMemory),
    /// Memory somebody else owns, and an image somebody else created.
    ///
    /// A swapchain's images are the case: the presentation engine allocates
    /// them, hands them out, and destroys them with the swapchain. Destroying
    /// one here would free a handle the engine still holds, so this variant
    /// exists to say "wrap it, do not own it" — a distinction the type system
    /// then keeps for us rather than leaving to a comment.
    Borrowed,
}

impl VulkanTexture {
    /// Wrap an image this backend did not create.
    ///
    /// The caller keeps ownership: destroying the returned texture releases
    /// nothing, which is what makes it safe to hand back a swapchain image the
    /// presentation engine still owns.
    pub fn wrap_image(
        image: vk::Image,
        extent: Extent2D,
        format: PixelFormat,
        layout: vk::ImageLayout,
    ) -> VulkanTexture {
        VulkanTexture {
            image,
            memory: TextureMemory::Borrowed,
            extent,
            format,
            layout: std::cell::Cell::new(layout),
            // A swapchain image is one level. Nothing here allocated it, so
            // nothing here may claim it holds more than it was handed.
            mip_levels: 1,
            usage: TextureUsage {
                render_target: true,
                sampled: false,
                ..TextureUsage::default()
            },
        }
    }

    pub fn extent(&self) -> Extent2D {
        self.extent
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn raw_image(&self) -> vk::Image {
        self.image
    }

    pub(crate) fn layout(&self) -> vk::ImageLayout {
        self.layout.get()
    }

    /// Record a layout change made by an operation outside this module.
    pub(crate) fn set_layout(&self, layout: vk::ImageLayout) {
        self.layout.set(layout);
    }

    /// Bytes a tightly packed readback of this texture occupies.
    pub fn byte_size(&self) -> u64 {
        self.extent.area() * self.format.bytes_per_pixel() as u64
    }
}

/// Map a portable format onto Vulkan's enumeration.
pub(crate) fn vk_format(format: PixelFormat) -> vk::Format {
    match format {
        PixelFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        PixelFormat::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        PixelFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        PixelFormat::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
        PixelFormat::Rgb10A2Unorm => vk::Format::A2B10G10R10_UNORM_PACK32,
        PixelFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        PixelFormat::R8Unorm => vk::Format::R8_UNORM,
    }
}

impl VulkanContext {
    /// Allocate a texture.
    ///
    /// External images are not handled yet: importing a dma-buf needs the
    /// external-memory plumbing, and accepting the descriptor while ignoring
    /// its external half would hand back a blank image that silently is not
    /// the caller's buffer.
    pub fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<VulkanTexture> {
        if desc.is_external() {
            return Err(Error::Unsupported("external image import"));
        }
        self.capabilities().check_texture(desc)?;
        if !self.capabilities().can_allocate(desc.extent) {
            return Err(Error::LimitExceeded {
                what: "texture dimension",
                requested: desc.extent.width.max(desc.extent.height) as u64,
                limit: self.capabilities().max_texture_size as u64,
            });
        }
        if desc.extent.is_empty() {
            return Err(Error::Unsupported("zero-sized texture"));
        }

        let mut usage = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
        if desc.usage.sampled {
            usage |= vk::ImageUsageFlags::SAMPLED;
        }
        if desc.usage.render_target {
            usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
        }

        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format(desc.format))
            .extent(vk::Extent3D {
                width: desc.extent.width,
                height: desc.extent.height,
                depth: 1,
            })
            .mip_levels(desc.mip_levels.max(1))
            .array_layers(1)
            .samples(sample_count_flags(desc.sample_count))
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let device = self.raw_device().clone();
        let image = unsafe { device.create_image(&info, None) }
            .map_err(|e| backend_err("create_image", e))?;

        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "texture",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_image(image, None) };
                Error::OutOfMemory {
                    what: "texture memory",
                }
                .with_detail(e)
            })?;

        unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()) }
            .map_err(|e| backend_err("bind_image_memory", e))?;

        Ok(VulkanTexture {
            image,
            memory: TextureMemory::Pooled(allocation),
            extent: desc.extent,
            format: desc.format,
            layout: std::cell::Cell::new(vk::ImageLayout::UNDEFINED),
            usage: desc.usage,
            mip_levels: desc.mip_levels.max(1),
        })
    }

    /// Release a texture and its memory.
    pub fn destroy_texture(&mut self, texture: VulkanTexture) {
        // Freeing has to match how the memory was obtained, or the pool is told
        // about an allocation it never made.
        match texture.memory {
            TextureMemory::Pooled(allocation) => {
                let _ = self.allocator_mut().free(allocation);
            }
            TextureMemory::Dedicated(memory) => {
                // SAFETY: nothing else holds this allocation, and every
                // submission using the image was waited on.
                unsafe { self.raw_device().free_memory(memory, None) };
            }
            // Nothing to free and nothing to destroy: whoever created the
            // image will do both.
            TextureMemory::Borrowed => return,
        }
        // SAFETY: the caller has given up the texture, and every submission
        // that used it was waited on before returning from the call that made
        // it.
        unsafe { self.raw_device().destroy_image(texture.image, None) };
    }

    /// Put a texture into the layout the presentation engine reads from.
    ///
    /// A presented image is read by something outside this device's command
    /// stream, so it has to be transitioned rather than left in whatever the
    /// last pass wanted. Doing it here keeps the layout bookkeeping with
    /// everything else that knows about layouts, instead of in a presentation
    /// target that would have to reach into a texture's internals.
    pub fn transition_for_present(&mut self, texture: &VulkanTexture) -> Result<()> {
        if texture.layout() == vk::ImageLayout::PRESENT_SRC_KHR {
            return Ok(());
        }
        let device = self.raw_device().clone();
        let cmd = self.begin_one_shot()?;
        transition(
            &device,
            cmd,
            texture.raw_image(),
            texture.layout(),
            vk::ImageLayout::PRESENT_SRC_KHR,
        );
        self.submit_one_shot(cmd)?;
        texture.set_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        Ok(())
    }

    /// Clear a texture to a solid color.
    ///
    /// Records, submits, and waits. Batching belongs to the renderer, which
    /// does not exist yet; doing it here would be an abstraction guessing at
    /// its caller.
    pub fn clear_texture(&mut self, texture: &mut VulkanTexture, color: [f32; 4]) -> Result<()> {
        let device = self.raw_device().clone();
        let cmd = self.begin_one_shot()?;

        transition(
            &device,
            cmd,
            texture.image,
            texture.layout.get(),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );

        let clear = vk::ClearColorValue { float32: color };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(vk::REMAINING_MIP_LEVELS)
            .layer_count(1);
        unsafe {
            device.cmd_clear_color_image(
                cmd,
                texture.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear,
                &[range],
            );
        }
        texture.layout.set(vk::ImageLayout::TRANSFER_DST_OPTIMAL);

        self.submit_one_shot(cmd)
    }

    /// Copy a texture back to host memory, tightly packed.
    /// Fill a texture from host memory, tightly packed and top row first.
    ///
    /// The mirror of [`Self::read_texture`], and stated in the same layout, so
    /// a round trip through the pair is the identity. That is what makes it
    /// testable without a decoder: write known bytes, read them back, compare.
    ///
    /// Image *decoding* is out of scope for this project, but getting decoded
    /// pixels onto the device is not — without this an image shader would have
    /// nothing to sample but what the renderer itself drew.
    pub fn write_texture(&mut self, texture: &mut VulkanTexture, pixels: &[u8]) -> Result<()> {
        let size = texture.byte_size();
        if pixels.len() as u64 != size {
            return Err(Error::Backend {
                backend: "vulkan",
                detail: format!("texture wants {size} bytes, {} supplied", pixels.len()),
            });
        }
        let device = self.raw_device().clone();

        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&buffer_info, None) }
            .map_err(|e| backend_err("create_buffer", e))?;
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let mut allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "upload",
                requirements,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_buffer(buffer, None) };
                Error::OutOfMemory {
                    what: "upload buffer",
                }
                .with_detail(e)
            })?;
        if let Err(e) =
            unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
        {
            let _ = self.allocator_mut().free(allocation);
            unsafe { device.destroy_buffer(buffer, None) };
            return Err(backend_err("bind_buffer_memory", e));
        }

        let staged = match allocation.mapped_slice_mut() {
            Some(slice) => {
                slice[..pixels.len()].copy_from_slice(pixels);
                Ok(())
            }
            None => Err(Error::Backend {
                backend: "vulkan",
                detail: "upload allocation was not host-visible".into(),
            }),
        };

        let outcome = staged.and_then(|()| {
            let cmd = self.begin_one_shot()?;
            transition(
                &device,
                cmd,
                texture.image,
                texture.layout.get(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: texture.extent.width,
                    height: texture.extent.height,
                    depth: 1,
                });
            // SAFETY: the command buffer is recording, the buffer holds the
            // staged pixels, and the image is in the layout named here.
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    buffer,
                    texture.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            texture.layout.set(vk::ImageLayout::TRANSFER_DST_OPTIMAL);
            // In the same submission as the copy that fed it, so the chain is
            // never a submission behind the level it was built from.
            generate_mipmaps(&device, cmd, texture);
            self.submit_one_shot(cmd)
        });

        let _ = self.allocator_mut().free(allocation);
        // SAFETY: the submission above was waited on, so the copy has finished
        // reading from this buffer.
        unsafe { device.destroy_buffer(buffer, None) };
        outcome
    }

    pub fn read_texture(&mut self, texture: &mut VulkanTexture) -> Result<Vec<u8>> {
        let size = texture.byte_size();
        let device = self.raw_device().clone();

        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&buffer_info, None) }
            .map_err(|e| backend_err("create_buffer", e))?;
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "readback",
                requirements,
                location: MemoryLocation::GpuToCpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_buffer(buffer, None) };
                Error::OutOfMemory {
                    what: "readback buffer",
                }
                .with_detail(e)
            })?;
        unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
            .map_err(|e| backend_err("bind_buffer_memory", e))?;

        let cmd = self.begin_one_shot()?;
        transition(
            &device,
            cmd,
            texture.image,
            texture.layout.get(),
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: texture.extent.width,
                height: texture.extent.height,
                depth: 1,
            });
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                texture.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
        }
        texture.layout.set(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);

        // The buffer and its allocation are released on every path below, so
        // the submission result is folded in rather than returned early.
        let pixels = self.submit_one_shot(cmd).and_then(|()| {
            // The submission was waited on, so the copy has completed and the
            // mapping is stable for the read.
            allocation
                .mapped_slice()
                .map(|s| s[..size as usize].to_vec())
                .ok_or(Error::Backend {
                    backend: "vulkan",
                    detail: "readback allocation was not host-visible".into(),
                })
        });

        let _ = self.allocator_mut().free(allocation);
        unsafe { device.destroy_buffer(buffer, None) };
        pixels
    }
}

/// Fill every level below the first by halving the one above it.
///
/// A blit per level rather than a single downsample of the original, because
/// that is what a mip chain means: level two is the average of level one and
/// not a quarter-scale filter over level zero, and the two differ once an image
/// has any detail near its own resolution. Linear filtering makes each blit the
/// average of the four texels it covers.
///
/// The barriers are per level rather than over the image, which is the one
/// place in this backend that has to be: each blit reads the level above while
/// writing the one below, so within a single chain the same image is a transfer
/// source and a transfer destination at once, in different subresources.
/// Transitioning the whole image between them would be transitioning a
/// subresource the blit is still reading.
///
/// Costs nothing on a texture with one level -- the loop does not run, and the
/// caller does not need to ask whether it should.
fn generate_mipmaps(device: &ash::Device, cmd: vk::CommandBuffer, texture: &VulkanTexture) {
    if texture.mip_levels <= 1 {
        return;
    }
    let level_range = |level: u32| {
        vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(level)
            .level_count(1)
            .layer_count(1)
    };
    let barrier = |level: u32, from: vk::ImageLayout, to: vk::ImageLayout| {
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(from)
            .new_layout(to)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(texture.image)
            .subresource_range(level_range(level))
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        // SAFETY: the command buffer is recording and the image outlives the
        // submission, which the one-shot helper waits on.
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    };

    let mut width = texture.extent.width.max(1) as i32;
    let mut height = texture.extent.height.max(1) as i32;
    for level in 1..texture.mip_levels {
        // The level above becomes readable. It was written either by the upload
        // copy, for level zero, or by the previous blit.
        barrier(
            level - 1,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        // An axis that has reached one texel stays there while the other keeps
        // halving, which is what makes a chain over a long thin image end in a
        // strip rather than in nothing.
        let next_width = (width / 2).max(1);
        let next_height = (height / 2).max(1);
        let blit = vk::ImageBlit::default()
            .src_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(level - 1)
                    .layer_count(1),
            )
            .src_offsets([
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: width,
                    y: height,
                    z: 1,
                },
            ])
            .dst_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(level)
                    .layer_count(1),
            )
            .dst_offsets([
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: next_width,
                    y: next_height,
                    z: 1,
                },
            ]);
        // SAFETY: as above, and both subresources are in the layouts named.
        unsafe {
            device.cmd_blit_image(
                cmd,
                texture.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                texture.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[blit],
                vk::Filter::LINEAR,
            );
        }
        width = next_width;
        height = next_height;
    }

    // Every level but the last is a transfer source now, and the caller's
    // record of the image's layout says destination. Putting them back is
    // cheaper than teaching that record about levels, and the transition that
    // follows -- to whatever samples this -- covers the whole image at once.
    for level in 0..texture.mip_levels - 1 {
        barrier(
            level,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
    }
}

/// Insert a full barrier around a layout change.
///
/// Deliberately heavy-handed: ALL_COMMANDS on both sides with full access
/// masks. These are one-shot setup and readback operations, not frame-loop
/// work, so precision here buys nothing and getting it wrong costs
/// hard-to-reproduce corruption.
pub(crate) fn transition(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    from: vk::ImageLayout,
    to: vk::ImageLayout,
) {
    let (src_stage, src_access) = source_scope(from);
    let (dst_stage, dst_access) = destination_scope(to);

    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(from)
        .new_layout(to)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                // Every level, because this transitions an image rather than a
                // level of one and a chain left half in the old layout is a
                // validation error waiting for the first minified draw. The
                // generation below transitions levels one at a time and says so
                // explicitly; nothing else here has any business splitting them.
                .level_count(vk::REMAINING_MIP_LEVELS)
                .layer_count(1),
        )
        .src_access_mask(src_access)
        .dst_access_mask(dst_access);

    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

/// What the image was being used for, given the layout it is leaving.
///
/// A barrier names the work it waits for and the work that waits on it. Naming
/// all of it -- every stage, every access -- is always correct and is what this
/// did: a full pipeline drain and a full cache flush for every transition. On
/// the tiled and bandwidth-limited parts this renderer targets that is not a
/// small waste, and it is avoidable, because a layout says what the image was
/// for.
///
/// Anything unrecognized falls back to naming everything, which is the
/// conservative direction: a scope too wide costs speed, and one too narrow is
/// a missing barrier that renders correctly here and wrongly elsewhere.
fn source_scope(layout: vk::ImageLayout) -> (vk::PipelineStageFlags, vk::AccessFlags) {
    match layout {
        // Nothing was in it worth preserving, so there is nothing to wait for.
        vk::ImageLayout::UNDEFINED | vk::ImageLayout::PREINITIALIZED => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
        ),
        vk::ImageLayout::TRANSFER_DST_OPTIMAL => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
        ),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_READ,
        ),

        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL => (
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        ),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        ),
        // Reading is the presentation engine's, which no stage here names and
        // which the acquire semaphore orders instead.
        vk::ImageLayout::PRESENT_SRC_KHR => (
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::AccessFlags::empty(),
        ),
        _ => (
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
        ),
    }
}

/// What the image is about to be used for, given the layout it is entering.
fn destination_scope(layout: vk::ImageLayout) -> (vk::PipelineStageFlags, vk::AccessFlags) {
    match layout {
        vk::ImageLayout::TRANSFER_DST_OPTIMAL => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
        ),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_READ,
        ),
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL => (
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::COLOR_ATTACHMENT_READ,
        ),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        ),
        // Handed to the presentation engine, which the present semaphore
        // orders. Naming a stage here would claim this device does the reading.
        vk::ImageLayout::PRESENT_SRC_KHR => (
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::AccessFlags::empty(),
        ),
        _ => (
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
        ),
    }
}

fn sample_count_flags(count: u32) -> vk::SampleCountFlags {
    match count {
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        _ => vk::SampleCountFlags::TYPE_1,
    }
}

pub(crate) fn backend_err(what: &str, e: vk::Result) -> Error {
    Error::Backend {
        backend: "vulkan",
        detail: format!("{what}: {e:?}"),
    }
}

/// Attach backend detail to an allocator failure without losing the category.
pub(crate) trait WithDetail {
    fn with_detail<E: std::fmt::Display>(self, e: E) -> Error;
}

impl WithDetail for Error {
    fn with_detail<E: std::fmt::Display>(self, e: E) -> Error {
        Error::Backend {
            backend: "vulkan",
            detail: format!("{self}: {e}"),
        }
    }
}

#[cfg(test)]
mod barrier_tests {
    use super::*;

    /// Every layout this renderer actually uses, and the direction it is used in.
    const USED: &[vk::ImageLayout] = &[
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::ImageLayout::PRESENT_SRC_KHR,
    ];

    #[test]
    fn a_known_layout_names_less_than_everything() {
        // The point of the table is that a transition waits for the work the
        // image was actually in, not for the whole pipeline to drain. Falling
        // back to `ALL_COMMANDS` for a layout this renderer uses every frame
        // would be correct and would quietly cost what the table was written to
        // save, with nothing failing to say so.
        for layout in USED {
            let (src_stage, _) = source_scope(*layout);
            let (dst_stage, _) = destination_scope(*layout);
            assert_ne!(
                src_stage,
                vk::PipelineStageFlags::ALL_COMMANDS,
                "{layout:?} falls back to a full drain as a source"
            );
            assert_ne!(
                dst_stage,
                vk::PipelineStageFlags::ALL_COMMANDS,
                "{layout:?} falls back to a full drain as a destination"
            );
        }
    }

    #[test]
    fn an_unknown_layout_names_everything() {
        // The fallback goes the safe way. A scope too wide costs speed; one too
        // narrow is a missing barrier, which renders correctly on the device it
        // was written on.
        let (stage, access) = source_scope(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        assert_eq!(stage, vk::PipelineStageFlags::ALL_COMMANDS);
        assert!(access.contains(vk::AccessFlags::MEMORY_WRITE));
    }

    #[test]
    fn leaving_undefined_waits_for_nothing() {
        // Nothing in the image was worth preserving, so there is no prior work
        // to order against -- and waiting for some anyway is the common way a
        // first-use transition costs a frame's worth of drain.
        let (stage, access) = source_scope(vk::ImageLayout::UNDEFINED);
        assert_eq!(stage, vk::PipelineStageFlags::TOP_OF_PIPE);
        assert!(access.is_empty());
    }
}

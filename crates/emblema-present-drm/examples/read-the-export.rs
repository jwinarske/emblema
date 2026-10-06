//! A Vulkan-exported linear dma-buf that the kernel reads back wrong, and the
//! one knob that decides whether it does.
//!
//! No renderer, no shaders, no pipeline. The image is filled by a buffer copy,
//! so the bytes in it are chosen by this program and not computed by a GPU.
//!
//! What it does, for each memory placement asked for:
//!
//! 1. allocates a `B8G8R8A8_UNORM` image with `DRM_FORMAT_MOD_LINEAR`, bound to
//!    memory of the requested type, exportable as a dma-buf;
//! 2. fills it from a staging buffer with a pattern whose green channel is the
//!    row index, so a misread row says which row was actually read;
//! 3. reads it back **through Vulkan**, which is the control: this is what the
//!    GPU has in the image;
//! 4. reads it back **through a userspace `mmap`** of the exported descriptor;
//! 5. hands the descriptor to `vkms` as a framebuffer, asks the CRTC to
//!    composite it through a writeback connector, and reads the result -- which
//!    is the kernel reading the same memory with `dma_buf_vmap`.
//!
//! Steps 3 and 4 agree wherever 4 is possible. Step 5 does not, and which
//! memory type the image landed in is what decides it.
//!
//! Usage:
//!
//! ```text
//! vram-export-repro [--card /dev/dri/cardN] [--size WxH] [placement ...]
//! ```
//!
//! with placements from `device-local`, `both`, `host-visible`, or none for all
//! three. The card must be one this process can become DRM master of.

use ash::vk;
use drm::buffer::Buffer as _;
use drm::control::{self, Device as _};
use drm::Device as _;
use std::collections::HashMap;

const MOD_LINEAR: u64 = 0;
const FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;
/// `DRM_FORMAT_XRGB8888`, which is `B8G8R8A8` little-endian with the alpha
/// byte ignored.
const FOURCC_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
/// `DRM_FORMAT_ARGB8888`.
const FOURCC_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Placement {
    DeviceLocal,
    Both,
    HostVisible,
}

impl Placement {
    fn flags(self) -> vk::MemoryPropertyFlags {
        match self {
            Self::DeviceLocal => vk::MemoryPropertyFlags::DEVICE_LOCAL,
            Self::Both => {
                vk::MemoryPropertyFlags::DEVICE_LOCAL | vk::MemoryPropertyFlags::HOST_VISIBLE
            }
            Self::HostVisible => {
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
            }
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::DeviceLocal => "device-local",
            Self::Both => "both",
            Self::HostVisible => "host-visible",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "device-local" => Some(Self::DeviceLocal),
            "both" => Some(Self::Both),
            "host-visible" => Some(Self::HostVisible),
            _ => None,
        }
    }
}

/// The byte a pixel should hold. Green is the row, so a row read from the
/// wrong offset reports which offset it came from.
fn expected(x: u32, y: u32) -> [u8; 4] {
    [
        (x & 0xff) as u8,
        (y & 0xff) as u8,
        ((x ^ y) & 0xff) as u8,
        0xff,
    ]
}

/// How a readback differs from the pattern, and what the green channel says
/// the rows actually were.
struct Verdict {
    rows_wrong: u32,
    first_wrong: Option<u32>,
    /// For the first wrong row, the row index its green channel claims.
    claims: Option<u8>,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.rows_wrong == 0 {
            return write!(f, "matches the pattern");
        }
        write!(f, "{} rows wrong", self.rows_wrong)?;
        if let Some(row) = self.first_wrong {
            write!(f, ", first at {row}")?;
        }
        if let Some(claims) = self.claims {
            write!(f, ", whose green says row {claims}")?;
        }
        Ok(())
    }
}

/// Compare a readback, which may be padded to `stride`, against the pattern.
fn check(bytes: &[u8], width: u32, height: u32, stride: u32) -> Verdict {
    let mut rows_wrong = 0;
    let mut first_wrong = None;
    let mut claims = None;
    for y in 0..height {
        let row = (y * stride) as usize;
        let mut bad = false;
        for x in 0..width {
            let at = row + (x * 4) as usize;
            if bytes[at..at + 4] != expected(x, y) {
                bad = true;
                break;
            }
        }
        if bad {
            if first_wrong.is_none() {
                first_wrong = Some(y);
                claims = Some(bytes[row + 1]);
            }
            rows_wrong += 1;
        }
    }
    Verdict {
        rows_wrong,
        first_wrong,
        claims,
    }
}

// ---------------------------------------------------------------- Vulkan side

struct Gpu {
    _entry: ash::Entry,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    family: u32,
    pool: vk::CommandPool,
    external: ash::khr::external_memory_fd::Device,
    name: String,
}

impl Gpu {
    fn new() -> Result<Self, String> {
        // SAFETY: the loader is used for the lifetime of this program.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| format!("loading Vulkan: {e}"))?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance_info = vk::InstanceCreateInfo::default().application_info(&app);
        let instance = unsafe { entry.create_instance(&instance_info, None) }
            .map_err(|e| format!("create_instance: {e}"))?;

        let devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| format!("enumerate_physical_devices: {e}"))?;
        // A hardware device: the whole question is about where its memory
        // lands, and a software one has one heap.
        let physical = devices
            .iter()
            .copied()
            .find(|d| {
                let p = unsafe { instance.get_physical_device_properties(*d) };
                p.device_type != vk::PhysicalDeviceType::CPU
            })
            .or_else(|| devices.first().copied())
            .ok_or("no Vulkan device")?;
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        let name = unsafe { std::ffi::CStr::from_ptr(properties.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        let family = unsafe { instance.get_physical_device_queue_family_properties(physical) }
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::TRANSFER))
            .ok_or("no transfer queue")? as u32;

        let extensions = [
            ash::ext::image_drm_format_modifier::NAME.as_ptr(),
            ash::khr::external_memory_fd::NAME.as_ptr(),
            ash::ext::external_memory_dma_buf::NAME.as_ptr(),
            ash::khr::image_format_list::NAME.as_ptr(),
            ash::khr::bind_memory2::NAME.as_ptr(),
            ash::khr::get_memory_requirements2::NAME.as_ptr(),
            ash::khr::sampler_ycbcr_conversion::NAME.as_ptr(),
            ash::khr::maintenance1::NAME.as_ptr(),
        ];
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&extensions);
        let device = unsafe { instance.create_device(physical, &device_info, None) }
            .map_err(|e| format!("create_device: {e}"))?;
        let queue = unsafe { device.get_device_queue(family, 0) };
        let pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                None,
            )
        }
        .map_err(|e| format!("create_command_pool: {e}"))?;
        let external = ash::khr::external_memory_fd::Device::new(&instance, &device);

        Ok(Self {
            _entry: entry,
            instance,
            physical,
            device,
            queue,
            family,
            pool,
            external,
            name,
        })
    }

    /// Candidate memory types for an image, printed once so a reader can see
    /// which index each placement selects.
    fn report_memory_types(&self, type_bits: u32) {
        let props = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical)
        };
        for i in 0..props.memory_type_count {
            if type_bits & (1 << i) == 0 {
                continue;
            }
            let t = props.memory_types[i as usize];
            let heap = props.memory_heaps[t.heap_index as usize];
            println!(
                "    type {i:2}  heap {}  {:>6} MiB  {:?}",
                t.heap_index,
                heap.size / (1024 * 1024),
                t.property_flags
            );
        }
    }

    fn memory_type(&self, type_bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        let props = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical)
        };
        (0..props.memory_type_count).find(|i| {
            type_bits & (1 << i) != 0
                && props.memory_types[*i as usize]
                    .property_flags
                    .contains(flags)
        })
    }

    fn run(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<(), String> {
        let info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cmd = unsafe { self.device.allocate_command_buffers(&info) }
            .map_err(|e| format!("allocate_command_buffers: {e}"))?[0];
        unsafe {
            self.device
                .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
                .map_err(|e| format!("begin: {e}"))?;
        }
        record(cmd);
        unsafe {
            self.device
                .end_command_buffer(cmd)
                .map_err(|e| format!("end: {e}"))?;
            let buffers = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&buffers);
            self.device
                .queue_submit(self.queue, &[submit], vk::Fence::null())
                .map_err(|e| format!("submit: {e}"))?;
            self.device
                .queue_wait_idle(self.queue)
                .map_err(|e| format!("queue_wait_idle: {e}"))?;
            self.device.free_command_buffers(self.pool, &[cmd]);
        }
        Ok(())
    }

    /// A host-visible buffer, for staging in and reading back out.
    fn buffer(&self, size: u64, usage: vk::BufferUsageFlags) -> Result<Staging, String> {
        let info = vk::BufferCreateInfo::default().size(size).usage(usage);
        let buffer = unsafe { self.device.create_buffer(&info, None) }
            .map_err(|e| format!("create_buffer: {e}"))?;
        let need = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let index = self
            .memory_type(
                need.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or("no host-visible memory for staging")?;
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(need.size)
            .memory_type_index(index);
        let memory = unsafe { self.device.allocate_memory(&allocate, None) }
            .map_err(|e| format!("allocate_memory (staging): {e}"))?;
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|e| format!("bind_buffer_memory: {e}"))?;
        Ok(Staging {
            buffer,
            memory,
            size: need.size,
        })
    }
}

struct Staging {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
}

/// The exported image, and what the GPU and a CPU map make of it.
struct Exported {
    image: vk::Image,
    memory: vk::DeviceMemory,
    fd: std::os::fd::OwnedFd,
    stride: u32,
    offset: u64,
    memory_type: u32,
}

fn build_and_fill(
    gpu: &Gpu,
    width: u32,
    height: u32,
    placement: Placement,
    attachment: bool,
    report_types: bool,
) -> Result<Exported, String> {
    let modifiers = [MOD_LINEAR];
    let mut list =
        vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(FORMAT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | if attachment {
                    // What a renderer asks for, and it changes the answer.
                    vk::ImageUsageFlags::COLOR_ATTACHMENT
                } else {
                    vk::ImageUsageFlags::empty()
                },
        )
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut list)
        .push_next(&mut external);
    let image = unsafe { gpu.device.create_image(&info, None) }
        .map_err(|e| format!("create_image: {e}"))?;

    let need = unsafe { gpu.device.get_image_memory_requirements(image) };
    if report_types {
        println!("  memory types an exportable linear image accepts:");
        gpu.report_memory_types(need.memory_type_bits);
    }
    let memory_type = gpu
        .memory_type(need.memory_type_bits, placement.flags())
        .ok_or_else(|| format!("no memory type is {:?}", placement.flags()))?;

    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let mut export = vk::ExportMemoryAllocateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let allocate = vk::MemoryAllocateInfo::default()
        .allocation_size(need.size)
        .memory_type_index(memory_type)
        .push_next(&mut dedicated)
        .push_next(&mut export);
    let memory = unsafe { gpu.device.allocate_memory(&allocate, None) }
        .map_err(|e| format!("allocate_memory: {e}"))?;
    unsafe { gpu.device.bind_image_memory(image, memory, 0) }
        .map_err(|e| format!("bind_image_memory: {e}"))?;

    // The modifier image's layout is read through the memory-plane aspect, not
    // the color one.
    let layout = unsafe {
        gpu.device.get_image_subresource_layout(
            image,
            vk::ImageSubresource::default()
                .aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT)
                .mip_level(0)
                .array_layer(0),
        )
    };

    // Fill from a staging buffer: the bytes are this program's, not a GPU's.
    let stride = layout.row_pitch as u32;
    let staging = gpu.buffer((stride * height) as u64, vk::BufferUsageFlags::TRANSFER_SRC)?;
    unsafe {
        let ptr = gpu
            .device
            .map_memory(staging.memory, 0, staging.size, vk::MemoryMapFlags::empty())
            .map_err(|e| format!("map_memory: {e}"))? as *mut u8;
        let bytes = std::slice::from_raw_parts_mut(ptr, (stride * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let at = (y * stride + x * 4) as usize;
                bytes[at..at + 4].copy_from_slice(&expected(x, y));
            }
        }
        gpu.device.unmap_memory(staging.memory);
    }

    let subresource = vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1);
    gpu.run(|cmd| unsafe {
        let to_dst = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        gpu.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_dst],
        );
        let region = vk::BufferImageCopy::default()
            .buffer_row_length(stride / 4)
            .image_subresource(subresource)
            .image_extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            });
        gpu.device.cmd_copy_buffer_to_image(
            cmd,
            staging.buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[region],
        );
        // To GENERAL and released to a foreign reader, which is the handover
        // the spec asks for before another device touches it. It makes no
        // difference to the outcome; it is here so that cannot be the answer.
        let to_general = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(gpu.family)
            .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        gpu.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_general],
        );
    })?;
    unsafe {
        gpu.device.destroy_buffer(staging.buffer, None);
        gpu.device.free_memory(staging.memory, None);
    }

    let fd = unsafe {
        gpu.external.get_memory_fd(
            &vk::MemoryGetFdInfoKHR::default()
                .memory(memory)
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
        )
    }
    .map_err(|e| format!("get_memory_fd: {e}"))?;
    // SAFETY: the driver handed this over and nothing else holds it.
    let fd = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };

    Ok(Exported {
        image,
        memory,
        fd,
        stride,
        offset: layout.offset,
        memory_type,
    })
}

/// What the GPU has in the image. The control for everything else.
fn read_through_vulkan(
    gpu: &Gpu,
    exported: &Exported,
    width: u32,
    height: u32,
) -> Result<Verdict, String> {
    let size = (exported.stride * height) as u64;
    let staging = gpu.buffer(size, vk::BufferUsageFlags::TRANSFER_DST)?;
    let subresource = vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1);
    gpu.run(|cmd| unsafe {
        let region = vk::BufferImageCopy::default()
            .buffer_row_length(exported.stride / 4)
            .image_subresource(subresource)
            .image_extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            });
        gpu.device.cmd_copy_image_to_buffer(
            cmd,
            exported.image,
            vk::ImageLayout::GENERAL,
            staging.buffer,
            &[region],
        );
    })?;
    let verdict = unsafe {
        let ptr = gpu
            .device
            .map_memory(staging.memory, 0, size, vk::MemoryMapFlags::empty())
            .map_err(|e| format!("map_memory: {e}"))? as *const u8;
        let bytes = std::slice::from_raw_parts(ptr, size as usize);
        let verdict = check(bytes, width, height, exported.stride);
        gpu.device.unmap_memory(staging.memory);
        verdict
    };
    unsafe {
        gpu.device.destroy_buffer(staging.buffer, None);
        gpu.device.free_memory(staging.memory, None);
    }
    Ok(verdict)
}

/// What a plain userspace map of the exported descriptor sees.
fn read_through_mmap(exported: &Exported, width: u32, height: u32) -> Result<Verdict, String> {
    let len = (exported.stride * height) as usize + exported.offset as usize;
    // SAFETY: the length covers the image and the mapping is dropped below.
    let map = unsafe {
        rustix::mm::mmap(
            std::ptr::null_mut(),
            len,
            rustix::mm::ProtFlags::READ,
            rustix::mm::MapFlags::SHARED,
            &exported.fd,
            0,
        )
    }
    .map_err(|e| format!("{e}"))?;
    // SAFETY: `len` bytes were just mapped.
    let bytes = unsafe { std::slice::from_raw_parts(map as *const u8, len) };
    let verdict = check(
        &bytes[exported.offset as usize..],
        width,
        height,
        exported.stride,
    );
    // SAFETY: the pointer and length are the ones mmap returned.
    let _ = unsafe { rustix::mm::munmap(map, len) };
    Ok(verdict)
}

// ------------------------------------------------------------------- KMS side

/// The one reader that disagrees: `vkms` compositing through a writeback
/// connector, which maps the import in the kernel.
struct Vkms {
    device: Card,
    connector: control::connector::Handle,
    crtc: control::crtc::Handle,
    plane: control::plane::Handle,
    mode: control::Mode,
    fourcc: u32,
    connector_props: HashMap<String, control::property::Handle>,
    crtc_props: HashMap<String, control::property::Handle>,
    plane_props: HashMap<String, control::property::Handle>,
}

struct Card(std::fs::File);

impl std::os::fd::AsFd for Card {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.0)
    }
}
impl drm::Device for Card {}
impl control::Device for Card {}

fn properties<T: control::ResourceHandle>(
    card: &Card,
    handle: T,
) -> HashMap<String, control::property::Handle> {
    let mut out = HashMap::new();
    let Ok(set) = card.get_properties(handle) else {
        return out;
    };
    for id in set.as_props_and_values().0 {
        if let Ok(info) = card.get_property(*id) {
            out.insert(info.name().to_string_lossy().into_owned(), *id);
        }
    }
    out
}

fn blob_value<T: control::ResourceHandle>(card: &Card, handle: T, name: &str) -> Option<Vec<u8>> {
    let set = card.get_properties(handle).ok()?;
    let (ids, values) = set.as_props_and_values();
    for (id, value) in ids.iter().zip(values) {
        let info = card.get_property(*id).ok()?;
        if info.name().to_string_lossy() == name {
            return card.get_property_blob(*value).ok();
        }
    }
    None
}

impl Vkms {
    fn open(path: &str) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        let card = Card(file);

        card.acquire_master_lock()
            .map_err(|e| format!("becoming DRM master of {path}: {e}"))?;
        card.set_client_capability(drm::ClientCapability::UniversalPlanes, true)
            .map_err(|e| format!("universal planes: {e}"))?;
        card.set_client_capability(drm::ClientCapability::Atomic, true)
            .map_err(|e| format!("atomic: {e}"))?;
        // Without this the writeback connector is not in the resource list at
        // all, whatever the hardware has.
        card.set_client_capability(drm::ClientCapability::WritebackConnectors, true)
            .map_err(|e| format!("writeback connectors: {e}"))?;

        let resources = card
            .resource_handles()
            .map_err(|e| format!("resource_handles: {e}"))?;

        let mut writeback = None;
        let mut mode = None;
        for handle in resources.connectors() {
            let Ok(info) = card.get_connector(*handle, false) else {
                continue;
            };
            if info.interface() == control::connector::Interface::Writeback {
                writeback = writeback.or(Some(info));
            } else if mode.is_none() {
                mode = info.modes().first().copied();
            }
        }
        let connector = writeback.ok_or("no writeback connector on this card")?;
        // A writeback connector reports no modes; borrow one that does.
        let mode = mode.ok_or("no connector here reports a mode to borrow")?;

        let connector_props = properties(&card, connector.handle());
        for name in [
            "WRITEBACK_FB_ID",
            "WRITEBACK_OUT_FENCE_PTR",
            "WRITEBACK_PIXEL_FORMATS",
            "CRTC_ID",
        ] {
            if !connector_props.contains_key(name) {
                return Err(format!("the writeback connector has no {name}"));
            }
        }
        let formats = blob_value(&card, connector.handle(), "WRITEBACK_PIXEL_FORMATS")
            .ok_or("WRITEBACK_PIXEL_FORMATS has no blob")?;
        let advertised: Vec<u32> = formats
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let fourcc = [FOURCC_XRGB8888, FOURCC_ARGB8888]
            .into_iter()
            .find(|want| advertised.contains(want))
            .ok_or("writeback advertises neither XRGB8888 nor ARGB8888")?;

        let mut chosen = None;
        for encoder in connector.encoders() {
            let Ok(info) = card.get_encoder(*encoder) else {
                continue;
            };
            for crtc in resources.filter_crtcs(info.possible_crtcs()) {
                let plane = card.plane_handles().ok().and_then(|planes| {
                    planes.into_iter().find(|p| {
                        let Ok(info) = card.get_plane(*p) else {
                            return false;
                        };
                        info.crtc().is_none_or(|bound| bound == crtc)
                            && resources
                                .filter_crtcs(info.possible_crtcs())
                                .contains(&crtc)
                            && properties(&card, *p).contains_key("IN_FORMATS")
                    })
                });
                if let Some(plane) = plane {
                    chosen = Some((crtc, plane));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let (crtc, plane) = chosen.ok_or("no CRTC with a plane for the writeback connector")?;

        Ok(Self {
            crtc_props: properties(&card, crtc),
            plane_props: properties(&card, plane),
            connector_props,
            connector: connector.handle(),
            crtc,
            plane,
            mode,
            fourcc,
            device: card,
        })
    }

    /// Composite the exported buffer and read back what the kernel made of it.
    fn composite(&self, exported: &Exported, width: u32, height: u32) -> Result<Verdict, String> {
        let handle = self
            .device
            .prime_fd_to_buffer(std::os::fd::AsFd::as_fd(&exported.fd))
            .map_err(|e| format!("prime_fd_to_buffer: {e}"))?;
        let source = Planar {
            width,
            height,
            fourcc: FOURCC_XRGB8888,
            modifier: Some(MOD_LINEAR),
            pitches: [exported.stride, 0, 0, 0],
            offsets: [exported.offset as u32, 0, 0, 0],
            handles: [Some(handle), None, None, None],
        };
        let source_fb = self
            .device
            .add_planar_framebuffer(&source, control::FbCmd2Flags::MODIFIERS)
            .map_err(|e| format!("the controller refused the exported buffer: {e}"))?;

        let mut dumb = self
            .device
            .create_dumb_buffer((width, height), drm::buffer::DrmFourcc::Xrgb8888, 32)
            .map_err(|e| format!("create_dumb_buffer: {e}"))?;
        let pitch = dumb.pitch();
        let dest = Planar {
            width,
            height,
            fourcc: self.fourcc,
            modifier: None,
            pitches: [pitch, 0, 0, 0],
            offsets: [0; 4],
            handles: [Some(dumb.handle()), None, None, None],
        };
        let dest_fb = self
            .device
            .add_planar_framebuffer(&dest, control::FbCmd2Flags::empty())
            .map_err(|e| format!("the writeback destination was refused: {e}"))?;

        let blob = self
            .device
            .create_property_blob(&self.mode)
            .map_err(|e| format!("create_property_blob: {e}"))?;
        let mut fence: i32 = -1;

        let mut atomic = control::atomic::AtomicModeReq::new();
        let connector: control::RawResourceHandle = self.connector.into();
        let crtc: control::RawResourceHandle = self.crtc.into();
        let plane: control::RawResourceHandle = self.plane.into();
        let mut set =
            |object, props: &HashMap<String, control::property::Handle>, name: &str, value: u64| {
                atomic.add_raw_property(object, props[name], value);
            };
        set(
            connector,
            &self.connector_props,
            "CRTC_ID",
            u32::from(self.crtc) as u64,
        );
        set(
            connector,
            &self.connector_props,
            "WRITEBACK_FB_ID",
            u32::from(dest_fb) as u64,
        );
        set(
            connector,
            &self.connector_props,
            "WRITEBACK_OUT_FENCE_PTR",
            &mut fence as *mut i32 as u64,
        );
        set(crtc, &self.crtc_props, "ACTIVE", 1);
        set(
            plane,
            &self.plane_props,
            "FB_ID",
            u32::from(source_fb) as u64,
        );
        set(
            plane,
            &self.plane_props,
            "CRTC_ID",
            u32::from(self.crtc) as u64,
        );
        set(plane, &self.plane_props, "SRC_X", 0);
        set(plane, &self.plane_props, "SRC_Y", 0);
        set(plane, &self.plane_props, "SRC_W", (width as u64) << 16);
        set(plane, &self.plane_props, "SRC_H", (height as u64) << 16);
        set(plane, &self.plane_props, "CRTC_X", 0);
        set(plane, &self.plane_props, "CRTC_Y", 0);
        set(plane, &self.plane_props, "CRTC_W", width as u64);
        set(plane, &self.plane_props, "CRTC_H", height as u64);
        atomic.add_property(self.crtc, self.crtc_props["MODE_ID"], blob);

        let committed = self
            .device
            .atomic_commit(control::AtomicCommitFlags::ALLOW_MODESET, atomic)
            .map_err(|e| format!("atomic_commit: {e}"));

        let verdict = committed.and_then(|()| {
            wait_for_fence(fence)?;
            let mapping = self
                .device
                .map_dumb_buffer(&mut dumb)
                .map_err(|e| format!("map_dumb_buffer: {e}"))?;
            Ok(check(&mapping, width, height, pitch))
        });

        let _ = self.device.destroy_framebuffer(dest_fb);
        let _ = self.device.destroy_framebuffer(source_fb);
        let _ = self.device.destroy_dumb_buffer(dumb);
        let _ = self.device.close_buffer(handle);
        verdict
    }
}

/// A sync_file polls readable when it signals, which is what says the
/// composition is finished; the commit returning only says it was accepted.
fn wait_for_fence(fd: i32) -> Result<(), String> {
    if fd < 0 {
        return Err("the commit returned no writeback fence".into());
    }
    // SAFETY: the kernel wrote this descriptor for this commit.
    let fence = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let mut fds = [rustix::event::PollFd::new(
        &fence,
        rustix::event::PollFlags::IN,
    )];
    let timeout = rustix::event::Timespec {
        tv_sec: 15,
        tv_nsec: 0,
    };
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(0) => Err("the writeback fence did not signal in time".into()),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("poll on the writeback fence: {e}")),
    }
}

struct Planar {
    width: u32,
    height: u32,
    fourcc: u32,
    modifier: Option<u64>,
    pitches: [u32; 4],
    offsets: [u32; 4],
    handles: [Option<drm::buffer::Handle>; 4],
}

impl drm::buffer::PlanarBuffer for Planar {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn format(&self) -> drm::buffer::DrmFourcc {
        drm::buffer::DrmFourcc::try_from(self.fourcc).unwrap_or(drm::buffer::DrmFourcc::Xrgb8888)
    }
    fn modifier(&self) -> Option<drm::buffer::DrmModifier> {
        self.modifier.map(drm::buffer::DrmModifier::from)
    }
    fn pitches(&self) -> [u32; 4] {
        self.pitches
    }
    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        self.handles
    }
    fn offsets(&self) -> [u32; 4] {
        self.offsets
    }
}

// ----------------------------------------------------------------------- main

fn main() -> std::process::ExitCode {
    let mut card = "/dev/dri/card0".to_string();
    let mut width = 1024u32;
    let mut height = 768u32;
    let mut placements = Vec::new();
    let mut attachment = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--card" => {
                i += 1;
                card = args.get(i).cloned().unwrap_or(card);
            }
            "--size" => {
                i += 1;
                if let Some((w, h)) = args.get(i).and_then(|s| s.split_once('x')) {
                    width = w.parse().unwrap_or(width);
                    height = h.parse().unwrap_or(height);
                }
            }
            "--attachment" => attachment = true,
            other => match Placement::parse(other) {
                Some(p) => placements.push(p),
                None => {
                    eprintln!("unknown argument {other}");
                    return std::process::ExitCode::FAILURE;
                }
            },
        }
        i += 1;
    }
    if placements.is_empty() {
        placements = vec![
            Placement::DeviceLocal,
            Placement::Both,
            Placement::HostVisible,
        ];
    }

    let gpu = match Gpu::new() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("{e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let vkms = match Vkms::open(&card) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    println!("gpu: {}", gpu.name);
    println!(
        "card: {card}, {width}x{height}, linear XRGB8888, usage {}\n",
        if attachment {
            "transfer and color attachment"
        } else {
            "transfer only"
        }
    );

    let mut first = true;
    for placement in placements {
        println!("placement {}:", placement.name());
        let exported = match build_and_fill(&gpu, width, height, placement, attachment, first) {
            Ok(e) => e,
            Err(e) => {
                println!("  skipped: {e}\n");
                continue;
            }
        };
        first = false;
        println!(
            "  memory type {}, stride {}, offset {}",
            exported.memory_type, exported.stride, exported.offset
        );
        match read_through_vulkan(&gpu, &exported, width, height) {
            Ok(v) => println!("  {:<17} {v}", "through Vulkan:"),
            Err(e) => println!("  {:<17} failed, {e}", "through Vulkan:"),
        }
        match read_through_mmap(&exported, width, height) {
            Ok(v) => println!("  {:<17} {v}", "through mmap:"),
            Err(e) => println!("  {:<17} failed, {e}", "through mmap:"),
        }
        match vkms.composite(&exported, width, height) {
            Ok(v) => println!("  {:<17} {v}", "through the CRTC:"),
            Err(e) => println!("  {:<17} failed, {e}", "through the CRTC:"),
        }
        println!();
        unsafe {
            gpu.device.destroy_image(exported.image, None);
            gpu.device.free_memory(exported.memory, None);
        }
    }
    std::process::ExitCode::SUCCESS
}

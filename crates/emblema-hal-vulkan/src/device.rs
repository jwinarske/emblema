//! Instance and device bring-up, and the capability detection everything
//! above the HAL branches on.
//!
//! Capability detection is the load-bearing part. The presentation layer
//! decides whether it can allocate its own scanout buffers or must import from
//! GBM, and whether the frame loop can stay explicit or needs a CPU wait
//! before commit, purely from what is reported here. Getting a bit wrong does
//! not fail loudly — it silently selects the slower path, or worse, selects
//! the fast path on a driver that cannot support it.

use crate::render::PipelineCache;
use crate::validation::{self, ValidationLog, ValidationMessage, VALIDATION_LAYER};
use ash::vk;
use emblema_hal::{Capabilities, DmaBufSupport, Error, Result, SampleCounts, SyncSupport};
use std::collections::HashSet;
use std::ffi::{c_char, CStr, CString};
use std::sync::Arc;

/// Which physical device to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevicePreference {
    /// Prefer discrete, then integrated, then anything else.
    #[default]
    Auto,
    /// Prefer a software rasterizer.
    ///
    /// The deterministic reference used for golden comparison: a CPU device
    /// produces the same pixels everywhere, which is what makes it usable as
    /// the oracle other backends are diffed against.
    Software,
    /// A specific index into the enumeration order.
    ///
    /// Not stable across processes: the order depends on the loader, on which ICDs are installed and
    /// on their own order. Useful for a test that has just enumerated, and not for a host that wants
    /// a particular GPU -- which is what the two below are for.
    Index(usize),
    /// The device whose `VkPhysicalDeviceIDProperties::deviceUUID` matches.
    ///
    /// The stable name for a GPU: the same sixteen bytes across processes, loaders and ICD order, and
    /// the same ones a compositor or another API reports for the same device. This is what a host
    /// that must share frames should ask for -- a dma-buf import across two physical devices costs a
    /// copy or fails outright, and two devices' DRM modifier sets need not intersect at all.
    Uuid([u8; 16]),
    /// The device whose DRM render node matches, by device number.
    ///
    /// For a host that starts from a DRM fd rather than a UUID -- a compositor usually does. Read
    /// through `VK_EXT_physical_device_drm`, so a driver without that extension can never match, and
    /// a device that reports no render node is skipped rather than guessed at.
    RenderNode {
        /// Major device number of the render node, as `stat` reports it.
        major: i64,
        /// Minor device number.
        minor: i64,
    },
}

/// How to create a context.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextConfig {
    pub device: DevicePreference,
    /// Capabilities to build this context without, whatever the device offers.
    ///
    /// Empty by default. Tests set it to reach a refusal on a machine whose
    /// device has the thing -- before this, several could only run on hardware
    /// that genuinely lacked it, and one of them said so.
    ///
    /// Withholding only; see `emblema_hal::Withheld` for why that direction is
    /// the only one there is. Where the capability is backed by an extension this
    /// is honored by not enabling it, so the device is genuinely built without it
    /// and the probe reports false on its own merits rather than being overruled.
    pub withheld: emblema_hal::Withheld,
    /// Request the validation layer and capture what it reports.
    ///
    /// Off by default because it costs real time per call. Tests turn it on
    /// and assert the log is clean, which is what keeps "validation-clean"
    /// from depending on someone reading stderr.
    pub validation: bool,
}

/// Instance extensions a presentation target may need.
///
/// `VK_KHR_surface` first because every other one depends on it. The platform
/// ones follow, and the headless surface last: it creates a surface with no
/// window behind it, which is what makes the swapchain path — capability
/// queries, format negotiation, acquire, present, recreation — checkable on a
/// machine with no display at all.
///
/// Creating windows is not this project's business, any more than mode setting
/// is. A caller brings a surface; these are what let one exist.
pub fn surface_extensions() -> &'static [&'static str] {
    &[
        "VK_KHR_surface",
        "VK_KHR_wayland_surface",
        "VK_KHR_xcb_surface",
        "VK_KHR_xlib_surface",
        "VK_EXT_headless_surface",
    ]
}

/// Semaphores a submission waits on and signals.
///
/// Grouped rather than passed loose so that adding a timeline value or a second
/// stage mask later changes one type rather than every signature between here
/// and a presentation target.
///
/// Vulkan-specific on purpose. The HAL's own synchronization is a fence and an
/// exported `sync_file`, which is what crosses to a display controller; a
/// swapchain's semaphores never leave the device and have no counterpart on a
/// backend that has no swapchain. A Vulkan presentation target using Vulkan
/// semaphores is not the layering violation that a renderer asking "is this
/// Vulkan?" would be.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameSync<'a> {
    /// Waited on before the color attachment is written.
    pub wait: &'a [vk::Semaphore],
    /// Signaled when the submission completes.
    pub signal: &'a [vk::Semaphore],
    /// Leave the target in the layout a presentation engine reads from.
    ///
    /// Folded into the render pass rather than done as a transition afterwards,
    /// and not as an optimization: a separate transition is a separate
    /// submission, and nothing orders it after a render that has not been
    /// waited for. Doing it here makes the ordering the render pass's, which is
    /// where it can be expressed without a stall.
    pub presents: bool,
}

/// Which extensions a withheld capability is backed by on this backend.
///
/// Exhaustive on purpose: a new `Capability` fails to compile here until someone
/// decides what it means for Vulkan. Empty is a legitimate answer --
/// `float_render_targets` comes from a format-properties query and has no
/// extension to leave out, so it is withheld by clearing the field after
/// detection instead.
///
/// These names are subtracted from what the device reports as *available*, not
/// from what this backend asks for. `resolve_extensions` re-adds an extension's
/// dependencies, so a name dropped from the request list comes back as some other
/// extension's dependency; subtracting from the available set instead means its
/// existing rule -- drop anything whose dependencies are missing -- cascades for
/// free and the device stays valid by construction. Nothing withheld here has a
/// dependent, so that cascade is not exercised yet; a capability whose extension
/// others depend on wants it checked.
fn withheld_extensions(capability: emblema_hal::Capability) -> &'static [&'static str] {
    match capability {
        emblema_hal::Capability::AdvancedBlend => &[ext::BLEND_OPERATION_ADVANCED],
        emblema_hal::Capability::FloatRenderTargets => &[],
    }
}

/// Extensions this backend asks for when the device offers them.
///
/// Absence is not an error — it selects a different path, and which one is
/// reported through [`Capabilities`] rather than inferred from the backend.
mod ext {
    pub const EXTERNAL_MEMORY_DMA_BUF: &str = "VK_EXT_external_memory_dma_buf";
    pub const EXTERNAL_MEMORY_FD: &str = "VK_KHR_external_memory_fd";
    pub const IMAGE_DRM_FORMAT_MODIFIER: &str = "VK_EXT_image_drm_format_modifier";
    pub const EXTERNAL_FENCE_FD: &str = "VK_KHR_external_fence_fd";
    pub const EXTERNAL_SEMAPHORE_FD: &str = "VK_KHR_external_semaphore_fd";
    pub const PHYSICAL_DEVICE_DRM: &str = "VK_EXT_physical_device_drm";
    /// Promoted to core in 1.2, so on the 1.1 baseline it must be requested
    /// explicitly as a dependency of the modifier extension.
    pub const IMAGE_FORMAT_LIST: &str = "VK_KHR_image_format_list";
    /// The advanced blend modes, separable and non-separable alike, which no
    /// combination of blend factors can express. Unrelated to the DRM path;
    /// gated the same way because the answer to "can this device do it" is a
    /// capability either way.
    pub const BLEND_OPERATION_ADVANCED: &str = "VK_EXT_blend_operation_advanced";
    /// Presenting into a surface. Absent on a device that can render but not
    /// display, which is a normal thing for a compute-only or headless card to
    /// be.
    pub const SWAPCHAIN: &str = "VK_KHR_swapchain";
}

/// Extensions each wanted extension depends on, beyond what the 1.1 baseline
/// already provides as core.
///
/// The specification requires every dependency to appear in the enable list
/// too, and a device created without them is invalid even when it appears to
/// work. Omitting one here surfaces only under validation, which is why the
/// resolution below drops any extension whose dependencies are unavailable
/// rather than enabling it regardless.
fn required_dependencies(name: &str) -> &'static [&'static str] {
    match name {
        ext::IMAGE_DRM_FORMAT_MODIFIER => &[ext::IMAGE_FORMAT_LIST],
        ext::EXTERNAL_MEMORY_DMA_BUF => &[ext::EXTERNAL_MEMORY_FD],
        _ => &[],
    }
}

/// Whether this device can do every advanced blend mode, coherently.
///
/// Three conditions, all required, and all reported as one flag because a
/// caller cannot do anything useful with two of the three. `allOperations`
/// covers the modes themselves -- and it is the reason this says every mode
/// rather than the separable ones: what it guarantees beyond the subset a
/// device may otherwise offer is the four non-separable operations, so
/// requiring it is what makes `Hue` through `Luminosity` safe to report. The
/// coherent feature is what lets overlapping
/// draws share a pass without a barrier between them; and the attachment limit
/// has to cover what a pass actually binds. Anything short of all three reports
/// false, and the modes are then refused rather than approximated.
fn probe_advanced_blend(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    enabled: &HashSet<String>,
) -> bool {
    if !enabled.contains(ext::BLEND_OPERATION_ADVANCED) {
        return false;
    }

    let mut properties = vk::PhysicalDeviceBlendOperationAdvancedPropertiesEXT::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut properties);
    unsafe { instance.get_physical_device_properties2(physical_device, &mut properties2) };

    let mut features = vk::PhysicalDeviceBlendOperationAdvancedFeaturesEXT::default();
    let mut features2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut features);
    unsafe { instance.get_physical_device_features2(physical_device, &mut features2) };

    properties.advanced_blend_all_operations == vk::TRUE
        && features.advanced_blend_coherent_operations == vk::TRUE
        && properties.advanced_blend_max_color_attachments >= 1
}

/// Expand the wanted set to include dependencies, dropping anything whose
/// dependencies this device does not offer.
fn resolve_extensions(wanted: &[&str], available: &HashSet<String>) -> HashSet<String> {
    let mut enabled = HashSet::new();
    for name in wanted {
        if !available.contains(*name) {
            continue;
        }
        let deps = required_dependencies(name);
        if deps.iter().any(|d| !available.contains(*d)) {
            // Enabling this would produce an invalid device. Skipping it means
            // the capability it backs is reported false, which is the honest
            // answer.
            continue;
        }
        enabled.insert((*name).to_string());
        enabled.extend(deps.iter().map(|d| (*d).to_string()));
    }
    enabled
}

/// A Vulkan device, its queue, and what it can do.
pub struct VulkanContext {
    // The messenger must be destroyed before the log it points at is dropped,
    // and before the instance that owns it.
    debug_messenger: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    validation_log: Arc<ValidationLog>,
    /// Whether the layer was asked to check synchronization as well as calls.
    ///
    /// Recorded because a clean validation log means two quite different things
    /// depending on it, and nothing else distinguishes them: a run with only
    /// core validation reports every call well formed and says nothing at all
    /// about whether one access was ordered against the next.
    sync_validation: bool,
    // Declaration order is destruction order: the device must outlive nothing
    // and the instance must outlive the device, so Drop tears down in reverse.
    // The allocator must release its memory before the device goes away, which
    // is why it is an Option -- Drop takes it and drops it explicitly first.
    allocator: Option<gpu_allocator::vulkan::Allocator>,
    pipelines: PipelineCache,
    command_pool: vk::CommandPool,
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family_index: u32,
    capabilities: Capabilities,
    /// Texture sampling objects, built the first time anything samples.
    ///
    /// Lazy rather than eager because a context that never draws an image
    /// should not allocate a sampler, a layout, and a one-pixel texture to sit
    /// unused -- and every context created by a test that only fills shapes is
    /// exactly that.
    descriptor_layout: Option<vk::DescriptorSetLayout>,
    /// Built on the first batch, like the sampling layout above it.
    material_layout: Option<vk::DescriptorSetLayout>,
    /// Fragment modules a caller registered, by the index they were given.
    ///
    /// Kept by the context rather than traveling with a batch because
    /// registering one leads to a pipeline, and a pipeline outlives every draw
    /// that uses it. The payload is kept rather than the module: a module is
    /// consumed by pipeline creation, and the same program may be needed again
    /// for a different blend, sample count or clip role.
    runtime_programs: Vec<Vec<u32>>,
    /// `minUniformBufferOffsetAlignment`, which sets the stride between one
    /// draw's paint and the next.
    uniform_alignment: u64,
    sampler: Option<vk::Sampler>,
    placeholder: Option<crate::resource::VulkanTexture>,
    /// Pool, view and set for the placeholder, owned by the context rather than
    /// by a submission so that a deferred submission may use it: a per-
    /// submission pool would have to survive until a fence the caller retires
    /// whenever it chooses.
    placeholder_binding: Option<(vk::DescriptorPool, vk::ImageView, vk::DescriptorSet)>,
    enabled_extensions: HashSet<String>,
    instance: ash::Instance,
    _entry: ash::Entry,
}

impl VulkanContext {
    /// Create a context, selecting a physical device by preference.
    pub fn new(preference: DevicePreference) -> Result<Self> {
        Self::with_config(ContextConfig {
            device: preference,
            validation: false,
            ..Default::default()
        })
    }

    /// Create a context with explicit configuration.
    pub fn with_config(config: ContextConfig) -> Result<Self> {
        let preference = config.device;
        // SAFETY: the loader is dlopened and the returned entry points are
        // used only for the lifetime of `entry`, which this struct owns.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| Error::Backend {
            backend: "vulkan",
            detail: format!("loader: {e}"),
        })?;

        let app_name =
            CString::new("emblema").expect("no interior NUL in an extension or layer name");
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .engine_name(&app_name)
            // 1.1 is the floor the project targets. Later features are used
            // when present rather than required.
            .api_version(vk::make_api_version(0, 1, 1, 0));

        // The layer and the debug-utils extension are only requested when both
        // are actually present, so asking for validation on a machine without
        // the SDK degrades to running without it rather than failing to start.
        let want_validation = config.validation
            && layer_available(&entry, VALIDATION_LAYER)
            && instance_extension_available(&entry, "VK_EXT_debug_utils");

        let layer_name =
            CString::new(VALIDATION_LAYER).expect("no interior NUL in an extension or layer name");
        let layer_ptrs: Vec<*const c_char> = if want_validation {
            vec![layer_name.as_ptr()]
        } else {
            Vec::new()
        };
        // Surface extensions are requested wherever the loader offers them, so
        // that a caller who later wants to present into a window finds the
        // instance already able to. Enabling one that goes unused costs
        // nothing; discovering it was missing costs recreating the instance,
        // and by then the device and every resource on it exist too.
        let mut instance_extensions: Vec<CString> = Vec::new();
        if want_validation {
            instance_extensions.push(
                CString::new("VK_EXT_debug_utils")
                    .expect("no interior NUL in an extension or layer name"),
            );
            // Carries the request for synchronization validation below. Its
            // absence is not fatal: the chained struct is then ignored and what
            // is lost is the extra checking rather than the instance.
            if layer_extension_available(&entry, VALIDATION_LAYER, "VK_EXT_validation_features") {
                instance_extensions.push(
                    CString::new("VK_EXT_validation_features")
                        .expect("no interior NUL in an extension or layer name"),
                );
            }
        }
        for &name in surface_extensions() {
            if instance_extension_available(&entry, name) {
                instance_extensions.push(
                    CString::new(name).expect("no interior NUL in an extension or layer name"),
                );
            }
        }
        let ext_ptrs: Vec<*const c_char> = instance_extensions.iter().map(|s| s.as_ptr()).collect();

        // Synchronization validation, which the layer does not do by default.
        // Core validation checks that each call is well formed; this checks
        // that one access is ordered against the next -- a missing barrier, a
        // read of an image the GPU has not finished writing. That is the class
        // of mistake this renderer is most exposed to, because it synchronizes
        // explicitly rather than through a driver that hides it, and the class
        // whose symptom is a correct picture on the device it was written on
        // and a wrong one elsewhere.
        // Asked of the layer, not of the loader. `VK_EXT_validation_features` is
        // the validation layer's own extension, so enumerating without naming
        // the layer does not find it -- which reads as "unavailable" on a
        // machine where it is installed and working.
        let sync_validation_available = want_validation
            && layer_extension_available(&entry, VALIDATION_LAYER, "VK_EXT_validation_features");
        // Synchronization only. Best-practices checking is worth running by
        // hand and was -- it is what found the barrier scopes -- but it is not
        // left on: much of its advice is vendor-specific, so a suite that
        // failed on it would fail on somebody else's GPU for reasons that are
        // not defects.
        let sync_validation = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
        let mut validation_features =
            vk::ValidationFeaturesEXT::default().enabled_validation_features(&sync_validation);

        let mut create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layer_ptrs)
            .enabled_extension_names(&ext_ptrs);
        if sync_validation_available {
            create_info = create_info.push_next(&mut validation_features);
        }
        let instance = unsafe { entry.create_instance(&create_info, None) }
            .map_err(|e| backend_err("create_instance", e))?;

        let validation_log = Arc::new(ValidationLog::default());
        let debug_messenger = if want_validation {
            let loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let info = validation::messenger_create_info()
                .user_data(Arc::as_ptr(&validation_log) as *mut std::ffi::c_void);
            match unsafe { loader.create_debug_utils_messenger(&info, None) } {
                Ok(m) => Some((loader, m)),
                Err(e) => {
                    unsafe { instance.destroy_instance(None) };
                    return Err(backend_err("create_debug_utils_messenger", e));
                }
            }
        } else {
            None
        };

        let physical_device = match select_physical_device(&instance, preference) {
            Ok(pd) => pd,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(e);
            }
        };

        Self::finish(
            entry,
            instance,
            physical_device,
            debug_messenger,
            validation_log,
            sync_validation_available,
            config.withheld,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        entry: ash::Entry,
        instance: ash::Instance,
        physical_device: vk::PhysicalDevice,
        debug_messenger: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
        validation_log: Arc<ValidationLog>,
        sync_validation: bool,
        withheld: emblema_hal::Withheld,
    ) -> Result<Self> {
        let mut available = match device_extensions(&instance, physical_device) {
            Ok(set) => set,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(e);
            }
        };

        // Request every extension the device offers that anything here can use.
        // Enabling one that goes unused costs nothing; discovering later that it
        // was not enabled costs a device recreation.
        let wanted = [
            ext::EXTERNAL_MEMORY_DMA_BUF,
            ext::EXTERNAL_MEMORY_FD,
            ext::IMAGE_DRM_FORMAT_MODIFIER,
            ext::EXTERNAL_FENCE_FD,
            ext::EXTERNAL_SEMAPHORE_FD,
            ext::PHYSICAL_DEVICE_DRM,
            ext::BLEND_OPERATION_ADVANCED,
            ext::SWAPCHAIN,
        ];
        // Taken out of what the device *offers*, before anything asks for it, so
        // everything downstream follows on its own: the probe below reports false
        // because the extension is not enabled, the device is created without the
        // feature it backs, and `enabled_extensions` -- which some paths consult
        // instead of the capability -- agrees with all of it.
        for capability in [
            emblema_hal::Capability::AdvancedBlend,
            emblema_hal::Capability::FloatRenderTargets,
        ] {
            if withheld.contains(capability) {
                for name in withheld_extensions(capability) {
                    available.remove(*name);
                }
            }
        }

        let enabled = resolve_extensions(&wanted, &available);
        let advanced_blend = probe_advanced_blend(&instance, physical_device, &enabled);

        let queue_family_index = match select_queue_family(&instance, physical_device) {
            Ok(i) => i,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(e);
            }
        };

        let enabled_cstrings: Vec<CString> = enabled
            .iter()
            .map(|s| CString::new(s.as_str()).expect("no interior NUL in an extension name"))
            .collect();
        let enabled_ptrs: Vec<*const c_char> =
            enabled_cstrings.iter().map(|s| s.as_ptr()).collect();

        let priorities = [1.0f32];
        let queue_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&priorities);
        let queue_infos = [queue_info];
        let mut device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&enabled_ptrs);

        // Coherent advanced blending has to be asked for at device creation.
        // Without it, overlapping draws in one pass need an explicit barrier
        // between them; the pipeline below refuses the modes outright unless
        // this came back enabled, so there is no path where a batch silently
        // reads a destination another draw has not finished writing.
        let mut advanced_features = vk::PhysicalDeviceBlendOperationAdvancedFeaturesEXT::default()
            .advanced_blend_coherent_operations(true);
        if advanced_blend {
            device_info = device_info.push_next(&mut advanced_features);
        }

        let device = match unsafe { instance.create_device(physical_device, &device_info, None) } {
            Ok(d) => d,
            Err(e) => {
                unsafe { instance.destroy_instance(None) };
                return Err(backend_err("create_device", e));
            }
        };

        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let mut capabilities =
            detect_capabilities(&instance, physical_device, &enabled, advanced_blend);
        // Covers what the subtraction above cannot: a capability read from a
        // format query rather than from an extension. Idempotent, so the fields
        // already false because their extension was left out stay false.
        withheld.apply_to(&mut capabilities);

        let allocator =
            gpu_allocator::vulkan::Allocator::new(&gpu_allocator::vulkan::AllocatorCreateDesc {
                instance: instance.clone(),
                device: device.clone(),
                physical_device,
                debug_settings: Default::default(),
                buffer_device_address: false,
                allocation_sizes: Default::default(),
            })
            .map_err(|e| Error::Backend {
                backend: "vulkan",
                detail: format!("allocator: {e}"),
            })?;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = unsafe { device.create_command_pool(&pool_info, None) }
            .map_err(|e| backend_err("create_command_pool", e))?;

        let uniform_alignment = unsafe { instance.get_physical_device_properties(physical_device) }
            .limits
            .min_uniform_buffer_offset_alignment;

        Ok(Self {
            debug_messenger,
            validation_log,
            sync_validation,
            allocator: Some(allocator),
            pipelines: PipelineCache::default(),
            command_pool,
            device,
            physical_device,
            queue,
            queue_family_index,
            capabilities,
            descriptor_layout: None,
            material_layout: None,
            runtime_programs: Vec::new(),
            uniform_alignment,
            sampler: None,
            placeholder: None,
            placeholder_binding: None,
            enabled_extensions: enabled,
            instance,
            _entry: entry,
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// This device's UUID, the stable sixteen bytes that name the GPU.
    ///
    /// What a host passes to [`DevicePreference::Uuid`] to come back to the same device, and what it
    /// compares against the UUID a compositor or another API reports -- the same bytes name the same
    /// GPU across processes, loaders and ICD order, which an enumeration index does not.
    ///
    /// Also how a host *confirms* it landed where it meant to, which matters because landing on the
    /// wrong GPU does not fail here: it fails later, at a dma-buf import, somewhere that cannot say
    /// why.
    #[must_use]
    pub fn device_uuid(&self) -> [u8; 16] {
        device_uuid(&self.instance, self.physical_device)
    }

    /// This device's DRM render node as `(major, minor)`, or `None` when it reports none.
    ///
    /// `None` for a driver without `VK_EXT_physical_device_drm`, and for one that has it and says it
    /// has no render node. A caller matching on a node cannot use either, so they are not
    /// distinguished -- see [`DevicePreference::RenderNode`].
    #[must_use]
    pub fn render_node(&self) -> Option<(i64, i64)> {
        device_render_node(&self.instance, self.physical_device)
    }

    /// The descriptor set layout every pipeline is built against.
    /// The layout for the set carrying a draw's paint.
    ///
    /// Lazy like the sampling layout beside it, and for the same reason: a
    /// context that never records a batch should build nothing.
    /// Register a caller's fragment program and return the name for it.
    ///
    /// Nothing is built here. A pipeline needs a render pass, a blend mode and
    /// a sample count that only a draw knows, so the payload is kept and the
    /// pipelines appear as the combinations actually used do.
    pub fn register_program(&mut self, program: &emblema_hal::RuntimeProgram) -> Result<u32> {
        if program.spirv.is_empty() {
            return Err(emblema_hal::Error::Unsupported(
                "a runtime program needs a SPIR-V fragment module for this backend",
            ));
        }
        // The same payload gives the same name back. See the trait's note:
        // a program leads to pipelines keyed by it, so a caller registering
        // one per frame would grow the cache without bound.
        if let Some(existing) = self
            .runtime_programs
            .iter()
            .position(|held| held.as_slice() == program.spirv.as_slice())
        {
            return Ok(existing as u32);
        }
        self.runtime_programs.push(program.spirv.clone());
        Ok((self.runtime_programs.len() - 1) as u32)
    }

    /// The payload a program was registered with.
    pub(crate) fn runtime_program(&self, id: u32) -> Result<&[u32]> {
        self.runtime_programs
            .get(id as usize)
            .map(Vec::as_slice)
            .ok_or(emblema_hal::Error::Unsupported(
                "a draw names a runtime program that was never registered",
            ))
    }

    pub(crate) fn material_layout(&mut self) -> Result<vk::DescriptorSetLayout> {
        if self.material_layout.is_none() {
            self.material_layout = Some(crate::materials::create_layout(&self.device)?);
        }
        Ok(self.material_layout.expect("just created"))
    }

    /// The smallest offset a dynamic uniform buffer binding may use.
    ///
    /// Read once at creation rather than per submission: it is a property of
    /// the device, and querying it inside the record path would mean a call
    /// into the loader for a number that cannot change.
    pub(crate) fn uniform_alignment(&self) -> u64 {
        self.uniform_alignment
    }

    pub(crate) fn descriptor_layout(&mut self) -> Result<vk::DescriptorSetLayout> {
        if self.descriptor_layout.is_none() {
            self.descriptor_layout = Some(crate::sampling::create_descriptor_layout(&self.device)?);
        }
        Ok(self.descriptor_layout.expect("just created"))
    }

    /// The one sampler every image draw uses.
    pub(crate) fn sampler(&mut self) -> Result<vk::Sampler> {
        if self.sampler.is_none() {
            self.sampler = Some(crate::sampling::create_sampler(&self.device)?);
        }
        Ok(self.sampler.expect("just created"))
    }

    /// A one-pixel opaque white texture, bound where a draw samples nothing.
    ///
    /// Left in a shader-readable layout for good: nothing writes to it after
    /// creation, so it needs no transition at any later point and cannot be
    /// caught in the wrong layout by a pass that happened to run first.
    ///
    /// White rather than transparent so that a bug binding it in place of a
    /// real texture shows up as a blank shape rather than as nothing at all.
    pub(crate) fn placeholder_texture(&mut self) -> Result<vk::Image> {
        if self.placeholder.is_none() {
            let mut texture = self.create_texture(&emblema_hal::TextureDescriptor::offscreen(
                emblema_hal::Extent2D::new(1, 1),
                emblema_hal::PixelFormat::Rgba8Unorm,
            ))?;
            self.write_texture(&mut texture, &[255, 255, 255, 255])?;

            let device = self.device.clone();
            let cmd = self.begin_one_shot()?;
            crate::resource::transition(
                &device,
                cmd,
                texture.raw_image(),
                texture.layout(),
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
            if let Err(e) = self.submit_one_shot(cmd) {
                self.destroy_texture(texture);
                return Err(e);
            }
            texture.set_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            self.placeholder = Some(texture);
        }
        // The image handle rather than the texture, so a caller holding it is
        // not also holding a borrow of the context it needs to keep using.
        Ok(self.placeholder.as_ref().expect("just created").raw_image())
    }

    /// The descriptor set bound where a draw samples nothing.
    /// The view of the one-pixel white texture, for a binding nothing fills.
    ///
    /// A layout declaring four images has to have four written whatever a draw
    /// reads, so the ones a program does not declare get this. It is the same
    /// view the placeholder set uses, made on the same first call.
    pub(crate) fn placeholder_view(&mut self) -> Result<vk::ImageView> {
        self.placeholder_set()?;
        Ok(self
            .placeholder_binding
            .expect("placeholder_set just made it")
            .1)
    }

    pub(crate) fn placeholder_set(&mut self) -> Result<vk::DescriptorSet> {
        if let Some((_, _, set)) = self.placeholder_binding {
            return Ok(set);
        }
        let image = self.placeholder_texture()?;
        let layout = self.descriptor_layout()?;
        let sampler = self.sampler()?;
        let binding = crate::sampling::create_placeholder_binding(
            &self.device.clone(),
            image,
            layout,
            sampler,
        )?;
        self.placeholder_binding = Some(binding);
        Ok(binding.2)
    }

    /// Whether the validation layer is actually installed and reporting.
    ///
    /// Requesting validation on a machine without the layer yields false here
    /// rather than an error, so tests can skip instead of failing on a machine
    /// that simply lacks the SDK.
    /// Whether the layer was asked to check synchronization as well as calls.
    ///
    /// Core validation checks that each call is well formed. Synchronization
    /// validation checks that one access is ordered against the next, which is
    /// a separate feature the layer does not enable by default -- and it is the
    /// one that matters most to a renderer that synchronizes explicitly, since
    /// a missing barrier renders correctly on the device it was written on.
    ///
    /// A caller asserting a clean log wants to know this: without it the log
    /// being empty says only that every call was well formed.
    pub fn sync_validation_active(&self) -> bool {
        self.sync_validation
    }

    pub fn validation_active(&self) -> bool {
        self.debug_messenger.is_some()
    }

    /// Everything the layer has reported for this context so far.
    pub fn validation_messages(&self) -> Vec<ValidationMessage> {
        self.validation_log.messages()
    }

    /// Whether the layer has reported no errors.
    pub fn validation_clean(&self) -> bool {
        self.validation_log.is_clean()
    }

    /// A handle on the log that outlives this context.
    ///
    /// Every other accessor here reads the log *through* the context, which
    /// cannot see the last thing the layer has to say. A child object that
    /// outlives its device is reported at `vkDestroyDevice`, which happens
    /// inside this context's own drop, and by then there is nothing left to
    /// ask -- so a whole class of fault was structurally invisible to every
    /// test rather than merely untested. Holding this across the drop is what
    /// makes teardown assertable.
    ///
    /// The messenger is destroyed after the device, deliberately, so the
    /// callback is still installed while those reports are made.
    pub fn validation_log(&self) -> Arc<ValidationLog> {
        Arc::clone(&self.validation_log)
    }

    pub fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    /// Whether a device extension was enabled at device creation.
    pub fn has_extension(&self, name: &str) -> bool {
        self.enabled_extensions.contains(name)
    }

    pub fn raw_device(&self) -> &ash::Device {
        &self.device
    }

    /// The loader, for a presentation target that needs an instance extension
    /// this crate does not itself use.
    pub fn raw_entry(&self) -> &ash::Entry {
        &self._entry
    }

    pub fn raw_instance(&self) -> &ash::Instance {
        &self.instance
    }

    pub fn raw_physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub fn raw_queue(&self) -> vk::Queue {
        self.queue
    }

    pub(crate) fn pipeline_cache(&self) -> &PipelineCache {
        &self.pipelines
    }

    pub(crate) fn pipeline_cache_mut(&mut self) -> &mut PipelineCache {
        &mut self.pipelines
    }

    pub(crate) fn allocator_mut(&mut self) -> &mut gpu_allocator::vulkan::Allocator {
        self.allocator
            .as_mut()
            .expect("allocator is taken only in Drop")
    }

    /// Submit a recorded command buffer with an exportable fence, without
    /// waiting.
    pub(crate) fn submit_exportable(
        &self,
        cmd: vk::CommandBuffer,
        framebuffer: vk::Framebuffer,
        view: vk::ImageView,
        sync: FrameSync<'_>,
    ) -> Result<crate::fence::VulkanFence> {
        unsafe { self.device.end_command_buffer(cmd) }
            .map_err(|e| backend_err("end_command_buffer", e))?;

        // The fence is plain: it is what decides when this submission's
        // resources may be released, and exporting would reset it.
        let info = vk::FenceCreateInfo::default();
        let fence = unsafe { self.device.create_fence(&info, None) }
            .map_err(|e| backend_err("create_fence", e))?;

        // The semaphore is the exportable half. Signaled by the same
        // submission, so its payload represents exactly the same completion,
        // and resetting it on export costs nothing because nothing else waits
        // on it.
        let can_export = self.has_extension(ext::EXTERNAL_SEMAPHORE_FD);
        let semaphore = if can_export {
            let mut export_info = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
            let info = vk::SemaphoreCreateInfo::default().push_next(&mut export_info);
            match unsafe { self.device.create_semaphore(&info, None) } {
                Ok(semaphore) => Some(semaphore),
                Err(e) => {
                    unsafe { self.device.destroy_fence(fence, None) };
                    return Err(backend_err("create_semaphore", e));
                }
            }
        } else {
            None
        };

        let cmds = [cmd];
        // The exportable semaphore and whatever the caller asked to signal are
        // both signaled by this one submission, so their payloads represent
        // exactly the same completion. A presentation target waits on its own;
        // a scanout path exports ours.
        let signals: Vec<vk::Semaphore> = semaphore
            .into_iter()
            .chain(sync.signal.iter().copied())
            .collect();
        // Color output is the only stage that touches the attachment, so
        // earlier stages may run before the wait is satisfied. Waiting at the
        // top of the pipe instead would serialize vertex work behind an image
        // the vertex stage never reads.
        let wait_stages: Vec<vk::PipelineStageFlags> =
            vec![vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT; sync.wait.len()];
        let submit = vk::SubmitInfo::default()
            .command_buffers(&cmds)
            .wait_semaphores(sync.wait)
            .wait_dst_stage_mask(&wait_stages)
            .signal_semaphores(&signals);
        if let Err(e) = unsafe { self.device.queue_submit(self.queue, &[submit], fence) } {
            unsafe {
                if let Some(semaphore) = semaphore {
                    self.device.destroy_semaphore(semaphore, None);
                }
                self.device.destroy_fence(fence, None);
            }
            return Err(backend_err("queue_submit", e));
        }

        let loader = can_export
            .then(|| ash::khr::external_semaphore_fd::Device::new(&self.instance, &self.device));
        Ok(crate::fence::VulkanFence::new(
            self.device.clone(),
            fence,
            self.command_pool,
            cmd,
            framebuffer,
            view,
            semaphore,
            loader,
        ))
    }

    /// Allocate and begin a command buffer for a single submission.
    pub(crate) fn begin_one_shot(&self) -> Result<vk::CommandBuffer> {
        let info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let buffers = unsafe { self.device.allocate_command_buffers(&info) }
            .map_err(|e| backend_err("allocate_command_buffers", e))?;
        let cmd = buffers[0];

        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { self.device.begin_command_buffer(cmd, &begin) }
            .map_err(|e| backend_err("begin_command_buffer", e))?;
        Ok(cmd)
    }

    /// End, submit, and wait for a one-shot command buffer, then free it.
    ///
    /// Blocking here is correct for setup and readback and wrong for the frame
    /// loop, which is fence-driven and never waits on the device. Nothing in
    /// this path runs per frame.
    pub(crate) fn submit_one_shot(&self, cmd: vk::CommandBuffer) -> Result<()> {
        let result = self.submit_and_wait(cmd);
        // SAFETY: the submission was waited on above, or never made, so the
        // buffer is no longer in use either way.
        unsafe { self.device.free_command_buffers(self.command_pool, &[cmd]) };
        result
    }

    fn submit_and_wait(&self, cmd: vk::CommandBuffer) -> Result<()> {
        unsafe { self.device.end_command_buffer(cmd) }
            .map_err(|e| backend_err("end_command_buffer", e))?;

        let fence_info = vk::FenceCreateInfo::default();
        let fence = unsafe { self.device.create_fence(&fence_info, None) }
            .map_err(|e| backend_err("create_fence", e))?;

        let cmds = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmds);
        let submitted = unsafe { self.device.queue_submit(self.queue, &[submit], fence) };

        let result = match submitted {
            Ok(()) => unsafe {
                self.device
                    .wait_for_fences(&[fence], true, u64::MAX)
                    .map_err(|e| match e {
                        vk::Result::TIMEOUT => Error::Timeout {
                            what: "a fence to signal",
                        },
                        vk::Result::ERROR_DEVICE_LOST => Error::DeviceLost,
                        other => backend_err("wait_for_fences", other),
                    })
            },
            Err(e) => Err(backend_err("queue_submit", e)),
        };

        unsafe { self.device.destroy_fence(fence, None) };
        result
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        // The allocator must free its memory while the device is still alive,
        // so it is dropped explicitly before anything else is destroyed.
        self.pipelines.destroy(&self.device);
        // Before the allocator, since the placeholder holds an allocation.
        if let Some(texture) = self.placeholder.take() {
            self.destroy_texture(texture);
        }
        // SAFETY: nothing is in flight, and no descriptor set still refers to
        // either of these -- every other pool is destroyed with the submission
        // that made it.
        unsafe {
            if let Some((pool, view, _)) = self.placeholder_binding.take() {
                self.device.destroy_descriptor_pool(pool, None);
                self.device.destroy_image_view(view, None);
            }
            if let Some(sampler) = self.sampler.take() {
                self.device.destroy_sampler(sampler, None);
            }
            // Both layouts, and they are separate fields because they are
            // separate sets: the texture and sampler in one, the material's
            // uniform block in the other. This one was missing for as long as
            // the second set has existed, and the leak it left was one layout
            // per device -- invisible to every test, since a context is
            // destroyed at the end of a process that is about to exit anyway,
            // and reported by the validation layer at `vkDestroyDevice` as a
            // child object outliving its parent.
            for layout in [self.descriptor_layout.take(), self.material_layout.take()]
                .into_iter()
                .flatten()
            {
                self.device.destroy_descriptor_set_layout(layout, None);
            }
        }
        drop(self.allocator.take());
        // SAFETY: every submission this context made was waited on before the
        // call that made it returned, so nothing is in flight. Objects are
        // destroyed inside-out: pool, then device, then the instance that
        // created it.
        unsafe {
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            // Before the instance, and before the log the callback points at.
            if let Some((loader, messenger)) = self.debug_messenger.take() {
                loader.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

impl std::fmt::Debug for VulkanContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanContext")
            .field("device", &self.capabilities.device_name)
            .field("driver", &self.capabilities.driver_name)
            .finish_non_exhaustive()
    }
}

fn layer_available(entry: &ash::Entry, name: &str) -> bool {
    let Ok(layers) = (unsafe { entry.enumerate_instance_layer_properties() }) else {
        return false;
    };
    layers.iter().any(|l| {
        // SAFETY: the loader guarantees a NUL-terminated name.
        unsafe { CStr::from_ptr(l.layer_name.as_ptr()) }
            .to_str()
            .map(|s| s == name)
            .unwrap_or(false)
    })
}

/// Whether a layer provides an instance extension.
///
/// Separate from the loader-wide query because a layer's own extensions are
/// only listed when that layer is named: asking the loader about one and
/// getting "no" says nothing about whether the layer has it.
fn layer_extension_available(entry: &ash::Entry, layer: &str, name: &str) -> bool {
    let Ok(layer_name) = CString::new(layer) else {
        return false;
    };
    let Ok(exts) = (unsafe { entry.enumerate_instance_extension_properties(Some(&layer_name)) })
    else {
        return false;
    };
    exts.iter().any(|e| {
        // SAFETY: the loader guarantees a NUL-terminated name.
        unsafe { CStr::from_ptr(e.extension_name.as_ptr()) }
            .to_str()
            .map(|s| s == name)
            .unwrap_or(false)
    })
}

fn instance_extension_available(entry: &ash::Entry, name: &str) -> bool {
    let Ok(exts) = (unsafe { entry.enumerate_instance_extension_properties(None) }) else {
        return false;
    };
    exts.iter().any(|e| {
        // SAFETY: the loader guarantees a NUL-terminated name.
        unsafe { CStr::from_ptr(e.extension_name.as_ptr()) }
            .to_str()
            .map(|s| s == name)
            .unwrap_or(false)
    })
}

fn backend_err(what: &str, e: vk::Result) -> Error {
    Error::Backend {
        backend: "vulkan",
        detail: format!("{what}: {e:?}"),
    }
}

fn device_extensions(instance: &ash::Instance, pd: vk::PhysicalDevice) -> Result<HashSet<String>> {
    let props = unsafe { instance.enumerate_device_extension_properties(pd) }
        .map_err(|e| backend_err("enumerate_device_extension_properties", e))?;
    Ok(props
        .iter()
        .filter_map(|p| {
            // SAFETY: the driver guarantees a NUL-terminated name.
            let raw = unsafe { CStr::from_ptr(p.extension_name.as_ptr()) };
            raw.to_str().ok().map(|s| s.to_owned())
        })
        .collect())
}

fn select_physical_device(
    instance: &ash::Instance,
    preference: DevicePreference,
) -> Result<vk::PhysicalDevice> {
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|e| backend_err("enumerate_physical_devices", e))?;
    if devices.is_empty() {
        return Err(Error::Backend {
            backend: "vulkan",
            detail: "no physical devices".into(),
        });
    }

    match preference {
        DevicePreference::Index(i) => devices.get(i).copied().ok_or(Error::Backend {
            backend: "vulkan",
            detail: format!("device index {i} out of range ({} present)", devices.len()),
        }),
        DevicePreference::Auto => Ok(*devices
            .iter()
            .max_by_key(|pd| {
                let props = unsafe { instance.get_physical_device_properties(**pd) };
                match props.device_type {
                    vk::PhysicalDeviceType::DISCRETE_GPU => 3,
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
                    vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
                    _ => 0,
                }
            })
            .expect("non-empty")),
        DevicePreference::Software => devices
            .iter()
            .find(|pd| {
                let props = unsafe { instance.get_physical_device_properties(**pd) };
                props.device_type == vk::PhysicalDeviceType::CPU
            })
            .copied()
            .ok_or(Error::Unsupported("no software rasterizer present")),
        // No match is an error rather than a fallback to `Auto`, for both of these. A host asking for
        // a particular GPU is asking because the wrong one does not work -- it shares frames with a
        // compositor on that device -- so quietly handing it another is worse than refusing: the
        // import fails later, somewhere that cannot say why.
        DevicePreference::Uuid(wanted) => devices
            .iter()
            .find(|pd| device_uuid(instance, **pd) == wanted)
            .copied()
            .ok_or_else(|| Error::Backend {
                backend: "vulkan",
                detail: format!(
                    "no device with uuid {}; {} present: {}",
                    hyphenated(&wanted),
                    devices.len(),
                    devices
                        .iter()
                        .map(|pd| hyphenated(&device_uuid(instance, *pd)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
        DevicePreference::RenderNode { major, minor } => devices
            .iter()
            .find(|pd| device_render_node(instance, **pd) == Some((major, minor)))
            .copied()
            .ok_or_else(|| Error::Backend {
                backend: "vulkan",
                detail: format!(
                    "no device with render node {major}:{minor}; {} present: {}",
                    devices.len(),
                    devices
                        .iter()
                        .map(|pd| match device_render_node(instance, *pd) {
                            Some((major, minor)) => format!("{major}:{minor}"),
                            None => "none".to_owned(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
    }
}

/// A device's UUID, the stable sixteen bytes that name it.
///
/// Core in Vulkan 1.1, which is what the instance asks for, so this needs no extension and cannot
/// fail: a driver that reported nothing would leave the zeroes the struct defaults to, which no real
/// device matches.
fn device_uuid(instance: &ash::Instance, pd: vk::PhysicalDevice) -> [u8; 16] {
    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
    unsafe { instance.get_physical_device_properties2(pd, &mut properties2) };
    id.device_uuid
}

/// A device's DRM render node as `(major, minor)`, or `None` when it has none to report.
///
/// `None` covers two different things on purpose, because a caller matching on a node cannot use
/// either: a driver without `VK_EXT_physical_device_drm`, and one that has it and says `has_render`
/// is false -- a device with only a primary node, which is not something to import across.
///
/// The extension is checked by name first. Chaining a struct the driver does not recognize leaves it
/// at its default, and a default of zero would read as render node `0:0` -- a match for a caller
/// unlucky enough to ask for it.
fn device_render_node(instance: &ash::Instance, pd: vk::PhysicalDevice) -> Option<(i64, i64)> {
    let available = device_extensions(instance, pd).ok()?;
    if !available.contains(ext::PHYSICAL_DEVICE_DRM) {
        return None;
    }
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
    unsafe { instance.get_physical_device_properties2(pd, &mut properties2) };
    (drm.has_render == vk::TRUE).then_some((drm.render_major, drm.render_minor))
}

/// A UUID in the usual 8-4-4-4-12 form, for a message a human has to compare against `vulkaninfo`.
fn hyphenated(uuid: &[u8; 16]) -> String {
    let hex: Vec<String> = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    let group = |range: core::ops::Range<usize>| hex[range].concat();
    format!(
        "{}-{}-{}-{}-{}",
        group(0..4),
        group(4..6),
        group(6..8),
        group(8..10),
        group(10..16)
    )
}

fn select_queue_family(instance: &ash::Instance, pd: vk::PhysicalDevice) -> Result<u32> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
    families
        .iter()
        .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .map(|i| i as u32)
        .ok_or(Error::Unsupported("no graphics queue family"))
}

fn detect_capabilities(
    instance: &ash::Instance,
    pd: vk::PhysicalDevice,
    enabled: &HashSet<String>,
    advanced_blend: bool,
) -> Capabilities {
    // Modifier queries need the extension enabled; without it the only honest
    // answer about layouts is that nothing is known.
    let modifiers_known = enabled.contains(ext::IMAGE_DRM_FORMAT_MODIFIER);
    let props = unsafe { instance.get_physical_device_properties(pd) };
    let limits = props.limits;

    // Sample counts a target can use are the intersection of what color and
    // depth attachments support. Reporting the color count alone would promise
    // a configuration that fails once depth is attached.
    let sample_mask =
        limits.framebuffer_color_sample_counts & limits.framebuffer_depth_sample_counts;

    // dma-buf export needs both the fd machinery and the dma-buf handle type;
    // either alone is useless. Negotiating a scanout-capable layout
    // additionally needs explicit modifiers.
    let fd = enabled.contains(ext::EXTERNAL_MEMORY_FD);
    let dma_buf = enabled.contains(ext::EXTERNAL_MEMORY_DMA_BUF);
    let modifiers = enabled.contains(ext::IMAGE_DRM_FORMAT_MODIFIER);

    // Every one of the four matters and the set is not arbitrary: a layer is
    // drawn into and composited, so it needs to be an attachment and to blend
    // as one; and it is then sampled, with a linear filter, by the pass above
    // it. A device offering three of the four cannot carry a floating-point
    // layer through the pipeline this renderer has.
    let wanted = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::SAMPLED_IMAGE
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR;
    let float_render_targets = unsafe {
        instance
            .get_physical_device_format_properties(pd, vk::Format::R16G16B16A16_SFLOAT)
            .optimal_tiling_features
            .contains(wanted)
    };

    Capabilities {
        advanced_blend,
        float_render_targets,
        max_texture_size: limits.max_image_dimension2_d,
        sample_counts: SampleCounts::from_mask(sample_mask.as_raw()),
        dma_buf: DmaBufSupport {
            import: fd && dma_buf,
            export: fd && dma_buf,
            modifiers,
        },
        sync: SyncSupport {
            // Export comes from a semaphore, not a fence: exporting a SYNC_FD
            // resets what it came from, and a reset fence can never be waited
            // on for retirement. So this reports the extension actually used.
            export_sync_file: enabled.contains(ext::EXTERNAL_SEMAPHORE_FD),
            // Import is the other direction — taking an out-fence from a
            // display commit and waiting on it — which is a fence operation.
            import_sync_file: enabled.contains(ext::EXTERNAL_FENCE_FD),
        },
        render_formats: if modifiers_known {
            crate::external::render_formats(instance, pd)
        } else {
            Vec::new()
        },
        device_name: device_name(&props),
        driver_name: format!("vulkan {}", api_version_string(props.api_version)),
        software: props.device_type == vk::PhysicalDeviceType::CPU,
    }
}

fn device_name(props: &vk::PhysicalDeviceProperties) -> String {
    // SAFETY: the driver guarantees a NUL-terminated name.
    unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn api_version_string(v: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(v),
        vk::api_version_minor(v),
        vk::api_version_patch(v)
    )
}

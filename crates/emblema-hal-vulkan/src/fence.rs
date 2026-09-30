//! Fences, and handing GPU completion to something outside this process.
//!
//! This is the second of the two requirements the HAL reserved from day one.
//! The DRM presentation path attaches the render-done fence to an atomic commit
//! as `IN_FENCE_FD`, so the kernel latches the page flip when rendering
//! finishes rather than the CPU waiting first and then committing. Losing that
//! costs a full frame of latency and puts a block in the middle of the frame
//! loop, which is exactly what explicit sync exists to avoid.
//!
//! Export comes from a semaphore rather than from the fence, and that is not a
//! stylistic choice. Exporting a `SYNC_FD` payload uses copy transference,
//! which resets the object it came from: a fence exported this way returns to
//! the unsignaled state with no pending work, so a later wait on it blocks
//! forever. Signaling both a fence and a semaphore from one submission keeps
//! the two jobs separate — the semaphore's payload is handed to the kernel, and
//! the fence stays intact for deciding when the submission's resources can be
//! released.
//!
//! Either way the object must be created exportable. Vulkan offers no way to
//! make one exportable after the fact, the same as for an exportable image.

use ash::vk;
use emblema_hal::{Error, HalFence, Result};
use std::time::Duration;

/// A submitted piece of work, and everything that must outlive it.
///
/// The command buffer and the transient framebuffer objects a pass built cannot
/// be released until the GPU is finished with them. A waiting submission can
/// free them as soon as it returns; a deferred one has to carry them until the
/// caller retires it, which is what this owns.
pub struct VulkanFence {
    device: ash::Device,
    fence: vk::Fence,
    pub(crate) command_pool: vk::CommandPool,
    pub(crate) command_buffer: vk::CommandBuffer,
    pub(crate) framebuffer: vk::Framebuffer,
    pub(crate) view: vk::ImageView,
    /// Signaled alongside the fence, and the thing actually exported.
    ///
    /// Separate from the fence because exporting resets what it came from, and
    /// resetting the fence would make retirement wait forever.
    semaphore: Option<vk::Semaphore>,
    /// Carried here rather than reached for through a context, because the HAL
    /// trait exports from a fence alone and a fence has no way back to one.
    export: Option<ash::khr::external_semaphore_fd::Device>,
    /// Geometry the submission is still reading.
    ///
    /// Held by the fence rather than by the context, because "still in use"
    /// is a property of one submission and the context may have several
    /// outstanding. A single list on the context is correct only while at most
    /// one frame is in flight; with two, retiring the older fence frees the
    /// newer frame's buffers out from under the GPU.
    pub(crate) retained: Vec<crate::render::StagedBuffer>,
    /// Descriptor sets and views for the textures the submission samples.
    ///
    /// Here for the same reason the framebuffer is: a descriptor pool cannot be
    /// destroyed while a command buffer that reads sets from it is in flight,
    /// and a deferred submission is in flight for as long as the caller likes.
    /// Held as an option because a submission that samples nothing needs no
    /// pool at all.
    ///
    /// The textures themselves stay with the caller rather than joining them
    /// here: a fence is handed to a page flip and so has to stay `Send`, and a
    /// texture tracks its own image layout in a cell.
    pub(crate) bindings: Option<crate::sampling::Bindings>,
    /// The paint set the submission reads, on the same terms as `bindings`:
    /// its pool cannot be destroyed while a command buffer using it is still
    /// in flight, and a deferred submission is in flight for as long as the
    /// caller likes.
    pub(crate) materials: Option<crate::materials::Materials>,
    retired: bool,
}

impl VulkanFence {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: ash::Device,
        fence: vk::Fence,
        command_pool: vk::CommandPool,
        command_buffer: vk::CommandBuffer,
        framebuffer: vk::Framebuffer,
        view: vk::ImageView,
        semaphore: Option<vk::Semaphore>,
        export: Option<ash::khr::external_semaphore_fd::Device>,
    ) -> Self {
        Self {
            device,
            fence,
            command_pool,
            command_buffer,
            framebuffer,
            view,
            semaphore,
            export,
            retained: Vec::new(),
            bindings: None,
            materials: None,
            retired: false,
        }
    }

    pub fn is_exportable(&self) -> bool {
        self.export.is_some() && self.semaphore.is_some()
    }

    /// Release everything, after waiting for the work to finish.
    ///
    /// Explicit rather than a `Drop` impl: the objects belong to a device this
    /// type only borrows, and dropping one silently while the GPU still reads
    /// its command buffer is exactly the corruption that is hard to reproduce.
    pub(crate) fn retire(&mut self) {
        if self.retired {
            return;
        }
        self.retired = true;
        // SAFETY: waiting first guarantees nothing below is still in use.
        unsafe {
            let _ = self.device.wait_for_fences(&[self.fence], true, u64::MAX);
            self.device.destroy_framebuffer(self.framebuffer, None);
            self.device.destroy_image_view(self.view, None);
            self.device
                .free_command_buffers(self.command_pool, &[self.command_buffer]);
            if let Some(semaphore) = self.semaphore.take() {
                self.device.destroy_semaphore(semaphore, None);
            }
            self.device.destroy_fence(self.fence, None);
        }
    }
}

impl HalFence for VulkanFence {
    fn is_signaled(&self) -> Result<bool> {
        // SAFETY: the fence belongs to this device and has not been destroyed;
        // retire is the only thing that destroys it and consumes the value.
        match unsafe { self.device.get_fence_status(self.fence) } {
            Ok(signaled) => Ok(signaled),
            Err(e) => Err(Error::Backend {
                backend: "vulkan",
                detail: format!("get_fence_status: {e:?}"),
            }),
        }
    }

    fn wait(&self, timeout: Duration) -> Result<bool> {
        let nanos = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: as above.
        let result = unsafe { self.device.wait_for_fences(&[self.fence], true, nanos) };
        match result {
            Ok(()) => Ok(true),
            // A timeout is an ordinary outcome for a frame slot still in
            // flight, so it is a value rather than an error.
            Err(vk::Result::TIMEOUT) => Ok(false),
            Err(vk::Result::ERROR_DEVICE_LOST) => Err(Error::DeviceLost),
            Err(e) => Err(Error::Backend {
                backend: "vulkan",
                detail: format!("wait_for_fences: {e:?}"),
            }),
        }
    }

    #[cfg(unix)]
    fn export_sync_file(&self) -> Result<std::os::fd::OwnedFd> {
        use std::os::fd::FromRawFd;

        let (Some(loader), Some(semaphore)) = (&self.export, self.semaphore) else {
            return Err(Error::Unsupported(
                "this submission has no exportable payload",
            ));
        };

        let info = vk::SemaphoreGetFdInfoKHR::default()
            .semaphore(semaphore)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        // SAFETY: the semaphore was created with a matching export handle type
        // and has a pending signal operation from the submission.
        let raw = unsafe { loader.get_semaphore_fd(&info) }.map_err(|e| Error::Backend {
            backend: "vulkan",
            detail: format!("get_semaphore_fd: {e:?}"),
        })?;

        // A sync_file export returns -1 when the work has already completed:
        // there is nothing left to wait on, so the kernel represents it as no
        // descriptor at all. Legitimate, but not something a caller can attach
        // to a commit, so it is reported rather than passed on as a handle.
        if raw < 0 {
            return Err(Error::Unsupported(
                "the fence had already signaled, so there is no sync_file to export",
            ));
        }

        // Ownership transfers here: the driver returns a fresh descriptor and
        // closing it becomes the caller's responsibility.
        // SAFETY: the driver returned a fresh, owned descriptor.
        Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) })
    }
}

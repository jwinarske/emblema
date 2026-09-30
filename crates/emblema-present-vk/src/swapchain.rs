//! Presenting through a `VkSwapchainKHR`.
//!
//! # Where the surface comes from
//!
//! A caller brings one. Creating windows is no more this project's business
//! than mode setting is: an application already has a window system connection
//! and a window, and asking it for the surface those imply is one call, where
//! owning that relationship would mean owning a windowing library.
//!
//! That boundary is also what makes this testable. `VK_EXT_headless_surface`
//! produces a surface with no window behind it, so the whole path — capability
//! query, format negotiation, present mode selection, acquire, present, and
//! recreation on resize — runs on a machine with no display, and every test
//! here does exactly that.
//!
//! # Synchronization
//!
//! Acquisition signals a semaphore, rendering waits on it and signals another,
//! and presentation waits on that. All three orderings are device-side: no
//! thread blocks to enforce any of them, which is the point — a CPU wait
//! between acquiring and drawing costs a frame of latency every frame.
//!
//! One CPU wait remains, and it is the one that should. Before reusing a
//! frame's semaphores and command resources, the frame that last used them has
//! to have finished. Waiting on that is what bounds how far ahead of the
//! display the renderer may run; without it the queue grows without limit and
//! latency with it. It normally does not block at all, because the frame being
//! waited on is two frames old.
//!
//! Two semaphores per frame is not enough, and this is the detail worth being
//! careful about. The render-finished semaphore must be per *image* rather than
//! per frame slot, because presentation waits on it and the engine decides when
//! it is done with an image — reusing one while a present still refers to it is
//! a wait on a payload that has already been consumed.

use ash::vk;
use emblema_hal::{Error, Extent2D, HalFence, PixelFormat, Result, FRAME_WAIT_TIMEOUT};
use emblema_hal_vulkan::{VulkanContext, VulkanFence, VulkanHal, VulkanTexture};
use emblema_present::PresentTarget;

/// How the presentation engine should pace frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresentMode {
    /// Queue frames and display them on refresh, never tearing and never
    /// dropping. The only mode a conformant implementation must offer, and the
    /// right default: it is the one that does not burn power drawing frames
    /// nobody sees.
    #[default]
    Fifo,
    /// Replace an undisplayed queued frame with a newer one.
    ///
    /// Lower latency at the cost of drawing frames that are discarded. Falls
    /// back to `Fifo` where the surface does not offer it, since it is
    /// optional and asking for it should not fail outright.
    Mailbox,
}

impl PresentMode {
    fn to_vk(self) -> vk::PresentModeKHR {
        match self {
            Self::Fifo => vk::PresentModeKHR::FIFO,
            Self::Mailbox => vk::PresentModeKHR::MAILBOX,
        }
    }
}

/// One acquired-but-not-presented frame.
struct Acquired {
    index: u32,
    /// Which slot's semaphores and fence this frame is using.
    slot: usize,
    /// Whether the frame was drawn through [`SwapchainTarget::submit`].
    submitted: bool,
}

/// How many frames may be recorded before one must have finished.
///
/// Two: one being displayed, one being drawn. A third adds latency for
/// throughput the renderer does not need, since a frame is submitted as a whole
/// rather than trickled out.
const FRAMES_IN_FLIGHT: usize = 2;

/// Per-frame synchronization, reused round-robin.
struct FrameSlot {
    /// Signaled by acquisition, waited on by the render.
    acquired: vk::Semaphore,
    /// The render submitted with this slot, if it has not been retired.
    ///
    /// Held rather than waited on immediately: retiring releases the geometry
    /// buffers and framebuffer that submission used, and doing that here rather
    /// than in the target is what keeps the resources alive exactly as long as
    /// the GPU is reading them.
    in_flight: Option<VulkanFence>,
    /// Layer targets the in-flight submission samples.
    ///
    /// Here rather than on the fence because a fence is handed to a page flip
    /// and so has to stay `Send`, while a texture tracks its own image layout
    /// in a cell. The lifetime rule is the same either way: released when the
    /// submission that reads them retires.
    /// Layer targets and baked gradients this frame owns, freed when its
    /// work is done. What each one was stops mattering once the submission
    /// is made; when it is safe to free is the same answer for all of them.
    retained: Vec<emblema_hal_vulkan::VulkanTexture>,
}

/// A swapchain, its images, and the frame in flight.
pub struct SwapchainTarget {
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    loader: ash::khr::swapchain::Device,
    surface_loader: ash::khr::surface::Instance,
    images: Vec<VulkanTexture>,
    /// Signaled when an image's render is done, waited on by presentation.
    ///
    /// One per image rather than per frame slot: presentation waits on it and
    /// the engine decides when it is finished with the image, so a semaphore
    /// reused while a present still refers to it would be waited on twice for
    /// one signal.
    rendered: Vec<vk::Semaphore>,
    slots: Vec<FrameSlot>,
    slot: usize,
    acquired: Option<Acquired>,
    extent: Extent2D,
    format: PixelFormat,
    vk_format: vk::Format,
    color_space: vk::ColorSpaceKHR,
    mode: PresentMode,
    presented: u64,
    cpu_waits: u64,
    /// Set when the engine reported the swapchain no longer matches its
    /// surface, so the next acquisition rebuilds before trying.
    stale: bool,
}

impl SwapchainTarget {
    /// Build a swapchain for a surface the caller created.
    ///
    /// The surface is borrowed, not adopted: it outlives this and is the
    /// caller's to destroy, because it was theirs to create and they may build
    /// another swapchain on it after this one is gone.
    ///
    /// `extent` is the size to ask for. A windowed surface dictates its own and
    /// ignores this; a surface that defers — a headless one does — takes it,
    /// clamped to what it allows. There is no useful default: deferring
    /// surfaces clamp an unstated size up from zero to the minimum, which is
    /// how a swapchain ends up one pixel across and every test of it still
    /// passes.
    pub fn new(
        ctx: &mut VulkanContext,
        surface: vk::SurfaceKHR,
        extent: Extent2D,
        mode: PresentMode,
    ) -> Result<Self> {
        let surface_loader = ash::khr::surface::Instance::new(ctx.raw_entry(), ctx.raw_instance());
        let loader = ash::khr::swapchain::Device::new(ctx.raw_instance(), ctx.raw_device());

        // A device that can render is not necessarily one that can present to
        // this surface: a headless card, or a second GPU with no connector.
        // Asking is the only way to know, and the answer is a capability rather
        // than something to infer from the device's identity.
        let supported = unsafe {
            surface_loader.get_physical_device_surface_support(
                ctx.raw_physical_device(),
                ctx.queue_family_index(),
                surface,
            )
        }
        .map_err(|e| backend_err("get_physical_device_surface_support", e))?;
        if !supported {
            return Err(Error::Unsupported(
                "this device's queue cannot present to this surface",
            ));
        }

        let mut slots = Vec::with_capacity(FRAMES_IN_FLIGHT);
        for _ in 0..FRAMES_IN_FLIGHT {
            let info = vk::SemaphoreCreateInfo::default();
            match unsafe { ctx.raw_device().create_semaphore(&info, None) } {
                Ok(acquired) => slots.push(FrameSlot {
                    acquired,
                    in_flight: None,
                    retained: Vec::new(),
                }),
                Err(e) => {
                    for slot in slots {
                        unsafe { ctx.raw_device().destroy_semaphore(slot.acquired, None) };
                    }
                    return Err(backend_err("create_semaphore", e));
                }
            }
        }

        let mut target = Self {
            surface,
            swapchain: vk::SwapchainKHR::null(),
            loader,
            surface_loader,
            images: Vec::new(),
            rendered: Vec::new(),
            slots,
            slot: 0,
            acquired: None,
            extent: Extent2D::new(0, 0),
            format: PixelFormat::Bgra8Unorm,
            vk_format: vk::Format::B8G8R8A8_UNORM,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            mode,
            presented: 0,
            cpu_waits: 0,
            stale: false,
        };
        if let Err(e) = target.rebuild(ctx, Some(extent)) {
            target.release_slots(ctx);
            return Err(e);
        }
        Ok(target)
    }

    /// Frames handed to the presentation engine so far.
    pub fn presented_frames(&self) -> u64 {
        self.presented
    }

    /// How many times a frame blocked waiting for an older frame to finish.
    ///
    /// This is the wait that bounds how far ahead of the display the renderer
    /// may run, so a nonzero count is the mechanism working rather than a
    /// defect. It is reported because the number climbing to one per frame
    /// means the GPU has become the limit, which is worth being able to see
    /// without a profiler.
    ///
    /// Nothing waits between acquiring an image and drawing into it, or between
    /// drawing and presenting: both of those are semaphores.
    pub fn throttle_waits(&self) -> u64 {
        self.cpu_waits
    }

    /// How many images the engine gave us.
    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    pub fn present_mode(&self) -> PresentMode {
        self.mode
    }

    /// The image acquired for this frame, for reading back what was drawn.
    ///
    /// A presented frame is not observable without a display, so this is how a
    /// test — or a screenshot — sees one. It reads the image the caller drew
    /// into rather than anything the engine has shown.
    pub fn acquired_image(&mut self) -> Result<&mut VulkanTexture> {
        let index = self
            .acquired
            .as_ref()
            .ok_or(Error::Unsupported("no frame is acquired"))?
            .index as usize;
        Ok(&mut self.images[index])
    }

    /// Rebuild the swapchain, keeping the old one alive during the transition.
    ///
    /// `requested` is the extent to ask for, or `None` to take whatever the
    /// surface reports. A surface that dictates its size — every windowed one
    /// does — ignores the request, which is why the extent is read back
    /// afterwards rather than assumed.
    fn rebuild(&mut self, ctx: &mut VulkanContext, requested: Option<Extent2D>) -> Result<()> {
        let capabilities = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(ctx.raw_physical_device(), self.surface)
        }
        .map_err(|e| backend_err("get_physical_device_surface_capabilities", e))?;

        let formats = unsafe {
            self.surface_loader
                .get_physical_device_surface_formats(ctx.raw_physical_device(), self.surface)
        }
        .map_err(|e| backend_err("get_physical_device_surface_formats", e))?;
        let (vk_format, color_space, format) = choose_format(&formats)?;

        let modes = unsafe {
            self.surface_loader
                .get_physical_device_surface_present_modes(ctx.raw_physical_device(), self.surface)
        }
        .map_err(|e| backend_err("get_physical_device_surface_present_modes", e))?;
        // FIFO is required of every implementation, so falling back to it
        // always succeeds; asking for mailbox where it is absent degrades
        // rather than failing, since it is a latency preference and not a
        // correctness requirement.
        let wanted = self.mode.to_vk();
        let present_mode = if modes.contains(&wanted) {
            wanted
        } else {
            vk::PresentModeKHR::FIFO
        };

        let extent = resolve_extent(&capabilities, requested.unwrap_or(self.extent));
        if extent.width == 0 || extent.height == 0 {
            // A minimized window reports a zero extent, and a swapchain of that
            // size is invalid. Reporting rather than creating one lets a frame
            // loop skip the frame, which is what it would have to do anyway.
            return Err(Error::Unsupported(
                "the surface has no area; nothing can be presented into it",
            ));
        }

        // One more than the minimum, so the engine always has an image to
        // display while the application draws into another. Clamped to the
        // maximum, which is zero when there is none.
        let mut count = capabilities.min_image_count + 1;
        if capabilities.max_image_count > 0 {
            count = count.min(capabilities.max_image_count);
        }

        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(count)
            .image_format(vk_format)
            .image_color_space(color_space)
            .image_extent(vk::Extent2D {
                width: extent.width,
                height: extent.height,
            })
            .image_array_layers(1)
            // Transfer destination as well as attachment, because reading a
            // presented frame back is how a test knows what was shown, and a
            // clear of an empty batch goes through a transfer.
            .image_usage(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(choose_composite_alpha(&capabilities))
            .present_mode(present_mode)
            // Pixels the window system obscures may be undefined, which is what
            // lets the engine skip rendering them. Nothing here reads a
            // presented image except through an explicit readback in a test,
            // and that reads the image it just drew rather than a displayed one.
            .clipped(true)
            .old_swapchain(self.swapchain);

        let swapchain = unsafe { self.loader.create_swapchain(&info, None) }
            .map_err(|e| backend_err("create_swapchain", e))?;

        // The old swapchain is retired by creating the new one but still has to
        // be destroyed, and only after nothing is using its images. That is the
        // one place a full stall is unavoidable: the alternative is freeing an
        // image the GPU is drawing into.
        self.drain_in_flight(ctx);
        self.release_images(ctx);
        if self.swapchain != vk::SwapchainKHR::null() {
            unsafe { self.loader.destroy_swapchain(self.swapchain, None) };
        }
        self.swapchain = swapchain;

        let images = unsafe { self.loader.get_swapchain_images(swapchain) }
            .map_err(|e| backend_err("get_swapchain_images", e))?;
        self.images = images
            .into_iter()
            // Undefined is right: an image the engine has just handed over
            // holds nothing anybody may read, and every pass either clears it
            // or transitions from whatever it left behind.
            .map(|image| {
                VulkanTexture::wrap_image(image, extent, format, vk::ImageLayout::UNDEFINED)
            })
            .collect();

        self.rendered.clear();
        for _ in 0..self.images.len() {
            let info = vk::SemaphoreCreateInfo::default();
            let semaphore = unsafe { ctx.raw_device().create_semaphore(&info, None) }
                .map_err(|e| backend_err("create_semaphore", e))?;
            self.rendered.push(semaphore);
        }

        self.extent = extent;
        self.format = format;
        self.vk_format = vk_format;
        self.color_space = color_space;
        self.stale = false;
        Ok(())
    }

    fn release_images(&mut self, ctx: &mut VulkanContext) {
        for texture in self.images.drain(..) {
            // Releases the wrapper and nothing else: the engine owns these.
            ctx.destroy_texture(texture);
        }
        // SAFETY: every submission that signaled one of these was waited on
        // by `drain_in_flight` before this runs.
        for semaphore in self.rendered.drain(..) {
            unsafe { ctx.raw_device().destroy_semaphore(semaphore, None) };
        }
        self.acquired = None;
    }

    /// Wait for everything still referring to this target's objects.
    ///
    /// Called before anything a submission or a presentation might still be
    /// using is destroyed. A real stall, and it happens only where the
    /// alternative is freeing an image the GPU is drawing into or a semaphore a
    /// present is waiting on: rebuilding and shutting down.
    ///
    /// Waiting on the render fences alone is not enough, and the validation
    /// layer is what says so. A fence covers the submission that signaled it;
    /// presentation is a separate queue operation that consumes the
    /// render-finished semaphore, and nothing tells us when it has. Idling the
    /// queue is what covers both.
    fn drain_in_flight(&mut self, ctx: &mut VulkanContext) {
        for index in 0..self.slots.len() {
            if let Some(fence) = self.slots[index].in_flight.take() {
                let _ = fence.wait(FRAME_WAIT_TIMEOUT);
                ctx.retire_fence(fence);
            }
            // After the fence, since the submission was sampling them.
            for layer in std::mem::take(&mut self.slots[index].retained) {
                ctx.destroy_texture(layer);
            }
        }
        // SAFETY: no other thread submits to this queue.
        unsafe {
            let _ = ctx.raw_device().queue_wait_idle(ctx.raw_queue());
        }
    }

    fn release_slots(&mut self, ctx: &mut VulkanContext) {
        self.drain_in_flight(ctx);
        for slot in self.slots.drain(..) {
            // SAFETY: nothing is waiting on these; every submission that used
            // one has completed.
            unsafe { ctx.raw_device().destroy_semaphore(slot.acquired, None) };
        }
    }

    /// Draw a batch into the acquired image, ordered by this frame's semaphores.
    ///
    /// **The way to draw a frame for this target.** Presentation needs the
    /// render to wait on acquisition and to signal something presentation can
    /// wait on in turn, and neither is expressible through the
    /// backend-agnostic submission: that one waits for completion before it
    /// returns, which would put a stall exactly where these semaphores exist to
    /// remove one, and would leave acquisition's semaphore signaled and never
    /// waited on.
    ///
    /// The scanout target asks the same of its callers for the same reason.
    /// Presenting a frame drawn any other way is refused rather than
    /// half-synchronized.
    pub fn submit(
        &mut self,
        ctx: &mut VulkanContext,
        batch: &emblema_hal::Batch,
        pass: emblema_hal::PassDescriptor,
    ) -> Result<()> {
        self.submit_textured(ctx, batch, pass, &[])
    }

    /// Render a recording and submit its root pass for display.
    ///
    /// This is the call a frame loop wants, and until it existed a frame with
    /// layers could not be presented at all: the swapchain took a batch, and a
    /// recording with layers is several. The layer passes are rendered first,
    /// each into a target of its own, through the waiting submission — they are
    /// prerequisites of the root rather than part of the frame's pacing, and
    /// waiting for them is what makes them ordered before it without a
    /// semaphore per layer. Only the root goes through the synchronized path,
    /// which is right: the acquire and present semaphores are about the pass
    /// that touches the presentable image, and that is exactly the root.
    ///
    /// The layer targets stay with this slot until its fence retires, since the
    /// submission is still sampling them.
    pub fn submit_recording(
        &mut self,
        ctx: &mut VulkanContext,
        recording: &emblema_core::Recording,
        images: &[&emblema_hal_vulkan::VulkanTexture],
    ) -> Result<()> {
        // The swapchain image is the root this frame lands in, so its format is
        // what a layer has to be able to hold.
        let transient = emblema_core::execute_layers::<emblema_hal_vulkan::VulkanHal>(
            ctx,
            recording,
            images,
            self.format.intermediate(),
        )?;
        let root = recording.root();
        let outcome = emblema_core::resolve_sources::<emblema_hal_vulkan::VulkanHal>(
            root, images, &transient,
        )
        .and_then(|table| self.submit_textured(ctx, &root.batch, root.descriptor, &table));

        // Kept whichever way the submission went. On success the fence is
        // reading them; on failure there is no fence, and dropping them here
        // rather than leaking is what the slot's own release does anyway.
        let slot = self.acquired.as_ref().map(|a| a.slot);
        match slot {
            Some(slot) if outcome.is_ok() => {
                self.slots[slot].retained.extend(transient.into_textures())
            }
            _ => transient.destroy(ctx),
        }
        outcome
    }

    /// The same, for a batch whose materials sample textures.
    ///
    /// What a frame with layers needs: the pass that lands in the presentable
    /// image is the one that composites them, so it samples the targets they
    /// were rendered into. Those must outlive this submission, and the caller
    /// keeps them until the fence for this slot retires.
    pub fn submit_textured(
        &mut self,
        ctx: &mut VulkanContext,
        batch: &emblema_hal::Batch,
        pass: emblema_hal::PassDescriptor,
        textures: &[&emblema_hal_vulkan::VulkanTexture],
    ) -> Result<()> {
        let Some(acquired) = self.acquired.as_ref() else {
            return Err(Error::Unsupported("submit without a matching acquire"));
        };
        let (index, slot) = (acquired.index as usize, acquired.slot);

        let wait = [self.slots[slot].acquired];
        let signal = [self.rendered[index]];
        let fence = ctx.submit_batch_deferred_synchronized(
            &mut self.images[index],
            batch,
            pass,
            textures,
            emblema_hal_vulkan::FrameSync {
                wait: &wait,
                signal: &signal,
                // The render pass leaves the image ready to present, so
                // nothing has to transition it afterwards -- and nothing
                // could, without a second submission that no semaphore orders
                // after this one.
                presents: true,
            },
        )?;
        self.slots[slot].in_flight = Some(fence);
        if let Some(acquired) = self.acquired.as_mut() {
            acquired.submitted = true;
        }
        Ok(())
    }
}

impl PresentTarget<VulkanHal> for SwapchainTarget {
    fn extent(&self) -> Extent2D {
        self.extent
    }

    fn format(&self) -> PixelFormat {
        self.format
    }

    fn acquire(&mut self, ctx: &mut VulkanContext) -> Result<&mut VulkanTexture> {
        if self.acquired.is_some() {
            return Err(Error::Unsupported(
                "acquire without a matching present; one frame is already in flight",
            ));
        }
        if self.stale {
            self.rebuild(ctx, None)?;
        }

        // Before reusing this slot's semaphores, the frame that last used them
        // has to have finished. This is what bounds how far ahead of the
        // display the renderer runs, and it normally does not block: the frame
        // being waited on is two frames old.
        let slot = self.slot;
        if let Some(fence) = self.slots[slot].in_flight.take() {
            self.cpu_waits += 1;
            fence.wait(FRAME_WAIT_TIMEOUT)?;
            ctx.retire_fence(fence);
        }
        // Whatever that frame composited is finished with, and this is the
        // first moment it is safe to say so.
        for layer in std::mem::take(&mut self.slots[slot].retained) {
            ctx.destroy_texture(layer);
        }

        // Two attempts at most: the first may find the swapchain out of date,
        // and the rebuild that follows produces one that matches the surface,
        // so a second failure is a real error rather than a race.
        for attempt in 0..2 {
            let result = unsafe {
                self.loader.acquire_next_image(
                    self.swapchain,
                    FRAME_WAIT_TIMEOUT.as_nanos() as u64,
                    self.slots[slot].acquired,
                    vk::Fence::null(),
                )
            };
            match result {
                // Suboptimal still produced an image, and refusing it would
                // drop a frame that is merely imperfect. The rebuild happens
                // on the next acquisition instead.
                Ok((index, suboptimal)) => {
                    self.stale = suboptimal;
                    // No wait here at all: the render will wait on the
                    // semaphore acquisition signals, on the device.
                    self.acquired = Some(Acquired {
                        index,
                        slot,
                        submitted: false,
                    });
                    self.slot = (slot + 1) % self.slots.len();
                    return Ok(&mut self.images[index as usize]);
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) if attempt == 0 => {
                    self.rebuild(ctx, None)?;
                }
                Err(e) => return Err(backend_err("acquire_next_image", e)),
            }
        }
        Err(Error::Unsupported(
            "the swapchain went out of date twice in a row",
        ))
    }

    fn present(&mut self, ctx: &mut VulkanContext) -> Result<()> {
        let Some((index, submitted)) = self
            .acquired
            .as_ref()
            .map(|a| (a.index as usize, a.submitted))
        else {
            return Err(Error::Unsupported("present without a matching acquire"));
        };

        if !submitted {
            // Acquisition signaled a semaphore that nothing waited on, and
            // presentation is about to wait on one nothing signaled. Neither
            // is recoverable here, and presenting anyway would hang rather than
            // merely look wrong.
            // The frame stays acquired: it can still be drawn properly and
            // presented, which is more useful than forcing the caller to
            // reacquire an image that is already theirs.
            return Err(Error::Unsupported(
                "this frame was not drawn through SwapchainTarget::submit",
            ));
        }
        self.acquired = None;

        // Normally nothing: the render pass already left the image ready to
        // present, which is the whole point of asking it to. It matters where
        // something else touched the image in between -- reading it back
        // leaves it in a transfer layout -- and there a transition is both
        // needed and safely ordered, since a readback waits for completion.
        ctx.transition_for_present(&self.images[index])?;

        let swapchains = [self.swapchain];
        let indices = [index as u32];
        // Waits on the render, on the device: the engine will not read the
        // image before the draw that filled it has completed, and no thread
        // blocks to arrange that.
        let wait = [self.rendered[index]];
        let info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait)
            .swapchains(&swapchains)
            .image_indices(&indices);

        let result = unsafe { self.loader.queue_present(ctx.raw_queue(), &info) };
        match result {
            Ok(suboptimal) => {
                self.stale |= suboptimal;
                self.presented += 1;
                Ok(())
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                // The frame is lost, which is not an error: the surface changed
                // under it. The next acquisition rebuilds.
                self.stale = true;
                Ok(())
            }
            Err(e) => Err(backend_err("queue_present", e)),
        }
    }

    fn reconfigure(&mut self, ctx: &mut VulkanContext, extent: Extent2D) -> Result<()> {
        self.rebuild(ctx, Some(extent))
    }

    fn destroy(mut self, ctx: &mut VulkanContext) {
        self.release_slots(ctx);
        self.release_images(ctx);
        // SAFETY: every frame in flight was waited on above, so nothing is
        // reading the swapchain or its images.
        unsafe {
            if self.swapchain != vk::SwapchainKHR::null() {
                self.loader.destroy_swapchain(self.swapchain, None);
            }
        }
        // The surface is the caller's, and stays.
    }
}

/// Pick a surface format, preferring one the renderer already speaks.
///
/// A surface offering `UNDEFINED` alone means it has no preference, which is
/// what a headless surface reports on some drivers; there the choice is ours
/// outright.
fn choose_format(
    formats: &[vk::SurfaceFormatKHR],
) -> Result<(vk::Format, vk::ColorSpaceKHR, PixelFormat)> {
    if formats.is_empty() {
        return Err(Error::Unsupported("the surface offers no formats"));
    }
    if formats.len() == 1 && formats[0].format == vk::Format::UNDEFINED {
        return Ok((
            vk::Format::B8G8R8A8_UNORM,
            vk::ColorSpaceKHR::SRGB_NONLINEAR,
            PixelFormat::Bgra8Unorm,
        ));
    }

    // Unsigned normalized first, and *not* the sRGB variants, which is the
    // opposite of what this preferred while the renderer worked in light.
    //
    // The color space asked for below is `SRGB_NONLINEAR`, which tells the
    // presentation engine the image already holds sRGB-encoded values -- and it
    // does, because that is what the pipeline carries from the API boundary all
    // the way to this write. An sRGB *format* would encode them a second time,
    // putting every window a little over a third too bright at mid gray. A
    // plain format hands the engine exactly the numbers it was promised.
    //
    // This is what upstream presents to as well: its render targets are
    // `kB8G8R8A8UNormInt` and `kR8G8B8A8UNormInt`, and the sRGB members of its
    // format enum are reached by blits and image formats rather than by
    // anything it draws into.
    //
    // The sRGB formats stay last rather than being dropped, because a surface
    // offering nothing else is better served too brightly than not at all.
    let wanted = [
        (vk::Format::B8G8R8A8_UNORM, PixelFormat::Bgra8Unorm),
        (vk::Format::R8G8B8A8_UNORM, PixelFormat::Rgba8Unorm),
        (vk::Format::B8G8R8A8_SRGB, PixelFormat::Bgra8UnormSrgb),
        (vk::Format::R8G8B8A8_SRGB, PixelFormat::Rgba8UnormSrgb),
    ];
    for (vk_format, format) in wanted {
        if let Some(found) = formats
            .iter()
            .find(|f| f.format == vk_format && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        {
            return Ok((found.format, found.color_space, format));
        }
    }
    Err(Error::Unsupported(
        "the surface offers no format this renderer can target",
    ))
}

/// The extent to build for, given what the surface allows.
///
/// A surface reporting a current extent of `u32::MAX` is saying the swapchain
/// decides — that is how a surface without a fixed size answers — so the
/// request is used, clamped to the allowed range. Anything else dictates, and
/// the request is ignored: asking a window for a size it did not offer produces
/// an invalid swapchain rather than a resized window.
fn resolve_extent(capabilities: &vk::SurfaceCapabilitiesKHR, requested: Extent2D) -> Extent2D {
    if capabilities.current_extent.width != u32::MAX {
        return Extent2D::new(
            capabilities.current_extent.width,
            capabilities.current_extent.height,
        );
    }
    Extent2D::new(
        requested.width.clamp(
            capabilities.min_image_extent.width,
            capabilities.max_image_extent.width,
        ),
        requested.height.clamp(
            capabilities.min_image_extent.height,
            capabilities.max_image_extent.height,
        ),
    )
}

/// How the surface's alpha meets the desktop behind it.
///
/// Opaque where it is offered, which is what a window without a transparency
/// request wants, and the only mode every implementation is likely to have.
/// Inherit is the fallback because it defers to whatever the platform already
/// decided, which is more likely right than picking a blend mode blind.
fn choose_composite_alpha(capabilities: &vk::SurfaceCapabilitiesKHR) -> vk::CompositeAlphaFlagsKHR {
    for candidate in [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
    ] {
        if capabilities.supported_composite_alpha.contains(candidate) {
            return candidate;
        }
    }
    vk::CompositeAlphaFlagsKHR::OPAQUE
}

fn backend_err(what: &str, e: vk::Result) -> Error {
    Error::Backend {
        backend: "vulkan",
        detail: format!("{what}: {e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(current: (u32, u32), min: (u32, u32), max: (u32, u32)) -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: current.0,
                height: current.1,
            },
            min_image_extent: vk::Extent2D {
                width: min.0,
                height: min.1,
            },
            max_image_extent: vk::Extent2D {
                width: max.0,
                height: max.1,
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_surface_with_a_fixed_size_dictates_it() {
        // Every windowed surface does. Building at a size it did not offer
        // produces an invalid swapchain rather than a resized window, so the
        // request is ignored rather than clamped toward.
        let capabilities = caps((800, 600), (1, 1), (4096, 4096));
        assert_eq!(
            resolve_extent(&capabilities, Extent2D::new(1920, 1080)),
            Extent2D::new(800, 600)
        );
    }

    #[test]
    fn a_surface_that_defers_takes_the_request_within_its_range() {
        let capabilities = caps((u32::MAX, u32::MAX), (16, 16), (1024, 1024));
        assert_eq!(
            resolve_extent(&capabilities, Extent2D::new(640, 480)),
            Extent2D::new(640, 480)
        );
        // And a request outside the range is clamped rather than refused: the
        // surface said it decides, and this is it deciding.
        assert_eq!(
            resolve_extent(&capabilities, Extent2D::new(4096, 8)),
            Extent2D::new(1024, 16)
        );
    }

    #[test]
    fn a_surface_with_no_preference_gets_the_renderer_s_own_choice() {
        let formats = [vk::SurfaceFormatKHR {
            format: vk::Format::UNDEFINED,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        }];
        let (vk_format, _, format) = choose_format(&formats).expect("choose");
        assert_eq!(vk_format, vk::Format::B8G8R8A8_UNORM);
        assert_eq!(format, PixelFormat::Bgra8Unorm);
    }

    #[test]
    fn a_plain_surface_format_is_chosen_over_an_srgb_one() {
        // The pipeline carries sRGB-encoded components all the way to this
        // write, and the presentation engine is told the image holds
        // sRGB-encoded components. Nothing has to encode between the two, so
        // the format that encodes nothing is the right one -- an sRGB
        // attachment would apply the transfer function a second time.
        //
        // This preferred the sRGB one while the renderer worked in light, which
        // was right then and is exactly backwards now.
        let formats = [
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_UNORM,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_SRGB,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
        ];
        let (vk_format, _, format) = choose_format(&formats).expect("choose");
        assert_eq!(vk_format, vk::Format::B8G8R8A8_UNORM);
        assert_eq!(format, PixelFormat::Bgra8Unorm);
    }

    #[test]
    fn a_surface_offering_only_linear_formats_still_works() {
        // Darkly, but a window is better than a refusal: the fallback exists so
        // a surface with no sRGB format is served rather than turned away.
        let formats = [vk::SurfaceFormatKHR {
            format: vk::Format::R8G8B8A8_UNORM,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        }];
        let (vk_format, _, format) = choose_format(&formats).expect("choose");
        assert_eq!(vk_format, vk::Format::R8G8B8A8_UNORM);
        assert_eq!(format, PixelFormat::Rgba8Unorm);
    }

    #[test]
    fn a_surface_offering_nothing_usable_is_refused() {
        let formats = [vk::SurfaceFormatKHR {
            format: vk::Format::R5G6B5_UNORM_PACK16,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        }];
        assert!(choose_format(&formats).is_err());
        assert!(choose_format(&[]).is_err());
    }

    #[test]
    fn composite_alpha_prefers_opaque_and_never_returns_nothing() {
        let with = |supported| vk::SurfaceCapabilitiesKHR {
            supported_composite_alpha: supported,
            ..Default::default()
        };
        assert_eq!(
            choose_composite_alpha(&with(
                vk::CompositeAlphaFlagsKHR::OPAQUE | vk::CompositeAlphaFlagsKHR::INHERIT
            )),
            vk::CompositeAlphaFlagsKHR::OPAQUE
        );
        assert_eq!(
            choose_composite_alpha(&with(vk::CompositeAlphaFlagsKHR::INHERIT)),
            vk::CompositeAlphaFlagsKHR::INHERIT
        );
    }
}

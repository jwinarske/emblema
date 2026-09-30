//! Render passes, pipelines, and drawing.
//!
//! Draws are accumulated into a [`Batch`] and submitted together: one geometry
//! upload, one render pass, one submission for the whole thing. A draw per
//! submission is fine for a test and hopeless for a frame loop, where the
//! fixed cost of beginning a pass and waiting on a fence dwarfs the drawing.

use crate::device::VulkanContext;
use crate::resource::{backend_err, transition, vk_format, VulkanTexture};
use ash::vk;
use emblema_hal::{
    Batch, BlendMode, ClipRole, Error, Extent2D, PassDescriptor, Result, TextureDescriptor,
    TextureUsage,
};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use std::collections::HashMap;

/// A render pass is distinguished by the attachment it targets and whether it
/// clears, because the load operation is baked into the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RenderPassKey {
    pub(crate) format: vk::Format,
    pub(crate) clears: bool,
    /// Sample count of the attachment being rendered into. Above one, the pass
    /// carries a resolve attachment as well.
    pub(crate) samples: u32,
    /// Whether the pass leaves its target ready for presentation.
    ///
    /// Part of the key because it changes an attachment's final layout, and
    /// two passes whose attachments differ are not compatible.
    pub(crate) presents: bool,
    /// The stencil attachment's format, or `None` for a pass that has none.
    ///
    /// Part of the key rather than a flag because passes are only compatible
    /// when their attachment formats match, so a pipeline built against a pass
    /// with a stencil cannot be used with one without.
    pub(crate) stencil: Option<vk::Format>,
}

/// A pipeline is distinguished by format and blend state.
///
/// Not by load operation: render passes with matching attachment formats and
/// sample counts are compatible, so one pipeline is usable with both the
/// clearing and the preserving pass. That compatibility is what lets a single
/// batch mix blend modes inside one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    pub(crate) format: vk::Format,
    pub(crate) blend: BlendMode,
    /// Rasterization sample count, which is baked into a pipeline and must
    /// match the render pass it is used with.
    pub(crate) samples: u32,
    /// The stencil attachment's format, or `None` where the pass has none.
    pub(crate) stencil: Option<vk::Format>,
    /// Which fragment program this pipeline runs.
    ///
    /// `None` is the built-in shader every material shares. `Some` names a
    /// caller's, registered with the context -- which is the whole of what a
    /// runtime effect is here: a pipeline built from a module this renderer
    /// did not know about when it was built.
    pub(crate) program: Option<u32>,
    /// What this pipeline does with the stencil.
    ///
    /// The value compared against is dynamic state rather than part of the key,
    /// so a clip stack of any depth costs three pipelines rather than three per
    /// level. Only the operation differs between them, and that is baked in.
    pub(crate) role: ClipRole,
}

/// Cached pipeline objects, built on demand and held for the context's life.
///
/// Pipeline creation costs milliseconds, so building one per draw would put
/// exactly the compilation stall in the frame loop that compiling everything
/// ahead of time is meant to avoid.
#[derive(Default)]
pub(crate) struct PipelineCache {
    pub(crate) layout: Option<vk::PipelineLayout>,
    pub(crate) render_passes: HashMap<RenderPassKey, vk::RenderPass>,
    pub(crate) pipelines: HashMap<PipelineKey, vk::Pipeline>,
}

impl PipelineCache {
    pub(crate) fn render_pass(&self, key: RenderPassKey) -> Option<vk::RenderPass> {
        self.render_passes.get(&key).copied()
    }

    pub(crate) fn pipeline(&self, key: PipelineKey) -> Option<vk::Pipeline> {
        self.pipelines.get(&key).copied()
    }

    pub(crate) fn layout(&self) -> Option<vk::PipelineLayout> {
        self.layout
    }

    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: called from context teardown, after the queue has drained.
        unsafe {
            for pipeline in self.pipelines.values() {
                device.destroy_pipeline(*pipeline, None);
            }
            for pass in self.render_passes.values() {
                device.destroy_render_pass(*pass, None);
            }
            if let Some(layout) = self.layout {
                device.destroy_pipeline_layout(layout, None);
            }
        }
        self.pipelines.clear();
        self.render_passes.clear();
        self.layout = None;
    }
}

impl VulkanContext {
    /// Draw a single set of triangles. A convenience over [`Batch`].
    ///
    /// The material is in linear space with **straight alpha**; it is
    /// premultiplied on the way to the target, which holds premultiplied color.
    pub fn draw_indexed(
        &mut self,
        target: &mut VulkanTexture,
        vertices: &[[f32; 2]],
        indices: &[u32],
        material: emblema_hal::Material,
        blend: BlendMode,
        clear: Option<[f32; 4]>,
    ) -> Result<()> {
        let mut batch = Batch::new();
        batch.push(vertices, indices, material, blend)?;
        self.submit_batch(
            target,
            &batch,
            PassDescriptor {
                clear,
                samples: 1,
                viewport: None,
            },
        )
    }

    /// Record and submit a whole batch as one render pass.
    pub fn submit_batch(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<()> {
        self.submit_batch_textured(target, batch, pass, &[])
    }

    /// Record and submit a whole batch as one render pass, with a texture table
    /// its image paints index.
    pub fn submit_batch_textured(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        pass: PassDescriptor,
        textures: &[&VulkanTexture],
    ) -> Result<()> {
        let format = vk_format(target.format());
        let clear = pass.clear;

        self.capabilities().check_blend_modes(batch)?;
        if !pass.samples.is_power_of_two() {
            return Err(Error::Unsupported("sample count is not a power of two"));
        }
        if !self.capabilities().sample_counts.supports(pass.samples) {
            return Err(Error::Unsupported("sample count not supported by device"));
        }
        if pass.is_multisampled() && clear.is_none() {
            // A multisample pass renders into a transient buffer and resolves
            // out of it. Preserving would mean seeding that buffer with the
            // target's existing contents, and there is no reverse of a resolve
            // to do it with -- it would take a full-screen draw. Refusing is
            // better than silently discarding what the target held.
            return Err(Error::Unsupported(
                "a multisampled pass must clear: give the pass a clear color, \
                 or draw it at one sample. Preserving would need a \
                 resolved-to-multisample copy, which this technique has no \
                 reverse of",
            ));
        }

        // Nothing to draw still clears, which is what a caller submitting an
        // empty scene expects. With no clear either, there is nothing to do.
        if batch.is_empty() {
            return match clear {
                Some(c) => self.clear_texture(target, c),
                None => Ok(()),
            };
        }

        let stencil_format = self.stencil_format_for(batch)?;
        let pass_key = RenderPassKey {
            format,
            clears: clear.is_some(),
            samples: pass.samples,
            presents: false,
            stencil: stencil_format,
        };
        let render_pass = self.ensure_render_pass(pass_key)?;
        for draw in batch.draws() {
            self.ensure_pipeline(
                PipelineKey {
                    format,
                    program: draw.material.program(),
                    blend: draw.blend,
                    samples: pass.samples,
                    stencil: stencil_format,
                    role: draw.stencil.role,
                },
                render_pass,
            )?;
        }

        let bindings = crate::sampling::build(self, batch, textures)?;

        let device = self.raw_device().clone();
        let vertex_buffer = self.upload(
            cast_bytes(batch.vertices()),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let index_buffer = self.upload(
            cast_bytes(batch.indices()),
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?;
        let (material_buffer, materials) = crate::materials::build(self, batch, target.format())?;

        let result = self.record_batch(
            &device,
            target,
            format,
            render_pass,
            batch,
            vertex_buffer.buffer,
            index_buffer.buffer,
            pass,
            stencil_format,
            &bindings,
            &materials,
            textures,
        );

        self.release(vertex_buffer);
        self.release(index_buffer);
        self.release(material_buffer);
        materials.destroy(&device);
        bindings.destroy(self);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn record_batch(
        &mut self,
        device: &ash::Device,
        target: &mut VulkanTexture,
        format: vk::Format,
        render_pass: vk::RenderPass,
        batch: &Batch,
        vertex_buffer: vk::Buffer,
        index_buffer: vk::Buffer,
        pass: PassDescriptor,
        stencil_format: Option<vk::Format>,
        bindings: &crate::sampling::Bindings,
        materials: &crate::materials::Materials,
        sampled: &[&VulkanTexture],
    ) -> Result<()> {
        let layout = self.pipeline_cache().layout().expect("ensured above");
        let extent = target.extent();
        let clear = pass.clear;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(target.raw_image())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        let view = unsafe { device.create_image_view(&view_info, None) }
            .map_err(|e| backend_err("create_image_view", e))?;

        // The multisample buffer is transient: rendered into, resolved out of,
        // and discarded. Allocating it per submission is wasteful and will move
        // into the frame's resource pool once one exists; keeping it here for
        // now avoids a cache whose invalidation rules nothing yet needs.
        let multisample = if pass.is_multisampled() {
            match self.create_multisample_buffer(target, pass.samples) {
                Ok(ms) => Some(ms),
                Err(e) => {
                    unsafe { device.destroy_image_view(view, None) };
                    return Err(e);
                }
            }
        } else {
            None
        };

        // Matches the multisample buffer in lifetime and in reason: written
        // during the pass, never read afterwards, discarded at the end.
        let stencil = match stencil_format {
            Some(format) => match crate::stencil::create(self, extent, pass.samples, format) {
                Ok(buffer) => Some(buffer),
                Err(e) => {
                    unsafe { device.destroy_image_view(view, None) };
                    if let Some((tex, ms_view)) = multisample {
                        unsafe { device.destroy_image_view(ms_view, None) };
                        self.destroy_texture(tex);
                    }
                    return Err(e);
                }
            },
            None => None,
        };

        let mut attachments: Vec<vk::ImageView> = match &multisample {
            // Order matches the render pass: the multisample attachment first,
            // then the resolve target the caller reads.
            Some((_, ms_view)) => vec![*ms_view, view],
            None => vec![view],
        };
        if let Some(stencil) = &stencil {
            attachments.push(stencil.view);
        }
        let fb_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let framebuffer = match unsafe { device.create_framebuffer(&fb_info, None) } {
            Ok(fb) => fb,
            Err(e) => {
                unsafe { device.destroy_image_view(view, None) };
                if let Some((tex, ms_view)) = multisample {
                    unsafe { device.destroy_image_view(ms_view, None) };
                    self.destroy_texture(tex);
                }
                if let Some(stencil) = stencil {
                    stencil.destroy(self);
                }
                return Err(backend_err("create_framebuffer", e));
            }
        };

        let previous_layout = target.layout();
        let outcome = (|| -> Result<()> {
            let cmd = self.begin_one_shot()?;

            // Before the render pass begins, since a layout transition is not
            // legal inside one. Whatever left these in a color-attachment or
            // transfer layout, a shader cannot read them there.
            crate::sampling::transition_for_sampling(device, cmd, sampled);

            if clear.is_none() {
                // Loading requires the attachment already be in the layout the
                // render pass declares, and the texture may be sitting in
                // whatever a previous readback left it in.
                transition(
                    device,
                    cmd,
                    target.raw_image(),
                    previous_layout,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                );
            }

            // One clear value per attachment, even though the resolve target's
            // load operation discards it. The stencil starts at zero, which is
            // the depth unclipped content draws at, so a pass with no clip
            // draws behaves exactly as one with no stencil attachment.
            let mut clear_values = vec![vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: clear.unwrap_or([0.0; 4]),
                },
            }];
            clear_values.resize(attachments.len(), vk::ClearValue::default());
            let clear_values = &clear_values[..];
            let area = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: extent.width,
                    height: extent.height,
                },
            };
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(render_pass)
                .framebuffer(framebuffer)
                .render_area(area)
                .clear_values(clear_values);

            let viewport = pass_viewport(pass, extent);

            unsafe {
                device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
                device.cmd_set_viewport(cmd, 0, &[viewport]);
                device.cmd_set_scissor(cmd, 0, &[area]);
                device.cmd_bind_vertex_buffers(cmd, 0, &[vertex_buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);

                let recording = PassRecording {
                    device,
                    cmd,
                    layout,
                    area,
                    bindings,
                    materials,
                };
                let mut state = RecordedState::at_pass_start(area);
                for (index, draw) in batch.draws().iter().enumerate() {
                    let pipeline = self
                        .pipeline_cache()
                        .pipeline(PipelineKey {
                            format,
                            program: draw.material.program(),
                            blend: draw.blend,
                            samples: pass.samples,
                            stencil: stencil_format,
                            role: draw.stencil.role,
                        })
                        .expect("ensured above");
                    record_draw(&recording, draw, index, pipeline, &mut state);
                }

                device.cmd_end_render_pass(cmd);
            }

            self.submit_one_shot(cmd)
        })();

        // SAFETY: the submission above was waited on, so neither object is in
        // use regardless of the result.
        unsafe {
            device.destroy_framebuffer(framebuffer, None);
            device.destroy_image_view(view, None);
        }
        if let Some((tex, ms_view)) = multisample {
            unsafe { device.destroy_image_view(ms_view, None) };
            self.destroy_texture(tex);
        }
        if let Some(stencil) = stencil {
            stencil.destroy(self);
        }

        if outcome.is_ok() {
            // The render pass declares this as its final layout, so the next
            // operation must transition from here rather than from whatever
            // the texture held before.
            target.set_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        }
        outcome
    }

    /// Allocate a transient multisample color buffer matching `target`.
    /// The stencil format this submission needs, or `None` if it needs none.
    ///
    /// Resolved once per submission rather than per draw so that every key
    /// built from it agrees, and refused up front where the clip stack is
    /// deeper than a stencil can distinguish: past that point the value wraps
    /// to zero and the clip admits everything it was meant to exclude, which is
    /// a picture rather than an error.
    fn stencil_format_for(&self, batch: &Batch) -> Result<Option<vk::Format>> {
        if !batch.uses_stencil() {
            return Ok(None);
        }
        batch.check_clip_depth()?;
        crate::stencil::stencil_format(self).map(Some)
    }

    fn create_multisample_buffer(
        &mut self,
        target: &VulkanTexture,
        samples: u32,
    ) -> Result<(VulkanTexture, vk::ImageView)> {
        let desc = TextureDescriptor {
            extent: target.extent(),
            format: target.format(),
            usage: TextureUsage {
                render_target: true,
                ..TextureUsage::default()
            },
            sample_count: samples,
            // A multisample buffer is resolved, never sampled, so a chain over
            // it would be levels nothing reads.
            mip_levels: 1,
            #[cfg(unix)]
            external: None,
        };
        let texture = self.create_texture(&desc)?;

        let device = self.raw_device().clone();
        let info = vk::ImageViewCreateInfo::default()
            .image(texture.raw_image())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk_format(target.format()))
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        match unsafe { device.create_image_view(&info, None) } {
            Ok(view) => Ok((texture, view)),
            Err(e) => {
                self.destroy_texture(texture);
                Err(backend_err("create_image_view", e))
            }
        }
    }

    /// Submit a batch without waiting, returning a fence that signals when the
    /// GPU is finished.
    ///
    /// The waiting form is right for tests and setup. A frame loop needs this
    /// one: it hands the caller something to attach to a page flip, or to wait
    /// on while the next frame is already being recorded. Everything the
    /// submission still needs — the command buffer and the pass's transient
    /// framebuffer objects — travels with the fence, because none of it can be
    /// released until the work completes.
    ///
    /// The returned fence must be handed back to [`Self::retire_fence`].
    pub fn submit_batch_deferred(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<crate::fence::VulkanFence> {
        self.submit_batch_deferred_synchronized(target, batch, pass, &[], Default::default())
    }

    /// Submit without waiting, gating the work on semaphores the caller owns.
    ///
    /// What a swapchain needs: the render must not touch the image before
    /// acquisition has signaled, and presentation must not read it before the
    /// render has. Neither is expressible with a fence, because both are
    /// device-side orderings that no one should be blocking a thread to
    /// enforce.
    /// `textures` is the table a [`emblema_hal::Material::Image`] slot indexes, exactly as
    /// the waiting form takes it. A frame that composites a layer needs it:
    /// the pass that lands in a presentable image is the one that samples the
    /// layer, so a deferred submission that could not sample anything meant a
    /// frame with layers could be rendered offscreen and never presented.
    pub fn submit_batch_deferred_synchronized(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        pass: PassDescriptor,
        textures: &[&VulkanTexture],
        sync: crate::device::FrameSync<'_>,
    ) -> Result<crate::fence::VulkanFence> {
        if pass.is_multisampled() {
            // The transient multisample buffer would have to outlive the
            // submission too. Supporting it means putting it in the fence
            // alongside the rest, which is worth doing when a caller needs it.
            return Err(Error::Unsupported(
                "deferred submission of a multisampled pass",
            ));
        }
        let format = vk_format(target.format());
        if batch.is_empty() {
            return Err(Error::Unsupported("deferred submission of an empty batch"));
        }

        let stencil_format = self.stencil_format_for(batch)?;
        let pass_key = RenderPassKey {
            format,
            clears: pass.clear.is_some(),
            samples: 1,
            presents: sync.presents,
            stencil: stencil_format,
        };
        let render_pass = self.ensure_render_pass(pass_key)?;
        for draw in batch.draws() {
            self.ensure_pipeline(
                PipelineKey {
                    format,
                    program: draw.material.program(),
                    blend: draw.blend,
                    samples: 1,
                    stencil: stencil_format,
                    role: draw.stencil.role,
                },
                render_pass,
            )?;
        }

        let device = self.raw_device().clone();
        let vertex_buffer = self.upload(
            cast_bytes(batch.vertices()),
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let index_buffer = self.upload(
            cast_bytes(batch.indices()),
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?;

        let result = self.record_deferred(
            &device,
            target,
            format,
            render_pass,
            batch,
            vertex_buffer.buffer,
            index_buffer.buffer,
            pass,
            stencil_format,
            textures,
            sync,
        );

        // The geometry buffers are host-visible and were fully written before
        // submission, so releasing them here would free memory the GPU is still
        // reading. They are retained until the fence retires.
        match result {
            Ok(mut fence) => {
                // Held by this fence rather than by the context, so that
                // retiring one frame cannot free another's geometry.
                fence.retained.push(vertex_buffer);
                fence.retained.push(index_buffer);
                Ok(fence)
            }
            Err(e) => {
                self.release(vertex_buffer);
                self.release(index_buffer);
                Err(e)
            }
        }
    }

    /// Release a deferred submission once its work has completed.
    pub fn retire_fence(&mut self, mut fence: crate::fence::VulkanFence) {
        // Waits for the submission, then releases what it held -- in that
        // order, since the buffers are what the submission was reading.
        fence.retire();
        for buffer in std::mem::take(&mut fence.retained) {
            self.release(buffer);
        }
        // After the wait inside `retire`, so nothing is still reading the sets.
        if let Some(bindings) = fence.bindings.take() {
            bindings.destroy(self);
        }
        if let Some(materials) = fence.materials.take() {
            materials.destroy(&self.raw_device().clone());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_deferred(
        &mut self,
        device: &ash::Device,
        target: &mut VulkanTexture,
        format: vk::Format,
        render_pass: vk::RenderPass,
        batch: &Batch,
        vertex_buffer: vk::Buffer,
        index_buffer: vk::Buffer,
        pass: PassDescriptor,
        stencil_format: Option<vk::Format>,
        textures: &[&VulkanTexture],
        sync: crate::device::FrameSync<'_>,
    ) -> Result<crate::fence::VulkanFence> {
        // The pool these sets come from cannot be destroyed while the command
        // buffer reading them is in flight, and a deferred submission is in
        // flight for as long as the caller likes -- so the bindings travel with
        // the fence, alongside the framebuffer and view they sit next to here.
        let bindings = crate::sampling::build(self, batch, textures)?;
        let (material_buffer, materials) = crate::materials::build(self, batch, target.format())?;
        let layout = self.pipeline_cache().layout().expect("ensured above");
        let extent = target.extent();

        let view_info = vk::ImageViewCreateInfo::default()
            .image(target.raw_image())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        let view = unsafe { device.create_image_view(&view_info, None) }
            .map_err(|e| backend_err("create_image_view", e))?;

        let attachments = [view];
        let fb_info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let framebuffer = match unsafe { device.create_framebuffer(&fb_info, None) } {
            Ok(fb) => fb,
            Err(e) => {
                unsafe { device.destroy_image_view(view, None) };
                return Err(backend_err("create_framebuffer", e));
            }
        };

        // Pipelines are resolved before recording so the loop below needs
        // nothing from the cache, which keeps the recording free of borrows on
        // the context while it holds the command buffer.
        let pipelines: Vec<vk::Pipeline> = batch
            .draws()
            .iter()
            .map(|draw| {
                self.pipeline_cache()
                    .pipeline(PipelineKey {
                        format,
                        program: draw.material.program(),
                        blend: draw.blend,
                        samples: 1,
                        stencil: stencil_format,
                        role: draw.stencil.role,
                    })
                    .expect("ensured above")
            })
            .collect();

        let previous_layout = target.layout();
        let cmd = self.begin_one_shot()?;

        // SAFETY: every object below belongs to this device and the command
        // buffer is in the recording state.
        unsafe {
            // Before the render pass begins, since a layout transition is not
            // legal inside one. A layer target sits in the color-attachment
            // layout its own pass left it in, which a shader cannot read.
            crate::sampling::transition_for_sampling(device, cmd, textures);

            if pass.clear.is_none() {
                transition(
                    device,
                    cmd,
                    target.raw_image(),
                    previous_layout,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                );
            }

            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: pass.clear.unwrap_or([0.0; 4]),
                },
            }];
            let area = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: extent.width,
                    height: extent.height,
                },
            };
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(render_pass)
                .framebuffer(framebuffer)
                .render_area(area)
                .clear_values(&clear_values);
            let viewport = pass_viewport(pass, extent);

            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor(cmd, 0, &[area]);
            device.cmd_bind_vertex_buffers(cmd, 0, &[vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);

            let recording = PassRecording {
                device,
                cmd,
                layout,
                area,
                bindings: &bindings,
                materials: &materials,
            };
            let mut state = RecordedState::at_pass_start(area);
            for (index, (draw, pipeline)) in batch.draws().iter().zip(&pipelines).enumerate() {
                record_draw(&recording, draw, index, *pipeline, &mut state);
            }
            device.cmd_end_render_pass(cmd);
        }

        let mut fence = self.submit_exportable(cmd, framebuffer, view, sync)?;
        fence.bindings = Some(bindings);
        fence.materials = Some(materials);
        fence.retained.push(material_buffer);
        target.set_layout(if sync.presents {
            vk::ImageLayout::PRESENT_SRC_KHR
        } else {
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        });
        Ok(fence)
    }

    fn ensure_render_pass(&mut self, key: RenderPassKey) -> Result<vk::RenderPass> {
        if let Some(pass) = self.pipeline_cache().render_pass(key) {
            return Ok(pass);
        }
        let device = self.raw_device().clone();
        let pass = build_render_pass(&device, key)?;
        self.pipeline_cache_mut().render_passes.insert(key, pass);
        Ok(pass)
    }

    fn ensure_pipeline(&mut self, key: PipelineKey, render_pass: vk::RenderPass) -> Result<()> {
        if self.pipeline_cache().pipeline(key).is_some() {
            return Ok(());
        }
        let device = self.raw_device().clone();
        let layout = match self.pipeline_cache().layout() {
            Some(l) => l,
            None => {
                // Built here rather than lazily beside the sampler, because
                // every pipeline declares the set whether it samples or not
                // and so cannot be created without it.
                let descriptors = self.descriptor_layout()?;
                let materials = self.material_layout()?;
                let l = build_pipeline_layout(&device, descriptors, materials)?;
                self.pipeline_cache_mut().layout = Some(l);
                l
            }
        };
        // Cloned out of the registry before building, because the build needs
        // the device mutably and a borrowed payload would hold the context.
        let fragment = match key.program {
            Some(id) => Some(self.runtime_program(id)?.to_vec()),
            None => None,
        };
        let pipeline = build_pipeline(&device, key, render_pass, layout, fragment.as_deref())?;
        self.pipeline_cache_mut().pipelines.insert(key, pipeline);
        Ok(())
    }

    pub(crate) fn upload(
        &mut self,
        bytes: &[u8],
        usage: vk::BufferUsageFlags,
    ) -> Result<StagedBuffer> {
        let device = self.raw_device().clone();
        let info = vk::BufferCreateInfo::default()
            .size(bytes.len().max(1) as u64)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&info, None) }
            .map_err(|e| backend_err("create_buffer", e))?;

        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let mut allocation = self
            .allocator_mut()
            .allocate(&AllocationCreateDesc {
                name: "upload",
                requirements,
                // Host-visible so the write below needs no staging copy.
                //
                // Every per-frame buffer comes through here -- vertices,
                // indices and materials alike -- and each is allocated, filled
                // and freed within the submission. This comment used to say
                // that geometry went through a ring allocator instead. There
                // is no ring allocator; there never was.
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| {
                unsafe { device.destroy_buffer(buffer, None) };
                Error::Backend {
                    backend: "vulkan",
                    detail: format!("upload allocation: {e}"),
                }
            })?;

        unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
            .map_err(|e| backend_err("bind_buffer_memory", e))?;

        let slice = allocation.mapped_slice_mut().ok_or(Error::Backend {
            backend: "vulkan",
            detail: "upload allocation was not host-visible".into(),
        })?;
        slice[..bytes.len()].copy_from_slice(bytes);

        Ok(StagedBuffer { buffer, allocation })
    }

    pub(crate) fn release(&mut self, staged: StagedBuffer) {
        let _ = self.allocator_mut().free(staged.allocation);
        // SAFETY: every submission using this buffer was waited on before the
        // call that made it returned.
        unsafe { self.raw_device().destroy_buffer(staged.buffer, None) };
    }
}

pub(crate) struct StagedBuffer {
    pub(crate) buffer: vk::Buffer,
    pub(crate) allocation: Allocation,
}

/// Where a pass's clip space lands in the target.
///
/// Without a viewport of its own a pass covers the target exactly, which is
/// what every pass wants whose geometry was recorded against the target it
/// renders into. A layer that narrowed its target after recording carries one,
/// and it keeps the extent the geometry was recorded against so that clip
/// space still maps to the same device pixels -- the offset slides those
/// pixels so the wanted sub-rectangle lands on the smaller target, which crops
/// rather than scales.
///
/// The offset is normally negative, which Vulkan allows: `viewportBoundsRange`
/// is at least [-2 * maxViewportDimension, 2 * maxViewportDimension - 1], and
/// an offset here never exceeds the size of the recorded space.
fn pass_viewport(pass: PassDescriptor, extent: Extent2D) -> vk::Viewport {
    let (offset, size) = match pass.viewport {
        Some(v) => (v.offset, v.extent),
        None => ([0.0, 0.0], extent),
    };
    vk::Viewport {
        x: offset[0],
        y: offset[1],
        width: size.width as f32,
        height: size.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }
}

fn build_render_pass(device: &ash::Device, key: RenderPassKey) -> Result<vk::RenderPass> {
    // Clearing makes prior contents irrelevant, so the attachment can declare
    // UNDEFINED and skip a transition. Loading must declare the layout the
    // image is actually in, and the caller transitions it there first.
    let (load_op, initial_layout) = if key.clears {
        (vk::AttachmentLoadOp::CLEAR, vk::ImageLayout::UNDEFINED)
    } else {
        (
            vk::AttachmentLoadOp::LOAD,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        )
    };
    let color = vk::AttachmentDescription::default()
        .format(key.format)
        .samples(sample_flags(key.samples))
        .load_op(load_op)
        // Multisample contents are consumed by the resolve and never read
        // again, so storing them would cost bandwidth for nothing. On a tiler
        // that is the difference between the multisample buffer staying in
        // tile memory and being written out to main memory.
        .store_op(if key.samples > 1 {
            vk::AttachmentStoreOp::DONT_CARE
        } else {
            vk::AttachmentStoreOp::STORE
        })
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(initial_layout)
        .final_layout(final_layout(key, false));

    // The resolve target is the texture the caller reads. Its prior contents
    // are irrelevant because the resolve overwrites every pixel the pass
    // touched.
    let resolve = vk::AttachmentDescription::default()
        .format(key.format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::DONT_CARE)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(final_layout(key, true));

    // Cleared at pass start and discarded at the end. No pass reads what
    // another left here: a clip stack that outlives a pass -- which one does
    // when a backdrop filter cuts the pass under it -- is rebuilt by drawing
    // the narrowings again, not by carrying the buffer across. So on a tiler
    // discarding keeps it in tile memory, and the clear is what every pass
    // starts from.
    let stencil = key.stencil.map(|format| {
        vk::AttachmentDescription::default()
            .format(format)
            .samples(sample_flags(key.samples))
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::CLEAR)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
    });

    let mut attachments: Vec<vk::AttachmentDescription> = if key.samples > 1 {
        vec![color, resolve]
    } else {
        vec![color]
    };
    // Last, so the color and resolve attachments keep the indices they had and
    // the framebuffer built alongside this needs no reordering.
    let stencil_index = attachments.len() as u32;
    if let Some(stencil) = stencil {
        attachments.push(stencil);
    }

    let color_refs = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let resolve_refs = [vk::AttachmentReference::default()
        .attachment(1)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let stencil_ref = vk::AttachmentReference::default()
        .attachment(stencil_index)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

    let mut subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs);
    if key.samples > 1 {
        subpass = subpass.resolve_attachments(&resolve_refs);
    }
    if key.stencil.is_some() {
        subpass = subpass.depth_stencil_attachment(&stencil_ref);
    }
    let subpasses = [subpass];
    // The attachment's layout transition happens as part of beginning the pass,
    // and nothing orders it against work outside the pass unless this says so.
    // Waiting on a semaphore does not: a wait with a destination stage of
    // color-attachment output orders the *draws* after it, while the transition
    // is free to run before the wait completes.
    //
    // That is not theoretical. A swapchain image is acquired, the acquire
    // signals a semaphore, the render waits on it -- and the render pass could
    // still transition the image while the presentation engine was reading it.
    // Synchronization validation reports it as a write-after-read hazard
    // against `vkAcquireNextImageKHR`, and no picture on this driver ever
    // showed it.
    //
    // Both directions, because a pass both begins and ends with a transition:
    // the first waits for prior color output before transitioning in, the
    // second makes this pass's writes visible before whatever reads the target
    // next, which is presentation or a later pass sampling it.
    // Every stage that touches an attachment here, not only the color one. A
    // stencil attachment is transitioned and then cleared by its load
    // operation, and those are two writes that need ordering exactly as much
    // as the color ones do -- reported as a write-after-write against the
    // pass's own layout transition when they are left out.
    let attachment_stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
    let attachment_writes = vk::AccessFlags::COLOR_ATTACHMENT_WRITE
        | vk::AccessFlags::COLOR_ATTACHMENT_READ
        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE
        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ;
    let dependencies = [
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(attachment_stages)
            .dst_stage_mask(attachment_stages)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(attachment_writes),
        vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(attachment_stages)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .src_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_access_mask(vk::AccessFlags::SHADER_READ),
    ];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&dependencies);

    unsafe { device.create_render_pass(&info, None) }
        .map_err(|e| backend_err("create_render_pass", e))
}

/// The layout an attachment is left in.
///
/// A multisampled pass writes into the multisample attachment and resolves into
/// the target, so it is the resolve attachment the caller reads and the resolve
/// attachment that has to be ready to present. The multisample one is discarded
/// either way.
fn final_layout(key: RenderPassKey, is_resolve: bool) -> vk::ImageLayout {
    let read_by_the_caller = is_resolve || key.samples == 1;
    if key.presents && read_by_the_caller {
        vk::ImageLayout::PRESENT_SRC_KHR
    } else {
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
    }
}

pub(crate) fn sample_flags(count: u32) -> vk::SampleCountFlags {
    match count {
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        _ => vk::SampleCountFlags::TYPE_1,
    }
}

fn build_pipeline_layout(
    device: &ash::Device,
    descriptor_layout: vk::DescriptorSetLayout,
    material_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout> {
    // Two sets: the first carries the texture an image paint samples, the
    // second the paint itself. One layout serves every pipeline, since they
    // all declare the same two -- which is also why a solid fill still binds a
    // texture it never reads.
    //
    // Separate sets rather than two bindings in one, because the texture set
    // includes a placeholder owned by the context and outliving any single
    // submission, while the paint's buffer is built per submission.
    let set_layouts = [descriptor_layout, material_layout];
    let info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
    unsafe { device.create_pipeline_layout(&info, None) }
        .map_err(|e| backend_err("create_pipeline_layout", e))
}

fn build_pipeline(
    device: &ash::Device,
    key: PipelineKey,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    fragment: Option<&[u32]>,
) -> Result<vk::Pipeline> {
    // The vertex stage is always this renderer's own: a runtime effect
    // replaces what a fragment does with a paint, not how geometry reaches
    // clip space, and letting it replace the latter would mean every effect
    // restating a convention it has no reason to know.
    let module_info = vk::ShaderModuleCreateInfo::default().code(emblema_shaders::SOLID_SPV);
    let module = unsafe { device.create_shader_module(&module_info, None) }
        .map_err(|e| backend_err("create_shader_module", e))?;
    let fragment_module = match fragment {
        Some(code) => {
            let info = vk::ShaderModuleCreateInfo::default().code(code);
            Some(
                unsafe { device.create_shader_module(&info, None) }
                    .map_err(|e| backend_err("create_shader_module", e))?,
            )
        }
        None => None,
    };
    let fs_module = fragment_module.unwrap_or(module);

    let vs_name = c"vs_main";
    let fs_name = c"fs_main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(vs_name),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fs_module)
            .name(fs_name),
    ];

    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(std::mem::size_of::<emblema_hal::Vertex>() as u32)
        .input_rate(vk::VertexInputRate::VERTEX)];
    // Offsets from the type rather than from arithmetic over the field widths.
    // Two backends describe this one struct to two APIs, and a description that
    // is wrong the same way in both is the failure nothing here would catch --
    // the pixels are compared against the other backend. Neither of them gets
    // to restate the layout.
    let attributes = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(std::mem::offset_of!(emblema_hal::Vertex, position) as u32),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(std::mem::offset_of!(emblema_hal::Vertex, uv) as u32),
        vk::VertexInputAttributeDescription::default()
            .location(2)
            .binding(0)
            .format(vk::Format::R32G32B32A32_SFLOAT)
            .offset(std::mem::offset_of!(emblema_hal::Vertex, color) as u32),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attributes);

    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        // No culling: 2D geometry arrives in whatever winding tessellation
        // produced, and discarding half of it by winding would drop triangles
        // that are perfectly visible.
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .polygon_mode(vk::PolygonMode::FILL)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(sample_flags(key.samples));

    // Factors come from the shared table rather than being restated here, so
    // the two backends cannot disagree about what a mode means. Color arrives
    // premultiplied from the shader, which is what that table assumes: source-
    // over is ONE rather than SRC_ALPHA, since using SRC_ALPHA against an
    // already-scaled source applies alpha twice and darkens every translucent
    // edge.
    // A clip draw contributes shape, not color. Masking every channel rather
    // than relying on a blend mode that happens to discard the source keeps the
    // clip geometry from touching the target even where blending is exotic.
    let attachment = vk::PipelineColorBlendAttachmentState::default().color_write_mask(
        if key.role.writes_color() {
            vk::ColorComponentFlags::RGBA
        } else {
            vk::ColorComponentFlags::empty()
        },
    );
    let blend_attachments = [match key.blend.factors() {
        _ if key.blend.is_plain_write() => {
            // Writing the source outright needs no blend unit at all, which is
            // worth switching off rather than expressing as ONE and ZERO.
            attachment.blend_enable(false)
        }
        Some(factors) => {
            let src = vk_blend_factor(factors.src);
            let dst = vk_blend_factor(factors.dst);
            attachment
                .blend_enable(true)
                .src_color_blend_factor(src)
                .dst_color_blend_factor(dst)
                .color_blend_op(vk::BlendOp::ADD)
                // The same factors for alpha as for color: with premultiplied
                // color the alpha channel is not a special case, and giving it
                // different factors is what breaks compositing a layer onto
                // something else.
                .src_alpha_blend_factor(src)
                .dst_alpha_blend_factor(dst)
                .alpha_blend_op(vk::BlendOp::ADD)
        }
        // An advanced mode names the whole equation in the blend op, so the
        // factors are ignored by the hardware entirely. They are still left at
        // ONE and ZERO rather than whatever the defaults happen to be, so a
        // driver that reads them cannot produce something arbitrary.
        None => attachment
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ZERO)
            .color_blend_op(vk_advanced_blend_op(key.blend))
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk_advanced_blend_op(key.blend)),
    }];
    let mut blend =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);

    // Both sides arrive premultiplied, and saying so is what makes the hardware
    // divide the alpha back out before evaluating the blend function; claiming
    // otherwise renders every translucent advanced blend at the wrong strength.
    // UNCORRELATED is the overlap model the compositing specification defines,
    // so it is what the reference formula in the HAL agrees with.
    let mut advanced = vk::PipelineColorBlendAdvancedStateCreateInfoEXT::default()
        .src_premultiplied(true)
        .dst_premultiplied(true)
        .blend_overlap(vk::BlendOverlapEXT::UNCORRELATED);
    if key.blend.is_advanced() {
        blend = blend.push_next(&mut advanced);
    }

    // The stencil operations differ per role and are baked in; the value they
    // compare against is the clip depth, which is dynamic so that nesting
    // deeper costs a state change rather than another pipeline.
    let stencil_op = vk::StencilOpState::default()
        .compare_op(vk::CompareOp::EQUAL)
        .fail_op(vk::StencilOp::KEEP)
        .depth_fail_op(vk::StencilOp::KEEP)
        .pass_op(match key.role {
            ClipRole::Content => vk::StencilOp::KEEP,
            // Clamping rather than wrapping: at the top of the range a wrap
            // lands back on zero and admits everything the clip excluded, where
            // a clamp merely fails to narrow further. Neither is correct, and
            // depth is checked against the limit before recording, so this is
            // the safer of two states that should not arise.
            ClipRole::Narrow => vk::StencilOp::INCREMENT_AND_CLAMP,
            ClipRole::Widen => vk::StencilOp::DECREMENT_AND_CLAMP,
        })
        .compare_mask(0xff)
        .write_mask(if key.role.writes_stencil() { 0xff } else { 0 });
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .stencil_test_enable(key.stencil.is_some())
        .front(stencil_op)
        .back(stencil_op);

    let dynamic_states = [
        vk::DynamicState::VIEWPORT,
        vk::DynamicState::SCISSOR,
        vk::DynamicState::STENCIL_REFERENCE,
    ];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&raster)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);

    let created =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };

    // The shader modules are only needed during creation.
    unsafe { device.destroy_shader_module(module, None) };
    if let Some(fragment) = fragment_module {
        unsafe { device.destroy_shader_module(fragment, None) };
    }

    match created {
        Ok(pipelines) => Ok(pipelines[0]),
        Err((_, e)) => Err(backend_err("create_graphics_pipelines", e)),
    }
}

/// What every draw in one pass shares.
struct PassRecording<'a> {
    device: &'a ash::Device,
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    /// The whole render area, which is also what an unclipped draw scissors to.
    area: vk::Rect2D,
    bindings: &'a crate::sampling::Bindings,
    materials: &'a crate::materials::Materials,
}

/// What has already been recorded, so each piece is set only where it changes.
///
/// Draws stay in submission order, and consecutive draws sharing a pipeline or
/// a clip are the common case rather than the exception.
struct RecordedState {
    pipeline: Option<vk::Pipeline>,
    descriptor_set: Option<vk::DescriptorSet>,
    scissor: vk::Rect2D,
    /// `None` until the first draw sets it, since a pass begins with the
    /// reference undefined rather than at any particular value.
    stencil_reference: Option<u32>,
}

impl RecordedState {
    /// The state a pass begins in, having just set the scissor to the whole
    /// render area.
    fn at_pass_start(area: vk::Rect2D) -> Self {
        Self {
            pipeline: None,
            descriptor_set: None,
            scissor: area,
            stencil_reference: None,
        }
    }
}

/// Record the per-draw state and the draw itself.
///
/// Shared between the waiting and deferred submission paths, which differ only
/// in how they resolve a pipeline. Keeping this in one place is what stops the
/// two from drifting: a scissor set in one and not the other would clip
/// correctly right up until a caller started presenting its frames.
///
/// # Safety
///
/// `cmd` must be recording, inside a render pass compatible with `pipeline`,
/// with the vertex and index buffers the draw's indices address already bound.
unsafe fn record_draw(
    pass: &PassRecording,
    draw: &emblema_hal::BatchDraw,
    index: usize,
    pipeline: vk::Pipeline,
    state: &mut RecordedState,
) {
    let PassRecording {
        device,
        cmd,
        layout,
        area,
        bindings,
        materials,
    } = *pass;
    unsafe {
        if state.pipeline != Some(pipeline) {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            state.pipeline = Some(pipeline);
        }

        // Vulkan's framebuffer origin is the top-left corner, which is the
        // convention `Scissor` is already stated in, so this is a widening
        // rather than a transformation. Clamping to the render area is not
        // optional: a scissor reaching outside it is invalid, and a clip
        // computed against a target the caller resized is exactly how that
        // happens.
        let wanted = match draw.clip {
            Some(clip) => {
                let clip = clip.clamped_to(emblema_hal::Extent2D::new(
                    area.extent.width,
                    area.extent.height,
                ));
                vk::Rect2D {
                    offset: vk::Offset2D {
                        x: clip.x as i32,
                        y: clip.y as i32,
                    },
                    extent: vk::Extent2D {
                        width: clip.width,
                        height: clip.height,
                    },
                }
            }
            None => area,
        };
        if state.scissor != wanted {
            device.cmd_set_scissor(cmd, 0, &[wanted]);
            state.scissor = wanted;
        }

        // Bound for every draw, not only the ones that sample: the shader
        // declares the texture whatever the paint is, and a descriptor a
        // pipeline statically uses must be bound even where the branch reading
        // it is unreachable. A draw that samples nothing gets a placeholder.
        let set = bindings.set_for(draw.material.texture_slots());
        if state.descriptor_set != Some(set) {
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                0,
                &[set],
                &[],
            );
            state.descriptor_set = Some(set);
        }

        // The clip depth is dynamic state, so nesting deeper is a state change
        // rather than another pipeline. It is set even for a pass with no
        // stencil attachment, where it is ignored -- writing it unconditionally
        // is cheaper than deciding, and leaves no path where a pipeline that
        // does test the stencil runs against a reference nobody set.
        let reference = draw.stencil.reference;
        if state.stencil_reference != Some(reference) {
            device.cmd_set_stencil_reference(cmd, vk::StencilFaceFlags::FRONT_AND_BACK, reference);
            state.stencil_reference = Some(reference);
        }

        // One set for the whole batch, rebound per draw with a different
        // offset into it. The set itself never changes, but a dynamic offset
        // travels with the binding call, so this is a rebind rather than a
        // separate command -- and cheaper than the descriptor update per draw
        // that a non-dynamic binding would need.
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            layout,
            1,
            &[materials.set()],
            &[materials.offset(index)],
        );
        device.cmd_draw_indexed(cmd, draw.index_count, 1, draw.first_index, 0, 0);
    }
}

/// Translate an advanced blend mode to the blend op that names its equation.
///
/// Every arm is a mode the extension defines with the same formula as the
/// compositing specification, which is what lets the conformance tests check
/// the hardware against the reference in the HAL. Panicking on a Porter-Duff
/// mode is right: callers reach this only through the `None` arm of `factors`,
/// so arriving here with one means the two classifications have diverged, and
/// guessing an op would turn that into a wrong picture instead of a crash.
fn vk_advanced_blend_op(mode: emblema_hal::BlendMode) -> vk::BlendOp {
    use emblema_hal::BlendMode;
    match mode {
        BlendMode::Multiply => vk::BlendOp::MULTIPLY_EXT,
        BlendMode::Screen => vk::BlendOp::SCREEN_EXT,
        BlendMode::Overlay => vk::BlendOp::OVERLAY_EXT,
        BlendMode::Darken => vk::BlendOp::DARKEN_EXT,
        BlendMode::Lighten => vk::BlendOp::LIGHTEN_EXT,
        BlendMode::ColorDodge => vk::BlendOp::COLORDODGE_EXT,
        BlendMode::ColorBurn => vk::BlendOp::COLORBURN_EXT,
        BlendMode::HardLight => vk::BlendOp::HARDLIGHT_EXT,
        BlendMode::SoftLight => vk::BlendOp::SOFTLIGHT_EXT,
        BlendMode::Difference => vk::BlendOp::DIFFERENCE_EXT,
        BlendMode::Exclusion => vk::BlendOp::EXCLUSION_EXT,
        // The non-separable four. Named HSL by the extension for the attributes
        // they exchange, which is the same thing the compositing specification
        // calls hue, saturation, color and luminosity.
        BlendMode::Hue => vk::BlendOp::HSL_HUE_EXT,
        BlendMode::Saturation => vk::BlendOp::HSL_SATURATION_EXT,
        BlendMode::Color => vk::BlendOp::HSL_COLOR_EXT,
        BlendMode::Luminosity => vk::BlendOp::HSL_LUMINOSITY_EXT,
        other => unreachable!("{other} is not an advanced blend mode"),
    }
}

/// Translate a portable blend factor.
fn vk_blend_factor(factor: emblema_hal::BlendFactor) -> vk::BlendFactor {
    use emblema_hal::BlendFactor;
    match factor {
        BlendFactor::Zero => vk::BlendFactor::ZERO,
        BlendFactor::One => vk::BlendFactor::ONE,
        BlendFactor::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
        BlendFactor::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        BlendFactor::DstAlpha => vk::BlendFactor::DST_ALPHA,
        BlendFactor::OneMinusDstAlpha => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
        BlendFactor::DstColor => vk::BlendFactor::DST_COLOR,
    }
}

/// Reinterpret a slice of plain data as bytes.
pub(crate) fn cast_bytes<T>(slice: &[T]) -> &[u8] {
    // SAFETY: T is a plain-data type here ([f32; 2], [f32; 4] or u32), every
    // byte of it is initialized, and the returned slice borrows the same memory
    // for a shorter-or-equal lifetime.
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

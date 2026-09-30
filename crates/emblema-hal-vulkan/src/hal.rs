//! The backend's implementation of the rendering HAL.
//!
//! Thin by design: every method forwards to an inherent method on
//! [`VulkanContext`]. Keeping the concrete API usable directly matters for
//! bring-up and for tests that need backend specifics, while the trait is what
//! the renderer and the conformance harness talk to.

use crate::device::VulkanContext;
use crate::fence::VulkanFence;
use crate::resource::VulkanTexture;
use emblema_hal::{
    Batch, Capabilities, Extent2D, Hal, HalContext, HalTexture, Modifier, PassDescriptor,
    PixelFormat, Result, TextureDescriptor,
};

/// Marker naming the Vulkan backend's family of types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VulkanHal;

impl Hal for VulkanHal {
    type Context = VulkanContext;
    type Texture = VulkanTexture;
    type Fence = VulkanFence;

    const NAME: &'static str = "vulkan";
}

impl HalTexture for VulkanTexture {
    fn extent(&self) -> Extent2D {
        VulkanTexture::extent(self)
    }

    fn format(&self) -> PixelFormat {
        VulkanTexture::format(self)
    }
}

impl HalContext for VulkanContext {
    type Hal = VulkanHal;

    fn capabilities(&self) -> &Capabilities {
        VulkanContext::capabilities(self)
    }

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<VulkanTexture> {
        VulkanContext::create_texture(self, desc)
    }

    fn destroy_texture(&mut self, texture: VulkanTexture) {
        VulkanContext::destroy_texture(self, texture)
    }

    fn register_program(&mut self, program: &emblema_hal::RuntimeProgram) -> Result<u32> {
        VulkanContext::register_program(self, program)
    }

    fn submit_batch(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<()> {
        VulkanContext::submit_batch(self, target, batch, pass)
    }

    fn submit_batch_textured(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        pass: PassDescriptor,
        textures: &[&VulkanTexture],
    ) -> Result<()> {
        VulkanContext::submit_batch_textured(self, target, batch, pass, textures)
    }

    fn read_texture(&mut self, texture: &mut VulkanTexture) -> Result<Vec<u8>> {
        VulkanContext::read_texture(self, texture)
    }

    fn write_texture(&mut self, texture: &mut VulkanTexture, pixels: &[u8]) -> Result<()> {
        VulkanContext::write_texture(self, texture, pixels)
    }

    fn create_exportable_texture(
        &mut self,
        extent: Extent2D,
        format: PixelFormat,
        modifiers: &[Modifier],
    ) -> Result<VulkanTexture> {
        VulkanContext::create_exportable_texture(self, extent, format, modifiers)
    }

    #[cfg(unix)]
    fn export_texture(
        &mut self,
        texture: &VulkanTexture,
    ) -> Result<emblema_hal::ExternalImageDesc> {
        VulkanContext::export_texture(self, texture)
    }

    fn submit_batch_deferred_textured(
        &mut self,
        target: &mut VulkanTexture,
        batch: &Batch,
        pass: PassDescriptor,
        textures: &[&VulkanTexture],
    ) -> Result<VulkanFence> {
        VulkanContext::submit_batch_deferred_synchronized(
            self,
            target,
            batch,
            pass,
            textures,
            Default::default(),
        )
    }

    fn retire_fence(&mut self, fence: VulkanFence) {
        VulkanContext::retire_fence(self, fence)
    }
}

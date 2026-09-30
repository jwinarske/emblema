//! Where a draw's paint lives on the way to the shader.
//!
//! # Why a uniform buffer rather than push constants
//!
//! Push constants were the obvious home: a material is small, they need no
//! allocation, and every device guarantees 128 bytes of them. That guarantee
//! is also the ceiling, and a material had grown to occupy it exactly -- so
//! the mechanism decided what a paint could hold, which is the wrong way round.
//! A color filter has to sit on top of whatever material is already there, and
//! a color matrix alone is twenty floats; neither fits beside a four-stop
//! gradient under any arrangement.
//!
//! The concern with moving was the hardware this targets: push constants are
//! the cheap path for small per-draw data on tile-based parts, and a
//! guaranteed minimum exists because parts that offer only the minimum are
//! exactly those. Impeller's own renderer settles it -- it places per-draw
//! uniform data in a per-frame host buffer, aligned to the backend's minimum
//! uniform offset, on the same class of hardware. A uniform buffer with a
//! dynamic offset per draw is what that is.
//!
//! # Shape
//!
//! One buffer per submission holding every draw's material end to end, each
//! padded to the device's minimum offset alignment, and one descriptor set
//! bound with a different dynamic offset per draw. The alternative -- a
//! descriptor set per draw -- would allocate in proportion to the batch and
//! rebind a set where rebinding an offset does.
//!
//! It is a second descriptor set rather than another binding in the texture
//! set, because the texture set includes a placeholder owned by the context
//! and outliving any one submission. Adding a per-submission buffer to it
//! would mean rewriting a long-lived set while an earlier submission might
//! still be reading it.

use crate::device::VulkanContext;
use crate::render::{cast_bytes, StagedBuffer};
use crate::resource::backend_err;
use ash::vk;
use emblema_hal::{Batch, Result, MATERIAL_FLOATS};

/// Bytes one material occupies before padding.
pub const MATERIAL_BYTES: u64 = (MATERIAL_FLOATS * 4) as u64;

/// The layout for the set holding the paint.
pub fn create_layout(device: &ash::Device) -> Result<vk::DescriptorSetLayout> {
    let bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    unsafe { device.create_descriptor_set_layout(&info, None) }
        .map_err(|e| backend_err("create_descriptor_set_layout", e))
}

/// One submission's materials: the buffer, the set that points at it, and the
/// stride between one draw's paint and the next.
pub struct Materials {
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    stride: u64,
}

impl Materials {
    pub fn set(&self) -> vk::DescriptorSet {
        self.set
    }

    /// The dynamic offset for the `index`th draw of the batch this was built
    /// from.
    pub fn offset(&self, index: usize) -> u32 {
        (index as u64 * self.stride) as u32
    }

    pub fn destroy(self, device: &ash::Device) {
        // SAFETY: the submission that used this set was waited on, and
        // destroying the pool frees every set allocated from it.
        unsafe { device.destroy_descriptor_pool(self.pool, None) };
    }
}

/// Pack a batch's materials into a buffer and point a descriptor set at it.
///
/// Returns the buffer separately from the set because the two have different
/// release rules: the buffer is host-visible memory the submission reads, and
/// travels with the geometry buffers, while the set's pool cannot be destroyed
/// until the command buffer reading it retires.
pub fn build(
    ctx: &mut VulkanContext,
    batch: &Batch,
    target_format: emblema_hal::PixelFormat,
) -> Result<(StagedBuffer, Materials)> {
    let stride = stride_for(ctx);
    let draws = batch.draw_count().max(1);

    // Written straight into the padded layout rather than packed and then
    // spread, so the padding between materials is written once as zeros and
    // nothing reads a gap that was never initialized.
    let mut bytes = vec![0u8; (draws as u64 * stride) as usize];
    for (index, draw) in batch.draws().iter().enumerate() {
        let packed = draw.to_uniform(target_format);
        let at = index * stride as usize;
        let words = cast_bytes(&packed);
        bytes[at..at + words.len()].copy_from_slice(words);
    }

    let buffer = ctx.upload(&bytes, vk::BufferUsageFlags::UNIFORM_BUFFER)?;

    let device = ctx.raw_device().clone();
    let layout = ctx.material_layout()?;
    let sizes = [vk::DescriptorPoolSize::default()
        .ty(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
        .descriptor_count(1)];
    let pool_info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(1)
        .pool_sizes(&sizes);
    let pool = match unsafe { device.create_descriptor_pool(&pool_info, None) } {
        Ok(pool) => pool,
        Err(e) => {
            ctx.release(buffer);
            return Err(backend_err("create_descriptor_pool", e));
        }
    };

    let layouts = [layout];
    let alloc = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    let set = match unsafe { device.allocate_descriptor_sets(&alloc) } {
        Ok(sets) => sets[0],
        Err(e) => {
            unsafe { device.destroy_descriptor_pool(pool, None) };
            ctx.release(buffer);
            return Err(backend_err("allocate_descriptor_sets", e));
        }
    };

    // The range is one material, not the whole buffer: a dynamic offset is
    // added to the offset here, and the range is what a draw may read from
    // there. Naming the whole buffer would let the last draw's offset plus the
    // range run past the end, which is invalid however little of it is read.
    let info = [vk::DescriptorBufferInfo::default()
        .buffer(buffer.buffer)
        .offset(0)
        .range(MATERIAL_BYTES)];
    let writes = [vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
        .buffer_info(&info)];
    // SAFETY: the set comes from the pool above and nothing reads it yet.
    unsafe { device.update_descriptor_sets(&writes, &[]) };

    Ok((buffer, Materials { pool, set, stride }))
}

/// Distance between one draw's material and the next.
///
/// The device's minimum offset alignment can be as much as 256 bytes, so this
/// is usually padding rather than a tight pack. Paying it is what makes one
/// buffer and one set serve a whole batch.
fn stride_for(ctx: &VulkanContext) -> u64 {
    let alignment = ctx.uniform_alignment().max(1);
    MATERIAL_BYTES.div_ceil(alignment) * alignment
}

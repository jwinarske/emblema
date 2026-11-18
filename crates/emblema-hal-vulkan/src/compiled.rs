//! What the driver compiled, kept for another run.
//!
//! A pipeline is compiled by the driver the first time it is built, and on a
//! small board that is most of a second each: a frame that draws something
//! for the first time waits for it. Mesa keeps what it compiled on disk by
//! itself; other drivers keep nothing, and every run compiles again. Vulkan's
//! own answer is the pipeline cache: every pipeline here is built through
//! one, its contents can be read out as bytes, and bytes read out by an
//! earlier run can be handed back before anything is built.
//!
//! Where the bytes are kept is the caller's: this crate reads and writes no
//! files. What it does is refuse bytes that are not this device's. The
//! specification has the driver check the header as well, but the bytes are
//! from a file anybody may have written, and a driver that trusted its own
//! header check less than it should is not something to find out from a
//! crash inside it.

use ash::vk;

use emblema_hal::Result;

use crate::device::VulkanContext;
use crate::resource::backend_err;

/// The header every pipeline cache begins with (version one, the only one).
const HEADER_LEN: usize = 32;

/// More than this is not handed to the driver: a cache of every pipeline
/// this renderer builds is some hundreds of kilobytes.
pub const PIPELINE_CACHE_LIMIT: usize = 64 << 20;

/// Whether `data` begins with the header of a cache this device wrote.
fn is_of(data: &[u8], properties: &vk::PhysicalDeviceProperties) -> bool {
    if data.len() < HEADER_LEN || data.len() > PIPELINE_CACHE_LIMIT {
        return false;
    }
    let word = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    word(0) as usize == HEADER_LEN
        && word(4) == vk::PipelineCacheHeaderVersion::ONE.as_raw() as u32
        && word(8) == properties.vendor_id
        && word(12) == properties.device_id
        && data[16..HEADER_LEN] == properties.pipeline_cache_uuid
}

impl VulkanContext {
    /// The cache every pipeline is built through, made empty at its first use.
    pub(crate) fn driver_cache(&mut self) -> Result<vk::PipelineCache> {
        let held = self.pipeline_cache().driver;
        if held != vk::PipelineCache::null() {
            return Ok(held);
        }
        let info = vk::PipelineCacheCreateInfo::default();
        // SAFETY: the device is live, and the cache is destroyed with the
        // context's pipelines.
        let made = unsafe { self.raw_device().create_pipeline_cache(&info, None) }
            .map_err(|e| backend_err("create_pipeline_cache", e))?;
        self.pipeline_cache_mut().driver = made;
        Ok(made)
    }

    /// Hand back what [`Self::pipeline_cache_data`] gave an earlier run, so
    /// pipelines the driver compiled then are not compiled again.
    ///
    /// Returns whether the bytes were taken. Bytes that are not a cache
    /// written by this device and this driver build (too short, longer than
    /// [`PIPELINE_CACHE_LIMIT`], or with another header) are left alone and
    /// `false` returned: a driver update makes every older cache one of
    /// those, and the caller's next save replaces it. A driver that refuses
    /// bytes whose header it wrote is `false` as well, and so is what a
    /// driver that keeps nothing gives for its data: lavapipe's is 20 bytes,
    /// short of a header.
    ///
    /// Call it before the first pipeline is built for it to matter; after,
    /// it adds to what the context has.
    ///
    /// # Errors
    ///
    /// Whatever making the context's own cache reports, which is not about
    /// the bytes.
    pub fn load_pipeline_cache(&mut self, data: &[u8]) -> Result<bool> {
        // SAFETY: the physical device is the context's own.
        let properties = unsafe {
            self.raw_instance()
                .get_physical_device_properties(self.raw_physical_device())
        };
        if !is_of(data, &properties) {
            return Ok(false);
        }
        let own = self.driver_cache()?;
        let info = vk::PipelineCacheCreateInfo::default().initial_data(data);
        // SAFETY: the device is live; `data` outlives the call, which copies
        // what it keeps.
        let Ok(loaded) = (unsafe { self.raw_device().create_pipeline_cache(&info, None) }) else {
            return Ok(false);
        };
        // SAFETY: both caches are this device's, neither is in use by another
        // thread (the context is not shared), and `loaded` is used nowhere
        // after it is merged.
        let merged = unsafe {
            let merged = self.raw_device().merge_pipeline_caches(own, &[loaded]);
            self.raw_device().destroy_pipeline_cache(loaded, None);
            merged
        };
        Ok(merged.is_ok())
    }

    /// What the driver has compiled for this context's pipelines, for
    /// [`Self::load_pipeline_cache`] in a later run. Empty if no pipeline has
    /// been built or loaded.
    ///
    /// The bytes are the driver's own and mean nothing to another device or
    /// another build of the driver. They hold compiled shaders and nothing of
    /// what was drawn.
    ///
    /// # Errors
    ///
    /// Whatever the driver reports reading its cache out.
    pub fn pipeline_cache_data(&self) -> Result<Vec<u8>> {
        let held = self.pipeline_cache().driver;
        if held == vk::PipelineCache::null() {
            return Ok(Vec::new());
        }
        // SAFETY: the cache is the context's own and the device is live.
        unsafe { self.raw_device().get_pipeline_cache_data(held) }
            .map_err(|e| backend_err("get_pipeline_cache_data", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties() -> vk::PhysicalDeviceProperties {
        vk::PhysicalDeviceProperties {
            vendor_id: 0x1010,
            device_id: 0x36,
            pipeline_cache_uuid: [7; 16],
            ..Default::default()
        }
    }

    fn header(len: u32, version: u32, vendor: u32, device: u32, uuid: [u8; 16]) -> Vec<u8> {
        let mut data = Vec::new();
        for word in [len, version, vendor, device] {
            data.extend_from_slice(&word.to_le_bytes());
        }
        data.extend_from_slice(&uuid);
        data
    }

    #[test]
    fn only_this_device_s_cache_is_taken() {
        let p = properties();
        let good = header(32, 1, 0x1010, 0x36, [7; 16]);
        assert!(is_of(&good, &p));
        let mut longer = good.clone();
        longer.extend_from_slice(&[0; 100]);
        assert!(is_of(&longer, &p));

        assert!(!is_of(&[], &p));
        assert!(!is_of(&good[..31], &p));
        assert!(!is_of(&header(36, 1, 0x1010, 0x36, [7; 16]), &p));
        assert!(!is_of(&header(32, 2, 0x1010, 0x36, [7; 16]), &p));
        assert!(!is_of(&header(32, 1, 0x1011, 0x36, [7; 16]), &p));
        assert!(!is_of(&header(32, 1, 0x1010, 0x37, [7; 16]), &p));
        assert!(!is_of(&header(32, 1, 0x1010, 0x36, [8; 16]), &p));
    }
}

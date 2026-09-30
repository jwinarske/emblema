//! Resource descriptions: what the renderer asks a backend to allocate.

use crate::format::{Extent2D, Fourcc, Modifier, PixelFormat};

/// How a texture will be used, which backends need up front to choose a
/// layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureUsage {
    /// Bound as a shader input.
    pub sampled: bool,
    /// Used as a color attachment.
    pub render_target: bool,
    /// Source or destination of a copy.
    pub transfer: bool,
    /// Will be exported for scanout.
    ///
    /// Backends that can allocate with explicit modifiers use this to pick a
    /// layout the display controller accepts, rather than one that is optimal
    /// for rendering alone.
    pub scanout: bool,
}

impl TextureUsage {
    /// Read by a shader and written from the processor, never rendered into.
    pub const fn sampled() -> Self {
        Self {
            sampled: true,
            render_target: false,
            transfer: true,
            scanout: false,
        }
    }

    /// A plain offscreen render target: the golden and conformance suites run
    /// entirely on these.
    pub const fn offscreen() -> Self {
        Self {
            sampled: true,
            render_target: true,
            transfer: true,
            scanout: false,
        }
    }
}

/// One plane of a dma-buf image.
#[cfg(unix)]
#[derive(Debug)]
pub struct DmaBufPlane {
    /// The dma-buf file descriptor. Ownership transfers to the importer.
    pub fd: std::os::fd::OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// An image that already exists, to be imported rather than allocated.
///
/// This is the first of the two requirements that exist in the HAL from day
/// one for the sake of the DRM path. The renderer must be able to draw into
/// images it did not allocate — buffers GBM made, or buffers another device
/// allocated and shared — because on a split render/display SoC the display
/// controller and the GPU are different devices with different ideas about
/// memory layout.
#[cfg(unix)]
#[derive(Debug)]
pub struct ExternalImageDesc {
    /// Planes making up the image. Most formats are single-plane.
    pub planes: Vec<DmaBufPlane>,
    /// The DRM format code the buffer was allocated with.
    pub fourcc: Fourcc,
    /// The layout the buffer was allocated with.
    ///
    /// [`Modifier::INVALID`] is accepted only where no negotiation took place;
    /// a buffer that came out of format negotiation always carries the
    /// explicit modifier that was agreed on.
    pub modifier: Modifier,
}

/// What to allocate, or what to import.
#[derive(Debug)]
pub struct TextureDescriptor {
    pub extent: Extent2D,
    pub format: PixelFormat,
    pub usage: TextureUsage,
    /// Sample count for MSAA targets. 1 means single-sampled.
    pub sample_count: u32,
    /// How many mip levels this texture holds. 1 is the image alone.
    ///
    /// A chain is not made for every texture, because it costs a third again
    /// in memory and a pass of downsampling on upload, and most textures here
    /// are drawn at or above their own size where it would never be read. A
    /// caller who will minify states it, and [`Self::mipmapped`] works out how
    /// many levels that takes.
    ///
    /// Levels past the first are filled by the backend when the texture is
    /// written, not by the caller: there is no way to hand them in, because a
    /// chain a caller built by some other rule would sample differently on the
    /// two backends and this renderer's whole test model is that they agree.
    pub mip_levels: u32,
    /// When present, import this existing image instead of allocating.
    #[cfg(unix)]
    pub external: Option<ExternalImageDesc>,
}

/// How many mip levels an image of this size has, counting the image itself.
///
/// Halving the larger axis until it reaches one texel, which is what both
/// backends mean by a complete chain: a level is not required to be square, and
/// an axis that reaches one stays there while the other keeps halving.
pub fn mip_levels_for(extent: Extent2D) -> u32 {
    let longest = extent.width.max(extent.height).max(1);
    // `ilog2` of a power of two is the exponent, and of anything else is the
    // exponent below it -- which is the count of halvings that still leave more
    // than one texel, so adding the level for the image itself is the whole
    // chain either way.
    longest.ilog2() + 1
}

impl TextureDescriptor {
    /// An offscreen render target, the workhorse of the test suites.
    pub fn offscreen(extent: Extent2D, format: PixelFormat) -> Self {
        Self {
            extent,
            format,
            usage: TextureUsage::offscreen(),
            sample_count: 1,
            mip_levels: 1,
            #[cfg(unix)]
            external: None,
        }
    }

    /// A texture only ever read by a shader.
    ///
    /// A baked gradient ramp and an uploaded image are both this: written once
    /// from the processor, sampled many times, never drawn into. Saying so
    /// costs a backend nothing and saves it a color attachment -- which is not
    /// merely tidiness on GLES, where a format can be filterable as a texture
    /// and not renderable as an attachment. Asking for a target a caller does
    /// not need is how a texture that would have worked fails to be created.
    pub fn sampled(extent: Extent2D, format: PixelFormat) -> Self {
        Self {
            extent,
            format,
            usage: TextureUsage::sampled(),
            sample_count: 1,
            mip_levels: 1,
            #[cfg(unix)]
            external: None,
        }
    }

    /// The same, with a full mip chain.
    ///
    /// Every level down to a single texel, which is what a caller minifying by
    /// an unknown amount needs and is only a third again in memory however far
    /// it goes -- each level is a quarter of the one above, and a quarter
    /// summed forever is a third.
    pub fn mipmapped(extent: Extent2D, format: PixelFormat) -> Self {
        Self {
            mip_levels: mip_levels_for(extent),
            ..Self::offscreen(extent, format)
        }
    }

    /// Whether this texture holds more than the image itself.
    pub fn is_mipmapped(&self) -> bool {
        self.mip_levels > 1
    }

    /// Whether this describes an import rather than an allocation.
    pub fn is_external(&self) -> bool {
        #[cfg(unix)]
        {
            self.external.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

/// How a buffer will be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BufferUsage {
    pub vertex: bool,
    pub index: bool,
    pub uniform: bool,
    pub transfer: bool,
}

#[derive(Debug, Clone)]
pub struct BufferDescriptor {
    pub size: u64,
    pub usage: BufferUsage,
    /// Whether the host needs to write to this buffer directly, as per-frame
    /// ring allocations do.
    pub host_visible: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offscreen_targets_are_not_external() {
        let desc = TextureDescriptor::offscreen(Extent2D::new(1920, 1080), PixelFormat::Rgba8Unorm);
        assert!(!desc.is_external());
        assert_eq!(desc.sample_count, 1);
        assert!(desc.usage.render_target);
        // Offscreen targets are read back and sampled, so both must be set for
        // the golden suites to work.
        assert!(desc.usage.transfer);
        assert!(desc.usage.sampled);
        assert!(!desc.usage.scanout);
    }

    #[test]
    fn scanout_usage_is_distinct_from_render_target_usage() {
        // A backend allocating for scanout must pick a layout the display
        // controller accepts, which is not necessarily the one that is fastest
        // to render into, so the two flags cannot be conflated.
        let usage = TextureUsage {
            scanout: true,
            ..TextureUsage::offscreen()
        };
        assert!(usage.scanout);
        assert!(usage.render_target);
        assert!(!TextureUsage::offscreen().scanout);
    }
}

//! A rendering context, over whichever backend is available.

use crate::backend::{no_backend, Backend, BackendPreference};
use emblema_core::Recording;
use emblema_hal::{Capabilities, Error, Extent2D, HalContext, PixelFormat, Result, RuntimeProgram};

/// A device to render with.
///
/// Which backend is behind it is decided at creation and reported by
/// [`Context::backend`]. Callers branch on [`Context::capabilities`] rather than
/// on which backend it turned out to be: a device without fence export is the
/// same problem whether it is Vulkan or GLES.
// The variants differ in size by a couple of kilobytes, and boxing the larger
// one would even them out. That trade is backwards here: a process creates one
// context and keeps it for its lifetime, so the saving is a one-off couple of
// kilobytes, while the cost is an indirection on the way to every backend call
// in the frame loop.
#[allow(clippy::large_enum_variant)]
pub enum Context {
    #[cfg(feature = "vulkan")]
    Vulkan(emblema_hal_vulkan::VulkanContext),
    #[cfg(feature = "gles")]
    Gles(emblema_hal_gles::GlesContext),
}

impl Context {
    /// Create a context, choosing a backend.
    pub fn new(preference: BackendPreference) -> Result<Self> {
        let mut attempts = Vec::new();

        #[cfg(feature = "vulkan")]
        if matches!(
            preference,
            BackendPreference::Auto | BackendPreference::Vulkan
        ) {
            match emblema_hal_vulkan::VulkanContext::new(emblema_hal_vulkan::DevicePreference::Auto)
            {
                Ok(ctx) => return Ok(Self::Vulkan(ctx)),
                Err(e) => attempts.push((Backend::Vulkan, e)),
            }
        }

        #[cfg(feature = "gles")]
        if matches!(
            preference,
            BackendPreference::Auto | BackendPreference::Gles
        ) {
            match emblema_hal_gles::GlesContext::new(emblema_hal_gles::DisplayTarget::Surfaceless) {
                Ok(ctx) => return Ok(Self::Gles(ctx)),
                Err(e) => attempts.push((Backend::Gles, e)),
            }
        }

        let _ = preference;
        Err(no_backend(attempts))
    }

    pub fn backend(&self) -> Backend {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(_) => Backend::Vulkan,
            #[cfg(feature = "gles")]
            Self::Gles(_) => Backend::Gles,
        }
    }

    pub fn capabilities(&self) -> &Capabilities {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => HalContext::capabilities(ctx),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => HalContext::capabilities(ctx),
        }
    }

    /// Create a surface to draw into.
    pub fn create_surface(&mut self, extent: Extent2D, format: PixelFormat) -> Result<Surface> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => Ok(Surface::Vulkan(HalContext::create_texture(
                ctx,
                &emblema_hal::TextureDescriptor::offscreen(extent, format),
            )?)),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => Ok(Surface::Gles(HalContext::create_texture(
                ctx,
                &emblema_hal::TextureDescriptor::offscreen(extent, format),
            )?)),
        }
    }

    /// Draw a recording into a surface.
    pub fn draw(&mut self, surface: &mut Surface, recording: &Recording) -> Result<()> {
        self.draw_with_images(surface, recording, &[])
    }

    /// Draw a recording whose paints sample images.
    ///
    /// `images` is the table a [`Shader::Image`] slot indexes, in slot order.
    /// A recording carries slots rather than images because it is produced
    /// without touching a device; supplying the table here is what resolves
    /// them.
    ///
    /// The surface is borrowed mutably and the images immutably, so a recording
    /// cannot sample the surface it draws into. That restriction is real, and
    /// having the compiler state it beats discovering it as a picture that
    /// differs by driver.
    ///
    /// [`Shader::Image`]: emblema_core::Shader::Image
    pub fn draw_with_images(
        &mut self,
        surface: &mut Surface,
        recording: &Recording,
        images: &[&Image],
    ) -> Result<()> {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => {
                let images = vulkan_textures(images)?;
                emblema_core::execute::<emblema_hal_vulkan::VulkanHal>(
                    ctx, texture, recording, &images,
                )
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => {
                let images = gles_textures(images)?;
                emblema_core::execute::<emblema_hal_gles::GlesHal>(ctx, texture, recording, &images)
            }
            // A surface belongs to the context that made it; pairing one with
            // another context would use a handle the device never allocated.
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this surface was created by a different context",
            )),
        }
    }

    /// Register a fragment program a caller's own build produced.
    ///
    /// Returns the index to name it by, which is what goes into the `program`
    /// of [`Shader::RuntimeEffect`]. Indices are per context: a recording that
    /// names one is bound to the context that registered it, in exactly the
    /// way one naming a texture slot is bound to the textures supplied
    /// beside it.
    ///
    /// Nothing here compiles a shader. The payload for the backend in use has
    /// to be present -- SPIR-V for Vulkan, GLSL ES for GLES -- and one built
    /// for the other backend alone is refused rather than silently ignored.
    /// See the architecture document for why a single source translated on
    /// load was declined.
    ///
    /// [`Shader::RuntimeEffect`]: emblema_core::Shader::RuntimeEffect
    pub fn register_program(&mut self, program: &RuntimeProgram) -> Result<u32> {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => ctx.register_program(program),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => ctx.register_program(program),
        }
    }

    /// Allocate an image this context can sample.
    ///
    /// The format decides how the bytes written into it are read, and the
    /// choice is not cosmetic -- though it now points the other way. Color
    /// inside the renderer is sRGB-encoded, and an sRGB format decodes on
    /// sample. A picture's bytes are sRGB-encoded,
    /// because that is what every image file holds, and the pipeline wants them
    /// encoded -- so a picture wants a *plain* format, and
    /// [`PixelFormat::Rgba8UnormSrgb`] would decode it into light that nothing
    /// downstream is expecting and wash it pale. Plain is also right for data
    /// that is not color at all: coverage, a mask, a lookup table, anything
    /// whose numbers mean themselves.
    ///
    /// This advice was the reverse of itself while the renderer worked in
    /// light, and neither choice is detectable afterwards, which is why it is
    /// stated here. Both produce an image; only one produces the right
    /// picture.
    pub fn create_image(&mut self, extent: Extent2D, format: PixelFormat) -> Result<Image> {
        let descriptor = emblema_hal::TextureDescriptor::offscreen(extent, format);
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => HalContext::create_texture(ctx, &descriptor).map(Image::Vulkan),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => HalContext::create_texture(ctx, &descriptor).map(Image::Gles),
        }
    }

    /// An image with a full mip chain, for drawing smaller than its own size.
    ///
    /// The levels are filled when the image is written, not here, and the
    /// caller supplies only the image itself: a chain built by any rule other
    /// than the backend's own would sample differently on the two backends,
    /// which is the one thing this renderer's tests exist to rule out.
    ///
    /// A third again in memory, which is why it is a separate call rather than
    /// what every image gets. Only [`Sampling::Mipmap`] reads past the first
    /// level, so an image created this way and drawn at any other quality costs
    /// the memory and nothing else.
    ///
    /// [`Sampling::Mipmap`]: crate::Sampling::Mipmap
    pub fn create_mipmapped_image(
        &mut self,
        extent: Extent2D,
        format: PixelFormat,
    ) -> Result<Image> {
        let descriptor = emblema_hal::TextureDescriptor::mipmapped(extent, format);
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(ctx) => HalContext::create_texture(ctx, &descriptor).map(Image::Vulkan),
            #[cfg(feature = "gles")]
            Self::Gles(ctx) => HalContext::create_texture(ctx, &descriptor).map(Image::Gles),
        }
    }

    /// Fill an image from host memory, tightly packed and top row first.
    ///
    /// Decoding is out of scope for this crate; bring `image` or another
    /// decoder and hand the pixels here.
    pub fn write_image(&mut self, image: &mut Image, pixels: &[u8]) -> Result<()> {
        match (self, image) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Image::Vulkan(texture)) => {
                HalContext::write_texture(ctx, texture, pixels)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Image::Gles(texture)) => {
                HalContext::write_texture(ctx, texture, pixels)
            }
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this image was created by a different context",
            )),
        }
    }

    /// Release an image.
    ///
    /// Explicit for the same reason a surface is: the memory belongs to the
    /// context, which an image cannot reach from its own `Drop`.
    pub fn destroy_image(&mut self, image: Image) {
        match (self, image) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Image::Vulkan(texture)) => {
                HalContext::destroy_texture(ctx, texture)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Image::Gles(texture)) => HalContext::destroy_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// Read a surface back, tightly packed, in whatever its format holds.
    ///
    /// Four bytes per pixel for the eight-bit formats, which is every surface a
    /// caller is likely to ask for; eight for `Rgba16Float`, as four
    /// half-floats. Nothing is converted on the way out -- the bytes are what
    /// the target holds, and a caller who chose a format wider than eight bits
    /// did so in order to read what it kept.
    pub fn read(&mut self, surface: &mut Surface) -> Result<Vec<u8>> {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => HalContext::read_texture(ctx, texture),
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => HalContext::read_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this surface was created by a different context",
            )),
        }
    }

    /// Release a surface.
    ///
    /// Explicit because the memory belongs to the context, which a surface
    /// cannot reach from its own `Drop`.
    pub fn destroy_surface(&mut self, surface: Surface) {
        match (self, surface) {
            #[cfg(feature = "vulkan")]
            (Self::Vulkan(ctx), Surface::Vulkan(texture)) => {
                HalContext::destroy_texture(ctx, texture)
            }
            #[cfg(feature = "gles")]
            (Self::Gles(ctx), Surface::Gles(texture)) => HalContext::destroy_texture(ctx, texture),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

/// Something to draw into.
/// A texture a paint can sample.
///
/// Distinct from a [`Surface`] in the type system even though both are textures
/// underneath, because the two are used in opposite directions and mixing them
/// up is exactly the mistake worth making impossible: a surface is drawn into,
/// an image is read from.
pub enum Image {
    #[cfg(feature = "vulkan")]
    Vulkan(emblema_hal_vulkan::VulkanTexture),
    #[cfg(feature = "gles")]
    Gles(emblema_hal_gles::GlesTexture),
}

impl Image {
    pub fn extent(&self) -> Extent2D {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.extent(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.extent(),
        }
    }
}

/// Unwrap a table of images to one backend's textures.
///
/// An image from another backend is an error rather than a skip: the slots are
/// positional, so dropping one would silently shift every slot after it.
#[cfg(feature = "vulkan")]
fn vulkan_textures<'a>(images: &[&'a Image]) -> Result<Vec<&'a emblema_hal_vulkan::VulkanTexture>> {
    images
        .iter()
        .map(|image| match image {
            Image::Vulkan(texture) => Ok(texture),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this image was created by a different context",
            )),
        })
        .collect()
}

#[cfg(feature = "gles")]
fn gles_textures<'a>(images: &[&'a Image]) -> Result<Vec<&'a emblema_hal_gles::GlesTexture>> {
    images
        .iter()
        .map(|image| match image {
            Image::Gles(texture) => Ok(texture),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported(
                "this image was created by a different context",
            )),
        })
        .collect()
}

pub enum Surface {
    #[cfg(feature = "vulkan")]
    Vulkan(emblema_hal_vulkan::VulkanTexture),
    #[cfg(feature = "gles")]
    Gles(emblema_hal_gles::GlesTexture),
}

impl Surface {
    pub fn extent(&self) -> Extent2D {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.extent(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.extent(),
        }
    }

    pub fn format(&self) -> PixelFormat {
        match self {
            #[cfg(feature = "vulkan")]
            Self::Vulkan(texture) => texture.format(),
            #[cfg(feature = "gles")]
            Self::Gles(texture) => texture.format(),
        }
    }
}

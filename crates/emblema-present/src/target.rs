//! The presentation trait, and the offscreen target.

use emblema_hal::{Extent2D, Hal, HalContext, PixelFormat, Result, TextureDescriptor};

/// Where finished frames go, and what paces the frame loop.
///
/// Presentation is the axis orthogonal to the rendering HAL: the HAL turns draw
/// commands into pixels in a GPU image, and a target decides how that image
/// reaches a display and when the next frame may start. Pacing lives here
/// because it differs completely between targets — swapchain acquire semantics
/// for a window, page-flip completion for direct scanout — while the renderer
/// above is identical either way.
///
/// The methods take a context because every backend's resources belong to one,
/// and a target holds images the backend allocated.
pub trait PresentTarget<H: Hal> {
    /// Current output geometry, which changes on resize or hotplug.
    fn extent(&self) -> Extent2D;

    fn format(&self) -> PixelFormat;

    /// Wait until a frame slot is available and return what to render into.
    ///
    /// This is where a target blocks, according to its own pacing policy. The
    /// renderer never blocks on presentation internals; it draws into whatever
    /// image it is handed.
    fn acquire(&mut self, ctx: &mut H::Context) -> Result<&mut H::Texture>;

    /// Submit the acquired frame for display.
    fn present(&mut self, ctx: &mut H::Context) -> Result<()>;

    /// Rebuild for new output geometry.
    fn reconfigure(&mut self, ctx: &mut H::Context, extent: Extent2D) -> Result<()>;

    /// Release everything the target holds.
    ///
    /// Explicit rather than a `Drop` impl because releasing needs the context
    /// that allocated the images, and `Drop` cannot ask for one.
    fn destroy(self, ctx: &mut H::Context);
}

/// A target that renders into an image and never displays it.
///
/// A first-class citizen rather than a test affordance: the entire golden and
/// conformance apparatus runs on this, so it is the target most exercised and
/// the one whose behavior every other target is compared against.
pub struct OffscreenTarget<H: Hal> {
    texture: Option<H::Texture>,
    extent: Extent2D,
    format: PixelFormat,
    /// Frames presented so far, which is what a pacing-free target can honestly
    /// report about its own progress.
    presented: u64,
}

impl<H: Hal> OffscreenTarget<H>
where
    H::Context: HalContext<Hal = H>,
{
    pub fn new(ctx: &mut H::Context, extent: Extent2D, format: PixelFormat) -> Result<Self> {
        let texture = ctx.create_texture(&TextureDescriptor::offscreen(extent, format))?;
        Ok(Self {
            texture: Some(texture),
            extent,
            format,
            presented: 0,
        })
    }

    pub fn presented_frames(&self) -> u64 {
        self.presented
    }

    /// Read the current contents back.
    pub fn read(&mut self, ctx: &mut H::Context) -> Result<Vec<u8>> {
        let texture = self
            .texture
            .as_mut()
            .expect("an offscreen target always holds its image");
        ctx.read_texture(texture)
    }
}

impl<H: Hal> PresentTarget<H> for OffscreenTarget<H>
where
    H::Context: HalContext<Hal = H>,
{
    fn extent(&self) -> Extent2D {
        self.extent
    }

    fn format(&self) -> PixelFormat {
        self.format
    }

    fn acquire(&mut self, _ctx: &mut H::Context) -> Result<&mut H::Texture> {
        // Never blocks: there is one image and nothing displaying it, so a
        // frame slot is always free. Targets that pace are the ones that wait.
        Ok(self
            .texture
            .as_mut()
            .expect("an offscreen target always holds its image"))
    }

    fn present(&mut self, _ctx: &mut H::Context) -> Result<()> {
        // Nothing to submit. Counting is what makes a frame loop over this
        // target observable, which the corpus runner relies on.
        self.presented += 1;
        Ok(())
    }

    fn reconfigure(&mut self, ctx: &mut H::Context, extent: Extent2D) -> Result<()> {
        if extent == self.extent {
            return Ok(());
        }
        // The new image is allocated before the old one is released, so a
        // failure leaves the target still usable at its previous size rather
        // than holding nothing.
        let replacement = ctx.create_texture(&TextureDescriptor::offscreen(extent, self.format))?;
        if let Some(old) = self.texture.replace(replacement) {
            ctx.destroy_texture(old);
        }
        self.extent = extent;
        Ok(())
    }

    fn destroy(mut self, ctx: &mut H::Context) {
        if let Some(texture) = self.texture.take() {
            ctx.destroy_texture(texture);
        }
    }
}

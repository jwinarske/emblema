//! Presenting through an EGL window surface.
//!
//! # Where the surface comes from
//!
//! A caller brings one, exactly as the Vulkan swapchain target takes a
//! `VkSurfaceKHR`. An application already has a window system connection and a
//! window; turning those into an `EGLSurface` is one call, and owning that
//! relationship would mean owning a windowing library.
//!
//! # Why rendering does not go straight into the window
//!
//! It could, and it would be wrong side up. Everything this renderer draws is
//! oriented so that a framebuffer's row zero holds the image's *top* row: the
//! shader translator negates Y when targeting GLSL, which is what makes reading
//! a target back need no flip and what makes the two backends agree pixel for
//! pixel. A window system reads a framebuffer the other way — row zero is the
//! bottom of what it shows — so handing it that content displays the frame
//! upside down.
//!
//! Rendering into an offscreen target and blitting it to the window with the
//! rows exchanged puts the flip in exactly one place, at the moment the image
//! stops being something this renderer reads and becomes something a window
//! system does. The alternative is a second orientation convention threaded
//! through the projection, the scissor, the stencil and the readback, each of
//! which then behaves differently depending on where the frame is going.
//!
//! The cost is one full-screen blit per frame. That is what an offscreen-then-
//! present design costs anywhere, and a multisampled frame is already paying it
//! for the resolve.
//!
//! # What the tests can and cannot see
//!
//! An EGL pbuffer is a surface with no window behind it, so surface creation,
//! making it current, the blit and the buffer swap all run with no display
//! present. What no test here can observe is what a window system would
//! actually put on screen. So the property checked is the one that decides it:
//! a presented frame must be the vertical mirror of the frame as rendered.

use emblema_hal::{Error, Extent2D, PixelFormat, Result, TextureDescriptor};
use emblema_hal_gles::{GlesContext, GlesHal, GlesTexture};
use emblema_present::PresentTarget;
use glow::HasContext;

/// A window surface and the offscreen target frames are drawn into.
pub struct WindowTarget {
    surface: khronos_egl::Surface,
    texture: Option<GlesTexture>,
    extent: Extent2D,
    format: PixelFormat,
    acquired: bool,
    presented: u64,
}

impl WindowTarget {
    /// Build a target for a surface the caller created.
    ///
    /// The surface is borrowed rather than adopted: it outlives this and is the
    /// caller's to destroy, because it was theirs to create.
    pub fn new(
        ctx: &mut GlesContext,
        surface: khronos_egl::Surface,
        extent: Extent2D,
        format: PixelFormat,
    ) -> Result<Self> {
        // The context was created surfaceless. Binding the surface now is what
        // gives the default framebuffer somewhere to go, and it stays bound
        // until this target is destroyed.
        ctx.make_surface_current(Some(surface))?;
        let texture = match ctx.create_texture(&TextureDescriptor::offscreen(extent, format)) {
            Ok(texture) => texture,
            Err(e) => {
                let _ = ctx.make_surface_current(None);
                return Err(e);
            }
        };
        Ok(Self {
            surface,
            texture: Some(texture),
            extent,
            format,
            acquired: false,
            presented: 0,
        })
    }

    /// Frames handed to the window system so far.
    pub fn presented_frames(&self) -> u64 {
        self.presented
    }

    /// Read the window's own buffer back, top row first.
    ///
    /// Reads the default framebuffer rather than the offscreen target, so what
    /// comes back is what the window system was given. The rows are exchanged
    /// on the way out for the same reason they were on the way in: this returns
    /// images in the renderer's orientation, and the window's buffer is in the
    /// other one.
    ///
    /// Present before calling this, or the buffer holds whatever the last swap
    /// left in it.
    pub fn read_presented(&mut self, ctx: &mut GlesContext) -> Result<Vec<u8>> {
        let bytes = self.format.bytes_per_pixel() as usize;
        let stride = self.extent.width as usize * bytes;
        let mut pixels = vec![0u8; stride * self.extent.height as usize];

        let gl = ctx.raw_gl();
        // SAFETY: the surface is current and the destination is sized for the
        // region being read.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                self.extent.width as i32,
                self.extent.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(&mut pixels),
            );
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("reading the window buffer produced GL error {error:#x}"),
                });
            }
        }

        let mut flipped = vec![0u8; pixels.len()];
        for row in 0..self.extent.height as usize {
            let from = (self.extent.height as usize - 1 - row) * stride;
            flipped[row * stride..(row + 1) * stride].copy_from_slice(&pixels[from..from + stride]);
        }
        Ok(flipped)
    }
}

impl PresentTarget<GlesHal> for WindowTarget {
    fn extent(&self) -> Extent2D {
        self.extent
    }

    fn format(&self) -> PixelFormat {
        self.format
    }

    fn acquire(&mut self, _ctx: &mut GlesContext) -> Result<&mut GlesTexture> {
        if self.acquired {
            return Err(Error::Unsupported(
                "acquire without a matching present; one frame is already in flight",
            ));
        }
        self.acquired = true;
        self.texture
            .as_mut()
            .ok_or(Error::Unsupported("this target has been destroyed"))
    }

    fn present(&mut self, ctx: &mut GlesContext) -> Result<()> {
        if !self.acquired {
            return Err(Error::Unsupported("present without a matching acquire"));
        }
        self.acquired = false;
        let texture = self
            .texture
            .as_ref()
            .ok_or(Error::Unsupported("this target has been destroyed"))?;

        // The frame is blitted out of the texture's own framebuffer, so a target
        // built without one has nothing to present from. It cannot happen here
        // -- this target creates its own texture as a render target -- and
        // saying so beats binding zero, which would silently present whatever
        // the default framebuffer held.
        let framebuffer = texture.raw_framebuffer().ok_or(Error::Unsupported(
            "presenting a texture that is not a render target",
        ))?;

        let (width, height) = (self.extent.width as i32, self.extent.height as i32);
        let gl = ctx.raw_gl();
        // SAFETY: the surface is current, and both framebuffers are complete
        // and of the extent named here.
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            // A blit is subject to the scissor test, and a clip left enabled by
            // the last draw would present only the part of the frame that draw
            // could touch.
            gl.disable(glow::SCISSOR_TEST);
            // The rows exchanged: the source's zero maps to the destination's
            // last. This one line is the whole of the orientation change, and
            // reversing the source range rather than the destination's is what
            // keeps the horizontal axis alone.
            gl.blit_framebuffer(
                0,
                height,
                width,
                0,
                0,
                0,
                width,
                height,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);

            let error = gl.get_error();
            if error != glow::NO_ERROR {
                return Err(Error::Backend {
                    backend: "gles",
                    detail: format!("presenting produced GL error {error:#x}"),
                });
            }
        }

        let (egl, display, _, _) = ctx.egl();
        egl.swap_buffers(display, self.surface)
            .map_err(|e| Error::Backend {
                backend: "gles",
                detail: format!("swap_buffers: {e:?}"),
            })?;
        self.presented += 1;
        Ok(())
    }

    fn reconfigure(&mut self, ctx: &mut GlesContext, extent: Extent2D) -> Result<()> {
        if extent == self.extent {
            return Ok(());
        }
        // Allocated before the old one is released, so a failure leaves the
        // target usable at the size it already had rather than with nothing.
        let replacement = ctx.create_texture(&TextureDescriptor::offscreen(extent, self.format))?;
        if let Some(old) = self.texture.replace(replacement) {
            ctx.destroy_texture(old);
        }
        self.extent = extent;
        self.acquired = false;
        Ok(())
    }

    fn destroy(mut self, ctx: &mut GlesContext) {
        if let Some(texture) = self.texture.take() {
            ctx.destroy_texture(texture);
        }
        // Unbound so the context is left as it was found: surfaceless, and
        // usable by whatever comes next. The surface itself is the caller's.
        let _ = ctx.make_surface_current(None);
    }
}

//! emblema: a tessellation-based 2D vector renderer.
//!
//! ```no_run
//! use emblema::{BackendPreference, Canvas, Color, Context, Extent2D, Paint, PixelFormat, Rect};
//!
//! // The backend is chosen at run time, so one binary serves a board with a
//! // working Vulkan driver and one where only GLES is usable.
//! let mut ctx = Context::new(BackendPreference::Auto)?;
//!
//! let size = Extent2D::new(256, 256);
//! let mut surface = ctx.create_surface(size, PixelFormat::Rgba8Unorm)?;
//!
//! let mut canvas = Canvas::new(size);
//! canvas.clear(Color::WHITE);
//! canvas.draw_rect(
//!     Rect::new(32.0, 32.0, 224.0, 224.0),
//!     &Paint::fill(Color::rgba8(0, 120, 220, 255)),
//! )?;
//!
//! // Recording is separate from submitting, so a whole frame is described
//! // before any of it reaches the GPU and every shape shares one pass.
//! ctx.draw(&mut surface, &canvas.finish())?;
//! let pixels = ctx.read(&mut surface)?;
//!
//! // Destroyed explicitly, because nothing here frees a GPU object on drop. A
//! // surface still alive when its context goes is a child outliving its device,
//! // which the validation layer reports as an error rather than a leak.
//! ctx.destroy_surface(surface);
//! # Ok::<(), emblema::Error>(())
//! ```
//!
//! Which backend runs is decided when the context is created, not when the
//! binary is built, so one image can boot on a board with a working Vulkan
//! driver and on one where only GLES is usable.
//!
//! Code above the backend branches on [`Context::capabilities`] rather than on
//! which backend it got. A device that cannot export a fence is the same
//! problem whichever API it speaks, and asking which backend it is gets both
//! cases wrong.

pub mod backend;
pub mod context;

pub use backend::{Backend, BackendPreference};
pub use context::{Context, Image, Surface};

// The drawing API, so an application depends on this crate alone.
pub use emblema_core::{
    Affine2, Atlas, AtlasError, AtlasRect, BlendMode, Canvas, Color, ColorFilter, ColorForm,
    ColorSpace, Coverage, Dash, Extent2D, FillRule, Gamma, GlyphKey, GradientStop, ImageFilter,
    Layer, LineCap, LineJoin, MaskBlurStyle, Morphology, Paint, Path, PathBuilder, PixelFormat,
    PointMode, PositionedGlyph, Recording, Rect, RoundingRadii, Sampling, Shader, SourceRect,
    Sprite, StrokeStyle, Style, TileMode, Transform2D, Vec2, VertexMode, Vertices, MAX_STOPS,
};
pub use emblema_hal::{
    mip_levels_for, BlendFactor, Capabilities, Error, Hal, Result, RuntimeProgram,
    TextureDescriptor, TextureUsage, MORPHOLOGY_TAPS, RUNTIME_FLOATS,
};

/// Draw a recording into a texture this crate did not create.
///
/// [`Context::draw`] covers the case where the target came from
/// [`Context::create_surface`], and that is the ordinary one. This is the other:
/// a swapchain hands out its own images, and drawing into one means naming a
/// texture the caller got from somewhere else. Without this the presentation
/// path re-exported above reached a swapchain and then had nothing to draw into
/// its images with, which is a window a caller can open and not fill.
pub use emblema_core::{execute, execute_deferred, execute_layers, render_offscreen};

/// Direct scanout to a display, with no compositor.
#[cfg(feature = "drm")]
pub use emblema_present_drm as drm;

/// The Vulkan backend itself.
///
/// Re-exported because [`Context`] already exposes it: `Context::Vulkan` is a
/// public variant holding a `VulkanContext`, so the type was part of this
/// crate's surface whether or not it could be named. Without this a caller
/// could match the variant and use what came out by inference, but could not
/// write its type down -- so no function of theirs could take one, and reaching
/// the swapchain meant depending on the backend crate separately and matching
/// its version by hand.
#[cfg(feature = "vulkan")]
pub use emblema_hal_vulkan as vulkan;

/// The GLES backend, re-exported for the same reason.
#[cfg(feature = "gles")]
pub use emblema_hal_gles as gles;

/// A Vulkan WSI swapchain, for drawing into a window system's surface.
///
/// This was a feature name that enabled nothing for as long as it existed:
/// `present-wsi` was in the default set and gated no dependency, and the crate
/// implementing it was not one. So the facade's default build advertised a
/// presentation path it had no route to, and a caller wanting a window had to
/// reach past this crate to `emblema-present-vk` -- which the playground does,
/// and which was the visible symptom nobody read as one.
#[cfg(feature = "present-wsi")]
pub use emblema_present_vk as wsi;

/// An EGL window surface, which is the GLES backend's way to the same place.
#[cfg(feature = "present-egl")]
pub use emblema_present_egl as egl;

/// Presentation targets and format negotiation.
pub use emblema_present as present;

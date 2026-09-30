//! The public API: what an application draws with.
//!
//! Everything below this — tessellation, batching, backends, presentation — is
//! reachable but not usually reached for. A caller describes a frame on a
//! [`Canvas`] and hands the resulting [`Recording`] to a target.
//!
//! Recording is deliberately separate from submitting. Describing a whole frame
//! before any of it reaches the GPU is what lets draws share a single pass, and
//! it keeps the API free of the question of when work is flushed.
//!
//! Color is linear throughout, converted at this boundary. Blending,
//! filtering, and antialiasing all average colors, and averaging sRGB-encoded
//! values is visibly wrong — see [`Color`].

pub mod canvas;
pub mod color;
pub mod execute;
pub mod paint;
pub mod ramp;
pub mod vertices;

pub use canvas::{
    Canvas, Layer, Morphology, Pass, PointMode, Recording, Rect, RoundingRadii, TextureSource,
};
pub use color::{Color, ColorSpace};
pub use emblema_text::{Atlas, AtlasError, AtlasRect, Coverage, GlyphKey, PositionedGlyph};
pub use execute::{
    execute, execute_deferred, execute_layers, render_offscreen, render_offscreen_into,
    resolve_sources,
};
pub use paint::{GradientStop, ImageFilter, MaskBlurStyle, Paint, Shader, Style};
pub use vertices::{SourceRect, Sprite, VertexMode, Vertices};

// Geometry a caller builds paths with, re-exported so an application needs one
// dependency rather than three.
pub use emblema_geometry::dash::Dash;
pub use emblema_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
pub use emblema_geometry::transform::Transform2D;
pub use emblema_geometry::{FillRule, Path, PathBuilder};
pub use emblema_hal::{
    BlendMode, ColorFilter, ColorForm, Extent2D, Gamma, PixelFormat, Sampling, TileMode, MAX_STOPS,
};

/// Two-dimensional vector and affine types, from `glam`.
pub use glam::{Affine2, Vec2};

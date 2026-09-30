//! Packing rasterized glyph coverage into a texture, and placing the result.
//!
//! This crate takes coverage bitmaps and answers where each one sits. It does
//! not produce them: turning an outline into coverage needs a font parser, and
//! that is out of scope here -- bring `swash` or `ttf-parser` and hand the
//! coverage over. Shaping and layout are out of scope for the same reason;
//! bring `cosmic-text` or `parley`. The boundary is what lets the atlas be
//! tested with no font anywhere in the tree, against bitmaps whose contents
//! are known exactly.
//!
//! Named here because this header claimed them and they would each change what
//! a caller has to do: subpixel positioning, signed-distance-field coverage
//! above a threshold size, and color glyphs drawn as image quads are **not
//! implemented**. Nor is paging -- a full atlas is repacked and then doubled,
//! which [`atlas`] explains and which was chosen over pages because a page
//! boundary splits a glyph run into more than one draw.

pub mod atlas;

pub use atlas::{Atlas, AtlasError, AtlasRect, Coverage, GlyphKey};

/// One glyph, placed.
///
/// The position is the glyph's top-left corner in user space, and the size is
/// what it covers there. Both come from the caller: shaping decides where
/// glyphs go and this does not shape. Keeping the destination separate from the
/// atlas rectangle is what lets a glyph be drawn at a size other than the one
/// it was rasterized at, which a caller doing so knowingly may want.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub key: GlyphKey,
    pub position: [f32; 2],
    pub size: [f32; 2],
}

impl PositionedGlyph {
    /// A glyph drawn at the size its coverage was rasterized at.
    pub fn new(key: GlyphKey, position: [f32; 2], rect: AtlasRect) -> Self {
        Self {
            key,
            position,
            size: [rect.width as f32, rect.height as f32],
        }
    }
}

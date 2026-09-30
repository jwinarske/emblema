//! A triangle mesh a caller supplies directly.
//!
//! Everything else this canvas draws is a shape it tessellates itself, which
//! means the triangles are always well formed and the question of what a
//! caller might hand over does not arise. Here it does, so the checking
//! happens once at construction rather than at every draw: a mesh that exists
//! is a mesh that can be drawn.

use crate::Color;
use emblema_hal::{Error, Result};
use glam::{Affine2, Vec2};

/// How positions are grouped into triangles.
///
/// The two compact forms exist because a strip or a fan states a triangle in
/// one vertex where a list states it in three, and a caller who has that shape
/// already should not have to expand it. They are expanded here rather than
/// carried to a backend, since the difference is an index buffer and both
/// backends draw indexed triangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VertexMode {
    /// Every three positions are one triangle.
    #[default]
    Triangles,
    /// Each position after the second closes a triangle with the two before
    /// it, alternating winding so the strip is consistently wound.
    TriangleStrip,
    /// Each position after the second closes a triangle with the first and the
    /// one before it.
    TriangleFan,
}

/// A mesh of triangles in user space, with optional texture coordinates.
///
/// Texture coordinates run from zero to one across the image and are read per
/// vertex, which is the point of supplying them: every other way of drawing an
/// image here maps it by position, and a mesh that wanted that mapping would
/// not need coordinates at all.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Vertices {
    positions: Vec<Vec2>,
    texture_coords: Vec<Vec2>,
    colors: Vec<Color>,
    indices: Vec<u32>,
}

impl Vertices {
    /// A mesh from positions alone, filled by the paint.
    ///
    /// The paint is sampled by position, as it is for any shape: a gradient
    /// runs across the mesh the way it would across a path covering the same
    /// area.
    pub fn new(mode: VertexMode, positions: Vec<Vec2>) -> Result<Self> {
        Self::build(mode, positions, Vec::new(), Vec::new(), None)
    }

    /// A mesh whose vertices name where in the paint they read.
    ///
    /// Any shader reads there, which is what `dart:ui` means by a color source
    /// at a mesh's coordinates: an image samples its texture, a gradient
    /// measures its ramp, and a caller's program receives them as `uv` beside
    /// the clip position and chooses. This used to say "requires an image
    /// paint", which was true of neither the last two.
    pub fn textured(
        mode: VertexMode,
        positions: Vec<Vec2>,
        texture_coords: Vec<Vec2>,
    ) -> Result<Self> {
        Self::build(mode, positions, texture_coords, Vec::new(), None)
    }

    /// A mesh whose vertices each carry a color.
    ///
    /// The color multiplies whatever the paint produced, so a white paint
    /// leaves the mesh's own colors, and a gradient paint is shaded by them.
    /// Multiplying is the only combination offered: `dart:ui` takes a blend
    /// mode here, and every other mode either discards one of the two inputs
    /// or is not expressible without a second value per fragment.
    pub fn colored(mode: VertexMode, positions: Vec<Vec2>, colors: Vec<Color>) -> Result<Self> {
        Self::build(mode, positions, Vec::new(), colors, None)
    }

    /// The same, with the triangles named by index rather than by order.
    pub fn indexed(
        mode: VertexMode,
        positions: Vec<Vec2>,
        texture_coords: Vec<Vec2>,
        indices: Vec<u32>,
    ) -> Result<Self> {
        Self::build(mode, positions, texture_coords, Vec::new(), Some(indices))
    }

    /// Everything at once, for a caller who has all of it.
    pub fn full(
        mode: VertexMode,
        positions: Vec<Vec2>,
        texture_coords: Vec<Vec2>,
        colors: Vec<Color>,
        indices: Vec<u32>,
    ) -> Result<Self> {
        Self::build(mode, positions, texture_coords, colors, Some(indices))
    }

    fn build(
        mode: VertexMode,
        positions: Vec<Vec2>,
        texture_coords: Vec<Vec2>,
        colors: Vec<Color>,
        indices: Option<Vec<u32>>,
    ) -> Result<Self> {
        if !texture_coords.is_empty() && texture_coords.len() != positions.len() {
            return Err(Error::Unsupported(
                "a mesh with texture coordinates needs one per position",
            ));
        }
        if !colors.is_empty() && colors.len() != positions.len() {
            return Err(Error::Unsupported(
                "a mesh with colors needs one per position",
            ));
        }
        // Checked here rather than clamped at draw time. An index past the end
        // reads whatever follows the buffer, which on one backend is a
        // validation error and on the other is a triangle drawn from
        // uninitialized floats -- so this is the difference between a refusal
        // and a picture that differs between backends for no visible reason.
        if let Some(indices) = &indices {
            if let Some(bad) = indices.iter().find(|i| **i as usize >= positions.len()) {
                let _ = bad;
                return Err(Error::Unsupported(
                    "a mesh index names a position the mesh does not have",
                ));
            }
        }
        if !positions.iter().all(|p| p.is_finite()) || !texture_coords.iter().all(|c| c.is_finite())
        {
            return Err(Error::Unsupported(
                "a mesh position or texture coordinate is not a finite number",
            ));
        }

        // The order the triangles are actually drawn in, which is where the
        // mode stops mattering: a strip and a fan are two ways of writing an
        // index buffer, and writing it here means nothing below this point has
        // to know which was used.
        let order: Vec<u32> = match indices {
            Some(indices) => indices,
            None => (0..positions.len() as u32).collect(),
        };
        let indices = expand(mode, &order);
        Ok(Self {
            positions,
            texture_coords,
            colors,
            indices,
        })
    }

    pub fn positions(&self) -> &[Vec2] {
        &self.positions
    }

    /// Texture coordinates, or empty where the mesh carries none.
    pub fn texture_coords(&self) -> &[Vec2] {
        &self.texture_coords
    }

    /// Per-vertex colors, or empty where the mesh carries none.
    pub fn colors(&self) -> &[Color] {
        &self.colors
    }

    /// Triangle indices, three per triangle, whatever mode built them.
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// One piece of a sprite sheet, and where it goes.
///
/// The unit a batched sprite draw is built from. `source` is in texels of the
/// sheet, because that is how a sheet's layout is known -- a packer emits
/// pixel rectangles, and normalizing them at every call site is how one of
/// them eventually gets normalized twice. The quad is that rectangle's size in
/// user units before the transform, so a sprite drawn with the identity lands
/// at its own size with its top-left corner at the origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    /// The part of the sheet to draw, in texels.
    pub source: SourceRect,
    /// A color multiplied into this sprite alone.
    ///
    /// White changes nothing, and is what [`Sprite::new`] and [`Sprite::at`]
    /// give. This is what makes one sheet serve a whole palette -- and it is
    /// per sprite rather than per batch, which is the difference between one
    /// draw and one draw per color.
    pub color: Color,
    /// Where it goes, applied to a quad running from the origin to the
    /// source's size.
    ///
    /// A full affine rather than the rotation-scale-translation that
    /// `dart:ui` restricts this to. That restriction buys a smaller per-sprite
    /// payload for a shader that applies the transform itself; these are
    /// applied here, where a general transform costs exactly the same and a
    /// caller who wants to skew a sprite is not told they may not.
    pub transform: Affine2,
}

/// A rectangle in texels, as a sprite's source.
///
/// Deliberately not the canvas `Rect`, which is in user space: a sheet
/// rectangle and a destination rectangle are different things, and the one
/// mistake this call invites is passing one where the other belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SourceRect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn is_finite(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
    }
}

impl Sprite {
    pub fn new(source: SourceRect, transform: Affine2) -> Self {
        Self {
            source,
            color: Color::WHITE,
            transform,
        }
    }

    /// The same sprite, tinted.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// A sprite placed without rotation or scaling.
    pub fn at(source: SourceRect, position: Vec2) -> Self {
        Self::new(source, Affine2::from_translation(position))
    }
}

/// Rewrite an order of vertices as a list of triangles.
///
/// A degenerate tail -- one or two vertices left over from a list, or fewer
/// than three altogether -- produces no triangle rather than an error, which
/// is what a mesh with nothing in it should do. An error would be a different
/// claim: that the caller made a mistake, when a mesh built from a loop that
/// found nothing is not one.
fn expand(mode: VertexMode, order: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    match mode {
        VertexMode::Triangles => {
            for triangle in order.chunks_exact(3) {
                out.extend_from_slice(triangle);
            }
        }
        VertexMode::TriangleStrip => {
            for (i, window) in order.windows(3).enumerate() {
                // Every other triangle has the opposite winding, so two of its
                // vertices swap. Emitting them in strip order instead would
                // alternate front and back faces, which matters the moment
                // anything culls -- and a mesh whose triangles disagree about
                // which side they face is wrong even where nothing does.
                if i % 2 == 0 {
                    out.extend_from_slice(&[window[0], window[1], window[2]]);
                } else {
                    out.extend_from_slice(&[window[1], window[0], window[2]]);
                }
            }
        }
        VertexMode::TriangleFan => {
            for window in order.windows(2).skip(1) {
                out.extend_from_slice(&[order[0], window[0], window[1]]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signed area of a triangle, whose sign is its winding.
    fn winding(a: Vec2, b: Vec2, c: Vec2) -> f32 {
        (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
    }

    #[test]
    fn a_strip_comes_out_consistently_wound() {
        // The claim the alternation exists to make, and the only one that says
        // why it is there: emitted in strip order the triangles would alternate
        // front and back faces, and a mesh whose triangles disagree about which
        // way they face is wrong the moment anything culls -- and wrong in a way
        // that draws correctly until something does.
        //
        // Beside the test that pins the index sequence rather than instead of
        // it. That one builds its strip from `points`, which are collinear, so
        // every triangle in it encloses nothing and has no winding to be
        // consistent about: it fixes the ordering, and this fixes what the
        // ordering is for.
        //
        // A zigzag rather than a straight run, because a degenerate strip has no
        // winding to be consistent about: three collinear points enclose nothing
        // and their signed area is zero, which agrees with everything.
        let strip: Vec<Vec2> = (0..6)
            .map(|i| Vec2::new(i as f32 * 10.0, if i % 2 == 0 { 0.0 } else { 12.0 }))
            .collect();
        let indices = expand(VertexMode::TriangleStrip, &[0, 1, 2, 3, 4, 5]);
        assert_eq!(indices.len(), 4 * 3, "six points make four triangles");

        let signs: Vec<f32> = indices
            .chunks_exact(3)
            .map(|t| {
                winding(
                    strip[t[0] as usize],
                    strip[t[1] as usize],
                    strip[t[2] as usize],
                )
                .signum()
            })
            .collect();
        assert!(
            signs.iter().all(|s| *s == signs[0]),
            "every triangle in a strip should wind the same way, got {signs:?}"
        );

        // And the alternation is a reordering rather than a different set: each
        // triangle still covers the three points the strip says it does.
        for (i, t) in indices.chunks_exact(3).enumerate() {
            let mut got = [t[0], t[1], t[2]];
            got.sort_unstable();
            let want = [i as u32, i as u32 + 1, i as u32 + 2];
            assert_eq!(got, want, "triangle {i} covers the wrong points");
        }
    }

    fn points(n: usize) -> Vec<Vec2> {
        (0..n).map(|i| Vec2::new(i as f32, 0.0)).collect()
    }

    #[test]
    fn a_strip_alternates_winding_so_every_triangle_faces_the_same_way() {
        let mesh = Vertices::new(VertexMode::TriangleStrip, points(4)).expect("strip");
        assert_eq!(mesh.indices(), &[0, 1, 2, 2, 1, 3]);
    }

    #[test]
    fn a_fan_shares_its_first_vertex_with_every_triangle() {
        let mesh = Vertices::new(VertexMode::TriangleFan, points(4)).expect("fan");
        assert_eq!(mesh.indices(), &[0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn a_list_drops_a_partial_triangle_rather_than_inventing_a_vertex() {
        let mesh = Vertices::new(VertexMode::Triangles, points(5)).expect("list");
        assert_eq!(mesh.indices(), &[0, 1, 2]);
    }

    #[test]
    fn too_few_positions_for_any_triangle_is_an_empty_mesh_rather_than_an_error() {
        for mode in [
            VertexMode::Triangles,
            VertexMode::TriangleStrip,
            VertexMode::TriangleFan,
        ] {
            let mesh = Vertices::new(mode, points(2)).expect("two points");
            assert!(mesh.is_empty(), "{mode:?} made a triangle from two points");
        }
    }

    #[test]
    fn an_index_past_the_end_is_refused_where_it_is_written() {
        let error = Vertices::indexed(VertexMode::Triangles, points(3), Vec::new(), vec![0, 1, 3]);
        assert!(error.is_err(), "an out-of-range index was accepted");
    }

    #[test]
    fn texture_coordinates_have_to_match_the_positions_they_belong_to() {
        let error = Vertices::textured(VertexMode::Triangles, points(3), points(2));
        assert!(error.is_err(), "a short coordinate list was accepted");
    }
}

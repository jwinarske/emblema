//! Draws accumulated for one submission.
//!
//! A batch is a declarative description of a scene: shared geometry plus a
//! list of draws over it. Backends are handed the whole thing rather than a
//! stream of recording calls, which lets each decide how to realize it — a
//! Vulkan backend binds pipelines only where they change, and a record-and-
//! replay backend can inspect the whole batch before touching any state.

use crate::material::ColorFilter;
use crate::{BlendMode, Error, Material, Result, Scissor};

/// What a draw does with the stencil buffer.
///
/// # Why the stencil holds a depth rather than a mask
///
/// The obvious encoding gives each clip a bit, which caps nesting at eight and
/// makes intersecting two clips a per-bit affair. Storing the *nesting depth*
/// instead lets a clip stack of any size fit in the same eight bits, and makes
/// the test a single comparison: content belongs to depth `d` and draws where
/// the stencil holds `d`, which is true only where every clip down to that
/// depth admitted the pixel.
///
/// It also makes undoing a clip a local operation. Because a stack unwinds in
/// the order it was built, no pixel can hold more than the depth being left, so
/// stepping back is a decrement rather than a recomputation from the remaining
/// clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ClipRole {
    /// Draw color where the stencil already matches. Leaves the stencil alone.
    #[default]
    Content,
    /// Narrow the clip: step the stencil forward where it matches and this draw
    /// covers. Writes no color.
    ///
    /// The geometry must be a triangulation of the clip region rather than an
    /// overlapping set, since a pixel covered twice would step forward twice
    /// and stop matching anything. The fill tessellator produces exactly that,
    /// which is what lets this be a plain increment instead of the parity trick
    /// an overlapping fan would need.
    Narrow,
    /// Widen the clip back: step the stencil back where it matches. Writes no
    /// color.
    Widen,
}

impl ClipRole {
    /// Whether this role writes to the color attachment.
    pub const fn writes_color(self) -> bool {
        matches!(self, Self::Content)
    }

    /// Whether this role modifies the stencil.
    pub const fn writes_stencil(self) -> bool {
        !matches!(self, Self::Content)
    }
}

/// The stencil state one draw needs.
///
/// `reference` is what the stencil is compared against, stated directly rather
/// than derived from a nesting depth, so the HAL needs no notion of a clip
/// stack: a narrowing draw compares against the depth it is leaving and a
/// widening draw against the one it is leaving behind, and which is which is
/// the recorder's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ClipState {
    pub reference: u32,
    pub role: ClipRole,
}

impl ClipState {
    /// Content outside any clip, which needs no stencil at all.
    pub const UNCLIPPED: Self = Self {
        reference: 0,
        role: ClipRole::Content,
    };

    pub const fn content(reference: u32) -> Self {
        Self {
            reference,
            role: ClipRole::Content,
        }
    }

    pub const fn narrow(from: u32) -> Self {
        Self {
            reference: from,
            role: ClipRole::Narrow,
        }
    }

    pub const fn widen(from: u32) -> Self {
        Self {
            reference: from,
            role: ClipRole::Widen,
        }
    }

    /// Whether this needs a stencil attachment to mean anything.
    pub const fn needs_stencil(self) -> bool {
        self.reference != 0 || self.role.writes_stencil()
    }
}

/// One vertex: where it is, and where it reads from.
///
/// # Why every vertex carries texture coordinates
///
/// Most geometry here does not need them — a solid fill and a gradient both
/// locate themselves from the interpolated clip position. A glyph run does: a
/// run is many quads reading different parts of one atlas, and a material is
/// per draw, so coordinates carried in the paint would mean a draw per glyph.
/// Text is the highest draw-count content there is, so that is the wrong place
/// to spend.
///
/// The cost is eight bytes on every vertex, including the ones that ignore
/// them. The alternative — a second vertex format and a second pipeline for
/// text — spends more in pipeline state and in the code that has to decide
/// which of two shapes a batch is in, to save memory on the geometry that is
/// already the cheapest to store.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct Vertex {
    /// Homogeneous clip position: the point as the recorder produced it,
    /// *before* the rasterizer divides.
    ///
    /// `w` is one for everything an affine transform placed, which is nearly
    /// everything, and the third float is what lets a transform with
    /// perspective say anything at all — there is no two-component form of a
    /// point that has been divided by a quantity varying across the triangle.
    ///
    /// Carrying it undivided rather than dividing on the way here buys two
    /// things beyond the mapping itself. The rasterizer clips against the plane
    /// where `w` reaches zero, so geometry crossing the vanishing line is cut
    /// there by the hardware instead of arriving as coordinates on both sides
    /// of infinity. And every varying beside this one — texture coordinates
    /// most of all — is then interpolated perspective-correctly, which is the
    /// difference between a textured quad seen at an angle and the diagonal
    /// seam that affine interpolation puts across it.
    pub position: [f32; 3],
    /// Where in a sampled texture this vertex reads, if the material samples
    /// one. Zero where it does not, which costs nothing to interpolate.
    pub uv: [f32; 2],
    /// A color multiplied into whatever the material produced, **premultiplied**.
    ///
    /// Opaque white for everything but a mesh a caller colored, and white is
    /// the identity, so a fill pays for this in bandwidth rather than in a
    /// second path. Sixteen bytes per vertex: at fifty thousand vertices a
    /// frame, which is a great deal of two-dimensional geometry, that is under
    /// fifty megabytes a second against a tiler already spending ten times
    /// that on the framebuffer alone. A second vertex layout and a second
    /// pipeline would save it and cost a permanent split in the batch model,
    /// which is the wrong trade at this magnitude.
    ///
    /// Premultiplied rather than straight because it is interpolated across a
    /// triangle, and interpolating straight color between vertices whose alpha
    /// differs gives a color no point on the edge actually has.
    pub color: [f32; 4],
}

impl Vertex {
    pub const fn new(position: [f32; 2], uv: [f32; 2]) -> Self {
        Self::projected([position[0], position[1], 1.0], uv)
    }

    /// A vertex that samples nothing.
    pub const fn at(position: [f32; 2]) -> Self {
        Self::new(position, [0.0, 0.0])
    }

    /// A vertex whose position is already homogeneous.
    ///
    /// The form a transform carrying perspective produces. [`Self::new`] is
    /// this with a `w` of one, which is what an affine always gives, and is why
    /// the ordinary constructors did not have to change when the third float
    /// arrived.
    pub const fn projected(position: [f32; 3], uv: [f32; 2]) -> Self {
        Self {
            position,
            uv,
            color: WHITE,
        }
    }

    /// A homogeneous vertex that samples nothing.
    pub const fn at_projected(position: [f32; 3]) -> Self {
        Self::projected(position, [0.0, 0.0])
    }

    /// The same vertex, tinted.
    ///
    /// `color` is premultiplied; see [`Self::color`].
    pub const fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }
}

/// The color that changes nothing when multiplied in.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// One draw within a batch.
#[derive(Debug, Clone)]
pub struct BatchDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: Material,
    /// A function applied to the material's color before the blend.
    ///
    /// Beside the material rather than inside it, for the same reason the
    /// blend mode is: it applies to every kind of material equally and belongs
    /// to none of them. It is packed into the same uniform the material is,
    /// because the shader reads one block per draw.
    pub filter: ColorFilter,
    pub blend: BlendMode,
    /// The region of the target this draw may write to.
    ///
    /// `None` is the whole target. It is distinct from a rectangle that happens
    /// to cover the target so a backend can tell "this draw was never clipped"
    /// from "this draw's clip works out to everything", and skip the state
    /// change in the first case without having to know the target's size.
    ///
    /// Independent of [`Self::stencil`], and both apply. An axis-aligned clip
    /// stays here even where a stencil is already in play, because a scissor is
    /// exact and costs nothing while a stencil pass costs a draw.
    pub clip: Option<Scissor>,
    /// What this draw does with the stencil buffer.
    pub stencil: ClipState,
    /// How a color the caller attached to a vertex or a sprite combines with
    /// what the material produced.
    ///
    /// [`BlendMode::Modulate`] multiplies them, which is what every draw did
    /// before this existed and is what a paint with no per-vertex color wants:
    /// white is the identity under it. Distinct from [`Self::blend`], which is
    /// how the result then reaches the target -- these two colors are both in
    /// the shader, so this one needs no extension and every mode is available.
    pub tint_blend: BlendMode,
    /// Read the paint at this draw's texture coordinates rather than at the
    /// position of the fragment.
    ///
    /// A property of the geometry rather than of the material, which is why it
    /// is here: a mesh that states a coordinate per vertex has said where each
    /// one sits in the paint's space, and there is nothing left to derive. An
    /// image already worked this way and had its own material for it; this is
    /// what lets a gradient or a caller's program do the same.
    ///
    /// False everywhere else, and it costs those draws nothing: the flag lands
    /// in a slot no material that could set it uses, and the shader's select
    /// is one instruction on a value it has already computed.
    pub paint_at_texture_coords: bool,
}

impl BatchDraw {
    /// The uniform block this draw's shader reads.
    ///
    /// The material and the filter are packed together because the shader
    /// takes one block per draw, and separately here because they are separate
    /// things: a filter applies to any material, and a material knows nothing
    /// about being filtered.
    /// `target` is the format this draw is about to be written into, which
    /// only the backend knows: a recording is built without one, and the same
    /// recording is drawn into an eight-bit surface and a float one. It decides
    /// the dither, and nothing else here.
    pub fn to_uniform(&self, target: crate::PixelFormat) -> [f32; crate::MATERIAL_FLOATS] {
        let mut out = self.material.to_uniform();
        self.filter.pack_into(&mut out);
        out[crate::material::layout::FILTER_PARAMS + 1] = self.tint_blend.code();
        if self.paint_at_texture_coords {
            // `geometry.x`, which no gradient writes. See the field's own note
            // and the `paint_space` comment in the shader.
            out[crate::material::layout::GEOMETRY] = 1.0;
        }
        let dither = crate::material::layout::DITHER;
        // Upstream's rate exactly: `kDitherRate` is 1/64 and is added to the
        // premultiplied color whatever the target is. That is a single constant
        // there because its values are encoded, so a quantization step is a
        // flat 1/255 wherever it stands -- and now for the same reason it is a
        // single constant here.
        //
        // Still zero for a target with no quantum to bridge. Half's precision
        // is relative, so there is no step to straddle and upstream's constant
        // would be noise added to a surface that had none.
        out[dither] = if target.quantization_step() > 0.0 {
            1.0 / 64.0
        } else {
            0.0
        };
        out
    }
}

/// Geometry and paint for a sequence of draws sharing one target.
///
/// Draws are kept in submission order rather than sorted by pipeline. Sorting
/// would cut pipeline binds, but 2D drawing is painter's-algorithm ordered:
/// reordering two overlapping draws changes which one ends up on top. Deciding
/// when a reorder is safe needs either overlap analysis or a depth buffer, and
/// that belongs to the layer that knows what the draws represent.
#[derive(Debug, Default, Clone)]
pub struct Batch {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    draws: Vec<BatchDraw>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a draw covering the whole target.
    ///
    /// Indices are relative to `vertices` and are rebased onto the batch's
    /// shared buffer, so a caller need not know what came before it.
    pub fn push(
        &mut self,
        vertices: &[[f32; 2]],
        indices: &[u32],
        material: Material,
        blend: BlendMode,
    ) -> Result<()> {
        self.push_clipped(vertices, indices, material, blend, None)
    }

    /// Append a draw confined to a region of the target.
    ///
    /// A separate entry point rather than an extra parameter on [`Self::push`]:
    /// most draws are unclipped, and threading `None` through every call site
    /// makes the ones that do carry a clip harder to pick out, not easier.
    ///
    /// An empty scissor drops the draw. Recording something that provably
    /// writes no pixel would cost a pipeline bind and a draw call to produce
    /// the same target, and a clip stack that has narrowed to nothing is a
    /// normal state for a scrolled-away subtree rather than an error.
    pub fn push_clipped(
        &mut self,
        vertices: &[[f32; 2]],
        indices: &[u32],
        material: Material,
        blend: BlendMode,
        clip: Option<Scissor>,
    ) -> Result<()> {
        self.push_with(
            vertices,
            indices,
            material,
            ColorFilter::None,
            blend,
            clip,
            ClipState::UNCLIPPED,
        )
    }

    /// Append a draw with an explicit stencil role.
    ///
    /// The general form the other two delegate to. A caller reaches for this
    /// only when building or unwinding a clip, or when drawing content inside
    /// one; everything else is confined by a scissor or not confined at all.
    #[allow(clippy::too_many_arguments)]
    pub fn push_with(
        &mut self,
        positions: &[[f32; 2]],
        indices: &[u32],
        material: Material,
        filter: ColorFilter,
        blend: BlendMode,
        clip: Option<Scissor>,
        stencil: ClipState,
    ) -> Result<()> {
        // Tessellated geometry has no texture coordinates of its own, and the
        // materials it carries do not read them.
        let vertices: Vec<Vertex> = positions.iter().copied().map(Vertex::at).collect();
        self.push_mesh(&vertices, indices, material, filter, blend, clip, stencil)
    }

    /// Append a draw whose vertices carry texture coordinates.
    ///
    /// The form a glyph run takes: one draw over many quads, each reading a
    /// different part of the same atlas.
    #[allow(clippy::too_many_arguments)]
    pub fn push_mesh(
        &mut self,
        vertices: &[Vertex],
        indices: &[u32],
        material: Material,
        filter: ColorFilter,
        blend: BlendMode,
        clip: Option<Scissor>,
        stencil: ClipState,
    ) -> Result<()> {
        self.push_mesh_tinted(
            vertices,
            indices,
            material,
            filter,
            blend,
            clip,
            stencil,
            BlendMode::Modulate,
            false,
        )
    }

    /// Append a mesh, saying how its vertex colors combine with the material.
    ///
    /// Separate from [`Self::push_mesh`] rather than an extra parameter on it,
    /// for the reason [`Self::push_clipped`] is separate: the mode is
    /// `Modulate` for everything that does not ask, white being the identity
    /// under it, and threading a parameter through every call site to say so
    /// would be noise at all of them and a decision at none.
    #[allow(clippy::too_many_arguments)]
    pub fn push_mesh_tinted(
        &mut self,
        vertices: &[Vertex],
        indices: &[u32],
        material: Material,
        filter: ColorFilter,
        blend: BlendMode,
        clip: Option<Scissor>,
        stencil: ClipState,
        tint_blend: BlendMode,
        paint_at_texture_coords: bool,
    ) -> Result<()> {
        if clip.is_some_and(Scissor::is_empty) {
            return Ok(());
        }
        if indices.len() % 3 != 0 {
            return Err(Error::Unsupported("index count is not a whole triangle"));
        }
        if let Some(&max) = indices.iter().max() {
            if max as usize >= vertices.len() {
                return Err(Error::Backend {
                    backend: "vulkan",
                    detail: format!(
                        "index {max} addresses past the {} vertices supplied",
                        vertices.len()
                    ),
                });
            }
        }
        if indices.is_empty() {
            return Ok(());
        }

        let base = u32::try_from(self.vertices.len()).map_err(|_| Error::LimitExceeded {
            what: "batch vertex count",
            requested: self.vertices.len() as u64,
            limit: u32::MAX as u64,
        })?;
        let first_index = self.indices.len() as u32;

        self.vertices.extend_from_slice(vertices);
        self.indices.extend(indices.iter().map(|i| i + base));

        // A draw that differs from the one before it in nothing a backend can
        // set is not a second draw. Its indices were just appended to the same
        // buffer, so extending the previous range covers both, and the
        // triangles are rasterized in the same order either way -- which is
        // what makes this safe under painter's-algorithm ordering, where two
        // overlapping shapes must not trade places.
        //
        // Adjacent only, never sorted. Reordering to create more of these is a
        // different decision with a different safety argument, and this one
        // needs none: the sequence is untouched.
        if let Some(last) = self.draws.last_mut() {
            if last.first_index + last.index_count == first_index
                && last.material == material
                && last.filter == filter
                && last.blend == blend
                && last.clip == clip
                && last.stencil == stencil
                && last.tint_blend == tint_blend
                && last.paint_at_texture_coords == paint_at_texture_coords
            {
                last.index_count += indices.len() as u32;
                return Ok(());
            }
        }

        self.draws.push(BatchDraw {
            filter,
            first_index,
            index_count: indices.len() as u32,
            material,
            blend,
            clip,
            stencil,
            tint_blend,
            paint_at_texture_coords,
        });
        Ok(())
    }

    /// Drop the contents but keep the allocations, for reuse next frame.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.draws.clear();
    }

    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Whether recording this needs a stencil attachment.
    ///
    /// Derived from the draws rather than declared alongside them, so a batch
    /// cannot ask for a clip and forget to say it needs somewhere to put it.
    /// Most batches clip nothing, and those pay for no attachment.
    pub fn uses_stencil(&self) -> bool {
        self.draws.iter().any(|draw| draw.stencil.needs_stencil())
    }

    /// The deepest clip stack an eight-bit stencil can distinguish.
    ///
    /// Eight bits is the only stencil depth every device is required to offer,
    /// on either graphics API, so this is the portable limit rather than any
    /// one device's.
    pub const MAX_CLIP_DEPTH: u32 = 255;

    /// Refuse a batch whose clip stack is deeper than a stencil can hold.
    ///
    /// Here rather than in each backend because the limit is a property of the
    /// stencil format both are required to offer, and the failure it prevents
    /// is one neither can detect afterwards: past the limit the value wraps or
    /// saturates, and either way a later test for a depth that no longer fits
    /// admits every pixel the clip was meant to exclude. Nothing about that
    /// looks like an error -- it draws content the caller clipped away.
    ///
    /// It was in one backend and not the other, so the same recording was
    /// refused on Vulkan and silently rendered wrong on GLES.
    pub fn check_clip_depth(&self) -> Result<()> {
        let depth = self.max_clip_depth();
        if depth > Self::MAX_CLIP_DEPTH {
            return Err(Error::LimitExceeded {
                what: "clip nesting depth",
                requested: depth as u64,
                limit: Self::MAX_CLIP_DEPTH as u64,
            });
        }
        Ok(())
    }

    /// The largest stencil value this batch can produce.
    pub fn max_clip_depth(&self) -> u32 {
        self.draws
            .iter()
            .map(|draw| match draw.stencil.role {
                ClipRole::Narrow => draw.stencil.reference + 1,
                _ => draw.stencil.reference,
            })
            .max()
            .unwrap_or(0)
    }

    /// The texture slots this batch samples, in ascending order without
    /// repeats.
    ///
    /// A backend uses this to size its bindings before recording, and to check
    /// the table it was given covers what the draws ask for.
    pub fn texture_slots(&self) -> Vec<u32> {
        let mut slots: Vec<u32> = self
            .draws
            .iter()
            .flat_map(|draw| draw.material.texture_slots())
            .flatten()
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }

    /// How many times a pipeline will be bound when this batch is recorded.
    ///
    /// Consecutive draws sharing a blend mode reuse the bound pipeline, so this
    /// counts transitions rather than draws.
    pub fn pipeline_binds(&self) -> usize {
        let mut binds = 0;
        let mut current: Option<BlendMode> = None;
        for draw in &self.draws {
            if current != Some(draw.blend) {
                binds += 1;
                current = Some(draw.blend);
            }
        }
        binds
    }
}

impl Batch {
    /// Shared vertex buffer, positions in clip space.
    /// Move every scissor into a target whose origin moved by `(dx, dy)`.
    ///
    /// For a layer whose target was narrowed after its draws were recorded.
    /// The geometry is left alone -- it is in clip space and the pass's
    /// viewport is what places it -- but a scissor is in target pixels, so it
    /// is the one recorded thing the move does reach.
    pub fn rebase_scissors(&mut self, dx: u32, dy: u32, extent: crate::Extent2D) {
        for draw in &mut self.draws {
            if let Some(clip) = draw.clip {
                draw.clip = Some(clip.shifted(dx, dy, extent));
            }
        }
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// Shared index buffer, already rebased onto [`Batch::vertices`].
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// The draws, in submission order.
    pub fn draws(&self) -> &[BatchDraw] {
        &self.draws
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRI: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];

    #[test]
    fn indices_are_rebased_onto_the_shared_buffer() {
        let mut batch = Batch::new();
        // Two colors, so the draws do not merge and the second one's own
        // range is visible. What is being checked is the rebasing, which a
        // merged pair would hide behind a single range covering both.
        batch
            .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        batch
            .push(&TRI, &[0, 1, 2], Material::solid([0.5; 4]), BlendMode::Src)
            .unwrap();

        // The second draw's indices must point at its own vertices, not the
        // first draw's, or both draws render the same triangle.
        assert_eq!(batch.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(batch.vertices.len(), 6);
        assert_eq!(batch.draws[1].first_index, 3);
        assert_eq!(batch.draw_count(), 2);
    }

    #[test]
    fn a_draw_that_differs_from_the_one_before_it_in_nothing_is_not_a_second_draw() {
        let mut batch = Batch::new();
        for _ in 0..4 {
            batch
                .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
                .unwrap();
        }
        assert_eq!(batch.draw_count(), 1, "four alike draws should be one");
        // All four triangles are still there, and still in order: merging
        // changes how many times a backend is asked to draw, not what it
        // draws.
        assert_eq!(batch.indices, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(batch.draws[0].index_count, 12);

        // A change in any one thing a backend sets ends the run.
        batch
            .push(
                &TRI,
                &[0, 1, 2],
                Material::solid([1.0; 4]),
                BlendMode::SrcOver,
            )
            .unwrap();
        assert_eq!(batch.draw_count(), 2);
    }

    #[test]
    fn pipeline_binds_count_transitions_not_draws() {
        let mut batch = Batch::new();
        // A different color each time, so no two draws merge and the count
        // this is about -- pipeline binds against draws -- stays a real
        // distinction rather than one merging has already collapsed.
        for (i, blend) in [
            BlendMode::Src,
            BlendMode::Src,
            BlendMode::SrcOver,
            BlendMode::SrcOver,
            BlendMode::Src,
        ]
        .into_iter()
        .enumerate()
        {
            let shade = i as f32 / 8.0;
            batch
                .push(&TRI, &[0, 1, 2], Material::solid([shade; 4]), blend)
                .unwrap();
        }
        // Five draws, three runs of like pipelines.
        assert_eq!(batch.draw_count(), 5);
        assert_eq!(batch.pipeline_binds(), 3);
    }

    #[test]
    fn an_empty_draw_adds_nothing() {
        let mut batch = Batch::new();
        batch
            .push(&[], &[], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        assert!(batch.is_empty());
        assert_eq!(batch.draw_count(), 0);
    }

    #[test]
    fn malformed_geometry_is_refused_where_it_is_pushed() {
        let mut batch = Batch::new();
        // Catching this at push means the caller learns which draw was wrong,
        // rather than a whole batch failing later at submission.
        assert!(batch
            .push(
                &[[0.0, 0.0]],
                &[0, 1, 2],
                Material::solid([1.0; 4]),
                BlendMode::Src
            )
            .is_err());
        assert!(batch
            .push(
                &[[0.0, 0.0]],
                &[0, 0],
                Material::solid([1.0; 4]),
                BlendMode::Src
            )
            .is_err());
        assert!(batch.is_empty(), "a refused draw must leave no residue");
    }

    #[test]
    fn clearing_keeps_the_batch_reusable() {
        let mut batch = Batch::new();
        batch
            .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        batch.clear();
        assert!(batch.is_empty());

        batch
            .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        // Rebasing must start from zero again rather than continuing from the
        // cleared contents.
        assert_eq!(batch.indices, vec![0, 1, 2]);
    }

    /// Both backends describe this struct to their own API by asking it where
    /// its fields are, so what they agree on is whatever this says. Pinning it
    /// means a reordering shows up here, once, rather than as geometry that
    /// reads its color out of its position on both backends identically.
    #[test]
    fn the_vertex_layout_is_what_both_backends_describe() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<Vertex>(), 36);
        assert_eq!(offset_of!(Vertex, position), 0);
        assert_eq!(offset_of!(Vertex, uv), 12);
        assert_eq!(offset_of!(Vertex, color), 20);
    }

    /// The property that let the third float arrive without touching a caller.
    #[test]
    fn an_ordinary_vertex_carries_a_w_of_one() {
        assert_eq!(Vertex::at([3.0, 4.0]).position, [3.0, 4.0, 1.0]);
        assert_eq!(
            Vertex::new([3.0, 4.0], [0.5, 0.5]).position,
            [3.0, 4.0, 1.0]
        );
        assert_eq!(
            Vertex::at_projected([3.0, 4.0, 2.0]).position,
            [3.0, 4.0, 2.0]
        );
    }
}

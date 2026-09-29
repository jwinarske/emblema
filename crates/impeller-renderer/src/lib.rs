//! Turning paths into geometry a backend can draw.
//!
//! This is where the two halves meet: tessellation produces triangles in path
//! space, and a backend wants them in clip space. Sitting between them means
//! this layer owns the coordinate mapping and the tolerance scaling that
//! depends on it.

use glam::{Affine2, Vec2, Vec3};
use impeller_geometry::dash::{dash_path, Dash};
use impeller_geometry::stroke::StrokeStyle;
use impeller_geometry::tessellate::{Tessellator, VertexBuffers};
use impeller_geometry::transform::{invert_to_local, viewport_projection, Transform2D};
use impeller_geometry::{flatten::DEFAULT_TOLERANCE, Path};
use impeller_hal::{
    Batch, BlendMode, ClipState, ColorFilter, Extent2D, Material, Result, Scissor, Stop, Vertex,
};

/// How a shape is painted.
///
/// Grouped rather than passed as loose parameters because color and blend mode
/// travel together everywhere and will grow into the material set: gradients,
/// image shaders, and filters all attach here.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    /// What fills the shape, already resolved into clip space.
    pub material: Material,
    /// A function applied to the material's color before the blend.
    pub filter: ColorFilter,
    pub blend: BlendMode,
    /// The region of the target this may write to, in device pixels.
    ///
    /// Per-draw state exactly like the blend mode, and carried here for the
    /// same reason: the clip in force when a shape is recorded is a property of
    /// that shape's draw, not of the batch. `None` is the whole target.
    pub clip: Option<Scissor>,
    /// What this draw does with the stencil.
    ///
    /// Independent of [`Self::clip`], and both apply. An axis-aligned clip
    /// stays a scissor even where a stencil is in play, since a scissor is
    /// exact and costs nothing while narrowing the stencil costs a draw.
    pub stencil: ClipState,
}

impl Paint {
    pub fn solid(color: [f32; 4]) -> Self {
        Self {
            material: Material::solid(color),
            filter: ColorFilter::None,
            blend: BlendMode::default(),
            clip: None,
            stencil: ClipState::UNCLIPPED,
        }
    }

    /// A linear gradient between two points **in user space**.
    ///
    /// The endpoints are transformed alongside the geometry, so a gradient
    /// rotates and scales with the shape it fills rather than staying pinned to
    /// the screen.
    pub fn linear_gradient(
        start: Vec2,
        end: Vec2,
        stops: Vec<Stop>,
        transform: Affine2,
        target: Extent2D,
    ) -> Self {
        let to_clip = viewport_projection(target.width, target.height) * transform;
        let axis = end - start;
        Self {
            material: Material::LinearGradient {
                axis: [axis.x, axis.y],
                // The axis stays in the space it was given in, and the shader is
                // told how to get back there. Differencing the two endpoints in
                // clip space instead would let the target's aspect ratio into
                // the gradient's direction.
                //
                // The start is inside that mapping rather than beside it, so
                // what the shader measures along the axis is already an offset
                // from it.
                to_local: invert_to_local(Transform2D::from(
                    to_clip * Affine2::from_translation(start),
                )),
                stops,
                tile: Default::default(),
                ramp: None,
            },
            filter: ColorFilter::None,
            blend: BlendMode::default(),
            clip: None,
            stencil: ClipState::UNCLIPPED,
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    pub fn with_clip(mut self, clip: Option<Scissor>) -> Self {
        self.clip = clip;
        self
    }

    pub fn with_stencil(mut self, stencil: ClipState) -> Self {
        self.stencil = stencil;
        self
    }
}

/// Tessellates paths and places the result in clip space.
///
/// Holds its scratch buffers across calls, so a steady frame loop stops
/// allocating once they reach working size.
///
/// Target size and tolerance are frame state rather than per-call arguments:
/// they are fixed for every shape in a frame, and threading them through each
/// call invited the mistake of passing different values within one frame.
pub struct Renderer {
    tessellator: Tessellator,
    clip_space: Vec<Vec3>,
    target: Extent2D,
    /// Where the target sits within the frame, in device pixels.
    ///
    /// Zero for the frame itself and non-zero for a layer given bounds, which
    /// renders into a target covering only part of it. Geometry arrives in the
    /// frame's device space either way, so this is what places it: without it a
    /// bounded layer's contents are mapped as though the layer filled the
    /// frame, and land wherever the smaller viewport happens to put them.
    origin: Vec2,
    tolerance: f32,
}

impl Default for Renderer {
    fn default() -> Self {
        Self {
            tessellator: Tessellator::default(),
            clip_space: Vec::new(),
            target: Extent2D::new(1, 1),
            origin: Vec2::ZERO,
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the target size and flattening tolerance for the frame.
    ///
    /// Must be called before drawing; the default target is a single pixel, so
    /// forgetting produces geometry collapsed to nothing rather than something
    /// subtly misplaced.
    pub fn begin_frame(&mut self, target: Extent2D, tolerance: f32) -> &mut Self {
        self.target = target;
        self.origin = Vec2::ZERO;
        self.tolerance = tolerance;
        self
    }

    /// Aim subsequent drawing at a target placed within the frame.
    ///
    /// For a layer that was given bounds: its target is smaller than the frame
    /// and offset within it, while the geometry handed over is still stated in
    /// the frame's device pixels, because that is the space the caller's
    /// transform produces. Tolerance is left alone deliberately — it is a
    /// device-space quantity and the device has not changed size.
    pub fn set_viewport(&mut self, origin: Vec2, target: Extent2D) -> &mut Self {
        self.target = target;
        self.origin = origin;
        self
    }

    pub fn target(&self) -> Extent2D {
        self.target
    }

    /// Device pixels to clip space for the target currently aimed at.
    ///
    /// Affine, and stays affine however the caller's transform is stated: a
    /// viewport is a scale and a flip. That is what keeps it out of the
    /// tolerance argument below -- it contributes no stretch of its own.
    pub fn projection(&self) -> Affine2 {
        viewport_projection(self.target.width, self.target.height)
            * Affine2::from_translation(-self.origin)
    }

    /// Tessellate a filled path and map it into clip space.
    ///
    /// `transform` maps path space to **device pixels** — it does not include
    /// the projection onto clip space, which is derived from `target`.
    /// Separating them is what keeps tolerance correct: tolerance is a
    /// device-space quantity, so the scale that matters for flattening is the
    /// path-to-pixel scale alone. Folding the projection in would divide by
    /// the target size and flatten far too coarsely.
    pub fn fill_path(
        &mut self,
        path: &Path,
        transform: impl Into<Transform2D>,
    ) -> ClipGeometry<'_> {
        let transform = transform.into();
        let path_tolerance = path_space_tolerance(self.tolerance, transform, path);
        let projection = self.projection();
        let buffers = self.tessellator.fill(path, path_tolerance);
        Self::to_clip_space(&mut self.clip_space, buffers, transform, projection)
    }

    /// Tessellate a stroked path and map it into clip space.
    ///
    /// The stroke width is in path space, so it is scaled by `transform` along
    /// with everything else. A caller wanting a width that is constant in
    /// device pixels divides it out first.
    pub fn stroke_path(
        &mut self,
        path: &Path,
        style: &StrokeStyle,
        dash: Option<&Dash>,
        transform: impl Into<Transform2D>,
    ) -> ClipGeometry<'_> {
        let transform = transform.into();
        let path_tolerance = path_space_tolerance(self.tolerance, transform, path);
        // Cut before stroking, so each dash is stroked as its own subpath and
        // gets its own caps. Here rather than in the caller because the
        // tolerance a dash is measured against is the one this line computes:
        // dashing a curve means walking its flattening, and walking a different
        // flattening from the one the stroke uses would put the dashes
        // fractionally off the line they lie on.
        let cut;
        let path = match dash {
            Some(dash) if dash.is_usable() => {
                cut = dash_path(path, dash, path_tolerance);
                &cut
            }
            _ => path,
        };
        let projection = self.projection();
        let buffers = self.tessellator.stroke(path, style, path_tolerance);
        Self::to_clip_space(&mut self.clip_space, buffers, transform, projection)
    }

    /// Tessellate a filled path and append it to a batch.
    ///
    /// The batch is what a backend is handed, so this is the path a real frame
    /// takes: many shapes accumulated, one submission. The borrowing form above
    /// exists for callers inspecting geometry without drawing it.
    pub fn fill_into(
        &mut self,
        batch: &mut Batch,
        path: &Path,
        transform: impl Into<Transform2D>,
        paint: &Paint,
    ) -> Result<()> {
        let geo = self.fill_path(path, transform);
        let vertices = geo.vertices();
        let indices = geo.indices.to_vec();
        batch.push_mesh(
            &vertices,
            &indices,
            paint.material.clone(),
            paint.filter,
            paint.blend,
            paint.clip,
            paint.stencil,
        )
    }

    /// Tessellate a filled path, carrying the paint's own space on the vertices
    /// rather than in the material.
    ///
    /// For a material that evaluates a field, the mapping from clip space into
    /// the shape's space is the only thing that differs between two of the same
    /// shape in different places -- so while it sits in the material, two such
    /// shapes compare unequal and the batch cannot merge them. A hundred and
    /// sixty identical rounded rectangles became a hundred and sixty draws for
    /// that reason alone, against one for the tessellated route.
    ///
    /// Applying the mapping here instead puts it on `uv`, which
    /// `paint_at_texture_coords` tells the shader to read as the paint's space.
    /// The material is then equal for every placement of one shape, and the
    /// draws merge.
    ///
    /// Exact rather than an approximation. The per-fragment mapping this
    /// replaces is applied to an interpolated position, and the routes that
    /// reach here are affine by construction -- an analytic shape refuses
    /// perspective. Over an affine the mapping is linear in what the rasterizer
    /// interpolates, so evaluating it at the corners is the same arithmetic in
    /// a different place.
    pub fn fill_into_local(
        &mut self,
        batch: &mut Batch,
        path: &Path,
        transform: impl Into<Transform2D>,
        paint: &Paint,
        clip_to_local: [f32; 12],
    ) -> Result<()> {
        let geo = self.fill_path(path, transform);
        let indices = geo.indices.to_vec();
        let vertices: Vec<Vertex> = geo
            .vertices()
            .into_iter()
            .map(|v| {
                let [x, y, w] = v.position;
                // The shader's own mapping, at a vertex: three packed columns
                // against the homogeneous clip position, then the divide. The
                // `max` guards the vanishing line as the shader does, so a
                // degenerate vertex lands far from the origin rather than at a
                // NaN the blend would spread.
                let m = [
                    clip_to_local[0] * x + clip_to_local[4] * y + clip_to_local[8] * w,
                    clip_to_local[1] * x + clip_to_local[5] * y + clip_to_local[9] * w,
                    clip_to_local[2] * x + clip_to_local[6] * y + clip_to_local[10] * w,
                ];
                let z = m[2].max(1e-6);
                Vertex::projected(v.position, [m[0] / z, m[1] / z])
            })
            .collect();
        batch.push_mesh_tinted(
            &vertices,
            &indices,
            paint.material.clone(),
            paint.filter,
            paint.blend,
            paint.clip,
            paint.stencil,
            BlendMode::Modulate,
            true,
        )
    }

    /// Tessellate a stroked path and append it to a batch.
    pub fn stroke_into(
        &mut self,
        batch: &mut Batch,
        path: &Path,
        style: &StrokeStyle,
        dash: Option<&Dash>,
        transform: impl Into<Transform2D>,
        paint: &Paint,
    ) -> Result<()> {
        let geo = self.stroke_path(path, style, dash, transform);
        let vertices = geo.vertices();
        let indices = geo.indices.to_vec();
        batch.push_mesh(
            &vertices,
            &indices,
            paint.material.clone(),
            paint.filter,
            paint.blend,
            paint.clip,
            paint.stencil,
        )
    }

    fn to_clip_space<'a>(
        scratch: &'a mut Vec<Vec3>,
        buffers: &'a VertexBuffers,
        transform: Transform2D,
        projection: Affine2,
    ) -> ClipGeometry<'a> {
        // One combined transform rather than two passes: composing first means
        // each vertex is touched once and rounds once.
        let to_clip = Transform2D::from(projection) * transform;
        scratch.clear();
        // Undivided. The rasterizer does that, and it needs the divisor first
        // in order to clip against the plane where it reaches zero -- which is
        // the whole of how geometry crossing the vanishing line is handled.
        scratch.extend(
            buffers
                .vertices
                .iter()
                .map(|p| to_clip.project_homogeneous(*p)),
        );
        ClipGeometry {
            vertices: scratch,
            indices: &buffers.indices,
        }
    }
}

/// Triangles in clip space, ready for a backend.
pub struct ClipGeometry<'a> {
    pub vertices: &'a [Vec3],
    pub indices: &'a [u32],
}

impl ClipGeometry<'_> {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Vertices as a flat array, which is the shape backends take them in.
    pub fn positions(&self) -> Vec<[f32; 3]> {
        self.vertices.iter().map(|v| [v.x, v.y, v.z]).collect()
    }

    /// The same, as vertices a batch takes directly.
    ///
    /// Tessellated geometry has no texture coordinates of its own, and the
    /// materials it carries locate themselves from the clip position instead.
    pub fn vertices(&self) -> Vec<Vertex> {
        self.vertices
            .iter()
            .map(|v| Vertex::at_projected([v.x, v.y, v.z]))
            .collect()
    }
}

/// Convert a device-space tolerance into path space.
///
/// The path is what makes this answerable under perspective: the stretch a
/// homography applies varies from point to point, so the question "how finely
/// must this be flattened" only has an answer over a region, and the path's own
/// bounds are that region. Under an affine the bounds make no difference and
/// the result is what it always was.
///
/// A path reaching the vanishing line has no bound to give, and the flat
/// tolerance stands in. What that produces is a shape flattened as though it
/// were unmagnified, whose far portion the rasterizer then clips away anyway --
/// coarse where it survives, rather than an unbounded number of segments spent
/// on a part of the plane that is not being drawn.
fn path_space_tolerance(tolerance: f32, transform: Transform2D, path: &Path) -> f32 {
    let bounds = path.bounds();
    let scale = transform.max_scale_over(bounds.min, bounds.max);
    match scale {
        Some(scale) => impeller_geometry::flatten::tolerance_for_scale(tolerance, scale),
        None => tolerance,
    }
}

/// The default flattening tolerance, in device pixels.
pub const TOLERANCE: f32 = DEFAULT_TOLERANCE;

#[cfg(test)]
mod tests {
    use super::*;
    use impeller_geometry::PathBuilder;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(x0, y0))
            .line_to(Vec2::new(x1, y0))
            .line_to(Vec2::new(x1, y1))
            .line_to(Vec2::new(x0, y1))
            .close();
        b.build()
    }

    #[test]
    fn a_full_target_rect_reaches_the_corners_of_clip_space() {
        let mut r = Renderer::new();
        r.begin_frame(Extent2D::new(64, 64), TOLERANCE);
        let geo = r.fill_path(&rect(0.0, 0.0, 64.0, 64.0), Affine2::IDENTITY);
        let xs: Vec<f32> = geo.vertices.iter().map(|v| v.x).collect();
        let ys: Vec<f32> = geo.vertices.iter().map(|v| v.y).collect();
        let min = |v: &[f32]| v.iter().copied().fold(f32::INFINITY, f32::min);
        let max = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);

        assert!((min(&xs) + 1.0).abs() < 1e-5);
        assert!((max(&xs) - 1.0).abs() < 1e-5);
        assert!((min(&ys) + 1.0).abs() < 1e-5);
        assert!((max(&ys) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn tolerance_is_scaled_by_the_transform_but_not_by_the_target_size() {
        // A curve tessellated under a 10x transform must gain segments,
        // because it covers ten times the device pixels.
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .cubic_to(
                Vec2::new(0.0, 10.0),
                Vec2::new(10.0, 10.0),
                Vec2::new(10.0, 0.0),
            )
            .close();
        let path = b.build();
        let target = Extent2D::new(256, 256);

        let mut r = Renderer::new();
        r.begin_frame(target, TOLERANCE);
        let plain = r.fill_path(&path, Affine2::IDENTITY).triangle_count();
        let scaled = r
            .fill_path(&path, Affine2::from_scale(Vec2::splat(10.0)))
            .triangle_count();
        assert!(
            scaled > plain,
            "scaling up should refine flattening: {plain} then {scaled}"
        );

        // Target size must not affect it. Folding the projection into the
        // tolerance scale would make a large target flatten coarsely.
        r.begin_frame(Extent2D::new(4096, 4096), TOLERANCE);
        let big_target = r.fill_path(&path, Affine2::IDENTITY).triangle_count();
        assert_eq!(big_target, plain);
    }

    #[test]
    fn an_empty_path_produces_no_geometry() {
        let mut r = Renderer::new();
        r.begin_frame(Extent2D::new(64, 64), TOLERANCE);
        let geo = r.fill_path(&Path::default(), Affine2::IDENTITY);
        assert!(geo.is_empty());
        assert!(geo.positions().is_empty());
    }
}

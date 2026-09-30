//! Mapping user space onto clip space.
//!
//! Three coordinate systems meet here, and confusing any two of them produces
//! output that is mirrored or off by a scale factor rather than obviously
//! broken:
//!
//! - **Path space** — whatever units a path was authored in.
//! - **Device space** — pixels on the target. Origin top-left, X right, Y
//!   *down*, which is the convention 2D UI works in.
//! - **Clip space** — what the vertex shader emits. Both axes in `[-1, 1]`,
//!   and Y runs *up*, following the WGSL convention that shader translation
//!   normalizes to. This is the axis that flips.
//!
//! Tolerance is a device-space quantity, so flattening a path that will be
//! scaled up has to happen more finely in path space. [`max_scale`] is what
//! that adjustment is computed from.

use glam::{Affine2, Mat2, Mat3, Vec2, Vec3};

/// A transform of the plane that may carry perspective.
///
/// A three-by-three homography acting on a point written `(x, y, 1)`, which is
/// what `dart:ui`'s four-by-four reduces to for content on the `z = 0` plane.
/// An affine is the case whose bottom row is `(0, 0, 1)`, and it is the common
/// one — [`is_affine`](Self::is_affine) is how the fast paths ask.
///
/// # Why this is a newtype and not a `Mat3`
///
/// Because [`glam::Mat3::transform_point2`] exists, compiles here, reads
/// correctly, and is wrong. It documents itself as assuming a valid affine
/// transform: it computes `x_axis * x + y_axis * y + z_axis` and stops, never
/// dividing by the `w` it just produced. Anyone converting a call site would
/// reach for it, and what comes out is a picture that is plausible and is not
/// of the transform that was asked for — the failure this renderer is built to
/// refuse.
///
/// So the divide is not optional here. This type offers
/// [`project_point2`](Self::project_point2), which divides, and
/// [`project_homogeneous`](Self::project_homogeneous), which hands back the
/// undivided triple for the one caller that wants it — the vertex path, where
/// the rasterizer does the divide and needs `w` to clip against first. There is
/// no method that maps a point without doing one or the other.
///
/// # Storage
///
/// glam's `Mat3` is column-major, so `z_axis.xy` is the translation and the
/// bottom row — the one that makes this projective — is spread across the three
/// columns as `(x_axis.z, y_axis.z, z_axis.z)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D(Mat3);

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<Affine2> for Transform2D {
    fn from(affine: Affine2) -> Self {
        Self::from_affine(affine)
    }
}

/// So that a caller holding an affine by reference need not name the lift.
impl From<&Affine2> for Transform2D {
    fn from(affine: &Affine2) -> Self {
        Self::from_affine(*affine)
    }
}

impl std::ops::Mul for Transform2D {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        Self(self.0 * rhs.0)
    }
}

/// So that composing with an affine reads the way composing two of these does.
///
/// Most of what a transform is composed with here is affine — a projection, a
/// translation to a paint's origin, a rotation folded into a gradient's space —
/// and lifting each of those at the call site would bury the composition it is
/// there to express.
impl std::ops::Mul<Affine2> for Transform2D {
    type Output = Self;

    fn mul(self, rhs: Affine2) -> Self {
        self * Self::from_affine(rhs)
    }
}

impl std::ops::Mul<Transform2D> for Affine2 {
    type Output = Transform2D;

    fn mul(self, rhs: Transform2D) -> Transform2D {
        Transform2D::from_affine(self) * rhs
    }
}

impl Transform2D {
    pub const IDENTITY: Self = Self(Mat3::IDENTITY);

    /// Lift an affine into a homography, which is exact and free.
    pub fn from_affine(affine: Affine2) -> Self {
        let m = affine.matrix2;
        let t = affine.translation;
        Self(Mat3::from_cols(
            Vec3::new(m.x_axis.x, m.x_axis.y, 0.0),
            Vec3::new(m.y_axis.x, m.y_axis.y, 0.0),
            Vec3::new(t.x, t.y, 1.0),
        ))
    }

    /// The affine this is, or `None` if it carries perspective.
    pub fn to_affine(self) -> Option<Affine2> {
        self.is_affine().then(|| {
            let c = self.0.to_cols_array();
            Affine2::from_mat2_translation(
                Mat2::from_cols(Vec2::new(c[0], c[1]), Vec2::new(c[3], c[4])),
                Vec2::new(c[6], c[7]),
            )
        })
    }

    /// Whether the bottom row is `(0, 0, 1)`, so that `w` is one everywhere.
    ///
    /// Tested exactly rather than against a tolerance, and the distinction is
    /// worth stating because it looks like an oversight. The bottom row's first
    /// two entries have units of inverse length, so there is no scale-free
    /// threshold to compare them against: whether `0.001` is negligible depends
    /// entirely on how far across the plane the geometry reaches. What this
    /// answers is "was perspective asked for", which is a question about how the
    /// transform was built, and a transform built from an affine has exact
    /// zeros there. Composition preserves them exactly, since the product's
    /// bottom row is `(0, 0, 1)` times the other matrix.
    ///
    /// A caller wanting "close enough to affine over this region" is asking a
    /// different question, and it needs the region.
    pub fn is_affine(self) -> bool {
        let c = self.0.to_cols_array();
        c[2] == 0.0 && c[5] == 0.0 && c[8] == 1.0
    }

    /// Reduce the `dart:ui` four-by-four, which is column-major and sixteen long.
    ///
    /// Exact for the content this renderer draws rather than an approximation of
    /// it. Applying a four-by-four to `(x, y, 0, 1)` never reads the `z` column,
    /// and the `z` row only produces a depth nothing here has a use for — so
    /// what is left, rows and columns zero, one and three, is the whole of what
    /// the matrix means on the plane.
    pub fn from_column_major_4x4(m: &[f32; 16]) -> Self {
        Self(Mat3::from_cols(
            Vec3::new(m[0], m[1], m[3]),
            Vec3::new(m[4], m[5], m[7]),
            Vec3::new(m[12], m[13], m[15]),
        ))
    }

    /// The four-by-four this reduces from, with the `z` axis left alone.
    pub fn to_column_major_4x4(self) -> [f32; 16] {
        let c = self.0.to_cols_array();
        [
            c[0], c[1], 0.0, c[2], //
            c[3], c[4], 0.0, c[5], //
            0.0, 0.0, 1.0, 0.0, //
            c[6], c[7], 0.0, c[8],
        ]
    }

    /// The point mapped and divided.
    ///
    /// A point on the vanishing line has no image, and the divide reports that
    /// as an infinity or a NaN rather than a wrong finite answer — which is what
    /// every caller here already guards for with `is_finite`.
    pub fn project_point2(self, point: Vec2) -> Vec2 {
        let h = self.project_homogeneous(point);
        h.truncate() / h.z
    }

    /// The point mapped and *not* divided.
    ///
    /// What a vertex carries. The rasterizer divides, and it needs `w` first in
    /// order to clip against the plane where `w` reaches zero.
    pub fn project_homogeneous(self, point: Vec2) -> Vec3 {
        self.0 * Vec3::new(point.x, point.y, 1.0)
    }

    /// The inverse, or `None` if there is not one.
    ///
    /// An affine inverts as an affine, through `Affine2` rather than through the
    /// general path, which is cheaper and — the part worth writing down — makes
    /// the exactness this code's property instead of a dependency's.
    ///
    /// Everything downstream that asks [`is_affine`](Self::is_affine) is asking
    /// about an exact zero, and an inverse taken the general way arrives at that
    /// zero through the same adjugate arithmetic as every other entry. It does
    /// in fact land on it today: glam returns a bottom row of exactly
    /// `(0, -0, 1)` for a lifted affine, and negative zero compares equal, so
    /// routing affines through the general path would work. It would work
    /// because of how a dependency happens to arrange one division, which is not
    /// a thing this can check and not a thing a version bump would announce. The
    /// branch costs a comparison and removes the question.
    pub fn inverse(self) -> Option<Self> {
        if let Some(affine) = self.to_affine() {
            let determinant = affine.matrix2.determinant();
            if determinant.abs() <= 1e-9 || !determinant.is_finite() {
                return None;
            }
            let inverse = affine.inverse();
            return inverse.is_finite().then(|| Self::from_affine(inverse));
        }
        let determinant = self.0.determinant();
        if determinant.abs() <= 1e-9 || !determinant.is_finite() {
            return None;
        }
        let inverse = self.0.inverse();
        inverse.is_finite().then_some(Self(inverse))
    }

    /// Whether every entry is a number, which a caller checks before handing
    /// this to a shader that would otherwise spread a NaN across a draw.
    pub fn is_finite(self) -> bool {
        self.0.is_finite()
    }

    /// The determinant, which is zero exactly when the plane has collapsed.
    pub fn determinant(self) -> f32 {
        self.0.determinant()
    }

    /// [`transformed_bounds`], for a transform already in this form.
    pub fn transformed_bounds_of(self, min: Vec2, max: Vec2) -> Option<(Vec2, Vec2)> {
        transformed_bounds(self, min, max)
    }

    /// The nine floats, in column order.
    pub fn to_cols_array(self) -> [f32; 9] {
        self.0.to_cols_array()
    }

    /// The largest factor by which this can stretch a direction anywhere in a
    /// box, or `None` if the box reaches the vanishing line.
    ///
    /// [`max_scale`] answers this for an affine with one number, because an
    /// affine stretches every part of the plane alike. A homography does not:
    /// writing it as `(A p + b) / (R · p + s)`, its derivative at `p` is
    /// `(A - N(p) Rᵀ) / w(p)` for `N` the mapped point and `w` the divisor, so
    /// the stretch grows as roughly one over `w` squared and the near end of a
    /// shape needs finer flattening than the far end.
    ///
    /// Bounding it over a box rather than solving for the worst point: `w` is
    /// affine, so its smallest value over a box is at a corner, and while `w`
    /// stays positive the image of the box is the convex hull of the mapped
    /// corners, so the largest `‖N‖` is at a corner too. Four evaluations, the
    /// same four [`transformed_bounds`] already makes.
    ///
    /// Taking the largest is conservative in the direction that costs
    /// triangles rather than correctness: the far end of a shape is flattened
    /// more finely than it needs, and the near end is flattened finely enough.
    ///
    /// For an affine this reduces to [`max_scale`] exactly — `R` is zero and
    /// `w` is one, leaving the longer basis vector — so no shape already being
    /// drawn is tessellated any differently than it was.
    pub fn max_scale_over(self, min: Vec2, max: Vec2) -> Option<f32> {
        let c = self.0.to_cols_array();
        let linear = Vec2::new(c[0], c[1])
            .length()
            .max(Vec2::new(c[3], c[4]).length());
        if self.is_affine() {
            return linear.is_finite().then_some(linear);
        }
        let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
        let mut smallest_w = f32::INFINITY;
        let mut furthest = 0.0f32;
        for corner in corners {
            let h = self.project_homogeneous(corner);
            smallest_w = smallest_w.min(h.z);
            furthest = furthest.max((h.truncate() / h.z).length());
        }
        // At or past the vanishing line there is no bound to give. The caller
        // decides what to do about it; inventing a number here would be the
        // one answer that cannot be checked.
        // NaN tested for by name rather than by inverting the comparison: a
        // NaN divisor compares false against everything, so `<=` alone would
        // let it through as though the box were safely in front.
        if smallest_w.is_nan() || smallest_w <= VANISHING_EPSILON || !furthest.is_finite() {
            return None;
        }
        let perspective_row = Vec2::new(c[2], c[5]).length();
        let bound = (linear + furthest * perspective_row) / smallest_w;
        if !bound.is_finite() {
            return None;
        }
        // The bound grows without limit toward the vanishing line, and past
        // some point the extra subdivision is buying detail no display
        // resolves. Capping it against the flat part keeps the work within a
        // fixed multiple of what the same shape costs under an affine.
        //
        // The cap matters more than it looks. `MAX_SEGMENTS` is a *silent*
        // quality floor -- a curve that reaches it is under-flattened and
        // nothing says so -- and without a cap here ordinary perspective
        // content would reach it and the picture would quietly go faceted.
        // With one, a curve needing ten segments flat needs at most six
        // hundred and forty, and `MAX_SEGMENTS` goes back to being a backstop
        // against nonsense.
        Some(bound.min(linear * MAX_PERSPECTIVE_REFINEMENT))
    }
}

/// How far past the affine cost a shape may be flattened. See
/// [`Transform2D::max_scale_over`].
const MAX_PERSPECTIVE_REFINEMENT: f32 = 64.0;

/// How near the vanishing line counts as on it.
///
/// Absolute rather than relative because `w` is already normalized: an affine
/// lifted here has `w` of exactly one everywhere, so this is a fraction of that
/// rather than of an arbitrary scale.
const VANISHING_EPSILON: f32 = 1e-6;

/// Map device pixels onto clip space for a target of the given size.
///
/// Device Y runs down from the top-left and clip Y runs up, so this flips the
/// Y axis. Getting the flip wrong renders everything upside down, which is
/// easy to miss on symmetric test content and obvious on text.
pub fn viewport_projection(width: u32, height: u32) -> Affine2 {
    // Guard against a zero-sized target producing infinities that then
    // propagate into every vertex as NaN.
    let w = if width == 0 { 1.0 } else { width as f32 };
    let h = if height == 0 { 1.0 } else { height as f32 };
    Affine2::from_cols(
        Vec2::new(2.0 / w, 0.0),
        Vec2::new(0.0, -2.0 / h),
        Vec2::new(-1.0, 1.0),
    )
}

/// The largest factor by which a transform can stretch a direction.
///
/// Used to scale flattening tolerance: a curve drawn at four times its
/// authored size needs finer subdivision in path space to stay within the same
/// device-space error. Estimated from the basis vector lengths, which bounds
/// the true largest singular value from below by at most a factor of the
/// square root of two — close enough for choosing a segment count, and cheaper
/// than a decomposition.
pub fn max_scale(transform: &Affine2) -> f32 {
    let x = transform.matrix2.x_axis.length();
    let y = transform.matrix2.y_axis.length();
    x.max(y)
}

/// Whether a transform maps axis-aligned rectangles to axis-aligned rectangles.
///
/// True for any composition of translation, scale, reflection, and quarter
/// turns; false as soon as an arbitrary rotation or a skew is involved. This is
/// what decides whether a rectangular clip can be handed to the fixed-function
/// scissor unit exactly, or whether it needs machinery that can express a
/// rotated quadrilateral.
///
/// The comparison is relative rather than exact because a quarter turn does not
/// produce exact zeros: `cos` of a right angle in `f32` is about `-4.4e-8`, so
/// a caller who asked for exactly that rotation would otherwise be told their
/// rectangle is no longer one. The threshold is scaled by the transform's own
/// magnitude, since an absolute one means something different at a scale of a
/// thousand than at a scale of a thousandth.
/// # Perspective is never axis-preserving, and the check is exact
///
/// A homography carries axis-aligned rectangles to axis-aligned rectangles only
/// when its bottom row is `(0, 0, s)` — that is, only when it is an affine.
/// With a bottom row of `(p, 0, 1)` a vertical line stays vertical, but a
/// horizontal one does not: `y` comes back divided by `p x + 1`, which varies
/// along the line. So anything that is not exactly affine is refused here.
///
/// Refused exactly, not within a tolerance, and deliberately so. The bottom
/// row's entries have units of inverse length, so there is no scale-free
/// threshold that could say whether a small one is negligible — it depends on
/// how far the geometry reaches. Being exact costs a transform with a
/// vanishingly small perspective term the scissor fast path, which is a stencil
/// pass nobody will measure; being approximate would cost a wrong clip, which
/// is a picture with pixels in it the caller removed.
///
/// This is the one place in the perspective work where the existing correct
/// code would have gone wrong without changing: it read the two-by-two and
/// nothing else, and a pure scale carrying a perspective row would have been
/// waved through to the scissor unit as a rectangle it is not.
pub fn preserves_axis_alignment(transform: impl Into<Transform2D>) -> bool {
    let transform = transform.into();
    let Some(affine) = transform.to_affine() else {
        return false;
    };
    let m = affine.matrix2;
    let magnitude = max_scale(&affine);
    if magnitude == 0.0 || !magnitude.is_finite() {
        // A degenerate transform collapses every rectangle to a line or a
        // point. That is a rectangle in the trivial sense and an empty clip in
        // practice, so it is left to the caller's intersection to resolve
        // rather than reported as an unsupported shape.
        return true;
    }
    let epsilon = 1e-6 * magnitude;
    // Diagonal is a scale, possibly reflected; anti-diagonal is that composed
    // with a quarter turn. Anything else rotates or skews.
    let diagonal = m.x_axis.y.abs() <= epsilon && m.y_axis.x.abs() <= epsilon;
    let anti_diagonal = m.x_axis.x.abs() <= epsilon && m.y_axis.y.abs() <= epsilon;
    diagonal || anti_diagonal
}

/// Bounds standing in for "anywhere", where a mapping gives none.
///
/// The conservative answer in the direction that is safe: every caller of
/// [`transformed_bounds`] narrows something by what it returns, so a bound that
/// is too large costs the chance to allocate something smaller, and one that is
/// too small drops a shape that should have been drawn. A layer's bounds are
/// clamped to its parent's target before anything is allocated, so this becomes
/// the target rather than a demand for a texture the size of the number.
///
/// A quarter of the float range rather than the whole of it, so that a caller
/// adding a blur's reach to one of these still has a finite number.
pub fn unbounded() -> (Vec2, Vec2) {
    let far = f32::MAX / 4.0;
    (Vec2::splat(-far), Vec2::splat(far))
}

/// The bounds of a rectangle's corners after a transform, if there are any.
///
/// Exact when [`preserves_axis_alignment`] holds, and the bounding box of a
/// rotated quadrilateral otherwise — which is why callers that need the clip to
/// be the region asked for must check that first rather than relying on this to
/// tell them.
///
/// # Why this can fail, and why it says so rather than guessing
///
/// Four corners bound a rectangle's image because a transform carries the
/// rectangle's convex hull onto the hull of the corners' images. Under a
/// homography that still holds — but only while the divisor stays positive
/// across the whole rectangle. Let the rectangle reach the vanishing line and
/// the corners land on both sides of infinity, and their bounding box is not
/// merely loose, it is wrong: too small, which is the unsafe direction, because
/// every caller here narrows something by it and a bound that is too small
/// drops a shape that should have been drawn.
///
/// So that case returns `None` rather than a number. Not an empty rectangle,
/// which would read as "nothing is there", and not an enormous one, which would
/// read as "everything is". `None` makes each caller say what it does about a
/// bound that does not exist, and there are two different right answers among
/// them: a clip that cannot be narrowed is left alone, and a layer that cannot
/// be sized is refused.
pub fn transformed_bounds(
    transform: impl Into<Transform2D>,
    min: Vec2,
    max: Vec2,
) -> Option<(Vec2, Vec2)> {
    let transform = transform.into();
    let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
    let mut mapped = [Vec2::ZERO; 4];
    for (out, corner) in mapped.iter_mut().zip(corners) {
        let h = transform.project_homogeneous(corner);
        if h.z.is_nan() || h.z <= VANISHING_EPSILON {
            return None;
        }
        *out = h.truncate() / h.z;
    }
    Some(mapped.iter().fold(
        (mapped[0], mapped[0]),
        |(lo, hi): (Vec2, Vec2), c: &Vec2| (lo.min(*c), hi.max(*c)),
    ))
}

/// Apply a transform to every point in place.
pub fn transform_points(points: &mut [Vec2], transform: &Affine2) {
    for p in points.iter_mut() {
        *p = transform.transform_point2(*p);
    }
}

/// Invert a paint's placement into the form a shader's `to_local` takes.
///
/// `local_to_clip` carries the paint's own space onto clip space — the canvas
/// transform, then wherever the paint sits, then whatever normalization its
/// kind wants, a radius or a rectangle's size. What comes back is the inverse,
/// as three columns of a three-by-three each padded to four floats, which is how
/// a `mat3x3` sits in a uniform block and what lets both backends copy the
/// packed material in without writing padding around anything.
///
/// # Why the whole matrix rather than a linear part and an anchor
///
/// It used to be four floats and a separately packed clip-space anchor, and the
/// shader subtracted the one before applying the other. That works only for an
/// affine, where the image of a difference is the difference of the images. A
/// map with perspective divides by a quantity that depends on the absolute
/// position, so an anchor subtracted before the mapping is subtracted in the
/// wrong space — correct only in the case that made it look correct. Inside the
/// matrix is the one place the translation belongs, and it rides there for free.
///
/// # A placement that has collapsed
///
/// A degenerate transform — a zero scale, or one axis collapsed — has no
/// inverse. That is a caller mistake rather than a renderer one, and the shape
/// it fills is collapsed to nothing anyway, so the paint it would have carried
/// is not observable: the geometry went through the same matrix the mapping is
/// the inverse of, which is the whole of why substituting anything here is safe.
///
/// The identity is what stands in, and it is chosen over anything else because
/// its bottom row is `(0, 0, 1)`. A fragment that reached it despite the above
/// would divide by one rather than by zero, and a NaN put into a fragment's
/// color survives the blend and spreads across whatever it touches.
pub fn invert_to_local(local_to_clip: Transform2D) -> [f32; 12] {
    to_local_columns(local_to_clip.inverse().unwrap_or(Transform2D::IDENTITY))
}

/// The same packing, for a mapping already stated in the direction the shader
/// reads it.
///
/// A layer composite and a picture's placement both build the clip-to-local
/// mapping directly, because they know the texture's size rather than a
/// placement to invert. Inverting a matrix only to invert it back would be
/// arithmetic with nothing to show for it.
pub fn to_local_columns(clip_to_local: Transform2D) -> [f32; 12] {
    let c = clip_to_local.to_cols_array();
    let padded = [
        c[0], c[1], c[2], 0.0, //
        c[3], c[4], c[5], 0.0, //
        c[6], c[7], c[8], 0.0,
    ];
    if padded.iter().all(|v| v.is_finite()) {
        padded
    } else {
        [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_translations_and_quarter_turns_keep_rectangles_rectangular() {
        let quarter = std::f32::consts::FRAC_PI_2;
        for transform in [
            Affine2::IDENTITY,
            Affine2::from_translation(Vec2::new(13.0, -4.0)),
            Affine2::from_scale(Vec2::new(3.0, 0.5)),
            // A reflection, which is a negative scale rather than a rotation.
            Affine2::from_scale(Vec2::new(-1.0, 1.0)),
            Affine2::from_angle(quarter),
            Affine2::from_angle(-quarter),
            Affine2::from_angle(2.0 * quarter),
            // Composition of all of them is still axis-preserving.
            Affine2::from_angle(quarter)
                * Affine2::from_scale(Vec2::new(2.0, 7.0))
                * Affine2::from_translation(Vec2::new(1.0, 1.0)),
        ] {
            assert!(
                preserves_axis_alignment(transform),
                "{transform:?} was rejected"
            );
        }
    }

    #[test]
    fn arbitrary_rotations_and_skews_do_not() {
        let mut skew = Affine2::IDENTITY;
        skew.matrix2.y_axis.x = 0.4;
        for transform in [
            Affine2::from_angle(0.3),
            Affine2::from_angle(std::f32::consts::FRAC_PI_4),
            skew,
        ] {
            assert!(
                !preserves_axis_alignment(transform),
                "{transform:?} was accepted"
            );
        }
    }

    #[test]
    fn the_threshold_scales_with_the_transform() {
        // The same tiny skew is noise beside a large scale and the whole of the
        // transform beside a small one. An absolute threshold would call both
        // the same thing.
        let skew = 1e-3;
        let mut large = Affine2::from_scale(Vec2::splat(1e4));
        large.matrix2.y_axis.x = skew;
        assert!(preserves_axis_alignment(large));

        let mut small = Affine2::from_scale(Vec2::splat(1e-2));
        small.matrix2.y_axis.x = skew;
        assert!(!preserves_axis_alignment(small));
    }

    #[test]
    fn a_quarter_turn_maps_a_rectangle_onto_the_other_axis() {
        let quarter = Affine2::from_angle(std::f32::consts::FRAC_PI_2);
        let (min, max) = transformed_bounds(quarter, Vec2::new(0.0, 0.0), Vec2::new(4.0, 1.0))
            .expect("an affine always has bounds");
        // Width and height exchange places; the corner positions follow the
        // rotation rather than staying put.
        assert!(
            (max.x - min.x - 1.0).abs() < 1e-5,
            "width became {}",
            max.x - min.x
        );
        assert!(
            (max.y - min.y - 4.0).abs() < 1e-5,
            "height became {}",
            max.y - min.y
        );
    }

    fn approx(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn the_top_left_pixel_maps_to_the_top_of_clip_space() {
        let p = viewport_projection(64, 64);
        // Device origin is top-left; clip Y runs up, so the top is +1.
        assert!(approx(
            p.transform_point2(Vec2::new(0.0, 0.0)),
            Vec2::new(-1.0, 1.0)
        ));
    }

    #[test]
    fn the_bottom_right_corner_maps_to_the_bottom_of_clip_space() {
        let p = viewport_projection(64, 64);
        assert!(approx(
            p.transform_point2(Vec2::new(64.0, 64.0)),
            Vec2::new(1.0, -1.0)
        ));
    }

    #[test]
    fn the_center_maps_to_the_origin() {
        let p = viewport_projection(800, 600);
        assert!(approx(
            p.transform_point2(Vec2::new(400.0, 300.0)),
            Vec2::ZERO
        ));
    }

    #[test]
    fn the_y_axis_is_flipped_and_the_x_axis_is_not() {
        let p = viewport_projection(100, 100);
        let top = p.transform_point2(Vec2::new(50.0, 10.0));
        let bottom = p.transform_point2(Vec2::new(50.0, 90.0));
        // Further down in device space must be further down in clip space,
        // which under a Y-up clip convention means a smaller value.
        assert!(top.y > bottom.y, "device Y down should map to clip Y up");

        let left = p.transform_point2(Vec2::new(10.0, 50.0));
        let right = p.transform_point2(Vec2::new(90.0, 50.0));
        assert!(left.x < right.x, "X must not be flipped");
    }

    #[test]
    fn a_non_square_target_scales_each_axis_independently() {
        let p = viewport_projection(200, 100);
        // Half the width in device space is the same clip distance as half the
        // height; a single shared scale factor would squash one axis.
        assert!(approx(
            p.transform_point2(Vec2::new(200.0, 0.0)),
            Vec2::new(1.0, 1.0)
        ));
        assert!(approx(
            p.transform_point2(Vec2::new(0.0, 100.0)),
            Vec2::new(-1.0, -1.0)
        ));
    }

    #[test]
    fn a_zero_sized_target_does_not_produce_non_finite_coordinates() {
        let p = viewport_projection(0, 0);
        let v = p.transform_point2(Vec2::new(1.0, 1.0));
        assert!(v.is_finite(), "got {v:?}");
    }

    #[test]
    fn max_scale_reports_the_larger_axis() {
        let t = Affine2::from_scale(Vec2::new(3.0, 7.0));
        assert!((max_scale(&t) - 7.0).abs() < 1e-5);
        assert!((max_scale(&Affine2::IDENTITY) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn max_scale_is_unchanged_by_rotation_and_translation() {
        let rotated = Affine2::from_angle(0.9) * Affine2::from_scale(Vec2::splat(2.0));
        assert!((max_scale(&rotated) - 2.0).abs() < 1e-4);

        let translated = Affine2::from_translation(Vec2::new(100.0, -50.0));
        assert!((max_scale(&translated) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn transforming_points_matches_transforming_them_one_at_a_time() {
        let t =
            Affine2::from_scale_angle_translation(Vec2::new(2.0, 3.0), 0.4, Vec2::new(5.0, -1.0));
        let source = [Vec2::new(1.0, 2.0), Vec2::new(-3.0, 4.0), Vec2::ZERO];
        let mut batch = source;
        transform_points(&mut batch, &t);
        for (i, p) in source.iter().enumerate() {
            assert!(approx(batch[i], t.transform_point2(*p)));
        }
    }

    /// A homography is defined by where it sends four points, so checking it
    /// against a square's corners checks the whole map.
    #[test]
    fn a_homography_maps_a_square_to_the_quadrilateral_its_corners_describe() {
        // Bottom row (0.002, 0, 1): w grows with x, so the far side shrinks.
        let t = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        assert!(!t.is_affine());
        // At x = 100 the divisor is 1.2, at x = 0 it is 1.
        let near = t.project_point2(Vec2::new(0.0, 100.0));
        let far = t.project_point2(Vec2::new(100.0, 100.0));
        assert!(approx(near, Vec2::new(0.0, 100.0)));
        assert!(approx(far, Vec2::new(100.0 / 1.2, 100.0 / 1.2)));
        // The undivided form carries the divisor rather than applying it.
        let homogeneous = t.project_homogeneous(Vec2::new(100.0, 100.0));
        assert!((homogeneous.z - 1.2).abs() < 1e-5);
    }

    /// The property that lets every existing call site keep working.
    #[test]
    fn an_affine_lifts_and_lowers_without_moving_a_point() {
        let affine = Affine2::from_angle(0.3)
            * Affine2::from_scale(Vec2::new(2.0, 7.0))
            * Affine2::from_translation(Vec2::new(11.0, -3.0));
        let lifted = Transform2D::from(affine);
        assert!(lifted.is_affine());
        assert_eq!(lifted.to_affine(), Some(affine));
        for p in [Vec2::ZERO, Vec2::new(5.0, -2.0), Vec2::new(-100.0, 40.0)] {
            assert!(approx(lifted.project_point2(p), affine.transform_point2(p)));
            // And w stayed exactly one, which is what keeps the divide free.
            assert_eq!(lifted.project_homogeneous(p).z, 1.0);
        }
    }

    #[test]
    fn a_transform_round_trips_through_the_four_by_four_form() {
        let t = Transform2D::from_column_major_4x4(&[
            2.0, 0.5, 0.0, 0.003, //
            -1.0, 3.0, 0.0, 0.007, //
            0.0, 0.0, 1.0, 0.0, //
            9.0, -4.0, 0.0, 1.5,
        ]);
        assert_eq!(
            Transform2D::from_column_major_4x4(&t.to_column_major_4x4()),
            t
        );
    }

    #[test]
    fn composing_two_homographies_agrees_with_transforming_twice() {
        let a = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            5.0, 0.0, 0.0, 1.0,
        ]);
        let b = Transform2D::from(Affine2::from_angle(0.4) * Affine2::from_scale(Vec2::splat(2.0)));
        for p in [Vec2::new(3.0, 4.0), Vec2::new(-20.0, 60.0)] {
            assert!(approx(
                (a * b).project_point2(p),
                a.project_point2(b.project_point2(p))
            ));
        }
    }

    /// Composition must not manufacture perspective that nobody asked for, or
    /// every affine fast path downstream turns itself off after one `concat`.
    #[test]
    fn composing_two_affines_stays_exactly_affine() {
        let a = Transform2D::from(Affine2::from_angle(0.3));
        let b = Transform2D::from(Affine2::from_scale(Vec2::new(1e6, 1e-6)));
        assert!((a * b).is_affine());
        assert!((b * a).is_affine());
    }

    #[test]
    fn the_inverse_of_a_projective_map_undoes_it() {
        let t = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.001, //
            0.0, 0.0, 1.0, 0.0, //
            7.0, -2.0, 0.0, 1.0,
        ]);
        let inverse = t.inverse().expect("invertible");
        for p in [Vec2::new(10.0, 20.0), Vec2::new(-30.0, 5.0)] {
            assert!(approx(inverse.project_point2(t.project_point2(p)), p));
        }
    }

    /// The contract the shader's clamp rests on: forward `w` positive means the
    /// inverse's divisor is positive too, so a fragment in front of the
    /// vanishing line is never mistaken for one behind it.
    #[test]
    fn the_inverse_keeps_the_sign_of_the_divisor() {
        let t = Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.001, //
            0.0, 0.0, 1.0, 0.0, //
            7.0, -2.0, 0.0, 1.0,
        ]);
        let inverse = t.inverse().expect("invertible");
        for p in [Vec2::new(10.0, 20.0), Vec2::new(-30.0, 5.0)] {
            let forward = t.project_homogeneous(p);
            assert!(forward.z > 0.0, "test point is in front");
            let back = inverse.project_homogeneous(forward.truncate() / forward.z);
            assert!(back.z > 0.0, "divisor flipped sign through the inverse");
        }
    }

    /// An affine's inverse has to come back exactly affine, not affine to
    /// within rounding, or `is_affine` answers no and the fast paths stop.
    #[test]
    fn the_inverse_of_an_affine_is_still_exactly_affine() {
        let affine = Affine2::from_angle(0.37)
            * Affine2::from_scale(Vec2::new(1e4, 3e-3))
            * Affine2::from_translation(Vec2::new(1234.0, -99.0));
        let inverse = Transform2D::from(affine).inverse().expect("invertible");
        assert!(inverse.is_affine());
        let c = inverse.to_cols_array();
        assert_eq!((c[2], c[5], c[8]), (0.0, 0.0, 1.0));
    }

    #[test]
    fn a_singular_matrix_has_no_inverse_to_report() {
        // Collapsed in one axis, which still has a well-defined image.
        assert!(Transform2D::from(Affine2::from_scale(Vec2::new(0.0, 1.0)))
            .inverse()
            .is_none());
        assert!(Transform2D::from(Affine2::from_scale(Vec2::ZERO))
            .inverse()
            .is_none());
        // Projective and singular: two columns the same.
        let degenerate = Transform2D::from_column_major_4x4(&[
            1.0, 2.0, 0.0, 3.0, //
            1.0, 2.0, 0.0, 3.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        assert!(degenerate.inverse().is_none());
    }

    /// A perspective transform whose divisor grows with x, so the plane is
    /// magnified toward negative x and shrunk toward positive.
    fn receding() -> Transform2D {
        Transform2D::from_column_major_4x4(&[
            1.0, 0.0, 0.0, 0.002, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    /// The property that lets this land without retessellating the world: under
    /// an affine the bound is what `max_scale` always said, whatever box it is
    /// asked about.
    #[test]
    fn an_affine_is_bounded_by_exactly_what_max_scale_reports() {
        for affine in [
            Affine2::IDENTITY,
            Affine2::from_scale(Vec2::new(3.0, 0.5)),
            Affine2::from_angle(0.7) * Affine2::from_scale(Vec2::new(9.0, 2.0)),
            Affine2::from_translation(Vec2::new(500.0, -20.0)),
        ] {
            let lifted = Transform2D::from(affine);
            for (min, max) in [
                (Vec2::ZERO, Vec2::splat(1.0)),
                (Vec2::splat(-1e4), Vec2::splat(1e4)),
            ] {
                assert_eq!(
                    lifted.max_scale_over(min, max),
                    Some(max_scale(&affine)),
                    "{affine:?} over {min:?}..{max:?}"
                );
            }
        }
    }

    #[test]
    fn perspective_is_bounded_more_loosely_where_it_magnifies() {
        let t = receding();
        // Toward negative x the divisor is below one, so the plane is
        // magnified and the same curve needs finer flattening.
        let near = t
            .max_scale_over(Vec2::new(-400.0, -10.0), Vec2::new(-300.0, 10.0))
            .expect("in front");
        let far = t
            .max_scale_over(Vec2::new(300.0, -10.0), Vec2::new(400.0, 10.0))
            .expect("in front");
        assert!(near > far, "near {near} should exceed far {far}");
        // And the far end is still bounded below by nothing silly.
        assert!(far > 0.0);
    }

    #[test]
    fn a_box_reaching_the_vanishing_line_has_no_bound_to_give() {
        let t = receding();
        // The divisor 1 + 0.002x reaches zero at x = -500.
        assert_eq!(
            t.max_scale_over(Vec2::new(-600.0, -10.0), Vec2::new(-400.0, 10.0)),
            None
        );
        assert_eq!(
            t.max_scale_over(Vec2::new(-500.0, -10.0), Vec2::new(-499.0, 10.0)),
            None
        );
    }

    /// Without a cap the bound runs away as a shape approaches the vanishing
    /// line, and `MAX_SEGMENTS` -- which is silent when it bites -- becomes the
    /// thing deciding how a picture looks.
    #[test]
    fn the_bound_is_capped_against_what_the_same_shape_costs_flat() {
        let t = receding();
        let just_in_front = t
            .max_scale_over(Vec2::new(-499.9, -1.0), Vec2::new(-499.5, 1.0))
            .expect("in front");
        assert_eq!(just_in_front, MAX_PERSPECTIVE_REFINEMENT);
    }

    /// The trap this whole change had to avoid, and the one place where code
    /// that was correct would have become wrong without being touched.
    ///
    /// `preserves_axis_alignment` read the two-by-two and nothing else. Widen
    /// the transform and leave that reading alone, and a pure scale carrying a
    /// perspective row is waved through to the fixed-function scissor as a
    /// rectangle -- which it is not, because the divisor varies along a
    /// horizontal line and carries `y` with it. The clip then admits pixels the
    /// caller asked to remove, silently, on both backends alike.
    #[test]
    fn a_projective_row_is_not_axis_preserving_however_plain_the_two_by_two_looks() {
        for row in [[0.002, 0.0], [0.0, 0.002], [-1e-4, 3e-5]] {
            let projective = Transform2D::from_column_major_4x4(&[
                3.0, 0.0, 0.0, row[0], //
                0.0, 7.0, 0.0, row[1], //
                0.0, 0.0, 1.0, 0.0, //
                20.0, -5.0, 0.0, 1.0,
            ]);
            // The two-by-two on its own is a plain scale, and would pass.
            assert!(preserves_axis_alignment(Affine2::from_scale(Vec2::new(
                3.0, 7.0
            ))));
            assert!(
                !preserves_axis_alignment(projective),
                "a perspective row of {row:?} was accepted as axis-preserving"
            );
        }
    }

    #[test]
    fn an_affine_always_has_bounds_and_a_box_across_the_horizon_has_none() {
        let t = receding();
        // Wholly in front: the four corners still bound the image, because a
        // homography with a positive divisor preserves convexity.
        let (min, max) = t
            .transformed_bounds_of(Vec2::new(-100.0, -50.0), Vec2::new(100.0, 50.0))
            .expect("in front");
        assert!(min.x < max.x && min.y < max.y);
        // Reaching the vanishing line at x = -500, where the corners land on
        // both sides of infinity and their box is too small rather than loose.
        assert_eq!(
            t.transformed_bounds_of(Vec2::new(-600.0, -50.0), Vec2::new(-400.0, 50.0)),
            None
        );
        // An affine never fails, which is what keeps every existing caller
        // taking the answer it always did.
        assert!(transformed_bounds(
            Affine2::from_scale(Vec2::new(1e6, 1e-6)),
            Vec2::splat(-1e3),
            Vec2::splat(1e3)
        )
        .is_some());
    }

    /// The fallback is wide rather than narrow, and finite enough to add to.
    #[test]
    fn unbounded_is_large_and_still_arithmetic() {
        let (min, max) = unbounded();
        assert!(min.x < -1e30 && max.x > 1e30);
        assert!(
            (max - min).is_finite(),
            "a reach added to this must stay finite"
        );
        assert!((min - Vec2::splat(1e6)).is_finite());
    }
}

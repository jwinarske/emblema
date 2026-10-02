//! The recording surface.
//!
//! A canvas is where drawing is expressed. It holds a transform stack and
//! accumulates shapes into a batch, which is what a backend is eventually
//! handed. Recording is separate from submitting so that a whole frame can be
//! described before any of it reaches the GPU — that separation is what lets
//! draws be batched into one pass rather than submitted one at a time.

use crate::paint::{GradientStop, ImageFilter, MaskBlurStyle, Paint, Shader, Style};
use crate::ramp::Ramp;
use crate::vertices::{Sprite, VertexMode, Vertices};
use crate::Color;
use emblema_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
use emblema_geometry::transform::{
    invert_to_local, max_scale, preserves_axis_alignment, to_local_columns, transformed_bounds,
    unbounded, viewport_projection, Transform2D,
};
use emblema_geometry::{FillRule, Path, PathBuilder, Rect as GeometryRect};
use emblema_hal::{
    Batch, BlendMode, ClipState, ColorFilter, Error, Extent2D, Material, PassDescriptor,
    PassViewport, Result, Sampling, Scissor, Stop, TileMode, Vertex, MAX_STOPS, MORPHOLOGY_TAPS,
};
use emblema_renderer::{Paint as RenderPaint, Renderer, TOLERANCE};
use emblema_text::{Atlas, PositionedGlyph};
use glam::{Affine2, Mat2, Vec2};

/// A rectangle in user coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn from_size(width: f32, height: f32) -> Self {
        Self::new(0.0, 0.0, width, height)
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }

    /// The same rectangle, grown by `reach` on every side.
    ///
    /// What a blur needs from the bounds of what it blurs: the result spreads
    /// outward, so a target sized to the content alone cuts the halo off square
    /// at the edge -- the failure looking exactly like a shadow with a straight
    /// side. Written once because it was written out twice, four lines at a
    /// time, and a third place needs it with one side extended further.
    pub fn outset(self, reach: f32) -> Self {
        Self::new(
            self.left - reach,
            self.top - reach,
            self.right + reach,
            self.bottom + reach,
        )
    }

    fn to_path(self) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(self.left, self.top))
            .line_to(Vec2::new(self.right, self.top))
            .line_to(Vec2::new(self.right, self.bottom))
            .line_to(Vec2::new(self.left, self.bottom))
            .close();
        // A rectangle is a rounded one whose corners are not rounded, and
        // recording it as such is what lets a blurred rectangle be evaluated
        // rather than blurred. Not a special case in the expression that draws
        // it: a corner radius of zero is where that approximation is at its
        // most accurate, since there is no corner to approximate.
        b.as_rounded_rect(
            GeometryRect::new(
                Vec2::new(self.left, self.top),
                Vec2::new(self.right, self.bottom),
            ),
            0.0,
        );
        b.build()
    }

    /// This rectangle with its corners rounded to `radius`.
    ///
    /// The radius is clamped to half the shorter side. Larger is not an error
    /// worth refusing -- a caller asking for a fully rounded end usually says
    /// so by passing something enormous -- and left unclamped the corner arcs
    /// would overlap and the outline would cross itself, which fills as
    /// something nobody asked for. Clamped, the shape degenerates to a stadium
    /// and then a circle, which is what asking for a huge radius means.
    ///
    /// A radius at or below zero gives the plain rectangle, so a caller can
    /// pass a radius that happens to be zero without special-casing it.
    pub fn to_rounded_path(self, radius: f32) -> Path {
        if self.is_empty() {
            return Path::default();
        }
        // NaN is checked before clamping rather than after, because `f32::min`
        // ignores a NaN operand: clamping first turns it into half the shorter
        // side, so arithmetic that went wrong would come back as a large
        // rounded rectangle instead of as the plain one asked for.
        //
        // An infinite radius is not rejected with it. It clamps like any other
        // large number, because a caller who writes one means the roundest
        // shape available rather than none at all, and answering with a square
        // would be the opposite of what was asked.
        if radius.is_nan() || radius <= 0.0 {
            return self.to_path();
        }
        let mut path = PathBuilder::new();
        self.add_rounded_contour(&mut path, radius);
        // Recorded beside the drawing rather than inferred later, so a route
        // that wants the shape rather than the outline can have it -- a mask
        // blur on this is evaluated in the fragment stage and needs a rect and
        // a radius, which eight cubics cannot give back.
        path.as_rounded_rect(
            GeometryRect::new(
                Vec2::new(self.left, self.top),
                Vec2::new(self.right, self.bottom),
            ),
            radius.min(self.width() / 2.0).min(self.height() / 2.0),
        );
        path.build()
    }

    /// This rectangle with each corner rounded to its own pair of radii.
    ///
    /// The eight numbers `dart:ui`'s `RRect` carries, in upstream's order:
    /// top-left, top-right, bottom-left, bottom-right, each an x radius and a
    /// y radius. [`Self::to_rounded_path`] is this with all four the same and
    /// circular, and goes through the same code.
    pub fn to_rounded_path_with_radii(self, radii: RoundingRadii) -> Path {
        if self.is_empty() {
            return Path::default();
        }
        let radii = self.fitted_radii(radii);
        if radii.iter().flatten().all(|v| *v <= 0.0) {
            return self.to_path();
        }
        let mut path = PathBuilder::new();
        self.add_rounded_contour_with_radii(&mut path, radii);
        path.build()
    }

    /// Add this rectangle's rounded outline to a path already being built.
    ///
    /// [`Self::to_rounded_path_with_radii`] is this and nothing else when the
    /// path holds one contour. It is public for the case that one cannot
    /// serve: a shape made of two of these -- the ring `drawDRRect` fills, or a
    /// caller's own compound outline -- needs both contours in one path, and a
    /// path built from two paths is not something this crate offers.
    ///
    /// The radii are fitted on the way in, so a
    /// caller passing radii that overrun a side gets the same scaling
    /// `drawRRect` would apply rather than an outline that crosses itself.
    pub fn add_rounded_outline(self, path: &mut PathBuilder, radii: RoundingRadii) {
        if self.is_empty() {
            return;
        }
        let radii = self.fitted_radii(radii);
        self.add_rounded_contour_with_radii(path, radii);
    }

    /// The radii this rectangle can actually carry, made finite and made to fit.
    ///
    /// The fitting is `dart:ui`'s rule: find the edge whose two radii overrun
    /// it worst, and scale *every* radius by that one ratio. Scaling only the
    /// offending pair would also keep the outline from crossing itself, and
    /// would change the shape's proportions -- the difference between a rounded
    /// rectangle whose corners all got smaller and one that came back with a
    /// lopsided pair nobody asked for.
    ///
    /// It is applied to the radii as given, which is the part that has to be
    /// resisted tidying. Holding each radius to the side it runs along first
    /// looks like a harmless guard and is not: it changes the ratios the rule
    /// then works from, so a circular radius larger than the rectangle comes
    /// back elliptical -- half the width across and half the height down,
    /// instead of the round end the caller asked for.
    ///
    /// Which leaves the two values the rule cannot divide by. NaN and anything
    /// at or below zero become zero, so a corner whose arithmetic went wrong is
    /// square rather than enormous. An infinity becomes the longer side, which
    /// is not arbitrary: once a uniform radius is large enough to bind, the
    /// result stops depending on how large it was -- the ratio shrinks exactly
    /// as fast as the radius grows -- so any sufficiently large stand-in gives
    /// the same shape a huge finite radius gives, which is what an infinite one
    /// has always meant here.
    fn fitted_radii(self, radii: RoundingRadii) -> RoundingRadii {
        let (w, h) = (self.width(), self.height());
        let mut r = radii;
        for corner in r.iter_mut() {
            for value in corner.iter_mut() {
                *value = if value.is_nan() || *value <= 0.0 {
                    0.0
                } else if value.is_infinite() {
                    w.max(h)
                } else {
                    *value
                };
            }
        }
        let [tl, tr, bl, br] = r;
        // Each edge against the two radii that meet along it. A sum of zero
        // cannot bind, and is skipped rather than divided by.
        let mut scale = 1.0f32;
        for (side, sum) in [
            (w, tl[0] + tr[0]),
            (h, tr[1] + br[1]),
            (w, bl[0] + br[0]),
            (h, tl[1] + bl[1]),
        ] {
            if sum > side {
                scale = scale.min(side / sum);
            }
        }
        if scale < 1.0 {
            for corner in r.iter_mut() {
                for value in corner.iter_mut() {
                    *value *= scale;
                }
            }
        }
        r
    }

    /// Append this rectangle's rounded outline to a builder, as one contour.
    ///
    /// Separate from [`Self::to_rounded_path`] because a shape made of two of
    /// these -- a ring between an outer rectangle and an inner one -- needs
    /// both in one path, and a path built from two paths is not something this
    /// crate offers.
    fn add_rounded_contour(self, path: &mut PathBuilder, radius: f32) {
        let radii = self.fitted_radii([[radius; 2]; 4]);
        self.add_rounded_contour_with_radii(path, radii);
    }

    /// The same, for radii that have already been fitted by `fitted_radii`.
    fn add_rounded_contour_with_radii(self, path: &mut PathBuilder, radii: RoundingRadii) {
        let [tl, tr, bl, br] = radii;
        // The same constant that makes four cubics a circle. A corner here is a
        // quarter *ellipse* rather than a quarter circle, and the constant
        // carries over unchanged: an ellipse is a circle scaled along each axis
        // independently, and scaling a cubic's control points by the same
        // factors scales the curve they describe.
        let k = |v: f32| KAPPA * v;
        let (l, t, r, b) = (self.left, self.top, self.right, self.bottom);
        path.move_to(Vec2::new(l + tl[0], t))
            .line_to(Vec2::new(r - tr[0], t))
            .cubic_to(
                Vec2::new(r - tr[0] + k(tr[0]), t),
                Vec2::new(r, t + tr[1] - k(tr[1])),
                Vec2::new(r, t + tr[1]),
            )
            .line_to(Vec2::new(r, b - br[1]))
            .cubic_to(
                Vec2::new(r, b - br[1] + k(br[1])),
                Vec2::new(r - br[0] + k(br[0]), b),
                Vec2::new(r - br[0], b),
            )
            .line_to(Vec2::new(l + bl[0], b))
            .cubic_to(
                Vec2::new(l + bl[0] - k(bl[0]), b),
                Vec2::new(l, b - bl[1] + k(bl[1])),
                Vec2::new(l, b - bl[1]),
            )
            .line_to(Vec2::new(l, t + tl[1]))
            .cubic_to(
                Vec2::new(l, t + tl[1] - k(tl[1])),
                Vec2::new(l + tl[0] - k(tl[0]), t),
                Vec2::new(l + tl[0], t),
            )
            .close();
    }
}

/// The single radius these corners all are, if they are all the same one.
///
/// `None` for anything the fragment-evaluated route cannot describe: a corner
/// that differs from another, or one whose two radii differ from each other.
fn uniform_circular_radius(radii: RoundingRadii) -> Option<f32> {
    let radius = radii[0][0];
    radii
        .iter()
        .flatten()
        .all(|v| *v == radius)
        .then_some(radius)
}

/// A rounded rectangle's four corners, each with an x and a y radius.
///
/// In upstream's order -- top-left, top-right, bottom-left, bottom-right --
/// rather than in the order the outline is traced, so that a caller porting
/// from `RoundingRadii` or from `dart:ui`'s `RRect` can copy the four across
/// without reordering them. The tracing order is the contour's business.
pub type RoundingRadii = [[f32; 2]; 4];

/// The outline of a rounded superellipse in `rect` with the given radii.
///
/// Here rather than on `Rect` beside `to_rounded_path_with_radii`, because a
/// superellipse is not a rounded rectangle with a different corner and putting
/// it there would suggest the two are interchangeable. They take the same four
/// numbers and draw measurably different shapes.
fn rsuperellipse_path(rect: Rect, radii: RoundingRadii) -> Path {
    use emblema_geometry::superellipse::{CornerRadii, RoundSuperellipse};
    let bounds = GeometryRect::new(
        Vec2::new(rect.left, rect.top),
        Vec2::new(rect.right, rect.bottom),
    );
    RoundSuperellipse::new(
        bounds,
        CornerRadii {
            top_left: Vec2::new(radii[0][0], radii[0][1]),
            top_right: Vec2::new(radii[1][0], radii[1][1]),
            bottom_left: Vec2::new(radii[2][0], radii[2][1]),
            bottom_right: Vec2::new(radii[3][0], radii[3][1]),
        },
    )
    .to_path()
}

/// Where a pass's texture slot gets its content.
///
/// A recording is produced without touching a device, so it can name neither a
/// texture nor an allocation. It names either an image the caller will supply
/// or an earlier pass in the same recording, and whoever executes it resolves
/// both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSource {
    /// The caller's image at this index in the table given at draw time.
    Image(u32),
    /// The output of the pass at this index in [`Recording::passes`].
    Layer(usize),
    /// The baked gradient at this index in [`Recording::ramps`].
    ///
    /// Unlike the other two this names something the recorder made rather than
    /// something it was given or rendered, so it is uploaded at submission and
    /// discarded with the frame.
    Ramp(usize),
}

/// One target's worth of drawing.
///
/// Cloneable so that a finished recording can be drawn into another one: the
/// passes are taken as they are, and the recording being drawn keeps its own
/// copy for a caller who draws it more than once.
#[derive(Clone)]
pub struct Pass {
    pub batch: Batch,
    pub descriptor: PassDescriptor,
    /// What each texture slot this pass samples refers to, in slot order.
    pub sources: Vec<TextureSource>,
    /// The size of the target this pass renders into.
    ///
    /// Per pass rather than per recording, because a layer given bounds
    /// renders into a target the size of those bounds. The root pass always
    /// carries the recording's extent, since that is the caller's surface.
    pub extent: Extent2D,
}

/// How a run of points is joined up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointMode {
    /// Each point on its own, drawn as the stroke's cap.
    #[default]
    Points,
    /// Each pair a segment, and an odd point at the end drawn as nothing.
    Lines,
    /// One open run through all of them.
    Polygon,
}

/// A finished recording, ready to submit.
///
/// More than one pass where the recording used layers. A layer is drawn into a
/// target of its own and then composited back, which cannot happen in the same
/// pass that samples it: reading an attachment being written needs machinery
/// this does not have, and the separation is what makes group opacity mean
/// "make this subtree, then fade it" rather than "fade each shape in it".
pub struct Recording {
    /// Layers first, each before whatever samples it; the root target last.
    ///
    /// The order falls out of how they are recorded rather than being sorted:
    /// a layer is finished when it is restored, which is necessarily before the
    /// draw that composites it.
    pub passes: Vec<Pass>,
    /// The size of the caller's surface, which is what the root pass renders
    /// at. A layer pass may be smaller; see [`Pass::extent`].
    pub extent: Extent2D,
    /// Gradients whose colors were too many to carry in a material, tabulated.
    ///
    /// Recorded here rather than per pass because a slot table is per pass and
    /// this is not: the same gradient drawn into a layer and again over it is
    /// one ramp named twice.
    pub ramps: Vec<Ramp>,
}

impl Recording {
    pub fn draw_count(&self) -> usize {
        self.passes.iter().map(|p| p.batch.draw_count()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.passes.iter().all(|p| p.batch.is_empty())
    }

    /// The pass drawn into the caller's surface.
    pub fn root(&self) -> &Pass {
        self.passes.last().expect("a recording always has a root")
    }

    /// How many offscreen targets executing this needs.
    pub fn layer_count(&self) -> usize {
        self.passes.len() - 1
    }
}

/// The target a pass renders into, placed within the frame.
///
/// A layer with bounds renders into a target the size of those bounds rather
/// than the size of the frame, which is most of the point of giving bounds: a
/// layer covering a tenth of the frame costs a tenth of the memory and a tenth
/// of the fill. Everything that turns a device position into a clip position
/// has to know where that target sits, which is what `origin` carries.
#[derive(Debug, Clone, Copy)]
struct Target {
    /// Where this target's top-left corner sits in the frame, in device pixels.
    /// Whole pixels, so that the composite samples texel centers exactly.
    origin: Vec2,
    extent: Extent2D,
}

impl Target {
    fn frame(extent: Extent2D) -> Self {
        Self {
            origin: Vec2::ZERO,
            extent,
        }
    }

    /// Device position to clip position for this target.
    ///
    /// Affine, and stays so: a viewport is a scale, a flip and an offset. It is
    /// lifted here because everything it composes with may not be.
    fn projection(&self) -> Transform2D {
        (viewport_projection(self.extent.width, self.extent.height)
            * Affine2::from_translation(-self.origin))
        .into()
    }

    /// The whole target, in the device space the projection expects.
    fn path(&self) -> Path {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + self.extent.width as f32,
            self.origin.y + self.extent.height as f32,
        )
        .to_path()
    }
}

/// One stencil clip in force, kept so it can be made again.
///
/// A stencil clip is a *draw*: it steps the stencil forward where the shape
/// covers, and every content draw after it tests against what that draw left.
/// A pass cut starts a new pass, whose stencil attachment begins cleared -- so
/// unless the narrowing draws are made again, every draw after the cut tests a
/// buffer nothing wrote and lands nowhere.
///
/// Kept as the shape rather than as the draw, because the shape is what
/// `clip_path` needs to make one and the draw lives in a batch that the cut has
/// already taken away.
#[derive(Debug, Clone)]
struct StencilClip {
    path: Path,
    /// The transform the shape was stated in. `clip_out_rect` builds its
    /// contours in device pixels and passes the identity; `clip_path` passes
    /// whatever was in force.
    transform: Transform2D,
    /// The scissor in force when it was made, which narrows the narrowing.
    scissor: Option<Scissor>,
}

/// Transform and clip, saved together.
///
/// One stack rather than two: a `save` and its `restore` bracket a subtree, and
/// letting the two pieces of state unwind independently would mean a caller
/// could balance one while leaving the other adrift.
#[derive(Debug)]
struct SavedState {
    transform: Transform2D,
    clip: Option<Scissor>,
    clip_bounds: Rect,
    depth: u32,
    /// How many stencil clips were in force, so `restore` can drop the ones
    /// this save added. Redundant with `depth` and kept beside it for the same
    /// reason the bounds are kept beside the scissor: one of them is the truth
    /// the device holds and the other is what would be needed to rebuild it.
    clips: usize,
    /// Set where this save opened a layer, holding what the layer displaced.
    layer: Option<LayerFrame>,
}

/// A layer in progress: the parent's recording, set aside until it returns.
#[derive(Debug)]
struct LayerFrame {
    batch: Batch,
    sources: Vec<TextureSource>,
    /// The stencil clips in force on the parent, set aside with its batch.
    ///
    /// A layer starts unclipped -- the composite that returns it is subject to
    /// the parent's clip instead -- so it needs a list of its own, and the
    /// parent's has to come back when it does. Held here rather than as a
    /// length in `SavedState`, because unlike every other save this one does
    /// not *extend* the list it found.
    clips: Vec<StencilClip>,
    paint: Layer,
    /// An image filter to run over the finished layer, if one was asked for.
    ///
    /// Beside the layer rather than inside it, because [`Layer`] is `Copy` and
    /// small and is copied at every `save_layer`, while a filter can hold a
    /// composition or a program's sixty-four uniform floats. The fields on the
    /// layer cover the four kinds that fit in one; this covers the rest, and
    /// the two orders a composition can take.
    filter: Option<ImageFilter>,
    /// The target the parent was drawing into, restored when the layer closes.
    parent: Target,
    /// Whether the parent's own batch had asked for antialiasing yet.
    ///
    /// Displaced with the batch and for the same reason: the sample count
    /// describes a pass, and a layer's draws belong to the layer's pass. A
    /// parent that has drawn nothing needing multisampling does not start
    /// needing it because something inside a layer did.
    anti_alias: bool,
    /// Whether the parent's batch began by redrawing the whole target.
    ///
    /// Displaced with the batch, like `anti_alias` and for the same reason: a
    /// layer's own batch starts empty and is seeded by nothing, while the parent's
    /// may have been cut before the layer opened.
    seeded_by_redraw: bool,
    /// The directions this layer's blur runs along.
    ///
    /// Taken from the transform in force when the layer opened, for the same
    /// reason its sigma is: the transform that decides them may be gone by the
    /// time the layer is composited, and both halves of a blur -- how far it
    /// reaches and which way -- have to be settled together or a target is
    /// sized for one blur and filled by another.
    blur_basis: BlurBasis,
}

/// The two directions a blur runs along, in device pixels.
///
/// A blur is two separable passes, and which directions they take is the whole
/// of the difference between a blur that turns with the caller's transform and
/// one that does not. `dart:ui` states a deviation per axis in the space the
/// caller was drawing in; if that space is turned relative to the target, the
/// passes have to turn with it or a horizontal smear stays horizontal on
/// screen while the thing it is smearing rotates underneath.
///
/// **This is not how upstream does it, and the difference is worth stating.**
/// `GaussianBlurFilterContents` removes the rotation instead: it re-renders its
/// input into what its own comment calls "un-rotated local space", scaled by
/// the transform but not turned by it, blurs along that space's own axes, and
/// applies the rotation to the *result*. Its reason is quality rather than
/// correctness -- the comment says the un-rotated space "is a requirement for
/// text to be rendered correctly", because taps landing on texel centers is
/// what keeps a glyph sharp.
///
/// That arrangement is not available here. A layer is a recorded pass with a
/// device-space target, a device-space scissor and a stencil buffer to match,
/// so its content cannot be drawn into a space of its own choosing after the
/// fact. What *is* available is the pass's `step`, which was already a free
/// two-vector rather than an axis flag -- the shader walks its taps along
/// whatever direction it is given. So the passes turn instead of the space.
///
/// The two are the same Gaussian. A blur with deviations along orthogonal
/// directions is separable along exactly those directions, so the picture is
/// upstream's; what differs is that a tap here lands between texels and is
/// resolved by the sampler, which costs a little sharpness that upstream's
/// arrangement does not.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BlurBasis {
    /// Unit vectors in device space, in the order the deviations are stated.
    directions: [Vec2; 2],
}

impl BlurBasis {
    /// The target's own axes, which is what every blur used before this and
    /// what a transform this cannot decompose still gets.
    const AXES: Self = Self {
        directions: [Vec2::X, Vec2::Y],
    };

    /// Where the caller's axes point once the transform has been applied.
    ///
    /// Refused rather than approximated in two cases. A transform with
    /// perspective has no single basis -- the directions would differ per
    /// fragment, which a pass walking a constant step cannot express. And a
    /// transform whose image axes are not perpendicular, which is a shear,
    /// leaves a Gaussian that two separable passes cannot state at all:
    /// separability is a property of orthogonal directions, and running the
    /// passes along oblique ones would not be a wrong blur so much as not a
    /// blur. Upstream is no better off here -- its `ExtractScale` takes the
    /// lengths of the image axes and drops the shear entirely.
    fn of(transform: Transform2D) -> Self {
        let Some(affine) = transform.to_affine() else {
            return Self::AXES;
        };
        let (x, y) = (affine.matrix2.x_axis, affine.matrix2.y_axis);
        let (lx, ly) = (x.length(), y.length());
        if !lx.is_finite() || !ly.is_finite() || lx <= 0.0 || ly <= 0.0 {
            return Self::AXES;
        }
        let directions = [x / lx, y / ly];
        // The cosine of the angle between them, both being unit vectors. A
        // thousandth is well inside what a rotation composed with a scale
        // produces and well outside anything a shear does.
        if directions[0].dot(directions[1]).abs() > 1e-3 {
            return Self::AXES;
        }
        Self { directions }
    }

    /// Whether these are the target's own axes, which several things below can
    /// take shortcuts for.
    fn is_axis_aligned(&self) -> bool {
        *self == Self::AXES
    }

    /// How far past its content a blur along these directions carries, as an
    /// outset on each of the target's axes.
    ///
    /// The bounding box of the two reaches laid along the two directions,
    /// which for the target's own axes is the pair of reaches unchanged.
    fn reach(&self, sigma: Vec2) -> Vec2 {
        self.box_of(Vec2::new(blur_reach(sigma.x), blur_reach(sigma.y)))
    }

    /// The target-axis bounding box of two distances laid along these directions.
    ///
    /// Shared with the morphology reach rather than written twice, because the
    /// two have to agree with the passes to the pixel: a pass walks `u` and `v`
    /// and whatever room is reserved has to contain where that walk ends. It was
    /// written twice for a while, and the morphology copy did not turn -- so a
    /// rotated dilation ran along the caller's axes and was clipped by an outset
    /// laid along the target's.
    fn box_of(&self, along: Vec2) -> Vec2 {
        let [u, v] = self.directions;
        Vec2::new(
            u.x.abs() * along.x + v.x.abs() * along.y,
            u.y.abs() * along.x + v.y.abs() * along.y,
        )
    }
}

/// Spreading or shrinking a finished layer, one axis at a time.
///
/// The pair `dart:ui` calls `ImageFilter.dilate` and `ImageFilter.erode`: each
/// output pixel takes the largest, or the smallest, of the input within a
/// rectangle around it. The rectangle is why the radii are per axis and why the
/// filter is separable at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Morphology {
    /// How far the structuring element reaches along each axis, in device
    /// pixels.
    pub radius: [f32; 2],
    /// Take the largest sample in reach rather than the smallest.
    pub dilate: bool,
}

impl Morphology {
    /// Spread the layer: each pixel becomes the largest within the radii.
    pub fn dilate(x: f32, y: f32) -> Self {
        Self {
            radius: Self::sane(x, y),
            dilate: true,
        }
    }

    /// Shrink the layer: each pixel becomes the smallest within the radii.
    pub fn erode(x: f32, y: f32) -> Self {
        Self {
            radius: Self::sane(x, y),
            dilate: false,
        }
    }

    /// Not negative, and finite.
    ///
    /// No longer rounded here, and the reason is the whole of the change that
    /// made the radius a local length. A structuring element is a set of sample
    /// positions and there is no half of one to keep -- but the positions are
    /// *device* texels, and this runs on the caller's number before any
    /// transform has been applied to it. Rounding a local length rounds the
    /// wrong quantity: at a scale of three, a radius of 1.5 rounded here becomes
    /// two and then six, where the sample positions it is naming are four and a
    /// half. `applied_radius` rounds instead, where the device radius is known.
    fn sane(x: f32, y: f32) -> [f32; 2] {
        let one = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
        [one(x), one(y)]
    }

    /// Whether this would leave every pixel where it was.
    pub fn is_identity(&self) -> bool {
        self.radius == [0.0, 0.0]
    }

    /// The radius worth applying on a target this size, per axis.
    ///
    /// A window reaching `extent` texels along an axis already spans the whole
    /// of it, so every output pixel is the extreme over the entire row or
    /// column and a wider one cannot change a single pixel. Dilating takes the
    /// largest sample and eroding the smallest; either way the taps a larger
    /// radius adds are all outside the image, and whatever the sampler returns
    /// there is already in the window. So this clamp is picture-preserving
    /// rather than a limit on the filter, which is what makes it safe to apply
    /// without the caller having asked.
    ///
    /// **Why it has to exist.** `sane` bounds the radius below and
    /// rejects what is not finite, and nothing bounded it above -- so the pass
    /// loop, which emits one pass per [`MORPHOLOGY_TAPS`] texels, turned a
    /// caller's float into that many passes. A radius of a million asked for
    /// sixty-two thousand of them and a radius of `1e20` exhausted memory,
    /// through the documented constructor and with no unsafe input anywhere:
    /// the value is finite, positive and whole. That is a recording's size made
    /// proportional to a scalar, which is the defect this closes.
    ///
    /// Non-finite gives zero, on the same terms as `sane` and for the same
    /// reason a non-finite sigma gives no blur. Checked before the `min` rather
    /// than after, because [`f32::min`] returns the other operand when one side
    /// is NaN -- the trap `Layer::with_blur_xy` and `Rect::outset` both name,
    /// and the one that would turn a NaN radius into a full-width dilation.
    /// It also has to be handled here rather than left to the loop: the loop's
    /// condition is false for NaN, so NaN was already harmless, but infinity
    /// entered it and never left, since subtracting the taps from infinity
    /// leaves infinity.
    pub fn applied_radius(&self, extent: Extent2D) -> [f32; 2] {
        let one = |radius: f32, along: u32| {
            if radius.is_finite() {
                // Rounded here rather than at construction, because this is the
                // first point at which the number is in device texels: the
                // caller states a local length and `Layer::scaled_by` converts
                // it. A structuring element is a set of sample positions and
                // there is no half of one, so the rounding still happens -- just
                // where the positions are.
                radius.max(0.0).min(along as f32).round()
            } else {
                0.0
            }
        };
        [
            one(self.radius[0], extent.width),
            one(self.radius[1], extent.height),
        ]
    }
}

/// What a mask blur is blurring: a shape, or a run of glyphs.
///
/// An enum rather than a closure because every style draws its content two or
/// three times and a closure taking `&mut Canvas` cannot be called twice while
/// the canvas is borrowed. Two variants is also the whole set: a mask blur
/// needs the content's coverage times one solid color, which a mesh's per-vertex
/// colors rule out and an image's texels rule out.
#[derive(Clone, Copy)]
enum Masked<'a> {
    Path(&'a Path),
    /// A mesh carrying no per-vertex colors, so the paint supplies the color
    /// the way it does for a path. One that carries them is refused before it
    /// reaches here: see `draw_vertices`.
    Mesh(&'a Vertices),
    Glyphs {
        glyphs: &'a [PositionedGlyph],
        atlas: &'a Atlas,
        slot: u32,
    },
}

/// How a layer is composited back onto what was underneath it.
///
/// A separate type from [`Paint`] rather than a reuse of it, because only two
/// of a paint's parts mean anything here — there is no shape to fill and no
/// geometry to stroke — and a caller handed a `Paint` would reasonably expect
/// its shader to matter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    /// Blur the group before compositing it, with this standard deviation per
    /// axis in device pixels.
    ///
    /// Zero on both for no blur, which is the default and costs nothing: the
    /// extra passes are only recorded where a caller asked for one, and an
    /// axis whose deviation is zero costs no pass of its own.
    ///
    /// Two deviations rather than one because `dart:ui`'s `ImageFilter.blur`
    /// states `sigmaX` and `sigmaY` separately, and because the blur is two
    /// separable passes here regardless -- so the second is a field rather
    /// than a mechanism. [`Self::with_blur`] sets both.
    ///
    /// On the group rather than on a paint, because a blur is a function of a
    /// finished image and a paint describes one shape. Blurring each shape and
    /// compositing the results is a different picture from blurring the
    /// composite, and the second is what a shadow or a frosted panel means.
    pub blur: Vec2,
    /// Scales the whole layer on the way back. This is what makes group opacity
    /// differ from per-shape opacity: two overlapping half-transparent shapes
    /// in a layer show one blended edge, where the same shapes drawn directly
    /// show where they cross.
    pub alpha: f32,
    /// How the finished layer meets what was underneath it.
    pub blend: BlendMode,
    /// Transform the finished layer on the way back, in device pixels.
    ///
    /// Distinct from transforming what goes into it, which the canvas's own
    /// transform already does. This resamples the finished image: magnifying
    /// through it gives the layer's own pixels enlarged, where drawing the
    /// same shapes under a larger transform would give them redrawn at the
    /// larger size. That difference is the whole of what a matrix image filter
    /// means -- a caller wanting the sharp one already has the transform
    /// stack.
    ///
    /// It resamples what the layer *captured*, and a layer captures no more
    /// than its target holds. So a matrix moves an image and cannot recover
    /// what fell outside the layer to begin with: a shape drawn past the edge
    /// of the frame is gone before this runs, where upstream sizes the layer
    /// through the filter and brings it back. `docs/non-parity.md` §12 has the
    /// case and what fixing it would cost.
    ///
    /// `None` composites the layer where it was drawn, which is what every
    /// layer did before this existed.
    pub matrix: Option<Transform2D>,
    /// Blur what is already on the target before the layer draws over it.
    ///
    /// This is the other blur, and the difference is which image is filtered.
    /// [`Self::blur`] softens the layer's own content, which is what a shadow
    /// is. This softens what is *behind* it and hands the result to the layer
    /// as its starting content, which is what frosted glass is: a panel that
    /// obscures rather than one that is itself indistinct.
    ///
    /// Zero for none, and it costs nothing when zero. When it is not zero it
    /// costs a copy of the target and two blur passes over it, because reading
    /// what a pass is writing is not something this renderer can do -- see the
    /// architecture note on cutting a pass.
    ///
    /// # Bounds stop being an optimization
    ///
    /// For every other layer, [`Canvas::save_layer_bounds`] only says where the
    /// content is, and the picture is the same either way. For this one the
    /// bounds are the region that gets filtered: a frosted panel is a bounded
    /// layer, and the same layer without bounds blurs the whole frame and
    /// composites the whole frame back. Both are meaningful and they are
    /// different pictures, so state the bounds when you mean a panel.
    pub backdrop_blur: f32,
    /// Which captured backdrop this layer filters, if it shares one.
    ///
    /// `dart:ui`'s `pushBackdropFilter` takes a `backdropId` and says what it
    /// means: "when the first backdrop filter with a given id is processed
    /// during rasterization, the state of the backdrop is recorded and cached.
    /// All subsequent backdrop filters with the same identifier will apply
    /// their filter to the cached backdrop." So it is a different picture, not
    /// only a faster one -- overlapping panels sharing an id each filter what
    /// was behind all of them, rather than each filtering the one before it.
    ///
    /// That is the case it exists for. A list of frosted rows over one
    /// background wants each row to show the background, and without the id
    /// the second row shows the first row's blur blurred again. With it, the
    /// capture happens once and each row filters it: the picture the caller
    /// meant, and one pass cut instead of one per row.
    ///
    /// An id keys a captured image and nothing more. A layer naming one from
    /// inside another layer still filters what the id captured rather than its
    /// own target's content, which is upstream's reading and is the point of
    /// the case it tests with each panel wrapped in a save layer of its own.
    ///
    /// `None` captures afresh, which is what every layer did before this
    /// existed and is right whenever the backdrops genuinely differ.
    pub backdrop_id: Option<i64>,
    /// Spread or shrink the group before compositing it. See [`Morphology`].
    ///
    /// On the group for the same reason [`Self::blur`] is: a maximum over a
    /// window is a function of a finished image. Dilating each shape and then
    /// compositing is a different picture from dilating the composite wherever
    /// two shapes overlap.
    ///
    /// `None` for no morphology, which is the default and costs nothing.
    pub morphology: Option<Morphology>,
    /// Recolor the finished group on its way back.
    ///
    /// What `dart:ui` means by the `colorFilter` on the paint `saveLayer`
    /// takes, and the same distinction group opacity has: filtering each shape
    /// and compositing the results differs from filtering the composite
    /// wherever two of them overlap, because a filter is not linear in general.
    ///
    /// It is also the only way to filter what a caller's own fragment program
    /// drew. A runtime effect is a whole pipeline rather than a material this
    /// renderer's shader evaluates, so there is nowhere in it to apply a
    /// matrix -- the filter has to act on the image the program produced.
    pub color_filter: ColorFilter,
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            blur: Vec2::ZERO,
            alpha: 1.0,
            blend: BlendMode::SrcOver,
            matrix: None,
            backdrop_blur: 0.0,
            backdrop_id: None,
            morphology: None,
            color_filter: ColorFilter::None,
        }
    }
}

impl Layer {
    /// Transform the finished layer on the way back. See [`Self::matrix`].
    ///
    /// Takes an affine or a projective transform, the same as `concat`. A layer
    /// moved by one of these is a finished image being placed, so perspective
    /// here gives the image seen at an angle rather than the content redrawn at
    /// one -- which is the whole distinction this filter exists to make, and it
    /// survives the widening unchanged.
    pub fn with_matrix(mut self, matrix: impl Into<Transform2D>) -> Self {
        self.matrix = Some(matrix.into());
        self
    }

    pub fn opacity(alpha: f32) -> Self {
        Self {
            alpha,
            ..Self::default()
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    /// Recolor the finished group on its way back. See [`Self::color_filter`].
    pub fn with_color_filter(mut self, filter: ColorFilter) -> Self {
        self.color_filter = filter;
        self
    }

    /// Spread or shrink the finished group. See [`Morphology`].
    ///
    /// A morphology that would move nothing becomes `None`, on the same
    /// reasoning as a blur of zero: a caller animating a radius down to nothing
    /// should get the unfiltered thing at the end, not two passes that copy the
    /// image twice to say so.
    pub fn with_morphology(mut self, morphology: Morphology) -> Self {
        self.morphology = if morphology.is_identity() {
            None
        } else {
            Some(morphology)
        };
        self
    }

    /// How far past the content this layer's filters reach, per axis, in device
    /// pixels.
    ///
    /// A blur carries color outward and so does a dilation; an erosion only
    /// eats inward and needs no room. The two can be asked for together, so
    /// this is their sum rather than whichever is larger -- the passes run one
    /// after the other, and the second reaches out from where the first put
    /// things.
    /// The same layer with its lengths converted into device pixels.
    ///
    /// A sigma reaches the caller in the space they were drawing in and every
    /// pass that acts on it works on a target, so one of the two has to move.
    /// This is that move, and it happens once at `save_layer` rather than at
    /// each use -- the transform that decides it is the one in force when the
    /// layer is opened, and it may be gone by the time the layer is composited.
    ///
    /// **Every length stated per axis takes the axis factors, and the one stated
    /// as a single number is converted elsewhere.** A blur's deviations and a
    /// morphology's radii are pairs, and each component takes the length of its
    /// own transformed basis vector -- which is what upstream's `ExtractScale`
    /// and `TransformDirection` take.
    ///
    /// `backdrop_blur` is deliberately *not* converted here, and the reason is
    /// that it cannot be. It is one number, so a caller states a blur that is
    /// round in their own space -- and a round blur under an anisotropic scale is
    /// an oval in device space, which one number cannot hold. Multiplying it by a
    /// single factor here is what used to happen and is what made a frosted panel
    /// stay round under `scale(2, 3)` while the same deviations on the layer
    /// itself stretched. So the field stays in the caller's space and
    /// `open_layer` converts it per axis at the point it becomes an
    /// `ImageFilter::Blur`, which has two components to put the answer in.
    ///
    /// Nothing between here and there reads it as a length: the two other uses
    /// are `> 0.0` tests asking whether a backdrop is wanted at all.
    fn scaled_by(self, scale: f32, axes: Vec2) -> Self {
        if !scale.is_finite() || scale <= 0.0 {
            return self;
        }
        Self {
            blur: self.blur * axes,
            backdrop_blur: self.backdrop_blur,
            // A morphology radius is a length in the caller's space too, and was
            // the one filter left out of this conversion -- on the stated
            // grounds that upstream had no morphology to be in parity with,
            // which was wrong. See `non-parity.md` 17.
            morphology: self.morphology.map(|m| Morphology {
                radius: [m.radius[0] * axes.x, m.radius[1] * axes.y],
                ..m
            }),
            ..self
        }
    }

    fn reach(&self, basis: BlurBasis) -> Vec2 {
        let blur = basis.reach(self.blur);
        match self.morphology {
            // Turned with the same basis the passes walk. An axis-aligned outset
            // would be the right box only while the directions are the axes, and
            // a rotated dilation would lose its corners to the scissor.
            Some(m) if m.dilate => blur + basis.box_of(Vec2::new(m.radius[0], m.radius[1])),
            _ => blur,
        }
    }

    /// Blur the finished group, with `sigma` in device pixels.
    ///
    /// Anything at or below zero, or not a number, means no blur rather than an
    /// error: a caller animating a shadow's softness to nothing should get the
    /// sharp thing, not a refusal at the end of the animation.
    /// Blur the backdrop this layer is drawn over.
    ///
    /// Ignored unless finite and positive, on the same terms as
    /// [`Self::with_blur`]: a sigma that is not a length describes no blur.
    pub fn with_backdrop_blur(mut self, sigma: f32) -> Self {
        self.backdrop_blur = if sigma.is_finite() && sigma > 0.0 {
            sigma
        } else {
            0.0
        };
        self
    }

    /// Share one captured backdrop with every layer naming the same `id`.
    ///
    /// See [`Self::backdrop_id`] for what is shared and why it changes the
    /// picture rather than only the cost. The id is the caller's, and means
    /// nothing beyond identity: any two values that differ keep their
    /// backdrops apart.
    ///
    /// The id outlives the target it captured. A layer naming one from inside
    /// another layer is seeded with the captured image placed where that
    /// layer's target sits within it, which is the same arithmetic a bounded
    /// layer is already seeded by.
    pub fn with_backdrop_id(mut self, id: i64) -> Self {
        self.backdrop_id = Some(id);
        self
    }

    pub fn with_blur(self, sigma: f32) -> Self {
        self.with_blur_xy(sigma, sigma)
    }

    /// Blur the finished group by a deviation per axis, in device pixels.
    ///
    /// Two, because `dart:ui`'s `ImageFilter.blur` states `sigmaX` and
    /// `sigmaY` separately. A blur here is two separable passes already, one
    /// along each axis, so each simply uses its own -- and a pass whose
    /// deviation is zero is skipped rather than run, which is what keeps a
    /// blur along one axis from resampling the other for nothing.
    pub fn with_blur_xy(mut self, sigma_x: f32, sigma_y: f32) -> Self {
        // Clamped to upstream's `kMaxSigma`, and clamped *after* the check
        // rather than before it: `f32::min` returns the other operand when one
        // is NaN, so clamping first turns a NaN into five hundred and asks for
        // the widest blur there is. The same trap is named at `Rect::outset`,
        // which is where this was learned the first time.
        let one = |sigma: f32| {
            if sigma.is_finite() && sigma > 0.0 {
                sigma.min(MAX_SIGMA)
            } else {
                0.0
            }
        };
        self.blur = Vec2::new(one(sigma_x), one(sigma_y));
        self
    }
}

/// Records drawing commands for one frame.
pub struct Canvas {
    renderer: Renderer,
    batch: Batch,
    transform: Transform2D,
    /// The region drawing is confined to, or `None` for the whole target.
    ///
    /// Kept in device pixels rather than user space because that is what it
    /// means: a clip is fixed at the moment it is applied, and a later
    /// transform moves the shapes drawn inside it without moving the clip.
    clip: Option<Scissor>,
    /// A rectangle around everything the clip still admits, in device pixels.
    ///
    /// Kept beside the scissor rather than derived from it, because a path
    /// clip narrows what may be drawn without narrowing the scissor at all --
    /// it goes to the stencil instead. A caller asking what is still reachable
    /// wants both accounted for, and only this has seen both.
    clip_bounds: Rect,
    /// How many clips of a shape a scissor cannot express are in force.
    ///
    /// Content draws where the stencil holds this, which is true only where
    /// every one of those clips admitted the pixel.
    depth: u32,
    /// Those clips, in the order they were built. See [`StencilClip`].
    clips: Vec<StencilClip>,
    stack: Vec<SavedState>,
    /// What the batch being recorded samples, in slot order.
    sources: Vec<TextureSource>,
    /// Layers already finished, in the order they were restored.
    ///
    /// A layer is restored before anything can composite it, so this order is
    /// already an order the passes can be executed in.
    finished: Vec<Pass>,
    extent: Extent2D,
    /// The target being recorded into: the frame, or a bounded layer's.
    target: Target,
    background: Option<Color>,
    /// Set once anything asks for antialiasing.
    ///
    /// Multisampling is a property of the pass, so it cannot vary per shape.
    /// Turning it on for the whole frame when any shape wants it is the
    /// behavior that surprises least; the alternative silently ignores the
    /// request on some shapes.
    anti_alias: bool,
    /// Whether the batch now open begins by redrawing the whole target.
    ///
    /// It does when a pass was cut: what follows the cut is a full-target `Src`
    /// blit of the pass just finished, unclipped and unstencilled, so every pixel
    /// is written before anything else in the batch. A pass like that can be given
    /// a clear color it will never show -- and a clear is what a multisampled pass
    /// requires here, the alternative being a resolved-to-multisample load that
    /// this technique has no reverse of.
    ///
    /// Tracked rather than inferred from `background` being absent, because a
    /// caller that never set a background is compositing onto whatever the surface
    /// already holds, and clearing *that* pass would erase it.
    seeded_by_redraw: bool,
    samples: u32,
    /// Gradients tabulated because their stops did not fit in a material.
    ///
    /// Across the whole recording rather than per pass, so the same gradient
    /// drawn into a layer and again over it is baked once.
    ramps: Vec<Ramp>,
    /// Backdrops captured once and filtered by every layer naming the same id.
    ///
    /// A `Canvas` records one frame and `finish` consumes it, so this lives
    /// exactly as long as the ids are meaningful and there is nothing to
    /// clear. Upstream's `Canvas` is reused across frames and empties its
    /// `backdrop_data_` at the end of each one.
    backdrops: std::collections::HashMap<i64, BackdropShare>,
}

/// A backdrop ready to be laid under a layer's own content.
///
/// Three things rather than a pass index, because a seed is a draw and a draw
/// needs a mapping: which pass holds the filtered image, which target that pass
/// is an image *of* -- they differ when a bounded layer seeds from its parent
/// -- and the matrix the filter asked for, which is the one filter that moves
/// the image rather than recomputing it where it stands.
#[derive(Clone, Copy)]
struct Backdrop {
    pass: usize,
    of: Target,
    /// `None` for every filter but a matrix, which is what it means for the
    /// image to be read where it was written.
    moved_by: Option<Affine2>,
}

/// One captured backdrop, and what has already been made of it.
///
/// The capture is a pass index, which is all a backdrop needs to be here: a
/// pass is the unit a later pass samples, so sharing one is sharing the index
/// rather than managing a texture. Upstream carries a `std::shared_ptr<Texture>`
/// in the same slot, and a second one for the filtered result, because its
/// filters produce snapshots rather than passes.
struct BackdropShare {
    /// The target the capture is an image of, which need not be the one a
    /// later layer naming the id is drawn into.
    ///
    /// An id keys a captured image and nothing else, so a layer opened inside
    /// some other layer still filters what the id captured. That is upstream's
    /// reading and it is exercised there:
    /// `CanRenderMultipleBackdropBlurWithSingleBackdropIdDifferentLayers` wraps
    /// every panel but the first in a save layer of its own and expects all six
    /// to show the same ground. Seeding needs no case for it -- a filtered pass
    /// is drawn into a rectangle *of* its source, which is already how a
    /// bounded layer is seeded from a full-target capture.
    captured_from: Target,
    /// The pass holding the backdrop as it was when the id was first used.
    captured: usize,
    /// Filters already run over `captured`, and where each landed.
    ///
    /// A filter is a function of its input, so the same filter over the same
    /// capture is the same image and there is no reason to compute it twice.
    /// This is upstream's `all_filters_equal` and `shared_filter_snapshot`,
    /// with the restriction lifted: upstream shares one snapshot only when
    /// *every* filter on the id is equal, which a list of rows that blur by
    /// different amounts fails outright. Keyed per filter, two amounts share
    /// what they have in common -- the capture -- and each still gets its own.
    ///
    /// A list rather than a map because a shared backdrop has a handful of
    /// distinct filters at most, and `ImageFilter` holds a program's uniforms.
    filtered: Vec<(ImageFilter, usize)>,
}

impl Canvas {
    /// Start recording for a target of the given size.
    pub fn new(extent: Extent2D) -> Self {
        let mut renderer = Renderer::new();
        renderer.begin_frame(extent, TOLERANCE);
        Self {
            renderer,
            batch: Batch::new(),
            transform: Transform2D::IDENTITY,
            clip: None,
            // The whole target to start with, which is what an unclipped
            // canvas may reach.
            clip_bounds: Rect::new(0.0, 0.0, extent.width as f32, extent.height as f32),
            depth: 0,
            clips: Vec::new(),
            stack: Vec::new(),
            sources: Vec::new(),
            finished: Vec::new(),
            extent,
            target: Target::frame(extent),
            background: None,
            anti_alias: false,
            seeded_by_redraw: false,
            samples: 4,
            ramps: Vec::new(),
            backdrops: std::collections::HashMap::new(),
        }
    }

    /// Sample count to use when anything is antialiased.
    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = samples.max(1);
        self
    }

    pub fn extent(&self) -> Extent2D {
        self.extent
    }

    /// Fill the whole target before drawing anything else.
    ///
    /// Required, not merely usual, if the frame itself draws an antialiased
    /// shape that is not analytic. Such a shape renders through a multisample
    /// buffer, and there is no way to seed that buffer with what the target
    /// already held -- so a multisampled pass has to clear, and a canvas with
    /// no background gives its root pass nothing to clear to. Drawing one is
    /// refused rather than quietly discarding whatever was on the surface.
    ///
    /// The frame itself, and not the picture: a sample count describes one
    /// pass. The same shape drawn inside a layer needs the layer's pass
    /// multisampled and leaves the frame alone, since what the frame does with
    /// a finished layer is draw one image quad. So a shadow, which is a layer
    /// underneath, needs no background at all.
    ///
    /// [`Paint::with_anti_alias`] is the other end of the choice.
    pub fn clear(&mut self, color: Color) -> &mut Self {
        self.background = Some(color);
        self
    }

    /// Current transform, mapping user coordinates to device pixels.
    pub fn transform(&self) -> Transform2D {
        self.transform
    }

    /// The region drawing is currently confined to, in device pixels.
    ///
    /// `None` is the whole target.
    pub fn clip(&self) -> Option<Scissor> {
        self.clip
    }

    /// The region drawing may still reach, in device pixels.
    ///
    /// Conservative: it is a rectangle around the clip rather than the clip
    /// itself, so a path clip reports its bounding box and a rotated
    /// rectangle reports the axis-aligned box around it. That is what makes it
    /// useful for the thing it is for -- deciding not to draw something --
    /// since a caller may skip anything outside it and must not assume
    /// everything inside it is visible.
    pub fn destination_clip_bounds(&self) -> Rect {
        self.clip_bounds
    }

    /// The same region in the coordinates the caller is currently drawing in.
    ///
    /// The device bounds carried back through the transform, which under a
    /// rotation gives the box around the rotated box and so grows a little
    /// each time. Conservative in the same direction as everything else here:
    /// too large is a missed optimization, too small is a missing shape.
    ///
    /// A transform that cannot be inverted has collapsed the plane, and
    /// nothing drawn through it reaches anything; the empty rectangle says so.
    pub fn local_clip_bounds(&self) -> Rect {
        let Some(inverse) = self.transform.inverse() else {
            return Rect::new(0.0, 0.0, 0.0, 0.0);
        };
        // Two different ways this can have no answer, and they take opposite
        // ones. A transform that has collapsed leaves nothing reachable, which
        // the check above answers with an empty rectangle. A transform whose
        // inverse carries the clip across the vanishing line leaves *more*
        // reachable than a box can say -- and since this rectangle is a promise
        // about what lies outside it, the safe failure there is everything.
        let (min, max) = transformed_bounds(
            inverse,
            Vec2::new(self.clip_bounds.left, self.clip_bounds.top),
            Vec2::new(self.clip_bounds.right, self.clip_bounds.bottom),
        )
        .unwrap_or_else(unbounded);
        Rect::new(min.x, min.y, max.x, max.y)
    }

    /// Narrow the tracked bounds to a rectangle already in device space.
    fn narrow_bounds(&mut self, min: Vec2, max: Vec2) {
        let current = self.clip_bounds;
        self.clip_bounds = Rect::new(
            current.left.max(min.x),
            current.top.max(min.y),
            current.right.min(max.x),
            current.bottom.min(max.y),
        );
        // An intersection that crossed over is empty rather than inside out,
        // which every consumer of a rectangle here would otherwise have to
        // check for itself.
        if self.clip_bounds.right < self.clip_bounds.left
            || self.clip_bounds.bottom < self.clip_bounds.top
        {
            self.clip_bounds = Rect::new(
                self.clip_bounds.left,
                self.clip_bounds.top,
                self.clip_bounds.left,
                self.clip_bounds.top,
            );
        }
    }

    /// Narrow the clip to a rectangle in user space.
    ///
    /// Intersects with the clip already in force rather than replacing it,
    /// which is what makes the clip stack compose: a subtree can only ever
    /// narrow what its parent allowed. The rectangle goes through the current
    /// transform, so it moves with the coordinate system the caller set up.
    ///
    /// # Rotation
    ///
    /// Returns [`Error::Unsupported`] when the current transform rotates or
    /// skews by anything other than a quarter turn, because the result is then
    /// a rotated quadrilateral and this clip is a rectangle. Refusing is the
    /// point: the tempting alternative is to use the rotated shape's bounding
    /// box, which admits pixels the caller asked to remove, and produces a
    /// picture that is wrong in a way that looks like a rendering bug rather
    /// than like a clip that was never applied. Arbitrary clip shapes need a
    /// stencil pass, which is a separate piece of machinery.
    pub fn clip_rect(&mut self, rect: Rect) -> Result<&mut Self> {
        // Under a transform that keeps rectangles rectangular, this is exactly
        // a scissor, which costs nothing and is exact. Under anything else the
        // result is a rotated quadrilateral, and taking its bounding box would
        // admit pixels the caller asked to remove -- so it goes through the
        // stencil like any other shape. That the fast path survives is the
        // point: a scissor stays the right answer for an axis-aligned clip even
        // now that the general one exists.
        if !preserves_axis_alignment(self.transform) {
            return self.clip_path(&rect.to_path());
        }
        let Some((min, max)) = transformed_bounds(
            self.transform,
            Vec2::new(rect.left, rect.top),
            Vec2::new(rect.right, rect.bottom),
        ) else {
            // Unreachable through the check above, which has already refused
            // anything but an affine -- and written as a fallback rather than
            // an assertion because the two are coupled only by argument, and
            // the stencil path is the right answer either way.
            return self.clip_path(&rect.to_path());
        };
        let narrowed = Scissor::from_device_bounds(
            (min - self.target.origin).into(),
            (max - self.target.origin).into(),
            self.target.extent,
        );
        self.clip = Some(match self.clip {
            Some(existing) => existing.intersect(narrowed),
            None => narrowed,
        });
        self.narrow_bounds(min, max);
        Ok(self)
    }

    /// Narrow the clip to everything *outside* a rectangle in user space.
    ///
    /// `dart:ui` spells this `clipRect` with `ClipOp.difference`, and it is the
    /// only clip that operation applies to there -- `clipPath` and `clipRRect`
    /// intersect and take no operation.
    ///
    /// Never a scissor, whatever the transform: the complement of a rectangle
    /// is not one, and a scissor is. So this goes through the stencil like an
    /// arbitrary shape, and the shape it narrows by is the target with the
    /// rectangle taken out of it -- two contours filled by the even-odd rule,
    /// which is what makes the second a hole in the first rather than a second
    /// region drawn over it. `draw_drrect` builds a ring the same way.
    ///
    /// Both contours are built in device coordinates and filled through the
    /// identity, rather than in user space through the transform. The reason is
    /// the target: it is a device rectangle, and expressing it in user space
    /// would mean inverting a transform that may not be invertible. The
    /// rectangle's own corners go through the transform here instead, so a
    /// rotation gives the quadrilateral it should rather than a box around one.
    pub fn clip_out_rect(&mut self, rect: Rect) -> Result<&mut Self> {
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }
        let mut builder = PathBuilder::new().with_fill_rule(FillRule::EvenOdd);
        // The whole target, which is what "outside the rectangle" is measured
        // against.
        let target = Rect::new(
            self.target.origin.x,
            self.target.origin.y,
            self.target.origin.x + self.target.extent.width as f32,
            self.target.origin.y + self.target.extent.height as f32,
        );
        builder
            .move_to(Vec2::new(target.left, target.top))
            .line_to(Vec2::new(target.right, target.top))
            .line_to(Vec2::new(target.right, target.bottom))
            .line_to(Vec2::new(target.left, target.bottom))
            .close();
        // The rectangle's corners under the transform, in the order that keeps
        // the contour simple. A degenerate one encloses no area and takes
        // nothing out, which is the right answer for a clip that removes an
        // empty rectangle.
        let corners = [
            Vec2::new(rect.left, rect.top),
            Vec2::new(rect.right, rect.top),
            Vec2::new(rect.right, rect.bottom),
            Vec2::new(rect.left, rect.bottom),
        ]
        .map(|corner| self.transform.project_point2(corner));
        // A corner that is not a number describes no rectangle, so there is
        // nothing to take out and the clip is left as it was. Under a transform
        // carrying perspective this is also what catches a corner past the
        // vanishing line, where the divide reports an infinity rather than a
        // wrong finite answer -- and where the four corners would no longer
        // describe the region even if they were finite.
        if !corners.iter().all(|corner| corner.is_finite()) {
            return Ok(self);
        }
        builder.move_to(corners[0]);
        for corner in &corners[1..] {
            builder.line_to(*corner);
        }
        builder.close();

        let paint = RenderPaint {
            material: Material::solid([1.0, 1.0, 1.0, 1.0]),
            filter: ColorFilter::None,
            blend: BlendMode::Src,
            clip: self.clip,
            stencil: ClipState::narrow(self.depth),
        };
        let path = builder.build();
        self.renderer
            .fill_into(&mut self.batch, &path, Affine2::IDENTITY, &paint)?;
        self.depth += 1;
        self.clips.push(StencilClip {
            path,
            transform: Transform2D::IDENTITY,
            scissor: paint.clip,
        });
        // The tracked bounds are left alone, and that is not an oversight.
        // Removing a rectangle from the middle of a region leaves its bounding
        // box exactly where it was, and removing one from the edge leaves a box
        // that is too large -- which is the safe direction, since these bounds
        // decide how much a caller draws and how large a layer is allocated.
        Ok(self)
    }

    /// Narrow the clip to an arbitrary path in user space.
    ///
    /// Intersects with the clip already in force, like [`Self::clip_rect`], and
    /// like it the path travels through the current transform. The shape is
    /// recorded into the batch as a draw that writes only the stencil, so it
    /// costs one draw where a rectangular clip costs none.
    ///
    /// The path is filled by the same tessellator that fills a drawn shape, so
    /// it obeys the same fill rule and produces the same edges. That matters
    /// for more than consistency: the tessellation is a set of non-overlapping
    /// triangles, which is what lets the stencil step forward by one rather
    /// than needing the parity trick an overlapping fan would.
    pub fn clip_path(&mut self, path: &Path) -> Result<&mut Self> {
        if self.clip.is_some_and(Scissor::is_empty) {
            // Already clipped to nothing; narrowing further changes nothing and
            // the draw would be dropped anyway.
            return Ok(self);
        }
        let paint = RenderPaint {
            // Masked off entirely by a clip draw, but a material is still
            // needed to build one. White says plainly that nothing here is a
            // color decision.
            material: Material::solid([1.0, 1.0, 1.0, 1.0]),
            filter: ColorFilter::None,
            blend: BlendMode::Src,
            clip: self.clip,
            stencil: ClipState::narrow(self.depth),
        };
        self.renderer
            .fill_into(&mut self.batch, path, self.transform, &paint)?;
        self.depth += 1;
        self.clips.push(StencilClip {
            path: path.clone(),
            transform: self.transform,
            scissor: paint.clip,
        });
        // A path clip narrows what may be drawn without touching the scissor,
        // so the tracked rectangle is the only place it is accounted for. Its
        // bounding box rather than the path: this rectangle is a promise about
        // what is *outside* it, and a box around a shape keeps that promise.
        let bounds = path.bounds();
        // A clip whose bounds cannot be computed narrows nothing here. The
        // stencil still confines it exactly; what is lost is only the tracked
        // rectangle used to skip work, and leaving it wide is the direction
        // that costs time rather than pixels.
        if let Some((min, max)) = transformed_bounds(self.transform, bounds.min, bounds.max) {
            self.narrow_bounds(min, max);
        }
        Ok(self)
    }

    /// Save the transform and clip so a later `restore` can return to them.
    pub fn save(&mut self) -> &mut Self {
        self.stack.push(SavedState {
            transform: self.transform,
            clip: self.clip,
            clip_bounds: self.clip_bounds,
            depth: self.depth,
            clips: self.clips.len(),
            layer: None,
        });
        self
    }

    /// Begin drawing into a layer of its own, composited back on `restore`.
    ///
    /// Everything recorded until the matching restore goes into a separate
    /// target and arrives as one image, which is what makes an alpha here apply
    /// to the group rather than to each shape in it.
    ///
    /// The layer starts unclipped and at the identity for clipping purposes,
    /// while keeping the current transform. It does not need to inherit the
    /// clip because the draw that composites it is subject to it: content the
    /// clip excludes is discarded once rather than prevented from being drawn,
    /// which costs some work in the layer and keeps a stencil clip from having
    /// to be rebuilt in a second target.
    pub fn save_layer(&mut self, layer: Layer) -> &mut Self {
        // A layer that captures a backdrop is handed to the bounded path with
        // no bounds, which narrows it to the clip in force. That path scales
        // the layer itself, so this one hands it over unscaled.
        //
        // A shared key goes that way too. What the key shares is the capture
        // and the filtered image over it, both of which stay the parent's size
        // because they have to serve every group naming it; the layer's own
        // target is bound by the clip like any other.
        if layer.backdrop_blur > 0.0 && layer.matrix.is_none() {
            let (min, max) = unbounded();
            // Cannot fail: only a backdrop *filter* can refuse, and this
            // passes none -- the blur rides on the layer.
            let _ = self.save_layer_device_bounds_running(layer, min, max, None, None);
            return self;
        }
        // The other entry to `open_layer` converts the sigmas where it computes
        // its reach; this one has no bounds to reach past and so converts them
        // here. Both are the same conversion at the same moment -- the layer is
        // opened under the transform the caller drew in, which is the one its
        // sigma is stated against.
        let layer = layer.scaled_by(max_scale_of(self.transform), axis_scales_of(self.transform));
        let widened = self.matrix_preimage(&layer);
        let pending = self.open_layer(layer, None, None, None).unwrap_or(None);
        if let Some(target) = widened {
            self.aim_at(target);
        }
        self.seed_backdrop(pending);
        self
    }

    /// The region a layer must record over when a matrix will move its result.
    ///
    /// A layer's target is the parent's, and a draw outside it is scissored
    /// away -- which is right for every layer except one whose matrix brings
    /// content in from outside. `dart:ui`'s matrix image filter is exactly
    /// that, and upstream's `MatrixImageFilterDoesntCullWhenTranslatedFromOffscreen`
    /// draws a circle three hundred points to the left of the frame and
    /// translates it back.
    ///
    /// What has to be recorded over is the *pre-image*: the region that, once
    /// the matrix has moved it, lands where the parent can see it. Union with
    /// the parent, because a matrix that moves some content in leaves the rest
    /// where it was.
    ///
    /// **Opening wide costs no memory, and that is what makes this safe.** The
    /// pass's extent comes from the narrowed target in `finish_layer`, which is
    /// the content's own bounds -- so a minifying matrix, whose inverse is a
    /// magnifying one, widens the region a draw may land in without allocating
    /// for a region no draw reached. What is allocated stays bounded by what
    /// the caller drew.
    fn matrix_preimage(&self, layer: &Layer) -> Option<Target> {
        let matrix = layer.matrix?;
        let parent = self.target;
        let (min, max) = (
            parent.origin,
            parent.origin + Vec2::new(parent.extent.width as f32, parent.extent.height as f32),
        );
        let (back_min, back_max) = transformed_bounds(matrix.inverse()?, min, max)?;
        let left = min.x.min(back_min.x).floor();
        let top = min.y.min(back_min.y).floor();
        let right = max.x.max(back_max.x).ceil();
        let bottom = max.y.max(back_max.y).ceil();
        // A matrix that folds the plane or carries the region across the
        // vanishing line has no region to report, and one that widens past what
        // an extent can hold is not a picture anybody asked for.
        let (width, height) = (right - left, bottom - top);
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return None;
        }
        Some(Target {
            origin: Vec2::new(left, top),
            extent: Extent2D::new(width as u32, height as u32),
        })
    }

    /// Push the layer frame, and cut the backdrop out if one was asked for.
    ///
    /// Returns what the layer's target must be seeded with once its size is
    /// settled, which is why this is separate from the seeding: a bounded layer
    /// does not know its own target until after the frame exists, and the seed
    /// has to land in the target the content will draw into.
    fn open_layer(
        &mut self,
        layer: Layer,
        filter: Option<ImageFilter>,
        backdrop: Option<&ImageFilter>,
        region: Option<Target>,
    ) -> Result<Option<Backdrop>> {
        let parent = self.target;
        // Where the backdrop's filter writes, which is the target the layer is
        // about to take rather than the parent it is captured from. `None` is
        // the caller that has not worked one out yet, and gets the parent.
        let region = region.unwrap_or(parent);
        // The layer's own sigma is the `Copy`-friendly spelling of the same
        // thing, so it becomes a filter here and there is one path below.
        //
        // And this is where it is converted into device pixels, because this is
        // the first place with two components to convert it into. `Layer::scaled_by`
        // leaves it alone for that reason: the caller stated one number, meaning a
        // blur round in their space, and under a transform whose axes scale
        // differently that is an oval. A filter handed over by the caller is
        // already converted -- `save_layer_filtered` and the backdrop entry points
        // run it through `ImageFilter::scaled_by` -- so only this arm converts.
        let axes = axis_scales_of(self.transform);
        let asked = match backdrop {
            Some(filter) => filter.clone(),
            None if layer.backdrop_blur > 0.0 => ImageFilter::Blur {
                sigma_x: layer.backdrop_blur * axes.x,
                sigma_y: layer.backdrop_blur * axes.y,
            },
            None => ImageFilter::None,
        };
        let filtered = if asked.is_identity() {
            None
        } else {
            // The frame is pushed whether or not this succeeds: a layer that
            // fails to filter its backdrop still has to be a layer, or the
            // `restore` the caller has already written closes something else.
            match self.backdrop_pass(layer.backdrop_id, parent, region, &asked) {
                Ok(pending) => Some(pending),
                Err(e) => {
                    self.push_layer_frame(layer, filter);
                    return Err(e);
                }
            }
        };
        self.push_layer_frame(layer, filter);
        Ok(filtered)
    }

    /// The pass a backdrop-filtered layer starts from, and what it is an image
    /// of.
    ///
    /// Without an id this is the whole of what a backdrop filter was: cut the
    /// target's content out into a pass, and filter that. With one, both halves
    /// are shared with whatever named the id before -- the capture always, so
    /// that the second layer filters what was behind the first rather than the
    /// first's own result, and the filtered pass as well when the filter
    /// matches one already run over that capture.
    ///
    /// The second element is the target the pass covers, which is `parent` for
    /// a fresh capture and whatever the id captured for a shared one. Seeding
    /// needs it to place the image, and the two can differ; see
    /// [`BackdropShare::captured_from`].
    ///
    /// The cut happens before the filter can refuse, which is the order the
    /// no-id path always had. A refusal therefore leaves the capture recorded,
    /// which is right: the backdrop was captured, and the next layer naming the
    /// id should filter that same image whether or not this one could.
    fn backdrop_pass(
        &mut self,
        id: Option<i64>,
        parent: Target,
        region: Target,
        filter: &ImageFilter,
    ) -> Result<Backdrop> {
        // A matrix is peeled off here rather than run as a pass. Every other
        // filter reads its input where the fragment is and writes there, so it
        // is a pass over a target the size of what it was handed; a matrix
        // moves the image instead, and a pass for it would need a target sized
        // for where the content went. It does not need one: what a backdrop is
        // seeded into is the layer's own target, already allocated and the same
        // size whatever the matrix says, and the seed is a draw with a mapping
        // -- so the matrix goes into that mapping and no pass is spent.
        //
        // Only at the top. Inside a composition the matrix is between two
        // filters that do run as passes, so its result is what the outer one
        // reads and there is no seed to fold it into; that stays refused, in
        // `filter_passes`.
        let (filter, moved_by) = match filter {
            ImageFilter::Matrix { transform } => {
                let Some(affine) = transform.to_affine() else {
                    return Err(Error::Unsupported(
                        "a projective matrix is not available as a backdrop filter; \
                         it maps the image through a division this seed does not do",
                    ));
                };
                if affine.matrix2.determinant() == 0.0 {
                    return Err(Error::Unsupported(
                        "a singular matrix is not available as a backdrop filter; \
                         it collapses the image and nothing says what it read",
                    ));
                }
                (&ImageFilter::None, Some(affine))
            }
            other => (other, None),
        };
        let Some(id) = id else {
            // The capture is the pass being cut, so it is the parent's size
            // whatever happens next. What the filter *writes* need not be: its
            // passes read the whole capture and only have to cover the region
            // the layer will hold, which is where the seed reads from.
            let cut = self.cut_pass();
            let pass = self.filter_passes(cut, region, filter, BlurBasis::of(self.transform))?;
            return Ok(Backdrop {
                pass,
                // Unless the filter had nothing to do -- a matrix backdrop
                // moves the capture rather than recomputing it, and hands the
                // cut straight back, which is the parent's region and not this
                // one.
                of: if pass == cut { parent } else { region },
                moved_by,
            });
        };

        // Read out by value first: what follows needs `&mut self`, and the
        // three things wanted from the entry are all `Copy`.
        let existing = self.backdrops.get(&id).map(|share| {
            let already = share
                .filtered
                .iter()
                .find(|(known, _)| known == filter)
                .map(|(_, pass)| *pass);
            (share.captured_from, share.captured, already)
        });

        let (over, captured) = match existing {
            // A filter is a function of its input, so the same filter over the
            // same capture is the same image and there is nothing to compute.
            Some((from, _, Some(pass))) => {
                return Ok(Backdrop {
                    pass,
                    of: from,
                    moved_by,
                })
            }
            Some((from, captured, None)) => (from, captured),
            None => {
                let cut = self.cut_pass();
                self.backdrops.insert(
                    id,
                    BackdropShare {
                        captured_from: parent,
                        captured: cut,
                        filtered: Vec::new(),
                    },
                );
                (parent, cut)
            }
        };

        let pass = self.filter_passes(captured, over, filter, BlurBasis::of(self.transform))?;
        if let Some(share) = self.backdrops.get_mut(&id) {
            share.filtered.push((filter.clone(), pass));
        }
        Ok(Backdrop {
            pass,
            of: over,
            moved_by,
        })
    }

    /// End the current target's pass here, and answer which pass now holds it.
    ///
    /// The architecture's rule is that a pass cannot sample the attachment it
    /// is writing, which is exactly what a backdrop filter asks for. So the
    /// pass stops: everything drawn into this target so far becomes a pass of
    /// its own, and what follows begins by drawing that pass back in.
    ///
    /// The redraw is the cost, one full-target copy per backdrop filter, and it
    /// is not avoidable without the machinery the rule exists for the absence
    /// of. It is a `Src` blit covering the whole target, so it reproduces what
    /// was cut exactly rather than compositing with it.
    fn cut_pass(&mut self) -> usize {
        let target = self.target;
        let batch = std::mem::take(&mut self.batch);
        let sources = std::mem::take(&mut self.sources);
        // The clear belongs to the half that starts from nothing. A layer's
        // target clears to transparent and the frame's to its background; after
        // the cut neither clears again, since the redraw below covers every
        // pixel with `Src`.
        let clear = if self.in_layer() {
            Some([0.0; 4])
        } else {
            self.background
                .take()
                .map(|c| c.to_array())
                .or_else(|| self.seeded_by_redraw.then_some([0.0; 4]))
        };
        self.finished.push(Pass {
            batch,
            descriptor: PassDescriptor {
                clear,
                samples: self.pass_samples(),
                viewport: None,
            },
            sources,
            extent: target.extent,
        });
        // The batch that continues is empty, so it inherits nothing: what
        // follows the cut is a full-target `Src` blit, which no sample count
        // changes, plus whatever is drawn next. A draw that wants
        // multisampling will say so again.
        self.anti_alias = false;
        let index = self.finished.len() - 1;
        self.draw_whole_pass(index, target, target, BlendMode::Src, None);
        self.seeded_by_redraw = true;
        self.rebuild_stencil_clips();
        index
    }

    /// Make the stencil clips again, in the batch that follows a cut.
    ///
    /// A stencil clip is a draw and a pass owns its stencil attachment, so
    /// cutting the pass throws the clip state away: the redraw restores the
    /// color and nothing restores the stencil, leaving every content draw after
    /// the cut testing for a depth no pixel holds. Which is to say it draws
    /// nothing, silently and everywhere.
    ///
    /// The narrowings are made again in the order they were made the first
    /// time, at the depths they were made at -- which they still are, since
    /// `clips` holds exactly those in force and each was made at its own index.
    /// A failure to build one is dropped rather than propagated: the shape
    /// tessellated once already to get here, and there is no result to fail
    /// into halfway through a cut.
    fn rebuild_stencil_clips(&mut self) {
        for depth in 0..self.clips.len() {
            let clip = self.clips[depth].clone();
            let paint = RenderPaint {
                material: Material::solid([1.0, 1.0, 1.0, 1.0]),
                filter: ColorFilter::None,
                blend: BlendMode::Src,
                clip: clip.scissor,
                stencil: ClipState::narrow(depth as u32),
            };
            let _ = self
                .renderer
                .fill_into(&mut self.batch, &clip.path, clip.transform, &paint);
        }
    }

    /// Whether a layer is open, which decides what a cut pass clears to.
    fn in_layer(&self) -> bool {
        self.stack.iter().any(|state| state.layer.is_some())
    }

    /// Draw a finished pass, sized for `source`, across the whole of `into`.
    ///
    /// `into` is a rectangle of `source`: the same target when a pass is being
    /// redrawn after a cut, and a sub-rectangle of it when a bounded layer is
    /// seeded with a backdrop. The mapping is the inverse of the one a layer
    /// composite uses, and reduces to the full-texture mapping when the two are
    /// the same size, which is what makes the unbounded case share this code.
    fn draw_whole_pass(
        &mut self,
        pass: usize,
        source: Target,
        into: Target,
        blend: BlendMode,
        moved_by: Option<Affine2>,
    ) {
        let slot = self.slot_for(TextureSource::Layer(pass));
        let offset = into.origin - source.origin;
        let (iw, ih) = (into.extent.width as f32, into.extent.height as f32);
        let (sw, sh) = (source.extent.width as f32, source.extent.height as f32);
        // Stated in the direction the shader reads it -- clip space to the
        // texture -- because the sizes are known and there is no placement to
        // invert. The anchor rides inside it like every other paint's.
        let anchor = Vec2::new(-1.0 - 2.0 * offset.x / iw, 1.0 + 2.0 * offset.y / ih);
        let scale = Mat2::from_diagonal(Vec2::new(0.5 * iw / sw, -0.5 * ih / sh));
        let clip_to_texture = Affine2::from_mat2(scale) * Affine2::from_translation(-anchor);
        // A matrix moves the image, so the fragment at a device point reads
        // where that point came *from*: the mapping gains the inverse, stated
        // in device pixels, between the texture coordinates and the device
        // position they name. `texels` is that position from a normalized
        // coordinate and `normalized` is its inverse, so with no matrix the
        // three cancel and this is the mapping above unchanged.
        let clip_to_texture = match moved_by {
            None => clip_to_texture,
            Some(matrix) => {
                let texels = Affine2::from_translation(source.origin)
                    * Affine2::from_scale(Vec2::new(sw, sh));
                let normalized = texels.inverse();
                normalized * matrix.inverse() * texels * clip_to_texture
            }
        };
        let material = Material::Image {
            to_local: to_local_columns(Transform2D::from(clip_to_texture)),
            slot,
            alpha: 1.0,
            // Clamped when the image stays where it was, which is every case
            // but one: the mapping then covers exactly the texture and there is
            // nothing outside it to decide about. A matrix leaves parts of the
            // target naming no texel, and those have to come out transparent so
            // what is underneath shows -- smearing the edge across them is what
            // a clamp would do and is not what a filter that moved an image
            // says about where the image is not.
            tile: if moved_by.is_some() {
                TileMode::Decal
            } else {
                TileMode::Clamp
            },
            // A layer is composited at its own size, so a texel lands on a
            // pixel and the filter has nothing to blend. Linear anyway, which
            // is what a target scaled by a resize would want.
            sampling: Sampling::Linear,
            source: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
        };
        let paint = RenderPaint {
            material,
            filter: ColorFilter::None,
            blend,
            // Neither clipped nor stencilled: this is the target's own content
            // being restored or seeded, not something the caller drew, and a
            // clip in force belongs to what comes after it.
            clip: None,
            stencil: ClipState::UNCLIPPED,
        };
        let whole = into.path();
        let _ = self
            .renderer
            .fill_into(&mut self.batch, &whole, Affine2::IDENTITY, &paint);
    }

    /// Put the filtered backdrop under the layer's own content.
    ///
    /// `Src`, because it is the layer's starting image rather than something
    /// composited onto it, and the layer's target was cleared to transparent.
    fn seed_backdrop(&mut self, pending: Option<Backdrop>) {
        let Some(Backdrop {
            pass,
            of: parent,
            moved_by,
        }) = pending
        else {
            return;
        };
        let into = self.target;
        self.draw_whole_pass(pass, parent, into, BlendMode::Src, moved_by);
    }

    fn push_layer_frame(&mut self, layer: Layer, filter: Option<ImageFilter>) {
        self.stack.push(SavedState {
            transform: self.transform,
            clip: self.clip,
            clip_bounds: self.clip_bounds,
            depth: self.depth,
            clips: self.clips.len(),
            layer: Some(LayerFrame {
                batch: std::mem::take(&mut self.batch),
                sources: std::mem::take(&mut self.sources),
                clips: std::mem::take(&mut self.clips),
                paint: layer,
                filter,
                parent: self.target,
                anti_alias: std::mem::take(&mut self.anti_alias),
                seeded_by_redraw: std::mem::take(&mut self.seeded_by_redraw),
                blur_basis: BlurBasis::of(self.transform),
            }),
        });
        self.clip = None;
        self.depth = 0;
    }

    /// Open a layer that only needs to cover `bounds`.
    ///
    /// `bounds` is in the current user space, and is the caller's promise that
    /// nothing drawn in the layer matters outside it. The layer gets a target
    /// the size of that region rather than the size of the frame, which is the
    /// whole reason to state it: a layer over a tenth of the frame then costs a
    /// tenth of the memory and a tenth of the fill, and a frame that opens
    /// several stops being dominated by full-frame allocations it does not use.
    ///
    /// The promise is enforced by construction rather than trusted. Content
    /// outside the region is clipped away by the target's own edges, so a
    /// caller who understates the bounds sees the drawing cut off — visible and
    /// attributable, rather than reading stale memory.
    ///
    /// The region is taken to device pixels and rounded outward to whole ones,
    /// so a fractional bound never costs coverage at the edge, and is narrowed
    /// to what the enclosing target can show, since a layer larger than that
    /// renders pixels nothing can sample. A region that comes out empty falls
    /// back to a full-size layer, that being a case where a smaller target
    /// would be a guess and guessing wrong loses drawing.
    ///
    /// A layer that filters its backdrop is the exception to all of this: for
    /// it the bounds are not an optimization but the region filtered, so
    /// omitting them filters the whole target rather than merely allocating
    /// more of one. See [`Layer::backdrop_blur`].
    ///
    /// Under a rotation the region becomes a quadrilateral, and the target is
    /// the box around it. That covers more than the caller promised, which is
    /// the safe direction: a target is an allocation rather than a clip the
    /// caller can observe, so over-covering costs a little memory where
    /// under-covering would lose drawing. This is the opposite of what
    /// [`Self::clip_rect`] does with the same box, and for the same reason —
    /// there the box would admit pixels the caller asked to remove.
    pub fn save_layer_bounds(&mut self, layer: Layer, bounds: Rect) -> &mut Self {
        let (min, max) = transformed_bounds(
            self.transform,
            Vec2::new(bounds.left, bounds.top),
            Vec2::new(bounds.right, bounds.bottom),
        )
        .unwrap_or_else(unbounded);
        self.save_layer_device_bounds(layer, min, max)
    }

    /// A layer filtered as a whole by any image filter.
    ///
    /// [`Layer`]'s own fields carry the four kinds that fit in a `Copy` struct
    /// -- a blur, a morphology, a matrix and a color filter -- and apply them
    /// in one fixed order. This takes an [`ImageFilter`], which adds the two
    /// they cannot say: a caller's fragment program, and a composition in
    /// whichever order the caller wrote it.
    ///
    /// The same operation `dart:ui` spells as an `imageFilter` on the paint
    /// handed to `saveLayer`, and the group counterpart of what
    /// `Paint::with_image_filter` already does for one draw.
    ///
    /// Refuses a matrix, which is a redirection rather than an absence: a
    /// group's finished image is moved by `Layer::with_matrix`, which states
    /// the same thing and is what `peel` turns this filter into everywhere
    /// else. Refused here rather than at `restore`, because a `restore` has no
    /// result to fail into and by then the caller has drawn into the layer.
    ///
    /// A *backdrop's* matrix is not refused; see `backdrop_pass`. The two are
    /// different questions -- a group's own image is composited by machinery
    /// that already takes a matrix, and a backdrop's is seeded by a draw that
    /// now does.
    pub fn save_layer_filtered(
        &mut self,
        layer: Layer,
        bounds: Option<Rect>,
        filter: &ImageFilter,
    ) -> Result<&mut Self> {
        if matches!(filter, ImageFilter::Matrix { .. }) {
            return Err(Error::Unsupported(
                "a matrix filters a group through `Layer::with_matrix`, which \
                 states where the finished image goes",
            ));
        }
        // Refused here for the reason the depth is a constant at all: everything
        // that reads a composition walks it recursively, and a composition deep
        // enough took the process down rather than returning an error. See
        // `ImageFilter::MAX_NESTING`.
        if filter.nests_deeper_than(ImageFilter::MAX_NESTING) {
            return Err(Error::Unsupported(
                "this image filter composes more filters than can be walked",
            ));
        }
        let (min, max) = match bounds {
            Some(bounds) => transformed_bounds(
                self.transform,
                Vec2::new(bounds.left, bounds.top),
                Vec2::new(bounds.right, bounds.bottom),
            )
            .unwrap_or_else(unbounded),
            None => unbounded(),
        };
        self.save_layer_device_bounds_running(layer, min, max, Some(filter.clone()), None)
    }

    /// A layer over a backdrop filtered by any image filter.
    ///
    /// [`Layer::backdrop_blur`] is the same operation with the one filter a
    /// `Copy` layer can hold, and is the spelling to reach for when a blur is
    /// what is wanted. This takes the rest: a color filter, a morphology, a
    /// caller's fragment program, or a composition of them -- which is what
    /// `SceneBuilder.pushBackdropFilter` accepts and what upstream's
    /// `SaveLayer` takes a `DlImageFilter` for.
    ///
    /// The bounds are the filtered region rather than an optimization, for the
    /// reason [`Layer::backdrop_blur`] gives: a frosted panel is a bounded
    /// layer, and the same layer unbounded filters the whole frame. Both are
    /// meaningful and they are different pictures.
    ///
    /// Refuses a matrix, which is the one kind that moves the image rather than
    /// recomputing it in place. See `filter_passes`.
    pub fn save_layer_backdrop(
        &mut self,
        layer: Layer,
        bounds: Option<Rect>,
        backdrop: &ImageFilter,
    ) -> Result<&mut Self> {
        // The same refusal `save_layer_filtered` makes, and for the same reason:
        // a backdrop's filter is read by the same recursive walks.
        if backdrop.nests_deeper_than(ImageFilter::MAX_NESTING) {
            return Err(Error::Unsupported(
                "this image filter composes more filters than can be walked",
            ));
        }
        let (min, max) = match bounds {
            Some(bounds) => transformed_bounds(
                self.transform,
                Vec2::new(bounds.left, bounds.top),
                Vec2::new(bounds.right, bounds.bottom),
            )
            .unwrap_or_else(unbounded),
            None => unbounded(),
        };
        self.save_layer_device_bounds_running(layer, min, max, None, Some(backdrop))
    }

    /// A bounded layer whose region is already in device pixels.
    ///
    /// The same thing [`Self::save_layer_bounds`] does, minus the transform.
    /// Every filter's reach is a length in device pixels, so a caller that has
    /// already worked out where a filtered result will land -- which composing
    /// two filters requires, since the inner one's growth has to fit inside the
    /// outer one's target -- has device coordinates in hand and would only be
    /// mapping them backward to have them mapped forward again. Under a scale
    /// the round trip is not the identity: a reach of eight device pixels
    /// divided by the scale and multiplied by it again is eight only if nothing
    /// rounds, and the bounds are floored and ceiled at the end.
    fn save_layer_device_bounds(&mut self, layer: Layer, min: Vec2, max: Vec2) -> &mut Self {
        // Cannot fail: only a backdrop filter can refuse, and this passes none.
        let _ = self.save_layer_device_bounds_running(layer, min, max, None, None);
        self
    }

    /// The same, with a caller's program to run over the finished layer.
    ///
    /// Separate rather than a defaulted argument because every caller but the
    /// three filtered draws passes `None`, and a parameter that is almost
    /// always one value reads as though it were sometimes the other.
    fn save_layer_device_bounds_running(
        &mut self,
        layer: Layer,
        min: Vec2,
        max: Vec2,
        filter: Option<ImageFilter>,
        backdrop: Option<&ImageFilter>,
    ) -> Result<&mut Self> {
        // A caller's sigma is in the space they were drawing in, which is what
        // `dart:ui` means by it and what upstream honors -- its
        // `GaussianBlurFilterContents` scales by `effect_transform.Basis()`,
        // and the backdrop path hands it `transform.Basis()`. Everything below
        // this line works in device pixels: the reach is applied to bounds
        // already transformed, and the passes that do the blurring run on a
        // target. So the conversion happens once, here, where the transform
        // that decides it is still the one the caller drew under.
        let basis = BlurBasis::of(self.transform);
        let scale = max_scale_of(self.transform);
        let axes = axis_scales_of(self.transform);
        let layer = layer.scaled_by(scale, axes);
        // And the filters beside it, which this line did not touch for as long as
        // it has existed. The comment above said the conversion happens once here
        // and it happened to the layer only, so `save_layer_filtered` with a blur
        // took a device sigma while the same filter through
        // `Paint::with_image_filter` -- which becomes a layer -- took a local one.
        let filter = filter.map(|filter| filter.scaled_by(scale, axes));
        let backdrop = backdrop
            .cloned()
            .map(|filter| filter.scaled_by(scale, axes));
        let backdrop = backdrop.as_ref();
        let reach = layer.reach(basis);
        // A layer opened under a clip cannot draw outside it, and neither can
        // the draw that composites it, so an unbounded one does not need a
        // target the size of its parent. A frosted panel is the case that
        // pays: the clip is the panel and the parent is the frame, and the
        // capture, the blur's passes and the intermediate were all frame-sized
        // for a panel a fortieth of the area.
        //
        // Only where a backdrop is being captured, and only for a layer that
        // shares no key and has no matrix. Each of those wants a region this
        // does not know how to compute: a shared capture has to cover every
        // group naming the id -- which is upstream's coverage union and is not
        // this -- a matrix records its pre-image, which is where the content
        // will come *from* rather than where the clip is, and a destructive
        // group naming the id -- which is upstream's coverage union and is not
        // this -- and a matrix records its pre-image, which is where the
        // content will come *from* rather than where the clip is.
        //
        // A destructive mode is not excluded, and the difference from
        // `narrowed` is the whole of why. That one shrinks to the layer's
        // *content*, which such a mode reaches past -- it writes where the
        // source drew nothing. This shrinks to the clip, which is the region
        // the composite can touch at all, so there is nothing outside it left
        // to affect.
        let captures_backdrop = backdrop.is_some() || layer.backdrop_blur > 0.0;
        let (min, max) = if captures_backdrop && layer.matrix.is_none() {
            let clip = self.clip_bounds;
            (
                Vec2::new(min.x.max(clip.left), min.y.max(clip.top)),
                Vec2::new(max.x.min(clip.right), max.y.min(clip.bottom)),
            )
        } else {
            (min, max)
        };
        // A filter given to the layer as a whole reaches past what it is handed
        // too, and by how much only the filter knows. Asked here rather than
        // left to the narrowing below, because the narrowing only runs for a
        // layer that was *not* given bounds: a caller who states bounds gets
        // the target those bounds describe, and a filter's spread has to be
        // added to it or the spread is cut off at the bound.
        //
        // Missing this made the two spellings of a layer blur disagree. A sigma
        // on the layer went through `reach` above and spread ten pixels past a
        // stated bound; the same blur handed over as the layer's filter stopped
        // dead at it. They are the same picture and the same comment two
        // functions up says so.
        let filter_reach = filter.as_ref().map(|filter| {
            let unit = Vec2::ZERO;
            let (min, max) = filter.covering(unit, unit, &|sigma| basis.reach(sigma));
            (-min).max(max)
        });
        // A blur reaches past what it was given. The caller states where the
        // content is, which is the question they can answer; how far a blur
        // carries it is this renderer's arithmetic, and a target sized to the
        // content alone would cut the halo off square at the bound -- the
        // failure looking exactly like a shadow with a straight edge.
        //
        // The kernel's own radius, `ceil((sigma - 0.5) * sqrt(3))`, which is
        // where the shader stops taking taps -- so the target covers every
        // texel the blur will actually read and no more. Nearer one and three
        // quarter deviations than three: the truncation is upstream's
        // `kKernelRadiusPerSigma`, and `blur_reach` is the one place it is
        // stated.
        let (min, max) = (min - reach, max + reach);
        // After the layer's own blur and morphology, because that is the order
        // `finish_layer` applies them in: the filter sees what those produced.
        let (min, max) = match filter_reach {
            Some(spread) => (min - spread, max + spread),
            None => (min, max),
        };
        let parent = self.target;
        let left = min.x.floor().max(parent.origin.x);
        let top = min.y.floor().max(parent.origin.y);
        let right = max
            .x
            .ceil()
            .min(parent.origin.x + parent.extent.width as f32);
        let bottom = max
            .y
            .ceil()
            .min(parent.origin.y + parent.extent.height as f32);
        // Worked out before the layer is opened, because opening it is what
        // builds the backdrop's passes and those want to know where they are
        // writing. The layer is still opened without *seeding*: the seed has to
        // land in the target the content will draw into, so it waits until that
        // target is the one aimed at.
        let narrowed = (right > left && bottom > top).then(|| Target {
            origin: Vec2::new(left, top),
            extent: Extent2D::new((right - left) as u32, (bottom - top) as u32),
        });
        let pending = self.open_layer(layer, filter, backdrop, narrowed)?;
        let Some(target) = narrowed else {
            // Nothing survived the clamp, so the layer keeps the full-size
            // target it was opened with and the backdrop is seeded across that.
            self.seed_backdrop(pending);
            return Ok(self);
        };
        self.aim_at(target);
        self.seed_backdrop(pending);
        Ok(self)
    }

    /// How many layers are open, for a caller checking its own balance.
    pub fn layer_depth(&self) -> usize {
        self.stack.iter().filter(|s| s.layer.is_some()).count()
    }

    /// The slot a source occupies in the pass being recorded, adding it if new.
    ///
    /// Slots are per pass rather than global, because each pass carries its own
    /// table. Deduplicating means a recording that draws the same image twenty
    /// times binds one texture rather than twenty.
    /// The slot holding this gradient's colors, when they cannot be carried.
    ///
    /// `None` for the ordinary case, which keeps the stops in push constants
    /// and costs no texture, no upload and no binding. Only a gradient with
    /// more stops than a material can hold pays for a ramp, and it pays once
    /// per distinct gradient rather than once per draw.
    fn ramp_for(&mut self, stops: &[GradientStop]) -> Option<u32> {
        (stops.len() > MAX_STOPS).then(|| self.ramp_slot(stops))
    }

    /// Tabulate a gradient's colors and return the slot holding them.
    ///
    /// Called only where the stops outnumber what a material can carry. The
    /// ramp is deduplicated across the recording by its contents, because the
    /// same gradient drawn twice is one texture, and a frame that paints a
    /// hundred list rows with one theme gradient should upload it once.
    fn ramp_slot(&mut self, stops: &[GradientStop]) -> u32 {
        let ramp = Ramp::bake(stops);
        let index = match self.ramps.iter().position(|r| *r == ramp) {
            Some(index) => index,
            None => {
                self.ramps.push(ramp);
                self.ramps.len() - 1
            }
        };
        self.slot_for(TextureSource::Ramp(index))
    }

    fn slot_for(&mut self, source: TextureSource) -> u32 {
        if let Some(index) = self.sources.iter().position(|s| *s == source) {
            return index as u32;
        }
        self.sources.push(source);
        (self.sources.len() - 1) as u32
    }

    /// Return to the most recently saved transform and clip.
    ///
    /// Restoring without a matching save leaves both alone rather than
    /// panicking: an unbalanced pair is a caller bug, but taking down a frame
    /// loop for it is worse than continuing with what is already correct.
    pub fn restore(&mut self) -> &mut Self {
        let Some(previous) = self.stack.pop() else {
            return self;
        };
        self.transform = previous.transform;
        self.clip = previous.clip;
        self.clip_bounds = previous.clip_bounds;

        if let Some(frame) = previous.layer {
            // The clip and stencil are restored before the composite is
            // recorded, so the layer arrives subject to what was in force when
            // it was opened rather than to whatever it did inside itself.
            self.depth = previous.depth;
            self.clips = frame.clips.clone();
            self.finish_layer(frame);
            return self;
        }

        // Everything this save added to the list goes with it. The widening
        // draws below undo the same clips on the device; this is the record of
        // them a later pass cut would rebuild from.
        self.clips.truncate(previous.clips);

        // A scissor is state the recorder holds, so restoring it is an
        // assignment. A stencil clip lives in a buffer on the device, so
        // restoring it is a draw: one covering the whole target per level being
        // left, each stepping back the pixels that reached that level.
        //
        // Deliberately unscissored. The step back lands exactly where the
        // matching narrowing landed, because that is where the stencil holds
        // the value being tested for -- so it needs no help from a scissor, and
        // asking it to agree with one that has already been restored to
        // something else would be a way to get it wrong.
        if self.depth > previous.depth {
            // Stated in device pixels, so it goes through the identity rather
            // than whatever transform happens to be in force.
            let whole = self.target.path();
            while self.depth > previous.depth {
                let paint = RenderPaint {
                    material: Material::solid([1.0, 1.0, 1.0, 1.0]),
                    filter: ColorFilter::None,
                    blend: BlendMode::Src,
                    clip: None,
                    stencil: ClipState::widen(self.depth),
                };
                let _ = self
                    .renderer
                    .fill_into(&mut self.batch, &whole, Affine2::IDENTITY, &paint);
                self.depth -= 1;
            }
        }
        self
    }

    /// How many saves are outstanding, for a caller checking its own balance.
    pub fn save_depth(&self) -> usize {
        self.stack.len()
    }

    pub fn translate(&mut self, x: f32, y: f32) -> &mut Self {
        self.concat(Affine2::from_translation(Vec2::new(x, y)))
    }

    pub fn scale(&mut self, x: f32, y: f32) -> &mut Self {
        self.concat(Affine2::from_scale(Vec2::new(x, y)))
    }

    /// Rotate by an angle in radians.
    pub fn rotate(&mut self, radians: f32) -> &mut Self {
        self.concat(Affine2::from_angle(radians))
    }

    /// Apply an arbitrary transform on top of the current one.
    ///
    /// Takes anything that is a transform of the plane, which is an `Affine2`
    /// for almost every caller and a [`Transform2D`] for one that wants
    /// perspective. Widened rather than replaced: nobody should have to build a
    /// three-by-three to move something ten pixels to the right.
    pub fn concat(&mut self, transform: impl Into<Transform2D>) -> &mut Self {
        self.transform = self.transform * transform.into();
        self
    }

    /// Apply a `dart:ui` four-by-four, which is column-major and sixteen long.
    ///
    /// The entry point for perspective, and the one `dart:ui` states: its
    /// `Canvas.transform` takes a matrix of this shape. Named apart from
    /// [`Self::transform`], which reports the current one and cannot share a
    /// name with a method that changes it.
    ///
    /// The reduction to a three-by-three is exact rather than a narrowing.
    /// Everything drawn here lies on the plane where `z` is zero, and a
    /// four-by-four applied to such a point never reads its `z` column; the `z`
    /// row yields a depth with nothing to do here. What is left -- rows and
    /// columns zero, one and three -- is the whole of what the matrix means on
    /// the plane. A caller whose matrix depends on `z` is getting the part of
    /// it that acts on what they are drawing, which is all of it.
    pub fn concat_4x4(&mut self, matrix: &[f32; 16]) -> &mut Self {
        self.concat(Transform2D::from_column_major_4x4(matrix))
    }

    pub fn draw_path(&mut self, path: &Path, paint: &Paint) -> Result<&mut Self> {
        if !paint.is_visible() || path.is_empty() {
            return Ok(self);
        }
        // Before the mask blur, so the two compose in the order they are
        // defined in: an image filter acts on what was drawn, and what was
        // drawn is whatever the mask blur produced.
        if !paint.image_filter.is_identity() {
            return self.draw_filtered(path, paint);
        }
        // A rounded rectangle that still knows it is one can have its mask blur
        // evaluated rather than run as passes. This is where a shadow arrives:
        // `draw_shadow` takes a path, as `dart:ui`'s `drawShadow` does, so
        // without the shape traveling with the outline every shadow would take
        // the general route however round it is.
        if let Some((bounds, radius)) = path.as_rounded_rect() {
            let rect = Rect::new(bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y);
            if let Some((material, pad, drawn)) = self.analytic_rrect_blur_draw(rect, radius, paint)
            {
                return self.draw_analytic(rect.outset(pad), material, &drawn);
            }
        }
        // A color filter is normally arithmetic in this renderer's own fragment
        // shader, which a runtime effect replaces outright -- there is nowhere
        // in a caller's program to put the matrix, and the program is what runs.
        // Left alone, the filter was accepted and silently did nothing, which
        // is the shape of failure this codebase least wants: no error, no
        // effect, and a caller with no way to tell.
        //
        // So the filter is applied to the image the program drew instead, which
        // is what it means anyway. It costs a layer, which is what every other
        // filter that acts on a finished image already costs.
        if matches!(paint.shader, Shader::RuntimeEffect { .. }) && !paint.color_filter.is_identity()
        {
            return self.draw_effect_filtered(path, paint);
        }
        if paint.mask_blur > 0.0 {
            return self.draw_masked(Masked::Path(path), paint);
        }
        if paint.anti_alias {
            self.anti_alias = true;
        }

        // A clip narrowed to nothing means this shape cannot reach any pixel.
        // Tessellating it to find that out is wasted work, and it is a normal
        // state rather than an error: a subtree scrolled out of view is clipped
        // away every frame it stays there.
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }

        // Resolved before the clip is read, since it may add a slot to this
        // pass's table.
        let material = self.material_for(&paint.shader);
        // A stroke too thin to cover a pixel is widened to one and dimmed to
        // pay for it. See `thin_stroke`; this is the tessellated route, where
        // the quantization it exists for is at its worst -- coverage comes
        // from a multisample buffer, so a sub-pixel line has four coverages
        // available to it and one of them is none.
        let (style, coverage) = match &paint.style {
            Style::Stroke(stroke) => {
                let (width, coverage) =
                    thin_stroke(self.transform, stroke.width, Zero::IsAHairline);
                (Style::Stroke(stroke.with_width(width)), coverage)
            }
            other => (*other, 1.0),
        };
        let render_paint = RenderPaint {
            material: material.with_opacity(coverage),
            filter: paint.color_filter,
            blend: paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        match &style {
            Style::Fill => {
                self.renderer
                    .fill_into(&mut self.batch, path, self.transform, &render_paint)?
            }
            Style::Stroke(stroke) => self.renderer.stroke_into(
                &mut self.batch,
                path,
                stroke,
                paint.dash.as_ref(),
                self.transform,
                &render_paint,
            )?,
        }
        Ok(self)
    }

    /// Resolve a shader against the current transform.
    ///
    /// Gradient endpoints go through the same mapping the geometry does, so a
    /// gradient rotates and scales with its shape rather than staying fixed to
    /// the screen. Doing it here rather than in the fragment stage means the
    /// shader receives clip-space endpoints and needs no transform of its own.
    fn material_for(&mut self, shader: &Shader) -> Material {
        let to_clip = self.target.projection() * self.transform;
        self.material_from(shader, to_clip)
    }

    /// The same, for a draw whose fragments locate themselves some other way.
    ///
    /// A mesh that states a coordinate per vertex has already put each one in
    /// the caller's own space, so the mapping the fragment stage applies must
    /// not carry the geometry's transform as well -- it would be applied twice,
    /// once by the vertices and once by the mapping. Passing the identity here
    /// leaves exactly the paint's own part: the translation that measures from
    /// a gradient's start, the rotation that orients a sweep, and nothing else.
    ///
    /// The shader then runs the same arithmetic on `(uv, 1)` that it runs on a
    /// clip position, which is why there is one path in it rather than two.
    fn material_at_texture_coords(&mut self, shader: &Shader) -> Material {
        self.material_from(shader, Transform2D::IDENTITY)
    }

    fn material_from(&mut self, shader: &Shader, to_clip: Transform2D) -> Material {
        let stops_of = |stops: &[crate::paint::GradientStop]| -> Vec<Stop> {
            stops
                .iter()
                .map(|s| Stop::new(s.color.to_array(), s.offset))
                .collect()
        };

        match shader {
            Shader::Solid(color) => Material::solid(color.to_array()),
            // Passed through untouched. Every other material here is resolved
            // -- geometry carried into clip space, a radius folded into a
            // matrix -- because this renderer's own shader expects it that
            // way. A caller's program expects whatever the caller wrote, and
            // there is nothing here that could resolve it correctly.
            Shader::RuntimeEffect {
                program,
                uniforms,
                images,
            } => Material::Runtime {
                program: *program,
                uniforms: uniforms.clone(),
                // Through this pass's own table, like every other texture: a
                // layer occupies a slot too, so a caller's index and the
                // pass's are not the same number once one is opened.
                textures: {
                    let mut bound = [None; emblema_hal::MAX_EFFECT_TEXTURES];
                    // Past the ceiling the extra names are dropped rather than
                    // refused: a program cannot declare a binding the layout
                    // does not have, so a caller naming more textures than that
                    // has named some the program could not read either.
                    for (slot, into) in images.iter().zip(bound.iter_mut()) {
                        *into = Some(self.slot_for(TextureSource::Image(*slot)));
                    }
                    bound
                },
            },
            Shader::Image {
                slot,
                rect,
                alpha,
                tile,
                source,
                tint,
                sampling,
            } => {
                // Texture coordinates run from zero to one across the
                // destination rectangle, so the mapping is: undo the transform
                // that took user space to clip space, then scale by the
                // rectangle's own size. Composing the two here means the shader
                // receives one matrix and does no inversion of its own.
                //
                // Built forwards and inverted once: the unit square of texture
                // coordinates is scaled by the rectangle's size, placed at its
                // corner, and carried to clip space by the transform in force.
                // A degenerate transform has no inverse to take, and the
                // identity stands in for it -- unobservable, because the
                // geometry went through the same matrix and has no area.
                let placement = to_clip
                    * Affine2::from_translation(Vec2::new(rect.left, rect.top))
                    * Affine2::from_scale(Vec2::new(
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    ));
                Material::Image {
                    to_local: invert_to_local(placement),
                    sampling: *sampling,
                    // The caller's index goes through this pass's own table,
                    // because a layer occupies a slot too and the two number
                    // independently. A recording that never uses a layer maps
                    // them one to one, which is why this is invisible until it
                    // is not.
                    slot: self.slot_for(TextureSource::Image(*slot)),
                    alpha: *alpha,
                    source: [source.left, source.top, source.right, source.bottom],
                    tint: tint.to_array(),
                    tile: *tile,
                }
            }
            Shader::LinearGradient {
                start,
                end,
                stops,
                tile,
            } => {
                let axis = *end - *start;
                if !axis.is_finite() || !start.is_finite() {
                    return Material::Solid([0.0; 4]);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::LinearGradient {
                    axis: [axis.x, axis.y],
                    // Maps a clip-space position back into the space the axis is
                    // stated in, which is the caller's, measured from the
                    // gradient's start. Without it the target's aspect ratio
                    // leaks into the gradient's direction.
                    to_local: invert_to_local(to_clip * Affine2::from_translation(*start)),
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
            Shader::RadialGradient {
                center,
                radius,
                stops,
                tile,
            } => {
                // Folding the center and the radius into the mapping means the
                // shader measures against unit distance from the origin and
                // never sees either.
                let placement = to_clip
                    * Affine2::from_translation(*center)
                    * Affine2::from_scale(Vec2::splat(*radius));
                if !placement.is_finite() {
                    return Material::Solid([0.0; 4]);
                }
                // A radius of nothing folds to a singular mapping, and the
                // inversion below answers a singular matrix with the identity.
                // That is the right answer for an inversion and the wrong one
                // here: it invents a radius of one clip unit, so a gradient the
                // caller asked to have no extent comes out spanning half the
                // target and changing with the target's size.
                //
                // What it should be is the limit of the real thing. As the
                // radius shrinks, every point but the center runs off the end
                // of the ramp -- so under clamp it settles on the last stop,
                // and under decal it leaves, because past the end is where
                // decal draws nothing. Repeat and mirror have no limit at all,
                // the parameter oscillating faster and faster, and they take
                // the same answer as clamp because a stable color is worth
                // more than an arbitrary one that shimmers.
                //
                // The limit rather than a refusal, on the same reasoning that
                // makes a mask blur of zero the sharp shape: a caller animating
                // a radius down to nothing should arrive somewhere, not have
                // the gradient disappear at the last frame.
                if !radius.is_finite() || *radius <= 0.0 {
                    if matches!(tile, TileMode::Decal) {
                        return Material::Solid([0.0; 4]);
                    }
                    let last = stops
                        .iter()
                        .max_by(|a, b| a.offset.total_cmp(&b.offset))
                        .map(|s| s.color.to_array())
                        .unwrap_or([0.0; 4]);
                    return Material::solid(last);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::RadialGradient {
                    to_local: invert_to_local(placement),
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
            Shader::ConicalGradient {
                start_center,
                start_radius,
                end_center,
                end_radius,
                stops,
                tile,
            } => {
                let axis = *end_center - *start_center;
                // The shader is told where the second center is with one float
                // instead of two, which is only possible if it already knows
                // the direction. Rotating the gradient's space so the axis
                // runs along +X is how it comes to know it, and the rotation
                // costs nothing: it folds into a matrix that has to be built
                // and inverted regardless.
                //
                // A zero axis leaves the angle undefined and the rotation
                // arbitrary, which is correct rather than merely harmless --
                // two concentric circles have no direction to preserve, and
                // every rotation is the right one.
                let angle = if axis == Vec2::ZERO {
                    0.0
                } else {
                    axis.y.atan2(axis.x)
                };
                let oriented =
                    to_clip * Affine2::from_translation(*start_center) * Affine2::from_angle(angle);
                if !oriented.is_finite() || !start_radius.is_finite() || !end_radius.is_finite() {
                    return Material::Solid([0.0; 4]);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::ConicalGradient {
                    to_local: invert_to_local(oriented),
                    // In the gradient's own space, which this rotation and the
                    // canvas transform's inverse together make into user space
                    // -- so both radii are the ones the caller stated, and
                    // neither is folded into the matrix the way a radial
                    // gradient's single radius is. A scale can normalize one
                    // radius; it cannot normalize two.
                    start_radius: *start_radius,
                    radius_delta: *end_radius - *start_radius,
                    separation: axis.length(),
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
            Shader::SweepGradient {
                center,
                start_angle,
                end_angle,
                stops,
                tile,
            } => {
                let placement = to_clip * Affine2::from_translation(*center);
                if !placement.is_finite() || !start_angle.is_finite() || !end_angle.is_finite() {
                    return Material::Solid([0.0; 4]);
                }
                let ramp_slot = self.ramp_for(stops);
                Material::SweepGradient {
                    to_local: invert_to_local(placement),
                    start_angle: *start_angle,
                    end_angle: *end_angle,
                    stops: stops_of(stops),
                    tile: *tile,
                    ramp: ramp_slot,
                }
            }
        }
    }

    /// Draw a shape through a blurred layer, which is what a mask blur is.
    ///
    /// Blurring coverage and then filling equals filling and then blurring
    /// exactly when the fill is constant, since a blur is linear. That identity
    /// is the whole implementation: a bounded layer, blurred, with the shape
    /// drawn into it. It also means the machinery is one already tested rather
    /// than a second blur written beside the first.
    ///
    /// The bounds are the shape's, widened by how far the blur reaches. A
    /// caller doing this by hand has to know that reach, which is exactly the
    /// part that is easy to get wrong and shows as a shadow with a straight
    /// edge where it was cut off.
    /// Draw into a layer, filter it, and composite it back.
    ///
    /// The same machinery a mask blur uses, without the restriction: a mask
    /// blur is only the same picture as this when the fill does not vary, and
    /// so takes a solid color, while filtering a result is defined whatever
    /// produced it. What is paid for that is a target of its own.
    /// The layer that applies one filter, where that filter is a leaf.
    ///
    /// `None` for the two that are not: `is_identity` keeps `None` out, and
    /// `peel` recurses through a composition's outer half until it reaches
    /// something that is not one -- so neither can arrive here. They were
    /// reachable when peeling took that half to be a leaf, and composing two
    /// compositions was then refused as unimplemented, having been assembled
    /// out of nothing but implemented filters.
    fn filter_layer(&self, filter: &ImageFilter) -> Option<(Layer, Option<ImageFilter>)> {
        if let ImageFilter::Runtime { program, uniforms } = filter {
            // The layer itself is plain: what makes it a filter is the pass run
            // over it at restore, which needs the program and cannot get it
            // from a `Copy` layer.
            return Some((
                Layer::opacity(1.0),
                Some(ImageFilter::Runtime {
                    program: *program,
                    uniforms: uniforms.clone(),
                }),
            ));
        }
        Some((
            match *filter {
                ImageFilter::Blur { sigma_x, sigma_y } => {
                    Layer::opacity(1.0).with_blur_xy(sigma_x, sigma_y)
                }
                ImageFilter::Matrix { transform } => Layer::opacity(1.0).with_matrix(transform),
                ImageFilter::Dilate { radius_x, radius_y } => {
                    Layer::opacity(1.0).with_morphology(Morphology::dilate(radius_x, radius_y))
                }
                ImageFilter::Erode { radius_x, radius_y } => {
                    Layer::opacity(1.0).with_morphology(Morphology::erode(radius_x, radius_y))
                }
                ImageFilter::Color(filter) => Layer::opacity(1.0).with_color_filter(filter),
                ImageFilter::Runtime { .. } => unreachable!("handled above"),
                ImageFilter::None | ImageFilter::Compose { .. } => return None,
            },
            None,
        ))
    }

    /// A glyph run drawn into a layer, and the layer filtered.
    ///
    /// The third of these, after the shape and the mesh, and the same shape as
    /// both. The bounds come from the run's own boxes, since a run has neither
    /// a path nor vertices to take them from.
    fn draw_glyphs_filtered(
        &mut self,
        glyphs: &[PositionedGlyph],
        atlas: &Atlas,
        atlas_slot: u32,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if paint
            .image_filter
            .nests_deeper_than(ImageFilter::MAX_NESTING)
        {
            return Err(Error::Unsupported(
                "this image filter composes more filters than can be walked",
            ));
        }
        let (outermost, rest) = paint.image_filter.peel();
        let Some((layer, runtime)) = self.filter_layer(&outermost) else {
            return Err(Error::Unsupported("this image filter is not implemented"));
        };
        let (mut min, mut max) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for glyph in glyphs {
            let corner = Vec2::from(glyph.position);
            min = min.min(corner);
            max = max.max(corner + Vec2::from(glyph.size));
        }
        if !min.is_finite() || !max.is_finite() {
            return Err(Error::Unsupported(
                "a glyph in this run is not at a finite position",
            ));
        }
        let (min, max) = transformed_bounds(self.transform, min, max).unwrap_or_else(unbounded);
        let basis = BlurBasis::of(self.transform);
        let (min, max) = rest.covering(min, max, &|sigma| basis.reach(sigma));
        let _ = self.save_layer_device_bounds_running(
            layer.with_blend(paint.blend),
            min,
            max,
            runtime,
            None,
        );
        let inner = paint
            .clone()
            .with_image_filter(rest)
            .with_blend(BlendMode::SrcOver);
        let failure = self.draw_glyphs(glyphs, atlas, atlas_slot, &inner).err();
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// A mesh drawn into a layer, and the layer filtered.
    ///
    /// The path version's twin, and separate rather than shared because the two
    /// differ in the only two places that matter -- where the bounds come from,
    /// and which draw call the remainder of the chain is handed back to. The
    /// peeling, the region arithmetic and the blend placement are the same and
    /// are described there.
    fn draw_vertices_filtered(&mut self, mesh: &Vertices, paint: &Paint) -> Result<&mut Self> {
        if paint
            .image_filter
            .nests_deeper_than(ImageFilter::MAX_NESTING)
        {
            return Err(Error::Unsupported(
                "this image filter composes more filters than can be walked",
            ));
        }
        let (outermost, rest) = paint.image_filter.peel();
        let Some((layer, runtime)) = self.filter_layer(&outermost) else {
            return Err(Error::Unsupported("this image filter is not implemented"));
        };
        // From the vertices themselves. A mesh has no path to take bounds from,
        // and the positions are in the same coordinates a path's points would
        // be, so the transform below is the one that applies to either.
        let positions = mesh.positions();
        let (mut min, mut max) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for point in positions {
            min = min.min(*point);
            max = max.max(*point);
        }
        if !min.is_finite() || !max.is_finite() {
            return Err(Error::Unsupported("a mesh position is not a finite number"));
        }
        let (min, max) = transformed_bounds(self.transform, min, max).unwrap_or_else(unbounded);
        let basis = BlurBasis::of(self.transform);
        let (min, max) = rest.covering(min, max, &|sigma| basis.reach(sigma));
        let _ = self.save_layer_device_bounds_running(
            layer.with_blend(paint.blend),
            min,
            max,
            runtime,
            None,
        );
        let inner = paint
            .clone()
            .with_image_filter(rest)
            .with_blend(BlendMode::SrcOver);
        let failure = self.draw_vertices(mesh, &inner).err();
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// A caller's program drawn into a layer, and the layer recolored.
    ///
    /// The bounds are the path's own, widened by a stroke where there is one.
    /// Nothing here reaches past that: a color filter moves colors rather than
    /// pixels, so the region it can affect is the region that was drawn.
    fn draw_effect_filtered(&mut self, path: &Path, paint: &Paint) -> Result<&mut Self> {
        let bounds = self.filter_bounds(path, paint);
        let (min, max) = transformed_bounds(
            self.transform,
            Vec2::new(bounds.left, bounds.top),
            Vec2::new(bounds.right, bounds.bottom),
        )
        .unwrap_or_else(unbounded);
        self.save_layer_device_bounds(
            Layer::opacity(1.0)
                .with_color_filter(paint.color_filter)
                // The blend belongs to the composite, not to the draw inside.
                // That is what `saveLayer` means by taking a paint, and it is
                // the only placement that gives the right answer for a blend
                // reading its destination: inside the layer the destination is
                // transparent black, so a mode like `Multiply` applied there
                // would come out empty and then be composited over the frame it
                // was supposed to darken.
                .with_blend(paint.blend),
            min,
            max,
        );
        // Without the filter, or this would open a layer inside itself forever,
        // and over an empty layer rather than through the caller's blend, which
        // the composite above now carries.
        let inner = paint
            .clone()
            .with_color_filter(ColorFilter::None)
            .with_blend(BlendMode::SrcOver);
        let failure = self.draw_path(path, &inner).err();
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    fn draw_filtered(&mut self, path: &Path, paint: &Paint) -> Result<&mut Self> {
        // One filter is peeled off here and the rest is handed back to
        // `draw_path`, which lands in this method again if anything is left.
        // A chain of filters is a stack of layers, and this builds the stack
        // one frame at a time rather than all at once -- which keeps the
        // single-filter case exactly what it was, with the remainder `None`.
        if paint
            .image_filter
            .nests_deeper_than(ImageFilter::MAX_NESTING)
        {
            return Err(Error::Unsupported(
                "this image filter composes more filters than can be walked",
            ));
        }
        let (outermost, rest) = paint.image_filter.peel();
        let Some((layer, runtime)) = self.filter_layer(&outermost) else {
            return Err(Error::Unsupported("this image filter is not implemented"));
        };
        let bounds = self.filter_bounds(path, paint);
        // In device pixels, because that is where a filter's reach is measured.
        // The outer layer has to cover everything the rest of the chain needs,
        // both what goes into it and what comes out -- a layer's target is
        // clipped to its parent's, so a target sized for the result alone would
        // crop the content before the inner filter ever ran.
        let (min, max) = transformed_bounds(
            self.transform,
            Vec2::new(bounds.left, bounds.top),
            Vec2::new(bounds.right, bounds.bottom),
        )
        .unwrap_or_else(unbounded);
        let basis = BlurBasis::of(self.transform);
        let (min, max) = rest.covering(min, max, &|sigma| basis.reach(sigma));
        // The caller's blend rides the outermost composite. Left on the draw
        // inside, it was applied against the layer's own transparent black --
        // so a mode that reads its destination found nothing there, and the
        // result was composited over the frame it was meant to combine with.
        // `Plus` over a cyan ground gave the source unchanged instead of the
        // sum, for every image filter, since filters were added.
        //
        // Only the outermost, which is why the inner paint is neutral: peeling
        // a chain opens a layer per link, and a blend carried down would be
        // applied once per link rather than once.
        let _ = self.save_layer_device_bounds_running(
            layer.with_blend(paint.blend),
            min,
            max,
            runtime,
            None,
        );
        // The mask blur, if there is one, is left on: it applies to the drawing
        // this filter is filtering.
        let inner = paint
            .clone()
            .with_image_filter(rest)
            .with_blend(BlendMode::SrcOver);
        let failure = self.draw_path(path, &inner).err();
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// A path's bounds, widened by however far a stroke reaches past it.
    ///
    /// How far a blur reaches is deliberately not added: opening a bounded
    /// layer with a blur already widens the region by that, and the layer is
    /// the right place for it, since it knows its own sigma.
    ///
    /// The stroke's own reach was untested for a while, because that blur
    /// widening covers any stroke whose half-width is under the blur's own
    /// reach, and every test had one. It takes a wide stroke and a small blur to tell the two
    /// apart, and there is one now.
    fn filter_bounds(&self, path: &Path, paint: &Paint) -> Rect {
        let bounds = path.bounds();
        let reach = match &paint.style {
            // The width the stroke is drawn at, which for a sub-pixel one is
            // wider than the width it asked for. A bound taken from the asked
            // width would cut the widening off at the edge of the layer.
            Style::Stroke(stroke) if stroke.width.is_finite() => {
                thin_stroke(self.transform, stroke.width, Zero::IsAHairline).0 / 2.0
            }
            _ => 0.0,
        };
        Rect::new(
            bounds.min.x - reach,
            bounds.min.y - reach,
            bounds.max.x + reach,
            bounds.max.y + reach,
        )
    }

    /// What a mask blur is blurring.
    ///
    /// A mask blur draws its content two or three times -- once into a blurred
    /// layer, sometimes again at full sharpness to combine with it -- and every
    /// style needs the same content each time. Naming the content rather than
    /// passing a path is what lets a glyph run take the same four styles a
    /// shape does, which matters because a text shadow is a mask blur over a
    /// run and is how the common case of shadowed text is drawn.
    fn draw_mask_content(&mut self, content: Masked<'_>, paint: &Paint) -> Result<&mut Self> {
        match content {
            Masked::Path(path) => self.draw_path(path, paint),
            Masked::Mesh(mesh) => self.draw_vertices(mesh, paint),
            Masked::Glyphs {
                glyphs,
                atlas,
                slot,
            } => self.draw_glyphs(glyphs, atlas, slot, paint),
        }
    }

    /// The bounds of what a mask blur is blurring, in the canvas's own units.
    fn mask_bounds(&self, content: Masked<'_>, paint: &Paint) -> Rect {
        match content {
            Masked::Path(path) => self.filter_bounds(path, paint),
            Masked::Mesh(mesh) => {
                let (mut min, mut max) =
                    (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
                for position in mesh.positions() {
                    min = min.min(*position);
                    max = max.max(*position);
                }
                Rect::new(min.x, min.y, max.x, max.y)
            }
            Masked::Glyphs { glyphs, .. } => {
                let (mut min, mut max) =
                    (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
                for glyph in glyphs {
                    let corner = Vec2::from(glyph.position);
                    min = min.min(corner);
                    max = max.max(corner + Vec2::from(glyph.size));
                }
                Rect::new(min.x, min.y, max.x, max.y)
            }
        }
    }

    /// A mask blur over a fill that varies, built the way `dart:ui` states it.
    ///
    /// The path below draws the paint through a blurred layer, which is the
    /// same picture only where the fill does not vary. This is the other order
    /// and the one a mask filter actually means: blur the shape's coverage,
    /// then fill through it. It needs no machinery that was not already here --
    /// the fill goes down across everything the blur reaches, the coverage is
    /// blurred in a layer of its own, and `DstIn` composites the second onto
    /// the first as a mask.
    ///
    /// Only a path. A glyph run reaches this call too, and a run tints one
    /// color by its own nature rather than by anything to do with blurring, so
    /// a gradient over one is refused here exactly as it is refused when
    /// nothing is blurred at all.
    fn draw_masked_through_coverage(
        &mut self,
        content: Masked<'_>,
        paint: &Paint,
    ) -> Result<&mut Self> {
        let bounds = self.mask_bounds(content, paint);
        // Wide enough for whichever blur this content's route will draw.
        let held = bounds.outset(self.mask_blur_reach(content, paint));
        // White, because what is wanted from the shape here is its coverage
        // rather than its color: the fill supplies the color and this supplies
        // where it lands.
        let coverage = Paint::fill(Color::WHITE).with_anti_alias(paint.anti_alias);
        // This one floods the held region with the caller's paint, so it has to
        // be a *fill* whatever the caller asked for. Left as the caller's own
        // style it drew the outline of the held rectangle when the caller was
        // stroking, and the mask below then cut that away to nothing -- the
        // rectangle's border is nowhere near the shape. A stroked shape with a
        // fill that varies came out empty, on both backends.
        //
        // Only the flood needs it. Handing the coverage the caller's style as
        // well changes no pixel of any plate here, so something other than this
        // line is already giving the mask the band rather than the region, and
        // a line that changes nothing does not go in with a comment saying it
        // does.
        let fill = paint
            .clone()
            .with_mask_blur(0.0)
            .with_blend(BlendMode::SrcOver)
            .with_style(Style::Fill);

        self.save_layer_bounds(Layer::opacity(1.0).with_blend(paint.blend), held);
        // Across everything the blur reaches, not across the shape: a blurred
        // mask is wider than what it was made from, and a fill stopping at the
        // shape's own edge would cut the halo off square.
        let failure = self.draw_rect(held, &fill).err().or_else(|| {
            if paint.mask_blur_style == MaskBlurStyle::Normal {
                // The blurred coverage is the whole mask, so the layer that
                // carries it can be the layer that blurs it.
                let mask = Layer::opacity(1.0)
                    .with_blur(paint.mask_blur)
                    .with_blend(BlendMode::DstIn);
                self.save_layer_bounds(mask, held);
                let inner = self.draw_mask_content(content, &coverage).err();
                self.restore();
                return inner;
            }
            // The other three combine the blurred coverage with the sharp one,
            // and the combination is not itself blurred -- so this layer
            // carries no blur, and the blur happens inside it on the operand
            // that wants it.
            self.save_layer_bounds(Layer::opacity(1.0).with_blend(BlendMode::DstIn), held);
            let inner = self.draw_mask_styles(
                content,
                &coverage,
                paint.mask_blur,
                paint.mask_blur_style,
                bounds,
            );
            self.restore();
            inner
        });
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    fn draw_masked(&mut self, content: Masked<'_>, paint: &Paint) -> Result<&mut Self> {
        if !matches!(paint.shader, Shader::Solid(_)) {
            if !matches!(content, Masked::Glyphs { .. }) {
                return self.draw_masked_through_coverage(content, paint);
            }
            // A glyph run, and it is the only thing left here. A run tints one
            // color by its own nature rather than by anything to do with
            // blurring, so a varying fill over one is refused exactly as it is
            // refused when nothing is blurred at all.
            //
            // See `Paint::mask_blur`: drawing the paint through a blurred layer
            // is the other order, and for anything that varies the two are
            // different pictures. Drawing one while the caller asked for the
            // other is the substitution this renderer refuses
            // elsewhere.
            return Err(Error::Unsupported(
                "a mask blur over a glyph run takes a solid color; \
                 draw into a blurred layer for anything else",
            ));
        }
        let bounds = self.mask_bounds(content, paint);
        // Without the mask, or this would open a layer inside itself forever --
        // and with the caller's blend taken off, because it belongs to the
        // composite that puts the finished mask on the frame rather than to a
        // draw inside a layer that starts empty. Left on, a mode reading its
        // destination found transparent black there, and `Plus` over a cyan
        // ground gave the source unchanged instead of the sum.
        //
        // The style's own blends below are a different thing and stay: they
        // combine the shape with its blur *within* the layer, which is exactly
        // where a destination-reading mode is supposed to look.
        let inner = paint
            .clone()
            .with_mask_blur(0.0)
            .with_blend(BlendMode::SrcOver);

        // The blurred coverage alone is the whole picture for the default
        // style, so it needs one layer and no second draw.
        if paint.mask_blur_style == MaskBlurStyle::Normal {
            self.save_layer_bounds(
                Layer::opacity(1.0)
                    .with_blur(paint.mask_blur)
                    .with_blend(paint.blend),
                bounds,
            );
            // The result is discarded to end the borrow before restoring, and
            // taken up again after: the layer has to be closed whether the
            // draw inside it succeeded or not, or every later draw lands in a
            // layer nobody composites.
            let failure = self.draw_mask_content(content, &inner).err();
            self.restore();
            return match failure {
                Some(e) => Err(e),
                None => Ok(self),
            };
        }

        // Every other style combines the blurred coverage with the shape's
        // own. That combination is the same whether what is being combined is
        // color or coverage, so it lives in `draw_mask_styles` and both routes
        // through here call it.
        // Wide enough for whichever blur this content's route will draw.
        let held = bounds.outset(self.mask_blur_reach(content, paint));
        self.save_layer_bounds(Layer::opacity(1.0).with_blend(paint.blend), held);
        let failure = self.draw_mask_styles(
            content,
            &inner,
            paint.mask_blur,
            paint.mask_blur_style,
            bounds,
        );
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// How far past its bounds a mask blur on this content will draw.
    ///
    /// The two routes reach different distances and the layer holding them has
    /// to fit whichever is used: a sampled blur stops at `blur_reach`, about
    /// 1.73 deviations, and an evaluated one carries to `pad_for_sigma`, about
    /// 2.5. Sized for the first, an evaluated blur is cut off square; sized for
    /// the second regardless, every *sampled* blur pays for a transient about
    /// twice the area to hold a halo it will never draw.
    ///
    /// So it asks which route the content will take, rather than taking the
    /// larger of the two and calling it safe.
    fn mask_blur_reach(&mut self, content: Masked<'_>, paint: &Paint) -> f32 {
        let sampled = blur_reach(paint.mask_blur);
        let probe = paint.clone().with_mask_blur_style(MaskBlurStyle::Normal);
        match self.analytic_style_blur(content, &probe, paint.mask_blur) {
            Some((_, _, pad)) => sampled.max(pad),
            None => sampled,
        }
    }

    /// The evaluated blur for a style's blurred ingredient, where one applies.
    ///
    /// The styles other than `Normal` combine a blurred shape with its sharp
    /// self, and the blurred half is the same thing `Normal` draws — so where
    /// `Normal` would be evaluated rather than sampled, this half can be too.
    /// `inner` is the paint with the blur already taken off, which is what the
    /// styles draw with, so the deviation is handed back in.
    ///
    /// Returns the rectangle the draw covers as well as the material, because
    /// the caller has to widen its layer to hold it: an evaluated blur reaches
    /// `pad_for_sigma`, which is further than the `blur_reach` a sampled one is
    /// truncated at, and a layer sized for the sampled reach cuts it.
    fn analytic_style_blur(
        &mut self,
        content: Masked<'_>,
        inner: &Paint,
        sigma: f32,
    ) -> Option<(Rect, Material, f32)> {
        let Masked::Path(path) = content else {
            return None;
        };
        let (bounds, radius) = path.as_rounded_rect()?;
        let rect = Rect::new(bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y);
        // Asked for as `Normal`, whatever style the caller wants: the blurred
        // half *is* the normal blur, and the style is the rule for combining it
        // with the sharp shape rather than a property of the blur. Left as the
        // caller's, the eligibility check refuses it -- which is right when the
        // question is "may this whole draw be one expression" and wrong here.
        let asked = inner
            .clone()
            .with_mask_blur(sigma)
            .with_mask_blur_style(MaskBlurStyle::Normal);
        let (material, pad) = self.analytic_rrect_blur(rect, radius, &asked)?;
        Some((rect, material, pad))
    }

    /// Combine a shape's blurred coverage with its sharp own, per style.
    ///
    /// Draws into whatever layer the caller has already opened, and that layer
    /// is not optional: drawn straight onto the target, a blend that reads the
    /// destination would reach what was already there rather than only what
    /// this is building.
    ///
    /// What is being combined is the caller's business. Given the paint the
    /// shape is filled with, this composes colors and is the whole picture;
    /// given white, it composes coverage, and the caller masks a varying fill
    /// through the result. The rules are identical either way, which is why
    /// there is one of these rather than two.
    ///
    /// Which of the two operands is drawn first is not a matter of taste. A
    /// blend only runs where its source produces a fragment, and the shape
    /// produces none outside itself -- so a rule that has to *remove*
    /// something outside the shape cannot be written with the shape as the
    /// source. The blurred layer composites as a quad over the whole region, so
    /// it is the operand that can act everywhere, and the two rules needing
    /// that are the two where the shape goes down first.
    fn draw_mask_styles(
        &mut self,
        content: Masked<'_>,
        inner: &Paint,
        sigma: f32,
        style: MaskBlurStyle,
        bounds: Rect,
    ) -> Option<Error> {
        // Not itself blurred, so nothing widens it on its behalf.
        let blurred = Layer::opacity(1.0).with_blur(sigma);
        match style {
            // Blur first, then the shape over it. `SrcOver` leaves the blur
            // where the shape is not, and `DstOut` takes the shape out of it
            // -- both of which want the destination untouched outside the
            // shape, which is what a source that draws nothing there gives.
            MaskBlurStyle::Solid | MaskBlurStyle::Outer => {
                let blend = match style {
                    MaskBlurStyle::Solid => BlendMode::SrcOver,
                    _ => BlendMode::DstOut,
                };
                // The blurred half, evaluated where it can be. These two
                // styles draw it first with the layer's own blend, so nothing
                // has to be relaxed to allow it -- unlike `Inner`, whose blur
                // is composited with `DstIn` and needs coverage this cannot
                // give from a quad.
                let first = match self.analytic_style_blur(content, inner, sigma) {
                    Some((rect, material, pad)) => {
                        self.draw_analytic(rect.outset(pad), material, inner).err()
                    }
                    None => {
                        self.save_layer_bounds(blurred, bounds);
                        let failed = self.draw_mask_content(content, inner).err();
                        self.restore();
                        failed
                    }
                };
                first.or_else(|| {
                    self.draw_mask_content(content, &inner.clone().with_blend(blend))
                        .err()
                })
            }
            // The shape first, and the blur composited onto it with `DstIn`.
            // The other order is the obvious one and is wrong: outside the
            // shape there is no fragment for `DstIn` to run on, so the blur
            // survives exactly where this style is supposed to discard it.
            // Compositing a layer covers the whole region, so putting the
            // blur on that side is what makes the rule act everywhere.
            MaskBlurStyle::Inner => {
                let first = self.draw_mask_content(content, inner).err();
                if first.is_some() {
                    return first;
                }
                // `DstIn` is the one blend the analytic route refuses
                // everywhere else, because it changes the destination where the
                // source drew nothing and a quad is smaller than the region a
                // layer composite covers. That argument does not reach here,
                // and the reason is specific rather than a relaxation: the only
                // thing in this layer is the sharp shape drawn a line above,
                // the shape is a fill and so lies within `rect`, and the quad
                // is `rect` grown by the blur's reach. Everything the blend
                // could have to zero is under the quad; outside it the layer is
                // still transparent, where zeroing and leaving alone are the
                // same. So the eligibility is asked about the blur -- with
                // `SrcOver`, since that is what the question means -- and the
                // blend is put on at the draw.
                match self.analytic_style_blur(content, inner, sigma) {
                    Some((rect, material, pad)) => {
                        let composite = inner.clone().with_blend(BlendMode::DstIn);
                        self.draw_analytic(rect.outset(pad), material, &composite)
                            .err()
                    }
                    None => {
                        self.save_layer_bounds(blurred.with_blend(BlendMode::DstIn), bounds);
                        let second = self.draw_mask_content(content, inner).err();
                        self.restore();
                        second
                    }
                }
            }
            MaskBlurStyle::Normal => unreachable!("handled by the caller"),
        }
    }

    /// Fill or stroke a rectangle.
    ///
    /// **A fill is tessellated.** It used to take the same distance field a rounded
    /// rectangle does, with no corner to round, on the reasoning that a shape
    /// computing its own coverage spares the pass multisampling. That was measured
    /// backwards on 2026-09-30: the field is the dearer route by about thirty per
    /// cent. On the `stacked interface` frame, tessellated at four samples against
    /// analytic at one -- 16.077 ms against 24.508 on a Pi 5, 66.100 against 93.981
    /// on a VisionFive 2, 1.786 against 2.758 on x86-64. Three architectures, one
    /// direction. `docs/on-a-board.md` has the conditions, the control and the
    /// arithmetic.
    ///
    /// The arithmetic the old comment had wrong: multisampling shades once per pixel
    /// and pays in attachment bandwidth and a resolve -- 1.23x on V3D, 2.62x on
    /// PowerVR, not 4x -- while the field costs an evaluation per fragment across a
    /// quad, and for a rectangle that field evaluates to a constant.
    ///
    /// It is also what upstream does. `FillRectGeometry::GetPositionBuffer`, read at
    /// `flutter/flutter` master on 2026-10-01, emits a four-vertex triangle strip
    /// under `Mode::kNormal`; there is no fragment-evaluated rectangle fill there at
    /// all. So the flip closes a divergence rather than opening one, and the edge a
    /// rectangle gets here is now the edge upstream gives it.
    ///
    /// **A stroke stays on the field.** What was measured is a fill, the stroke route
    /// is careful about a corner the field would otherwise round -- see
    /// `analytic_rrect` -- and nothing here has measured what that costs.
    pub fn draw_rect(&mut self, rect: Rect, paint: &Paint) -> Result<&mut Self> {
        if rect.is_empty() {
            return Ok(self);
        }
        if matches!(paint.style, Style::Stroke(_)) {
            if let Some((material, to_local)) = self.analytic_rrect(rect, 0.0, paint) {
                return self.draw_analytic_local(rect, material, paint, to_local);
            }
        }
        let path = rect.to_path();
        self.draw_path(&path, paint)
    }

    /// Fill or stroke a rectangle with rounded corners.
    ///
    /// The shape an interface is mostly made of, and the reason it is here
    /// rather than left to the caller: building it from arcs by hand is a dozen
    /// lines that have to get the corner tangents right, and getting them
    /// slightly wrong shows as a corner that is subtly not round.
    ///
    /// Analytic for a solid fill or stroke that asked for antialiasing, which is
    /// what `analytic_rrect` admits. The field is implemented for this shape, a
    /// plain rectangle, a circle and an oval.
    ///
    /// Tessellation is the fallback: a gradient or image fill, an aliased fill, or
    /// a square-cornered stroke goes to `draw_path`.
    pub fn draw_rrect(&mut self, rect: Rect, radius: f32, paint: &Paint) -> Result<&mut Self> {
        if rect.is_empty() {
            return Ok(self);
        }
        // Before the sharp field, because that one refuses a mask blur and this
        // is what answers it: the blur is folded into the expression rather
        // than run as passes around the draw.
        if let Some((material, pad, drawn)) = self.analytic_rrect_blur_draw(rect, radius, paint) {
            return self.draw_analytic(rect.outset(pad), material, &drawn);
        }
        if let Some((material, to_local)) = self.analytic_rrect(rect, radius, paint) {
            return self.draw_analytic_local(rect, material, paint, to_local);
        }
        let path = rect.to_rounded_path(radius);
        self.draw_path(&path, paint)
    }

    /// A rounded rectangle whose corners need not match.
    ///
    /// The eight numbers `dart:ui`'s `RRect` carries, in the order upstream's
    /// `RoundingRadii` carries them: top-left, top-right, bottom-left,
    /// bottom-right, each an x radius and a y radius. Radii that overrun the
    /// side they share are fitted by `dart:ui`'s rule -- see
    /// [`Rect::to_rounded_path_with_radii`].
    ///
    /// Tessellated, always. The fragment-evaluated route [`Self::draw_rrect`]
    /// can take is a signed distance to a shape with one circular radius, and
    /// eight numbers is a different function rather than that one with more
    /// arguments -- so a corner that differs from its neighbors costs the
    /// tessellation a uniform one avoids. That is the whole of what this costs,
    /// and it is why the uniform call is still the one to reach for: it is not
    /// a worse spelling of this, it is the case that has a shader.
    pub fn draw_rrect_with_radii(
        &mut self,
        rect: Rect,
        radii: RoundingRadii,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if rect.is_empty() {
            return Ok(self);
        }
        // A caller who spells a uniform shape this way still gets the shader,
        // which matters because the general call is the one a `dart:ui` port
        // reaches for and most of what it is handed is uniform.
        if let Some(radius) = uniform_circular_radius(radii) {
            return self.draw_rrect(rect, radius, paint);
        }
        let path = rect.to_rounded_path_with_radii(radii);
        self.draw_path(&path, paint)
    }

    /// Draw Flutter's rounded superellipse -- the iOS-style squircle.
    ///
    /// `dart:ui`'s `drawRSuperellipse`. The corners are not arcs of one curve:
    /// each is a superellipse arc, a circular arc across the diagonal, and a
    /// second superellipse arc, with the superellipse's degree read from a
    /// fitted table on the ratio of side to radius. See
    /// [`emblema_geometry::superellipse`] for what that means and for what
    /// checks the transcription of the table.
    ///
    /// Tessellated rather than evaluated in the fragment stage. A rounded
    /// rectangle of a single radius takes the analytic route because there is
    /// a closed form to evaluate; this shape has none, which is the same
    /// reason it took so long to arrive.
    pub fn draw_rsuperellipse(
        &mut self,
        rect: Rect,
        radii: RoundingRadii,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if rect.is_empty() {
            return Ok(self);
        }
        self.draw_path(&rsuperellipse_path(rect, radii), paint)
    }

    /// Clip to Flutter's rounded superellipse.
    ///
    /// `dart:ui`'s `clipRSuperellipse`, and the same shape
    /// [`Canvas::draw_rsuperellipse`] draws.
    pub fn clip_rsuperellipse(&mut self, rect: Rect, radii: RoundingRadii) -> Result<&mut Self> {
        self.clip_path(&rsuperellipse_path(rect, radii))
    }

    /// How far past the shape a blurred rounded rectangle has to draw.
    ///
    /// Wider than `blur_reach`, and deliberately: that is where a *sampled*
    /// Gaussian's kernel is truncated, and this expression has no kernel to
    /// truncate. It is evaluated everywhere and falls off smoothly, so what
    /// bounds it is where the falloff stops being visible rather than where the
    /// taps stop. Upstream's `PadForSigma`, whose comment records that three
    /// deviations was tried and left a cutoff on large blurs.
    fn pad_for_sigma(sigma: f32) -> f32 {
        sigma * (sigma / 47.6 + 2.5).min(3.5)
    }

    /// The gate above, with the one substitution upstream makes, and the paint
    /// to draw the result with.
    ///
    /// Three shapes reach the evaluated blur -- a rounded-rectangle path,
    /// `draw_rrect` and `draw_circle` -- and all three share this. The fourth
    /// caller of [`Self::analytic_rrect_blur`] deliberately does not: it is
    /// building the blurred *ingredient* of a style that combines it with the
    /// sharp shape inside a layer, and a substitution good for a whole draw is
    /// not good for one operand of a combination.
    fn analytic_rrect_blur_draw(
        &mut self,
        rect: Rect,
        radius: f32,
        paint: &Paint,
    ) -> Option<(Material, f32, Paint)> {
        let drawn = Self::erasing_blur_paint(paint);
        let (material, pad) = self.analytic_rrect_blur(rect, radius, &drawn)?;
        Some((material, pad, drawn))
    }

    /// `Clear` on a blurred shape, expressed as a mode that respects coverage.
    ///
    /// The gate below refuses a mode that writes where its source is
    /// transparent, and it is right to: the field is evaluated across a quad
    /// larger than the shape, so such a mode erases what is behind it in the
    /// gap. `Clear` is the one mode that can be admitted anyway, because on a
    /// coverage it is not what its factors say. Clearing by an amount `c` is
    /// `dst * (1 - c)`, which is `DstOut` against a white source -- and white
    /// is exact rather than approximate, since `Clear` discards the source
    /// color by definition and cannot care which one it had.
    ///
    /// This is upstream's own special case, in upstream's own one place.
    /// `SolidRRectLikeBlurContents::Render` checks for `BlendMode::kClear`,
    /// forces the color to white, and sets a flag that turns the pipeline's
    /// blend into a reverse subtraction -- destination factor `One`, source
    /// factor `DestinationColor`, so `dst - src * dst`. The arithmetic is the
    /// same; upstream reaches it by subtraction because its fragment writes
    /// coverage directly, and this reaches it by naming the mode that already
    /// means it.
    ///
    /// Only `Clear`. Upstream does not generalize this and neither does this:
    /// the other six modes that ignore coverage have no such reading and stay
    /// on the route that gets them wrong, which is `docs/non-parity.md`
    /// section 15.
    fn erasing_blur_paint(paint: &Paint) -> Paint {
        if paint.blend != BlendMode::Clear {
            return paint.clone();
        }
        paint
            .clone()
            .with_shader(Shader::Solid(Color::WHITE))
            .with_blend(BlendMode::DstOut)
            // Dropped rather than carried, because it provably cannot matter:
            // a color filter transforms the source color and `Clear` discards
            // it. Carried into the substitution it would stop being a no-op
            // and start changing how much is erased.
            .with_color_filter(ColorFilter::None)
    }

    /// Draw a blurred rounded rectangle without a blur pass, if this one can be.
    ///
    /// The route upstream takes for the same shape, and the reason it is worth
    /// taking is that the general one costs three passes per shape where this
    /// costs one draw in the pass already open.
    ///
    /// `analytic_stroke` cannot answer this one, and the difference is the
    /// point: it refuses a mask blur outright, because a sharp distance field
    /// on an oversized quad cannot be put inside a layer and blurred. That is
    /// exactly the refusal this route lifts, by folding the blur into the field
    /// rather than wrapping the draw in one. Its other refusals do carry over
    /// and are restated here.
    /// Returns the material and how far past the rectangle it must be drawn,
    /// both in the shape's own space.
    fn analytic_rrect_blur(
        &mut self,
        rect: Rect,
        radius: f32,
        paint: &Paint,
    ) -> Option<(Material, f32)> {
        if !paint.is_visible() || self.clip.is_some_and(Scissor::is_empty) {
            return None;
        }
        // This emits a fragment across the whole padded quad, which is much
        // larger than the shape, so a mode that touches the destination where
        // the source is transparent would erase what is behind it in the gap.
        if !paint.blend.respects_coverage() {
            return None;
        }
        // An image filter wants a layer around the draw, and a quad larger than
        // its shape is the wrong thing to put in one.
        if !paint.image_filter.is_identity() {
            return None;
        }
        // A fill only. A blurred *stroke* is the blur of a band rather than of
        // the shape: a different picture, and not one this expression states.
        // A dash is refused with it, since there is no stroke to dash.
        if !matches!(paint.style, Style::Fill) {
            return None;
        }
        // The blurred coverage itself. The other three styles are that
        // combined with the sharp shape, which the general route assembles out
        // of two draws and this cannot say in one.
        if paint.mask_blur_style != MaskBlurStyle::Normal {
            return None;
        }
        let Shader::Solid(color) = &paint.shader else {
            return None;
        };
        if radius.is_nan() || radius < 0.0 {
            return None;
        }
        // `is_finite` first, so a sigma that is not a number is refused rather
        // than compared: every comparison against NaN is false, so a bare
        // `<= 0.0` would let it through.
        if !paint.mask_blur.is_finite() || paint.mask_blur <= 0.0 {
            return None;
        }
        // The deviation is in device pixels -- see `non-parity.md` on a shadow's
        // elevation -- and everything below is in the shape's own space, so it
        // has to be carried across. Getting this wrong is invisible at the
        // identity and doubles the blur's tail on a canvas scaled by two.
        let affine = self.transform.to_affine()?;
        let sx = affine.matrix2.x_axis.length();
        let sy = affine.matrix2.y_axis.length();
        if !sx.is_finite() || !sy.is_finite() || sx <= 0.0 || sy <= 0.0 {
            return None;
        }
        // One deviation cannot describe a blur that is wider along one axis
        // than the other, and this expression carries exactly one. A transform
        // that scales the axes differently goes to the general route, which
        // blurs in device space and does not care.
        if (sx - sy).abs() > 1e-3 * sx.max(sy) {
            return None;
        }
        // Stated in the shape's own space already, so there is nothing to
        // convert. It used to be `paint.mask_blur / sx`, which is what a
        // device-space sigma needed and what made a blur refuse to grow with
        // the transform.
        let sigma = paint.mask_blur;

        let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
        let material = self.rrect_blur_material(rect, radius, sigma, *color)?;
        Some((material, Self::pad_for_sigma(sigma)))
    }

    /// The material for a blurred rounded rectangle drawn without a blur pass.
    ///
    /// Raph Levien's approximation, transcribed from upstream's
    /// `SolidRRectLikeBlurContents::PopulateFragContext`, which evaluates the
    /// same method. Every line is arithmetic the shader would otherwise repeat
    /// per fragment, and none of it is this renderer's invention -- the
    /// constants are upstream's, and where one looks arbitrary it is because
    /// it was fitted rather than derived.
    ///
    /// `rect` and `radius` are in the shape's own space, `sigma` in device
    /// pixels, as everywhere else here.
    fn rrect_blur_material(
        &self,
        rect: Rect,
        radius: f32,
        sigma: f32,
        color: Color,
    ) -> Option<Material> {
        // Below one, the approximation is sharper than the pixel grid can show
        // and the error function saturates; upstream floors it in the same
        // place. The root of two is the conversion between the deviation this
        // renderer states and the one the expression below is written in.
        let sigma = (sigma * std::f32::consts::SQRT_2).max(1.0);
        let mut size = Vec2::new(rect.width(), rect.height());
        if !size.x.is_finite() || !size.y.is_finite() || !sigma.is_finite() {
            return None;
        }
        if size.x <= 0.0 || size.y <= 0.0 {
            return None;
        }
        let min_edge = size.x.min(size.y);
        let r_max = 0.5 * min_edge;

        // Two corner radii rather than the caller's one: a blurred corner is
        // rounder than the shape's, and how much rounder depends on the
        // deviation. Their ratio becomes the exponent, so a sharp corner under
        // a wide blur is measured with a larger exponent than a round one.
        let r0 = radius.hypot(sigma * 1.15).min(r_max);
        let r1 = radius.hypot(sigma * 2.0).min(r_max);
        let exponent = 2.0 * r1 / r0;
        let s_inv = 1.0 / sigma;

        // Pull the long end in. A rectangle much longer than it is wide blurs
        // to something the axis-wise expression makes too eccentric, and this
        // shortens the long axis by an amount that vanishes as either side
        // grows past the deviation -- which is why it is a pair of Gaussians
        // of the sides rather than a ratio.
        let falloff = |v: f32| (-(v * s_inv * 0.5).powi(2)).exp();
        let delta = 1.25 * sigma * (falloff(size.x) - falloff(size.y));
        // Whichever axis is longer is the one shortened, which is a deliberate
        // departure from upstream and the only one in this expression.
        // Upstream writes `rSize += NegPos(delta)`, which is
        // `{min(delta, 0), max(delta, 0)}` -- so it shortens x when x is the
        // long axis and *lengthens* y when y is, leaving the same rectangle
        // blurred to two different widths depending on which way it is turned.
        // Its own comment says "pull in long end", which is what this does in
        // both orientations. See `non-parity.md`.
        size.x += delta.min(0.0);
        size.y += (-delta).min(0.0);

        let adjust = size * 0.5 - Vec2::splat(r1);
        // Normalizes the fade, so the middle of a shape large against its blur
        // reaches full coverage rather than something near it.
        let scale = 0.5 * erf7(s_inv * 0.5 * (size.x.max(size.y) - 0.5 * radius));

        let center = Vec2::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        );
        let to_clip = self.target.projection() * self.transform;
        let material = Material::RoundedRectBlur {
            color: color.to_array(),
            to_local: invert_to_local(to_clip * Affine2::from_translation(center)),
            adjust: [adjust.x, adjust.y],
            r1,
            exponent,
            s_inv,
            min_edge,
            scale,
        };
        // A field that is not a number puts NaN through every fragment, and a
        // NaN coverage is a shape that vanishes or a frame that does. Upstream
        // checks the same eight and declines the same way.
        [adjust.x, adjust.y, r1, exponent, s_inv, min_edge, scale]
            .iter()
            .all(|v| v.is_finite())
            .then_some(material)
    }

    /// A paint for the fragment-evaluated rounded rectangle, where one applies.
    ///
    /// A solid-colored fill or stroke that asked for antialiasing. The material
    /// carries a stroke width and an outer radius, and the bench times the stroked
    /// case as `stroked field`.
    ///
    /// Refused: a gradient or an image, which would need its own mapping and this
    /// one at once -- the case the push-constant budget was sized against; an
    /// aliased fill, which asks for the hard edges the tessellated path gives; and
    /// a square-cornered stroked rectangle, for the reason stated where the body
    /// declines it.
    /// The material, and the clip-to-local mapping it would otherwise carry.
    ///
    /// Returned beside the material rather than inside it so that two of this
    /// shape in different places compare equal and the batch merges them; the
    /// mapping rides on the vertices instead. See `Renderer::fill_into_local`.
    fn analytic_rrect(
        &self,
        rect: Rect,
        radius: f32,
        paint: &Paint,
    ) -> Option<(Material, [f32; 12])> {
        // Invisible and fully-clipped shapes are rejected before the path is
        // built, and this route has to reject them too. It bypasses
        // `draw_path`, which is where those checks live -- so an alpha of zero
        // went from recording nothing to recording a quad the fragment stage
        // discards a pixel at a time.
        if !paint.is_visible() || self.clip.is_some_and(Scissor::is_empty) {
            return None;
        }
        // A stroked rectangle with square corners is the one shape this route
        // draws differently, and it took a plate sending a rectangle through
        // `draw_rect` to notice. A distance field's stroke is the band a fixed
        // distance either side of the outline, and at a vertex that band's
        // outer edge is an arc -- so the corners come out *rounded*, where
        // `dart:ui`'s default join is a miter and the tessellated route gives
        // the sharp corner it asks for. Measured on a nine-wide stroke, one
        // route draws the corner pixel and the other does not: a difference of
        // two hundred and fifty-three out of two hundred and fifty-five.
        //
        // With a radius the outline has no vertex, so there is no join to get
        // wrong and the arc is what the shape is. Only the square-cornered case
        // is refused, and only where the caller asked for something other than
        // the round join the field would give them anyway.
        // A bevel cuts the corner off, which is neither the arc a round join
        // gives nor the point a miter does, and no offset of this shape is that
        // -- so it is the one join still sent to the tessellator. The other two
        // are the outer offset with and without a radius, which is what
        // `outer_radius` carries below.
        let mitered = match &paint.style {
            Style::Stroke(style) if style.width > 0.0 => {
                if style.join == LineJoin::Bevel {
                    return None;
                }
                style.join == LineJoin::Miter
            }
            _ => false,
        };
        // Widened and dimmed on the same terms the tessellated route is, so
        // that which route a shape takes stays invisible. See `thin_stroke`.
        let (stroke, coverage) =
            thin_stroke(self.transform, analytic_stroke(paint)?, Zero::IsNotAStroke);
        let Shader::Solid(color) = &paint.shader else {
            return None;
        };
        // A radius of zero is a plain rectangle, which this field describes as
        // well as any other: with no corner to round, the expression is the
        // distance to the nearer edge. NaN is not a radius and falls back.
        if radius.is_nan() || radius < 0.0 {
            return None;
        }
        let to_clip = self.target.projection() * self.transform;
        let center = Vec2::new(
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0,
        );
        let clip_to_local = invert_to_local(to_clip * Affine2::from_translation(center));
        Some((
            Material::RoundedRect {
                color: dimmed(*color, coverage),
                half_size: [rect.width() / 2.0, rect.height() / 2.0],
                // Identity, because the mapping travels on the vertices instead
                // -- `clip_to_local` below, applied by `fill_into_local`. It maps a
                // clip-space position back into the shape's own space, measured
                // from its center, so the distance is measured where the radius
                // means what the caller said; measuring in clip space would round
                // the corners by different amounts on each axis of a target that is
                // not square. Keeping it out of the material is what lets two of
                // this shape in different places merge into one draw.
                to_local: to_local_columns(Transform2D::IDENTITY),
                radius: radius.min(rect.width() / 2.0).min(rect.height() / 2.0),
                // A mitered square corner offsets to a square corner, which is a
                // radius of nothing. Everything else grows by half the stroke:
                // a rounded corner because that is what offsetting an arc does, and
                // a round join because the arc is what it asks for.
                outer_radius: if mitered && radius <= 0.0 {
                    0.0
                } else {
                    radius.min(rect.width() / 2.0).min(rect.height() / 2.0) + stroke / 2.0
                },
                stroke,
            },
            clip_to_local,
        ))
    }

    /// Draw the quad a fragment-evaluated shape is painted onto.
    ///
    /// Outset by a pixel, because the coverage ramp runs half a pixel either
    /// side of the edge: a quad ending exactly at the shape would clip the
    /// outer half of its own antialiasing and leave a hard edge on a shape
    /// drawn to be soft. Outsetting costs a ring of fragments that compute a
    /// coverage of zero.
    fn draw_analytic(
        &mut self,
        rect: Rect,
        material: Material,
        paint: &Paint,
    ) -> Result<&mut Self> {
        // A pixel for the coverage ramp, plus half the stroke where one is
        // traced: an outline straddles the edge, so it reaches outward by half
        // its width beyond the shape it belongs to.
        let reach = 1.0
            + thin_stroke(
                self.transform,
                analytic_stroke(paint).unwrap_or(0.0),
                Zero::IsNotAStroke,
            )
            .0 / 2.0;
        let outset = Rect::new(
            rect.left - reach,
            rect.top - reach,
            rect.right + reach,
            rect.bottom + reach,
        );
        let render_paint = RenderPaint {
            material,
            filter: paint.color_filter,
            blend: paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        self.renderer.fill_into(
            &mut self.batch,
            &outset.to_path(),
            self.transform,
            &render_paint,
        )?;
        Ok(self)
    }

    /// [`Self::draw_analytic`], with the shape's own space carried on the
    /// vertices instead of in the material.
    ///
    /// Separate rather than an `Option` on the one above, because the two say
    /// different things: a material that maps clip space itself is one shape's
    /// worth of draw, and one that reads `uv` merges with every other placement
    /// of the same shape. Only the rounded rectangle takes this route so far --
    /// the ellipse and the blurred rectangle still carry their mapping.
    fn draw_analytic_local(
        &mut self,
        rect: Rect,
        material: Material,
        paint: &Paint,
        clip_to_local: [f32; 12],
    ) -> Result<&mut Self> {
        let reach = 1.0
            + thin_stroke(
                self.transform,
                analytic_stroke(paint).unwrap_or(0.0),
                Zero::IsNotAStroke,
            )
            .0 / 2.0;
        let outset = Rect::new(
            rect.left - reach,
            rect.top - reach,
            rect.right + reach,
            rect.bottom + reach,
        );
        let render_paint = RenderPaint {
            material,
            filter: paint.color_filter,
            blend: paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        self.renderer.fill_into_local(
            &mut self.batch,
            &outset.to_path(),
            self.transform,
            &render_paint,
            clip_to_local,
        )?;
        Ok(self)
    }

    /// Fill or stroke a circle.
    ///
    /// An antialiased solid fill goes through the same distance field a rounded
    /// rectangle does, because it is one: a square whose corner radius is half
    /// its side has no straight edge left, and the field reduces exactly to the
    /// distance from the center less the radius. So a circle costs two
    /// triangles and needs no shader of its own, where four cubics flattened to
    /// The shadow an object at `elevation` casts, under one light.
    ///
    /// The rule rather than the picture, which is why this is a call and not a
    /// blurred draw a caller assembles: the whole point of an elevation is
    /// that everything at the same height casts a consistent shadow, and
    /// consistency is what a rule spread across call sites loses first.
    ///
    /// The model is a single light above and behind the viewer, which is what
    /// Impeller uses and what makes a raised object's shadow fall downward on
    /// screen. Three things follow from the elevation and nothing else: the
    /// shadow is offset downward by it, blurred in proportion to it, and drawn
    /// at a quarter of the stated color's alpha before that alpha is adjusted
    /// for the color's own luminance.
    ///
    /// `elevation` is in the same units the canvas draws in. Impeller scales
    /// it by a device pixel ratio first, which is a framework concept rather
    /// than a rendering one; a caller who has one should apply it here.
    ///
    /// `transparent_occluder` is accepted and unused, which is what upstream
    /// does with it -- `DlDispatcherBase::drawShadow` takes the flag and its
    /// drawing code never reads it. It stays on the signature because
    /// `dart:ui` has it, and a caller porting a call should not have to find
    /// out that one argument went missing.
    pub fn draw_shadow(
        &mut self,
        path: &Path,
        color: Color,
        elevation: f32,
        _transparent_occluder: bool,
    ) -> Result<&mut Self> {
        if !elevation.is_finite() || elevation <= 0.0 || color.is_invisible() {
            // Nothing at ground level: an object resting on the surface casts
            // no shadow, which is the same answer as an invisible one.
            return Ok(self);
        }

        // Elevation gives a kernel radius, which the blur takes as a
        // deviation only after converting. Skipping the conversion is the
        // larger half of what made a shadow here twice as soft as upstream's.
        let sigma = sigma_for_radius(LIGHT_RADIUS * elevation);
        let shade = tonal_shadow_color(color);
        // Divided by the transform's scale, which is upstream's division and
        // is here for upstream's reason. A mask blur's sigma is stated in the
        // space the drawing is in and grows with the transform, so a shadow
        // whose softness comes from an *elevation* rather than from a length
        // would grow with it too -- and upstream's does not. Its
        // `drawShadow` divides its radius by the canvas scale precisely to
        // cancel the multiplication its filter applies, leaving a shadow's
        // softness fixed in device pixels however large the caster is drawn.
        //
        // This division did not exist while the sigma was device-space,
        // because there was no multiplication to cancel. Removing it was the
        // half of the blur change that a test caught rather than the half that
        // was designed: `a_shadows_softness_is_fixed_in_device_pixels` is that
        // test, and it was written when the opposite convention made the same
        // shadow come out right without any division at all.
        let scale = max_scale_of(self.transform);
        let sigma = if scale.is_finite() && scale > 0.0 {
            sigma / scale
        } else {
            sigma
        };
        let paint = Paint::fill(shade).with_mask_blur(sigma);

        // The whole shadow, whatever the occluder is, which is what upstream
        // draws: `DlDispatcherBase::drawShadow` takes `transparent_occluder`
        // and its drawing code never reads it.
        //
        // This used to punch the caster's outline out of the shadow for an
        // opaque one, on the grounds that the part a caster covers is spent.
        // The grounds were sound and the result was not worth them. An opaque
        // caster drawn over its own shadow is the arrangement the flag
        // describes, and there the two are the same picture -- measured at
        // zero of sixty-five thousand bytes differing -- while the punch cost a
        // layer, so every shadow was five passes where four will do. A
        // deviation that produces identical pixels more slowly has nothing to
        // recommend it.
        self.save();
        self.translate(0.0, elevation);
        let failure = self.draw_path(path, &paint).err();
        self.restore();
        match failure {
            Some(e) => Err(e),
            None => Ok(self),
        }
    }

    /// a tolerance cost vertices in proportion to how large it is drawn.
    pub fn draw_circle(&mut self, center: Vec2, radius: f32, paint: &Paint) -> Result<&mut Self> {
        // NaN named rather than caught by a negated comparison, which reads as
        // a typo and which clippy objects to on exactly those grounds.
        if radius.is_nan() || radius <= 0.0 {
            return Ok(self);
        }
        let bounds = Rect::new(
            center.x - radius,
            center.y - radius,
            center.x + radius,
            center.y + radius,
        );
        // A circle is a rounded rectangle whose corners are half its side, which
        // is how the line below already treats it and how upstream states it:
        // `DrawOval` on a square sends `RRectBlurShape(rect, width * 0.5)`.
        if let Some((material, pad, drawn)) = self.analytic_rrect_blur_draw(bounds, radius, paint) {
            return self.draw_analytic(bounds.outset(pad), material, &drawn);
        }
        if let Some((material, to_local)) = self.analytic_rrect(bounds, radius, paint) {
            return self.draw_analytic_local(bounds, material, paint, to_local);
        }
        let path = circle_path(center, radius);
        self.draw_path(&path, paint)
    }

    /// Fill or stroke an ellipse inscribed in `bounds`.
    ///
    /// A circle where the bounds are square, and the shape a rounded rectangle
    /// cannot become: past half its shorter side a rounded rectangle stops
    /// changing, giving a stadium, where an ellipse keeps curving on both axes.
    ///
    /// An antialiased solid fill is evaluated per fragment like the other
    /// shapes here. Everything else is tessellated from four cubics, which
    /// approximate an ellipse exactly as well as they approximate a circle --
    /// which is to say closely, and not exactly.
    pub fn draw_oval(&mut self, bounds: Rect, paint: &Paint) -> Result<&mut Self> {
        if bounds.is_empty() {
            return Ok(self);
        }
        if let Some(material) = self.analytic_ellipse(bounds, paint) {
            return self.draw_analytic(bounds, material, paint);
        }
        let path = oval_path(bounds);
        self.draw_path(&path, paint)
    }

    /// A paint for the fragment-evaluated ellipse, where one applies.
    ///
    /// The same conditions the rounded rectangle answers to, and for the same
    /// reasons.
    fn analytic_ellipse(&self, bounds: Rect, paint: &Paint) -> Option<Material> {
        if !paint.is_visible() || self.clip.is_some_and(Scissor::is_empty) {
            return None;
        }
        let (stroke, coverage) =
            thin_stroke(self.transform, analytic_stroke(paint)?, Zero::IsNotAStroke);
        let Shader::Solid(color) = &paint.shader else {
            return None;
        };
        let to_clip = self.target.projection() * self.transform;
        let center = Vec2::new(
            (bounds.left + bounds.right) / 2.0,
            (bounds.top + bounds.bottom) / 2.0,
        );
        Some(Material::Ellipse {
            color: dimmed(*color, coverage),
            half_size: [bounds.width() / 2.0, bounds.height() / 2.0],
            to_local: invert_to_local(to_clip * Affine2::from_translation(center)),
            stroke,
        })
    }

    /// Draw a run of positioned glyphs from one atlas, as a single draw.
    ///
    /// `atlas_slot` indexes the image table supplied at draw time, and each
    /// glyph names where in that atlas its coverage sits. A run is one draw
    /// however many glyphs it holds, because the coordinates travel on the
    /// vertices rather than in the paint — which is the reason vertices carry
    /// them at all, text being the highest draw-count content there is.
    ///
    /// The paint's color is the text color; its shader is otherwise ignored,
    /// since the atlas supplies coverage rather than color. Gradient-filled
    /// text needs the run drawn into a layer and the gradient drawn through it,
    /// which is a thing a caller can already build out of what is here.
    pub fn draw_glyphs(
        &mut self,
        glyphs: &[PositionedGlyph],
        atlas: &Atlas,
        atlas_slot: u32,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if glyphs.is_empty() || !paint.is_visible() {
            return Ok(self);
        }
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }
        // A run does not go through `draw_path` either, so it has to notice
        // these itself -- and did not, which made it the third call to accept a
        // filter and quietly draw without one.
        if !paint.image_filter.is_identity() {
            return self.draw_glyphs_filtered(glyphs, atlas, atlas_slot, paint);
        }
        if paint.mask_blur > 0.0 {
            // The same four styles a shape gets, over the run instead. A run is
            // coverage times one solid color, which is exactly the case where
            // blurring the coverage and blurring the result agree -- so unlike
            // a mesh, there is nothing here to refuse. This is how a text
            // shadow is drawn.
            return self.draw_masked(
                Masked::Glyphs {
                    glyphs,
                    atlas,
                    slot: atlas_slot,
                },
                paint,
            );
        }

        let color = match &paint.shader {
            Shader::Solid(color) => color.to_array(),
            // A run tints one color, so anything else has no meaning here. The
            // first stop of a gradient is a guess at what was meant, and a
            // guess drawn is worse than a refusal read.
            _ => {
                return Err(Error::Unsupported(
                    "glyphs take a solid color; fill a layer through a gradient instead",
                ))
            }
        };

        let to_clip = self.target.projection() * self.transform;
        let mut vertices = Vec::with_capacity(glyphs.len() * 4);
        let mut indices = Vec::with_capacity(glyphs.len() * 6);
        for glyph in glyphs {
            let Some(rect) = atlas.get(glyph.key) else {
                // A glyph nobody added would sample whatever texel happens to
                // sit at the origin, which draws a plausible smudge. Refusing
                // names the glyph instead.
                return Err(Error::Unsupported(
                    "a glyph in this run is not in the atlas",
                ));
            };
            if rect.width == 0 || rect.height == 0 {
                // A space occupies no texels and needs no quad, but it still
                // traveled through the run rather than being the caller's to
                // filter out.
                continue;
            }

            let [u0, v0, u1, v1] = rect.uv(atlas.size());
            let [x, y] = glyph.position;
            let [w, h] = glyph.size;
            let corners = [
                ([x, y], [u0, v0]),
                ([x + w, y], [u1, v0]),
                ([x + w, y + h], [u1, v1]),
                ([x, y + h], [u0, v1]),
            ];
            let base = vertices.len() as u32;
            for ([px, py], uv) in corners {
                // Homogeneous, so the atlas coordinate beside it is
                // interpolated perspective-correctly rather than swimming
                // across the quad.
                let clip = to_clip.project_homogeneous(Vec2::new(px, py));
                vertices.push(Vertex::projected([clip.x, clip.y, clip.z], uv));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        if indices.is_empty() {
            return Ok(self);
        }

        let slot = self.slot_for(TextureSource::Image(atlas_slot));
        self.batch.push_mesh(
            &vertices,
            &indices,
            Material::Glyph { color, slot },
            paint.color_filter,
            paint.blend,
            self.clip,
            ClipState::content(self.depth),
        )?;
        Ok(self)
    }

    /// Draw a mesh of triangles the caller supplied.
    ///
    /// The one drawing call whose geometry does not come from tessellating a
    /// shape, which is why it is also the one that can produce triangles this
    /// renderer would never have made: overlapping, degenerate, wound either
    /// way. None of that is checked beyond what [`Vertices`] checks at
    /// construction, because a mesh is a caller saying what to draw rather
    /// than what to draw *around*, and second-guessing it would defeat the
    /// point.
    ///
    /// Not antialiased. Coverage here comes from the rasterizer's own rule
    /// rather than from a distance field or a tessellated feather, and the
    /// interior edges of a mesh are seams between triangles that must not be
    /// feathered at all -- a mesh whose triangles each faded at their borders
    /// would show a lattice of its own construction.
    ///
    /// Texture coordinates, where the mesh carries them, require an image
    /// paint: they say where in a texture each vertex reads, and a paint with
    /// no texture leaves them meaning nothing. The reverse is allowed -- an
    /// image paint with no coordinates maps by position, the same as on any
    /// other shape.
    pub fn draw_vertices(&mut self, mesh: &Vertices, paint: &Paint) -> Result<&mut Self> {
        if mesh.is_empty() || !paint.is_visible() {
            return Ok(self);
        }
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }
        // The same routing `draw_path` does, and it has to be repeated here
        // because a mesh does not go through `draw_path` at all. Left out, an
        // image filter on a mesh -- or on an atlas, which is a mesh by the time
        // it arrives -- was accepted and silently dropped.
        if !paint.image_filter.is_identity() {
            return self.draw_vertices_filtered(mesh, paint);
        }
        if paint.mask_blur > 0.0 {
            // A mask blur blurs coverage and then fills through it, so the fill
            // has to have a value everywhere the blurred coverage reaches --
            // including outside the shape, where the halo is. A paint has one
            // there, whether it is a color or a gradient, because a paint is a
            // function of position. Per-vertex colors do not: they are defined
            // on the mesh's own triangles and nowhere else, and the halo has
            // nothing to take its color from.
            //
            // So the refusal is about where the color comes from rather than
            // about meshes. A mesh built from positions alone is filled by the
            // paint exactly as a path is, and takes the same route.
            if mesh.colors().is_empty() {
                return self.draw_masked(Masked::Mesh(mesh), paint);
            }
            // Refused rather than dropped, and rather than approximated:
            // extrapolating vertex colors into the halo would be inventing a
            // picture, which is the substitution this renderer refuses.
            return Err(Error::Unsupported(
                "a mask blur over a mesh with per-vertex colors has no color for \
                 its halo; draw the mesh into a blurred layer instead",
            ));
        }

        let textured = !mesh.texture_coords().is_empty();
        // An image mesh has a material of its own, which samples the sheet at
        // the interpolated coordinate. Every other shader reads its coordinate
        // from the fragment's position, and the flag passed to the batch below
        // is what redirects it to the vertices instead -- so the material is
        // the ordinary one and nothing here has to know which kind it is.
        //
        // This used to refuse anything but an image, and the refusal said the
        // coordinates needed an image paint to read. `dart:ui` reads whatever
        // color source the paint carries at a mesh's coordinates, image or not.
        //
        // It then refused a caller's program alone, on the reasoning that a
        // program replaces this renderer's fragment shader outright and so has
        // nowhere for a per-vertex coordinate to arrive. That was wrong, and
        // wrong about this renderer rather than about upstream: a program
        // replaces the *fragment* stage, and the vertex stage it runs behind is
        // this renderer's own whatever the fragment does. That stage hands on
        // `uv` beside `clip`, and a program declaring the same `VertexOutput`
        // -- which one has to, to be a pipeline here at all -- receives both.
        //
        // So a program reads the mesh's coordinate by reading `in.uv`, and
        // chooses, where every built-in shader is told which to use by the flag
        // below. `effect_mesh_uv.wgsl` is the fixture that proves it: the
        // sibling of `effect.wgsl` differing in one line.
        let at_coords = textured && !matches!(paint.shader, Shader::Image { .. });
        let material = match (&paint.shader, textured) {
            (Shader::Image { .. }, true) => self.mesh_material(&paint.shader)?,
            _ if at_coords => self.material_at_texture_coords(&paint.shader),
            _ => self.material_for(&paint.shader),
        };

        let to_clip = self.target.projection() * self.transform;
        let coords = mesh.texture_coords();
        let colors = mesh.colors();
        let vertices: Vec<Vertex> = mesh
            .positions()
            .iter()
            .enumerate()
            .map(|(i, position)| {
                let clip = to_clip.project_homogeneous(*position);
                let uv = coords.get(i).copied().unwrap_or(Vec2::ZERO);
                let vertex = Vertex::projected([clip.x, clip.y, clip.z], [uv.x, uv.y]);
                match colors.get(i) {
                    // Premultiplied here rather than in the shader, because
                    // what the rasterizer interpolates between two vertices is
                    // what gets multiplied in, and interpolating straight color
                    // across an edge whose alpha varies gives a color neither
                    // end asked for.
                    Some(color) => vertex.with_color(premultiplied(*color)),
                    None => vertex,
                }
            })
            .collect();

        self.batch.push_mesh_tinted(
            &vertices,
            mesh.indices(),
            material,
            paint.color_filter,
            paint.blend,
            self.clip,
            ClipState::content(self.depth),
            paint.tint_blend,
            // Where the coordinates say something the material cannot work out
            // for itself. An image mesh is excluded because it already reads
            // them, through a material of its own; everything else -- a
            // gradient, a caller's program -- derives its coordinate from the
            // fragment's position unless told otherwise, and this is the
            // telling. The material above was built to match.
            at_coords,
        )?;
        Ok(self)
    }

    /// Draw many pieces of one sheet, each with its own transform, in one draw.
    ///
    /// This is `draw_vertices` with the mesh built for you, and building it is
    /// the part worth not getting wrong: each sprite is a quad running from
    /// the origin to its source's size, placed by its transform, with the
    /// source rectangle divided by the sheet's size to become the
    /// coordinates its corners read. A caller assembling that themselves has
    /// four chances per sprite to transpose an axis.
    ///
    /// One draw for the whole batch: a hundred sprites from one sheet differ
    /// only in their vertices, so they have no reason to be a hundred draws.
    /// Anything that would split them -- a different sheet, a different paint
    /// -- is a second call.
    ///
    /// A loop over [`Self::draw_vertices`] would end up as one draw too, since
    /// a batch merges adjacent draws that differ in nothing. What this saves
    /// is the per-sprite arithmetic rather than the draw calls.
    ///
    /// `sheet` is the size of the uploaded texture in texels, which the paint
    /// does not carry: a recording is built without touching a device and has
    /// never seen how large the texture is.
    ///
    /// Per-sprite colors, which `dart:ui` offers here, need a color per vertex
    /// and this vertex format has none. A tint on the paint applies to the
    /// whole batch.
    pub fn draw_atlas(
        &mut self,
        sprites: &[Sprite],
        sheet: Extent2D,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if sprites.is_empty() || !paint.is_visible() {
            return Ok(self);
        }
        if sheet.width == 0 || sheet.height == 0 {
            return Err(Error::Unsupported(
                "a sprite sheet with no texels has nothing to read",
            ));
        }
        if !sprites.iter().all(|s| s.source.is_finite()) {
            return Err(Error::Unsupported(
                "a sprite's source rectangle is not a finite number",
            ));
        }

        let texels = Vec2::new(sheet.width as f32, sheet.height as f32);
        let mut positions = Vec::with_capacity(sprites.len() * 4);
        let mut coords = Vec::with_capacity(sprites.len() * 4);
        let mut colors = Vec::with_capacity(sprites.len() * 4);
        let mut indices = Vec::with_capacity(sprites.len() * 6);
        for sprite in sprites {
            let source = &sprite.source;
            let size = Vec2::new(source.width, source.height);
            let origin = Vec2::new(source.x, source.y);
            // Corners in the same order for both, so a coordinate and the
            // position it belongs to are written by one step of the loop and
            // cannot come apart.
            let corners = [
                Vec2::ZERO,
                Vec2::new(size.x, 0.0),
                size,
                Vec2::new(0.0, size.y),
            ];
            let base = positions.len() as u32;
            for corner in corners {
                positions.push(sprite.transform.transform_point2(corner));
                coords.push((origin + corner) / texels);
                // The same color at all four corners: a sprite is tinted as a
                // whole, and a gradient across one is a mesh rather than a
                // sprite.
                colors.push(sprite.color);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        let mesh = Vertices::full(VertexMode::Triangles, positions, coords, colors, indices)?;
        self.draw_vertices(&mesh, paint)
    }

    /// The material for a mesh reading a texture at its own coordinates.
    ///
    /// Separate from [`Self::material_for`] because an image drawn on a shape
    /// and an image drawn on a mesh are different materials rather than one
    /// material in two modes -- the mapping, and the source rectangle that
    /// selects part of a sheet, are both things the vertices have already
    /// said. A caller wanting one sprite from a sheet states its texels in the
    /// coordinates.
    fn mesh_material(&mut self, shader: &Shader) -> Result<Material> {
        let Shader::Image {
            slot,
            alpha,
            tile,
            tint,
            source,
            sampling,
            ..
        } = shader
        else {
            return Err(Error::Unsupported("a mesh material needs an image paint"));
        };
        // A source rectangle and a per-vertex coordinate are two answers to
        // the same question, and applying one on top of the other would mean a
        // coordinate of one landed at the rectangle's far edge rather than at
        // the texture's. Refused rather than ignored: a caller who set it
        // meant something by it, and the mesh is where they say it instead.
        if *source != Rect::new(0.0, 0.0, 1.0, 1.0) {
            return Err(Error::Unsupported(
                "a textured mesh states its own coordinates; put the source rectangle in them",
            ));
        }
        let table_slot = self.slot_for(TextureSource::Image(*slot));
        Ok(Material::Mesh {
            slot: table_slot,
            alpha: *alpha,
            tint: tint.to_array(),
            tile: *tile,
            sampling: *sampling,
        })
    }

    /// Draw a run of points, joined as the mode says.
    ///
    /// A point is a segment of no length, which is what decides how it looks:
    /// the stroke's cap is the whole of the shape. A round cap gives a dot the
    /// width across, a square cap a square of that side, and a butt cap --
    /// which adds nothing to either end of a segment -- gives nothing at all.
    /// That is not a special case being handled; it is the only reading of a
    /// zero-length segment that stays consistent with how every other stroke
    /// is drawn.
    ///
    /// Takes the stroke's width even from a paint asking to fill, since there
    /// is no interior to fill and a point with no width is not a shape.
    pub fn draw_points(
        &mut self,
        mode: PointMode,
        points: &[Vec2],
        paint: &Paint,
    ) -> Result<&mut Self> {
        let stroke = match &paint.style {
            Style::Stroke(stroke) => *stroke,
            Style::Fill => StrokeStyle::default(),
        };
        // Negative only, which is upstream's guard -- `PointFieldGeometry`
        // refuses `radius_ < 0.0` and widens everything else. A width of zero is
        // a point one pixel across, the same reading a zero-width stroke gets.
        if !stroke.width.is_finite() || stroke.width < 0.0 {
            return Ok(self);
        }

        match mode {
            // One draw for the whole field, which is what upstream's
            // `PointFieldGeometry` produces and what this used to fail to.
            //
            // It used to be a draw per point, and the reason is worth keeping
            // because it is not the obvious one. The other two modes below
            // issue a call per segment and still come out as one draw: a batch
            // merges draws that share a material, and consecutive segments do.
            // A point did not, because the analytic circle it went through
            // carries its center in `to_local` -- so two points at two places
            // were two materials, and no number of them could merge.
            //
            // `Material::PointField` is the same field with the center taken
            // off the paint and put on the vertices, as the unit circle's
            // corners in the texture coordinate. Every point is then the same
            // material and the field is one draw, with the edge each of them
            // had: the shader differentiates the implicit function across the
            // pixel rather than forming a distance, and a derivative of an
            // interpolated value is as good as one of a computed value.
            PointMode::Points => {
                let radius = point_radius(self.transform, stroke.width);
                if !matches!(stroke.cap, LineCap::Round | LineCap::Square) {
                    // A butt cap extends a segment by nothing, and nothing is
                    // what a segment of no length becomes.
                    return Ok(self);
                }
                let round = matches!(stroke.cap, LineCap::Round);
                // A mask blur or an image filter wants a layer around the
                // draw, which is machinery the per-point route already reaches
                // through `draw_circle` and this one skips entirely. Two tests
                // that walk every call taking a paint caught exactly that: the
                // field accepted a blur and drew what it draws without one.
                //
                // The condition is the same one `analytic_stroke` states for
                // the same reason, and it is worth keeping the shape of that
                // rule rather than a list: every route that needs a layer has
                // to decline here.
                if paint.mask_blur > 0.0 || !paint.image_filter.is_identity() {
                    return self.draw_points_individually(points, &stroke, paint);
                }
                let material = self.material_for(&paint.shader);
                let Material::Solid(color) = material else {
                    // A field takes one color, which is what a point field is.
                    // Anything else keeps the old route, where each point is
                    // its own shape and can carry its own mapping.
                    return self.draw_points_individually(points, &stroke, paint);
                };

                // A round cap's quad is a pixel wider than the disc, for the
                // reason `draw_analytic` outsets its own: the coverage ramp
                // runs half a pixel either side of the edge, and a quad ending
                // exactly on the circle clips the outer half of its own
                // antialiasing. Measured without it, ninety-two pixels of the
                // frame differed from the same dots drawn one at a time, at
                // the four places the circle meets its quad.
                //
                // The texture coordinate is scaled to match, so that the unit
                // circle still falls where the disc's edge is rather than at
                // the corner of the widened quad. A square cap is the quad
                // itself and takes neither: its coordinates are zero, where
                // the field is saturated, so it covers what it is given.
                let (extent, coord) = if round {
                    (radius + 1.0, (radius + 1.0) / radius.max(1e-6))
                } else {
                    (radius, 0.0)
                };
                let to_clip = self.target.projection() * self.transform;
                let mut vertices: Vec<Vertex> = Vec::with_capacity(points.len() * 4);
                let mut indices: Vec<u32> = Vec::with_capacity(points.len() * 6);
                for point in points {
                    if !point.is_finite() {
                        continue;
                    }
                    let base = vertices.len() as u32;
                    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                        let at = Vec2::new(point.x + dx * extent, point.y + dy * extent);
                        let clip = to_clip.project_homogeneous(at);
                        vertices.push(Vertex::projected(
                            [clip.x, clip.y, clip.z],
                            [dx * coord, dy * coord],
                        ));
                    }
                    indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                }
                if indices.is_empty() {
                    return Ok(self);
                }
                self.batch.push_mesh_tinted(
                    &vertices,
                    &indices,
                    Material::PointField { color },
                    paint.color_filter,
                    paint.blend,
                    self.clip,
                    ClipState::content(self.depth),
                    paint.tint_blend,
                    false,
                )?;
            }
            PointMode::Lines => {
                for pair in points.chunks_exact(2) {
                    self.draw_line(pair[0], pair[1], paint)?;
                }
            }
            PointMode::Polygon => {
                if points.len() >= 2 {
                    let mut b = PathBuilder::new();
                    b.move_to(points[0]);
                    for point in &points[1..] {
                        b.line_to(*point);
                    }
                    self.draw_path(&b.build(), paint)?;
                }
            }
        }
        Ok(self)
    }

    /// The field drawn a point at a time, for a paint a field cannot carry.
    ///
    /// A point field is one color by construction -- the material holds one --
    /// so a gradient or an image paint still goes through the shape each point
    /// has, and costs a draw each. That is the old behavior kept for the case
    /// it was right for rather than a fallback nobody exercises: `dart:ui`
    /// allows any paint on `drawPoints`, and a caller who asks for a gradient
    /// across a field should get one.
    fn draw_points_individually(
        &mut self,
        points: &[Vec2],
        stroke: &StrokeStyle,
        paint: &Paint,
    ) -> Result<&mut Self> {
        // Widened on the same terms the field route is, so which route a point
        // takes stays invisible -- this one is reached for a mask blur, an image
        // filter or a shader, none of which changes how small a point may be.
        let radius = point_radius(self.transform, stroke.width);
        // Filled rather than stroked: what is being drawn is the cap itself,
        // and asking the stroker for a segment of no length is asking it for a
        // direction that does not exist.
        let dot = paint.clone().with_style(Style::Fill);
        for point in points {
            if !point.is_finite() {
                continue;
            }
            match stroke.cap {
                LineCap::Round => {
                    self.draw_circle(*point, radius, &dot)?;
                }
                LineCap::Square => {
                    self.draw_rect(
                        Rect::new(
                            point.x - radius,
                            point.y - radius,
                            point.x + radius,
                            point.y + radius,
                        ),
                        &dot,
                    )?;
                }
                LineCap::Butt => {}
            }
        }
        Ok(self)
    }

    /// Draw a finished recording into this one.
    ///
    /// `dart:ui` calls this `drawPicture`, and it is a smaller thing here than
    /// there, which is worth saying plainly. An `SkPicture` is a command list,
    /// so replaying one under a new transform re-runs the commands and
    /// re-tessellates at the new scale. A [`Recording`] is already tessellated:
    /// its paths were flattened at a tolerance taken from the transform in force
    /// when they were recorded, and its vertices are in clip space. Magnified,
    /// it shows the polygon it was flattened to.
    ///
    /// So this composes scenes at about the scale they were recorded at. It is
    /// not the reuse optimization the same call is elsewhere, and a caller who
    /// wants that should re-record.
    ///
    /// What it costs is a layer, because that is what it is: the recording's
    /// passes are taken as they are and its root becomes an image this canvas
    /// samples. Nothing is re-recorded and no geometry is touched, which is
    /// what makes it cheap and also what makes it unable to re-tessellate.
    ///
    /// # Slots
    ///
    /// A recording naming a caller's image by index keeps that index. The two
    /// recordings are submitted with one image table, so an index means the
    /// same thing in both -- a caller composing recordings that disagree about
    /// what image three is has to renumber before recording, which is a thing
    /// they can see and this call cannot.
    ///
    /// Its layers and its baked gradients are renumbered, since those name
    /// positions in lists this recording is appending to.
    pub fn draw_recording(&mut self, recording: &Recording, paint: &Paint) -> Result<&mut Self> {
        if recording.is_empty() || !paint.is_visible() {
            return Ok(self);
        }
        if self.clip.is_some_and(Scissor::is_empty) {
            return Ok(self);
        }

        // Where the passes and ramps being appended will land. Taken before
        // anything is pushed, because every index inside the recording is
        // relative to lists that start here.
        let pass_offset = self.finished.len();
        let ramp_offset = self.ramps.len();
        self.ramps.extend(recording.ramps.iter().cloned());

        for pass in &recording.passes {
            let mut pass = pass.clone();
            for source in &mut pass.sources {
                *source = match *source {
                    // A layer names a pass by position in the recording it came
                    // from, and that recording's passes are now further along.
                    TextureSource::Layer(index) => TextureSource::Layer(index + pass_offset),
                    TextureSource::Ramp(index) => TextureSource::Ramp(index + ramp_offset),
                    // The caller's own table, which both recordings share.
                    TextureSource::Image(index) => TextureSource::Image(index),
                };
            }
            self.finished.push(pass);
        }

        // The recording's root is the image this canvas now samples, and every
        // other pass it brought is a layer feeding that one.
        let root = self.finished.len() - 1;
        let slot = self.slot_for(TextureSource::Layer(root));

        // The recording rendered at its own extent, and it lands here at that
        // size with its top-left at the origin of the current transform. A
        // recording is a picture rather than a shape, so it has no bounds of
        // its own to place -- where it goes is what the transform says.
        let extent = recording.extent;
        let placement = Rect::new(0.0, 0.0, extent.width as f32, extent.height as f32);
        // The mapping from a fragment's clip position back to a texel of the
        // recording, which is the same pair a layer composite builds -- except
        // that a layer sits at a known place in its parent and a recording is
        // placed by the transform, so the mapping is built from that instead.
        //
        // Clip space runs from -1 to 1 and upward, so the axes are halved and Y
        // is negated. Inverting the transform is what carries a fragment back
        // into the recording's own coordinates; a transform that folds the
        // plane has no inverse, and the geometry collapses with it, so both
        // halves go together and nothing is drawn.
        let to_clip = self.target.projection() * self.transform;
        if to_clip.determinant().abs() <= f32::EPSILON {
            return Ok(self);
        }
        // The recording's texture spans its own extent from the origin, so
        // that rectangle carried through the transform is the placement, and
        // the mapping the shader wants is its inverse.
        let picture_to_clip =
            to_clip * Affine2::from_scale(Vec2::new(extent.width as f32, extent.height as f32));
        if !picture_to_clip.is_finite() {
            return Ok(self);
        }
        let material = Material::Image {
            to_local: invert_to_local(picture_to_clip),
            slot,
            alpha: 1.0,
            tile: TileMode::Clamp,
            // At its own size a texel lands on a pixel, and under a transform
            // the recording is resampled -- which is what a picture drawn
            // scaled means, and the same answer a matrix image filter gives.
            sampling: Sampling::Linear,
            source: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
        };
        let render_paint = RenderPaint {
            material,
            filter: paint.color_filter,
            blend: paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        self.renderer.fill_into(
            &mut self.batch,
            &placement.to_path(),
            self.transform,
            &render_paint,
        )?;
        Ok(self)
    }

    /// Fill the ring between two rounded rectangles.
    ///
    /// One path of two contours filled by the even-odd rule, which is what
    /// makes the inner one a hole rather than a second ring drawn on top. A
    /// caller could assemble this, and the reason not to leave them to it is
    /// the rule: two contours wound the same way fill solid under the nonzero
    /// rule and hollow under even-odd, and which one a border needs is not
    /// something to rediscover per call site.
    pub fn draw_drrect(
        &mut self,
        outer: Rect,
        outer_radius: f32,
        inner: Rect,
        inner_radius: f32,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if outer.is_empty() {
            return Ok(self);
        }
        self.draw_drrect_with_radii(
            outer,
            [[outer_radius; 2]; 4],
            inner,
            [[inner_radius; 2]; 4],
            paint,
        )
    }

    /// A ring between two rounded rectangles whose corners need not match.
    ///
    /// [`Self::draw_drrect`] with the eight numbers each, and the same shape:
    /// two contours filled even-odd, which is what makes the inner one a hole.
    /// Each rectangle's radii are fitted to it on its own, since a ring is two
    /// rounded rectangles and not one shape with a thickness.
    pub fn draw_drrect_with_radii(
        &mut self,
        outer: Rect,
        outer_radii: RoundingRadii,
        inner: Rect,
        inner_radii: RoundingRadii,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if outer.is_empty() {
            return Ok(self);
        }
        let mut b = PathBuilder::new().with_fill_rule(FillRule::EvenOdd);
        outer.add_rounded_contour_with_radii(&mut b, outer.fitted_radii(outer_radii));
        if !inner.is_empty() {
            inner.add_rounded_contour_with_radii(&mut b, inner.fitted_radii(inner_radii));
        }
        self.draw_path(&b.build(), paint)
    }

    /// Fill everything the clip still admits.
    ///
    /// `dart:ui` calls this `drawPaint`. What it fills is the clip rather than
    /// the target, which is why it is a call and not a rectangle a caller
    /// writes: the rectangle they would need is the clip's bounds carried back
    /// through the current transform, and getting that wrong is invisible
    /// until a transform is in force.
    ///
    /// The bounds are conservative, so this may cover more than the clip
    /// admits -- and the clip then removes the excess, which is what makes a
    /// superset the safe direction to be wrong in.
    pub fn draw_paint(&mut self, paint: &Paint) -> Result<&mut Self> {
        let bounds = self.local_clip_bounds();
        if bounds.is_empty() {
            return Ok(self);
        }
        self.draw_rect(bounds, paint)
    }

    /// Fill everything the clip admits with one color.
    ///
    /// `dart:ui` calls this `drawColor`. Distinct from [`Self::clear`], which
    /// replaces the whole target and ignores the clip: this is a draw, so it
    /// blends and it obeys what is in force.
    pub fn draw_color(&mut self, color: Color, blend: BlendMode) -> Result<&mut Self> {
        self.draw_paint(&Paint::fill(color).with_blend(blend))
    }

    /// Draw an image stretched by its middle, keeping its corners.
    ///
    /// `dart:ui` calls this `drawImageNine`, and it is the shape every
    /// resizable panel with a border is made of. `center` is the part of the
    /// image, in texels, that may stretch; what surrounds it is divided into
    /// eight pieces that stretch along one axis or neither.
    ///
    /// A caller could write the nine draws. The reason not to leave them to it
    /// is the rule rather than the arithmetic: which pieces stretch in which
    /// direction is the whole of what a nine-patch means, and nine call sites
    /// are nine chances to stretch a corner.
    ///
    /// The paint supplies the blend, the filters and whether the edges are
    /// antialiased; its shader is replaced, since each of the nine pieces
    /// needs its own. A caller wanting the whole thing faded should draw it
    /// into a layer, which is what fading a group means everywhere else here.
    ///
    /// A center reaching an edge leaves pieces of no width or height, and
    /// those are skipped rather than drawn empty. A center outside the image
    /// entirely is refused, since there is no reading of it that leaves nine
    /// pieces.
    pub fn draw_image_nine(
        &mut self,
        slot: u32,
        size: Extent2D,
        center: Rect,
        into: Rect,
        paint: &Paint,
    ) -> Result<&mut Self> {
        if size.width == 0 || size.height == 0 || into.is_empty() {
            return Ok(self);
        }
        let (w, h) = (size.width as f32, size.height as f32);
        if center.left < 0.0
            || center.top < 0.0
            || center.right > w
            || center.bottom > h
            || center.right < center.left
            || center.bottom < center.top
        {
            return Err(Error::Unsupported(
                "a nine-patch center must lie within the image it divides",
            ));
        }
        if paint.mask_blur > 0.0 {
            // A nine-patch is nine quads, and each is drawn on its own. A mask
            // blur asked for here would soften each of them separately -- nine
            // haloes with seams between them rather than one softened patch --
            // which is not what was asked for and is not worth guessing at.
            //
            // Refused deliberately, because it used to be refused by accident:
            // the pieces are drawn with an image paint, and a mask blur over
            // anything that varied was refused wholesale further down. That is
            // no longer true, so the reason has to live where it applies.
            return Err(Error::Unsupported(
                "a mask blur over a nine-patch; its pieces are drawn separately \
                 and would each be softened on their own",
            ));
        }

        // The three spans along each axis, in texels and then in the
        // destination. The outer two keep their size and the middle takes
        // whatever is left, which is the whole of the rule.
        let source_x = [0.0, center.left, center.right, w];
        let source_y = [0.0, center.top, center.bottom, h];
        let left_keep = center.left;
        let right_keep = w - center.right;
        let top_keep = center.top;
        let bottom_keep = h - center.bottom;
        // A destination too small to hold both fixed edges would give the
        // middle a negative size, so the edges are scaled down together rather
        // than one of them overrunning the other.
        let squeeze_x = ((into.right - into.left) / (left_keep + right_keep)).min(1.0);
        let squeeze_y = ((into.bottom - into.top) / (top_keep + bottom_keep)).min(1.0);
        let dest_x = [
            into.left,
            into.left + left_keep * squeeze_x,
            into.right - right_keep * squeeze_x,
            into.right,
        ];
        let dest_y = [
            into.top,
            into.top + top_keep * squeeze_y,
            into.bottom - bottom_keep * squeeze_y,
            into.bottom,
        ];

        // One mesh rather than nine draws, which is what this used to be.
        //
        // Each patch reads a different part of the image into a different
        // place, and the obvious way to say that is a shader per patch with
        // its own source rectangle -- nine paints, nine draws. Carrying the
        // coordinates on the vertices instead says the same thing in one
        // draw, which is how `draw_atlas` has always done it, and a nine-patch
        // is the same shape of work: several quads out of one sheet.
        //
        // What made the nine separate is that a per-draw source rectangle also
        // *clamps* to itself, so a patch could not sample its neighbor. Merged
        // naively that clamping goes and the seams bleed -- measured, twelve
        // hundred pixels of a hundred-and-twenty-eight-square frame, by as
        // much as a hundred and seventy levels, which on the one primitive
        // whose whole purpose is stretching without artifacts is not a
        // trade worth making.
        //
        // The half-texel inset below is what buys it back. A linear sample
        // reaches exactly half a texel past its coordinate, so pulling each
        // patch's edge in by that much removes the reach across the boundary
        // rather than compensating for it. With it the merged draw is
        // byte-identical to the nine: zero pixels differ, at the same worst
        // case of zero. The sampling is this call's own rather than the
        // caller's, and is linear, which is what makes half a texel the right
        // number rather than a guess.
        let (half_u, half_v) = (0.5 / w, 0.5 / h);
        let mut positions = Vec::with_capacity(36);
        let mut coords = Vec::with_capacity(36);
        let mut indices = Vec::with_capacity(54);
        for row in 0..3 {
            for column in 0..3 {
                let source = Rect::new(
                    source_x[column] / w,
                    source_y[row] / h,
                    source_x[column + 1] / w,
                    source_y[row + 1] / h,
                );
                let destination = Rect::new(
                    dest_x[column],
                    dest_y[row],
                    dest_x[column + 1],
                    dest_y[row + 1],
                );
                if source.is_empty() || destination.is_empty() {
                    continue;
                }
                // Inset toward the middle, and never past it: a patch one texel
                // across has nothing to give up, and an inset that crossed
                // would turn the quad inside out.
                let mid_u = (source.left + source.right) * 0.5;
                let mid_v = (source.top + source.bottom) * 0.5;
                let u0 = (source.left + half_u).min(mid_u);
                let u1 = (source.right - half_u).max(mid_u);
                let v0 = (source.top + half_v).min(mid_v);
                let v1 = (source.bottom - half_v).max(mid_v);

                let base = positions.len() as u32;
                positions.extend([
                    Vec2::new(destination.left, destination.top),
                    Vec2::new(destination.right, destination.top),
                    Vec2::new(destination.right, destination.bottom),
                    Vec2::new(destination.left, destination.bottom),
                ]);
                coords.extend([
                    Vec2::new(u0, v0),
                    Vec2::new(u1, v0),
                    Vec2::new(u1, v1),
                    Vec2::new(u0, v1),
                ]);
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        if positions.is_empty() {
            return Ok(self);
        }

        let mesh = crate::vertices::Vertices::indexed(
            crate::vertices::VertexMode::Triangles,
            positions,
            coords,
            indices,
        )?;
        // The paint supplies everything but the shader -- its blend, its
        // filters, whether it antialiases -- and the shader is the sheet, read
        // by the coordinates the vertices carry.
        let painted = paint.clone().with_shader(Shader::Image {
            slot,
            rect: into,
            alpha: 1.0,
            tile: TileMode::Clamp,
            source: Rect::new(0.0, 0.0, 1.0, 1.0),
            tint: Color::WHITE,
            sampling: Sampling::Linear,
        });
        self.draw_vertices(&mesh, &painted)
    }

    /// A line, and for a hairline one snapped to a pixel.
    ///
    /// The snap is the third part of upstream's minimum-size rule and the one
    /// that decides whether a hairline *looks* like one. A line one pixel wide
    /// whose center sits on a pixel boundary covers half of each of the two rows
    /// it straddles, so it draws gray and two pixels soft; the same line centered
    /// in a row covers it and draws black and one pixel crisp. Upstream moves it:
    /// `LineGeometry::GetPositionBuffer` carries the endpoints into device space,
    /// drops the transform, and rounds the constant coordinate to a half with
    /// `RoundToHalf` -- which is a pixel's middle.
    ///
    /// Gated exactly as upstream gates it, on `width_ == 0.f` and
    /// `IsTranslationScaleOnly`. A width the caller stated is a width to respect,
    /// and under a rotation or a shear no axis-aligned line stays one.
    ///
    /// Narrower than upstream in one way, which is deliberate: a paint wanting a
    /// layer keeps the ordinary path. A mask blur's sigma is stated in user space
    /// and this draws in device space, so the two would disagree under a scale --
    /// and a snap is worth half a pixel, not a wrong blur.
    pub fn draw_line(&mut self, from: Vec2, to: Vec2, paint: &Paint) -> Result<&mut Self> {
        if let Some((p0, p1)) = self.hairline_snap(from, to, paint) {
            let mut b = PathBuilder::new();
            b.move_to(p0).line_to(p1);
            let path = b.build();
            // Drawn in device space, so the widening reads a basis of one and
            // gives exactly the pixel the snap aligned it to.
            let held = self.transform;
            self.transform = Transform2D::IDENTITY;
            let drawn = self.draw_path(&path, paint).map(|_| ());
            self.transform = held;
            drawn?;
            return Ok(self);
        }
        let mut b = PathBuilder::new();
        b.move_to(from).line_to(to);
        let path = b.build();
        self.draw_path(&path, paint)
    }

    /// The device-space endpoints of a hairline worth snapping, or `None`.
    ///
    /// `None` covers every case that keeps the ordinary path: a stated width, a
    /// transform that is not a translation and a scale, a paint that wants a
    /// layer, and a line that is not axis-aligned once transformed.
    fn hairline_snap(&self, from: Vec2, to: Vec2, paint: &Paint) -> Option<(Vec2, Vec2)> {
        let Style::Stroke(stroke) = &paint.style else {
            return None;
        };
        if stroke.width != 0.0 {
            return None;
        }
        if paint.mask_blur > 0.0 || !paint.image_filter.is_identity() {
            return None;
        }
        // A solid color only, and this is the guard that matters most. Every
        // other shader is *mapped* by the transform -- a gradient's stops are
        // stated in the space the draw was recorded in, an image's source
        // rectangle likewise -- and this draw hands the batch an identity
        // transform, so the mapping would be taken without the scale. Measured
        // while writing this: a hairline under a doubling read its gradient at
        // sixteen per cent where the same line a pixel wide read it at four.
        //
        // Such a hairline keeps the ordinary path and stays half a pixel off
        // center, which is the right way round: a snap is worth half a pixel,
        // and a shader read in the wrong place is worth nothing.
        if !matches!(paint.shader, Shader::Solid(_)) {
            return None;
        }
        let affine = self.transform.to_affine()?;
        // A translation and a scale, which is what leaves an axis-aligned line
        // axis-aligned. Read off the matrix rather than tracked, so a caller who
        // arrived here through `concat` is judged on what they actually have.
        let m = affine.matrix2;
        if m.x_axis.y != 0.0 || m.y_axis.x != 0.0 {
            return None;
        }
        let (p0, p1) = (affine.transform_point2(from), affine.transform_point2(to));
        if !p0.is_finite() || !p1.is_finite() {
            return None;
        }
        // A pixel's middle, which is where a one-pixel line has to sit to cover
        // one pixel. Upstream's `RoundToHalf`.
        let to_half = |v: f32| v.floor() + 0.5;
        // Vertical first, matching upstream's order, and either branch leaves
        // the other coordinate alone -- the line's length is the caller's.
        if (p0.x - p1.x).abs() < CLOSE_ENOUGH {
            let x = to_half(p0.x);
            return Some((Vec2::new(x, p0.y), Vec2::new(x, p1.y)));
        }
        if (p0.y - p1.y).abs() < CLOSE_ENOUGH {
            let y = to_half(p0.y);
            return Some((Vec2::new(p0.x, y), Vec2::new(p1.x, y)));
        }
        None
    }

    /// Close a layer: file its pass, and composite it onto the parent.
    /// The device-space rectangle a finished layer's draws actually cover.
    ///
    /// Read off the geometry rather than accumulated draw by draw, because the
    /// geometry is what gets rasterized. Anything covering the whole target --
    /// a `clear`, a fill the size of the frame -- has a quad that says so, so
    /// the cases upstream has to name as "unbounded content" need no special
    /// case here: their coverage comes out as the whole target and nothing is
    /// narrowed.
    ///
    /// Vertices are homogeneous clip positions for `target`, so this is the
    /// inverse of the projection they went through:
    /// `viewport_projection` maps a device offset `d` to `2d/extent - 1` in x
    /// and `1 - 2d/extent` in y.
    ///
    /// `None` where there is no geometry, and where any vertex sits at or past
    /// the vanishing line -- the first has nothing to bound, and the second has
    /// no finite bound to give.
    fn covered(batch: &Batch, target: Target) -> Option<Rect> {
        let (w, h) = (target.extent.width as f32, target.extent.height as f32);
        let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
        let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for vertex in batch.vertices() {
            let [x, y, w_clip] = vertex.position;
            // The rasterizer divides by this, so a vertex on the vanishing line
            // has no device position at all and one just past it has a position
            // on the wrong side. Neither can be bounded, and the honest answer
            // is to narrow nothing.
            if !w_clip.is_finite() || w_clip.abs() <= 1e-6 || !x.is_finite() || !y.is_finite() {
                return None;
            }
            let dx = (x / w_clip + 1.0) * 0.5 * w;
            let dy = (1.0 - y / w_clip) * 0.5 * h;
            left = left.min(dx);
            right = right.max(dx);
            top = top.min(dy);
            bottom = bottom.max(dy);
        }
        if !(right > left && bottom > top) {
            return None;
        }
        Some(Rect::new(
            target.origin.x + left,
            target.origin.y + top,
            target.origin.x + right,
            target.origin.y + bottom,
        ))
    }

    /// The target a layer's content would fit in, if it is worth narrowing to.
    ///
    /// `None` keeps the target the layer was opened with, which is the whole
    /// parent. That is the answer whenever the content covers it anyway, and
    /// whenever the arithmetic cannot be trusted to cover everything the pass
    /// will draw.
    fn narrowed(
        &self,
        batch: &Batch,
        filter: Option<&ImageFilter>,
        paint: Layer,
        basis: BlurBasis,
        layer: Target,
    ) -> Option<Target> {
        // A layer composited with a mode that changes the destination where the
        // source drew nothing has to cover everything it might affect, not just
        // what it covered. `DstIn` is the one that shows it: narrowed to its
        // content, it stops taking the alpha out of everything outside that,
        // and a mask leaves the rest of the picture opaque.
        //
        // The same condition upstream spells `flood_output_coverage` in
        // `ComputeSaveLayerCoverage`, over the same nine modes.
        if paint.blend.is_destructive() {
            return None;
        }
        let content = Self::covered(batch, layer)?;
        // The same expansion the caller-bounded path makes, and for the same
        // reason: the content is where the draws are, and how far the layer's
        // own blur carries them past that is this renderer's arithmetic.
        let reach = paint.reach(basis);
        let (min, max) = (
            Vec2::new(content.left - reach.x, content.top - reach.y),
            Vec2::new(content.right + reach.x, content.bottom + reach.y),
        );
        // A filter given to the layer as a whole reaches past what it is
        // handed, and by how much is the filter's own arithmetic rather than
        // anything this can assume: a composition asks both halves, a matrix
        // carrying content across the vanishing line widens to everything, and
        // a color filter needs exactly what it was given. `covering` is the
        // same question a draw carrying an image filter already asks before it
        // opens its own layer.
        //
        // After the layer's own blur and morphology, because that is the order
        // `finish_layer` applies them in: the filter sees what those produced.
        let (min, max) = match filter {
            Some(filter) => filter.covering(min, max, &|sigma| basis.reach(sigma)),
            None => (min, max),
        };
        let left = min.x.floor().max(layer.origin.x);
        let top = min.y.floor().max(layer.origin.y);
        let right = max.x.ceil().min(layer.origin.x + layer.extent.width as f32);
        let bottom = max
            .y
            .ceil()
            .min(layer.origin.y + layer.extent.height as f32);
        if !(right > left && bottom > top) {
            return None;
        }
        let narrowed = Target {
            origin: Vec2::new(left, top),
            extent: Extent2D::new((right - left) as u32, (bottom - top) as u32),
        };
        // Nothing gained, and a viewport that offsets by zero into a target of
        // the same size is a slower way to spell what it already was.
        if narrowed.extent == layer.extent {
            return None;
        }
        Some(narrowed)
    }

    fn finish_layer(&mut self, frame: LayerFrame) {
        // Read before the frame is taken apart below, and only these two are
        // needed: whether a whole-layer filter is in play, and the layer's own
        // blur and morphology, which decide how far past its content the pass
        // will draw.
        let (filter, paint) = (frame.filter.clone(), frame.paint);
        let mut batch = std::mem::replace(&mut self.batch, frame.batch);
        let sources = std::mem::replace(&mut self.sources, frame.sources);

        // A layer clears to transparent rather than to the frame's background:
        // it is composited over what is already there, so anywhere it drew
        // nothing must contribute nothing. Clearing to the background instead
        // would paint an opaque rectangle over the parent.
        let opened = self.target;
        // What the layer's draws turned out to cover. A layer opened with no
        // bounds was given the whole parent, because nothing was known then
        // about where its content would land; now the content is recorded and
        // its own geometry says. Narrowing here rather than at `save_layer` is
        // what makes the derived bound possible at all.
        //
        // The geometry is *not* rewritten to suit the smaller target -- it is
        // in clip space, and so are the materials, and a radial gradient's
        // center among them. The pass keeps the space it was recorded against
        // and carries a viewport that lands it on the narrowed target, which
        // crops instead of scaling. See `PassViewport`.
        let layer = match self.narrowed(&batch, filter.as_ref(), paint, frame.blur_basis, opened) {
            Some(narrowed) => {
                let (dx, dy) = (
                    (narrowed.origin.x - opened.origin.x) as u32,
                    (narrowed.origin.y - opened.origin.y) as u32,
                );
                // The one recorded thing the move reaches: a scissor is in
                // target pixels where the geometry is in clip space.
                batch.rebase_scissors(dx, dy, narrowed.extent);
                narrowed
            }
            None => opened,
        };
        let viewport = (layer.extent != opened.extent).then(|| PassViewport {
            offset: [
                layer.origin.x - opened.origin.x,
                layer.origin.y - opened.origin.y,
            ]
            .map(|v| -v),
            extent: opened.extent,
        });
        self.aim_at(frame.parent);

        self.finished.push(Pass {
            batch,
            descriptor: PassDescriptor {
                clear: Some([0.0; 4]),
                samples: self.pass_samples(),
                viewport,
            },
            sources,
            extent: layer.extent,
        });
        // After the pass above, which is the one the layer's own draws went
        // into, and before the composite below, which is an image quad that
        // multisampling cannot change.
        self.anti_alias = frame.anti_alias;
        self.seeded_by_redraw = frame.seeded_by_redraw;
        let mut index = self.finished.len() - 1;
        if frame.paint.blur.max_element() > 0.0 {
            index = self.blur_passes(index, layer, frame.paint.blur, frame.blur_basis);
        }
        // After the blur, because that is the order the reach above assumes
        // when a layer asks for both: the blur softens the content and the
        // morphology then works on what the blur produced.
        if let Some(morphology) = frame.paint.morphology {
            index = self.morphology_passes(index, layer, morphology, frame.blur_basis);
        }
        // Last, so a filter given to the layer as a whole sees whatever the
        // layer's own fields produced rather than the other way round. That is
        // the order a composition states: the outermost filter is peeled first
        // and becomes this layer, and anything inner was applied by the draw
        // inside it.
        //
        // The failure is swallowed rather than returned, and it is the one
        // place in this file that does so: `restore` has no result, and the
        // only filter that can refuse here is a matrix, which the two callers
        // that can supply one already refuse before opening the layer. So this
        // is unreachable rather than ignored.
        if let Some(filter) = frame.filter.clone() {
            if let Ok(filtered) = self.filter_passes(index, layer, &filter, frame.blur_basis) {
                index = filtered;
            }
        }
        let slot = self.slot_for(TextureSource::Layer(index));

        // Where the layer sits in the parent's clip space, and how much of that
        // space one unit of the layer's texture spans. Clip space runs from -1
        // to 1 and upward, so the axes are halved and Y is negated; the ratio
        // of the two extents is what makes a layer smaller than its parent
        // sample across its own full width rather than a fraction of it.
        //
        // For a full-size layer this comes out to the identity mapping between
        // the target and the image — origin at the top-left of clip space,
        // axes halved — which is what it was before bounds existed.
        let parent = frame.parent;
        let offset = layer.origin - parent.origin;
        let anchor = Vec2::new(
            -1.0 + 2.0 * offset.x / parent.extent.width as f32,
            1.0 - 2.0 * offset.y / parent.extent.height as f32,
        );
        let scale = Mat2::from_diagonal(Vec2::new(
            0.5 * parent.extent.width as f32 / layer.extent.width as f32,
            -0.5 * parent.extent.height as f32 / layer.extent.height as f32,
        ));
        // Stated in the direction the shader reads it: a fragment's clip
        // position to the texel it samples, with the anchor inside the matrix
        // rather than packed beside it.
        let clip_to_texture = Affine2::from_mat2(scale) * Affine2::from_translation(-anchor);
        // A layer asking to be transformed on the way back needs both halves
        // moved, and moving only the geometry is the mistake worth naming: the
        // mapping below carries a fragment's clip position to a texel, so
        // geometry moved without it shows the layer through a window that
        // moved rather than showing a layer that moved.
        // A matrix that folds the plane has no inverse, so there is no mapping
        // saying which texel a fragment reads -- and no geometry either, since
        // the rectangle collapses to a line. Both halves are dropped together:
        // dropping only the mapping leaves a layer drawn through a singular
        // matrix, which is nothing at all, and the fallback would be invisible.
        //
        // The matrix is stated in device pixels and the mapping is in clip
        // space, so it is conjugated into clip space and composed ahead of the
        // mapping in inverse. Both are resolved here, once, rather than tested
        // in one place and taken in another -- the geometry below moves by the
        // first and the mapping by the second, and a pair either exists or
        // neither half does.
        let projection = self.target.projection();
        let placement = frame.paint.matrix.and_then(|matrix| {
            let in_clip = projection * matrix * projection.inverse()?;
            in_clip.is_finite().then_some(())?;
            Some((matrix, in_clip.inverse()?))
        });
        let clip_to_texture = Transform2D::from(clip_to_texture);
        let clip_to_texture = match placement {
            None => clip_to_texture,
            Some((_, undo)) => clip_to_texture * undo,
        };
        let material = Material::Image {
            to_local: to_local_columns(clip_to_texture),
            slot,
            alpha: frame.paint.alpha,
            tile: TileMode::Clamp,
            // A layer composites at its own size, so nothing is between texels
            // to choose between.
            sampling: Sampling::Linear,
            source: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
        };
        let paint = RenderPaint {
            material,
            filter: frame.paint.color_filter,
            blend: frame.paint.blend,
            clip: self.clip,
            stencil: ClipState::content(self.depth),
        };
        // Exactly the layer's own rectangle, stated in device pixels and drawn
        // through the identity: a layer's contents are already where they
        // belong, the transform having been applied to the shapes inside it
        // rather than again to the finished image. Covering more than the layer
        // would sample outside it, which the clamped sampler answers by
        // smearing the edge texels across the rest of the frame.
        let whole = layer.path();
        let _ = self.renderer.fill_into(
            &mut self.batch,
            &whole,
            placement.map_or(Transform2D::IDENTITY, |(matrix, _)| matrix),
            &paint,
        );
    }

    /// Run an image filter over a pass, and answer which pass now holds it.
    ///
    /// The one place that knows how each kind of filter becomes passes, so a
    /// caller with an `ImageFilter` and a pass does not have to. A composition
    /// is its inner half then its outer, which is what composing means and is
    /// the order `peel` takes them apart in.
    ///
    /// A matrix does not become a pass. Every other kind reads its input where
    /// the fragment is and writes there; a matrix *moves* the image, so as a
    /// pass it would need a target sized for where the content went rather than
    /// where it was. A backdrop's matrix never gets here -- `backdrop_pass`
    /// peels it off and folds it into the seed, which draws into a target that
    /// was already the right size -- so what reaches this arm is a matrix
    /// somewhere inside a composition, where the seed is not what reads it, and
    /// that is refused rather than approximated.
    fn filter_passes(
        &mut self,
        source: usize,
        target: Target,
        filter: &ImageFilter,
        basis: BlurBasis,
    ) -> Result<usize> {
        Ok(match filter {
            ImageFilter::None => source,
            ImageFilter::Blur { sigma_x, sigma_y } => {
                self.blur_passes(source, target, Vec2::new(*sigma_x, *sigma_y), basis)
            }
            ImageFilter::Dilate { radius_x, radius_y } => self.morphology_passes(
                source,
                target,
                Morphology::dilate(*radius_x, *radius_y),
                basis,
            ),
            ImageFilter::Erode { radius_x, radius_y } => self.morphology_passes(
                source,
                target,
                Morphology::erode(*radius_x, *radius_y),
                basis,
            ),
            ImageFilter::Runtime { program, uniforms } => {
                self.runtime_pass(source, target, *program, uniforms)
            }
            ImageFilter::Color(recolor) => self.recolor_pass(source, target, *recolor),
            ImageFilter::Compose { outer, inner } => {
                let inner = self.filter_passes(source, target, inner, basis)?;
                self.filter_passes(inner, target, outer, basis)?
            }
            ImageFilter::Matrix { .. } => {
                return Err(Error::Unsupported(
                    "a matrix inside a composition is not available as a filter; it \
                     moves the image rather than recomputing it in place, and only \
                     the outermost one folds into a backdrop's seed",
                ))
            }
        })
    }

    /// A color filter run over a pass, and the pass it landed at.
    ///
    /// The image material with the filter on the paint rather than on the
    /// material, which is where a color filter lives: it acts on whatever the
    /// fragment computed, and here what the fragment computed is the texel.
    fn recolor_pass(&mut self, source: usize, target: Target, recolor: ColorFilter) -> usize {
        let to_local = to_local_columns(Transform2D::from(
            Affine2::from_mat2(Mat2::from_diagonal(Vec2::new(0.5, -0.5)))
                * Affine2::from_translation(Vec2::new(1.0, -1.0)),
        ));
        self.filter_pass_recolored(
            source,
            target,
            Material::Image {
                to_local,
                slot: 0,
                alpha: 1.0,
                tile: TileMode::Clamp,
                sampling: Sampling::Linear,
                source: [0.0, 0.0, 1.0, 1.0],
                tint: [1.0, 1.0, 1.0, 1.0],
            },
            recolor,
        )
    }

    /// A caller's program run over a finished layer, and the pass it landed at.
    ///
    /// The whole of what a runtime image filter is, and it is short for the
    /// reason recorded on [`ImageFilter::Runtime`]: a filter pass covers a
    /// target the size of its source, so a fragment's clip position is already
    /// its texture coordinate and the program needs no mapping handed to it.
    /// Upstream re-rasterizes its input to arrange the same thing.
    ///
    /// Slot zero, because `filter_pass` gives the pass one source and that
    /// source is the layer being filtered. A program declaring the binding
    /// every draw already fills reads it and nothing further is bound.
    fn runtime_pass(
        &mut self,
        source: usize,
        target: Target,
        program: u32,
        uniforms: &[f32],
    ) -> usize {
        let mut textures = [None; emblema_hal::MAX_EFFECT_TEXTURES];
        textures[0] = Some(0);
        self.filter_pass(
            source,
            target,
            Material::Runtime {
                program,
                uniforms: uniforms.to_vec(),
                textures,
            },
        )
    }

    /// One pass covering `target`, drawing `material` over all of it, and the
    /// index it landed at.
    ///
    /// What every image filter is made of. The material carries the mapping
    /// from the quad's clip position to the sampled texture's coordinates --
    /// the same pair a layer composite uses at zero offset, since a filter's
    /// target is the same size as its source -- and the filtering itself. The
    /// composite that follows is untouched, and still applies the layer's alpha
    /// and blend: keeping the filter passes pure means neither has to know
    /// about compositing.
    fn filter_pass(&mut self, source: usize, target: Target, material: Material) -> usize {
        self.filter_pass_recolored(source, target, material, ColorFilter::None)
    }

    /// The same, with a color filter applied to what the material produced.
    fn filter_pass_recolored(
        &mut self,
        source: usize,
        target: Target,
        material: Material,
        recolor: ColorFilter,
    ) -> usize {
        // A pass of its own, so its slot table starts empty and the one slot it
        // uses is the pass it samples.
        let mut batch = Batch::new();
        let sources = vec![TextureSource::Layer(source)];
        let paint = RenderPaint {
            material,
            filter: recolor,
            // Replaces rather than blends: the target is cleared and this
            // covers all of it, so anything else would blend against the clear
            // for no reason.
            blend: BlendMode::Src,
            clip: None,
            stencil: ClipState::UNCLIPPED,
        };
        let quad = target.path();
        // The canvas's own renderer, aimed at the filter target for the one
        // draw and put back afterward. A fresh `Renderer` here would build a
        // pair of tessellators per pass per filtered layer per frame, for a
        // quad -- and would be the kind of allocation that never shows up in a
        // profile as itself.
        self.renderer.set_viewport(target.origin, target.extent);
        let _ = self
            .renderer
            .fill_into(&mut batch, &quad, Affine2::IDENTITY, &paint);
        self.renderer
            .set_viewport(self.target.origin, self.target.extent);

        self.finished.push(Pass {
            batch,
            descriptor: PassDescriptor {
                clear: Some([0.0; 4]),
                // One sample: this reads a resolved image and writes another,
                // so multisampling it would resolve twice for no difference.
                samples: 1,
                viewport: None,
            },
            sources,
            extent: target.extent,
        });
        self.finished.len() - 1
    }

    /// Spread or shrink a finished layer, and answer which pass holds the
    /// result.
    ///
    /// One pass per axis where the radius fits the shader's tap budget, and
    /// more where it does not. Splitting is exact rather than an
    /// approximation: dilating by `a` and then by `b` dilates by `a + b`,
    /// because the structuring elements add, and the same holds for erosion.
    /// So a radius of eighty runs as thirty-two, thirty-two and sixteen, and
    /// the picture is the one a single pass of eighty would have given.
    ///
    /// An axis with no radius contributes no passes at all. A dilation of ten
    /// horizontally and none vertically is one pass, not two, and the second
    /// would only have copied the image.
    fn morphology_passes(
        &mut self,
        source: usize,
        target: Target,
        morphology: Morphology,
        basis: BlurBasis,
    ) -> usize {
        // A filter pass covers the whole target, so its mapping is the fixed
        // one from clip space to the unit square and carries no transform of
        // the caller's at all. That is what keeps `step` -- a constant offset
        // between taps, in texels -- meaning the same thing at every fragment,
        // which a mapping with perspective would not.
        let to_local = to_local_columns(Transform2D::from(
            Affine2::from_mat2(Mat2::from_diagonal(Vec2::new(0.5, -0.5)))
                * Affine2::from_translation(Vec2::new(1.0, -1.0)),
        ));
        // Clamped to what the target can tell apart. The loop below emits a
        // pass per `MORPHOLOGY_TAPS` texels of radius, so without this a
        // caller's float decides how many passes a recording holds --
        // `Morphology::applied_radius` has the arithmetic and the case that
        // found it.
        let radius = morphology.applied_radius(target.extent);
        // Along the caller's axes once the transform has turned them, not the
        // target's -- the same arrangement and the same arithmetic as
        // `blur_passes`, for the same reason `non-parity.md` 14 gives: a layer's
        // content cannot be re-rendered into a space of its own choosing after
        // the fact, so the passes turn instead of the space. A unit device
        // direction moves `d.x` texels across and `d.y` down, so in normalized
        // coordinates it is `d` divided componentwise by the extent, which
        // reduces to the pair of axis steps this used to state whenever the
        // directions are the axes.
        //
        // A dilation is a maximum over a set of sample positions, and turning
        // the set is what makes a rotated square dilate to a square rather than
        // to one standing on the screen's axes.
        let (w, h) = (target.extent.width as f32, target.extent.height as f32);
        let [u, v] = basis.directions;
        let axes = [
            ([u.x / w, u.y / h], radius[0]),
            ([v.x / w, v.y / h], radius[1]),
        ];

        let mut sampled = source;
        for (step, radius) in axes {
            let mut left = radius;
            while left > 0.0 {
                let taken = left.min(MORPHOLOGY_TAPS as f32);
                left -= taken;
                let material = Material::Morphology {
                    to_local,
                    slot: 0,
                    step,
                    radius: taken,
                    dilate: morphology.dilate,
                };
                sampled = self.filter_pass(sampled, target, material);
            }
        }
        sampled
    }

    /// Blur a finished layer, and answer which pass now holds the result.
    ///
    /// Two passes, one per axis, because a two-dimensional Gaussian is the
    /// product of two one-dimensional ones: the same picture as a square of
    /// taps, at a fraction of the work. Each renders a quad covering its whole
    /// target and samples the pass before it.
    ///
    /// Both are the size of the layer, so a bounded layer's blur costs what the
    /// bound says and not what the frame is. The composite that follows is
    /// unchanged, and still applies the layer's alpha and blend -- keeping the
    /// blur passes pure means neither has to know about compositing.
    /// Halve a pass into a target of half its size, by sampling it.
    ///
    /// A linear sample taken at the center of a two-by-two block averages
    /// exactly those four texels, so halving with the sampler *is* a box
    /// filter and needs no kernel of its own. That is the whole reason the
    /// reduction below is a chain of halvings rather than one jump to the
    /// final size: a single bilinear tap spanning an eight-by-eight block
    /// reads four of its sixty-four texels and calls the rest absent, which is
    /// how a downsample turns a smooth image into a crawling one.
    fn halve_pass(&mut self, source: usize, into: Target) -> usize {
        // Clip space to the unit square, the same fixed mapping a blur pass
        // uses: a filter covers its whole target and samples the whole of what
        // it was given, whatever the two sizes are.
        let to_local = to_local_columns(Transform2D::from(
            Affine2::from_mat2(Mat2::from_diagonal(Vec2::new(0.5, -0.5)))
                * Affine2::from_translation(Vec2::new(1.0, -1.0)),
        ));
        self.filter_pass(
            source,
            into,
            Material::Image {
                to_local,
                slot: 0,
                alpha: 1.0,
                tile: TileMode::Clamp,
                sampling: Sampling::Linear,
                source: [0.0, 0.0, 1.0, 1.0],
                tint: [1.0, 1.0, 1.0, 1.0],
            },
        )
    }

    /// How far to shrink a target before blurring it, as a power of two.
    ///
    /// The taps are one per texel while the radius fits in the budget and
    /// spread apart once it does not, which keeps a wide blur wide but samples
    /// it more and more coarsely -- past a deviation of about nineteen the gaps
    /// between taps open and a smooth ramp starts to show them. Upstream
    /// answers this by blurring a smaller copy, so the taps stay one per texel
    /// and it is the *image* that loses detail rather than the kernel. For a
    /// blur this wide that detail was leaving anyway.
    ///
    /// A power of two so that every step is an exact halving, which is what
    /// makes the sampler a box filter above.
    fn blur_downsample(sigma: f32, extent: u32) -> u32 {
        // A sigma that is not a length asks for no reduction. `with_blur_xy`
        // refuses one, but `Layer::blur` is a public field and a struct literal
        // carries whatever it likes past that -- and an infinite sigma makes the
        // condition below true however far the reduction goes, leaving the extent
        // as the loop's only stop.
        if !sigma.is_finite() || sigma <= 0.0 {
            return 1;
        }
        let mut scale = 1u32;
        // Stopped once the axis would round to nothing. The extent is floored
        // at one texel where it is built, so this is not what keeps a target
        // from having no pixels -- it is what keeps the reduction from spending
        // passes halving a single texel into itself, which a deviation of a few
        // hundred against a small layer will otherwise ask for.
        //
        // Written as `scale <= extent / 2` rather than `extent / (scale * 2) >= 1`,
        // which says the same thing and computes `scale * 2`. That product
        // overflows once `scale` reaches two to the thirty-first, which a layer
        // whose bounds make the extent that large reaches -- and the overflow was
        // not the worse half: in release the multiply wraps to zero and the
        // division that followed it panicked instead, so both profiles died on the
        // same input. Found by a generated sequence of canvas calls.
        while blur_radius(sigma / scale as f32) > BLUR_MAX_TAPS && scale <= extent / 2 {
            scale *= 2;
        }
        scale
    }

    fn blur_passes(
        &mut self,
        source: usize,
        target: Target,
        sigma: Vec2,
        basis: BlurBasis,
    ) -> usize {
        // Clip space spans two units and runs upward, so this is the mapping
        // that turns a full-target quad's clip position into the texture
        // coordinates of the pass it samples -- the same pair a layer
        // composite uses at zero offset, since these targets are the same size.
        // A filter pass covers the whole target, so its mapping is the fixed
        // one from clip space to the unit square and carries no transform of
        // the caller's at all. That is what keeps `step` -- a constant offset
        // between taps, in texels -- meaning the same thing at every fragment,
        // which a mapping with perspective would not.
        let to_local = to_local_columns(Transform2D::from(
            Affine2::from_mat2(Mat2::from_diagonal(Vec2::new(0.5, -0.5)))
                * Affine2::from_translation(Vec2::new(1.0, -1.0)),
        ));
        // Past the tap budget the image is shrunk rather than the taps spread,
        // which is upstream's answer and keeps the taps one per texel. Halved
        // repeatedly, so each step is an exact box filter; the deviation shrinks
        // with the image, and the composite that puts the layer back scales it
        // up again -- it maps clip space to a normalized coordinate, so it does
        // not care what resolution answers.
        // Per axis, because the deviations are. An axis blurred hard needs its
        // reduction and an axis blurred not at all must not be reduced with it
        // -- shrinking and enlarging a sharp axis for the sake of the other one
        // would soften it, which is the whole difference between a directional
        // blur and a soft smear.
        //
        // Only where the passes run along the target's own axes, though. Turned
        // off them, reducing one axis and not the other would shear the
        // directions the steps are stated in: two perpendicular directions in a
        // stretched image are no longer perpendicular, and separability is
        // exactly the property that would be lost. So a turned blur reduces both
        // axes together, which is a similarity and leaves the angle alone.
        let scale = if basis.is_axis_aligned() {
            [
                Self::blur_downsample(sigma.x, target.extent.width),
                Self::blur_downsample(sigma.y, target.extent.height),
            ]
        } else {
            let smallest = target.extent.width.min(target.extent.height);
            let together = Self::blur_downsample(sigma.x, smallest)
                .max(Self::blur_downsample(sigma.y, smallest));
            [together, together]
        };
        let reduced = |by: [u32; 2]| Target {
            origin: target.origin,
            extent: Extent2D::new(
                (target.extent.width / by[0]).max(1),
                (target.extent.height / by[1]).max(1),
            ),
        };
        let blurred = reduced(scale);
        let sigma = Vec2::new(sigma.x / scale[0] as f32, sigma.y / scale[1] as f32);

        let mut source = source;
        let mut step_scale = [1u32; 2];
        while step_scale[0] < scale[0] || step_scale[1] < scale[1] {
            // Each iteration halves whichever axes are still above their own
            // target, so every step remains at most a halving per axis and
            // therefore an exact box filter, which is the property the
            // reduction rests on.
            for axis in 0..2 {
                if step_scale[axis] < scale[axis] {
                    step_scale[axis] *= 2;
                }
            }
            source = self.halve_pass(source, reduced(step_scale));
        }

        // A step of one texel along each axis, in the sampled texture's own
        // coordinates. The shader cannot derive this: it does not know the size
        // of what it is sampling.
        //
        // A unit device direction moves `d.x` texels across and `d.y` down, so
        // in normalized coordinates it is `d` divided componentwise by the
        // extent. That reduces to the pair of axis steps this used to state when
        // the directions are the axes, and it is the whole of what makes a blur
        // turn with its caller.
        let (w, h) = (blurred.extent.width as f32, blurred.extent.height as f32);
        let [u, v] = basis.directions;
        let steps = [(sigma.x, [u.x / w, u.y / h]), (sigma.y, [v.x / w, v.y / h])];

        let mut sampled = source;
        for (sigma, step) in steps {
            // Skipped rather than run with a deviation of zero. A zero-sigma
            // pass is the identity in arithmetic and a resample in fact, and a
            // blur stated along one axis is exactly the case where the other
            // pass must leave the image alone.
            if !ImageFilter::blurs(sigma) {
                continue;
            }
            sampled = self.filter_pass(
                sampled,
                blurred,
                Material::Blur {
                    to_local,
                    slot: 0,
                    step,
                    sigma,
                },
            );
        }
        sampled
    }

    /// Point both the canvas and its renderer at a target.
    ///
    /// Together rather than separately: the canvas derives a paint's mapping
    /// from the target and the renderer derives the geometry's, and the two
    /// have to agree or a shape lands in one place while its gradient runs
    /// through another.
    fn aim_at(&mut self, target: Target) {
        self.target = target;
        self.renderer.set_viewport(target.origin, target.extent);
    }

    /// The sample count a pass renders at.
    fn pass_samples(&self) -> u32 {
        // One sample unless something asked for antialiasing, because
        // multisampling costs bandwidth and a frame of solid rectangles gains
        // nothing from it.
        if self.anti_alias {
            self.samples
        } else {
            1
        }
    }

    /// Finish recording.
    pub fn finish(mut self) -> Recording {
        // A layer left open is a caller mistake, and the useful recovery is to
        // composite it anyway: the alternative is silently dropping everything
        // drawn since the unbalanced `save_layer`, which looks like a rendering
        // fault rather than like the missing `restore` it is.
        while self.stack.iter().any(|s| s.layer.is_some()) {
            self.restore();
        }

        let samples = self.pass_samples();
        let root = Pass {
            batch: self.batch,
            descriptor: PassDescriptor {
                clear: self
                    .background
                    .map(|c| c.to_array())
                    .or_else(|| self.seeded_by_redraw.then_some([0.0; 4])),
                samples,
                // The root pass renders into the caller's surface, which is the
                // space its geometry was recorded against.
                viewport: None,
            },
            sources: self.sources,
            extent: self.extent,
        };
        let mut passes = self.finished;
        passes.push(root);
        Recording {
            passes,
            extent: self.extent,
            ramps: self.ramps,
        }
    }
}

/// How much a transform magnifies, for converting a length into device pixels.
///
/// The larger of the two basis lengths, which is what upstream's
/// `GetMaxBasisLengthXY` gives and what `thin_stroke` already uses for the same
/// kind of conversion. A projective transform has no single answer -- it
/// magnifies differently at every point -- so it converts by its affine part,
/// which is exact wherever the perspective is mild and is the same reading the
/// stroke widening takes.
fn max_scale_of(transform: Transform2D) -> f32 {
    match transform.to_affine() {
        Some(affine) => max_scale(&affine),
        None => 1.0,
    }
}

/// How far each of the caller's own axes is stretched, separately.
///
/// `max_scale_of` answers the same question with one number, which is the right
/// answer for a length that has no axis -- a stroke's width, a tessellation
/// tolerance -- and the wrong one for a pair of lengths that do. Under
/// `scale(2, 5)` it reports five for both, so a radius stated per axis comes out
/// five times too long on one of them.
///
/// The lengths of the transformed basis vectors, which is what upstream's
/// `ExtractScale` takes. A rotation therefore contributes nothing to either,
/// which is correct: it turns the axes rather than stretching them, and
/// `BlurBasis` is what carries the turn.
///
/// Falls back to the single factor where there is no basis to read -- a
/// transform with perspective has no constant one -- so the axis-free answer is
/// what a filter gets exactly where a per-axis answer would be invented.
fn axis_scales_of(transform: Transform2D) -> Vec2 {
    let Some(affine) = transform.to_affine() else {
        return Vec2::splat(max_scale_of(transform));
    };
    let lengths = Vec2::new(
        affine.matrix2.x_axis.length(),
        affine.matrix2.y_axis.length(),
    );
    if !lengths.x.is_finite() || !lengths.y.is_finite() || lengths.min_element() <= 0.0 {
        return Vec2::splat(max_scale_of(transform));
    }
    lengths
}

/// The constant that makes four cubics approximate a circle.
const KAPPA: f32 = 0.552_284_8;

/// The smallest a stroke is drawn, in device pixels.
///
/// Upstream's `kMinStrokeSize`, and the same value for the same reason: a
/// stroke narrower than a pixel has no way to cover one, so it either lands on
/// a sample or misses it. Widening to exactly one and paying for the width in
/// alpha is what makes a thin line fade instead of flicker.
const MIN_STROKE_PIXELS: f32 = 1.0;

/// The width a stroke is drawn at, and the alpha that pays for the difference.
///
/// A stroke narrower than a device pixel cannot be drawn at the width it asks
/// for. Multisampling quantizes it: at four samples the only coverages a
/// sub-pixel line can have are none, a quarter, a half and three quarters, so
/// a line a fifth of a pixel wide draws at a quarter or vanishes outright
/// depending on where it falls. Measured here before this existed, a stroke of
/// 0.18 device pixels and one of 0.3 drew identically, and one of 0.15 drew
/// nothing at all.
///
/// So the geometry is widened to a whole pixel and the alpha is scaled down to
/// pay for it, which is what upstream does — `ComputePixelHalfWidth` takes
/// `max(width, kMinStrokeSize / max_basis)` and `ComputeStrokeAlphaCoverage`
/// returns `clamp(scaled_width * 2, 0, 1)`, a factor its own comment calls
/// eyeballed from Skia. Both are carried rather than improved on: the whole
/// point is that a Flutter app's hairlines look here as they look there, and a
/// different constant would be a different picture at every sub-pixel width.
///
/// **Zero is a hairline**, which is upstream's reading and `dart:ui`'s
/// documented one: "Defaults to 0.0, which correspond to a hairline width".
/// Upstream spells it as an explicit exception rather than letting it fall out
/// of the arithmetic — `ComputeStrokeAlphaCoverage` opens with
/// `if (scaled_stroke_width == 0.0 || scaled_stroke_width >= kMinStrokeSize)
/// return 1.0`, so zero takes the widening to a whole pixel and pays nothing in
/// alpha for it.
///
/// This renderer read zero as no stroke until 2026-09-17, on the reasoning that
/// a caller animating a width to nothing should dim continuously rather than
/// jump back to full at the end. That reasoning is sound and it is not
/// upstream's: the jump is in upstream on purpose, and zero is the *default*
/// value of `Paint.strokeWidth` rather than an edge a caller has to ask for --
/// so a Flutter app that strokes without setting a width drew a hairline there
/// and nothing here. Parity wins, and the discontinuity comes with it.
///
/// Which zero means which is what [`Zero`] carries. The analytic route spells a
/// *fill* as a stroke width of zero, and widening that one would put a
/// one-pixel outline around every filled shape, so the distinction cannot be
/// left to the number.
///
/// A projective transform is left alone too. The widening asks how much a
/// transform stretches, which for a homography is a different answer at every
/// point; upstream guards the same case with `HasPerspective2D`.
fn thin_stroke(transform: Transform2D, width: f32, zero: Zero) -> (f32, f32) {
    let Some(affine) = transform.to_affine() else {
        return (width, 1.0);
    };
    if !width.is_finite() || width < 0.0 {
        return (width, 1.0);
    }
    if width == 0.0 && zero == Zero::IsNotAStroke {
        return (width, 1.0);
    }
    let basis = max_scale(&affine);
    if !basis.is_finite() || basis <= 0.0 {
        return (width, 1.0);
    }
    let scaled = basis * width;
    if scaled >= MIN_STROKE_PIXELS {
        return (width, 1.0);
    }
    // Upstream's exception, and the reason it is one: the dimming below would
    // take a width of zero to an alpha of zero, which is a hairline nobody can
    // see rather than the thinnest line the device can draw.
    let coverage = match scaled == 0.0 {
        true => 1.0,
        false => (scaled * 2.0).clamp(0.0, 1.0),
    };
    (MIN_STROKE_PIXELS / basis, coverage)
}

/// The radius a point is drawn at, which for a small one is wider than asked.
///
/// The same problem `thin_stroke` solves and a different constant, both taken
/// from upstream. A point smaller than a pixel lands on a sample or misses it,
/// so `PointFieldGeometry` widens the *radius* to `0.5f / max_basis` --
/// `std::max(radius_, min_size)`, half a pixel, so the point comes out one pixel
/// across. A stroke's constant is a whole pixel because a stroke width spans the
/// line rather than reaching from its middle.
///
/// Nothing is dimmed to pay for it, which is upstream's rule and not an omission
/// here: a stroke fades as it thins and a point does not. A point is a disc
/// whose area falls away as the square of its radius, so dimming it as well
/// would take it to nothing twice over, and a field of them is a scatter plot
/// whose small marks are the ones a reader has to see.
///
/// A projective transform is left alone, as it is for a stroke: the stretch a
/// homography applies is a different answer at every point.
fn point_radius(transform: Transform2D, width: f32) -> f32 {
    let radius = width / 2.0;
    let Some(affine) = transform.to_affine() else {
        return radius;
    };
    if !radius.is_finite() || radius < 0.0 {
        return radius;
    }
    let basis = max_scale(&affine);
    if !basis.is_finite() || basis <= 0.0 {
        return radius;
    }
    radius.max(MIN_POINT_RADIUS_PIXELS / basis)
}

/// When two coordinates are the same coordinate, for deciding axis alignment.
///
/// Upstream's `kEhCloseEnough`, and the same value. A line off vertical by less
/// than this covers the same pixels a vertical one does, so treating it as
/// vertical moves nothing a reader could see.
const CLOSE_ENOUGH: f32 = 1e-3;

/// Half a device pixel, so the smallest point drawn covers one.
///
/// Upstream's `min_size` in `PointFieldGeometry::GetPositionBuffer`, which is
/// `0.5f / max_basis` rather than [`MIN_STROKE_PIXELS`] over it, for the reason
/// [`point_radius`] gives: this one is a radius.
const MIN_POINT_RADIUS_PIXELS: f32 = 0.5;

/// What a stroke width of zero means at a call site, which is not always the
/// same thing.
///
/// A caller asking for a zero-width stroke wants a hairline. The analytic route
/// reaches [`thin_stroke`] through `analytic_stroke`, which reports a *fill* as
/// a width of zero -- and a fill widened to a pixel would grow an outline it
/// never asked for. So the two are told apart here rather than by the number,
/// which they share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zero {
    /// A stroke of no width, which draws the thinnest line the device can.
    IsAHairline,
    /// Not a stroke at all, so there is no width to widen.
    IsNotAStroke,
}

/// A color at `factor` of its opacity.
///
/// The analytic route's materials carry one color rather than a material this
/// can be asked of, so the two spellings of the same dimming sit beside each
/// other: [`Material::with_opacity`] and this.
fn dimmed(color: crate::Color, factor: f32) -> [f32; 4] {
    let mut c = color.to_array();
    c[3] *= factor;
    c
}

/// The stroke width a paint implies for a fragment-evaluated shape, if one can
/// be drawn that way at all.
///
/// `Some(0.0)` fills, `Some(w)` traces an outline, and `None` means this paint
/// has to be tessellated: without antialiasing there is nothing a distance
/// field offers that triangles do not, and a width that is not finite and
/// positive is not a stroke.
///
/// The joins and caps a stroke style also carries are ignored rather than
/// refused. These shapes are closed curves with no corners to join and no ends
/// to cap, so every setting produces the same outline -- which is why a stroked
/// one can take this path at all.
fn analytic_stroke(paint: &Paint) -> Option<f32> {
    if !paint.anti_alias {
        return None;
    }
    // The shape is drawn on a quad larger than itself and emits a fragment
    // everywhere on it, so a mode that discards the destination where the
    // source is transparent would erase what is behind it in the gap between
    // the two -- and in the corner of a rounded rectangle that gap is most of
    // the corner. Tessellating covers only the shape and has no such gap.
    if !paint.blend.respects_coverage() {
        return None;
    }
    // A distance field describes a continuous outline and has no notion of a
    // position along it, so a dashed stroke cannot be expressed this way at
    // all. Falling back to tessellation is what makes the dash appear; without
    // this the rounded rectangle and the circle would draw a solid outline and
    // silently ignore the pattern, which is the shape of bug the analytic path
    // is most able to hide -- the result looks like a stroke, because it is
    // one.
    if paint.dash.as_ref().is_some_and(|dash| dash.is_usable()) {
        return None;
    }
    // A mask blur is a layer around the draw, and a distance field evaluated on
    // a quad is not a shape that can be put inside one and blurred: the quad is
    // larger than the shape and the layer would blur its edges too.
    if paint.mask_blur > 0.0 {
        return None;
    }
    // An image filter is a layer around the draw for the same reason, and this
    // guard was missing when image filters were added -- so a blurred or moved
    // circle drew as though neither had been asked for. Nothing failed: the
    // shape was still a shape, and only a test comparing where it landed said
    // otherwise. Every route that needs a layer has to be refused here, which
    // is worth stating as the rule rather than as three cases.
    if !paint.image_filter.is_identity() {
        return None;
    }
    match &paint.style {
        Style::Fill => Some(0.0),
        Style::Stroke(stroke) => {
            let width = stroke.width;
            (width.is_finite() && width > 0.0).then_some(width)
        }
    }
}

/// An ellipse inscribed in a rectangle, as four cubics.
///
/// The same construction a circle uses, with a different radius on each axis:
/// the constant that makes a cubic approximate a quarter turn does not care
/// which, since it scales with the axis it belongs to.
fn oval_path(bounds: Rect) -> Path {
    let (cx, cy) = (
        (bounds.left + bounds.right) / 2.0,
        (bounds.top + bounds.bottom) / 2.0,
    );
    let (rx, ry) = (bounds.width() / 2.0, bounds.height() / 2.0);
    let (kx, ky) = (KAPPA * rx, KAPPA * ry);
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(cx + rx, cy))
        .cubic_to(
            Vec2::new(cx + rx, cy + ky),
            Vec2::new(cx + kx, cy + ry),
            Vec2::new(cx, cy + ry),
        )
        .cubic_to(
            Vec2::new(cx - kx, cy + ry),
            Vec2::new(cx - rx, cy + ky),
            Vec2::new(cx - rx, cy),
        )
        .cubic_to(
            Vec2::new(cx - rx, cy - ky),
            Vec2::new(cx - kx, cy - ry),
            Vec2::new(cx, cy - ry),
        )
        .cubic_to(
            Vec2::new(cx + kx, cy - ry),
            Vec2::new(cx + rx, cy - ky),
            Vec2::new(cx + rx, cy),
        )
        .close();
    b.build()
}

fn circle_path(center: Vec2, radius: f32) -> Path {
    let (cx, cy) = (center.x, center.y);
    let r = radius;
    let k = KAPPA * r;
    let mut b = PathBuilder::new();
    // Recorded for the same reason a rounded rectangle's is: a shadow arrives
    // as a path, and a circle is the one oval the blurred-rectangle expression
    // describes exactly -- corners of half the side, on a square.
    b.as_rounded_rect(
        GeometryRect::new(Vec2::new(cx - r, cy - r), Vec2::new(cx + r, cy + r)),
        r,
    );
    b.move_to(Vec2::new(cx + r, cy))
        .cubic_to(
            Vec2::new(cx + r, cy + k),
            Vec2::new(cx + k, cy + r),
            Vec2::new(cx, cy + r),
        )
        .cubic_to(
            Vec2::new(cx - k, cy + r),
            Vec2::new(cx - r, cy + k),
            Vec2::new(cx - r, cy),
        )
        .cubic_to(
            Vec2::new(cx - r, cy - k),
            Vec2::new(cx - k, cy - r),
            Vec2::new(cx, cy - r),
        )
        .cubic_to(
            Vec2::new(cx + k, cy - r),
            Vec2::new(cx + r, cy - k),
            Vec2::new(cx + r, cy),
        )
        .close();
    b.build()
}

/// How far a shadow's blur reaches per unit of elevation, as a kernel radius.
///
/// The light's radius over its height, which is the ratio that decides how
/// quickly a shadow softens as its caster rises. Upstream writes it
/// `constexpr Scalar kLightRadius = 800 / 600;` in the dispatcher that draws
/// the shadow, and those are integer literals: the division happens in `int`
/// and the constant is one, not the one and a third the comment beside it
/// describes.
///
/// One is what this uses, because parity is against what upstream does rather
/// than against what it appears to have meant. This did carry the ratio the
/// comment states, and combined with reading the result as a deviation instead
/// of a radius it made every shadow here about twice as soft as the same
/// elevation gives upstream.
///
/// Note that `DlCanvas` has a second pair of these, `kShadowLightRadius` over
/// `kShadowLightHeight`, which are `DlScalar` and so do divide to one and a
/// third. Those size the shadow's *bounds*; this one draws it. Reading the
/// wrong pair is easy and gives a shadow a third too wide.
const LIGHT_RADIUS: f32 = 1.0;

/// Kernel radius per deviation, for turning one into the other.
///
/// The square root of three, matching upstream's `kKernelRadiusPerSigma`. A
/// shadow's elevation gives a radius, and the blur takes a deviation, so the
/// conversion is not optional -- and upstream's is affine rather than a plain
/// scale, adding half a pixel so that a radius shrinking to nothing still
/// leaves a deviation the blur can act on.
const KERNEL_RADIUS_PER_SIGMA: f32 = 1.732_050_8;

/// The deviation upstream blurs a shadow of this radius by.
///
/// `Radius::operator Sigma` in `impeller/geometry/sigma.cc`, which is
/// `radius / kKernelRadiusPerSigma + 0.5` for a positive radius and zero
/// otherwise.
fn sigma_for_radius(radius: f32) -> f32 {
    if radius > 0.0 {
        radius / KERNEL_RADIUS_PER_SIGMA + 0.5
    } else {
        0.0
    }
}

/// What fraction of the stated color's alpha a shadow starts from.
///
/// A quarter, matching Impeller. A shadow is a suggestion of occlusion rather
/// than an absence of light, and at full alpha it reads as a hole. This is what
/// the tonal remap below begins with rather than the alpha a shadow ends up
/// drawn at -- for anything but a gray it raises the alpha again.
const SHADOW_ALPHA: f32 = 0.25;

/// A shadow's color, tonally adjusted the way upstream adjusts it.
///
/// Ported from `DlDispatcherBase::drawShadow`, which says it ports
/// `SkShadowUtils::ComputeTonalColors`. The rule is that a colored shadow does
/// not read as a gray one tinted: a saturated shadow needs more alpha and less
/// saturation than the number a caller gave it, or it looks like a colored
/// object lying on the surface rather than an absence of light.
///
/// The arithmetic runs on **sRGB-encoded** components rather than on light,
/// because upstream's do: its pipeline holds encoded values throughout, so the
/// luminance this keys on is a luminance of encoded numbers. Computing it on
/// linear light would be the same formula answering a different question, and
/// would part company with upstream by more than the remap is worth.
///
/// It is the identity for a black shadow, which is nearly every shadow: at zero
/// luminance the color term falls out and the alpha is left at the quarter
/// above. What it changes is a colored one, and by a lot -- a fully saturated
/// red at full alpha comes out at better than twice the alpha and about
/// five eighths of the red.
fn tonal_shadow_color(color: Color) -> Color {
    let [r, g, b, a] = color.to_srgb();
    let alpha = a * SHADOW_ALPHA;

    let luminance = (r.min(g).min(b) + r.max(g).max(b)) * 0.5;

    let alpha_adjust = (2.6 + (-2.666_67 + 1.066_67 * alpha) * alpha) * alpha;
    let color_alpha = (3.544_762 + (-4.891_428 + 2.346_6 * luminance) * luminance) * luminance;
    let color_alpha = (alpha_adjust * color_alpha).clamp(0.0, 1.0);

    let grayscale_alpha = (alpha * (1.0 - 0.4 * luminance)).clamp(0.0, 1.0);

    let color_scale = color_alpha * (1.0 - grayscale_alpha);
    let tonal_alpha = color_scale + grayscale_alpha;
    // Guarded because a fully transparent shadow leaves both terms at zero, and
    // the ratio below is the only place that could divide by it.
    let unpremul_scale = if tonal_alpha != 0.0 {
        color_scale / tonal_alpha
    } else {
        0.0
    };

    Color::srgb(
        unpremul_scale * r,
        unpremul_scale * g,
        unpremul_scale * b,
        tonal_alpha,
    )
}

/// How far past its content a blur of this deviation reaches.
///
/// `(sigma - 0.5) * sqrt(3)`, which is upstream's `CalculateBlurRadius` and is
/// also where the shader stops taking taps -- so a target sized by this covers
/// everything the blur will actually read, and no more. It was three
/// deviations, which covered more of the curve than upstream does and made
/// every blur here wider than the same request gives it.
///
/// Shared rather than written twice. A bounded layer sizes its target with it,
/// and a mask blur style that combines the blurred coverage with the shape's
/// own has to size the layer holding both by the same rule -- which it did not
/// at first, and the halo was cut off square at the shape's own bounds.
/// The widest deviation a blur is asked for, matching upstream's `kMaxSigma`.
///
/// Not a limitation so much as the end of the useful range: at five hundred a
/// blur of anything smaller than a wall is a flat wash, and the reduction that
/// keeps the taps affordable has long since taken the image down to a handful
/// of texels.
pub(crate) const MAX_SIGMA: f32 = 500.0;

/// The most taps a blur takes each way from center.
///
/// Must match `max_taps` in the blur shader, which is where the budget is
/// actually spent; this is the copy that decides when to shrink the image
/// instead of spreading the taps across it.
pub(crate) const BLUR_MAX_TAPS: f32 = 32.0;

/// The kernel radius a deviation gives, in texels of whatever it is blurring.
///
/// Upstream's `CalculateBlurRadius`, which is `Radius(Sigma(sigma))`. Shared
/// so that the rule deciding how far a blur reaches, the rule sizing a target
/// to hold it, and the rule deciding whether it needs shrinking first cannot
/// drift apart -- they are the same question asked by three callers.
/// The error function, to about seven digits.
///
/// The same rational approximation the shader evaluates, and it has to be the
/// same one: `scale` is computed here and the fade there, so two different
/// approximations would normalize the shape against a curve it is not drawn
/// with. Upstream keeps a copy on each side for the same reason.
pub(crate) fn erf7(value: f32) -> f32 {
    let x = value * std::f32::consts::FRAC_2_SQRT_PI;
    let xx = x * x;
    let series = x + (0.24295 + (0.03395 + 0.0104 * xx) * xx) * (x * xx);
    series / (1.0 + series * series).sqrt()
}

pub(crate) fn blur_radius(sigma: f32) -> f32 {
    ((sigma - 0.5) * KERNEL_RADIUS_PER_SIGMA).max(0.0)
}

pub(crate) fn blur_reach(sigma: f32) -> f32 {
    if sigma > 0.0 {
        blur_radius(sigma).ceil()
    } else {
        0.0
    }
}

/// A color in the form a vertex carries one.
///
/// Premultiplied, because a vertex color is interpolated across a triangle and
/// straight color interpolated between differing alphas is wrong at every point
/// between the ends.
fn premultiplied(color: Color) -> [f32; 4] {
    let [r, g, b, a] = color.to_array();
    [r * a, g * a, b * a, a]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        Canvas::new(Extent2D::new(128, 128))
    }

    /// A backdrop layer opened under a clip is the size of the clip.
    ///
    /// It used to be the size of the parent, whatever the clip said, so a
    /// frosted panel a fortieth of the frame's area recorded a frame-sized
    /// target. The clip binds the layer's content and the draw that composites
    /// it alike, so there is nothing outside it for the layer to hold.
    ///
    /// The extents are asserted rather than the pixels because the pixels do
    /// not change: the sibling test below is what says they do not.
    #[test]
    fn a_backdrop_layer_under_a_clip_is_no_larger_than_the_clip() {
        let mut canvas = canvas();
        canvas.clear(Color::srgb(0.2, 0.3, 0.4, 1.0));
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::srgb(0.9, 0.2, 0.2, 1.0)),
            )
            .expect("the ground");
        canvas.save();
        canvas
            .clip_rect(Rect::new(40.0, 40.0, 80.0, 80.0))
            .expect("the clip");
        canvas.save_layer(Layer::opacity(1.0).with_backdrop_blur(4.0));
        canvas
            .draw_rect(
                Rect::new(40.0, 40.0, 80.0, 80.0),
                &Paint::fill(Color::srgb(1.0, 0.6, 0.0, 0.6)),
            )
            .expect("the panel");
        canvas.restore();
        canvas.restore();

        let recording = canvas.finish();
        let frame = recording.extent;
        let narrowed: Vec<_> = recording
            .passes
            .iter()
            .map(|pass| pass.extent)
            .filter(|extent| *extent != frame)
            .collect();
        // The layer's own target and both passes of the backdrop's blur. What
        // stays frame-sized is the pass being captured and the one the frame
        // ends in, and neither is allocated for this layer.
        assert_eq!(
            narrowed.len(),
            3,
            "three passes should be sized to the clip, and {} are: {:?}",
            narrowed.len(),
            recording
                .passes
                .iter()
                .map(|pass| pass.extent)
                .collect::<Vec<_>>()
        );
        for extent in narrowed {
            assert!(
                extent.width <= 48 && extent.height <= 48,
                "a pass came out {}x{} for a clip forty pixels across",
                extent.width,
                extent.height
            );
        }
    }

    /// A layer sharing a backdrop key is narrowed too, and its capture is not.
    ///
    /// These two are separate regions and were once excluded together. The
    /// layer's own target is bound by the clip exactly as an unshared one is.
    /// What the key shares is the capture and the filtered image over it, and
    /// those have to cover every group naming the key -- so they stay the
    /// parent's size, and a second group somewhere else reads the same pass.
    #[test]
    fn a_shared_backdrop_layer_is_narrowed_while_its_capture_is_not() {
        let mut canvas = canvas();
        canvas.clear(Color::srgb(0.2, 0.3, 0.4, 1.0));
        let panel = |canvas: &mut Canvas, clip: Rect| {
            canvas.save();
            canvas.clip_rect(clip).expect("the clip");
            canvas.save_layer(
                Layer::opacity(1.0)
                    .with_backdrop_blur(4.0)
                    .with_backdrop_id(7),
            );
            canvas
                .draw_rect(clip, &Paint::fill(Color::srgb(1.0, 0.6, 0.0, 0.6)))
                .expect("the panel");
            canvas.restore();
            canvas.restore();
        };
        // Opposite corners, so nothing can pass by covering one with the other.
        panel(&mut canvas, Rect::new(8.0, 8.0, 48.0, 48.0));
        panel(&mut canvas, Rect::new(80.0, 80.0, 120.0, 120.0));

        let recording = canvas.finish();
        let frame = recording.extent;
        let extents: Vec<_> = recording.passes.iter().map(|pass| pass.extent).collect();
        let narrowed: Vec<_> = extents.iter().filter(|e| **e != frame).collect();
        // One target per panel, and nothing else: the capture and the two
        // passes its blur takes are shared, and all three cover the parent.
        assert_eq!(
            narrowed.len(),
            2,
            "one narrowed target per panel and no more, in {extents:?}"
        );
        for extent in narrowed {
            assert!(
                extent.width <= 40 && extent.height <= 40,
                "a panel target came out {}x{} for a clip forty pixels across",
                extent.width,
                extent.height
            );
        }
        // The second panel pays for its own target and nothing else: the cut,
        // and the two passes of the blur over it, are taken once between them.
        // Six in all -- the cut, the blur's two, a target each, and the pass
        // the frame ends in, which holds both composites.
        assert_eq!(
            extents.len(),
            6,
            "a cut, two blur passes, a target each, and the frame's own, in \
             {extents:?}"
        );
    }

    /// And it draws what stating the clip as the layer's bounds would.
    ///
    /// The narrowing above is an optimization, so the thing to check is that it
    /// is only that. A caller who writes the bounds out has always taken the
    /// bounded path; a caller who relies on the clip now takes it too, and the
    /// two recordings have to describe the same picture.
    #[test]
    fn narrowing_to_the_clip_records_what_stating_it_would() {
        let build = |bounded: bool| {
            let mut canvas = canvas();
            canvas.clear(Color::srgb(0.1, 0.1, 0.1, 1.0));
            canvas
                .draw_rect(
                    Rect::new(0.0, 20.0, 128.0, 60.0),
                    &Paint::fill(Color::srgb(0.9, 0.4, 0.1, 1.0)),
                )
                .expect("the ground");
            let clip = Rect::new(24.0, 34.0, 104.0, 94.0);
            canvas.save();
            canvas.clip_rect(clip).expect("the clip");
            let layer = Layer::opacity(1.0).with_backdrop_blur(6.0);
            if bounded {
                canvas.save_layer_bounds(layer, clip);
            } else {
                canvas.save_layer(layer);
            }
            canvas.restore();
            canvas.restore();
            canvas.finish()
        };
        let from_clip = build(false);
        let from_bounds = build(true);
        assert_eq!(from_clip.passes.len(), from_bounds.passes.len());
        for (a, b) in from_clip.passes.iter().zip(from_bounds.passes.iter()) {
            assert_eq!(a.extent, b.extent, "the two spellings size a pass alike");
            assert_eq!(
                a.batch.vertices(),
                b.batch.vertices(),
                "the two spellings record the same geometry"
            );
        }
    }

    /// How far a blur reaches, which four comments in this tree got wrong.
    ///
    /// The kernel is truncated at `(sigma - 0.5) * sqrt(3)` -- upstream's
    /// `kKernelRadiusPerSigma` -- and every layer sized to hold a blur asks
    /// `blur_reach` for it. Nothing pinned the arithmetic, so the prose drifted
    /// to "three deviations" in four places and stayed there: a test written
    /// against that phrase expected 88 where the renderer gives 68, and the
    /// phrase was wrong rather than the renderer.
    ///
    /// Pinned here because the number is not free to change. It decides how
    /// wide a halo is, so moving it moves every blurred picture away from
    /// upstream's -- and it decides a target's size, so getting it *smaller*
    /// cuts the halo off square, which reads as a shadow with a straight edge
    /// rather than as arithmetic.
    #[test]
    fn a_blur_reaches_the_radius_its_kernel_is_truncated_at() {
        // Sqrt three per deviation, less the half pixel upstream subtracts, and
        // rounded out to a whole texel because a target is whole texels.
        assert_eq!(blur_reach(8.0), 13.0, "ceil(7.5 * sqrt(3))");
        assert_eq!(blur_reach(2.0), 3.0, "ceil(1.5 * sqrt(3))");

        // Not three deviations, which is what the prose used to say. Stated as
        // its own assertion because the two agree closely enough at small sigma
        // that a spot check would not tell them apart.
        assert!(
            blur_reach(50.0) < 3.0 * 50.0,
            "the kernel is truncated nearer one and three quarter deviations \
             than three"
        );

        // A sigma that describes no blur reaches nowhere, and a layer asking
        // for one gets no widening rather than half a pixel of it.
        assert_eq!(blur_reach(0.0), 0.0);
        assert_eq!(blur_reach(0.5), 0.0, "the half pixel is subtracted first");
        assert_eq!(blur_reach(-1.0), 0.0);

        // And it never shrinks as the blur widens, which is what makes a target
        // sized from it safe.
        let mut previous = 0.0;
        for tenth in 0..600 {
            let reach = blur_reach(tenth as f32 / 10.0);
            assert!(
                reach >= previous,
                "reach fell at sigma {}",
                tenth as f32 / 10.0
            );
            previous = reach;
        }
    }

    #[test]
    fn drawing_accumulates_into_one_batch() {
        let mut canvas = canvas();
        let paint = Paint::fill(Color::WHITE);
        canvas
            .draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &paint)
            .unwrap();
        canvas
            .draw_rect(Rect::new(20.0, 20.0, 30.0, 30.0), &paint)
            .unwrap();

        // Two shapes, one batch: describing the frame before submitting is what
        // lets them share a pass.
        //
        // And one *draw*, which is stronger and newer. Both take the analytic
        // route, and that route carries the shape's own space on its vertices
        // rather than in its material, so two rectangles of a size differing
        // only in where they sit are the same material and merge. It was two
        // until then, with the placement inside the material making every
        // instance unequal to every other.
        let recording = canvas.finish();
        assert_eq!(recording.draw_count(), 1);
    }

    #[test]
    fn an_invisible_paint_records_nothing() {
        let mut canvas = canvas();
        let invisible = Paint::fill(Color::WHITE.with_alpha(0.0));
        canvas
            .draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &invisible)
            .unwrap();
        canvas
            .draw_circle(Vec2::new(5.0, 5.0), 4.0, &invisible)
            .unwrap();

        // Rejecting early keeps empty geometry out of the batch rather than
        // tessellating it and discovering it was empty.
        assert!(canvas.finish().is_empty());
    }

    #[test]
    fn outsetting_grows_every_side_and_negating_it_shrinks_back() {
        let rect = Rect::new(10.0, 20.0, 40.0, 60.0);
        let grown = rect.outset(5.0);
        assert_eq!(
            (grown.left, grown.top, grown.right, grown.bottom),
            (5.0, 15.0, 45.0, 65.0)
        );
        // Symmetric, which is what lets a caller undo one: a blur's reach is
        // the same in both directions and a target sized by it has to be too.
        let back = grown.outset(-5.0);
        assert_eq!(
            (back.left, back.top, back.right, back.bottom),
            (rect.left, rect.top, rect.right, rect.bottom)
        );
    }

    #[test]
    fn degenerate_shapes_record_nothing() {
        let mut canvas = canvas();
        let paint = Paint::fill(Color::WHITE);
        canvas
            .draw_rect(Rect::new(10.0, 10.0, 10.0, 20.0), &paint)
            .unwrap();
        canvas.draw_circle(Vec2::ZERO, 0.0, &paint).unwrap();
        assert!(canvas.finish().is_empty());
    }

    #[test]
    fn save_and_restore_return_the_previous_transform() {
        let mut canvas = canvas();
        canvas.translate(10.0, 20.0);
        let outer = canvas.transform();

        canvas.save();
        canvas.scale(3.0, 3.0).rotate(0.5);
        assert_ne!(canvas.transform(), outer);

        canvas.restore();
        assert_eq!(canvas.transform(), outer);
        assert_eq!(canvas.save_depth(), 0);
    }

    #[test]
    fn saves_nest() {
        let mut canvas = canvas();
        let identity = canvas.transform();
        canvas.save();
        canvas.translate(5.0, 0.0);
        canvas.save();
        canvas.translate(5.0, 0.0);
        assert_eq!(canvas.save_depth(), 2);

        canvas.restore();
        assert_eq!(
            canvas.transform(),
            Transform2D::from(Affine2::from_translation(Vec2::new(5.0, 0.0)))
        );
        canvas.restore();
        assert_eq!(canvas.transform(), identity);
    }

    #[test]
    fn restoring_too_often_leaves_the_transform_alone() {
        let mut canvas = canvas();
        canvas.translate(7.0, 3.0);
        let current = canvas.transform();
        // An unbalanced restore is a caller bug, but taking down a frame loop
        // for it is worse than continuing with a transform that is still right.
        canvas.restore().restore().restore();
        assert_eq!(canvas.transform(), current);
    }

    #[test]
    fn transforms_compose_in_the_order_applied() {
        let mut canvas = canvas();
        canvas.translate(10.0, 0.0).scale(2.0, 2.0);
        // Scale then translate, or translate then scale, place a point very
        // differently; the later call applies in the frame the earlier set up.
        let mapped = canvas.transform().project_point2(Vec2::new(1.0, 0.0));
        assert!((mapped - Vec2::new(12.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn antialiasing_is_off_unless_something_asks_for_it() {
        let mut canvas = canvas();
        let aliased = Paint::fill(Color::WHITE).with_anti_alias(false);
        canvas
            .draw_rect(Rect::from_size(10.0, 10.0), &aliased)
            .unwrap();
        // Multisampling costs bandwidth, and a frame of solid rectangles gains
        // nothing from it.
        assert_eq!(canvas.finish().root().descriptor.samples, 1);
    }

    #[test]
    fn one_antialiased_shape_antialiases_the_frame() {
        let mut canvas = canvas();
        canvas
            .draw_rect(
                Rect::from_size(10.0, 10.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .unwrap();
        // A path, because it has to be tessellated. A circle or a rounded
        // rectangle would antialias itself from a distance field and would
        // rightly leave the pass alone -- which the test below is about.
        let mut triangle = PathBuilder::new();
        triangle
            .move_to(Vec2::new(20.0, 20.0))
            .line_to(Vec2::new(40.0, 20.0))
            .line_to(Vec2::new(20.0, 40.0))
            .close();
        canvas
            .draw_path(&triangle.build(), &Paint::fill(Color::WHITE))
            .unwrap();

        // Sampling is a property of the pass, so it cannot vary per shape.
        // Honoring the request for the frame beats silently ignoring it.
        assert!(canvas.finish().root().descriptor.samples > 1);
    }

    #[test]
    fn a_shape_that_antialiases_itself_leaves_the_pass_alone() {
        // The saving that comes with the distance field, and the reason it is
        // worth having beyond the vertex count: a frame whose only antialiased
        // shapes compute their own coverage does not have to multisample, which
        // is four times the fill and four times the bandwidth for an edge it
        // was already going to get right.
        let mut canvas = canvas();
        canvas
            .draw_circle(Vec2::splat(20.0), 8.0, &Paint::fill(Color::WHITE))
            .unwrap();
        canvas
            .draw_rrect(
                Rect::new(30.0, 30.0, 60.0, 50.0),
                6.0,
                &Paint::fill(Color::WHITE),
            )
            .unwrap();
        assert_eq!(
            canvas.finish().root().descriptor.samples,
            1,
            "an analytic shape should not multisample the pass"
        );
    }

    /// A black shadow is untouched by the tonal remap; a colored one is not.
    ///
    /// Both halves matter. The remap exists for colored shadows, and if it
    /// moved a black one it would change nearly every shadow anybody draws --
    /// so the identity is the safety property and the change is the feature.
    ///
    /// The figures are upstream's formula evaluated by hand rather than
    /// recorded from this implementation, which is the only way a test of a
    /// port can fail when the port is wrong.
    #[test]
    fn a_colored_shadow_is_tonally_adjusted_and_a_black_one_is_not() {
        // Black: at zero luminance the color term vanishes and the alpha is
        // left at the quarter it started from.
        let black = super::tonal_shadow_color(Color::srgb(0.0, 0.0, 0.0, 1.0));
        let [r, g, b, a] = black.to_srgb();
        assert_eq!([r, g, b], [0.0, 0.0, 0.0], "a black shadow gained color");
        assert!(
            (a - 0.25).abs() < 1e-6,
            "a black shadow should keep the quarter alpha, got {a}"
        );

        // Saturated red at full alpha. Luminance is a half, so:
        //   alpha_adjust    = (2.6 + (-2.66667 + 1.06667*0.25)*0.25)*0.25 = 0.5
        //   color_alpha     = (3.544762 + (-4.891428 + 2.3466*0.5)*0.5)*0.5
        //                   = 0.842849, times alpha_adjust  = 0.421424
        //   grayscale_alpha = 0.25 * (1 - 0.4*0.5)           = 0.2
        //   color_scale     = 0.421424 * 0.8                 = 0.337140
        //   tonal_alpha     = 0.537140
        //   unpremul_scale  = 0.337140 / 0.537140            = 0.627650
        let red = super::tonal_shadow_color(Color::srgb(1.0, 0.0, 0.0, 1.0));
        let [r, g, b, a] = red.to_srgb();
        assert!(
            (a - 0.537_140).abs() < 1e-4,
            "the tonal alpha for a red shadow is {a}, not 0.53714"
        );
        assert!(
            (r - 0.627_650).abs() < 1e-4,
            "the tonal red for a red shadow is {r}, not 0.62765"
        );
        assert_eq!([g, b], [0.0, 0.0], "a red shadow gained other channels");

        // And it is emphatically not what taking a quarter of the alpha gives,
        // which is what this used to do.
        assert!(
            a > 0.5,
            "the remap left the alpha near the naive quarter: {a}"
        );

        // A mid-tone, which is the case that says *which space* this runs in.
        // Neither of the colors above can: zero and one are fixed points of the
        // transfer function, so a saturated primary is the same number encoded
        // or linear and the two readings agree by accident. Here they do not --
        // the same formula on linear light gives (0.137, 0.032, 0.004) at an
        // alpha of 0.410, against upstream's (0.345, 0.173, 0.058) at 0.506.
        let midtone = super::tonal_shadow_color(Color::srgb(0.6, 0.3, 0.1, 1.0));
        let [r, g, b, a] = midtone.to_srgb();
        for (got, want, name) in [
            (a, 0.506_265, "alpha"),
            (r, 0.345_193, "red"),
            (g, 0.172_596, "green"),
            (b, 0.057_532, "blue"),
        ] {
            assert!(
                (got - want).abs() < 1e-4,
                "the tonal {name} for a mid-tone shadow is {got}, not {want} -- \
                 which is what running the remap on light rather than on encoded \
                 values would give"
            );
        }
    }

    #[test]
    fn clearing_sets_the_background_rather_than_recording_a_draw() {
        let mut canvas = canvas();
        canvas.clear(Color::rgba8(20, 30, 40, 255));
        let recording = canvas.finish();
        // A clear is a pass property; recording it as a full-target rectangle
        // would cost a draw and defeat the load operation.
        assert!(recording.is_empty());
        assert!(recording.root().descriptor.clear.is_some());
    }

    /// A morphology radius cannot decide how many passes a recording holds.
    ///
    /// The loop emits a pass per `MORPHOLOGY_TAPS` texels, so before the clamp a
    /// radius of a million gave sixty-two thousand passes and `1e20` exhausted
    /// memory -- through `Morphology::dilate`, whose sanitizing rejects only what
    /// is negative or not finite. Nothing about that input was malformed.
    #[test]
    fn a_huge_morphology_radius_is_bounded_by_what_the_target_can_show() {
        // The morphology materials a radius produces, in order. Compared rather
        // than counted, because the clamp's justification is that a wider window
        // cannot change a pixel -- and identical materials over identical passes
        // is that claim in a form a machine can check without a device. It says
        // the two requests are not merely the same size but the same recording,
        // and the shader is a function of what it is handed.
        let filters = |radius: f32| {
            let mut canvas = canvas();
            canvas.clear(Color::BLACK);
            canvas.save_layer(
                Layer::opacity(1.0).with_morphology(Morphology::dilate(radius, radius)),
            );
            canvas
                .draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE))
                .expect("a rectangle is a rectangle");
            canvas.restore();
            let recording = canvas.finish();
            let morphology: Vec<Material> = recording
                .passes
                .iter()
                .flat_map(|pass| pass.batch.draws())
                .filter(|draw| matches!(draw.material, Material::Morphology { .. }))
                .map(|draw| draw.material.clone())
                .collect();
            (recording.passes.len(), morphology)
        };

        // A 128-square target, so each axis saturates at 128 texels: four passes
        // an axis at thirty-two taps each, plus the root and the composite.
        let (passes, saturated) = filters(128.0);
        assert!(passes < 16, "{passes} passes to saturate a 128 square");
        assert_eq!(saturated.len(), 8, "{saturated:?}");
        for radius in [1e3f32, 1e6, 1e20, f32::MAX] {
            let (grew, wider) = filters(radius);
            assert_eq!(
                grew, passes,
                "a radius of {radius:e} cost more passes than saturating the target"
            );
            assert_eq!(
                wider, saturated,
                "a radius of {radius:e} asked for a different filter than saturating the target"
            );
        }
    }

    /// Infinity terminates, which it did not before.
    ///
    /// `Morphology::dilate` maps it to zero, so this reaches the loop the only way
    /// left: the fields are public, so a struct literal skips the constructor.
    /// Subtracting the taps from infinity leaves infinity, so the loop never ended
    /// and each turn of it allocated a pass.
    #[test]
    fn a_non_finite_morphology_radius_is_no_filter_rather_than_no_end() {
        for radius in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let morphology = Morphology {
                radius: [radius, radius],
                dilate: true,
            };
            assert_eq!(
                morphology.applied_radius(Extent2D::new(128, 128)),
                [0.0, 0.0],
                "a radius of {radius} survived as something to apply"
            );
            let mut canvas = canvas();
            canvas.clear(Color::BLACK);
            canvas.save_layer(Layer {
                morphology: Some(morphology),
                ..Layer::opacity(1.0)
            });
            canvas.restore();
            // Reaching here at all is the assertion.
            assert!(!canvas.finish().passes.is_empty());
        }
    }

    /// The clamp is per axis, and is the extent of that axis.
    #[test]
    fn the_applied_radius_is_clamped_to_each_axis_separately() {
        let morphology = Morphology {
            radius: [1e9, 7.0],
            dilate: false,
        };
        assert_eq!(
            morphology.applied_radius(Extent2D::new(64, 256)),
            [64.0, 7.0],
            "the clamp took the wrong axis or clamped what did not need it"
        );
        // Negative is refused rather than reflected, as `sane` does.
        let negative = Morphology {
            radius: [-5.0, -0.0],
            dilate: true,
        };
        assert_eq!(negative.applied_radius(Extent2D::new(64, 64)), [0.0, 0.0]);
    }

    /// A blur's reduction terminates whatever the sigma and the extent.
    ///
    /// The reduction doubles a divisor until the kernel fits the tap budget or
    /// the axis runs out. An infinite sigma never satisfies the first, so the
    /// extent is the only stop -- and against a very large extent the divisor
    /// reached two to the thirty-first, where the old condition's `scale * 2`
    /// overflowed. Release was worse rather than better: the product wrapped to
    /// zero and the division panicked.
    ///
    /// A struct literal is how a non-finite sigma gets here at all, since
    /// `Layer::with_blur_xy` maps one to zero. `Layer::blur` is a public field.
    ///
    /// Found by `a_sequence_of_any_operations_records_something_addressable`,
    /// which generated a layer with an infinite blur inside bounds of `-1e20`.
    #[test]
    fn the_blur_reduction_terminates_for_any_sigma_and_extent() {
        // Reaching the end of this at all is most of the assertion.
        for extent in [0u32, 1, 2, 3, 128, u32::MAX / 2, u32::MAX - 1, u32::MAX] {
            for sigma in [
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NAN,
                -1.0,
                0.0,
                1e30,
                f32::MAX,
                500.0,
                8.0,
            ] {
                let scale = Canvas::blur_downsample(sigma, extent);
                assert!(
                    scale.is_power_of_two(),
                    "sigma {sigma:e} over {extent} gave a divisor of {scale}, which \
                     is not a power of two, so the halvings are not exact"
                );
                assert!(
                    scale == 1 || scale <= extent,
                    "sigma {sigma:e} reduced a {extent}-texel axis by {scale}, which \
                     is more than the axis has"
                );
            }
        }

        // A sigma that is not a length reduces nothing, on the same terms as
        // `with_blur_xy` refusing it and `Morphology::applied_radius` refusing a
        // non-finite radius.
        for sigma in [f32::INFINITY, f32::NAN, -1.0, 0.0] {
            assert_eq!(
                Canvas::blur_downsample(sigma, u32::MAX),
                1,
                "a sigma of {sigma} asked for a reduction"
            );
        }

        // And a real one still reduces, so the guard above did not turn the
        // reduction off for everything.
        assert!(
            Canvas::blur_downsample(500.0, 4096) > 1,
            "a wide blur on a large target stopped reducing, which is what the tap \
             budget exists to avoid"
        );
    }
}

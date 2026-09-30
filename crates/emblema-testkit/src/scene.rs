//! Scenes as data, and the corpus of them.
//!
//! One corpus, many executions: the same scenes drive golden comparison,
//! cross-backend conformance, performance runs, and on-device runs. A new
//! feature adds scenes once and every execution mode picks them up, which is
//! what keeps the authoring cost flat as the matrix grows.

use crate::shape::Shape;
use emblema_core::{ImageFilter, MaskBlurStyle, PointMode, VertexMode};
use emblema_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
use emblema_geometry::transform::Transform2D;
use emblema_geometry::FillRule;
use emblema_hal::{BlendMode, Extent2D, TileMode};
use emblema_hal::{ColorFilter, Sampling};
use glam::{Affine2, Mat2, Vec2};

/// A transform of the plane, as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub scale: [f32; 2],
    /// Rotation in radians, applied after scale and before translation.
    pub rotate: f32,
    /// Shear coefficients: how much of Y is added to X, and of X to Y.
    ///
    /// A class of its own rather than a special case of the other two. A shear
    /// is the only transform here that is not conformal -- it takes the right
    /// angles of a rectangle and leaves a parallelogram, so no axis survives
    /// it and a scissor cannot express a clip under one. It is also what tells
    /// a packed inverse matrix from a transposed one: under a scale, or a
    /// rotation of a symmetric shape, the two agree.
    pub skew: [f32; 2],
    pub translate: [f32; 2],
    /// The divisor's dependence on each axis, which is what makes a scene
    /// recede rather than merely shrink.
    ///
    /// Zero for everything that predates perspective, which is nearly every
    /// scene, and the reason the field could be added without touching one of
    /// them. A value here of `p` makes the divisor `1 + p · (x, y)`, so the
    /// plane is magnified where that falls below one and compressed where it
    /// rises above -- and reaches the vanishing line where it hits zero, which
    /// a scene meant to be compared across devices should stay well away from.
    pub perspective: [f32; 2],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            scale: [1.0, 1.0],
            rotate: 0.0,
            skew: [0.0, 0.0],
            translate: [0.0, 0.0],
            perspective: [0.0, 0.0],
        }
    }
}

impl Transform {
    pub fn translate(x: f32, y: f32) -> Self {
        Self {
            translate: [x, y],
            ..Default::default()
        }
    }

    pub fn scale(x: f32, y: f32) -> Self {
        Self {
            scale: [x, y],
            ..Default::default()
        }
    }

    /// Divide, then scale, then shear, then rotate, then translate.
    ///
    /// The order is stated because it is not recoverable from the result: a
    /// shear before a rotation and one after it are different transforms, and
    /// a scene that did not say which it meant would mean different things to
    /// a reader and to the executor. Scale comes first among the affine parts
    /// so a shear coefficient is read in the shape's own units rather than in
    /// scaled ones, and perspective comes before all of them for the same
    /// reason: the divisor is read in those units too, so a scene states how
    /// its own shape recedes rather than how the placed one does.
    ///
    /// It matters more here than anywhere else in this order. A perspective
    /// term before a translation and one after it are not merely different
    /// transforms; they put the vanishing line in different places.
    pub fn to_projective(self) -> Transform2D {
        let perspective = Transform2D::from_column_major_4x4(&[
            1.0,
            0.0,
            0.0,
            self.perspective[0],
            0.0,
            1.0,
            0.0,
            self.perspective[1],
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ]);
        Transform2D::from(self.affine_parts()) * perspective
    }

    /// The affine parts alone, or `None` if this asks for perspective.
    ///
    /// What a caller reaches for to check the composition order against a
    /// vector or a point, where a homography would answer the question with an
    /// extra divide in the way. Nothing in the executor needs it any more:
    /// every place a scene's transform reaches takes a homography now.
    pub fn to_affine(self) -> Option<Affine2> {
        (self.perspective == [0.0, 0.0]).then(|| self.affine_parts())
    }

    fn affine_parts(self) -> Affine2 {
        let shear = Affine2::from_mat2(Mat2::from_cols(
            Vec2::new(1.0, self.skew[1]),
            Vec2::new(self.skew[0], 1.0),
        ));
        Affine2::from_translation(Vec2::from(self.translate))
            * Affine2::from_angle(self.rotate)
            * shear
            * Affine2::from_scale(Vec2::from(self.scale))
    }
}

/// A stroke's parameters, as data.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeSpec {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    /// Alternating drawn and skipped lengths, and where in them to start.
    ///
    /// Empty for a solid stroke, which is what every scene predating dashes
    /// wants and what `new` gives.
    pub dash: Option<(Vec<f32>, f32)>,
}

/// A mesh of triangles, which is the one thing a scene draws that it did not
/// describe as a shape.
///
/// Its own node rather than a kind of [`Item`], because an item is a shape
/// with a fill and everything that follows from that -- a stroke, a clip built
/// from its outline, a transform applied to its path. A mesh has none of them:
/// it is the triangles, stated.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshSpec {
    pub mode: VertexMode,
    pub positions: Vec<[f32; 2]>,
    /// One per position, or empty. Multiplied into whatever the fill produced.
    pub colors: Vec<[f32; 4]>,
    /// One per position, or empty. Needs an image fill to read.
    pub texture_coords: Vec<[f32; 2]>,
    /// Three per triangle, or empty for the positions in order.
    pub indices: Vec<u32>,
    pub fill: Fill,
    /// How each vertex's color combines with the fill.
    pub tint_blend: BlendMode,
    pub blend: BlendMode,
    pub transform: Transform,
    /// Filter what the mesh drew. See [`Item::image_filter`].
    pub image_filter: ImageFilter,
    /// Blur the mesh's coverage before filling it. Zero for none.
    ///
    /// Only where the mesh carries no per-vertex colors: with them there is no
    /// color for the halo outside the triangles, and the draw is refused. See
    /// `Canvas::draw_vertices`.
    pub mask_blur: MaskBlur,
}

/// A color over everything the clip admits -- `drawPaint` and `drawColor`.
///
/// Its own node because it has no shape. What it covers is the clip carried
/// back through the transform, which is a rectangle the caller cannot easily
/// write once a transform is in force, and getting it from the canvas is the
/// whole of the call. A clip is named here rather than on an item because
/// there is no item: the clip is what gives this node its extent.
#[derive(Debug, Clone, PartialEq)]
pub struct PaintSpec {
    /// What it fills with, which is not always a color.
    ///
    /// `drawPaint` takes a paint, and a paint carries a shader -- so a program
    /// or a gradient over everything the clip admits is the same call as a
    /// color over it, and upstream has a scene for exactly that. Held as a
    /// [`Fill`] rather than as a color for that reason.
    pub fill: Fill,
    pub blend: BlendMode,
    /// Narrow to this before filling. Without one the fill covers the frame,
    /// which is a picture but not much of a test.
    pub clip: Option<[f32; 4]>,
    /// And remove this from what the clip admits.
    pub clip_out: Option<[f32; 4]>,
    pub transform: Transform,
}

/// A sheet stretched by its middle, keeping its corners.
///
/// Its own node because it is nine draws rather than one, and which of the nine
/// stretches in which direction is the whole of what a nine-patch means. A
/// scene naming the pieces would be describing the answer; naming the center
/// and the destination leaves the arithmetic where the call is.
#[derive(Debug, Clone, PartialEq)]
pub struct NinePatchSpec {
    /// The part of the sheet that may stretch, in texels.
    pub center: [f32; 4],
    /// Where the whole thing goes.
    pub into: [f32; 4],
    pub alpha: f32,
    pub blend: BlendMode,
    pub transform: Transform,
}

/// Points, segments or an open run through a list of positions.
///
/// Its own node rather than a shape, because what a point *is* here is the
/// stroke's cap applied to a segment of no length -- so a paint with no stroke
/// draws nothing, and the mode decides how many segments the list becomes
/// rather than what shape it is.
#[derive(Debug, Clone, PartialEq)]
pub struct PointsSpec {
    pub mode: PointMode,
    pub points: Vec<[f32; 2]>,
    pub stroke: StrokeSpec,
    pub color: [f32; 4],
    pub blend: BlendMode,
    pub transform: Transform,
    /// Confine the whole run to a shape, in the run's own space.
    ///
    /// On the run rather than on each point, which is the distinction the
    /// scene exists for: the modes that join points into lines and polygons
    /// produce several draws from one call, and a clip has to cut all of them
    /// the same way. A renderer giving those draws different depths would let
    /// the clip take some and not others.
    pub clip_shape: Option<Shape>,
}

/// A finished recording, drawn into the scene that holds it.
///
/// The children are recorded into a canvas of their own and the result is drawn
/// through `draw_recording`, which is what `dart:ui` calls `drawPicture`. The
/// distinction from a layer is what happens to the passes: a layer's are this
/// recording's from the start, and a picture's are another recording's,
/// appended with every index inside them moved along.
#[derive(Debug, Clone, PartialEq)]
pub struct PictureSpec {
    /// The size of the canvas the children are recorded into. What lands where
    /// depends on it: a picture is placed by the transform and covers its own
    /// extent, having no bounds of its own.
    pub size: Extent2D,
    pub children: Vec<Node>,
    pub transform: Transform,
    pub blend: BlendMode,
}

/// A run of glyphs, placed.
///
/// A scene names glyphs by index into the fixture set for the reason it names
/// no texture: it has to describe a picture without a device, and a font file
/// is a device of its own -- one whose version decides what the picture is. The
/// executor builds the atlas, uploads it, and supplies the slot.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRunSpec {
    /// Which fixture glyph, and where its top-left corner goes.
    pub glyphs: Vec<(u32, [f32; 2])>,
    pub color: [f32; 4],
    pub blend: BlendMode,
    pub transform: Transform,
    /// Filter what the run drew. See [`Item::image_filter`].
    pub image_filter: ImageFilter,
    /// Blur the run's coverage before filling it. Zero for none.
    pub mask_blur: f32,
    pub mask_blur_style: MaskBlurStyle,
}

/// A shape's shadow, and whether the shape will cover it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowSpec {
    pub shape: Shape,
    pub color: [f32; 4],
    /// How far above the surface the caster sits, in the canvas's own units.
    pub elevation: f32,
    /// Whether the caster will fail to hide the part beneath it.
    pub transparent_occluder: bool,
    pub transform: Transform,
    /// Draw the caster on top afterwards, which is what a shadow is for.
    pub with_caster: bool,
}

/// One piece of the fixture sheet, placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteSpec {
    /// The part of the sheet to draw, in texels.
    pub source: [f32; 4],
    /// Where it goes, as a rotation in radians, a uniform scale, and a
    /// translation. Stated in the terms a sprite is usually placed in rather
    /// than as a matrix, which is what `dart:ui` restricts this call to and
    /// what makes a scene readable.
    pub rotate: f32,
    pub scale: f32,
    pub translate: [f32; 2],
    /// Multiplied into this sprite alone. White changes nothing.
    pub color: [f32; 4],
}

/// A batch of sprites out of the fixture sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasSpec {
    pub sprites: Vec<SpriteSpec>,
    /// How each sprite's color combines with the sheet's texels.
    pub tint_blend: BlendMode,
    pub blend: BlendMode,
    /// Scales every sprite.
    pub alpha: f32,
}

/// How a scene item softens its own coverage, if it does.
///
/// On the item rather than the stroke, because a fill can be blurred too --
/// which is what a shadow under a solid shape is.
pub type MaskBlur = f32;

impl StrokeSpec {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
            dash: None,
        }
    }

    /// The same stroke, cut by a dash pattern.
    pub fn dashed(mut self, intervals: Vec<f32>, phase: f32) -> Self {
        self.dash = Some((intervals, phase));
        self
    }

    pub fn to_style(&self) -> StrokeStyle {
        StrokeStyle {
            width: self.width,
            cap: self.cap,
            join: self.join,
            miter_limit: self.miter_limit,
        }
    }
}

/// A color stop, as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    /// Linear color with straight alpha.
    pub color: [f32; 4],
    pub offset: f32,
}

impl Stop {
    pub fn new(color: [f32; 4], offset: f32) -> Self {
        Self { color, offset }
    }
}

/// What fills a shape.
///
/// Kept as data alongside the geometry so a gradient scene serializes with
/// everything else, rather than needing code to reconstruct it.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Solid([f32; 4]),
    /// A gradient between two points in the item's own coordinate space, so it
    /// travels through the item's transform with the geometry.
    LinearGradient {
        start: [f32; 2],
        end: [f32; 2],
        stops: Vec<Stop>,
        /// What fills the shape past the endpoints.
        tile: TileMode,
    },
    /// A gradient outward from a center, reaching its last stop at `radius`.
    RadialGradient {
        center: [f32; 2],
        radius: f32,
        stops: Vec<Stop>,
        /// What fills the shape past the radius.
        tile: TileMode,
    },
    /// A gradient around a center, between two angles in radians.
    SweepGradient {
        center: [f32; 2],
        start_angle: f32,
        end_angle: f32,
        stops: Vec<Stop>,
        /// What fills the directions the arc does not cover.
        tile: TileMode,
    },
    /// The fixture fragment program, with the floats it reads.
    ///
    /// A scene names no program for the same reason it names no texture: it
    /// has to be writable without a device. It says it uses the one fixture
    /// effect, and the executor registers it.
    RuntimeEffect {
        /// Which fixture program, by the index the executor registers it at:
        /// zero is the one that draws two colors either side of a threshold,
        /// one is the one that differences two textures.
        program: u32,
        uniforms: Vec<f32>,
        /// Which fixture textures the program samples, in binding order.
        ///
        /// Named as slots rather than as handles, for the reason a scene names
        /// no texture: zero is the sheet and one is the glyph atlas, which the
        /// executor uploads and binds. Empty for a program that samples
        /// nothing, which is what every plate wanted until one wanted two.
        images: Vec<u32>,
    },
    /// A piece of the fixture sheet, mapped onto a rectangle.
    ///
    /// There is one texture a scene can name, and it does not name it: the
    /// executor uploads [`crate::fixture`] when a scene needs it and supplies
    /// it as slot zero. A scene that carried a texture handle would be a
    /// scene that could not be written down without a device, which is the
    /// one property this format exists to keep.
    Image {
        /// Where in the scene the sheet lands, as `[left, top, right, bottom]`.
        rect: [f32; 4],
        /// Which part of the sheet to read, from zero to one in each axis.
        source: [f32; 4],
        tile: TileMode,
        sampling: Sampling,
        /// Scales the sampled color.
        alpha: f32,
        /// Multiplies the sampled color. White changes nothing.
        tint: [f32; 4],
    },
    /// A gradient between two circles, reaching its last stop on the second.
    ConicalGradient {
        start_center: [f32; 2],
        start_radius: f32,
        end_center: [f32; 2],
        end_radius: f32,
        stops: Vec<Stop>,
        /// What fills the parameter outside the two circles.
        tile: TileMode,
    },
}

/// A rectangle filled by a gradient that spans only a quarter of it.
///
/// Shared by the tiling scenes so the three differ in exactly one field, which
/// is what makes a difference between their images mean what it says.
fn tiled_gradient_item(tile: TileMode) -> Item {
    Item::filled(
        Shape::Rect {
            min: [4.0, 4.0],
            max: [124.0, 124.0],
        },
        Fill::LinearGradient {
            start: [4.0, 0.0],
            end: [34.0, 0.0],
            stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
            tile,
        },
    )
    // Composited rather than the corpus default of `Src`, which replaces. A
    // decal draws nothing outside the ramp, and "nothing" written by a mode
    // that replaces is a transparent hole punched through the background --
    // correct for `Src` and not what a decal means. The other two modes are
    // opaque everywhere and would not care, but they use the same blend so the
    // three scenes differ in exactly one field and a difference between their
    // images says what it looks like it says.
    .with_blend(BlendMode::SrcOver)
}

/// One thing to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Blur this item's coverage before filling it. Zero for none.
    pub mask_blur: MaskBlur,
    /// Which part of the blurred coverage survives.
    pub mask_blur_style: MaskBlurStyle,
    /// Filter what this item drew, rather than the color it computed.
    pub image_filter: ImageFilter,
    pub shape: Shape,
    /// Stroke the shape rather than filling it.
    pub stroke: Option<StrokeSpec>,
    pub transform: Transform,
    pub fill: Fill,
    /// Recolor whatever the fill produced, before blending.
    ///
    /// Here rather than left to the public API's own tests because the corpus
    /// is what compares the two backends against each other, and "the same
    /// arithmetic, translated twice" is exactly the thing it exists to check.
    pub color_filter: ColorFilter,
    pub blend: BlendMode,
    /// Confine this item to a rectangle, given in the item's own space as
    /// `[left, top, right, bottom]` and carried through its transform.
    pub clip: Option<[f32; 4]>,
    /// Keep this item out of a rectangle, stated the same way.
    ///
    /// `dart:ui` spells it `clipRect` with `ClipOp.difference`, and it applies
    /// after [`Self::clip`] so a scene can state both -- which is the case
    /// worth having, since a difference clip alone leaves the bounds it was
    /// given untouched.
    pub clip_out: Option<[f32; 4]>,
    /// Draw the shape through `draw_path` rather than through whatever call
    /// the public API offers for it.
    ///
    /// Five shapes have their own entry point -- rectangle, circle, oval,
    /// rounded rectangle and difference of two -- and a scene naming one gets
    /// that call, so the choice each makes between an analytic field and a
    /// tessellation stays under test. This says the other thing: the same
    /// outline, handed over as a path.
    ///
    /// It is not a way of writing the same picture twice. Upstream keeps both
    /// forms of a scene for the cases where the two routes can disagree -- a
    /// wide stroke around a small rectangle is one, since the analytic route
    /// covers a pixel once where a tessellated one can lay two quads over it
    /// -- and a pair here can assert that they agree, which neither plate can
    /// say alone.
    pub as_path: bool,
    /// Confine this item to an arbitrary shape, in the item's own space.
    ///
    /// Needs the stencil rather than the scissor, and so exercises a quite
    /// different path from [`Self::clip`] even though both narrow what the item
    /// may reach. Applied per item: the clip is built before the item and
    /// stepped back after it, so items stay independent of each other.
    pub clip_shape: Option<Shape>,
}

impl Item {
    /// Trace this item's outline rather than filling it, keeping whatever
    /// fill it already has.
    ///
    /// [`Self::stroke`] takes a color, which is what almost every stroke
    /// wants; this is for the ones that do not, and a gradient along a stroke
    /// is the case that needed it.
    pub fn with_stroke(mut self, spec: StrokeSpec) -> Self {
        self.stroke = Some(spec);
        self
    }

    /// Draw this item's shape as a path rather than through its own call.
    pub fn as_path(mut self) -> Self {
        self.as_path = true;
        self
    }

    /// Recolor what this item draws.
    pub fn with_color_filter(mut self, filter: ColorFilter) -> Self {
        self.color_filter = filter;
        self
    }

    pub fn fill(shape: Shape, color: [f32; 4]) -> Self {
        Self {
            mask_blur: 0.0,
            mask_blur_style: MaskBlurStyle::Normal,
            image_filter: ImageFilter::None,
            shape,
            stroke: None,
            transform: Transform::default(),
            fill: Fill::Solid(color),
            color_filter: ColorFilter::None,
            blend: BlendMode::SrcOver,
            clip: None,
            clip_out: None,
            as_path: false,
            clip_shape: None,
        }
    }

    /// A shape filled with a gradient between two points in its own space.
    pub fn gradient(shape: Shape, start: [f32; 2], end: [f32; 2], stops: Vec<Stop>) -> Self {
        Self::filled(
            shape,
            Fill::LinearGradient {
                start,
                end,
                stops,
                tile: TileMode::Clamp,
            },
        )
    }

    /// A shape filled with any fill.
    pub fn filled(shape: Shape, fill: Fill) -> Self {
        Self {
            mask_blur: 0.0,
            mask_blur_style: MaskBlurStyle::Normal,
            image_filter: ImageFilter::None,
            shape,
            stroke: None,
            transform: Transform::default(),
            fill,
            color_filter: ColorFilter::None,
            blend: BlendMode::SrcOver,
            clip: None,
            clip_out: None,
            as_path: false,
            clip_shape: None,
        }
    }

    pub fn stroke(shape: Shape, spec: StrokeSpec, color: [f32; 4]) -> Self {
        Self {
            mask_blur: 0.0,
            mask_blur_style: MaskBlurStyle::Normal,
            image_filter: ImageFilter::None,
            shape,
            stroke: Some(spec),
            transform: Transform::default(),
            fill: Fill::Solid(color),
            color_filter: ColorFilter::None,
            blend: BlendMode::SrcOver,
            clip: None,
            clip_out: None,
            as_path: false,
            clip_shape: None,
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    /// Soften this item's coverage, which is what a shadow is.
    /// Filter what this item drew. See [`ImageFilter`].
    pub fn with_image_filter(mut self, filter: ImageFilter) -> Self {
        self.image_filter = filter;
        self
    }

    /// Keep only the part of the blurred coverage the style names.
    ///
    /// Separate from [`Self::with_mask_blur`] rather than an argument to it,
    /// because every scene predating the styles wants the default and stating
    /// it at each of them would say nothing.
    pub fn with_mask_blur_style(mut self, style: MaskBlurStyle) -> Self {
        self.mask_blur_style = style;
        self
    }

    pub fn with_mask_blur(mut self, sigma: f32) -> Self {
        self.mask_blur = sigma;
        self
    }

    /// Confine this item to `[left, top, right, bottom]` in its own space.
    /// Keep this item out of a rectangle. See [`Self::clip_out`].
    pub fn with_clip_out(mut self, rect: [f32; 4]) -> Self {
        self.clip_out = Some(rect);
        self
    }

    pub fn with_clip(mut self, clip: [f32; 4]) -> Self {
        self.clip = Some(clip);
        self
    }

    /// Confine this item to a shape in its own space, through the stencil.
    pub fn with_clip_shape(mut self, shape: Shape) -> Self {
        self.clip_shape = Some(shape);
        self
    }

    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// Whether the public API will draw this item from a distance field rather
    /// than from triangles.
    ///
    /// Mirrors the condition `Canvas::draw_rrect` applies. Stated here because
    /// the tolerance is derived from what a scene does, and what this one does
    /// depends on which path the call takes -- so a scene that says "rounded
    /// rectangle, filled, antialiased" is saying "coverage from a distance
    /// field", whether or not it knows the name for it.
    /// Whether any edge of this item can pass close to a pixel's center.
    ///
    /// Which is the whole question for an aliased draw. Without multisampling a
    /// pixel is covered or it is not, decided by whether its center falls
    /// inside, and both specifications pin that rule down exactly -- so two
    /// devices rasterizing the same triangle agree, and the corpus's aliased
    /// scenes compare bit-for-bit across devices because of it.
    ///
    /// What neither specification pins down is the triangle. Vertices are
    /// transformed by the vertex shader, and no specification requires two
    /// implementations to compute the same product of the same matrix and the
    /// same vector to the last bit. So an edge arrives a hair either side of
    /// where it arrived elsewhere, and where a pixel's center sits within that
    /// hair, one device covers it and the other does not -- one pixel, at full
    /// scale, because there is no partial coverage to soften it.
    ///
    /// An axis-aligned rectangle on integer coordinates cannot do this, and
    /// that is not luck: its edges land on pixel *boundaries*, and centers sit
    /// half a pixel away from the nearest one. A hair's difference moves
    /// nothing. Every other shape here has edges at arbitrary positions,
    /// including a rectangle that has been rotated, sheared, scaled
    /// fractionally, or given perspective.
    ///
    /// A stroke counts whatever it traces, because its outline is generated
    /// geometry offset by half a width rather than the shape's own edges.
    fn edges_can_tie(&self) -> bool {
        fn whole(v: f32) -> bool {
            v.fract() == 0.0
        }
        let t = &self.transform;
        let aligned = t.rotate == 0.0
            && t.skew == [0.0, 0.0]
            && t.perspective == [0.0, 0.0]
            && t.scale.iter().all(|v| whole(*v))
            && t.translate.iter().all(|v| whole(*v));
        if !aligned || self.stroke.is_some() {
            return true;
        }
        match &self.shape {
            Shape::Rect { min, max } => !min.iter().chain(max).all(|v| whole(*v)),
            _ => true,
        }
    }

    fn is_analytic(&self) -> bool {
        let shaped = match self.shape {
            crate::shape::Shape::RoundedRect { radius, .. } => radius > 0.0,
            crate::shape::Shape::Circle { radius, .. } => radius > 0.0,
            crate::shape::Shape::Oval { min, max } => max[0] > min[0] && max[1] > min[1],
            _ => false,
        };
        // A stroke of these shapes is evaluated the same way, so it earns the
        // same budget: the outline is the field narrowed to a band, not a
        // different kind of drawing.
        //
        // The blend is part of the condition because it is part of the one the
        // canvas applies: a mode that discards the destination where the source
        // is transparent cannot be drawn on a quad larger than its shape, and
        // falls back. Leaving it out here would give the distance field's
        // budget to a scene drawn from triangles.
        shaped && matches!(self.fill, Fill::Solid(_)) && self.blend.respects_coverage()
    }
}

/// How a group is composited back onto what is underneath it.
///
/// The two parts of a layer that mean anything: there is no shape to fill and
/// no geometry to stroke, so a paint would mostly be fields that do nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSpec {
    /// Filter the finished group as a whole.
    ///
    /// The fields below cover the four kinds a `Layer` can hold and apply them
    /// in one fixed order. This is what a group can be given that they cannot
    /// say: a caller's program, and a composition in whichever order it was
    /// written. See `Canvas::save_layer_filtered`.
    pub filter: ImageFilter,
    /// Filter what lies behind the group before drawing over it.
    ///
    /// [`Self::backdrop_blur`] is the same operation with the one filter that
    /// fits in a number, and is what nearly every plate wants. This is the
    /// rest -- a color filter, a morphology, a caller's program, or a
    /// composition -- and it is why this type is not `Copy`: a filter holds a
    /// program's uniforms, and no amount of arranging makes that fit in a
    /// register. It is a test-support type held in a `Box` already, so the cost
    /// is a `clone` at two call sites rather than anything a caller pays.
    pub backdrop: ImageFilter,
    /// Standard deviation of a blur over the finished group, in device pixels.
    /// Zero for none.
    pub blur: f32,
    /// Transform the finished group on the way back, resampling it.
    ///
    /// Distinct from the transform beside a layer node, which moves what goes
    /// into the group. Conflating the two would make a group redraw where it
    /// should resample.
    pub matrix: Option<Transform>,
    pub alpha: f32,
    pub blend: BlendMode,
    /// Standard deviation of a blur over what lies behind the group, in device
    /// pixels. Zero for none.
    pub backdrop_blur: f32,
    /// Spread or shrink the finished group. `None` for neither.
    pub morphology: Option<MorphologySpec>,
    /// Recolor the finished group on its way back.
    pub color_filter: ColorFilter,
    /// Share one captured backdrop with every layer naming the same key.
    ///
    /// `dart:ui`'s `backdropId`. See `Layer::backdrop_id` for what it means;
    /// what it means for a plate is that two overlapping panels each filter
    /// the ground rather than the second filtering the first.
    pub backdrop_id: Option<i64>,
}

/// A dilation or an erosion of a finished group.
///
/// Named here rather than reused from the renderer for the reason the whole
/// scene format is: a scene is data about a picture and must not need a device
/// to describe one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MorphologySpec {
    /// Reach along each axis, in device pixels.
    pub radius: [f32; 2],
    /// Take the largest sample in reach rather than the smallest.
    pub dilate: bool,
}

impl Default for LayerSpec {
    fn default() -> Self {
        Self {
            blur: 0.0,
            matrix: None,
            alpha: 1.0,
            blend: BlendMode::SrcOver,
            filter: ImageFilter::None,
            backdrop: ImageFilter::None,
            backdrop_blur: 0.0,
            morphology: None,
            color_filter: ColorFilter::None,
            backdrop_id: None,
        }
    }
}

impl LayerSpec {
    /// Spread the finished group by these radii, in device pixels.
    pub fn dilated(x: f32, y: f32) -> Self {
        Self {
            morphology: Some(MorphologySpec {
                radius: [x, y],
                dilate: true,
            }),
            ..Self::default()
        }
    }

    /// Shrink the finished group by these radii, in device pixels.
    pub fn eroded(x: f32, y: f32) -> Self {
        Self {
            morphology: Some(MorphologySpec {
                radius: [x, y],
                dilate: false,
            }),
            ..Self::default()
        }
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

    pub fn with_blur(mut self, blur: f32) -> Self {
        self.blur = blur;
        self
    }

    pub fn with_backdrop_blur(mut self, blur: f32) -> Self {
        self.backdrop_blur = blur;
        self
    }

    /// Share this layer's captured backdrop with others naming the same key.
    pub fn with_backdrop_id(mut self, id: i64) -> Self {
        self.backdrop_id = Some(id);
        self
    }
}

/// One entry in a scene: something to draw, or a group to draw and composite.
///
/// A scene is a tree rather than a list because a layer contains things. That
/// is the only reason — everything else about a scene stayed flat, and the flat
/// constructors below are unchanged, so a scene that has no layers reads
/// exactly as it did.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// Boxed because an item describes a whole paint -- a gradient's stops, a
    /// stroke, a color filter's matrix -- and a layer node beside it holds
    /// little more than a list. Without the indirection every node in a tree
    /// would be the size of the largest item in it.
    Draw(Box<Item>),
    /// Triangles a scene supplies directly.
    ///
    /// Boxed like a drawn item and for the same reason: it carries lists.
    Mesh(Box<MeshSpec>),
    /// Pieces of the fixture sheet, each placed and tinted on its own.
    Atlas(Box<AtlasSpec>),
    /// A run of glyphs read from the fixture glyph atlas as coverage.
    Glyphs(Box<GlyphRunSpec>),
    /// Points, segments, or an open run through a list of positions.
    Points(Box<PointsSpec>),
    /// The fixture sheet stretched by its middle, keeping its corners.
    NinePatch(Box<NinePatchSpec>),
    /// A color over everything the clip admits.
    Paint(Box<PaintSpec>),
    /// A recording of its own, drawn into this one.
    Picture(Box<PictureSpec>),
    /// The shadow a shape at some elevation casts.
    Shadow(Box<ShadowSpec>),
    /// A group rendered into a target of its own and composited back.
    ///
    /// `bounds` is the region the group promises to stay inside, in the space
    /// the layer opens in, and is what lets the target be smaller than the
    /// frame. `None` asks for a full-size target, which is what a caller who
    /// does not know gets.
    Layer {
        /// Boxed like every other variant here, and now for a reason the
        /// others did not have: a layer carries a whole paint's worth of
        /// filters, and adding a shear to a transform was enough to make this
        /// the largest variant and every node in a tree the size of it.
        layer: Box<LayerSpec>,
        bounds: Option<[f32; 4]>,
        /// Applied before the layer opens, so it moves the bounds along with
        /// the contents rather than only the contents.
        transform: Transform,
        children: Vec<Node>,
    },
    /// A clip in force over a run of nodes -- `save`, `clipRect`, `restore`.
    ///
    /// Distinct from the clip an item carries, which narrows one draw. A clip
    /// is state on the canvas and applies to everything drawn while it is in
    /// force, and the difference shows wherever a run of draws shares one:
    /// repeating an item's clip across them is the same picture only if
    /// nothing between them reads the clip, which a backdrop filter does.
    ///
    /// Nested rather than sequenced, which is the one place this departs from
    /// the calls it names. Upstream clips and then keeps drawing siblings; a
    /// scene is a tree, so what would be the following siblings are this
    /// node's children instead. The picture is the same and the shape of the
    /// recording is the same -- a `save`, the clip, the children, a `restore`.
    Clip {
        /// Keep only what this admits.
        rect: Option<[f32; 4]>,
        /// And remove this from it.
        rect_out: Option<[f32; 4]>,
        /// And keep only what this shape admits, for a clip that is not a
        /// rectangle.
        ///
        /// Separate from the rectangles rather than replacing them: a
        /// rectangular clip is the one an axis-aligned scissor can carry, and
        /// the renderer takes that route when it can. Stating a rectangle as a
        /// shape here would send it down the stencil instead and quietly
        /// measure something else.
        shape: Option<Shape>,
        /// In force over the clip and over what it scopes alike.
        ///
        /// Upstream's shape clips arrive under a `Save` and a transform, and
        /// the transform is what makes them interesting: a clip that did not
        /// turn with the caller would stay square. It applies before the clip
        /// is taken, so the shape is stated in the space the caller is drawing
        /// in rather than in the frame's.
        transform: Transform,
        children: Vec<Node>,
    },
}

impl From<Item> for Node {
    fn from(item: Item) -> Self {
        Self::Draw(Box::new(item))
    }
}

/// Whether a draw of one color puts that color into the target unchanged.
///
/// `SrcOver` is the interesting case and the reason this is a function rather
/// than a comparison. It is named as the mode that combines the fragment with
/// what was already there -- but only where there is something to combine, and
/// an opaque source leaves nothing: `src + dst * (1 - 1)` is `src`, exactly,
/// in floating point and in the fixed-point store alike. So an opaque solid
/// draw stores whatever it says its mode is.
///
/// The distinction matters only where coverage is binary, which is where
/// `Tolerance::EXACT` is reachable at all: a partially covered pixel carries a
/// source alpha below one whatever the color said, and does combine. Every
/// scene that can produce one is multisampled or analytic, and both of those
/// are answered before this is asked.
fn stores(blend: BlendMode, color: [f32; 4]) -> bool {
    blend != BlendMode::SrcOver || color[3] >= 1.0
}

impl Node {
    /// A group with a full-size target, which is what a caller states when it
    /// does not know what the group covers.
    pub fn layer(layer: LayerSpec, children: Vec<Node>) -> Self {
        Self::Layer {
            layer: Box::new(layer),
            bounds: None,
            transform: Transform::default(),
            children,
        }
    }

    /// A group that promises to stay inside `[left, top, right, bottom]`.
    pub fn bounded_layer(layer: LayerSpec, bounds: [f32; 4], children: Vec<Node>) -> Self {
        Self::Layer {
            layer: Box::new(layer),
            bounds: Some(bounds),
            transform: Transform::default(),
            children,
        }
    }

    pub fn with_transform(mut self, applied: Transform) -> Self {
        if let Self::Layer { transform, .. } = &mut self {
            *transform = applied;
        }
        self
    }

    /// Every item in this subtree, for the derivations that ask what a scene
    /// contains without caring how it is grouped.
    fn items(&self) -> Box<dyn Iterator<Item = &Item> + '_> {
        match self {
            Self::Draw(item) => Box::new(std::iter::once(item.as_ref())),
            // A mesh and a sprite batch are not items and have none. What the
            // derivations need from them is asked for separately, by
            // `samples_fixture` and `blends`, which are exhaustive over this
            // enum so that a node kind cannot be added without deciding.
            Self::Mesh(_)
            | Self::Atlas(_)
            | Self::Shadow(_)
            | Self::Glyphs(_)
            | Self::Points(_)
            | Self::NinePatch(_)
            | Self::Paint(_) => Box::new(std::iter::empty()),
            Self::Picture(picture) => Box::new(picture.children.iter().flat_map(Node::items)),
            Self::Layer { children, .. } | Self::Clip { children, .. } => {
                Box::new(children.iter().flat_map(Node::items))
            }
        }
    }

    fn items_mut(&mut self) -> Box<dyn Iterator<Item = &mut Item> + '_> {
        match self {
            Self::Draw(item) => Box::new(std::iter::once(item.as_mut())),
            Self::Mesh(_)
            | Self::Atlas(_)
            | Self::Shadow(_)
            | Self::Glyphs(_)
            | Self::Points(_)
            | Self::NinePatch(_)
            | Self::Paint(_) => Box::new(std::iter::empty()),
            Self::Picture(picture) => {
                Box::new(picture.children.iter_mut().flat_map(Node::items_mut))
            }
            Self::Layer { children, .. } | Self::Clip { children, .. } => {
                Box::new(children.iter_mut().flat_map(Node::items_mut))
            }
        }
    }

    /// Whether this subtree composites a group at all.
    /// Whether anything here adds to the destination rather than replacing or
    /// mixing toward it.
    ///
    /// An exhaustive match on purpose, and the third time this file has been
    /// caught by the same thing. A derivation that enumerates what it cares
    /// about misses whatever is added next: a new *fill* kind inherited the
    /// exact rule once, a new *node* kind did it again with glyph runs, and
    /// this was first written as a walk over `items()` -- which cannot see an
    /// atlas at all, because an atlas is a node and not an item. Written this
    /// way a new variant does not compile until somebody answers for it.
    fn blends_additively(&self) -> bool {
        match self {
            Self::Draw(item) => item.blend == BlendMode::Plus,
            Self::Mesh(mesh) => mesh.blend == BlendMode::Plus,
            Self::Atlas(atlas) => atlas.blend == BlendMode::Plus,
            Self::Paint(paint) => paint.blend == BlendMode::Plus,
            Self::Points(points) => points.blend == BlendMode::Plus,
            // These carry no blend of their own: a nine-patch and a shadow
            // draw with the paint they are given, a glyph run is coverage, and
            // a picture composites what it recorded.
            Self::NinePatch(_) | Self::Glyphs(_) | Self::Shadow(_) | Self::Picture(_) => false,
            Self::Layer { children, .. } | Self::Clip { children, .. } => {
                children.iter().any(Self::blends_additively)
            }
        }
    }

    /// How many targets deep the most deeply nested group in this node is.
    ///
    /// Zero for anything drawn straight into the frame. One for a group, and
    /// one more for each group inside it, because each is rendered into a
    /// target of its own and then composited out of it.
    ///
    /// Exhaustive on purpose. This file has now had three derivations go wrong
    /// by listing the cases that matter and missing one added later, and the
    /// comment on `tolerance` says the lesson is about enumerating rather than
    /// about any particular list.
    fn layer_depth(&self) -> u8 {
        match self {
            // A picture is composited from a target of its own whatever it
            // holds, which is the same extra store a layer is.
            Self::Picture(_) => 1,
            Self::Draw(_)
            | Self::Mesh(_)
            | Self::Atlas(_)
            | Self::Shadow(_)
            | Self::Glyphs(_)
            | Self::Points(_)
            | Self::NinePatch(_)
            | Self::Paint(_) => 0,
            Self::Layer { children, .. } => {
                1 + children.iter().map(Node::layer_depth).max().unwrap_or(0)
            }
            // A clip is state on the canvas, not a target: it narrows what the
            // group it scopes may draw into and opens nothing of its own.
            Self::Clip { children, .. } => {
                children.iter().map(Node::layer_depth).max().unwrap_or(0)
            }
        }
    }

    fn samples_fixture(&self) -> bool {
        // Exhaustive, and the direction it can be wrong in is why. A fill kind
        // that reads the sheet and is answered `false` here does not fail a
        // test -- the executor simply does not upload the fixture, and the
        // scene draws something else. That is the dangerous direction, and a
        // wildcard is what would take it: this is the same shape as the
        // derivations that missed a new fill kind, a mesh, an atlas and a glyph
        // run in turn, and the lesson those taught was to stop enumerating what
        // matters and make the compiler ask about everything.
        let reads_sheet = |fill: &Fill| match fill {
            Fill::Image { .. } => true,
            // A program naming slot zero reads the sheet as surely as an image
            // fill does, and the executor has to upload it either way.
            Fill::RuntimeEffect { images, .. } => images.contains(&0),
            // A color reads nothing. A gradient past `MAX_STOPS` does reach a
            // texture, but the recorder builds that ramp itself out of the
            // stops the fill carries -- it is not the fixture sheet and nothing
            // has to be uploaded for it.
            Fill::Solid(_)
            | Fill::LinearGradient { .. }
            | Fill::RadialGradient { .. }
            | Fill::SweepGradient { .. }
            | Fill::ConicalGradient { .. } => false,
        };
        match self {
            Self::Draw(item) => reads_sheet(&item.fill),
            Self::Mesh(mesh) => reads_sheet(&mesh.fill),
            // A flood fill carries a fill like any other draw, so a program
            // over the whole clip reads the sheet the same way one over a shape
            // does.
            Self::Paint(paint) => reads_sheet(&paint.fill),
            // A sprite batch is pieces of the sheet by definition.
            Self::Atlas(_) => true,
            // A run reads the glyph atlas, which is a texture of its own rather
            // than the sheet -- see `uses_glyphs`.
            // A nine-patch is pieces of the sheet, like a sprite batch.
            Self::NinePatch(_) => true,
            // Points are solid color; they sample nothing.
            Self::Shadow(_) | Self::Glyphs(_) | Self::Points(_) => false,
            Self::Picture(picture) => picture.children.iter().any(Node::samples_fixture),
            Self::Layer { children, .. } | Self::Clip { children, .. } => {
                children.iter().any(Node::samples_fixture)
            }
        }
    }

    /// Whether this subtree reads the fixture glyph atlas.
    ///
    /// A separate question from [`Self::samples_fixture`] because it is a
    /// separate texture: the sheet is color and the glyph atlas is coverage,
    /// and a scene may want either, both, or neither.
    /// Whether this node draws a shape a fragment evaluates rather than one
    /// the rasterizer covers.
    ///
    /// `Item::is_analytic` answers this for the shapes an item can hold, and
    /// cannot answer it here: a field of points is not an item and has no
    /// shape. That is the third time this derivation has been extended by a
    /// kind it could not see -- a new fill kind, then a new node kind, and now
    /// a new *material* kind -- and the lesson the second one recorded holds:
    /// the trouble is derivations that enumerate, not the lists they enumerate.
    /// This one is caught before the scene that needs it was added rather than
    /// after, which is the only reason it is not a fourth story about a scene
    /// that diverged by a unit on every edge.
    ///
    /// A round cap only. Its coverage comes from the same implicit disc an
    /// ellipse uses, evaluated from the vertices; a square cap is the quad
    /// itself, whose edges the rasterizer covers like any other triangle's.
    fn is_analytic(&self) -> bool {
        match self {
            Self::Points(spec) => {
                matches!(spec.mode, PointMode::Points)
                    && matches!(spec.stroke.cap, LineCap::Round)
                    && spec.blend.respects_coverage()
            }
            _ => false,
        }
    }

    /// Whether every fragment this node produces is a color written as it was
    /// given, rather than one arrived at by arithmetic.
    ///
    /// The distinction decides whether a scene is held to `Tolerance::EXACT`,
    /// so being wrong in the false direction costs a byte-for-byte comparison
    /// and being wrong in the true direction costs a failing test on a second
    /// device. Three derivations here have been wrong in the true direction, by
    /// enumerating the kinds that compute and missing one added later -- a
    /// glyph run, a mesh, an atlas -- so this enumerates *everything* and the
    /// compiler keeps the list complete.
    ///
    /// When a new kind arrives, the question to answer is whether two
    /// rasterizers could put its fragments a unit apart. If that cannot be
    /// settled, `false` is the answer to give: a scene compared a unit loosely
    /// still catches every failure worth catching, and one compared exactly
    /// that should not have been fails on a machine nobody has in front of
    /// them.
    fn stores_rather_than_computes(&self) -> bool {
        match self {
            // A solid color, written rather than blended, is the whole of the
            // case: any other fill computes.
            Self::Draw(item) => match item.fill {
                Fill::Solid(color) => stores(item.blend, color),
                _ => false,
            },
            // A flood fill is a draw without a shape and answers the same way.
            Self::Paint(paint) => match paint.fill {
                Fill::Solid(color) => stores(paint.blend, color),
                _ => false,
            },
            // Points are stroked geometry in one color, so they store like any
            // other draw of one.
            Self::Points(points) => stores(points.blend, points.color),
            // A mesh's colors are interpolated across a triangle by the
            // rasterizer, which is arithmetic neither backend's shader
            // performs; without colors it is a draw of whatever its fill says.
            Self::Mesh(mesh) => {
                mesh.colors.is_empty()
                    && match mesh.fill {
                        Fill::Solid(color) => stores(mesh.blend, color),
                        _ => false,
                    }
            }
            // These sample a texture, which is an interpolation whatever else
            // they do: sprites out of the sheet, coverage out of the glyph
            // atlas, nine quads of a stretched image.
            Self::Atlas(_) | Self::Glyphs(_) | Self::NinePatch(_) => false,
            // A shadow is a blur, and a layer is composited out of a target it
            // was rendered into.
            Self::Shadow(_) | Self::Layer { .. } => false,
            // Neither a clip nor a picture computes anything itself; both are
            // whatever they hold.
            Self::Clip { children, .. } => children.iter().all(Node::stores_rather_than_computes),
            Self::Picture(picture) => picture
                .children
                .iter()
                .all(Node::stores_rather_than_computes),
        }
    }

    fn uses_glyphs(&self) -> bool {
        // Exhaustive for the reason `samples_fixture` is: answering `false` for
        // a fill that does read the glyph atlas leaves it unbuilt rather than
        // failing anything.
        let reads_atlas = |fill: &Fill| match fill {
            Fill::RuntimeEffect { images, .. } => images.contains(&1),
            Fill::Solid(_)
            | Fill::Image { .. }
            | Fill::LinearGradient { .. }
            | Fill::RadialGradient { .. }
            | Fill::SweepGradient { .. }
            | Fill::ConicalGradient { .. } => false,
        };
        match self {
            Self::Glyphs(_) => true,
            Self::Draw(item) => reads_atlas(&item.fill),
            Self::Mesh(mesh) => reads_atlas(&mesh.fill),
            Self::Paint(paint) => reads_atlas(&paint.fill),
            // Points are stroked geometry in a solid color, reading no texture
            // of either kind.
            Self::Atlas(_) | Self::Shadow(_) | Self::Points(_) | Self::NinePatch(_) => false,
            Self::Picture(picture) => picture.children.iter().any(Node::uses_glyphs),
            Self::Layer { children, .. } | Self::Clip { children, .. } => {
                children.iter().any(Node::uses_glyphs)
            }
        }
    }

    /// Every blend mode this subtree uses, for the capability derivation.
    fn blends(&self) -> Box<dyn Iterator<Item = BlendMode> + '_> {
        match self {
            Self::Draw(item) => Box::new(std::iter::once(item.blend)),
            Self::Mesh(mesh) => Box::new(std::iter::once(mesh.blend)),
            Self::Atlas(atlas) => Box::new(std::iter::once(atlas.blend)),
            // A shadow blends against what is under it and nothing else.
            Self::Shadow(_) => Box::new(std::iter::once(BlendMode::SrcOver)),
            Self::Glyphs(run) => Box::new(std::iter::once(run.blend)),
            Self::Points(points) => Box::new(std::iter::once(points.blend)),
            Self::NinePatch(nine) => Box::new(std::iter::once(nine.blend)),
            Self::Paint(paint) => Box::new(std::iter::once(paint.blend)),
            Self::Picture(picture) => Box::new(
                std::iter::once(picture.blend)
                    .chain(picture.children.iter().flat_map(Node::blends)),
            ),
            Self::Layer {
                layer, children, ..
            } => {
                Box::new(std::iter::once(layer.blend).chain(children.iter().flat_map(Node::blends)))
            }
            Self::Clip { children, .. } => Box::new(children.iter().flat_map(Node::blends)),
        }
    }

    fn has_bounded_layer(&self) -> bool {
        match self {
            Self::Picture(picture) => picture.children.iter().any(Node::has_bounded_layer),
            Self::Draw(_)
            | Self::Mesh(_)
            | Self::Atlas(_)
            | Self::Shadow(_)
            | Self::Glyphs(_)
            | Self::Points(_)
            | Self::NinePatch(_)
            | Self::Paint(_) => false,
            Self::Layer {
                bounds, children, ..
            } => bounds.is_some() || children.iter().any(Node::has_bounded_layer),
            Self::Clip { children, .. } => children.iter().any(Node::has_bounded_layer),
        }
    }

    /// Whether any layer here filters what is behind it.
    ///
    /// Such a layer's bounds are not an optimization. They decide which part of
    /// the target is filtered, so stripping them changes the picture rather
    /// than only the allocation -- an unbounded backdrop blur covers the frame
    /// and blurs all of it. That is correct for both, and it is why the
    /// bounded-equals-unbounded comparison cannot be asked of these.
    fn filters_its_backdrop(&self) -> bool {
        match self {
            Self::Picture(picture) => picture.children.iter().any(Node::filters_its_backdrop),
            Self::Draw(_)
            | Self::Mesh(_)
            | Self::Atlas(_)
            | Self::Shadow(_)
            | Self::Glyphs(_)
            | Self::Points(_)
            | Self::NinePatch(_)
            | Self::Paint(_) => false,
            Self::Layer {
                layer, children, ..
            } => {
                // Both spellings. A sigma is the one filter a `Copy` layer can
                // hold and the general field takes the rest, and a color filter
                // over the backdrop decides which part of the target it covers
                // exactly as a blur does -- so a plate stating one that way was
                // being handed to the comparison this exists to keep it out of.
                layer.backdrop_blur > 0.0
                    || !layer.backdrop.is_identity()
                    || children.iter().any(Node::filters_its_backdrop)
            }
            Self::Clip { children, .. } => children.iter().any(Node::filters_its_backdrop),
        }
    }

    /// Whether this subtree carries anything whose only job is to change how
    /// what it draws looks.
    ///
    /// An image filter, a mask blur, and a mode combining a carried color with
    /// a paint's result are all of that kind: each is asked for by a scene, and
    /// each can be dropped on the floor without any other test noticing. A
    /// tolerance comparison between two backends is silent about a feature both
    /// of them ignore.
    fn carries_a_visual_feature(&self) -> bool {
        if self.asks_for_perspective() {
            return true;
        }
        // A color filter counts, and did not until a sweep of the advanced
        // blends found seventeen plates reporting a mode they never drew. That
        // failure was not about color filters, but it was about this: a scene
        // asking for something the picture never received, with nothing able to
        // say so. Every field here is a thing a plate can ask for and not get.
        let item_does = |item: &Item| {
            !matches!(item.image_filter, ImageFilter::None)
                || item.mask_blur > 0.0
                || item.color_filter != ColorFilter::None
        };
        match self {
            Self::Draw(item) => item_does(item),
            Self::Mesh(mesh) => {
                !matches!(mesh.image_filter, ImageFilter::None)
                    || mesh.tint_blend != BlendMode::Modulate
            }
            Self::Atlas(atlas) => atlas.tint_blend != BlendMode::Modulate,
            Self::Glyphs(run) => run.mask_blur > 0.0,
            Self::Shadow(_) | Self::Points(_) | Self::NinePatch(_) | Self::Paint(_) => false,
            Self::Picture(picture) => picture.children.iter().any(Node::carries_a_visual_feature),
            Self::Layer {
                layer, children, ..
            } => {
                layer.blur > 0.0
                    || layer.backdrop_blur > 0.0
                    || layer.morphology.is_some()
                    || layer.color_filter != ColorFilter::None
                    // The two general filters, which had been left out. A
                    // sigma and a program are the same kind of thing to a
                    // plate, and a plate whose only feature was the general
                    // spelling of one was skipped by the check that asks
                    // whether a feature reaches the picture.
                    || !layer.filter.is_identity()
                    || !layer.backdrop.is_identity()
                    || children.iter().any(Node::carries_a_visual_feature)
            }
            // A clip is geometry, not a feature: it says where a draw lands and
            // nothing about how it looks.
            Self::Clip { children, .. } => children.iter().any(Node::carries_a_visual_feature),
        }
    }

    /// Take those features away, leaving the geometry and the color.
    /// Every transform this node states, its children aside.
    ///
    /// Collected in one place so that a question asked about transforms is
    /// asked of all of them: there are nine, and a variant left out of the
    /// answer is a variant whose transform nothing checks.
    fn transforms_mut(&mut self) -> Vec<&mut Transform> {
        match self {
            Self::Draw(item) => vec![&mut item.transform],
            Self::Mesh(mesh) => vec![&mut mesh.transform],
            // A sprite batch places each sprite with an affine of its own,
            // which stays affine: `drawAtlas` takes an `RSTransform`, so even
            // that is past parity.
            Self::Atlas(_) => Vec::new(),
            Self::Glyphs(run) => vec![&mut run.transform],
            Self::Points(points) => vec![&mut points.transform],
            Self::NinePatch(nine) => vec![&mut nine.transform],
            Self::Paint(paint) => vec![&mut paint.transform],
            Self::Picture(picture) => vec![&mut picture.transform],
            Self::Shadow(shadow) => vec![&mut shadow.transform],
            Self::Layer {
                layer, transform, ..
            } => match layer.matrix.as_mut() {
                Some(matrix) => vec![transform, matrix],
                None => vec![transform],
            },
            // Its rectangles are stated in the space it is opened in, like a
            // layer's bounds, and its own transform is what opens that space.
            Self::Clip { transform, .. } => vec![transform],
        }
    }

    /// Whether anything here asks for perspective.
    ///
    /// Grouped with the visual features below rather than treated as part of
    /// the geometry, because it falls into the same trap and for the same
    /// reason: a scene whose perspective term is small enough to be invisible
    /// agrees with itself on both backends and proves nothing, exactly as a
    /// filter that was silently dropped would.
    fn asks_for_perspective(&self) -> bool {
        match self {
            Self::Draw(item) => item.transform.perspective != [0.0, 0.0],
            Self::Mesh(mesh) => mesh.transform.perspective != [0.0, 0.0],
            Self::Atlas(_) => false,
            Self::Glyphs(run) => run.transform.perspective != [0.0, 0.0],
            Self::Points(points) => points.transform.perspective != [0.0, 0.0],
            Self::NinePatch(nine) => nine.transform.perspective != [0.0, 0.0],
            Self::Paint(paint) => paint.transform.perspective != [0.0, 0.0],
            Self::Shadow(shadow) => shadow.transform.perspective != [0.0, 0.0],
            Self::Picture(picture) => {
                picture.transform.perspective != [0.0, 0.0]
                    || picture.children.iter().any(Node::asks_for_perspective)
            }
            Self::Layer {
                layer,
                transform,
                children,
                ..
            } => {
                transform.perspective != [0.0, 0.0]
                    || layer.matrix.is_some_and(|m| m.perspective != [0.0, 0.0])
                    || children.iter().any(Node::asks_for_perspective)
            }
            // Its own transform as well as its children's. A clip opens the
            // space its children are stated in, so a projection there is the
            // one the clip's own edges are drawn through -- and it was left out
            // here while `plain` stripped it, so a scene whose only perspective
            // sat on a clip reported no feature and was compared against a copy
            // of itself.
            Self::Clip {
                transform,
                children,
                ..
            } => {
                transform.perspective != [0.0, 0.0]
                    || children.iter().any(Node::asks_for_perspective)
            }
        }
    }

    fn flatten_perspective(&mut self) {
        for transform in self.transforms_mut() {
            transform.perspective = [0.0, 0.0];
        }
        // Exhaustive rather than a catch-all, so a node that gains children has
        // to say here whether they are reached. `plain` recurses through a clip
        // itself and re-runs this at each node, which hid the omission: a clip's
        // children kept their perspective under this function alone.
        match self {
            Self::Picture(picture) => picture
                .children
                .iter_mut()
                .for_each(Node::flatten_perspective),
            Self::Layer { children, .. } | Self::Clip { children, .. } => {
                children.iter_mut().for_each(Node::flatten_perspective)
            }
            Self::Draw(_)
            | Self::Mesh(_)
            | Self::Atlas(_)
            | Self::Shadow(_)
            | Self::Glyphs(_)
            | Self::Points(_)
            | Self::NinePatch(_)
            | Self::Paint(_) => {}
        }
    }

    fn plain(&mut self) {
        self.flatten_perspective();
        match self {
            Self::Draw(item) => {
                item.image_filter = ImageFilter::None;
                item.mask_blur = 0.0;
                item.color_filter = ColorFilter::None;
            }
            Self::Mesh(mesh) => {
                mesh.image_filter = ImageFilter::None;
                mesh.tint_blend = BlendMode::Modulate;
            }
            Self::Atlas(atlas) => atlas.tint_blend = BlendMode::Modulate,
            Self::Glyphs(run) => run.mask_blur = 0.0,
            Self::Shadow(_) | Self::Points(_) | Self::NinePatch(_) | Self::Paint(_) => {}
            Self::Picture(picture) => picture.children.iter_mut().for_each(Node::plain),
            Self::Layer {
                layer, children, ..
            } => {
                layer.blur = 0.0;
                layer.backdrop_blur = 0.0;
                layer.morphology = None;
                layer.color_filter = ColorFilter::None;
                layer.filter = ImageFilter::None;
                layer.backdrop = ImageFilter::None;
                children.iter_mut().for_each(Node::plain);
            }
            Self::Clip { children, .. } => children.iter_mut().for_each(Node::plain),
        }
    }

    fn unbound(&mut self) {
        if let Self::Layer {
            bounds, children, ..
        } = self
        {
            *bounds = None;
            children.iter_mut().for_each(Node::unbound);
        }
    }
}

/// A named scene: everything needed to render one comparable image.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub name: &'static str,
    pub size: Extent2D,
    pub background: [f32; 4],
    /// MSAA sample count. 1 renders aliased.
    pub samples: u32,
    pub items: Vec<Node>,
}

impl Scene {
    /// A scene that draws a flat list of items.
    ///
    /// Kept alongside [`Self::tree`] rather than replaced by it because most
    /// scenes have no groups, and making every one of them say so would be
    /// noise in the place a reader looks to see what a scene draws.
    pub fn new(name: &'static str, items: Vec<Item>) -> Self {
        Self::tree(
            name,
            items.into_iter().map(Box::new).map(Node::Draw).collect(),
        )
    }

    /// A scene whose entries may be groups.
    pub fn tree(name: &'static str, items: Vec<Node>) -> Self {
        Self {
            name,
            size: Extent2D::new(128, 128),
            background: [0.0, 0.0, 0.0, 1.0],
            samples: 1,
            items,
        }
    }

    /// Every item the scene draws, whatever it is grouped inside.
    ///
    /// Grouping is what a layer is for, and every derivation below asks what a
    /// scene contains rather than how it is arranged, so they all walk the tree
    /// through this rather than each learning its shape.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().flat_map(Node::items)
    }

    /// The same, for a caller that alters a scene to check a test can fail.
    pub fn items_mut(&mut self) -> impl Iterator<Item = &mut Item> {
        self.items.iter_mut().flat_map(Node::items_mut)
    }

    /// Whether anything in this scene adds to the destination.
    ///
    /// Exposed because a scene that blends additively has to be judged on a
    /// different axis: see [`crate::image::Tolerance::ACCUMULATED`].
    pub fn blends_additively(&self) -> bool {
        self.items.iter().any(Node::blends_additively)
    }

    /// Whether any aliased edge in this scene can fall on a rasterization tie.
    pub fn edges_can_tie(&self) -> bool {
        self.samples == 1 && self.items().any(Item::edges_can_tie)
    }

    /// Whether any group in this scene was told the region it covers.
    /// Whether any layer in this scene filters what is behind it.
    pub fn filters_its_backdrop(&self) -> bool {
        self.items.iter().any(Node::filters_its_backdrop)
    }

    pub fn has_bounded_layer(&self) -> bool {
        self.items.iter().any(Node::has_bounded_layer)
    }

    /// The same scene with every layer asking for a full-size target.
    ///
    /// Bounds are an optimization: the same drawing, into a target that happens
    /// to be smaller. So this is the scene that must render identically, and
    /// producing it by stripping the original rather than by writing it out
    /// twice is what keeps the two from drifting apart.
    pub fn unbounded(&self) -> Self {
        let mut stripped = self.clone();
        stripped.items.iter_mut().for_each(Node::unbound);
        stripped
    }

    /// Whether anything here asks for a feature that only changes appearance.
    pub fn carries_a_visual_feature(&self) -> bool {
        self.items.iter().any(Node::carries_a_visual_feature)
    }

    /// The same scene with those features taken away.
    ///
    /// The pair is what makes a feature checkable at all. Comparing backends
    /// says they agree; it cannot say they did anything, and a filter silently
    /// dropped would agree perfectly with itself.
    pub fn plain(&self) -> Self {
        let mut stripped = self.clone();
        stripped.items.iter_mut().for_each(Node::plain);
        stripped
    }

    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = samples;
        self
    }

    pub fn with_background(mut self, background: [f32; 4]) -> Self {
        self.background = background;
        self
    }

    /// How closely two implementations must agree on this scene.
    ///
    /// The rule is where the value came from, not what the picture looks like:
    /// **exact where a value is transported, tolerant where it is computed per
    /// fragment.** A solid fill copies a color through the pipeline, and any
    /// difference there is a defect. A gradient evaluates one, a blend converts
    /// an intermediate result to fixed point, and a multisample resolve
    /// averages — none of which the specification requires to be bit-identical
    /// across implementations, since shader arithmetic is permitted some error
    /// and compilers may fuse operations differently.
    ///
    /// Assigning this per scene by hand would drift as the corpus grows, and
    /// would let a genuine divergence be waved through by loosening one entry.
    /// Pixels an aliased scene may lose to a rasterization tie, as a fraction.
    ///
    /// A thousandth, which is sixteen pixels at the corpus's size, and is the
    /// multisample budget's number arrived at by the multisample budget's
    /// argument: an edge tie is a property of how much edge the geometry has,
    /// not of how many samples resolve it, so the count that bounds one bounds
    /// the other. What differs is the price of a tie, not how many there are --
    /// without multisampling there is no partial coverage, so a tie costs the
    /// whole pixel instead of a quarter of it.
    ///
    /// Measured against the corpus on a Raspberry Pi 5, comparing v3d against
    /// llvmpipe: one pixel for the perspective and transformed-gradient
    /// scenes, ten for each sweep -- two contiguous runs of five where the
    /// circle's edge lies nearest to forty-five degrees and steps diagonally
    /// through the grid. Ten of sixteen thousand, against a circumference of
    /// three hundred and fifty pixels.
    ///
    /// It stays able to tell a tie from a defect for the same reason: geometry
    /// in the wrong place moves a whole edge, and a color computed differently
    /// moves a whole shape, each hundreds of pixels rather than ten.
    const TIE_BUDGET: f32 = 0.001;

    /// How many more levels a filtered texture read is allowed, per channel.
    ///
    /// Two where this scene reads a texture whose size nothing here knows, and
    /// zero otherwise. The two is measured: one scene rendered twice on a
    /// Raspberry Pi 5, differing only in the sampler's filter, is byte for byte
    /// identical between v3d and llvmpipe through `Sampling::Nearest` and reaches
    /// a delta of three across seventy-two per cent of the frame through
    /// `Sampling::Linear`. Same geometry, same shader, same upload, same blend --
    /// so the filter is the mechanism.
    ///
    /// Its own term because a filter is not a store. The per-store rule counts
    /// arithmetic on a value that was written down; a filter forms a weighted sum
    /// of texels with weights of implementation-defined precision, so it
    /// disagrees by more than the last bit of one store and on every interpolated
    /// pixel rather than at an edge.
    ///
    /// **The rule is about what is knowable, not about which filter is bound,**
    /// and that distinction took three wrong answers to find. A material reports
    /// the filter and cannot report what it does: it has never seen the texture
    /// and so cannot tell whether a linear read lands between texels or on one.
    /// `Pass::sources` can, because it says what the texture *is*:
    ///
    /// - [`TextureSource::Image`] is the caller's, and its size is not in the
    ///   recording. A linear read of it may interpolate over every pixel or none,
    ///   and nothing here can rule either out, so the term is granted.
    /// - [`TextureSource::Layer`] is a pass's output, and `Pass::extent` gives
    ///   its size. A group composited back at the size it was rendered reads
    ///   texel centers and returns them, which is why the plain layer scenes hold
    ///   to the per-store bound on a board and why granting them this would be
    ///   widening a guard that measures something.
    /// - [`TextureSource::Ramp`] is a table this recorder baked, of a width it
    ///   chose.
    ///
    /// A blur and a morphology are granted it whatever they read, since they step
    /// out by a sigma or a radius that need not land on a center. That is what
    /// covers a dilated layer, whose disagreement is real and whose composite is
    /// not the cause.
    ///
    /// Recording a scene needs no device, so this costs a tessellation per call
    /// and reaches no GPU. A scene that cannot be recorded reports zero rather
    /// than panicking: whatever is wrong with it belongs in the message of the
    /// test that tries to render it.
    fn filter_term(&self) -> u8 {
        use emblema_core::canvas::TextureSource;

        let Ok(recording) = crate::record_scene(self) else {
            return 0;
        };
        let unknown_size = recording.passes.iter().any(|pass| {
            pass.batch.draws().iter().any(|draw| {
                if draw.material.reads_at_computed_offsets() {
                    return true;
                }
                // A filter that is bound and could interpolate, over a texture
                // whose size is the caller's business rather than this
                // recording's.
                let filtered = !matches!(
                    draw.material.sampling(),
                    None | Some(emblema_hal::material::Sampling::Nearest)
                );
                filtered
                    && draw
                        .material
                        .texture_slot()
                        .and_then(|slot| pass.sources.get(slot as usize))
                        .is_some_and(|source| matches!(source, TextureSource::Image(_)))
            })
        });
        if unknown_size {
            2
        } else {
            0
        }
    }

    pub fn tolerance(&self) -> crate::image::Tolerance {
        let filter = self.filter_term();
        // Any fill that is not a plain color is evaluated per fragment, so
        // this asks what the fill is not rather than listing the kinds that
        // are. Enumerating them meant a new gradient kind silently inherited
        // the exact rule and failed the moment it was added.
        // A layer is composited back with a blend and an alpha, which is the
        // same per-fragment arithmetic a translucent draw does, so a scene that
        // groups anything is computed whatever its items are.
        // A distance field first, because what it permits is smaller than the
        // multisample budget and needs no count: the derivative that sets the
        // edge width is implementation-defined, so coverage differs by a unit
        // or two along the whole edge rather than by a sample's worth at a few
        // pixels.
        //
        // Conditional on multisampling too, since that is what makes the
        // executor ask for antialiasing and the call take that path. Without
        // it this would loosen the aliased rounded-rectangle scene, which is
        // drawn from triangles and should still compare exactly.
        if self.samples > 1
            && (self.items().any(Item::is_analytic) || self.items.iter().any(Node::is_analytic))
        {
            return crate::image::Tolerance::ANALYTIC.widened_by(filter);
        }
        // An additive blend before the rest, because it is the one case where a
        // fragment's rounding does not replace the last one but is added to it.
        // Overlapping draws then accumulate, so the bound is per draw that can
        // land on a pixel rather than per pixel. See `Tolerance::ACCUMULATED`.
        if self.items.iter().any(Node::blends_additively) {
            return self
                .allowing_ties(crate::image::Tolerance::ACCUMULATED)
                .widened_by(filter);
        }
        // Multisampling first, because it permits something the others do not:
        // a whole sample's worth of difference at an edge, on a few pixels. The
        // rest permit a unit everywhere and nothing more.
        if self.samples > 1 {
            return crate::image::Tolerance::MULTISAMPLED.widened_by(filter);
        }
        // A glyph run is computed whatever else the scene holds: its coverage
        // comes from a texture and is multiplied by the paint's color per
        // fragment, which is the same arithmetic a translucent draw does.
        //
        // Asked of the nodes rather than of the items, and that is the whole
        // point. A run is not an item, so a walk over items cannot see it --
        // the scene came out `EXACT` and diverged by one unit on every
        // partially covered pixel the moment it was added. That was the second
        // time a derivation here enumerated what it cared about and missed what
        // came later, the first being a new *fill* kind.
        //
        // It sprang twice more the same afternoon -- a mesh, whose colors the
        // rasterizer interpolates, and an atlas, which samples the sheet -- and
        // at four instances the lesson stopped being about which kinds to add.
        // The question is asked the other way round now: `stores_rather_than_computes`
        // is an exhaustive match over every node kind, so a kind added to the
        // scene format cannot be missed here. It has to say which it is, and
        // the direction to be wrong in is written down beside it.
        let computed = !self.items.iter().all(Node::stores_rather_than_computes);
        let depth = self.items.iter().map(Node::layer_depth).max().unwrap_or(0);
        // Both conditions, and the second is not redundant. A scene can be made
        // entirely of draws that store and still be composited out of a target,
        // and the *composite* is arithmetic whatever it is compositing: a
        // sample, an alpha, and a blend. `picture-drawn-into-a-picture` is the
        // scene that says so -- every draw in it writes a solid color and the
        // two backends still land two levels apart on a tenth of the frame.
        if !computed && depth == 0 {
            return self
                .allowing_ties(crate::image::Tolerance::EXACT)
                .widened_by(filter);
        }
        // One unit per fixed-point store the fragment passes through, which is
        // what the per-channel bound has always meant -- `ROUNDING` is this
        // rule at a depth of zero, and every scene without a group still gets
        // exactly it.
        //
        // A group is rendered into a target of its own and then composited out
        // of it, so its fragments are quantized twice rather than once, and two
        // devices whose arithmetic differs in the last bits can land two levels
        // apart rather than one. Measured on a Raspberry Pi 5, where v3d and
        // llvmpipe put `layer-blended-composite` two levels apart on seventeen
        // interior pixels -- interior, not edges, so it is the arithmetic and
        // not the rasterizer.
        //
        // Derived from depth rather than from whether a group is present at
        // all, because the mechanism is per store and says so. The nested
        // scene in the corpus is allowed three by this and uses two, which is
        // slack that is stated rather than discovered: if a scene ever needs
        // the third, the rule already predicted it.
        self.allowing_ties(crate::image::Tolerance::new(1 + depth, 0.0))
            .widened_by(filter)
    }

    /// Add the tie budget to a profile, where this scene's edges can tie.
    ///
    /// Applied to every aliased profile rather than to the ones that were seen
    /// to need it. The mechanism is the vertex transform, which does not know
    /// what the fragment shader will go on to compute, so a scene that fills a
    /// circle with a flat color can tie exactly as readily as one that fills it
    /// with a gradient -- it is only that on this device pair the flat ones did
    /// not. Granting it where it was measured and withholding it elsewhere
    /// would be fitting the budget to a device.
    ///
    /// That is not free, and the cost is worth naming rather than leaving to be
    /// found. Fourteen scenes that compare bit-for-bit across devices today are
    /// aliased, curved or transformed, and hold to [`Tolerance::EXACT`] only
    /// because no edge of theirs has yet landed on a tie. They keep the exact
    /// per-channel bound -- what they gain is room for sixteen pixels to fall
    /// the other way, which is the room the mechanism says they need.
    ///
    /// `max` rather than assignment so a profile that already carries a wider
    /// count keeps it.
    fn allowing_ties(&self, tolerance: crate::image::Tolerance) -> crate::image::Tolerance {
        match self.edges_can_tie() {
            true => crate::image::Tolerance::new(
                tolerance.per_channel,
                tolerance.outlier_fraction.max(Self::TIE_BUDGET),
            ),
            false => tolerance,
        }
    }

    /// Whether any item in this scene samples the fixture sheet.
    ///
    /// Derived rather than declared, for the same reason the tolerance and the
    /// capability requirement are: a flag written alongside the scene is one
    /// that can be forgotten, and forgetting this one means a draw refused for
    /// naming a texture nobody supplied.
    pub fn samples_fixture(&self) -> bool {
        self.items.iter().any(Node::samples_fixture)
    }

    /// Whether any node in this scene reads the fixture glyph atlas.
    pub fn uses_glyphs(&self) -> bool {
        self.items.iter().any(Node::uses_glyphs)
    }

    /// Whether a device can render this scene at all.
    ///
    /// Derived from what the scene contains, for the same reason the tolerance
    /// is: a requirement written alongside the scene is one that can be
    /// forgotten, and a scene needing a capability nobody declared would be
    /// reported as a backend regression rather than as the known gap it is.
    ///
    /// The distinction this draws matters to the cross-backend comparison. A
    /// scene refused by a device that the scene says needs nothing special is a
    /// defect; a scene refused by a device the scene says cannot render it is a
    /// gap, and the corpus reports the second as coverage it did not get rather
    /// than as a pass.
    pub fn supported_by(&self, capabilities: &emblema_hal::Capabilities) -> bool {
        if !capabilities.sample_counts.supports(self.samples) {
            return false;
        }
        if !capabilities.advanced_blend
            && self
                .items
                .iter()
                .flat_map(Node::blends)
                .any(|b| b.is_advanced())
        {
            return false;
        }
        true
    }
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
const GREEN: [f32; 4] = [0.2, 1.0, 0.2, 1.0];
const BLUE: [f32; 4] = [0.2, 0.2, 1.0, 1.0];

/// A backdrop and three overlapping translucent circles, one per mode.
///
/// The backdrop is a gradient rather than a flat color because several of these
/// modes are functions of the backdrop's value — dodge, burn and the two
/// contrast modes all behave differently at each end — and a flat backdrop
/// would exercise one point of each curve. The circles overlap each other so
/// the second and third blend against a result the first produced, which is
/// what a coherent blend has to get right and an incoherent one does not.
fn advanced_blend_items(modes: &[BlendMode; 3]) -> Vec<Item> {
    let mut items = vec![Item::filled(
        Shape::Rect {
            min: [0.0, 0.0],
            max: [128.0, 128.0],
        },
        Fill::LinearGradient {
            start: [0.0, 0.0],
            end: [128.0, 128.0],
            stops: vec![
                Stop::new([0.05, 0.1, 0.35, 1.0], 0.0),
                Stop::new([0.6, 0.55, 0.2, 1.0], 0.5),
                Stop::new([0.95, 0.9, 0.85, 1.0], 1.0),
            ],
            tile: TileMode::Clamp,
        },
    )];
    let placements = [
        ([48.0, 44.0], [0.9, 0.35, 0.2, 0.8]),
        ([80.0, 56.0], [0.25, 0.7, 0.85, 0.8]),
        ([64.0, 88.0], [0.6, 0.85, 0.3, 0.8]),
    ];
    for (mode, (center, color)) in modes.iter().zip(placements) {
        items.push(
            Item::fill(
                Shape::Circle {
                    center,
                    radius: 34.0,
                },
                color,
            )
            .with_blend(*mode),
        );
    }
    items
}

/// Three sharp elbows side by side, for a scene that varies the join.
///
/// Open, so the ends carry caps and the corner carries a join, and sharp,
/// because the three joins differ by how they fill the outside of a corner and
/// a shallow one leaves almost nothing to differ over.
fn elbows() -> Vec<Vec<[f32; 2]>> {
    (0..3)
        .map(|i| {
            let x = 22.0 + i as f32 * 42.0;
            vec![[x - 14.0, 96.0], [x, 40.0], [x + 14.0, 96.0]]
        })
        .collect()
}

/// A five-pointed star as one closed path, which crosses itself five times.
fn pentagram() -> Vec<[f32; 2]> {
    (0..5)
        .map(|k| {
            let angle = (-90.0 + k as f32 * 144.0).to_radians();
            [64.0 + 52.0 * angle.cos(), 64.0 + 52.0 * angle.sin()]
        })
        .collect()
}

/// The ground most of the corpus clears to.
///
/// Stated in eighths of a byte rather than as round decimals, and the reason
/// is a tie. A tenth of 255 is 25.5 exactly, and two rasterizers broke that tie
/// two different ways -- this machine's Vulkan and its GLES landing on 25 and
/// the software one on 26 -- so every scene using it differed by a unit on
/// every pixel of ground it left uncovered. Inside the budget, and spending the
/// budget on the clear color rather than on the drawing it is there to check.
///
/// Any value off a half is fine; this is the nearest one to what was meant.
const DARK_GROUND: [f32; 4] = [15.0 / 255.0, 18.0 / 255.0, 26.0 / 255.0, 1.0];

/// The scene corpus.
///
/// Deliberately small and varied rather than large: each scene is here because
/// it exercises something the others do not, so a failure names a capability
/// rather than merely a picture. Regression pins are appended as bugs are
/// fixed, and that set only grows.
///
/// Audited on 2026-09-02 by asking which node kinds and fills it actually held,
/// which turned out to be seven of them. Meshes, atlases, shadows, pictures,
/// flood fills, image fills and morphologies were all absent -- each a
/// capability compared only in the catalog, whose budget is three per cent of a
/// frame against this collection's unit or two per pixel. All seven are here
/// now, chosen for arithmetic two devices can disagree about rather than to
/// complete a list.
///
/// Two are still absent on purpose. A layer's *general* filter is a composition
/// of things already here -- its blur, its morphology and its color filter each
/// have a scene -- so a scene for the general spelling would exercise the
/// composing and not the filter. And a caller's program has
/// `tests/runtime_effect.rs`, which builds a pipeline from a module on each
/// backend and compares the pictures: a stronger check than a scene here, since
/// it is the pipeline construction that differs between them rather than the
/// arithmetic.
pub fn corpus() -> Vec<Scene> {
    vec![
        Scene::new(
            "rect-fill",
            vec![Item::fill(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                WHITE,
            )],
        ),
        Scene::new(
            "circle-fill",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 48.0,
                },
                WHITE,
            )],
        ),
        // Concave, so it must not take the convex fan path.
        Scene::new(
            "concave-polygon",
            vec![Item::fill(
                Shape::Polygon(vec![
                    [16.0, 16.0],
                    [112.0, 16.0],
                    [112.0, 64.0],
                    [64.0, 64.0],
                    [64.0, 112.0],
                    [16.0, 112.0],
                ]),
                WHITE,
            )],
        ),
        Scene::new(
            "overlapping-opaque",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [16.0, 16.0],
                        max: [80.0, 80.0],
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Rect {
                        min: [48.0, 48.0],
                        max: [112.0, 112.0],
                    },
                    BLUE,
                ),
            ],
        ),
        Scene::new(
            "translucent-stack",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [8.0, 8.0],
                        max: [120.0, 120.0],
                    },
                    BLUE,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [56.0, 56.0],
                        radius: 40.0,
                    },
                    [1.0, 0.0, 0.0, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::Circle {
                        center: [80.0, 80.0],
                        radius: 40.0,
                    },
                    [0.0, 1.0, 0.0, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
            ],
        ),
        // The separable blend modes, three per scene so the mixing is visible
        // where they overlap each other as well as the backdrop. These need an
        // advanced-blend extension, so a device without one reports them as a
        // declared gap through `supported_by` rather than failing to render.
        Scene::new(
            "advanced-blend-darkening",
            advanced_blend_items(&[BlendMode::Multiply, BlendMode::ColorBurn, BlendMode::Darken]),
        ),
        Scene::new(
            "advanced-blend-lightening",
            advanced_blend_items(&[BlendMode::Screen, BlendMode::ColorDodge, BlendMode::Lighten]),
        ),
        Scene::new(
            "advanced-blend-contrast",
            advanced_blend_items(&[
                BlendMode::Overlay,
                BlendMode::HardLight,
                BlendMode::SoftLight,
            ]),
        ),
        Scene::new(
            "advanced-blend-non-separable",
            advanced_blend_items(&[BlendMode::Hue, BlendMode::Saturation, BlendMode::Color]),
        ),
        Scene::new(
            "advanced-blend-luminosity",
            advanced_blend_items(&[BlendMode::Luminosity, BlendMode::Hue, BlendMode::Luminosity]),
        ),
        Scene::new(
            "advanced-blend-inverting",
            advanced_blend_items(&[
                BlendMode::Difference,
                BlendMode::Exclusion,
                BlendMode::Multiply,
            ]),
        ),
        // Clipping. A scissor is exact, so these compare bit-for-bit between
        // backends and devices -- which is what makes them worth having:
        // an off-by-one or a mirrored axis shows up as a hard failure rather
        // than as something within tolerance.
        Scene::new(
            "clipped-circle",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 56.0,
                },
                RED,
            )
            // Deliberately off-center, and clipping the circle on three
            // sides but not the fourth, so a mirrored or transposed clip
            // produces a different picture rather than the same one.
            .with_clip([20.0, 8.0, 100.0, 72.0])],
        ),
        Scene::new(
            "clip-varies-between-draws",
            vec![
                // Overlapping bands, each clipped differently, with an
                // unclipped shape between them. Clip state persists until it is
                // set again, so this catches a clip leaking into a later draw
                // as well as one never being applied.
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    BLUE,
                )
                .with_clip([0.0, 0.0, 40.0, 128.0]),
                Item::fill(
                    Shape::Rect {
                        min: [48.0, 48.0],
                        max: [80.0, 80.0],
                    },
                    WHITE,
                ),
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    GREEN,
                )
                .with_clip([96.0, 24.0, 128.0, 104.0]),
            ],
        ),
        Scene::new(
            "clip-follows-its-transform",
            vec![
                // The clip is stated in the item's own space, so it travels
                // through the rotation and scale with the shape. A clip applied
                // in device pixels instead would sit at the target's origin.
                Item::fill(
                    Shape::Rect {
                        min: [-40.0, -40.0],
                        max: [40.0, 40.0],
                    },
                    RED,
                )
                .with_transform(Transform {
                    scale: [1.0, 1.0],
                    rotate: std::f32::consts::FRAC_PI_2,
                    skew: [0.0, 0.0],
                    translate: [72.0, 56.0],
                    perspective: [0.0, 0.0],
                })
                .with_clip([-40.0, -40.0, 10.0, 24.0]),
            ],
        ),
        // Clipping by a shape a rectangle cannot express, which goes through
        // the stencil rather than the scissor. Like the scissor scenes these
        // compare exactly: a pixel is either admitted or it is not, with no
        // per-fragment arithmetic to permit a difference.
        Scene::new(
            "shape-clipped-fill",
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                RED,
            )
            // A triangle whose bounding box reaches three corners the triangle
            // itself misses, so clipping to its bounds would be a visibly
            // different picture.
            .with_clip_shape(Shape::Polygon(vec![
                [64.0, 12.0],
                [116.0, 104.0],
                [20.0, 92.0],
            ]))],
        ),
        Scene::new(
            "shape-clip-and-scissor-together",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 60.0,
                },
                GREEN,
            )
            // Both mechanisms at once on one item, which is the arrangement
            // that would break if either were applied in place of the other
            // rather than alongside it.
            .with_clip_shape(Shape::Polygon(vec![
                [10.0, 118.0],
                [64.0, 8.0],
                [118.0, 118.0],
            ]))
            .with_clip([0.0, 0.0, 78.0, 128.0])],
        ),
        Scene::new(
            "shape-clip-follows-its-transform",
            vec![Item::fill(
                Shape::Rect {
                    min: [-56.0, -56.0],
                    max: [56.0, 56.0],
                },
                BLUE,
            )
            .with_transform(Transform {
                scale: [1.0, 1.0],
                // An eighth turn, which no scissor expresses: the clip becomes
                // a diamond and its bounding box is visibly larger.
                rotate: std::f32::consts::FRAC_PI_4,
                skew: [0.0, 0.0],
                translate: [64.0, 64.0],
                perspective: [0.0, 0.0],
            })
            .with_clip_shape(Shape::Rect {
                min: [-38.0, -38.0],
                max: [38.0, 38.0],
            })],
        ),
        Scene::new(
            "stroke-polygon-and-curve",
            vec![
                Item::stroke(
                    Shape::Polygon(vec![[24.0, 32.0], [64.0, 96.0], [104.0, 32.0]]),
                    StrokeSpec {
                        width: 10.0,
                        cap: LineCap::Round,
                        join: LineJoin::Round,
                        miter_limit: 4.0,
                        dash: None,
                    },
                    GREEN,
                ),
                Item::stroke(
                    Shape::Cubic {
                        start: [16.0, 112.0],
                        c0: [48.0, 64.0],
                        c1: [80.0, 160.0],
                        end: [112.0, 112.0],
                    },
                    StrokeSpec::new(6.0),
                    WHITE,
                ),
            ],
        ),
        Scene::new(
            // Perspective, which the transform this schema lowers to could not
            // state until it stopped being affine. Two shapes rather than one:
            // a gradient, because a paint's mapping is projective too and a
            // gradient that stayed straight while its shape converged would be
            // the visible sign that only half the change landed; and a stroked
            // curve, because flattening happens before the transform and the
            // magnified end is where too coarse a tolerance shows as facets.
            //
            // Kept mild on purpose. The divisor runs between one and about one
            // and a half here, nowhere near the vanishing line -- close to it
            // every rasterization tiebreak is amplified by the square of the
            // divisor, and a plate placed there would disagree between devices
            // for reasons that have nothing to do with what it is testing.
            "perspective",
            vec![
                Item::gradient(
                    Shape::Rect {
                        min: [8.0, 8.0],
                        max: [120.0, 120.0],
                    },
                    [8.0, 0.0],
                    [120.0, 0.0],
                    vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                )
                .with_transform(Transform {
                    perspective: [0.004, 0.0],
                    ..Transform::default()
                }),
                Item::stroke(
                    Shape::Cubic {
                        start: [16.0, 112.0],
                        c0: [36.0, 24.0],
                        c1: [92.0, 24.0],
                        end: [112.0, 112.0],
                    },
                    StrokeSpec::new(5.0),
                    WHITE,
                )
                .with_transform(Transform {
                    perspective: [0.004, 0.0],
                    ..Transform::default()
                }),
            ],
        ),
        Scene::new(
            "transformed",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [32.0, 32.0],
                    },
                    RED,
                )
                .with_transform(Transform::translate(16.0, 16.0)),
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [16.0, 16.0],
                    },
                    GREEN,
                )
                .with_transform(Transform {
                    scale: [3.0, 1.5],
                    rotate: 0.4,
                    skew: [0.0, 0.0],
                    translate: [64.0, 64.0],
                    perspective: [0.0, 0.0],
                }),
            ],
        ),
        // The same content as circle-fill, multisampled: the pair is what makes
        // an antialiasing regression visible as a diff rather than a judgment.
        Scene::new(
            "circle-antialiased",
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 48.0,
                },
                WHITE,
            )],
        )
        .with_samples(4),
        // Gradients are where the two backends most easily diverge: one sends
        // the paint as push constants, the other as individually-set uniforms,
        // and the fragment locates itself from an interpolated clip position
        // whose orientation the two APIs disagree about. Comparing them is the
        // point of having these in the corpus rather than only in a suite
        // someone remembers to run twice.
        Scene::new(
            "gradient-horizontal",
            vec![Item::gradient(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                [8.0, 0.0],
                [120.0, 0.0],
                vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
            )],
        ),
        // Vertical as well as horizontal: an axis mix-up leaves one of the two
        // looking perfectly correct.
        Scene::new(
            "gradient-vertical",
            vec![Item::gradient(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 52.0,
                },
                [0.0, 12.0],
                [0.0, 116.0],
                vec![
                    Stop::new(RED, 0.0),
                    Stop::new(GREEN, 0.5),
                    Stop::new(BLUE, 1.0),
                ],
            )],
        ),
        // Under a transform, so the endpoints are exercised through the same
        // mapping the geometry takes rather than only through the identity.
        Scene::new(
            "gradient-transformed",
            vec![Item::gradient(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [64.0, 64.0],
                },
                [0.0, 0.0],
                [64.0, 0.0],
                vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
            )
            .with_transform(Transform {
                scale: [1.5, 1.5],
                rotate: 0.6,
                skew: [0.0, 0.0],
                translate: [40.0, 16.0],
                perspective: [0.0, 0.0],
            })],
        ),
        Scene::new(
            "gradient-radial",
            vec![Item::filled(
                Shape::Rect {
                    min: [4.0, 4.0],
                    max: [124.0, 124.0],
                },
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 56.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        // Offset the first circle from the second so the picture is one no
        // radial gradient could produce: the rings bunch on the side the first
        // circle sits toward. Keeping it inside the second circle means every
        // point lies on some circle of the family, so the tile is fully
        // covered and comparable to the radial one beside it -- what happens
        // where no circle reaches is a unit test rather than a picture.
        Scene::new(
            "gradient-conical",
            vec![Item::filled(
                Shape::Rect {
                    min: [4.0, 4.0],
                    max: [124.0, 124.0],
                },
                Fill::ConicalGradient {
                    start_center: [44.0, 44.0],
                    start_radius: 0.0,
                    end_center: [64.0, 64.0],
                    end_radius: 60.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        // A luminance matrix over a gradient: the filter runs per fragment on
        // a color that varies, which is what makes it a filter rather than a
        // recoloring of the stops. Asymmetric on purpose -- every row is the
        // same weights, so a transposed matrix would leave each end its own
        // hue instead of turning both gray.
        Scene::new(
            "color-filter-luminance",
            vec![Item::filled(
                Shape::Rect {
                    min: [4.0, 4.0],
                    max: [124.0, 124.0],
                },
                Fill::LinearGradient {
                    start: [4.0, 4.0],
                    end: [124.0, 124.0],
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )
            .with_color_filter(ColorFilter::matrix([
                0.2126, 0.7152, 0.0722, 0.0, 0.0, //
                0.2126, 0.7152, 0.0722, 0.0, 0.0, //
                0.2126, 0.7152, 0.0722, 0.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]))],
        ),
        Scene::new(
            "gradient-sweep",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 56.0,
                },
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::TAU,
                    stops: vec![
                        Stop::new(RED, 0.0),
                        Stop::new(GREEN, 0.5),
                        Stop::new(BLUE, 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        // A gradient shorter than the shape it fills, once per tile mode. The
        // ramp spans a quarter of the width, so what happens outside it is most
        // of the picture rather than a strip at the edge: clamping shows two
        // flat bands, repeating shows four ramps, and decal leaves the rest
        // empty over whatever the scene put behind it.
        // Six stops, which is more than a material carries, so this is the
        // path that tabulates them into a texture. Here rather than only in
        // the public API tests because the corpus is what compares the two
        // backends against each other: a ramp bound correctly on one and not
        // the other is exactly the divergence nothing else would notice.
        Scene::new(
            "gradient-many-stops",
            vec![Item::filled(
                Shape::Rect {
                    min: [4.0, 4.0],
                    max: [124.0, 124.0],
                },
                Fill::LinearGradient {
                    start: [4.0, 0.0],
                    end: [124.0, 0.0],
                    stops: vec![
                        Stop::new(RED, 0.0),
                        Stop::new([1.0, 1.0, 0.0, 1.0], 0.2),
                        Stop::new(GREEN, 0.4),
                        Stop::new([0.0, 1.0, 1.0, 1.0], 0.6),
                        Stop::new(BLUE, 0.8),
                        Stop::new([1.0, 0.0, 1.0, 1.0], 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        // The two together: more stops than a material carries, tiled. Each
        // half of this is covered alone, which is the arrangement in which a
        // composition bug lives -- and the ramp is sampled through a
        // clamp-to-edge sampler, so a repeating one is where a wrong address
        // mode would show as a soft band at every period.
        Scene::new(
            "gradient-many-stops-repeated",
            vec![Item::filled(
                Shape::Rect {
                    min: [4.0, 4.0],
                    max: [124.0, 124.0],
                },
                Fill::LinearGradient {
                    start: [4.0, 0.0],
                    end: [44.0, 0.0],
                    stops: vec![
                        Stop::new(RED, 0.0),
                        Stop::new([1.0, 1.0, 0.0, 1.0], 0.2),
                        Stop::new(GREEN, 0.4),
                        Stop::new([0.0, 1.0, 1.0, 1.0], 0.6),
                        Stop::new(BLUE, 0.8),
                        Stop::new([1.0, 0.0, 1.0, 1.0], 1.0),
                    ],
                    tile: TileMode::Repeat,
                },
            )],
        ),
        Scene::new(
            "gradient-tiled-clamp",
            vec![tiled_gradient_item(TileMode::Clamp)],
        ),
        Scene::new(
            "gradient-tiled-repeat",
            vec![tiled_gradient_item(TileMode::Repeat)],
        ),
        Scene::new(
            "gradient-tiled-mirror",
            vec![tiled_gradient_item(TileMode::Mirror)],
        ),
        Scene::new(
            "gradient-tiled-decal",
            vec![
                Item::filled(
                    Shape::Rect {
                        min: [4.0, 4.0],
                        max: [124.0, 124.0],
                    },
                    Fill::Solid(WHITE),
                ),
                tiled_gradient_item(TileMode::Decal),
            ],
        ),
        // A partial sweep, which is the only case where a sweep has an outside
        // at all: a full turn covers every direction and tiles to itself.
        Scene::new(
            "gradient-sweep-partial",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 56.0,
                },
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        Scene::new(
            "curve-antialiased",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [8.0, 96.0],
                    c0: [48.0, 8.0],
                    c1: [80.0, 152.0],
                    end: [120.0, 40.0],
                },
                StrokeSpec {
                    width: 8.0,
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    miter_limit: 4.0,
                    dash: None,
                },
                WHITE,
            )],
        )
        .with_samples(4),
        // A path that crosses itself, filled by each rule. The two differ only
        // in the middle: a pentagram wound once has a center the crossings
        // enclose twice, so non-zero fills it and even-odd leaves it hollow.
        //
        // One subpath rather than the two a corpus would otherwise reach for,
        // because that is the case that was wrong. A path of several subpaths
        // goes through the sweep, which has always honored the rule; a single
        // subpath is offered to a convexity test first, and a self-crossing
        // one whose turns all agree was called convex and fan-filled -- by a
        // routine with no notion of a fill rule, which therefore discarded it.
        Scene::new(
            "self-crossing-nonzero",
            vec![Item::filled(
                Shape::RuledPolygon {
                    points: pentagram(),
                    rule: FillRule::NonZero,
                },
                Fill::Solid(WHITE),
            )],
        ),
        Scene::new(
            "self-crossing-evenodd",
            vec![Item::filled(
                Shape::RuledPolygon {
                    points: pentagram(),
                    rule: FillRule::EvenOdd,
                },
                Fill::Solid(WHITE),
            )],
        ),
        // The stroke joins, one elbow each. A join is what fills the outside
        // of a corner, and the three fill it differently: a miter runs out to
        // the point where the two edges would meet, a bevel cuts straight
        // across, and a round arcs between. Nothing else in the corpus varies
        // this -- the scene that used to be named for it states one value.
        // Dashes, on a straight run and around a curve. The curve is the half
        // worth comparing between backends: a dash is measured along the
        // flattened path, so a backend flattening differently would place the
        // dashes differently, and nothing else in the corpus would notice.
        // The two things an arc is for. Both here because they exercise
        // different halves: the ring is a stroke along a curve with two caps,
        // the slice is a filled region whose straight edges meet at a point,
        // and an arc that strayed from its radius would show as a wobble in one
        // and a dent in the other.
        Scene::new(
            "arc-ring-and-slice",
            vec![
                Item::stroke(
                    Shape::Arc {
                        center: [64.0, 40.0],
                        radii: [30.0, 30.0],
                        start: -std::f32::consts::FRAC_PI_2,
                        // Three quarters, which is what a progress ring at
                        // seventy-five percent looks like.
                        sweep: std::f32::consts::TAU * 0.75,
                        through_center: false,
                    },
                    StrokeSpec::new(8.0),
                    GREEN,
                ),
                Item::filled(
                    Shape::Arc {
                        center: [64.0, 96.0],
                        // Elliptical, so a slice drawn as though it were
                        // circular is visibly the wrong shape.
                        radii: [36.0, 24.0],
                        start: -std::f32::consts::FRAC_PI_2 * 0.6,
                        sweep: std::f32::consts::PI * 0.8,
                        through_center: true,
                    },
                    Fill::Solid(RED),
                ),
            ],
        ),
        // A shadow, which is what a mask blur is for: the same shape softened
        // and drawn behind the thing casting it. Both are here because the
        // point is the relationship -- a soft copy offset under a hard one --
        // and either alone would be a blurred rectangle.
        Scene::new(
            "mask-blur-shadow",
            vec![
                Item::filled(
                    Shape::RoundedRect {
                        min: [30.0, 34.0],
                        max: [102.0, 82.0],
                        radius: 12.0,
                    },
                    Fill::Solid([0.0, 0.0, 0.0, 0.55]),
                )
                .with_mask_blur(6.0)
                .with_blend(BlendMode::SrcOver),
                Item::filled(
                    Shape::RoundedRect {
                        min: [26.0, 26.0],
                        max: [98.0, 74.0],
                        radius: 12.0,
                    },
                    Fill::Solid(WHITE),
                )
                .with_blend(BlendMode::SrcOver),
            ],
        )
        // A light ground, because a dark shadow on the corpus's black default
        // is a shadow nobody can see. The first version of this scene was
        // exactly that: correct, compared across backends, and blank to look
        // at.
        .with_background([0.82, 0.84, 0.88, 1.0]),
        Scene::new(
            "stroke-dashed",
            vec![
                Item::stroke(
                    Shape::Polyline(vec![[12.0, 24.0], [116.0, 24.0]]),
                    StrokeSpec::new(8.0).dashed(vec![14.0, 8.0], 0.0),
                    RED,
                ),
                // The same pattern started inside its own gap, so the two lines
                // are offset against each other rather than merely repeated.
                Item::stroke(
                    Shape::Polyline(vec![[12.0, 48.0], [116.0, 48.0]]),
                    StrokeSpec::new(8.0).dashed(vec![14.0, 8.0], 14.0),
                    GREEN,
                ),
                Item::stroke(
                    Shape::Circle {
                        center: [64.0, 92.0],
                        radius: 28.0,
                    },
                    StrokeSpec::new(6.0).dashed(vec![10.0, 6.0], 0.0),
                    BLUE,
                ),
            ],
        ),
        Scene::new(
            "stroke-joins",
            vec![
                (LineJoin::Miter, RED),
                (LineJoin::Round, GREEN),
                (LineJoin::Bevel, BLUE),
            ]
            .into_iter()
            .zip(elbows())
            .map(|((join, color), points)| {
                Item::stroke(
                    Shape::Polyline(points),
                    StrokeSpec {
                        width: 14.0,
                        // Butt, so the ends contribute nothing and the only
                        // difference between these is the corner.
                        cap: LineCap::Butt,
                        join,
                        miter_limit: 8.0,
                        dash: None,
                    },
                    color,
                )
            })
            .collect(),
        ),
        // The stroke caps, one segment each. A cap is what closes an open
        // end, so a corpus of closed shapes cannot exercise one however many
        // strokes it has -- which is what the corpus was.
        Scene::new(
            "stroke-caps",
            vec![
                (LineCap::Butt, RED),
                (LineCap::Square, GREEN),
                (LineCap::Round, BLUE),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (cap, color))| {
                let y = 32.0 + i as f32 * 32.0;
                Item::stroke(
                    Shape::Polyline(vec![[32.0, y], [96.0, y]]),
                    StrokeSpec {
                        width: 16.0,
                        cap,
                        join: LineJoin::Miter,
                        miter_limit: 4.0,
                        dash: None,
                    },
                    color,
                )
            })
            .collect(),
        ),
        // A scene that clears to nothing rather than to black, which is what a
        // caller rendering a sprite or an overlay asks for -- and which no
        // other scene here does, so the whole corpus was opaque and every
        // statement about alpha was trivially true of it.
        //
        // Deliberately full of partial alpha, since that is the only place a
        // color that was never premultiplied shows: at full alpha the two
        // conventions agree exactly. And deliberately mixed, because the two
        // routes premultiply in different lines -- a polygon goes through the
        // one shared by solids and gradients, and the other two are evaluated
        // per fragment and each end in their own.
        Scene::new(
            "transparent-background",
            vec![
                Item::fill(
                    Shape::Polygon(vec![[6.0, 118.0], [64.0, 8.0], [122.0, 118.0]]),
                    [0.3, 0.9, 0.4, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::Circle {
                        center: [48.0, 64.0],
                        radius: 30.0,
                    },
                    [1.0, 0.25, 0.15, 0.6],
                )
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::RoundedRect {
                        min: [60.0, 44.0],
                        max: [116.0, 88.0],
                        radius: 14.0,
                    },
                    [0.2, 0.5, 1.0, 0.45],
                )
                .with_blend(BlendMode::SrcOver),
            ],
        )
        .with_background([0.0, 0.0, 0.0, 0.0])
        .with_samples(4),
        // An ellipse, which nothing else here can express: a rounded rectangle
        // given a large radius becomes a stadium, and a circle is one only
        // where the bounds are square. Multisampled, so the executor asks for
        // antialiasing and the public call evaluates it per fragment.
        Scene::new(
            "oval",
            vec![
                Item::fill(
                    Shape::Oval {
                        min: [8.0, 36.0],
                        max: [120.0, 76.0],
                    },
                    RED,
                )
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::Oval {
                        min: [44.0, 4.0],
                        max: [84.0, 124.0],
                    },
                    [0.2, 0.6, 1.0, 0.55],
                )
                .with_blend(BlendMode::SrcOver),
            ],
        )
        .with_samples(4),
        // The three fragment-evaluated shapes, traced rather than filled. An
        // outline is the band where the field is small, so it costs one
        // subtraction and no vertices -- and the three have to agree on what a
        // width means, which is the whole width centered on the edge.
        Scene::new(
            "analytic-outlines",
            vec![
                Item::stroke(
                    Shape::RoundedRect {
                        min: [8.0, 8.0],
                        max: [120.0, 56.0],
                        radius: 16.0,
                    },
                    StrokeSpec::new(7.0),
                    RED,
                )
                .with_blend(BlendMode::SrcOver),
                Item::stroke(
                    Shape::Circle {
                        center: [34.0, 92.0],
                        radius: 26.0,
                    },
                    StrokeSpec::new(7.0),
                    GREEN,
                )
                .with_blend(BlendMode::SrcOver),
                Item::stroke(
                    Shape::Oval {
                        min: [68.0, 70.0],
                        max: [122.0, 114.0],
                    },
                    StrokeSpec::new(7.0),
                    BLUE,
                )
                .with_blend(BlendMode::SrcOver),
            ],
        )
        .with_samples(4),
        // Rounded rectangles, which an interface is mostly made of and which
        // nothing else here draws. Two radii and a stroke: a modest one where
        // the straight edges still dominate, one large enough to be clamped to
        // half the shorter side and come out a stadium, and a stroked outline
        // where the corner arcs meet the straight runs and a tangent that was
        // slightly wrong shows as a kink.
        Scene::new(
            "rounded-rect",
            vec![
                Item::fill(
                    Shape::RoundedRect {
                        min: [12.0, 16.0],
                        max: [116.0, 60.0],
                        radius: 12.0,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::RoundedRect {
                        min: [12.0, 72.0],
                        max: [116.0, 116.0],
                        // Far past half the height, so the clamp is what
                        // decides the shape.
                        radius: 400.0,
                    },
                    BLUE,
                ),
            ],
        ),
        // Flutter's rounded superellipse, which is a different curve from the
        // rounded rectangles above and takes the same four numbers. Two
        // shapes, chosen so that between them they reach both halves of the
        // fitted table rather than only the common one.
        //
        // The first is wide and shallow: its two octants come out at ratios of
        // 5.2 and 2.2, so one is extrapolated past the table's last row and
        // the other is interpolated inside it, in a single shape. The second
        // asks for a radius larger than the box, which is clamped per axis and
        // is where a shape whose corners overrun would otherwise cross itself.
        Scene::new(
            "round-superellipse",
            vec![
                Item::fill(
                    Shape::RoundSuperellipse {
                        min: [12.0, 16.0],
                        max: [116.0, 60.0],
                        radii: [[20.0, 20.0]; 4],
                    },
                    RED,
                ),
                Item::fill(
                    Shape::RoundSuperellipse {
                        min: [12.0, 72.0],
                        max: [116.0, 116.0],
                        radii: [[400.0, 400.0]; 4],
                    },
                    BLUE,
                ),
            ],
        ),
        // Stroked, because the outline is conics and cubics rather than the
        // arcs the rounded rectangle uses, and a stroker meets them at
        // different tangents. A join that was slightly wrong shows as a kink
        // where the superellipse arc hands over to the circular one.
        Scene::new(
            "round-superellipse-stroked",
            vec![Item::stroke(
                Shape::RoundSuperellipse {
                    min: [20.0, 20.0],
                    max: [108.0, 108.0],
                    radii: [[34.0, 34.0]; 4],
                },
                StrokeSpec {
                    width: 9.0,
                    cap: LineCap::Butt,
                    join: LineJoin::Round,
                    miter_limit: 4.0,
                    dash: None,
                },
                GREEN,
            )],
        ),
        // Multisampled, so the executor asks for antialiasing and the public
        // call takes its analytic path -- which the corpus would otherwise
        // never reach, since every other scene here hands over a path.
        Scene::new(
            "rounded-rect-analytic",
            vec![Item::fill(
                Shape::RoundedRect {
                    min: [24.0, 24.0],
                    max: [104.0, 88.0],
                    radius: 22.0,
                },
                GREEN,
            )
            // Composited rather than replaced, which is what lets the shape be
            // evaluated per fragment at all: the quad it is drawn on is larger
            // than the shape, and replacing would erase the gap between them.
            .with_blend(BlendMode::SrcOver)],
        )
        .with_samples(4),
        Scene::new(
            "rounded-rect-stroked",
            vec![Item::stroke(
                Shape::RoundedRect {
                    min: [20.0, 20.0],
                    max: [108.0, 108.0],
                    radius: 28.0,
                },
                StrokeSpec {
                    width: 9.0,
                    cap: LineCap::Butt,
                    join: LineJoin::Round,
                    miter_limit: 4.0,
                    dash: None,
                },
                GREEN,
            )],
        )
        .with_samples(4),
        // A blurred group, which is three passes rather than one: the contents,
        // then one per axis of a separable Gaussian. Two backends that agreed
        // on everything else could still differ here, since this is the only
        // thing that samples a target it just rendered, twice, with computed
        // weights.
        Scene::tree(
            "layer-blurred",
            vec![Node::layer(
                LayerSpec::default().with_blur(6.0),
                vec![
                    Item::fill(
                        Shape::Rect {
                            min: [32.0, 32.0],
                            max: [96.0, 72.0],
                        },
                        WHITE,
                    )
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [64.0, 92.0],
                            radius: 18.0,
                        },
                        RED,
                    )
                    .into(),
                ],
            )],
        ),
        // The other blur: what is behind the group rather than the group
        // itself. The backdrop here is a hard-edged pattern, because the whole
        // question is whether those edges are soft inside the panel and sharp
        // outside it -- a scene of gradients would be blurred and unblurred
        // alike.
        //
        // Bounded, so the layer covers the panel rather than the frame, which
        // is what a frosted panel is and also what exercises the mapping
        // between a cut backdrop and a smaller target.
        Scene::tree(
            "layer-backdrop-blurred",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [64.0, 128.0],
                    },
                    RED,
                )
                .into(),
                Item::fill(
                    Shape::Rect {
                        min: [64.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    BLUE,
                )
                .into(),
                Item::fill(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 22.0,
                    },
                    GREEN,
                )
                .into(),
                Node::Layer {
                    layer: Box::new(LayerSpec::default().with_backdrop_blur(5.0)),
                    bounds: Some([16.0, 40.0, 112.0, 88.0]),
                    transform: Transform::default(),
                    // Composited, not the corpus default of `Src`. The layer
                    // starts out holding the filtered backdrop, and a sheet
                    // that replaces rather than blends erases it -- leaving a
                    // scene that renders identically with the filter on and
                    // off, which is how this was found.
                    children: vec![Item::fill(
                        Shape::RoundedRect {
                            min: [16.0, 40.0],
                            max: [112.0, 88.0],
                            radius: 10.0,
                        },
                        [1.0, 1.0, 1.0, 0.25],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into()],
                },
            ],
        ),
        // The same backdrop with a stencil clip in force when it cuts the pass,
        // which is the one arrangement that exercises rebuilding one.
        //
        // A backdrop filter cannot sample the attachment it is writing, so the
        // pass stops there and the next begins by drawing it back in. A scissor
        // clip survives that by itself, being state the recorder holds; a
        // stencil clip does not, being a draw in the batch the cut took away.
        // The narrowings are made again in the pass that follows, and this is
        // the scene whose `passes` and `draws` say what that costs -- one draw
        // per clip in force, which no other scene here can show because no
        // other one has a clip a scissor cannot express and a backdrop at once.
        //
        // The picture is the reason it is a *corpus* scene rather than only a
        // count. `docs/playground-parity.md` records that upstream's own
        // regression test for this cannot be a plate, because it covers its
        // evidence with an opaque draw. Nothing covers this one: the notch is
        // the clip, the blurred band is the backdrop, and both have to be there
        // on both backends.
        Scene::tree(
            "layer-backdrop-under-a-difference-clip",
            vec![Node::Clip {
                rect: None,
                // Never a scissor whatever the transform: the complement of a
                // rectangle is not one, so this goes to the stencil.
                rect_out: Some([96.0, 96.0, 128.0, 128.0]),
                shape: None,
                transform: Transform::default(),
                children: vec![
                    Item::fill(
                        Shape::Rect {
                            min: [0.0, 0.0],
                            max: [64.0, 128.0],
                        },
                        RED,
                    )
                    .into(),
                    Item::fill(
                        Shape::Rect {
                            min: [64.0, 0.0],
                            max: [128.0, 128.0],
                        },
                        BLUE,
                    )
                    .into(),
                    Node::Layer {
                        layer: Box::new(LayerSpec::default().with_backdrop_blur(5.0)),
                        bounds: Some([16.0, 40.0, 112.0, 88.0]),
                        transform: Transform::default(),
                        children: vec![Item::fill(
                            Shape::RoundedRect {
                                min: [16.0, 40.0],
                                max: [112.0, 88.0],
                                radius: 10.0,
                            },
                            [1.0, 1.0, 1.0, 0.25],
                        )
                        .with_blend(BlendMode::SrcOver)
                        .into()],
                    },
                ],
            }],
        ),
        // Layers. Everything below here needs the scene to be a tree, and none
        // of it could be said at all while a scene was a flat list of items --
        // which is why layer compositing went uncompared across backends for as
        // long as it did.
        //
        // Group opacity is the reason layers exist. Two translucent circles
        // drawn directly show where they cross; the same pair made first and
        // faded once does not. A backend that composited per shape rather than
        // per group would differ exactly on the overlap.
        Scene::tree(
            "layer-group-opacity",
            vec![Node::layer(
                LayerSpec::opacity(0.55),
                vec![
                    Item::fill(
                        Shape::Circle {
                            center: [52.0, 64.0],
                            radius: 32.0,
                        },
                        RED,
                    )
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [80.0, 64.0],
                            radius: 32.0,
                        },
                        GREEN,
                    )
                    .into(),
                ],
            )],
        ),
        // A layer that meets what is underneath through a blend rather than
        // through the default. The composite is one draw of the whole group, so
        // this is the mode applied once to a finished image -- a different
        // thing from the same mode on each shape, and the pair above is what
        // makes the difference visible.
        Scene::tree(
            "layer-blended-composite",
            vec![
                Item::filled(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    Fill::LinearGradient {
                        start: [0.0, 0.0],
                        end: [128.0, 128.0],
                        stops: vec![
                            Stop::new([0.05, 0.1, 0.35, 1.0], 0.0),
                            Stop::new([0.9, 0.85, 0.4, 1.0], 1.0),
                        ],
                        tile: TileMode::Clamp,
                    },
                )
                .into(),
                Node::layer(
                    LayerSpec::opacity(0.8).with_blend(BlendMode::SrcOver),
                    vec![Item::fill(
                        Shape::Circle {
                            center: [64.0, 64.0],
                            radius: 36.0,
                        },
                        [0.2, 0.9, 0.7, 1.0],
                    )
                    .into()],
                ),
            ],
        ),
        // A layer given the region it covers, so its target is smaller than the
        // frame and sits at an offset inside it. Everything that maps between
        // spaces has to agree about where that target is: the gradient states
        // its endpoints in the scene's space and the geometry is tessellated in
        // device pixels, and the two are projected separately.
        Scene::tree(
            "layer-bounded",
            vec![Node::bounded_layer(
                LayerSpec::opacity(0.7),
                [24.0, 40.0, 96.0, 104.0],
                vec![
                    Item::filled(
                        Shape::Rect {
                            min: [24.0, 40.0],
                            max: [96.0, 104.0],
                        },
                        Fill::LinearGradient {
                            start: [24.0, 40.0],
                            end: [96.0, 104.0],
                            stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                            tile: TileMode::Clamp,
                        },
                    )
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [60.0, 72.0],
                            radius: 26.0,
                        },
                        [1.0, 1.0, 1.0, 0.7],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            )],
        ),
        // Layers nested, the inner one bounded and opened under a transform and
        // inside both kinds of clip. An inner target is placed within its
        // parent's rather than within the frame, so compositing it as though
        // the parent filled the frame puts it off by the parent's own offset --
        // which a single layer cannot catch and this does.
        Scene::tree(
            "layer-nested-clipped",
            vec![Node::bounded_layer(
                LayerSpec::opacity(0.75),
                [24.0, 40.0, 96.0, 104.0],
                vec![
                    Item::filled(
                        Shape::Rect {
                            min: [24.0, 40.0],
                            max: [96.0, 104.0],
                        },
                        Fill::LinearGradient {
                            start: [24.0, 40.0],
                            end: [96.0, 104.0],
                            stops: vec![
                                Stop::new([1.0, 0.25, 0.0, 1.0], 0.0),
                                Stop::new(BLUE, 1.0),
                            ],
                            tile: TileMode::Clamp,
                        },
                    )
                    .with_clip([30.0, 46.0, 90.0, 98.0])
                    .with_clip_shape(Shape::Polygon(vec![
                        [60.0, 44.0],
                        [92.0, 100.0],
                        [28.0, 100.0],
                    ]))
                    .into(),
                    Node::bounded_layer(
                        LayerSpec::opacity(0.5),
                        [38.0, 52.0, 82.0, 96.0],
                        vec![Item::fill(
                            Shape::Circle {
                                center: [60.0, 74.0],
                                radius: 22.0,
                            },
                            GREEN,
                        )
                        .into()],
                    )
                    .with_transform(Transform {
                        scale: [1.0, 1.0],
                        rotate: 0.0,
                        skew: [0.0, 0.0],
                        translate: [4.0, 6.0],
                        perspective: [0.0, 0.0],
                    }),
                ],
            )],
        ),
        // Text, which the corpus could not describe until a scene could name a
        // glyph run. What it buys is not another picture but the invariants
        // this collection already asserts, applied to a path that was outside
        // them: that a run over an opaque ground leaves no pixel transparent,
        // that no channel exceeds the alpha it was multiplied by, and that the
        // same run renders identically twice. Coverage arrives premultiplied
        // from a texture rather than computed, so none of those followed from
        // the shape cases.
        // Two scenes for two draws that used to be many, and they are here
        // because the counts baseline covers this collection and not the
        // catalog. Both wins were asserted directly when they landed; neither
        // was guarded against creeping back, which is what a row in
        // `tests/cost-baseline.txt` is for.
        // Three capabilities the corpus had no scene for at all, found by asking
        // it which node kinds and fills it holds. Each is here on the rule this
        // collection is built on -- a scene earns its place by exercising
        // something the others do not -- and each was being compared only in the
        // catalog, whose budget is three per cent of a frame against this one's
        // unit or two per pixel.
        Scene::new(
            "image-sampled",
            vec![Item::filled(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                // Magnified about fourteen times from an eight-texel sheet, so
                // every interior pixel is an interpolation between four texels
                // rather than a texel copied. That is the arithmetic two
                // backends can disagree about and nothing else here performs:
                // the nine-patch beside this one stretches too, but through a
                // call that places nine quads, where this is one draw and the
                // sampler.
                Fill::Image {
                    rect: [8.0, 8.0, 120.0, 120.0],
                    source: [0.0, 0.0, 1.0, 1.0],
                    tile: TileMode::Clamp,
                    sampling: Sampling::Linear,
                    alpha: 1.0,
                    tint: [1.0, 1.0, 1.0, 1.0],
                },
            )
            .with_blend(BlendMode::SrcOver)],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "mesh-interpolated",
            // Two triangles sharing an edge, each vertex a different color, so
            // every interior fragment is a barycentric mix of three of them.
            // `draw_vertices` is a whole call this collection did not reach, and
            // the interpolation is the part of it that is arithmetic rather than
            // geometry -- a fragment's color here is computed by the rasterizer,
            // not by any shader either backend shares.
            //
            // The shared edge is what makes it more than a gradient: the two
            // triangles must agree exactly along it, and a renderer that
            // interpolated per triangle in device space rather than per vertex
            // would leave a seam that neither triangle alone would show.
            vec![Node::Mesh(Box::new(MeshSpec {
                mode: VertexMode::Triangles,
                positions: vec![[16.0, 16.0], [112.0, 16.0], [112.0, 112.0], [16.0, 112.0]],
                colors: vec![
                    [1.0, 0.0, 0.0, 1.0],
                    [0.0, 1.0, 0.0, 1.0],
                    [0.0, 0.0, 1.0, 1.0],
                    [1.0, 1.0, 0.0, 1.0],
                ],
                texture_coords: Vec::new(),
                indices: vec![0, 1, 2, 0, 2, 3],
                fill: Fill::Solid([1.0, 1.0, 1.0, 1.0]),
                tint_blend: BlendMode::Modulate,
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
                image_filter: ImageFilter::None,
                mask_blur: 0.0,
            }))],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "layer-dilated",
            // A cross dilated by a layer's morphology, which is a separable
            // maximum over the neighborhood rather than a weighted sum -- the
            // one filter here whose arithmetic is a comparison. A cross rather
            // than a rectangle because a dilation fills an inner corner by its
            // radius, which a convex shape cannot show.
            //
            // Asymmetric radii, so a pass that ran the same distance along both
            // axes would draw this and a scene with one radius identically.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    morphology: Some(MorphologySpec {
                        radius: [8.0, 3.0],
                        dilate: true,
                    }),
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![
                    Item::fill(
                        Shape::Rect {
                            min: [56.0, 24.0],
                            max: [72.0, 104.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                    Item::fill(
                        Shape::Rect {
                            min: [24.0, 56.0],
                            max: [104.0, 72.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            }],
        )
        .with_background(DARK_GROUND),
        // The same cross, the same radii, drawn at half the size under a scale
        // of two -- so the content lands on exactly the pixels `layer-dilated`
        // covers, and the only thing that can differ between the two scenes is
        // how far the dilation reached.
        //
        // Which makes this a pin on the radius convention rather than a picture.
        // The radius is a local length scaled by the transform at the pass, as
        // upstream's is, so the pair differs: eight and three against sixteen and
        // six. It did not until 2026-09-29, when the convention was flipped to
        // match upstream, and this scene is what made the flip visible --
        // `the_dilation_under_a_scale_reaches_twice_as_far` is the assertion, and
        // it is the inverse of the one that stood here before. `non-parity.md` 17
        // has what remains, which is the conversion's shape rather than its units.
        // Writing the scene before the change is what let either direction fail
        // loudly.
        Scene::tree(
            "layer-dilated-under-scale",
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    morphology: Some(MorphologySpec {
                        radius: [8.0, 3.0],
                        dilate: true,
                    }),
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::scale(2.0, 2.0),
                children: vec![
                    Item::fill(
                        Shape::Rect {
                            min: [28.0, 12.0],
                            max: [36.0, 52.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                    Item::fill(
                        Shape::Rect {
                            min: [12.0, 28.0],
                            max: [52.0, 36.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            }],
        )
        .with_background(DARK_GROUND),
        // The same cross under an *anisotropic* scale, which is the case a single
        // conversion factor cannot carry. `non-parity.md` 17 is about the shape of
        // that conversion rather than its units: upstream transforms one direction
        // vector per pass and takes a length from each, so `scale(2, 5)` on radii
        // of eight and three reaches sixteen along x and fifteen along y. Here the
        // factor is `max_scale_of`, the larger of the two, so both radii are
        // multiplied by five and x reaches forty.
        //
        // Drawn at a fifth the height and half the width so the cross lands on
        // device pixels comparable to the pair above, and sized to keep forty
        // clear of `applied_radius`'s clamp -- which is the target extent, so a
        // radius that clamped would report the clamp instead of the convention and
        // the test below would pass for the wrong reason.
        //
        // **This scene cannot be checked by rendering it.** The comment on the
        // next scene says why in full: every comparison the corpus makes is
        // between two backends or two devices, and the conversion happens above
        // both, so both sides are wrong together and agree. What makes this scene
        // worth having is `the_dilation_under_an_anisotropic_scale_uses_one_factor`
        // reading the radii out of the recording -- and that test asserts today's
        // behavior, so the per-axis change fails it and has to say so.
        //
        // Unlike the pair above, this one the *cost* baseline sees. A pass covers
        // `MORPHOLOGY_TAPS` texels and that is thirty-two, so the inflated forty
        // along x arrives as two passes where sixteen would arrive as one: five
        // passes here against the four the pair records, and four again once the
        // conversion is per axis. The pair could not show that because eight and
        // sixteen both fit in one pass. So the scene is pinned twice, and the cost
        // table is the one that moves without anybody reading a radius.
        Scene::tree(
            "layer-dilated-under-anisotropic-scale",
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    morphology: Some(MorphologySpec {
                        radius: [8.0, 3.0],
                        dilate: true,
                    }),
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::scale(2.0, 5.0),
                children: vec![
                    Item::fill(
                        Shape::Rect {
                            min: [28.0, 6.0],
                            max: [36.0, 20.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                    Item::fill(
                        Shape::Rect {
                            min: [14.0, 11.0],
                            max: [50.0, 15.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            }],
        )
        .with_background(DARK_GROUND),
        // A composed filter handed to a group, under a scale. Three things here
        // had no corpus scene between them until this one: `LayerSpec::filter`,
        // which reaches `save_layer_filtered` and which nothing in the corpus set
        // at all; a whole-group filter under a transform; and a composition, which
        // is what that field exists for.
        //
        // What this scene does *not* do is worth stating, because the obvious
        // reading is wrong. A blur handed over as a filter was once left in device
        // pixels while the same blur on the layer itself was converted to them,
        // and a scene like this one would not have caught that: every comparison
        // the corpus makes is between two backends or two devices, and the
        // conversion happens above both, so each side would have been wrong
        // together and agreed. The cost row does not move either -- measured, by
        // reverting the conversion: the sigma changes and the pass and draw counts
        // do not. `emblema`'s `filter_space.rs` is what holds the five
        // spellings against each other, and it is a unit test for that reason.
        //
        // What this scene is for is the part a cross-comparison can see: whether
        // two backends agree about a composed filter's passes, and two devices
        // about its arithmetic. Neither had a corpus scene to disagree over.
        //
        // Blur inside, recolor outside, so the composition is not symmetric and a
        // pass order swapped between the halves would show. The blur is the half
        // that carries a length, which makes it the half the conversion has to
        // reach through the `Compose` to find.
        // A blur under an *anisotropic* scale, which is the case one conversion
        // factor cannot carry. `non-parity.md` 20 is the entry: upstream's
        // `ExtractScale` takes the length of each transformed basis vector, so
        // `scale(2, 3)` on deviations of six and two reaches twelve and six. Here
        // `Layer::scaled_by` multiplies both by `max_scale_of`, the larger of the
        // two, so x reaches eighteen -- and `scale(3, 2)` reaches eighteen and six
        // as well, which is the sharp end of it: two transforms that transpose
        // each other produce one picture.
        //
        // Stated through `filter` rather than `blur`, because `LayerSpec::blur` is
        // one number and cannot say a deviation per axis at all. That also puts
        // the scene on `ImageFilter::scaled_by`, which is the spelling the defect
        // lives in.
        //
        // **The deviations are small on purpose.** A sigma drives the reduction of
        // entry 6 as well as the blur itself: past a kernel radius of
        // `BLUR_MAX_TAPS` the pass halves its target and divides the deviation,
        // which moves the pass count. Six and two under a factor of three reach
        // eighteen, and eighteen is under the threshold of about nineteen -- so
        // neither the inflated pair nor the corrected one reduces, and the pass
        // count is the same either way. A scene whose deviations crossed that
        // threshold would move its cost row for a second reason and leave a reader
        // unable to tell which one did it.
        //
        // So this scene is pinned by reading the sigmas out of the recording, in
        // `a_blur_under_an_anisotropic_scale_uses_one_factor`, and not by its cost
        // row and not by its picture -- every comparison the corpus makes is
        // between two backends or two devices, and the conversion sits above both.
        Scene::tree(
            "layer-blurred-under-anisotropic-scale",
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    filter: ImageFilter::Blur {
                        sigma_x: 6.0,
                        sigma_y: 2.0,
                    },
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::scale(2.0, 3.0),
                children: vec![
                    Item::fill(
                        Shape::Rect {
                            min: [22.0, 10.0],
                            max: [42.0, 18.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [32.0, 30.0],
                            radius: 8.0,
                        },
                        [0.4, 0.9, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            }],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "layer-composed-filter-under-scale",
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    filter: ImageFilter::Compose {
                        outer: Box::new(ImageFilter::Color(ColorFilter::Blend {
                            color: [0.2, 0.5, 1.0, 0.6],
                            mode: BlendMode::SrcOver,
                        })),
                        inner: Box::new(ImageFilter::Blur {
                            sigma_x: 4.0,
                            sigma_y: 1.5,
                        }),
                    },
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::scale(2.0, 2.0),
                children: vec![
                    Item::fill(
                        Shape::Rect {
                            min: [20.0, 14.0],
                            max: [44.0, 26.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                    Item::fill(
                        Shape::Circle {
                            center: [32.0, 42.0],
                            radius: 12.0,
                        },
                        [1.0, 0.9, 0.2, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            }],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "atlas-turned-sprites",
            // Sprites out of the sheet, each turned and scaled by an amount
            // nothing else uses, which is what `drawAtlas` is: one call
            // producing many quads, each placed by its own similarity. Turned
            // rather than merely placed, because an axis-aligned sprite lands
            // texel on pixel and never asks the sampler anything -- the
            // rotation is what makes every fragment an interpolation, and it is
            // computed per sprite from a transform this collection has no other
            // scene for.
            vec![Node::Atlas(Box::new(AtlasSpec {
                sprites: (0..4)
                    .map(|i| {
                        let n = i as f32;
                        SpriteSpec {
                            // A quadrant of the eight-texel sheet each, so the
                            // four differ in what they sample as well as where
                            // they land.
                            source: [(i % 2) as f32 * 4.0, (i / 2) as f32 * 4.0, 0.0, 0.0],
                            rotate: 0.2 + n * 0.35,
                            scale: 6.0,
                            translate: [32.0 + (i % 2) as f32 * 64.0, 32.0 + (i / 2) as f32 * 64.0],
                            color: [1.0, 1.0, 1.0, 1.0],
                        }
                    })
                    .map(|mut sprite| {
                        sprite.source[2] = sprite.source[0] + 4.0;
                        sprite.source[3] = sprite.source[1] + 4.0;
                        sprite
                    })
                    .collect(),
                tint_blend: BlendMode::Modulate,
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            }))],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "layer-resampled-by-its-matrix",
            // A group whose matrix magnifies it on the way back, which is a
            // different thing from drawing its contents larger: what is
            // resampled is the finished image, so the edges arrive soft and
            // enlarged rather than redrawn sharp. `dart:ui` has both and the
            // distinction is the reason.
            //
            // Here for the sampler rather than for the distinction, which the
            // catalog states with a pair of plates side by side. Every interior
            // pixel of the magnified region is an interpolation between four
            // texels of a target this renderer produced, and the machinery that
            // performs it is the same seed-and-composite path a backdrop filter
            // uses -- so a change to that path moves this scene, which was true
            // of no scene here until now.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    matrix: Some(Transform {
                        scale: [3.0, 3.0],
                        translate: [-56.0, -56.0],
                        ..Transform::default()
                    }),
                    ..LayerSpec::default()
                }),
                bounds: Some([40.0, 40.0, 88.0, 88.0]),
                transform: Transform::default(),
                children: vec![
                    Item::fill(
                        Shape::Circle {
                            center: [64.0, 64.0],
                            radius: 16.0,
                        },
                        [1.0, 0.4, 0.1, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                    Item::fill(
                        Shape::Rect {
                            min: [44.0, 44.0],
                            max: [60.0, 60.0],
                        },
                        [0.2, 0.7, 1.0, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                ],
            }],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "shadow-cast-by-a-card",
            // `drawShadow`, which nothing else here reaches. The mask blur two
            // scenes up is a paint's, softening a shape where it stands; this
            // is the call that derives an offset, a deviation and an alpha from
            // one elevation and remaps the color tonally on the way -- three
            // numbers and a curve, none of which a blurred shape exercises.
            //
            // On a pale ground, alone in this collection, because a shadow is a
            // darkening and the dark ground everything else uses would hide the
            // thing under test.
            vec![Node::Shadow(Box::new(ShadowSpec {
                shape: Shape::RoundedRect {
                    min: [32.0, 34.0],
                    max: [96.0, 86.0],
                    radius: 10.0,
                },
                color: [0.0, 0.0, 0.0, 1.0],
                elevation: 6.0,
                transparent_occluder: false,
                transform: Transform::default(),
                with_caster: true,
            }))],
        )
        // The pale ground the catalog's shadow plates use, stated as eighths of
        // a byte rather than as round decimals: nine tenths of 255 is 229.5
        // exactly, and a background sitting on a tie makes every pixel outside
        // the shadow differ by a unit between two backends for no reason of the
        // scene's own. Measured before it was changed -- 79 per cent of the
        // frame, all of it ground.
        .with_background([230.0 / 255.0, 230.0 / 255.0, 235.0 / 255.0, 1.0])
        .with_samples(4),
        Scene::tree(
            "picture-drawn-into-a-picture",
            // `drawPicture`, which composes a finished recording into the one
            // being made. It is here for the counts rather than for the pixels:
            // this renderer tessellates a sub-picture again rather than
            // replaying its draws, which `docs/non-parity.md` records as a
            // deliberate difference, and `cost-baseline.txt` is the only thing
            // in the tree that can say what that costs. Nothing was measuring
            // it, so nothing would have noticed the count doubling.
            //
            // Nested twice and placed by a transform, so the inner picture's
            // geometry is walked through two placements and the row below
            // counts every vertex of it.
            vec![Node::Picture(Box::new(PictureSpec {
                size: Extent2D::new(128, 128),
                transform: Transform {
                    scale: [0.5, 0.5],
                    translate: [32.0, 32.0],
                    ..Transform::default()
                },
                blend: BlendMode::SrcOver,
                children: vec![
                    Item::fill(
                        Shape::Circle {
                            center: [64.0, 64.0],
                            radius: 48.0,
                        },
                        [0.9, 0.3, 0.2, 1.0],
                    )
                    .with_blend(BlendMode::SrcOver)
                    .into(),
                    Node::Picture(Box::new(PictureSpec {
                        size: Extent2D::new(128, 128),
                        transform: Transform {
                            scale: [0.5, 0.5],
                            translate: [32.0, 32.0],
                            ..Transform::default()
                        },
                        blend: BlendMode::SrcOver,
                        children: vec![Item::fill(
                            Shape::Rect {
                                min: [24.0, 24.0],
                                max: [104.0, 104.0],
                            },
                            [0.2, 0.6, 0.9, 1.0],
                        )
                        .with_blend(BlendMode::SrcOver)
                        .into()],
                    })),
                ],
            }))],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "flood-fill-under-a-rotation",
            // `drawPaint`, which covers whatever the clip admits rather than
            // any shape. The call is the last of this collection's uncovered
            // ones and it earns a scene for the extent rather than the fill:
            // what a paint covers is the clip carried *back* through the
            // transform, and a renderer taking it forward instead leaves the
            // frame's corners empty under a rotation.
            //
            // Filled with a gradient rather than a color so the picture says
            // which way the paint's own space runs, and clipped away from one
            // corner so the scene shows a paint obeying a clip as well as
            // covering one.
            vec![Node::Clip {
                rect: None,
                rect_out: Some([96.0, 96.0, 128.0, 128.0]),
                shape: None,
                transform: Transform::default(),
                children: vec![Node::Paint(Box::new(PaintSpec {
                    fill: Fill::LinearGradient {
                        start: [0.0, 0.0],
                        end: [128.0, 128.0],
                        stops: vec![
                            Stop::new([0.10, 0.16, 0.55, 1.0], 0.0),
                            Stop::new([0.95, 0.80, 0.20, 1.0], 1.0),
                        ],
                        tile: TileMode::Clamp,
                    },
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: None,
                    transform: Transform {
                        rotate: 30.0f32.to_radians(),
                        translate: [64.0, 64.0],
                        ..Transform::default()
                    },
                }))],
            }],
        )
        .with_background(DARK_GROUND),
        Scene::tree(
            "nine-patch-stretched",
            vec![Node::NinePatch(Box::new(NinePatchSpec {
                // Stretched hard in both directions, which is what a
                // nine-patch is for and where the seams between its quads show
                // if the half-texel inset that replaced their per-draw
                // clamping ever stops being applied.
                // In texels of the fixture sheet, which is eight by eight: the
                // middle four, leaving a two-texel frame that must not stretch.
                center: [2.0, 2.0, 6.0, 6.0],
                into: [6.0, 6.0, 122.0, 122.0],
                alpha: 1.0,
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
            }))],
        )
        .with_background(DARK_GROUND)
        .with_samples(4),
        Scene::tree(
            "point-field",
            vec![Node::Points(Box::new(PointsSpec {
                // A ring of dots, round-capped, which is the arrangement that
                // costs a draw each unless the field carries its centers on
                // the vertices. Round rather than square because that is the
                // one the fragment evaluates: a square cap is its quad, and
                // its edges are the rasterizer's.
                mode: PointMode::Points,
                // Twelve dots at a radius of eleven rather than
                // twenty-four at five and a half, and the size is the part
                // that had to be measured. The field's edge comes from the
                // derivative of an interpolated value, and how much two
                // devices disagree about that grows as the disc shrinks:
                // at five and a half, v3d's neighbor and the software
                // rasterizer differed by sixteen on thirty-two pixels, twice
                // what `Tolerance::ANALYTIC` allows. Bending that budget for
                // one scene would be the same enumerating mistake this file's
                // derivation has made twice; drawing a disc big enough to be
                // measured is not.
                points: (0..12)
                    .map(|i| {
                        let a = i as f32 * std::f32::consts::TAU / 12.0;
                        [64.0 + 40.0 * a.cos(), 64.0 + 40.0 * a.sin()]
                    })
                    .collect(),
                stroke: StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(22.0)
                },
                color: WHITE,
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
                clip_shape: None,
            }))],
        )
        .with_background(DARK_GROUND)
        .with_samples(4),
        Scene::tree(
            "glyph-run",
            vec![Node::Glyphs(Box::new(GlyphRunSpec {
                // A block rather than a line, because the corpus requires a
                // scene to cover enough of its target to be testing something
                // and one row of four glyphs covers under two per cent. Five
                // rows of seven is text-shaped and covers enough of the frame
                // for the invariants below to have somewhere to fail.
                glyphs: (0..5)
                    .flat_map(|row| {
                        (0..7).map(move |column| {
                            (
                                (row * 7 + column) % 4,
                                [10.0 + column as f32 * 16.0, 24.0 + row as f32 * 18.0],
                            )
                        })
                    })
                    .collect(),
                color: [1.0, 0.85, 0.2, 1.0],
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
                image_filter: ImageFilter::None,
                mask_blur: 0.0,
                mask_blur_style: MaskBlurStyle::Normal,
            }))],
        )
        .with_background([0.05, 0.06, 0.09, 1.0]),
        // The same block over nothing at all. Coverage from a texture is where
        // premultiplication is easiest to get wrong -- the texel is a coverage
        // and the color is the paint's, so the multiplication happens in the
        // shader rather than being carried in -- and the check for it can only
        // fail where alpha is partial, which over an opaque ground it never is.
        // The half-covered glyph in the fixture is what makes it partial here.
        Scene::tree(
            "glyph-run-over-nothing",
            vec![Node::Glyphs(Box::new(GlyphRunSpec {
                glyphs: (0..5)
                    .flat_map(|row| {
                        (0..7).map(move |column| {
                            (
                                (row * 7 + column) % 4,
                                [10.0 + column as f32 * 16.0, 24.0 + row as f32 * 18.0],
                            )
                        })
                    })
                    .collect(),
                color: [0.4, 0.9, 1.0, 1.0],
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
                image_filter: ImageFilter::None,
                mask_blur: 0.0,
                mask_blur_style: MaskBlurStyle::Normal,
            }))],
        )
        .with_background([0.0, 0.0, 0.0, 0.0]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backdrop filter counts whichever field it was stated in.
    ///
    /// `filters_its_backdrop` is what keeps such a scene out of the
    /// bounded-against-unbounded comparison, whose premise is that stripping a
    /// layer's bounds changes only the allocation. For these it changes the
    /// picture -- the bounds say which part of the target is filtered -- so a
    /// spelling this missed would be compared against a different picture and
    /// fail for the wrong reason. It missed the general field until this.
    #[test]
    fn a_backdrop_filter_counts_in_either_spelling() {
        let of = |layer: LayerSpec| {
            Scene::tree(
                "probe",
                vec![Node::Layer {
                    layer: Box::new(layer),
                    bounds: Some([8.0, 8.0, 40.0, 40.0]),
                    transform: Transform::default(),
                    children: vec![],
                }],
            )
            .filters_its_backdrop()
        };
        assert!(
            of(LayerSpec::default().with_backdrop_blur(4.0)),
            "the sigma a Copy layer can hold"
        );
        assert!(
            of(LayerSpec {
                backdrop: ImageFilter::blur(4.0),
                ..LayerSpec::default()
            }),
            "the same blur written as the general filter"
        );
        assert!(
            of(LayerSpec {
                backdrop: ImageFilter::Color(ColorFilter::blend(
                    [1.0, 0.0, 0.0, 1.0],
                    BlendMode::Exclusion
                )),
                ..LayerSpec::default()
            }),
            "a color filter over the backdrop, which is not a blur and is still \
             a filter"
        );
        assert!(
            !of(LayerSpec::default()),
            "and a layer that filters nothing does not count"
        );
    }

    /// A projection on a clip's own transform is a feature, and is strippable.
    ///
    /// The pair is what makes any feature checkable: a scene is rendered with it
    /// and again with it taken out, and the two have to differ. Perspective on a
    /// clip was missing from the first half and present in the second, which is
    /// the worst way round -- `plain` removed it, `carries_a_visual_feature`
    /// said there was nothing to remove, and a scene whose only projection sat
    /// on a clip would have been left out of the comparison it needed and
    /// reported as covered.
    #[test]
    fn a_projection_on_a_clip_is_a_feature_and_comes_off() {
        let projected = || {
            Scene::tree(
                "probe",
                vec![Node::Clip {
                    rect: Some([8.0, 8.0, 120.0, 120.0]),
                    rect_out: None,
                    shape: None,
                    transform: Transform {
                        perspective: [0.004, 0.0],
                        ..Transform::default()
                    },
                    children: vec![Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [0.0, 0.0],
                            max: [128.0, 128.0],
                        },
                        [1.0, 1.0, 0.0, 1.0],
                    )))],
                }],
            )
        };
        assert!(
            projected().carries_a_visual_feature(),
            "a projection on the clip's own transform is what the clip's edges \
             are drawn through"
        );
        assert!(
            !projected().plain().carries_a_visual_feature(),
            "and stripping has to take it off, or the pair is a scene against \
             itself"
        );

        // And the other half of the same omission: a projection under a clip,
        // which `plain` reached only because it recurses through one itself.
        let under = || {
            Scene::tree(
                "probe",
                vec![Node::Clip {
                    rect: Some([8.0, 8.0, 120.0, 120.0]),
                    rect_out: None,
                    shape: None,
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(
                        Item::fill(
                            Shape::Rect {
                                min: [0.0, 0.0],
                                max: [128.0, 128.0],
                            },
                            [1.0, 1.0, 0.0, 1.0],
                        )
                        .with_transform(Transform {
                            perspective: [0.004, 0.0],
                            ..Transform::default()
                        }),
                    ))],
                }],
            )
        };
        assert!(under().carries_a_visual_feature());
        assert!(
            !under().plain().carries_a_visual_feature(),
            "a clip's children have to be reached too"
        );
    }

    #[test]
    fn a_shear_composes_in_the_order_the_transform_states() {
        // `to_affine` names an order -- scale, shear, rotate, translate -- and
        // an order stated in prose and nowhere else is one a refactor can
        // reverse silently. The two arrangements differ, which is what makes
        // this checkable at all: a shear before a rotation and one after it
        // are not the same transform.
        let skewed = Transform {
            skew: [0.5, 0.0],
            ..Transform::default()
        };
        // The shear adds half of Y to X, so the unit Y vector leans and the
        // unit X vector does not. Reading the columns says which axis moved,
        // and a transposed matrix would move the other one.
        let m = skewed.to_affine().expect("no perspective asked for");
        assert_eq!(m.transform_vector2(Vec2::X), Vec2::X, "X is untouched");
        assert_eq!(
            m.transform_vector2(Vec2::Y),
            Vec2::new(0.5, 1.0),
            "Y leans by the coefficient"
        );

        // Scale first, so the coefficient is read in the shape's own units.
        // Were the shear applied after the scale, doubling X would double the
        // lean as well, and a scene that scaled a sheared shape would slant
        // differently for having been written the other way round.
        let scaled = Transform {
            skew: [0.5, 0.0],
            scale: [2.0, 1.0],
            ..Transform::default()
        };
        assert_eq!(
            scaled
                .to_affine()
                .expect("affine")
                .transform_vector2(Vec2::Y),
            Vec2::new(0.5, 1.0),
            "the lean is in unscaled units"
        );

        // And the rotation is outside the shear rather than inside it: a
        // quarter turn carries the leaning Y axis onto a leaning X axis. The
        // other order would lean the axis the turn had already moved, and the
        // two results are not the same vector.
        let turned = Transform {
            skew: [0.5, 0.0],
            rotate: std::f32::consts::FRAC_PI_2,
            ..Transform::default()
        };
        let y = turned
            .to_affine()
            .expect("affine")
            .transform_vector2(Vec2::Y);
        assert!(
            (y - Vec2::new(-1.0, 0.5)).length() < 1e-6,
            "a quarter turn should carry the sheared axis, got {y:?}"
        );
    }

    #[test]
    fn every_scene_has_a_distinct_name() {
        // Names key report rows and tolerance tables, so a duplicate would make
        // two scenes indistinguishable in results.
        let mut names: Vec<&str> = corpus().iter().map(|s| s.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate scene name");
    }

    #[test]
    fn every_scene_draws_something() {
        for scene in corpus() {
            assert!(!scene.items.is_empty(), "{} draws nothing", scene.name);
            assert!(!scene.size.is_empty(), "{} has no area", scene.name);
            assert!(scene.samples.is_power_of_two(), "{}", scene.name);
        }
    }

    #[test]
    fn the_corpus_covers_more_than_one_kind_of_work() {
        let scenes = corpus();
        assert!(scenes.iter().any(|s| s.samples > 1), "no antialiased scene");
        assert!(
            scenes.iter().any(|s| s.items().any(|i| i.stroke.is_some())),
            "no stroked scene"
        );
        assert!(
            scenes
                .iter()
                .any(|s| s.items().any(|i| i.blend == BlendMode::SrcOver)),
            "no blended scene"
        );
        assert!(
            scenes
                .iter()
                .any(|s| s.items().any(|i| i.transform != Transform::default())),
            "no transformed scene"
        );
    }

    #[test]
    fn a_transform_composes_scale_rotation_and_translation() {
        let t = Transform {
            scale: [2.0, 2.0],
            rotate: 0.0,
            skew: [0.0, 0.0],
            translate: [10.0, 5.0],
            perspective: [0.0, 0.0],
        };
        let p = t
            .to_affine()
            .expect("affine")
            .transform_point2(Vec2::new(1.0, 1.0));
        assert!((p - Vec2::new(12.0, 7.0)).length() < 1e-5);
    }
}

#[cfg(test)]
mod tolerance_tests {
    use super::*;
    use crate::image::Tolerance;

    fn rounded(radius: f32) -> Shape {
        Shape::RoundedRect {
            min: [10.0, 10.0],
            max: [90.0, 70.0],
            radius,
        }
    }

    /// An axis-aligned rectangle on whole coordinates is the one shape whose
    /// edges cannot land on a tie, and everything else is judged against it.
    #[test]
    fn only_geometry_that_can_land_on_a_tie_gets_that_budget() {
        let grid = Shape::Rect {
            min: [10.0, 10.0],
            max: [90.0, 70.0],
        };
        let square = |shape: Shape| Scene::new("s", vec![Item::fill(shape, WHITE)]);

        assert_eq!(
            square(grid.clone()).tolerance().outlier_fraction,
            0.0,
            "a rectangle whose edges sit on pixel boundaries was given room to \
             lose one, and its centers are half a pixel from the nearest edge"
        );

        // Curved: an edge crosses the grid at every angle, so it passes close
        // to a center somewhere along its length.
        assert!(
            square(Shape::Circle {
                center: [50.0, 40.0],
                radius: 30.0,
            })
            .tolerance()
            .outlier_fraction
                > 0.0,
            "a circle's edge cannot avoid pixel centers and was given no room"
        );

        // The same rectangle, turned. The shape did not change and the
        // alignment did, which is the distinction the derivation rests on.
        let turned = Scene::new(
            "s",
            vec![Item::fill(grid.clone(), WHITE).with_transform(Transform {
                rotate: 0.3,
                ..Transform::default()
            })],
        );
        assert!(
            turned.tolerance().outlier_fraction > 0.0,
            "a rotated rectangle was treated as though it were still aligned"
        );

        // And half a pixel over, which is the case a whole-number test catches
        // and a "did anyone set a transform" test does not.
        let nudged = Scene::new(
            "s",
            vec![Item::fill(grid, WHITE).with_transform(Transform {
                translate: [0.5, 0.0],
                ..Transform::default()
            })],
        );
        assert!(
            nudged.tolerance().outlier_fraction > 0.0,
            "a rectangle moved onto pixel centers was treated as aligned"
        );
    }

    /// Multisampling resolves a tie into partial coverage, so the aliased
    /// budget must not follow the geometry into a multisampled scene -- the
    /// multisample profile already carries its own, for the same edges.
    #[test]
    fn a_multisampled_scene_keeps_its_own_budget() {
        // A rectangle rather than a curve, and the shape is the whole of what
        // this asserts. A multisampled circle is drawn from its distance field
        // and is answered by the branch above this one; the question here is
        // what the scenes that reach *this* branch are given.
        let scene = Scene::new(
            "s",
            vec![Item::fill(
                Shape::Rect {
                    min: [20.0, 10.0],
                    max: [80.0, 70.0],
                },
                WHITE,
            )],
        )
        .with_samples(4);
        assert_eq!(scene.tolerance(), Tolerance::MULTISAMPLED);
    }

    /// Storing every fragment is not enough to be compared exactly.
    ///
    /// `EXACT` asks two things and the second is easy to lose: that no draw
    /// computes its color, *and* that nothing is composited out of a target on
    /// the way to the frame. A picture is such a target, and the composite is
    /// arithmetic -- a sample, an alpha and a blend -- whatever it is
    /// compositing. So a scene made entirely of solid opaque draws is exact
    /// drawn flat and is not exact drawn inside a picture.
    ///
    /// The corpus has a scene that says the same thing on a device --
    /// `picture-drawn-into-a-picture`, where two backends land two levels apart
    /// on a tenth of the frame with nothing but solid colors in it -- but that
    /// one needs two devices and skips without them. This is the same claim
    /// where it cannot skip.
    #[test]
    fn storing_every_fragment_is_not_exact_through_a_picture() {
        let draw = || {
            Item::fill(
                Shape::Rect {
                    min: [10.0, 10.0],
                    max: [90.0, 70.0],
                },
                WHITE,
            )
        };
        let flat = Scene::new("flat", vec![draw()]);
        assert_eq!(
            flat.tolerance().per_channel,
            Tolerance::EXACT.per_channel,
            "an opaque solid draw stores every fragment and is compared exactly"
        );

        let through_a_picture = Scene::tree(
            "through-a-picture",
            vec![Node::Picture(Box::new(PictureSpec {
                size: Extent2D::new(128, 128),
                transform: Transform::default(),
                blend: BlendMode::SrcOver,
                children: vec![Node::Draw(Box::new(draw()))],
            }))],
        );
        assert_ne!(
            through_a_picture.tolerance().per_channel,
            Tolerance::EXACT.per_channel,
            "the composite out of a picture's target is arithmetic even when \
             everything drawn into it was written down rather than computed"
        );
        assert_eq!(through_a_picture.tolerance().per_channel, 2);
    }

    /// One unit per store, and a group is a store.
    #[test]
    fn a_group_is_allowed_the_rounding_of_the_target_it_is_composited_from() {
        // Translucent, and that is what makes it a fragment arrived at by
        // arithmetic rather than one written down. An opaque `SrcOver` draw
        // leaves nothing to combine with and stores, so it would be held to
        // `EXACT` and this test would be counting stores that do not happen.
        let inner = || {
            Item::fill(
                Shape::Rect {
                    min: [10.0, 10.0],
                    max: [90.0, 70.0],
                },
                [1.0, 1.0, 1.0, 0.5],
            )
            .with_blend(BlendMode::SrcOver)
        };
        let flat = Scene::new("flat", vec![inner()]);
        assert_eq!(flat.tolerance().per_channel, 1, "one store, one rounding");

        let grouped = Scene::tree(
            "grouped",
            vec![Node::layer(
                LayerSpec::default(),
                vec![Node::Draw(Box::new(inner()))],
            )],
        );
        assert_eq!(
            grouped.tolerance().per_channel,
            2,
            "a group is rendered into a target and composited out of it, so \
             its fragments are quantized twice"
        );

        let nested = Scene::tree(
            "nested",
            vec![Node::layer(
                LayerSpec::default(),
                vec![Node::layer(
                    LayerSpec::default(),
                    vec![Node::Draw(Box::new(inner()))],
                )],
            )],
        );
        assert_eq!(
            nested.tolerance().per_channel,
            3,
            "nesting adds a target, and the bound counts targets"
        );
    }

    #[test]
    fn only_a_scene_that_will_be_drawn_analytically_gets_that_budget() {
        // The budget is for coverage computed from a screen-space derivative,
        // and a scene drawn from triangles must not receive it just for
        // containing the same shape. Aliased, the call tessellates.
        let aliased = Scene::new("aliased", vec![Item::fill(rounded(12.0), WHITE)]);
        // On the per-channel bound, which is what the analytic budget widens
        // and what this test is about. Not on the whole profile: a rounded
        // rectangle is curved, so it also carries the tie budget, and asserting
        // the profile would tie this test to a question it is not asking.
        assert_eq!(
            aliased.tolerance().per_channel,
            Tolerance::EXACT.per_channel
        );
        assert_ne!(
            aliased.tolerance().per_channel,
            Tolerance::ANALYTIC.per_channel,
            "a scene drawn from triangles was given the distance-field budget"
        );

        // Composited rather than replaced, which these have to say: the
        // constructors default to replacing, and a shape drawn on a quad
        // larger than itself cannot replace what is behind the gap.
        let antialiased = Scene::new(
            "antialiased",
            vec![Item::fill(rounded(12.0), WHITE).with_blend(BlendMode::SrcOver)],
        )
        .with_samples(4);
        assert_eq!(antialiased.tolerance(), Tolerance::ANALYTIC);

        // A stroke of the same shape is the same field narrowed to a band, so
        // it earns the same budget rather than the multisample one.
        let stroked = Scene::new(
            "stroked",
            vec![Item::stroke(rounded(12.0), StrokeSpec::new(4.0), WHITE)
                .with_blend(BlendMode::SrcOver)],
        )
        .with_samples(4);
        assert_eq!(stroked.tolerance(), Tolerance::ANALYTIC);

        // And replacing disqualifies it, because the canvas will tessellate it
        // instead -- so the budget has to describe the route actually taken.
        let replaced = Scene::new(
            "replaced",
            vec![Item::fill(rounded(12.0), WHITE).with_blend(BlendMode::Src)],
        )
        .with_samples(4);
        assert_eq!(replaced.tolerance(), Tolerance::MULTISAMPLED);

        // A radius of zero is a plain rectangle, with no distance field.
        let square = Scene::new("square", vec![Item::fill(rounded(0.0), WHITE)]).with_samples(4);
        assert_eq!(square.tolerance(), Tolerance::MULTISAMPLED);
    }
}

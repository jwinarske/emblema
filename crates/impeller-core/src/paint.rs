//! How a shape is drawn.

use crate::canvas::{Morphology, Rect};
use crate::color::Color;
use glam::Vec2;
use impeller_geometry::dash::Dash;
use impeller_geometry::stroke::StrokeStyle;
use impeller_geometry::transform::{transformed_bounds, unbounded, Transform2D};
use impeller_hal::{BlendMode, Extent2D, TileMode};
use impeller_hal::{ColorFilter, Sampling};

/// A color stop in a gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub color: Color,
    /// Position along the gradient, from zero to one.
    pub offset: f32,
}

impl GradientStop {
    pub fn new(color: Color, offset: f32) -> Self {
        Self { color, offset }
    }
}

/// What fills a shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Shader {
    Solid(Color),
    /// A gradient along the line between two points **in user space**.
    ///
    /// The endpoints travel through the canvas transform with the geometry, so
    /// a gradient rotates and scales with the shape it fills rather than
    /// staying pinned to the screen.
    LinearGradient {
        start: Vec2,
        end: Vec2,
        stops: Vec<GradientStop>,
        /// What fills the shape beyond the two endpoints.
        ///
        /// A gradient is defined by two points and a shape is rarely exactly
        /// that long, so this is not an edge case: clamping holds the end
        /// colors, repeating tiles the ramp, and decal leaves the outside
        /// empty. Set with [`Paint::with_tile_mode`], the same call an image
        /// uses.
        tile: TileMode,
    },
    /// A gradient outward from a center **in user space**, reaching its last
    /// stop at `radius`.
    RadialGradient {
        center: Vec2,
        radius: f32,
        stops: Vec<GradientStop>,
        /// What fills the shape beyond `radius`. See [`Shader::LinearGradient`].
        tile: TileMode,
    },
    /// A texture, mapped onto a rectangle **in user space**.
    ///
    /// The rectangle travels through the canvas transform with the geometry, so
    /// an image rotates and scales with the shape it fills. It is the region
    /// the image covers, not the region drawn: filling a circle with this paint
    /// draws a circular piece of the image.
    ///
    /// `slot` indexes the table supplied when the recording is drawn. The
    /// canvas names a slot rather than holding a texture because it records
    /// without touching a device, and a backend texture is not something it can
    /// name. Assigning slots is the caller's business for now; a registry that
    /// did it for them is a separate piece of design.
    Image {
        slot: u32,
        rect: Rect,
        /// Scales the sampled color, for drawing an image translucently.
        alpha: f32,
        tile: TileMode,
        /// Which part of the image to draw, from zero to one in each axis.
        ///
        /// The whole image by default. Set it with [`Paint::with_source`], or
        /// with [`Paint::with_source_pixels`] if you would rather state it in
        /// texels and hand over the size you uploaded.
        source: Rect,
        /// Multiplies the sampled color. White changes nothing.
        tint: Color,
        /// How to read between texels. See [`Paint::with_sampling`].
        sampling: Sampling,
    },
    /// A gradient around a center **in user space**, running between two angles
    /// in radians, measured counter-clockwise from the positive X axis.
    SweepGradient {
        center: Vec2,
        start_angle: f32,
        end_angle: f32,
        stops: Vec<GradientStop>,
        /// What fills the directions the arc does not cover.
        ///
        /// A sweep of a full turn covers every direction and this changes
        /// nothing; it is a partial sweep that has an outside.
        tile: TileMode,
    },
    /// A caller's own fragment program.
    ///
    /// The one shader here this renderer did not write. `program` names one
    /// registered with a context, and `uniforms` are the floats it reads --
    /// laid out as the program's own declaration expects, which is a contract
    /// between the caller's shader and the caller's code and not something
    /// this renderer can check.
    ///
    /// At most [`RUNTIME_FLOATS`] of them; the rest are dropped rather than
    /// overflowing into a block that has no room for them.
    ///
    /// [`RUNTIME_FLOATS`]: impeller_hal::RUNTIME_FLOATS
    RuntimeEffect {
        program: u32,
        uniforms: Vec<f32>,
        /// A texture slot the program may sample, if it declares one.
        ///
        /// The same table an image paint names, so a caller supplies them in
        /// the same array beside the recording, in the order the program
        /// declares its bindings.
        ///
        /// A binding the program declares and this leaves empty reads a
        /// one-pixel opaque white texture -- what a solid fill binds, and what
        /// keeps a pipeline's declared binding satisfied. Beyond
        /// [`MAX_EFFECT_TEXTURES`] a caller is asking for a second descriptor
        /// set layout, which would be one layout per program; the ceiling is
        /// what buys a single shared one.
        ///
        /// [`MAX_EFFECT_TEXTURES`]: impeller_hal::MAX_EFFECT_TEXTURES
        images: Vec<u32>,
    },
    /// A gradient between two circles **in user space**, reaching its first
    /// stop on the first circle and its last on the second.
    ///
    /// This is the general form: a radial gradient is the case where the first
    /// circle is a point at the second's center, and the two-point form is what
    /// `dart:ui` reaches by giving `Gradient.radial` a focal point. It is kept
    /// separate from [`Shader::RadialGradient`] rather than replacing it,
    /// because the radial case folds its radius into a matrix and solves no
    /// quadratic at all, and paying for the general solve on every ordinary
    /// gradient would be a poor trade on the hardware this targets.
    ///
    /// Where no circle in the family passes through a point, nothing is drawn
    /// there -- which is most of the plane when one circle is far outside the
    /// other. That is a property of the two circles rather than of the tile
    /// mode, which acts only where the parameter exists.
    ConicalGradient {
        start_center: Vec2,
        start_radius: f32,
        end_center: Vec2,
        end_radius: f32,
        stops: Vec<GradientStop>,
        /// What fills the parameter outside the two circles. See
        /// [`Shader::LinearGradient`].
        tile: TileMode,
    },
}

impl Shader {
    /// How many color stops this paint carries, or zero where it carries none.
    ///
    /// Public because the threshold is: at or below [`MAX_STOPS`] a gradient
    /// costs nothing beyond its material, and above it the recorder bakes a
    /// small texture. Neither is visible in the result, so this exists for a
    /// caller who cares about the cost rather than the picture.
    ///
    /// [`MAX_STOPS`]: impeller_hal::MAX_STOPS
    pub fn stop_count(&self) -> usize {
        match self {
            Self::Solid(_) | Self::Image { .. } | Self::RuntimeEffect { .. } => 0,
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. }
            | Self::ConicalGradient { stops, .. } => stops.len(),
        }
    }

    /// Whether this would draw anything at all.
    pub fn is_visible(&self) -> bool {
        match self {
            Self::Solid(color) => !color.is_invisible(),
            // What a caller's program draws is not knowable from here.
            Self::RuntimeEffect { .. } => true,
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. }
            | Self::ConicalGradient { stops, .. } => stops.iter().any(|s| !s.color.is_invisible()),
            // What the texture holds is unknown here, so only a zero alpha or
            // an empty destination makes an image provably invisible.
            Self::Image { alpha, rect, .. } => *alpha > 0.0 && !rect.is_empty(),
        }
    }
}

/// Which part of a blurred shape survives.
///
/// A mask blur produces two things a picture can be made from: the shape's own
/// coverage, and that coverage blurred. Every style here is one combination of
/// the two, and naming them is what `dart:ui` and Skia both do -- a shadow
/// wants the blur outside and nothing inside, a glow wants it inside, and a
/// soft-edged solid wants the shape intact with the blur around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaskBlurStyle {
    /// The blurred coverage, and nothing else. Soft on both sides of the edge.
    #[default]
    Normal,
    /// The shape at full strength, with the blur outside it.
    Solid,
    /// The blur outside the shape only. What a drop shadow behind an opaque
    /// object is: the part that would be hidden is not drawn at all.
    Outer,
    /// The blur inside the shape only, which is an inner shadow or a glow
    /// confined to what it lights.
    Inner,
}

/// A function applied to what a paint drew, rather than to the color it
/// computed.
///
/// The distinction from a color filter is what the input is. A color filter
/// sees one color at a time and cannot know its neighbors; this sees the
/// picture, which is what a blur needs.
///
/// The distinction from [`Paint::mask_blur`] is what is blurred. A mask blur
/// blurs coverage and then fills, which is only the same picture as blurring
/// the result when the fill does not vary -- so it takes a solid color and
/// refuses anything else. This blurs the result, which is defined for every
/// paint and is what `dart:ui` means by an image filter. For a solid color the
/// two agree, and there is a test that says so.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ImageFilter {
    #[default]
    None,
    /// Blur what was drawn, by a standard deviation per axis in the space the
    /// caller is drawing in.
    ///
    /// The caller's space rather than device pixels, which is what `dart:ui`
    /// means by a sigma and what the rest of this renderer does with one: a card
    /// lifting under a scale blurs by more as its content grows. `scaled_by`
    /// converts it when the layer opens. This doc said "device pixels" for as
    /// long as one path failed to convert it, which is the state
    /// `impeller-rs`'s `filter_space.rs` now rules out.
    ///
    /// Two, because `dart:ui`'s `ImageFilter.blur` takes `sigmaX` and
    /// `sigmaY` separately and upstream's `MakeBlur` passes both through. A
    /// blur is two separable passes here already, one along each axis, so the
    /// second deviation costs nothing but the field: each pass uses its own,
    /// and a pass whose deviation is zero is skipped rather than run as an
    /// identity that would resample for no reason.
    ///
    /// [`Self::blur`] is the isotropic case, which is nearly every use.
    Blur { sigma_x: f32, sigma_y: f32 },
    /// Move what was drawn, in device pixels, by resampling it.
    ///
    /// Not the same as drawing under the transform, which is what the canvas's
    /// own stack does. This draws at the size it was written and then moves
    /// the finished image, so magnifying gives enlarged pixels where the
    /// transform stack would give the shape redrawn larger. That difference is
    /// the reason `dart:ui` has both, and a caller wanting the sharp one
    /// already has `concat`.
    Matrix { transform: Transform2D },
    /// Spread what was drawn, taking the largest sample within these radii in
    /// device pixels.
    ///
    /// `ImageFilter.dilate`. On premultiplied color and channel by channel, so
    /// a translucent shape spreads its coverage along with its color.
    Dilate { radius_x: f32, radius_y: f32 },
    /// Shrink what was drawn, taking the smallest sample within these radii in
    /// device pixels.
    ///
    /// `ImageFilter.erode`, and the dual of [`Self::Dilate`]: what one does to
    /// a shape the other does to the space around it.
    Erode { radius_x: f32, radius_y: f32 },
    /// Recolor what was drawn, rather than what the paint computed.
    ///
    /// `dart:ui`'s `ColorFilter` is itself an `ImageFilter`, and upstream's
    /// `DlImageFilter::MakeColorFilter` is the same thing. A paint carries a
    /// color filter of its own, so the obvious question is what this adds, and
    /// the answer is narrower than it first looks: **it can be half of a
    /// [`Self::Compose`], and `Paint::color_filter` cannot.** Recoloring a
    /// blurred result, or blurring a recolored one, has no other spelling here.
    ///
    /// On a single draw with a solid fill it is the same picture as the paint's
    /// own filter, and a test pins that rather than leaving it to be discovered.
    /// A paint's image filter applies per draw -- each draw gets a layer of its
    /// own -- so there is no arrangement of two draws under one paint for the
    /// two to disagree about. Where they would disagree is inside a
    /// composition, which is the case this exists for.
    Color(ColorFilter),
    /// Run a caller's fragment program over what was drawn.
    ///
    /// `dart:ui`'s `ImageFilter.shader`, and upstream's
    /// `DlImageFilter::MakeRuntimeEffect`. The program reads its input as a
    /// texture with an identity transform -- the same guarantee upstream
    /// re-rasterizes to provide, and one this renderer has by construction: a
    /// filter is a pass whose target is the size of its source, covered by a
    /// quad, so a fragment's clip position *is* its texture coordinate. A
    /// program written for this samples exactly as the fixture programs do.
    ///
    /// The uniforms are the whole material block, because a program replaces
    /// the shader that would have read it. `RUNTIME_FLOATS` is the bound and
    /// anything past it is dropped, as it is for a runtime effect used as a
    /// paint.
    Runtime { program: u32, uniforms: Vec<f32> },
    /// Filter with `inner`, then filter that result with `outer`.
    ///
    /// `ImageFilter.compose`, and the reason this type is not `Copy`: a filter
    /// that holds filters cannot be a fixed size without a bound on the depth,
    /// and `dart:ui` puts no bound on it.
    ///
    /// A caller can already nest [`Canvas::save_layer`] calls to the same
    /// effect, and did before this existed. What this adds is that the bounds
    /// come out right without being worked out by hand: the outer layer has to
    /// be opened wide enough to hold whatever the inner one grew to, which
    /// means knowing how far each filter in the chain reaches.
    ///
    /// [`Canvas::save_layer`]: crate::Canvas::save_layer
    ///
    /// # How deep one may go
    ///
    /// Two hundred and fifty-six levels, and past that the calls taking a filter
    /// refuse it rather than crash. Everything that reads a composition walks it
    /// recursively, and the depth that can be walked is a property of the stack
    /// rather than of the picture -- a debug build managed two thousand and
    /// forty-eight and died at three thousand and seventy-two. Each level also
    /// costs a pass and a target, so the limit is far above any use.
    ///
    /// One thing no refusal here can cover: this is a chain of `Box`es, so
    /// *dropping* a composition recurses as deeply as it nests. A filter built deep
    /// enough overflows the stack being freed, in the caller's own code, before it
    /// is handed to anything. Building one is what has to be avoided, not passing
    /// it.
    Compose {
        outer: Box<ImageFilter>,
        inner: Box<ImageFilter>,
    },
}

/// How far a blur of the given deviations reaches, as an outset per target axis.
///
/// A function rather than a pair of numbers because the answer depends on which
/// directions the blur will run along, and those come from the transform in
/// force where the layer opened -- which `paint.rs` has no view of. See
/// `BlurBasis` in `canvas.rs`.
pub(crate) type BlurReach<'a> = &'a dyn Fn(Vec2) -> Vec2;

impl ImageFilter {
    /// A blur of the same deviation along both axes, which is nearly every use.
    pub fn blur(sigma: f32) -> Self {
        Self::Blur {
            sigma_x: sigma,
            sigma_y: sigma,
        }
    }

    /// A blur with a deviation per axis, as `dart:ui`'s `ImageFilter.blur`
    /// states one.
    pub fn blur_xy(sigma_x: f32, sigma_y: f32) -> Self {
        Self::Blur { sigma_x, sigma_y }
    }

    /// Whether a deviation describes a blur at all.
    ///
    /// Stated positively rather than as a negated comparison, which reads
    /// badly on a type where two values can be incomparable: a sigma that is
    /// not finite is not a blur, and neither is one that is zero or less.
    pub(crate) fn blurs(sigma: f32) -> bool {
        sigma.is_finite() && sigma > 0.0
    }

    /// This filter with its lengths converted from the caller's space to device
    /// pixels.
    ///
    /// A caller's sigma is stated in the space they were drawing in, which is
    /// what `dart:ui` means by it and what upstream honors by scaling with
    /// `effect_transform.Basis()`. `Layer::scaled_by` does this for the fields a
    /// `Copy` layer carries, and for a long time nothing did it for the filter
    /// handed alongside them -- so the same `ImageFilter::Blur` meant a local
    /// sigma through `Paint::with_image_filter`, which becomes a layer and is
    /// scaled, and a device sigma through `Canvas::save_layer_filtered`, which
    /// was not. One value in one type meaning two things depending on which call
    /// received it, which is the defect this closes rather than a difference
    /// between the two spellings.
    ///
    /// **The lengths scale and the rest does not**, which is not an oversight:
    ///
    /// - A blur's deviations and a morphology's radii are lengths in the
    ///   caller's space. The morphology pair used to be left out, on the reading
    ///   that its radius was device pixels here on purpose; that reading rested
    ///   on upstream having no morphology, which was wrong, and both now scale.
    ///   `Layer::scaled_by` does the same to the fields a `Copy` layer carries,
    ///   so the two spellings of a dilation agree.
    /// - A matrix is not a length. It moves a finished image, and the transform
    ///   is already in the space the image is in.
    /// - A color filter and a caller's program carry no lengths at all.
    ///
    /// Matched arm by arm with no wildcard, so a variant added later fails to
    /// compile here instead of silently not being converted. That is the whole
    /// reason this is written out rather than done with an `if let`.
    pub(crate) fn scaled_by(self, scale: f32) -> Self {
        if !scale.is_finite() || scale <= 0.0 {
            return self;
        }
        match self {
            Self::Blur { sigma_x, sigma_y } => Self::Blur {
                sigma_x: sigma_x * scale,
                sigma_y: sigma_y * scale,
            },
            Self::Dilate { radius_x, radius_y } => Self::Dilate {
                radius_x: radius_x * scale,
                radius_y: radius_y * scale,
            },
            Self::Erode { radius_x, radius_y } => Self::Erode {
                radius_x: radius_x * scale,
                radius_y: radius_y * scale,
            },
            Self::Compose { outer, inner } => Self::Compose {
                outer: Box::new(outer.scaled_by(scale)),
                inner: Box::new(inner.scaled_by(scale)),
            },
            Self::None | Self::Matrix { .. } | Self::Color(_) | Self::Runtime { .. } => self,
        }
    }

    /// Whether this would change anything.
    pub fn is_identity(&self) -> bool {
        match self {
            Self::None => true,
            // Stated positively rather than as a negated comparison, which
            // reads badly on a type where two values can be incomparable: a
            // sigma that is not finite is not a blur, and neither is one that
            // is zero or less.
            Self::Blur { sigma_x, sigma_y } => !Self::blurs(*sigma_x) && !Self::blurs(*sigma_y),
            // A matrix that changes nothing is one that costs a layer for
            // nothing, so it is worth recognizing.
            Self::Matrix { transform } => {
                !transform.is_finite() || *transform == Transform2D::IDENTITY
            }
            // Positive is a filter, however small, and this used to round first
            // on the reasoning that a radius of a third of a pixel names no
            // sample the filter could take. That reasoning died with the radius
            // becoming a length in the caller's space: a third of *what* is not
            // known here, and a local 0.2 under a scale of ten is two device
            // texels -- a filter this would have discarded. Whether a radius
            // rounds to nothing is decided in `Morphology::applied_radius`,
            // where the scale has been applied.
            Self::Dilate { radius_x, radius_y } | Self::Erode { radius_x, radius_y } => {
                let positive = |v: f32| v.is_finite() && v > 0.0;
                !positive(*radius_x) && !positive(*radius_y)
            }
            // Composing two filters that each change nothing changes nothing,
            // and costs two layers to say so.
            // A color filter that recolors nothing is one, which keeps a
            // caller who builds one from a default out of a layer they did not
            // ask to pay for. Asked of the filter rather than compared against
            // `ColorFilter::None`, which is what this used to do: a blend in
            // `Dst` mode returns the destination untouched and an identity
            // matrix recolors nothing, and both paid for a pass and a resample
            // to arrive at what they were handed. `ColorFilter::is_identity`
            // knows all three and is the same question.
            Self::Color(filter) => filter.is_identity(),
            // Never. What a caller's program does is the caller's business and
            // nothing here can read it, so the only safe answer is that it does
            // something -- and answering otherwise would drop the pass that
            // runs it.
            Self::Runtime { .. } => false,
            Self::Compose { outer, inner } => outer.is_identity() && inner.is_identity(),
        }
    }

    /// Filter with `inner` first and then with `outer`.
    ///
    /// The order `dart:ui` states, which is the order the names suggest once
    /// you picture them nested: the inner filter is the one closer to what was
    /// drawn. A composition where either half does nothing is the other half,
    /// since the layer it would have taken is a copy of an image.
    pub fn compose(outer: ImageFilter, inner: ImageFilter) -> Self {
        if inner.is_identity() {
            return outer;
        }
        if outer.is_identity() {
            return inner;
        }
        Self::Compose {
            outer: Box::new(outer),
            inner: Box::new(inner),
        }
    }

    /// The outermost filter that is not itself a composition, and everything
    /// that has to run before it.
    ///
    /// A chain is applied innermost first, so drawing one means peeling from
    /// the other end: open a layer for the outermost, and hand the remainder
    /// back to be drawn inside it. The remainder is a filter in its own right,
    /// which is what lets the caller recurse without knowing how deep it is.
    ///
    /// Recursive on the *outer* half, and that is the whole point. A first
    /// version took `outer` to be a leaf and returned it directly, which is
    /// true for anything `compose` builds left to right and false the moment a
    /// caller composes two compositions -- `compose(compose(a, b), c)` was
    /// refused as an unimplemented filter, having been assembled entirely out
    /// of implemented ones.
    /// The deepest composition this will walk.
    ///
    /// Every function that reads a composition walks it recursively -- `peel`,
    /// `covering`, `is_identity`, `scaled_by` -- and the passes it turns into are
    /// built by a recursion too. So the depth that can be serviced is a property of
    /// the stack rather than of the picture, and a limit stated here is a refusal
    /// where the absence of one was a fatal stack overflow.
    ///
    /// Measured on 2026-09-24 rather than guessed: a debug build serviced a
    /// composition two thousand and forty-eight deep and died at three thousand and
    /// seventy-two. A release build has smaller frames and would reach further,
    /// which is the wrong direction to calibrate from -- a limit has to hold in the
    /// profile that fails first, and the suite is that profile.
    ///
    /// Two hundred and fifty-six is far below it and far above any use. `dart:ui`
    /// callers compose two or three filters; each level here also costs a pass and
    /// a target, so a composition this deep is already a recording of two hundred
    /// and sixty passes over a picture nobody can see the shape of.
    pub(crate) const MAX_NESTING: usize = 256;

    /// Whether this composition nests deeper than `limit`.
    ///
    /// Iterative, with its own worklist, which is the point: a recursive depth
    /// check would overflow on exactly the input it exists to refuse. Stops as soon
    /// as the limit is passed rather than measuring the whole tree, so a
    /// pathological filter costs the limit and not its own size.
    ///
    /// A composition is a tree rather than a list -- both halves can compose -- so
    /// the walk carries a depth per node instead of counting iterations.
    pub(crate) fn nests_deeper_than(&self, limit: usize) -> bool {
        let mut work = vec![(self, 0usize)];
        while let Some((filter, depth)) = work.pop() {
            if depth > limit {
                return true;
            }
            if let Self::Compose { outer, inner } = filter {
                work.push((outer, depth + 1));
                work.push((inner, depth + 1));
            }
        }
        false
    }

    pub(crate) fn peel(&self) -> (ImageFilter, ImageFilter) {
        match self {
            Self::Compose { outer, inner } => {
                let (outermost, rest) = outer.peel();
                (outermost, Self::compose(rest, (**inner).clone()))
            }
            leaf => (leaf.clone(), Self::None),
        }
    }

    /// The region of device pixels a target must cover for this filter to run
    /// over content occupying `min`..`max`.
    ///
    /// Both what goes in and what comes out, which is not the same question for
    /// every filter and is the one that matters. A blur or a dilation only
    /// grows, so its output contains its input and the distinction is invisible
    /// -- the case that makes it visible is a matrix, which *moves* the image.
    /// A composition puts the inner filter's layer inside the outer one's
    /// target, and a layer's target is clipped to its parent's, so a target
    /// sized only for where a matrix filter put things would crop the content
    /// before the matrix ever ran: the inner layer draws the shape where it was
    /// written and the composite is what moves it.
    ///
    /// Erosion is left alone rather than shrunk. Covering more than the result
    /// needs costs a little memory; covering less loses drawing.
    pub(crate) fn covering(&self, min: Vec2, max: Vec2, blur: BlurReach) -> (Vec2, Vec2) {
        match self {
            // A color filter changes what a pixel is and never which pixels
            // there are, so it needs exactly the region it was given. A runtime
            // effect is the same on the side that matters here: it may read
            // anywhere in its input, and it writes only where the pass covers.
            Self::None | Self::Erode { .. } | Self::Color(_) | Self::Runtime { .. } => (min, max),
            Self::Blur { sigma_x, sigma_y } => {
                // Per axis, which is the whole reason a filter states two: a
                // blur along one axis alone must not widen the other, or the
                // target holding it is bigger than the picture in it.
                //
                // Which axes, though, is the caller's to say. A blur runs along
                // the directions the caller's own axes point once the transform
                // has been applied, so the outset is the bounding box of the two
                // reaches laid along those -- and asking here with the target's
                // axes when the blur will run along turned ones sizes a target
                // for one blur and fills it with another. That is not a
                // conservative error: it is short exactly where the blur went.
                let reach = blur(Vec2::new(*sigma_x, *sigma_y));
                (min - reach, max + reach)
            }
            Self::Dilate { radius_x, radius_y } => {
                let radius = Morphology::dilate(*radius_x, *radius_y).radius;
                let reach = Vec2::new(radius[0], radius[1]);
                (min - reach, max + reach)
            }
            Self::Matrix { transform } => {
                // A matrix that carries the content across the vanishing line
                // moves it further than a box can state, and this is a
                // statement about how far a filter reaches -- so it widens to
                // everything rather than reporting a reach that is too short.
                let (moved_min, moved_max) =
                    transformed_bounds(*transform, min, max).unwrap_or_else(unbounded);
                (min.min(moved_min), max.max(moved_max))
            }
            Self::Compose { outer, inner } => {
                let (min, max) = inner.covering(min, max, blur);
                outer.covering(min, max, blur)
            }
        }
    }
}

/// Fill the shape, or trace its outline.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Style {
    #[default]
    Fill,
    /// Stroke with the given width and joinery.
    ///
    /// The width is in the coordinate space the shape is drawn in, so it scales
    /// with the canvas transform. A caller wanting a hairline that stays one
    /// pixel wide under zoom divides by the current scale.
    Stroke(StrokeStyle),
}

/// Everything about how a shape is painted.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub shader: Shader,
    pub style: Style,
    /// Which part of the blurred shape survives. Ignored without a mask blur.
    pub mask_blur_style: MaskBlurStyle,
    /// A function applied to what this paint drew, after the shader and any
    /// color filter.
    ///
    /// Costs a layer: what is drawn goes into a target of its own, is
    /// filtered, and is composited back. See [`ImageFilter`] for how this
    /// differs from a color filter and from a mask blur.
    pub image_filter: ImageFilter,
    /// A function applied to the color the shader produces, before blending.
    ///
    /// Beside the shader rather than inside it because it applies to all of
    /// them, and after it rather than before because that is where `dart:ui`
    /// puts a color filter and where it is useful: what is being recolored is
    /// the result of the fill, not the inputs it was built from.
    pub color_filter: ColorFilter,
    /// Cut a stroke into a dash pattern before drawing it.
    ///
    /// On the paint rather than inside [`Style::Stroke`] for two reasons. It
    /// keeps the geometry crate's stroke description a small copyable value,
    /// which the tessellator wants and a pattern of arbitrary length would
    /// end. And it says what it is: a dashed stroke is a stroke of a different
    /// path, so this describes what happens to the path on the way in rather
    /// than how the outline is built.
    ///
    /// Ignored by a fill, which has no length to measure along.
    pub dash: Option<Dash>,
    /// Blur the shape's coverage before filling it, in device pixels.
    ///
    /// What a soft shadow is made of: the same shape, softened, drawn behind
    /// the thing casting it. Zero for none.
    ///
    /// `dart:ui` states this as blurring the mask and then applying the paint
    /// through it, and that is what a shape gets. Two orders exist and they
    /// agree only where the fill does not vary, because a blur is linear:
    /// `blur(C·α)` is `C·blur(α)` for a constant `C` and for nothing else. A
    /// solid color therefore takes the cheaper route and is drawn through a
    /// blurred layer, which is that identity used; anything that varies is
    /// drawn the way the specification says, with the fill laid across
    /// everything the blur reaches and blurred coverage composited onto it.
    ///
    /// Two things still take a solid color only, and neither is about blurring.
    /// A glyph run tints one color by its own nature, so a gradient over one is
    /// refused here exactly as it is when nothing is blurred. And a mesh
    /// carries a color per vertex, which is a second thing multiplying the
    /// coverage rather than a fill to lay down.
    ///
    /// The three styles that combine a blurred mask with a sharp one need the
    /// coverage twice and are not built for a varying fill; they take a solid
    /// color, which is every use a shadow has.
    pub mask_blur: f32,
    pub blend: BlendMode,
    /// How a color carried per vertex or per sprite combines with what this
    /// paint produced.
    ///
    /// `dart:ui` passes this as an argument to `drawVertices` and `drawAtlas`
    /// rather than on the paint. It sits here instead because those are the
    /// only two calls that can carry such a color, and an argument every other
    /// call would have to pass and ignore is worse than a field they leave
    /// alone. [`BlendMode::Modulate`] multiplies, which is the identity for the
    /// white a caller who supplied no color gets.
    ///
    /// Distinct from [`Self::blend`], which decides how the result reaches the
    /// target. Both colors here are in the shader already, so every mode is
    /// available to this one without an extension.
    pub tint_blend: BlendMode,
    /// Whether to antialias this shape's edges.
    ///
    /// Recorded per paint but applied per pass, because multisampling is a
    /// property of the target rather than of a draw: a canvas that mixes the
    /// two antialiases everything.
    pub anti_alias: bool,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            shader: Shader::Solid(Color::BLACK),
            style: Style::Fill,
            image_filter: ImageFilter::None,
            mask_blur_style: MaskBlurStyle::default(),
            color_filter: ColorFilter::None,
            dash: None,
            mask_blur: 0.0,
            blend: BlendMode::SrcOver,
            tint_blend: BlendMode::Modulate,
            anti_alias: true,
        }
    }
}

impl Paint {
    /// A solid fill.
    pub fn fill(color: Color) -> Self {
        Self {
            shader: Shader::Solid(color),
            ..Default::default()
        }
    }

    /// A fill that runs between colors along a line in user space.
    ///
    /// Any number of them. Up to [`MAX_STOPS`] the colors travel with the
    /// material; beyond that the recorder tabulates them into a texture the
    /// shader samples, which costs one small upload per distinct gradient and
    /// is otherwise invisible. Ask [`Shader::stop_count`] if you want to know
    /// which will happen.
    ///
    /// [`MAX_STOPS`]: impeller_hal::MAX_STOPS
    pub fn linear_gradient(start: Vec2, end: Vec2, stops: Vec<GradientStop>) -> Self {
        Self {
            shader: Shader::LinearGradient {
                start,
                end,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// A fill that runs outward from a center in user space.
    ///
    /// Any number of stops; see [`Paint::linear_gradient`].
    pub fn radial_gradient(center: Vec2, radius: f32, stops: Vec<GradientStop>) -> Self {
        Self {
            shader: Shader::RadialGradient {
                center,
                radius,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// A fill that runs around a center in user space.
    ///
    /// Angles are in radians, counter-clockwise from the positive X axis. At
    /// any number of stops; see [`Paint::linear_gradient`].
    pub fn sweep_gradient(
        center: Vec2,
        start_angle: f32,
        end_angle: f32,
        stops: Vec<GradientStop>,
    ) -> Self {
        Self {
            shader: Shader::SweepGradient {
                center,
                start_angle,
                end_angle,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// A fill computed by a program the caller supplied.
    ///
    /// See [`Shader::RuntimeEffect`]. The program has to be registered with
    /// the context that will draw the recording, and the index it was given is
    /// what goes here.
    pub fn runtime_effect(program: u32, uniforms: Vec<f32>) -> Self {
        Self {
            shader: Shader::RuntimeEffect {
                program,
                uniforms,
                images: Vec::new(),
            },
            ..Default::default()
        }
    }

    /// Give a runtime effect a texture to sample.
    ///
    /// Ignored by every other kind of paint, which either samples nothing or
    /// already names what it samples. Called more than once, the slots stack up
    /// in the order the program declares its bindings.
    pub fn with_effect_image(mut self, slot: u32) -> Self {
        if let Shader::RuntimeEffect { images, .. } = &mut self.shader {
            images.push(slot);
        }
        self
    }

    /// Give a runtime effect every texture it samples at once.
    ///
    /// The order is the order the program declares its bindings, which for a
    /// program written in WGSL is the order of the binding numbers rather than
    /// the order of the declarations.
    pub fn with_effect_images(mut self, slots: &[u32]) -> Self {
        if let Shader::RuntimeEffect { images, .. } = &mut self.shader {
            images.clear();
            images.extend_from_slice(slots);
        }
        self
    }

    /// A fill that runs between two circles in user space.
    ///
    /// The general gradient: the first stop lands on the first circle and the
    /// last on the second, and every stop between them on a circle
    /// interpolated between the two. `radial_gradient` is the case where the
    /// first circle is a point at the second's center, and is worth reaching
    /// for when that is what you mean -- it does less work per fragment.
    ///
    /// Any number of stops; see [`Paint::linear_gradient`].
    pub fn conical_gradient(
        start_center: Vec2,
        start_radius: f32,
        end_center: Vec2,
        end_radius: f32,
        stops: Vec<GradientStop>,
    ) -> Self {
        Self {
            shader: Shader::ConicalGradient {
                start_center,
                start_radius,
                end_center,
                end_radius,
                stops,
                tile: TileMode::default(),
            },
            ..Default::default()
        }
    }

    /// How to read a texture between its texels.
    ///
    /// Linear by default, which is what an image drawn at any size but its own
    /// wants. [`Sampling::Nearest`] is for the cases where blending is the
    /// wrong answer: pixel art, and a sprite drawn at exactly its own size
    /// where certainty that no neighbor bled in matters more than smoothness.
    ///
    /// Ignored by a paint with no texture.
    pub fn with_sampling(mut self, sampling: Sampling) -> Self {
        if let Shader::Image { sampling: at, .. } = &mut self.shader {
            *at = sampling;
        }
        self
    }

    /// Keep only part of the blurred shape. See [`MaskBlurStyle`].
    ///
    /// Ignored unless a mask blur is set, since there is nothing to take a
    /// part of otherwise.
    pub fn with_mask_blur_style(mut self, style: MaskBlurStyle) -> Self {
        self.mask_blur_style = style;
        self
    }

    /// Filter what this paint draws, rather than the color it computes.
    ///
    /// Unlike a mask blur this takes any shader, because blurring a result is
    /// defined whatever produced it. It is the more expensive of the two: a
    /// layer is allocated, drawn into and composited back, where a mask blur
    /// of a solid color reaches the same picture the same way but is only
    /// offered where that equivalence holds.
    pub fn with_image_filter(mut self, filter: ImageFilter) -> Self {
        self.image_filter = filter;
        self
    }

    /// Apply a function to the color this paint produces, before it blends.
    ///
    /// Works on any shader, which is the point: the image tint that came
    /// before it could only recolor an image. A gradient, a shape, a run of
    /// glyphs all take one now.
    pub fn with_color_filter(mut self, filter: ColorFilter) -> Self {
        self.color_filter = filter;
        self
    }

    /// Blur the shape's coverage, for a shadow or a glow.
    ///
    /// Ignored unless finite and positive. Applies to a fill or a stroke alike
    /// -- what is blurred is whatever coverage the shape produces.
    pub fn with_mask_blur(mut self, sigma: f32) -> Self {
        self.mask_blur = if sigma.is_finite() && sigma > 0.0 {
            sigma
        } else {
            0.0
        };
        self
    }

    /// Dash this paint's stroke, or clear an existing pattern with `None`.
    ///
    /// The lengths are in the space the shape is drawn in, so a pattern scales
    /// with the canvas transform along with the line it cuts. A pattern that
    /// cannot be walked -- empty, negative, or summing to zero -- draws the
    /// line whole rather than drawing nothing, since an undashed line leads
    /// back to the pattern and an absent one leads nowhere.
    pub fn with_dash(mut self, dash: impl Into<Option<Dash>>) -> Self {
        self.dash = dash.into();
        self
    }

    /// A stroke of the given width, with default caps and joins.
    /// A fill that samples an image across a rectangle in user space.
    ///
    /// `slot` indexes the table supplied when the recording is drawn, and
    /// `rect` is the region the image covers rather than the region drawn: the
    /// shape being filled decides what is painted, this decides where the image
    /// sits under it.
    pub fn image(slot: u32, rect: Rect) -> Self {
        Self {
            shader: Shader::Image {
                slot,
                rect,
                alpha: 1.0,
                tile: TileMode::default(),
                source: Rect::new(0.0, 0.0, 1.0, 1.0),
                tint: Color::WHITE,
                sampling: Sampling::default(),
            },
            ..Default::default()
        }
    }

    /// What happens outside the paint's own extent.
    ///
    /// Means the same thing for a gradient as for an image, which is why it is
    /// one call rather than two: past the end of the ramp, past the radius, or
    /// outside the swept arc, the color either holds, or repeats, or stops.
    /// Ignored by a solid paint, which has no outside.
    pub fn with_tile_mode(mut self, tile: TileMode) -> Self {
        match &mut self.shader {
            Shader::Image { tile: current, .. }
            | Shader::LinearGradient { tile: current, .. }
            | Shader::RadialGradient { tile: current, .. }
            | Shader::SweepGradient { tile: current, .. }
            | Shader::ConicalGradient { tile: current, .. } => *current = tile,
            // Neither has an outside a tile mode could describe: a solid has
            // no edge, and where a caller's program stops is the caller's
            // arithmetic rather than this renderer's.
            Shader::Solid(_) | Shader::RuntimeEffect { .. } => {}
        }
        self
    }

    /// Draw only part of the image, in coordinates from zero to one.
    ///
    /// What a sprite sheet needs, and a nine-patch border. Normalized rather
    /// than in texels because a canvas records without touching a device and
    /// has never seen the image: it holds a slot, not a texture, and cannot ask
    /// how large it is. The caller who uploaded it can, which is what
    /// [`Paint::with_source_pixels`] is for.
    ///
    /// Ignored by paints that are not images.
    pub fn with_source(mut self, source: Rect) -> Self {
        if let Shader::Image { source: at, .. } = &mut self.shader {
            *at = source;
        }
        self
    }

    /// The same, stated in texels of an image of the given size.
    ///
    /// A degenerate size would divide by zero, so it leaves the paint alone --
    /// drawing the whole image, which is what the paint already said.
    pub fn with_source_pixels(self, source: Rect, size: Extent2D) -> Self {
        if size.width == 0 || size.height == 0 {
            return self;
        }
        let (w, h) = (size.width as f32, size.height as f32);
        self.with_source(Rect::new(
            source.left / w,
            source.top / h,
            source.right / w,
            source.bottom / h,
        ))
    }

    /// Multiply an image paint's sampled color. Ignored by other paints.
    ///
    /// What turns one monochrome icon sheet into every state a control has:
    /// upload the shapes once as white on transparent, and tint per draw. A
    /// white tint is the identity, which is what an image that never mentions
    /// one already carries.
    ///
    /// Composes with everything else on the paint, so a tinted sprite from a
    /// sheet is one call for the piece and one for the color.
    pub fn with_tint(mut self, tint: Color) -> Self {
        if let Shader::Image { tint: at, .. } = &mut self.shader {
            *at = tint;
        }
        self
    }

    /// Scale an image paint's sampled color. Ignored by other paints.
    pub fn with_image_alpha(mut self, alpha: f32) -> Self {
        if let Shader::Image { alpha: current, .. } = &mut self.shader {
            *current = alpha;
        }
        self
    }

    pub fn stroke(color: Color, width: f32) -> Self {
        Self {
            shader: Shader::Solid(color),
            style: Style::Stroke(StrokeStyle::new(width)),
            ..Default::default()
        }
    }

    pub fn with_shader(mut self, shader: Shader) -> Self {
        self.shader = shader;
        self
    }

    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    /// How a per-vertex or per-sprite color combines with this paint's result.
    ///
    /// Ignored by every call that carries no such color, there being nothing to
    /// combine.
    pub fn with_tint_blend(mut self, tint_blend: BlendMode) -> Self {
        self.tint_blend = tint_blend;
        self
    }

    /// Whether this paint's edges are smoothed. On by default.
    ///
    /// How that is paid for depends on the shape rather than on this flag. A
    /// rectangle, rounded rectangle, oval or circle is drawn from a distance
    /// field and antialiases itself inside its own fragment shader, costing a
    /// pass nothing. Anything that reaches the tessellator instead -- an
    /// arbitrary path, a stroke, a line, a double rounded rectangle -- has only
    /// triangles to work with, so its pass becomes multisampled.
    ///
    /// That second case carries a requirement worth knowing before meeting it:
    /// a multisampled pass must clear, so the pass it lands in needs something
    /// to clear to. Drawn straight onto the frame, that means the canvas needs
    /// a background -- see [`crate::Canvas::clear`], which is where a caller
    /// refused for this reason should look. Drawn inside a layer it means
    /// nothing to the caller, the layer's target clearing to transparent on
    /// its own.
    pub fn with_anti_alias(mut self, anti_alias: bool) -> Self {
        self.anti_alias = anti_alias;
        self
    }

    /// Whether drawing with this paint would change anything.
    ///
    /// A transparent fill and a zero-width stroke both draw nothing, and
    /// skipping them early keeps empty geometry out of the batch rather than
    /// tessellating it and discovering it was empty.
    pub fn is_visible(&self) -> bool {
        if !self.shader.is_visible() {
            return false;
        }
        match &self.style {
            Style::Fill => true,
            // `can_draw` rather than `is_visible`, and the difference is a
            // width of zero: a hairline, which the canvas widens to a device
            // pixel before any geometry is built for it.
            Style::Stroke(stroke) => stroke.can_draw(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_paint_is_an_opaque_antialiased_fill() {
        let paint = Paint::default();
        assert_eq!(paint.style, Style::Fill);
        assert_eq!(paint.blend, BlendMode::SrcOver);
        assert!(paint.anti_alias);
        assert!(paint.is_visible());
    }

    #[test]
    fn a_transparent_paint_draws_nothing_whatever_its_style() {
        let clear = Color::WHITE.with_alpha(0.0);
        assert!(!Paint::fill(clear).is_visible());
        assert!(!Paint::stroke(clear, 4.0).is_visible());

        // A gradient every stop of which is transparent draws nothing either.
        let invisible = Paint::linear_gradient(
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            vec![GradientStop::new(clear, 0.0), GradientStop::new(clear, 1.0)],
        );
        assert!(!invisible.is_visible());
    }

    #[test]
    fn a_zero_width_stroke_draws_nothing() {
        // Animating a width to zero should stop drawing rather than emit
        // degenerate geometry for the tessellator to discard.
        // A width of zero is a hairline rather than nothing, so the paint is
        // visible; a negative one still is not a stroke.
        assert!(Paint::stroke(Color::BLACK, 0.0).is_visible());
        assert!(!Paint::stroke(Color::BLACK, -1.0).is_visible());
        assert!(Paint::stroke(Color::BLACK, 0.5).is_visible());
    }
}

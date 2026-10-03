//! What fills a shape.
//!
//! A material is the paint as a backend sees it: fully resolved, in clip space,
//! and packed into the layout the shader expects. Resolving happens above,
//! because gradient geometry has to travel through the same transform the shape
//! did.

use crate::blend::BlendMode;

/// Floats in the packed representation.
///
/// This was 128 bytes for a long time because that is exactly what every
/// device guarantees as push constants, and the material traveled as one.
/// That guarantee was also a ceiling, and the material had grown to occupy it
/// exactly -- so the mechanism was deciding what a paint could hold, which is
/// the wrong way round. Materials travel in a uniform buffer now, and the
/// limit that binds is `maxUniformBufferRange`, whose guaranteed minimum is
/// sixteen kilobytes: two orders of magnitude of room rather than none.
///
/// The size has not changed with the mechanism, and should not change without
/// a material that needs it. Every float here is read by a shader that
/// branches on the kind, and floats nobody reads are bandwidth in the one
/// place a renderer spends it per draw.
pub const MATERIAL_FLOATS: usize = 64;

/// Enforced at compile time rather than by a test, so a material that outgrew
/// what every device guarantees could not be built at all.
///
/// The bound is `maxUniformBufferRange`'s guaranteed minimum. A material is
/// nowhere near it; the assertion is here because the previous bound was
/// reached, and the way that was noticed was this line failing to compile.
const _: () = assert!(
    MATERIAL_FLOATS * 4 <= 16384,
    "a material must fit the 16 KiB of uniform buffer range every device guarantees"
);

/// The most stops a gradient carries in the material itself.
///
/// Four covers the overwhelming majority of real gradients, and covers them
/// without a texture, an upload or a binding. It is not a limit on what can be
/// drawn: beyond this the recorder tabulates the stops into a ramp and the
/// shader samples it instead, which is what this constant used to say was
/// waiting on the HAL learning to sample textures. It has since learned.
pub const MAX_STOPS: usize = 4;

/// Offsets into the packed layout, matching the shader's declaration.
///
/// Public because it is a contract between the shader and every backend, not an
/// internal detail. Both backends copy the packed floats straight into a
/// uniform buffer, so a member is found by its offset here rather than by a
/// name -- and naming the offsets in one place keeps the layout from being
/// written out again as bare indices that quietly go stale when it grows.
pub mod layout {
    /// Four stop colors.
    pub const STOPS: usize = 0;
    /// Stop positions.
    pub const OFFSETS: usize = 16;
    /// Endpoints, or center plus angles.
    pub const GEOMETRY: usize = 20;
    /// Clip space to the paint's own space: three columns of a three-by-three,
    /// in column order, each padded to four floats.
    ///
    /// Twelve floats rather than nine because that is how a `mat3x3` sits in a
    /// uniform block, and because every member here is a four-component vector
    /// on purpose -- it is what lets both backends copy the packed material
    /// straight in without writing padding around anything.
    ///
    /// The paint's origin is inside this matrix rather than beside it in
    /// `GEOMETRY`, which is why the first two floats there are unclaimed for
    /// every kind that carries a mapping. See `invert_to_local`.
    pub const TO_LOCAL: usize = 24;
    /// Stop count, material kind, and two floats whose meaning the kind
    /// decides -- a corner radius, a stroke width, a blur's deviation, a
    /// gradient's tile mode, a conical gradient's separation.
    ///
    /// A zero stop count means the colors are in a ramp texture; see
    /// `stop_count_code`.
    pub const PARAMS: usize = 36;
    /// A color filter's matrix, by column.
    pub const FILTER: usize = 40;
    /// The constant a color filter adds.
    pub const FILTER_OFFSET: usize = 56;
    /// Which color filter, if any, and in which form its matrix is stated.
    ///
    /// The second float holds the tint blend, and the last two hold the dither;
    /// see [`DITHER`]. Four unrelated things share a slot because the slot is a
    /// four-component vector whether or not anything fills it, and a fifth
    /// vector would cost every draw sixteen bytes to carry three unused floats.
    pub const FILTER_PARAMS: usize = 60;
    /// The dither amplitude, and whether the target encodes on write.
    ///
    /// Amplitude first, in the target's storage units, with zero meaning no
    /// dithering; then a flag saying whether a step of that size is a step of
    /// encoded value rather than of light. Both are written by the backend at
    /// submission rather than by the recorder, because the target's format is
    /// what decides them and a recording is made without one -- which is also
    /// what keeps the color policy intact. The shader is handed two numbers and
    /// still knows nothing about formats.
    pub const DITHER: usize = 62;
}

/// The number the shader reads for a tile mode.
///
/// Written once rather than at each call site: an image and a gradient must
/// agree about what `1.0` means, and two copies of a mapping eventually do not.
/// The stop count the shader reads, which doubles as the ramp flag.
///
/// Zero means the colors are in a texture rather than in this material, and
/// the shader samples them instead of walking the stops. A sentinel rather
/// than a flag of its own because the two are mutually exclusive by
/// construction -- a ramp exists only when the stops outnumbered what fits, so
/// a material with a ramp has no count worth reporting -- and because the float
/// this frees is the one a conical gradient needs for its second center. That
/// is the whole reason the fourth parameter slot was available to it.
fn stop_count_code(count: usize, ramp: &Option<u32>) -> f32 {
    match ramp {
        Some(_) => 0.0,
        None => count.max(1) as f32,
    }
}

fn tile_code(tile: TileMode) -> f32 {
    match tile {
        TileMode::Clamp => tile::CLAMP,
        TileMode::Repeat => tile::REPEAT,
        TileMode::Decal => tile::DECAL,
        TileMode::Mirror => tile::MIRROR,
    }
}

/// Kind selector shared with the shader.
pub mod kind {
    pub const SOLID: f32 = 0.0;
    pub const LINEAR: f32 = 1.0;
    pub const RADIAL: f32 = 2.0;
    pub const SWEEP: f32 = 3.0;
    pub const IMAGE: f32 = 4.0;
    pub const GLYPH: f32 = 5.0;
    pub const BLUR: f32 = 6.0;
    pub const ROUNDED_RECT: f32 = 7.0;
    pub const ELLIPSE: f32 = 8.0;
    pub const CONICAL: f32 = 9.0;
    pub const MESH: f32 = 10.0;
    pub const MORPHOLOGY: f32 = 11.0;
    pub const ROUNDED_RECT_BLUR: f32 = 12.0;
    pub const POINT_FIELD: f32 = 13.0;
}

/// How many textures one runtime program may sample.
///
/// A fixed ceiling because the descriptor set layout is shared: every pipeline
/// this renderer builds is built against one layout, so the bindings it
/// declares are the same for a solid fill and for a caller's program. Raising
/// this costs an image binding on every draw; removing the ceiling costs a
/// layout per program.
///
/// Four covers what `dart:ui` shaders ask for in practice -- an image and a
/// mask, an image and a gradient ramp, occasionally a third.
pub const MAX_EFFECT_TEXTURES: usize = 4;

/// How far one morphology pass reaches, in texels each way.
///
/// The shader's loop has to be bounded, and this is the bound. It is not a
/// limit on the filter: a larger radius becomes more passes, since dilating
/// twice dilates by the sum. Sixty-five samples per pixel per pass is already
/// enough bandwidth that splitting is the cheaper answer anyway.
pub const MORPHOLOGY_TAPS: u32 = 32;

/// How many floats a runtime effect may take.
///
/// The whole material block, because an effect replaces the shader that would
/// have read it as a material. A caller writing an effect declares the same
/// std140 block and reads these as their own.
pub const RUNTIME_FLOATS: usize = MATERIAL_FLOATS;

/// How a texture is read between its texels.
///
/// Not two samplers. The same argument that keeps tile modes in the shader
/// keeps this there: a sampler baked with a filter would mean one sampler per
/// combination and a descriptor set per draw that used a different one. A
/// linear sampler read exactly at a texel's center returns that texel and
/// nothing else, so nearest sampling is the coordinate snapped to the nearest
/// center before the read, which is one multiply-floor-divide and no bindings
/// at all.
///
/// `dart:ui` offers four qualities. These are its first two; the other two are
/// mipmapped and bicubic, and neither exists here to select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sampling {
    /// Blend the texels around the coordinate. The right default: an image
    /// drawn at any size but its own is otherwise a mess of hard edges.
    #[default]
    Linear,
    /// The one texel the coordinate falls in.
    ///
    /// What pixel art needs, and what a sprite drawn at exactly its own size
    /// wants in order to be certain no neighbor bled in.
    Nearest,
    /// A bicubic reconstruction over the sixteen texels around the coordinate.
    ///
    /// What `dart:ui` calls `FilterQuality.high`. Sharper than linear under
    /// magnification, because the curve through four texels along an axis has
    /// a slope where a straight line between two has a corner, and it costs
    /// sixteen reads per fragment to say so.
    ///
    /// The particular curve is Mitchell-Netravali with `B` and `C` both a
    /// third, which is what Skia's high quality has always meant and so what a
    /// caller porting from Flutter is expecting. It rings slightly -- the
    /// weights go a little negative between one and two texels out -- and that
    /// overshoot is the sharpening, not an error in it.
    Cubic,
    /// A linear read of the mip level that matches how far the image is being
    /// minified, blended with the level either side of it.
    ///
    /// What `dart:ui` calls `FilterQuality.medium`, and the one quality that
    /// answers minification rather than magnification. Drawn at half its size
    /// an image read linearly skips every other texel and aliases; read from
    /// the level built for that size, every texel of the original contributes.
    ///
    /// It needs a texture that was allocated with a chain, since a level that
    /// was never made cannot be read. On a texture without one every read lands
    /// on the image itself and this is linear -- which is a picture that is
    /// merely worse rather than wrong, and is what a sampler does with a level
    /// it does not have.
    Mipmap,
}

/// The number the shader reads for a sampling mode.
fn sampling_code(sampling: Sampling) -> f32 {
    match sampling {
        Sampling::Linear => sampling::LINEAR,
        Sampling::Nearest => sampling::NEAREST,
        Sampling::Cubic => sampling::CUBIC,
        Sampling::Mipmap => sampling::MIPMAP,
    }
}

/// Sampling selector shared with the shader.
pub mod sampling {
    pub const LINEAR: f32 = 0.0;
    pub const NEAREST: f32 = 1.0;
    pub const CUBIC: f32 = 2.0;
    pub const MIPMAP: f32 = 3.0;
}

/// Tile mode selector shared with the shader.
pub mod tile {
    pub const CLAMP: f32 = 0.0;
    pub const REPEAT: f32 = 1.0;
    pub const DECAL: f32 = 2.0;
    pub const MIRROR: f32 = 3.0;
}

/// What space a color filter's matrix expects its input in.
///
/// A `dart:ui` color matrix is defined on straight color, which is the form a
/// person writes a saturation or a sepia matrix in. A blend against a constant
/// color is defined on premultiplied color, which is the form the compositing
/// rules are stated in. Both are affine, both are one matrix here, and this is
/// which one -- because the shader works in premultiplied color and has to know
/// whether to undo that before applying the matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorForm {
    /// Premultiplied, which is what the shader already has.
    #[default]
    Premultiplied,
    /// Straight, which the shader divides out first and restores after.
    Straight,
}

/// A function applied to a material's color, after the material and before the
/// blend.
///
/// Only affine functions, which is less of a restriction than it sounds: a
/// `dart:ui` color matrix is affine by definition, and every separable
/// Porter-Duff blend against a *constant* color is affine in the other operand.
/// So one matrix in the shader serves both, and which blend modes are offered
/// is decided on the processor rather than in the fragment.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ColorFilter {
    #[default]
    None,
    /// `columns[j]` scales the input's `j`th channel, and `offset` is added.
    ///
    /// Stored by column rather than by row -- the transpose of how `dart:ui`
    /// writes it -- because that is what makes the shader four multiply-adds
    /// of vectors rather than four dot products, and because a column is a
    /// `vec4` where a row of the five-wide form is not.
    Matrix {
        columns: [[f32; 4]; 4],
        offset: [f32; 4],
        form: ColorForm,
    },
    /// Blend a constant color against the material's own, in a mode a matrix
    /// cannot state.
    ///
    /// `dart:ui`'s `ColorFilter.mode` takes any blend mode, and most of them
    /// are affine in the destination once the source is fixed -- those become
    /// a [`Self::Matrix`] and cost the shader nothing beyond the multiply it
    /// was already doing. The advanced modes are not affine: they are
    /// piecewise, or they exchange components between channels. This is where
    /// they go.
    ///
    /// It is arithmetic against a constant rather than against the frame, so
    /// it needs no framebuffer fetch and no extension -- unlike the same mode
    /// set on the paint, which reaches the hardware's blending unit and is
    /// gated on one. The shader evaluates it with the same function a mesh's
    /// per-vertex tint uses.
    ///
    /// `color` is premultiplied, as [`Self::blend`] leaves it and as the
    /// specification states its operands.
    Blend { color: [f32; 4], mode: BlendMode },
    /// The sRGB transfer function, one direction or the other.
    ///
    /// The one filter `dart:ui` offers that a matrix cannot express: the curve
    /// is piecewise, and the piece that covers almost all of the range has an
    /// exponent in it. Approximating it as a power of 2.2 -- which is the usual
    /// shortcut -- is wrong by about a percent in the midtones and much more
    /// near black, where the linear segment is doing the work, so the shader
    /// evaluates the real thing instead.
    ///
    /// Applied to straight color, per channel, leaving alpha alone. Gamma on a
    /// premultiplied channel would be encoding the alpha along with the color.
    Gamma { direction: Gamma },
}

/// Which way through the sRGB transfer function a [`ColorFilter::Gamma`] goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gamma {
    /// Linear light in, sRGB's encoding out. `ColorFilter.linearToSrgbGamma`.
    LinearToSrgb,
    /// sRGB's encoding in, linear light out. `ColorFilter.srgbToLinearGamma`.
    SrgbToLinear,
}

impl ColorFilter {
    /// A filter from the twenty numbers `dart:ui` states one in.
    ///
    /// Row-major and five wide: four scales and a constant per output channel,
    /// red row first, applied to straight color with each channel from zero to
    /// one. The constants are in the same units, so a matrix written against
    /// the 0-255 convention has to have its last column divided by 255 first.
    pub fn matrix(rows: [f32; 20]) -> Self {
        let mut columns = [[0.0f32; 4]; 4];
        let mut offset = [0.0f32; 4];
        for (out, row) in rows.chunks_exact(5).enumerate() {
            for (channel, value) in row[..4].iter().enumerate() {
                columns[channel][out] = *value;
            }
            offset[out] = row[4];
        }
        Self::Matrix {
            columns,
            offset,
            form: ColorForm::Straight,
        }
    }

    /// Linear light encoded into sRGB, which is `ColorFilter.linearToSrgbGamma`.
    pub fn linear_to_srgb() -> Self {
        Self::Gamma {
            direction: Gamma::LinearToSrgb,
        }
    }

    /// sRGB decoded back to linear light, which is
    /// `ColorFilter.srgbToLinearGamma`.
    pub fn srgb_to_linear() -> Self {
        Self::Gamma {
            direction: Gamma::SrgbToLinear,
        }
    }

    /// A filter that blends a constant color against the material's own.
    ///
    /// The material is the destination and `color` is the source, which is the
    /// way round `dart:ui` states `ColorFilter.mode` and the way round that
    /// makes an icon sheet tinted by `SrcIn` mean what everyone expects.
    ///
    /// Most modes are affine in the destination once the source is fixed, so
    /// each becomes a matrix and the shader needs no blending arithmetic at
    /// all. The advanced modes are not affine -- they are piecewise, or
    /// exchange components between channels -- so those become
    /// [`Self::Blend`] and are evaluated per fragment against the constant, by
    /// the same function a mesh's per-vertex tint goes through.
    ///
    /// Every mode is therefore a filter and none of them is refused. This used
    /// to answer a `Result` and say in its own documentation that the advanced
    /// modes were "refused rather than approximated", which the code below has
    /// never done. Eighteen call sites carried an `expect` that could not fire, and
    /// several of their messages stated the opposite of what the branch they
    /// were on does.
    ///
    /// `color` is straight, like every color a caller states.
    pub fn blend(color: [f32; 4], mode: BlendMode) -> Self {
        use BlendMode as B;
        let alpha = color[3];
        // Premultiplied, because the rules below are stated for premultiplied
        // operands and the shader's own color is premultiplied too.
        let s = [color[0] * alpha, color[1] * alpha, color[2] * alpha, alpha];
        let zero = [[0.0f32; 4]; 4];
        let scaled = |k: f32| {
            let mut m = zero;
            for (i, column) in m.iter_mut().enumerate() {
                column[i] = k;
            }
            m
        };
        // The destination's alpha reaches every output channel through this
        // column alone, which is what makes the modes that multiply by it --
        // or by one minus it -- a matrix rather than a special case.
        let with_alpha_column = |mut m: [[f32; 4]; 4], column: [f32; 4]| {
            for (i, value) in column.iter().enumerate() {
                m[3][i] += *value;
            }
            m
        };
        let negated = [-s[0], -s[1], -s[2], -s[3]];
        let (columns, offset) = match mode {
            B::Clear => (zero, [0.0; 4]),
            B::Src => (zero, s),
            B::Dst => (scaled(1.0), [0.0; 4]),
            B::SrcOver => (scaled(1.0 - alpha), s),
            B::DstOver => (with_alpha_column(scaled(1.0), negated), s),
            B::SrcIn => (with_alpha_column(zero, s), [0.0; 4]),
            B::DstIn => (scaled(alpha), [0.0; 4]),
            B::SrcOut => (with_alpha_column(zero, negated), s),
            B::DstOut => (scaled(1.0 - alpha), [0.0; 4]),
            B::SrcATop => (with_alpha_column(scaled(1.0 - alpha), s), [0.0; 4]),
            B::DstATop => (with_alpha_column(scaled(alpha), negated), s),
            B::Xor => (with_alpha_column(scaled(1.0 - alpha), negated), s),
            B::Plus => (scaled(1.0), s),
            B::Modulate => {
                let mut m = zero;
                for (i, column) in m.iter_mut().enumerate() {
                    column[i] = s[i];
                }
                (m, [0.0; 4])
            }
            // Not affine, so not a matrix. Evaluated per fragment instead,
            // against the same constant, by the same function a mesh's
            // per-vertex tint goes through.
            mode => return Self::Blend { color: s, mode },
        };
        Self::Matrix {
            columns,
            offset,
            form: ColorForm::Premultiplied,
        }
    }

    /// Whether this changes anything.
    pub fn is_identity(&self) -> bool {
        match self {
            Self::None => true,
            Self::Matrix {
                columns,
                offset,
                form: _,
            } => {
                offset.iter().all(|v| *v == 0.0)
                    && columns.iter().enumerate().all(|(j, column)| {
                        column
                            .iter()
                            .enumerate()
                            .all(|(i, v)| *v == if i == j { 1.0 } else { 0.0 })
                    })
            }
            // `Dst` returns the destination untouched, which is the identity
            // and is the only mode here that is. It cannot arrive through
            // `blend`, which sends every affine mode to a matrix, but it can be
            // written by hand.
            Self::Blend { mode, .. } => *mode == BlendMode::Dst,
            // The curve is the identity at exactly three points -- zero, one,
            // and nowhere else on the range -- so as a function it never is.
            Self::Gamma { .. } => false,
        }
    }

    /// The code the shader reads to choose a path.
    fn code(&self) -> f32 {
        match self {
            Self::None => filter::NONE,
            Self::Matrix {
                form: ColorForm::Premultiplied,
                ..
            } => filter::PREMULTIPLIED,
            Self::Matrix {
                form: ColorForm::Straight,
                ..
            } => filter::STRAIGHT,
            Self::Gamma {
                direction: Gamma::LinearToSrgb,
            } => filter::LINEAR_TO_SRGB,
            Self::Gamma {
                direction: Gamma::SrgbToLinear,
            } => filter::SRGB_TO_LINEAR,
            Self::Blend { .. } => filter::BLEND,
        }
    }

    pub(crate) fn pack_into(&self, out: &mut [f32; MATERIAL_FLOATS]) {
        out[layout::FILTER_PARAMS] = self.code();
        match self {
            Self::Matrix {
                columns, offset, ..
            } => {
                for (j, column) in columns.iter().enumerate() {
                    out[layout::FILTER + j * 4..layout::FILTER + j * 4 + 4].copy_from_slice(column);
                }
                out[layout::FILTER_OFFSET..layout::FILTER_OFFSET + 4].copy_from_slice(offset);
            }
            // The constant goes where a matrix's offset would, which is what it
            // is: the term that does not depend on the material. The mode goes
            // in the first float of the matrix itself, which this filter has no
            // use for -- sixteen floats are already reserved for every draw,
            // and spending a fifth vector to carry one number would cost every
            // draw that does not blend.
            Self::Blend { color, mode } => {
                out[layout::FILTER_OFFSET..layout::FILTER_OFFSET + 4].copy_from_slice(color);
                out[layout::FILTER] = mode.code();
            }
            Self::None | Self::Gamma { .. } => {}
        }
    }
}

/// Filter selector shared with the shader.
pub mod filter {
    pub const NONE: f32 = 0.0;
    pub const PREMULTIPLIED: f32 = 1.0;
    pub const STRAIGHT: f32 = 2.0;
    pub const LINEAR_TO_SRGB: f32 = 3.0;
    pub const SRGB_TO_LINEAR: f32 = 4.0;
    /// A blend against a constant color, for the modes a matrix cannot state.
    pub const BLEND: f32 = 5.0;

    /// Whether a code names a filter that reads straight rather than
    /// premultiplied color.
    ///
    /// The shader decides this by comparison rather than by equality, and gets
    /// the same answer, so the boundary lives here where both can cite it: a
    /// filter added above this line is straight unless it says otherwise.
    ///
    /// [`BLEND`] says otherwise, which is what the sentence above anticipated.
    /// The compositing specification states every blend on premultiplied
    /// operands and the shader's own blend function takes them that way, so
    /// handing it straight color would be handing it the wrong numbers.
    pub fn is_straight(code: f32) -> bool {
        code > PREMULTIPLIED && code < BLEND
    }
}

/// A color stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    /// Linear color with straight alpha.
    pub color: [f32; 4],
    /// Position along the gradient, from zero to one.
    pub offset: f32,
}

impl Stop {
    pub fn new(color: [f32; 4], offset: f32) -> Self {
        Self { color, offset }
    }
}

/// Maps a clip-space offset into a gradient's own space, in column order.
///
/// Clip space is anisotropic whenever the target is not square, and a transform
/// may rotate or skew as well, so a circle in user space is an ellipse there.
/// Radial and sweep gradients measure distance and angle, both of which that
/// distortion changes, so they map back before measuring. A linear gradient
/// projects onto an axis, which distortion does not affect, and so does not
/// need this.
pub type ToLocal = [f32; 12];

/// How a shape is filled.
#[derive(Debug, Clone, PartialEq)]
pub enum Material {
    Solid([f32; 4]),
    /// A gradient along an axis, starting at a point **in clip space**.
    ///
    /// Clip space for the start, because the fragment stage locates itself from
    /// an interpolated clip position: the alternative, the fragment coordinate
    /// builtin, has a different origin in each graphics API and would run the
    /// gradient in opposite directions on the two backends.
    ///
    /// The axis is in the gradient's own space, and `to_local` maps a clip-space
    /// offset into it — the same pairing radial and sweep use, and for the same
    /// reason. Clip space is anisotropic whenever the target is not square, so
    /// projecting onto an axis *there* weights the two axes by the target's
    /// shape: on a target twice as wide as it is tall, a diagonal gradient runs
    /// in the wrong direction.
    LinearGradient {
        /// End minus start, in the gradient's own space.
        axis: [f32; 2],
        to_local: ToLocal,
        stops: Vec<Stop>,
        /// Texture slot holding this gradient's colors, when they did not fit.
        ///
        /// `None` is the ordinary case: the stops travel in the material and
        /// the shader walks them. `Some` means the recorder tabulated them into
        /// an image instead, because there were more than [`MAX_STOPS`], and
        /// the shader reads the color at the parameter rather than computing
        /// it. The two must agree where both are possible, which is what makes
        /// the choice invisible to a caller.
        ramp: Option<u32>,
        /// What happens beyond the two endpoints.
        ///
        /// The parameter a gradient is sampled by runs from zero at one end to
        /// one at the other and is defined everywhere else too, so a shape
        /// larger than its gradient asks a question the stops do not answer.
        /// Clamping holds the end colors, which is the usual choice; repeating
        /// tiles the ramp, which is what a stripe pattern is; decal draws
        /// nothing outside, the same meaning it has for an image.
        tile: TileMode,
    },
    /// A gradient outward from a center, **in clip space**, where `to_local`
    /// carries the radius: it maps the clip-space offset so that the gradient's
    /// edge lands at unit distance.
    RadialGradient {
        to_local: ToLocal,
        stops: Vec<Stop>,
        /// Texture slot holding this gradient's colors, when they did not fit.
        ///
        /// `None` is the ordinary case: the stops travel in the material and
        /// the shader walks them. `Some` means the recorder tabulated them into
        /// an image instead, because there were more than [`MAX_STOPS`], and
        /// the shader reads the color at the parameter rather than computing
        /// it. The two must agree where both are possible, which is what makes
        /// the choice invisible to a caller.
        ramp: Option<u32>,
        /// What happens beyond the radius. See [`Material::LinearGradient`].
        tile: TileMode,
    },
    /// A gradient around a center, **in clip space**, running from `start_angle`
    /// to `end_angle` in radians.
    SweepGradient {
        to_local: ToLocal,
        start_angle: f32,
        end_angle: f32,
        stops: Vec<Stop>,
        /// Texture slot holding this gradient's colors, when they did not fit.
        ///
        /// `None` is the ordinary case: the stops travel in the material and
        /// the shader walks them. `Some` means the recorder tabulated them into
        /// an image instead, because there were more than [`MAX_STOPS`], and
        /// the shader reads the color at the parameter rather than computing
        /// it. The two must agree where both are possible, which is what makes
        /// the choice invisible to a caller.
        ramp: Option<u32>,
        /// What happens outside the swept arc.
        ///
        /// Unlike the other two this can be a no-op: a sweep covering the whole
        /// turn has no outside, and every direction lands within it.
        tile: TileMode,
    },
    /// A gradient between two circles, the general form the other two are
    /// special cases of.
    ///
    /// `center` is the first circle's center **in clip space**, and `to_local`
    /// maps a clip-space offset into a space where that center is the origin
    /// and the second circle's center lies at `(separation, 0)`. Putting the
    /// separation on an axis costs nothing -- the rotation folds into a matrix
    /// that has to be there anyway -- and buys the second center for one float
    /// instead of two, which is what makes this fit at all.
    ///
    /// The radii are in that same space and are *not* normalized, unlike the
    /// radial gradient's, because there are two of them and a scale can only
    /// remove one. Carrying both plainly also means the degenerate cases need
    /// no special handling: concentric circles are `separation == 0`, and a
    /// cone rather than a tube is `radius_delta != 0`.
    ConicalGradient {
        to_local: ToLocal,
        /// Radius of the first circle.
        start_radius: f32,
        /// Second radius minus the first.
        radius_delta: f32,
        /// Distance between the two centers.
        separation: f32,
        stops: Vec<Stop>,
        /// Texture slot holding this gradient's colors, when they did not fit.
        /// See [`Material::LinearGradient`].
        ramp: Option<u32>,
        /// What happens where the parameter leaves the unit interval. See
        /// [`Material::LinearGradient`].
        tile: TileMode,
    },
    /// A caller's own fragment program, with the floats it reads.
    ///
    /// The program is named rather than carried: registering one builds a
    /// pipeline, which is expensive and outlives any draw, so a context holds
    /// them and a material names which. The floats travel in the same uniform
    /// block every other material uses, which is what lets an effect exist
    /// without a second descriptor set.
    Runtime {
        program: u32,
        uniforms: Vec<f32>,
        /// The textures the program may sample, in the order it declares them.
        ///
        /// `None` in a position means the program does not read that binding,
        /// and the backend binds its placeholder there -- a pipeline must have
        /// every binding it declares bound, however unreachable the branch
        /// reading it.
        ///
        /// The count is fixed rather than free because the descriptor set
        /// layout every pipeline is built against has to be one layout. Four is
        /// what that costs: three unused image bindings on a draw that samples
        /// nothing, against a second set and a second layout for the draws that
        /// want more than one.
        textures: [Option<u32>; MAX_EFFECT_TEXTURES],
    },
    /// A texture, sampled at coordinates the vertices carry.
    ///
    /// Deliberately not [`Material::Image`] with a switch on where its
    /// coordinates come from. An image mapped from clip space needs an origin,
    /// a matrix and a source rectangle to say which part of a sheet it draws;
    /// a mesh needs none of them, because a caller stating a coordinate per
    /// vertex has already answered all three. What is left is small enough to
    /// be its own thing, and keeping it separate means neither carries a field
    /// that means nothing for it.
    ///
    /// This is what makes a sprite batch one draw: a hundred quads reading a
    /// hundred different parts of one sheet differ only in their vertices.
    Mesh {
        /// Index into the texture table given at submission.
        slot: u32,
        /// Scales the sampled color, applied to premultiplied color like
        /// [`Material::Image`]'s.
        alpha: f32,
        /// Straight color the sampled texel is multiplied by; white changes
        /// nothing.
        tint: [f32; 4],
        /// What happens where a vertex names a coordinate outside the texture.
        tile: TileMode,
        /// How to read between texels.
        sampling: Sampling,
    },
    /// A texture, sampled through a mapping from clip space.
    ///
    /// `origin` and `to_local` together are an affine: a clip-space position
    /// maps to texture coordinates as `to_local * (clip - origin)`, which lands
    /// the image's top-left corner at zero and its bottom-right at one. The
    /// same pair a radial gradient uses, for the same reason — clip space is
    /// anisotropic on a non-square target, so a mapping that ignored it would
    /// stretch every image by the aspect ratio.
    ///
    /// Which texture is not named here. The material is data the recorder
    /// produces without touching the device, so it carries a slot into the
    /// table supplied at submission instead of a backend handle.
    Image {
        to_local: ToLocal,
        /// Index into the texture table given at submission.
        slot: u32,
        /// Scales the sampled color, for drawing an image translucently.
        ///
        /// Applied to premultiplied color, so it scales the whole texel rather
        /// than only its alpha. A texture holds premultiplied color whether it
        /// was uploaded or rendered into, and treating a sample as straight
        /// alpha would apply the alpha twice — invisible for an opaque image,
        /// and plain the moment one translucent image is drawn into another.
        alpha: f32,
        tile: TileMode,
        /// How to read between texels.
        sampling: Sampling,
        /// The part of the texture to draw, as `[u0, v0, u1, v1]` from zero to
        /// one.
        ///
        /// The whole texture is `[0, 0, 1, 1]`, which is what every caller
        /// wanted until sprite sheets. Normalized rather than in texels because
        /// a material is built by a recorder that has never seen the texture
        /// and cannot know how large it is; the caller who uploaded it does.
        ///
        /// Applied after tiling rather than before, so a repeat repeats the
        /// selected piece rather than the whole sheet -- which is the only
        /// reading of "tile this sprite" that means anything.
        source: [f32; 4],
        /// Straight color the sampled texel is multiplied by; white changes
        /// nothing.
        ///
        /// What turns one monochrome icon sheet into every state a control has.
        /// A generalization of `alpha` rather than a rival to it: a tint of
        /// `[1, 1, 1, a]` is exactly that scaling, and both are applied because
        /// removing the narrower one would break callers for no gain.
        ///
        /// Straight rather than premultiplied because that is how a caller
        /// states a color, and the shader premultiplies it before multiplying a
        /// texel that already is -- scaling color by the tint's alpha as well,
        /// which is what keeps the result premultiplied rather than merely
        /// close to it.
        tint: [f32; 4],
    },
    /// A rounded rectangle evaluated per fragment rather than tessellated.
    ///
    /// The shape an interface is mostly made of, and the one where computing
    /// coverage beats building triangles for it. A tessellated rounded
    /// rectangle costs vertices proportional to how round it is and has hard
    /// edges unless the whole pass is multisampled; this is two triangles
    /// whatever the radius, and antialiases itself from the distance field it
    /// already computes.
    ///
    /// The geometry is in the shape's own space, with `to_local` mapping a
    /// clip-space position into it -- the same pairing the gradients use, and
    /// for the same reason: clip space is anisotropic on a target that is not
    /// square, and a distance measured there would round the corners by
    /// different amounts on each axis.
    RoundedRect {
        color: [f32; 4],
        /// Half the width and height, in the shape's own space.
        half_size: [f32; 2],
        to_local: ToLocal,
        /// Corner radius, in the shape's own space.
        radius: f32,
        /// The radius the outline's *outer* edge turns through, which is not
        /// always the radius grown by half the stroke.
        ///
        /// An outline is the difference of two offset shapes rather than a band
        /// around one, and the outer offset is where a join shows. Grow a
        /// rounded corner and you get a bigger rounded corner, so this is
        /// `radius + stroke / 2` for anything with a radius, and for anything
        /// with a round join. A *mitered* square corner is the exception: its
        /// offset is still square, so this is zero and the field draws the
        /// point the join asks for.
        ///
        /// Carried rather than derived because the shader cannot see the join,
        /// and because deriving it wrongly is what made a square-cornered
        /// stroke come out with rounded corners and be refused this route
        /// altogether.
        outer_radius: f32,
        /// Trace the outline at this width rather than filling, in the shape's
        /// own space. Zero fills.
        ///
        /// Costs a distance field nothing: the field already says how far every
        /// fragment is from the edge, so an outline is the band where that is
        /// small. Tessellating one instead means building a second shape --
        /// offset inward and outward, with the corners resolved -- which is
        /// where a stroked outline gets its vertex count and its joins.
        stroke: f32,
    },
    /// A *blurred* rounded rectangle evaluated per fragment, with no blur pass.
    ///
    /// The sibling of [`Self::RoundedRect`], and the reason it is worth having
    /// is that the general route costs three passes per shape -- one for the
    /// content and two for a separable Gaussian -- where this costs one draw in
    /// the pass already being recorded. On a Raspberry Pi 5 three shadows that
    /// way are 7.8 ms of a 26.8 ms frame and nine of its fourteen passes.
    ///
    /// The method is Raph Levien's "Blurred rounded rectangles", which is what
    /// upstream's `SolidRRectBlurContents` evaluates too: the exact convolution
    /// of a Gaussian with a rounded rectangle has no closed form, and this
    /// approximates it as a product of two error functions along an axis,
    /// corrected for the corners by measuring distance with an exponent other
    /// than two. Every field here is a number the CPU precomputed for that
    /// expression rather than anything a caller stated -- see
    /// `Canvas::rrect_blur_material`, which is the only place they are derived.
    ///
    /// Only where every corner shares one circular radius, which is upstream's
    /// condition too. Anything else takes the general route.
    RoundedRectBlur {
        color: [f32; 4],
        to_local: ToLocal,
        /// Half the rectangle, less `r1`, in the shape's own space.
        adjust: [f32; 2],
        /// The corner radius the approximation uses, which is not the caller's:
        /// it grows with the deviation, because a blurred corner is rounder
        /// than a sharp one.
        r1: f32,
        /// The exponent the corner distance is measured with. Two is a circle;
        /// this is larger, which is what makes the blurred corner's profile
        /// match a Gaussian's rather than a circle's.
        exponent: f32,
        /// One over the deviation, which is what the error function takes.
        s_inv: f32,
        /// The shorter side, which bounds how far the fade can reach before the
        /// two edges of the shape meet.
        min_edge: f32,
        /// Normalizes the fade so that the middle of a large shape reaches full
        /// coverage.
        scale: f32,
    },
    /// An ellipse evaluated per fragment.
    ///
    /// Its own variant rather than a rounded rectangle with a large radius,
    /// which gives a stadium: past half the shorter side a rounded rectangle
    /// stops changing, where an ellipse keeps curving along both axes.
    ///
    /// The same geometry a rounded rectangle carries, less the radius, which
    /// the two axes already state.
    Ellipse {
        color: [f32; 4],
        /// The two semi-axes, in the shape's own space.
        half_size: [f32; 2],
        to_local: ToLocal,
        /// Trace the outline at this width rather than filling. Zero fills.
        stroke: f32,
    },
    /// One axis of a separable Gaussian blur of a sampled texture.
    ///
    /// Separable because a two-dimensional Gaussian is the product of two
    /// one-dimensional ones, so blurring along each axis in turn gives the
    /// same result as a square of taps at a fraction of the cost: at a radius
    /// of sixteen that is thirty-three taps against a thousand and eighty-nine.
    /// Two passes are the price, which is why this names an axis rather than
    /// describing the whole blur.
    Blur {
        to_local: ToLocal,
        slot: u32,
        /// One tap's step, in the sampled texture's own coordinates.
        ///
        /// The axis and the texel size together: a horizontal pass over a
        /// target `w` wide steps `(1/w, 0)`. Stated here rather than derived in
        /// the shader because the shader does not know the size of what it is
        /// sampling.
        step: [f32; 2],
        /// Standard deviation, in taps.
        ///
        /// The tap count follows from it -- the kernel reaches
        /// `(sigma - 0.5) * sqrt(3)` each way, which is upstream's radius for a
        /// given deviation -- so a caller sets how soft the result is and
        /// nothing else.
        sigma: f32,
    },
    /// One axis of a morphological filter of a finished layer.
    ///
    /// The largest or smallest sample within a radius, per channel, which is
    /// what `dart:ui` calls `ImageFilter.dilate` and `ImageFilter.erode`. Like
    /// the blur it is separable -- a rectangular structuring element is the
    /// product of two intervals -- so two passes give the square of taps.
    ///
    /// Unlike the blur it is also *decomposable*: dilating by `a` and then by
    /// `b` is dilating by `a + b` exactly, because the structuring elements add
    /// under the Minkowski sum. A radius past what one pass can reach is
    /// therefore split across passes rather than approximated by sampling more
    /// sparsely. Sparse taps work for a blur, where a missed sample costs a
    /// little smoothness, and do not work here: the result is a maximum, so a
    /// missed sample is a scallop in the edge.
    Morphology {
        to_local: ToLocal,
        slot: u32,
        /// One tap's step, in the sampled texture's own coordinates. As
        /// [`Self::Blur::step`].
        step: [f32; 2],
        /// How many texels each way this pass reaches, at most
        /// [`MORPHOLOGY_TAPS`].
        ///
        /// A whole number of texels, because the structuring element is a set
        /// of samples rather than a weighting of them and there is no meaning
        /// to half of one.
        radius: f32,
        /// The largest sample in reach rather than the smallest.
        dilate: bool,
    },
    /// Coverage sampled from an atlas, tinting one color.
    ///
    /// Distinct from [`Self::Image`] in two ways that matter. The texture is
    /// read as *coverage* rather than as color — one channel scaling a solid,
    /// which is what antialiased text is — and the coordinates come from the
    /// vertices rather than from a mapping in the paint, so a run of glyphs
    /// reading different parts of one atlas is a single draw.
    Glyph {
        /// Linear color with straight alpha, as the text is painted.
        color: [f32; 4],
        /// Index into the texture table given at submission.
        slot: u32,
    },
    /// Many discs of one color, evaluated from the vertices rather than the
    /// paint, so that a field of them is one draw.
    ///
    /// Carries no mapping and no size, which is the whole point of it. Every
    /// other fragment-evaluated shape here locates itself through `to_local`,
    /// and `to_local` holds the shape's center -- so two of them at different
    /// places are two materials and cannot share a draw. This one locates
    /// itself from the interpolated texture coordinate, which the vertices
    /// carry as the unit circle's corners, so any number of discs at any
    /// centers are one material and one draw.
    ///
    /// The edge is the same edge. `disc_coverage` in the shader differentiates
    /// the implicit function across the pixel rather than forming a distance,
    /// and a derivative of an interpolated value is as available as that of a
    /// computed one.
    PointField {
        /// One color for the field, premultiplied as everything here is.
        color: [f32; 4],
    },
}

/// What happens outside an image's own bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileMode {
    /// Hold the edge pixel. The usual choice for drawing an image once.
    #[default]
    Clamp,
    /// Repeat the image, tiling the plane.
    Repeat,
    /// Draw nothing outside the image.
    ///
    /// Distinct from clamping in the only case that matters: a shape larger
    /// than the image it is filled with. Clamping smears the border across the
    /// remainder, which reads as a rendering fault rather than as a choice.
    Decal,
    /// Repeat, reversing every other copy.
    ///
    /// What repeating is for when the two ends do not match. A ramp tiled by
    /// [`TileMode::Repeat`] jumps from its last color back to its first at
    /// every period, and that discontinuity is a visible seam; reflecting each
    /// alternate copy joins end to end and leaves none. The period is twice as
    /// long, since a copy and its reflection make one.
    Mirror,
}

impl Material {
    pub fn solid(color: [f32; 4]) -> Self {
        Self::Solid(color)
    }

    /// How this material reads a texture, if it reads one.
    ///
    /// `None` for a material that samples nothing, which is not the same answer
    /// as [`Sampling::Nearest`]: one says there is no texture and the other says
    /// there is and it is read at a texel's center.
    ///
    /// Here because a caller above the HAL may need to know whether a draw
    /// interpolates between texels, and only this type knows. The test harness
    /// asks it to size a cross-device tolerance: a filter that forms a weighted
    /// sum of texels does so with weights of implementation-defined precision,
    /// so two devices disagree on every interpolated pixel by more than the last
    /// bit of one store. Deriving that from a recording rather than from the
    /// scene that produced it is the point -- the scene model has four separate
    /// places a sampled texture can come from and a comment recording four
    /// occasions on which enumerating them missed one.
    pub fn sampling(&self) -> Option<Sampling> {
        match self {
            Self::Image { sampling, .. } | Self::Mesh { sampling, .. } => Some(*sampling),
            // Reads a texture and states no filter: a blur and a morphology
            // sample their own target at offsets they compute, and a glyph reads
            // coverage out of the atlas. What they do between texels is the
            // shader's, not a field's, so there is no mode to report here.
            Self::Blur { .. }
            | Self::Morphology { .. }
            | Self::Glyph { .. }
            | Self::Runtime { .. } => None,
            // Reads no texture at all.
            Self::Solid(_)
            | Self::LinearGradient { .. }
            | Self::RadialGradient { .. }
            | Self::SweepGradient { .. }
            | Self::ConicalGradient { .. }
            | Self::RoundedRect { .. }
            | Self::RoundedRectBlur { .. }
            | Self::Ellipse { .. }
            | Self::PointField { .. } => None,
        }
    }

    /// Whether this material reads between texels rather than at one.
    ///
    /// Every mode but [`Sampling::Nearest`] forms a weighted sum: linear over
    /// four texels, bicubic over sixteen, mipmapped over two levels of the
    /// first. Measured on a Raspberry Pi 5, one scene rendered twice differing
    /// only in this: nearest is byte for byte identical between v3d and llvmpipe
    /// over the whole frame, and linear reaches a delta of three across seventy
    /// per cent of it.
    /// Whether this material reads its texture at offsets it computes, rather
    /// than at the coordinate it was handed.
    ///
    /// True for the two filters that walk a neighborhood: a blur steps out by a
    /// sigma, a morphology by a radius, and neither need land on a texel center
    /// whatever size the texture is. So this is the one case where a material
    /// can say that a read interpolates without knowing the texture -- which is
    /// why it is a separate question from [`Self::sampling`], and why a layer
    /// dilated by a morphology is one of the scenes that put two devices three
    /// levels apart.
    pub fn reads_at_computed_offsets(&self) -> bool {
        matches!(self, Self::Blur { .. } | Self::Morphology { .. })
    }

    /// The same material at `factor` of its opacity.
    ///
    /// Applied to every color a material carries, since a gradient's stops may
    /// differ in alpha and scaling them together is what keeps the ramp the
    /// same ramp. Straight alpha, so this happens before the premultiply the
    /// packing does rather than after it.
    ///
    /// Two variants are left alone and it is worth saying which. A blur or a
    /// morphology is a filter over a finished pass rather than a paint, so
    /// there is no color in it to dim -- and nothing asks this of one. A
    /// caller's program is the other: its output is whatever it computes, and
    /// this renderer has no uniform it may write to. That is a real limit
    /// wherever the caller wanted the dimming, and it is stated at the one
    /// place that asks for it.
    #[must_use]
    pub fn with_opacity(mut self, factor: f32) -> Self {
        let scale = |color: &mut [f32; 4]| color[3] *= factor;
        match &mut self {
            Self::Solid(color)
            | Self::RoundedRect { color, .. }
            | Self::RoundedRectBlur { color, .. }
            | Self::Ellipse { color, .. }
            | Self::PointField { color }
            | Self::Glyph { color, .. } => scale(color),
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. }
            | Self::ConicalGradient { stops, .. } => {
                for stop in stops.iter_mut() {
                    scale(&mut stop.color);
                }
            }
            Self::Mesh { alpha, .. } | Self::Image { alpha, .. } => *alpha *= factor,
            Self::Runtime { .. } | Self::Blur { .. } | Self::Morphology { .. } => {}
        }
        self
    }

    /// Whether drawing with this would change anything.
    pub fn is_invisible(&self) -> bool {
        match self {
            Self::Solid(color) | Self::PointField { color } => color[3] <= 0.0,
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. }
            | Self::ConicalGradient { stops, .. } => {
                stops.is_empty() || stops.iter().all(|s| s.color[3] <= 0.0)
            }
            // What the texture holds is unknown here, so only a zero alpha
            // makes an image provably invisible.
            Self::Image { alpha, .. } | Self::Mesh { alpha, .. } => *alpha <= 0.0,
            // What a caller's program draws is unknowable from here, so the
            // only honest answer is that it might draw something.
            Self::Runtime { .. } => false,
            // A blur of nothing is nothing, but the pass still has to run: what
            // it samples is not knowable from here.
            Self::Blur { .. } | Self::Morphology { .. } => false,
            // No `half_size` to test: a blurred rectangle with no area still
            // draws, because the blur carries color past where the shape is.
            Self::RoundedRectBlur { color, .. } => color[3] <= 0.0,
            Self::RoundedRect {
                color, half_size, ..
            }
            | Self::Ellipse {
                color, half_size, ..
            } => color[3] <= 0.0 || half_size[0] <= 0.0 || half_size[1] <= 0.0,
            Self::Glyph { color, .. } => color[3] <= 0.0,
        }
    }

    /// The texture slot this samples, for a backend building its bindings.
    ///
    /// Matched exhaustively rather than with a catch-all. A variant that
    /// samples something and is not listed here reports no slot, so the backend
    /// binds its placeholder and the draw comes out flat white -- a plausible
    /// picture rather than an error, and one nothing else would explain.
    /// Every texture slot this material samples, in binding order.
    ///
    /// One entry for everything but a runtime program, which may declare
    /// several. A backend building descriptors needs the whole tuple, since a
    /// set holds all of them at once; a backend asking only which slot to bind
    /// first has [`Self::texture_slot`].
    pub fn texture_slots(&self) -> [Option<u32>; MAX_EFFECT_TEXTURES] {
        match self {
            Self::Runtime { textures, .. } => *textures,
            other => {
                let mut slots = [None; MAX_EFFECT_TEXTURES];
                slots[0] = other.texture_slot();
                slots
            }
        }
    }

    pub fn texture_slot(&self) -> Option<u32> {
        match self {
            Self::Image { slot, .. }
            | Self::Mesh { slot, .. }
            | Self::Glyph { slot, .. }
            | Self::Blur { slot, .. }
            | Self::Morphology { slot, .. } => Some(*slot),
            // A gradient names a texture only when its colors were too many to
            // carry, which is why this is an option rather than a slot.
            Self::LinearGradient { ramp, .. }
            | Self::RadialGradient { ramp, .. }
            | Self::SweepGradient { ramp, .. }
            | Self::ConicalGradient { ramp, .. } => *ramp,
            Self::Solid(_)
            | Self::PointField { .. }
            | Self::RoundedRect { .. }
            | Self::RoundedRectBlur { .. }
            | Self::Ellipse { .. } => None,
            // Whatever a caller named, and `None` where they named nothing --
            // in which case the placeholder is bound and a program that
            // samples anyway reads opaque white.
            // The first of them, for the callers that still ask a material for
            // one slot. `texture_slots` is what a backend binding several
            // should ask.
            Self::Runtime { textures, .. } => textures[0],
        }
    }

    /// The stops, for any material that has them.
    fn stops(&self) -> &[Stop] {
        match self {
            Self::Solid(_)
            | Self::PointField { .. }
            | Self::Image { .. }
            | Self::Mesh { .. }
            | Self::Glyph { .. }
            | Self::Blur { .. }
            | Self::Morphology { .. }
            | Self::RoundedRect { .. }
            | Self::RoundedRectBlur { .. }
            | Self::Ellipse { .. }
            | Self::Runtime { .. } => &[],
            Self::LinearGradient { stops, .. }
            | Self::RadialGradient { stops, .. }
            | Self::SweepGradient { stops, .. }
            | Self::ConicalGradient { stops, .. } => stops,
        }
    }

    /// Pack into the layout the shader declares.
    ///
    /// Every member is a four-component vector, which is what lets this be a
    /// flat array of floats copied straight into a uniform buffer: the std140
    /// rules the shader's block is declared with place a `vec4` and an array
    /// of them at exactly these offsets, so no member needs padding written
    /// around it.
    ///
    /// Stops beyond the limit are dropped rather than resampled, and the count
    /// travels alongside so the shader ignores unused entries instead of
    /// blending toward whatever happens to be in them.
    /// Whether every pixel this material writes comes out fully opaque.
    ///
    /// Used to decide whether a draw may be split into several, which is only safe when
    /// writing a pixel twice gives what writing it once does. Under `SrcOver` that holds
    /// exactly when the source is opaque.
    ///
    /// **False unless shown otherwise.** A solid fill carries its alpha and a gradient's
    /// stops carry theirs. Everything else -- images, the analytic shapes, blurs, a
    /// caller's own program -- is refused outright.
    ///
    /// A tabulated gradient still answers from its stops, and that is worth saying because
    /// the first version of this refused one. `ramp` being `Some` means the *shader* reads
    /// a texture instead of walking the list, not that the list is gone: the recorder maps
    /// every stop into the material either way and bakes the ramp from the same list, by
    /// interpolating between them. Interpolating between opaque colors gives an opaque
    /// one, so the texels are opaque exactly when the stops are. Refusing a ramp cost the
    /// whole of occlusion culling's benefit on the bench's frame, whose wash is five stops
    /// against a `MAX_STOPS` of four.
    pub fn is_opaque(&self) -> bool {
        let stops_opaque =
            |stops: &[Stop]| !stops.is_empty() && stops.iter().all(|s| s.color[3] >= 1.0);
        match self {
            Material::Solid(color) => color[3] >= 1.0,
            Material::LinearGradient { stops, .. }
            | Material::RadialGradient { stops, .. }
            | Material::SweepGradient { stops, .. }
            | Material::ConicalGradient { stops, .. } => stops_opaque(stops),
            _ => false,
        }
    }

    /// Whether this material's shading reads screen-space derivatives.
    ///
    /// `dpdx` and `dpdy` are computed across a two-by-two quad of fragments, using helper
    /// invocations outside the primitive. Whether a scissor keeps those helpers alive is
    /// not something either specification settles, and implementations differ: Mesa
    /// 26.2.3 keeps them and 25.2.8 does not. So a draw whose shading depends on them
    /// gives a different answer at a scissor's edge than away from it, and splitting such
    /// a draw by scissor changes the picture along the seam.
    ///
    /// Measured rather than reasoned about: an analytic shadow drawn once reads 217 where
    /// the same shadow drawn in four scissored strips reads 206, at the pixel on the
    /// strip boundary, on Mesa 25.2.8. See `a_scissor_does_not_change_what_a_draw_paints`.
    ///
    /// **True unless shown otherwise**, which is the only safe direction: a material
    /// wrongly called derivative-free leaves a seam in the frame. The two groups that are
    /// shown otherwise are solid fills, whose coverage comes from the rasterizer, and the
    /// gradients, which sample their ramp at an explicit level of zero. The rest either
    /// take a coverage from the gradient of a distance field -- the rounded rectangle, the
    /// ellipse and the rounded-rectangle blur -- or a texture level from the rate their
    /// coordinates change, which is every image fill, or are a caller's own program.
    pub fn needs_screen_derivatives(&self) -> bool {
        !matches!(
            self,
            Material::Solid(_)
                | Material::LinearGradient { .. }
                | Material::RadialGradient { .. }
                | Material::SweepGradient { .. }
                | Material::ConicalGradient { .. }
        )
    }

    pub fn to_uniform(&self) -> [f32; MATERIAL_FLOATS] {
        let mut out = [0.0f32; MATERIAL_FLOATS];

        if let Self::Solid(color) = self {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::SOLID;
            return out;
        }

        // A glyph is a solid color plus a texture read; the coordinates come
        // from the vertices, so nothing about the mapping is packed here.
        if let Self::Glyph { color, .. } = self {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::GLYPH;
            return out;
        }

        // An image carries no stops and no count, and must be packed before
        // the gradient path below decides it has too few to interpolate.
        if let Self::PointField { color } = self {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::POINT_FIELD;
            return out;
        }

        if let Self::Ellipse {
            color,
            half_size,
            to_local,
            stroke,
        } = self
        {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::GEOMETRY + 2] = half_size[0];
            out[layout::GEOMETRY + 3] = half_size[1];
            out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::ELLIPSE;
            out[layout::PARAMS + 3] = *stroke;
            return out;
        }

        if let Self::RoundedRect {
            color,
            half_size,
            to_local,
            radius,
            outer_radius,
            stroke,
        } = self
        {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::GEOMETRY + 1] = *outer_radius;
            out[layout::GEOMETRY + 2] = half_size[0];
            out[layout::GEOMETRY + 3] = half_size[1];
            out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::ROUNDED_RECT;
            out[layout::PARAMS + 2] = *radius;
            out[layout::PARAMS + 3] = *stroke;
            return out;
        }

        if let Self::RoundedRectBlur {
            color,
            to_local,
            adjust,
            r1,
            exponent,
            s_inv,
            min_edge,
            scale,
        } = self
        {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(color);
            out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
            // The first two floats of `GEOMETRY` are unclaimed for every kind
            // that carries a mapping, which this does, so the two that follow
            // are what a rounded rectangle would have used for its half size --
            // and it needs none, because `adjust` already has the shape in it.
            out[layout::GEOMETRY] = adjust[0];
            out[layout::GEOMETRY + 1] = adjust[1];
            out[layout::GEOMETRY + 2] = *s_inv;
            out[layout::GEOMETRY + 3] = *min_edge;
            // `OFFSETS` holds stop positions for a gradient and nothing for
            // anything else, which is what makes it the seventh float this
            // needs and the only kind here that borrows it.
            out[layout::OFFSETS] = *scale;
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::ROUNDED_RECT_BLUR;
            out[layout::PARAMS + 2] = *r1;
            out[layout::PARAMS + 3] = *exponent;
            return out;
        }

        if let Self::Blur {
            to_local,
            step,
            sigma,
            ..
        } = self
        {
            out[layout::GEOMETRY + 2] = step[0];
            out[layout::GEOMETRY + 3] = step[1];
            out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::BLUR;
            out[layout::PARAMS + 2] = *sigma;
            return out;
        }

        if let Self::Morphology {
            to_local,
            step,
            radius,
            dilate,
            ..
        } = self
        {
            out[layout::GEOMETRY + 2] = step[0];
            out[layout::GEOMETRY + 3] = step[1];
            out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::MORPHOLOGY;
            out[layout::PARAMS + 2] = radius.clamp(0.0, MORPHOLOGY_TAPS as f32);
            out[layout::PARAMS + 3] = if *dilate { 1.0 } else { 0.0 };
            return out;
        }

        if let Self::Runtime { uniforms, .. } = self {
            // Straight into the block, in the order the caller wrote them. No
            // kind is set: nothing in the shared shader will read this, and a
            // caller's program does not branch on one.
            let count = uniforms.len().min(RUNTIME_FLOATS);
            out[..count].copy_from_slice(&uniforms[..count]);
            return out;
        }

        if let Self::Mesh {
            alpha,
            tint,
            tile,
            sampling,
            ..
        } = self
        {
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(tint);
            out[layout::GEOMETRY] = *alpha;
            out[layout::GEOMETRY + 1] = tile_code(*tile);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::MESH;
            out[layout::PARAMS + 2] = sampling_code(*sampling);
            return out;
        }

        if let Self::Image {
            to_local,
            alpha,
            tile,
            sampling,
            source,
            tint,
            ..
        } = self
        {
            // Into the stop colors, which an image has none of. Eight floats
            // that would otherwise travel as zeros on every image draw.
            out[layout::STOPS..layout::STOPS + 4].copy_from_slice(source);
            out[layout::STOPS + 4..layout::STOPS + 8].copy_from_slice(tint);
            out[layout::GEOMETRY + 2] = *alpha;
            out[layout::GEOMETRY + 3] = tile_code(*tile);
            out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
            out[layout::PARAMS] = 1.0;
            out[layout::PARAMS + 1] = kind::IMAGE;
            out[layout::PARAMS + 2] = sampling_code(*sampling);
            return out;
        }

        let stops = self.stops();
        let count = stops.len().min(MAX_STOPS);
        for (i, stop) in stops.iter().take(count).enumerate() {
            out[layout::STOPS + i * 4..layout::STOPS + i * 4 + 4].copy_from_slice(&stop.color);
            out[layout::OFFSETS + i] = stop.offset;
        }
        out[layout::PARAMS] = count.max(1) as f32;

        // A gradient with fewer than two stops has nothing to interpolate
        // between, so it renders as its first color rather than sending the
        // shader down a path that would read an entry nothing wrote.
        if count < 2 {
            out[layout::PARAMS + 1] = kind::SOLID;
            return out;
        }

        match self {
            Self::Solid(_)
            | Self::PointField { .. }
            | Self::Image { .. }
            | Self::Mesh { .. }
            | Self::Glyph { .. }
            | Self::Blur { .. }
            | Self::Morphology { .. }
            | Self::RoundedRect { .. }
            | Self::RoundedRectBlur { .. }
            | Self::Ellipse { .. }
            | Self::Runtime { .. } => {
                unreachable!("handled above")
            }
            Self::LinearGradient {
                ramp,
                axis,
                to_local,
                tile,
                ..
            } => {
                out[layout::PARAMS] = stop_count_code(count, ramp);
                out[layout::PARAMS + 2] = tile_code(*tile);
                out[layout::GEOMETRY + 2] = axis[0];
                out[layout::GEOMETRY + 3] = axis[1];
                out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
                out[layout::PARAMS + 1] = kind::LINEAR;
            }
            Self::RadialGradient {
                ramp,
                to_local,
                tile,
                ..
            } => {
                out[layout::PARAMS] = stop_count_code(count, ramp);
                out[layout::PARAMS + 2] = tile_code(*tile);
                out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
                out[layout::PARAMS + 1] = kind::RADIAL;
            }
            Self::SweepGradient {
                ramp,
                to_local,
                start_angle,
                end_angle,
                tile,
                ..
            } => {
                out[layout::PARAMS] = stop_count_code(count, ramp);
                out[layout::PARAMS + 2] = tile_code(*tile);
                out[layout::GEOMETRY + 2] = *start_angle;
                out[layout::GEOMETRY + 3] = *end_angle;
                out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
                out[layout::PARAMS + 1] = kind::SWEEP;
            }
            Self::ConicalGradient {
                ramp,
                to_local,
                start_radius,
                radius_delta,
                separation,
                tile,
                ..
            } => {
                out[layout::PARAMS] = stop_count_code(count, ramp);
                out[layout::PARAMS + 2] = tile_code(*tile);
                out[layout::GEOMETRY + 2] = *start_radius;
                out[layout::GEOMETRY + 3] = *radius_delta;
                out[layout::TO_LOCAL..layout::TO_LOCAL + 12].copy_from_slice(to_local);
                out[layout::PARAMS + 1] = kind::CONICAL;
                out[layout::PARAMS + 3] = *separation;
            }
        }
        out
    }

    /// The caller's program this draws with, where it uses one.
    ///
    /// `None` is every material the built-in shader draws, which is all of
    /// them but one.
    pub fn program(&self) -> Option<u32> {
        match self {
            Self::Runtime { program, .. } => Some(*program),
            _ => None,
        }
    }

    /// Which shader variant this needs, for keying a pipeline.
    ///
    /// Every variant lives in one program today, selected by a uniform, so this
    /// exists for the moment a material needs its own pipeline rather than
    /// pretending that moment has arrived.
    pub fn variant(&self) -> MaterialVariant {
        match self {
            Self::Solid(_) => MaterialVariant::Solid,
            _ => MaterialVariant::Gradient,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialVariant {
    Solid,
    Gradient,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a material can say about a texture read, and what it cannot.
    ///
    /// The second half is the point. A material has never seen the texture it
    /// names -- `Image::source` says so in as many words -- so it can report the
    /// filter that is bound and never whether that filter interpolates, which
    /// depends on the texture's size against the size it is drawn at. Only the
    /// two neighborhood filters escape that, because they step out by a sigma or
    /// a radius whatever the size is.
    #[test]
    fn a_material_reports_its_filter_and_not_what_the_filter_does() {
        let image = |sampling| Material::Image {
            to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
            slot: 3,
            alpha: 1.0,
            tile: TileMode::Clamp,
            sampling,
            source: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0; 4],
        };

        for sampling in [
            Sampling::Nearest,
            Sampling::Linear,
            Sampling::Cubic,
            Sampling::Mipmap,
        ] {
            assert_eq!(image(sampling).sampling(), Some(sampling));
            assert_eq!(image(sampling).texture_slot(), Some(3));
            // Even a linear one: whether it lands between texels is the
            // caller's geometry against a size this type does not hold.
            assert!(
                !image(sampling).reads_at_computed_offsets(),
                "an image is read where it is drawn, not at an offset of its own"
            );
        }

        // The one case a material can settle by itself.
        let morphology = Material::Morphology {
            to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
            slot: 0,
            step: [1.0, 0.0],
            radius: 2.0,
            dilate: true,
        };
        assert_eq!(
            morphology.sampling(),
            None,
            "it carries no filter to report"
        );
        assert!(morphology.reads_at_computed_offsets());

        // Reads a texture, states no filter, and walks nothing: the pair of
        // answers is why `sampling` returning `None` cannot be read as "no
        // texture".
        let glyph = Material::Glyph {
            color: [1.0; 4],
            slot: 1,
        };
        assert_eq!(glyph.sampling(), None);
        assert_eq!(glyph.texture_slot(), Some(1));
        assert!(!glyph.reads_at_computed_offsets());

        // Reads nothing at all.
        let solid = Material::solid([1.0; 4]);
        assert_eq!(solid.sampling(), None);
        assert_eq!(solid.texture_slot(), None);
        assert!(!solid.reads_at_computed_offsets());
    }

    /// A mapping with the given two-by-two linear part and no translation.
    ///
    /// Most of these tests care that the mapping survives packing, not what it
    /// is, and said so in four floats before the paint's origin moved inside
    /// it. This keeps them saying that.
    fn linear_to_local(m: [f32; 4]) -> ToLocal {
        [
            m[0], m[1], 0.0, 0.0, //
            m[2], m[3], 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0,
        ]
    }

    fn two_stops() -> Vec<Stop> {
        vec![
            Stop::new([1.0, 0.0, 0.0, 1.0], 0.0),
            Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
        ]
    }

    #[test]
    fn a_solid_color_lands_in_the_first_stop_and_selects_the_solid_path() {
        let packed = Material::solid([0.25, 0.5, 0.75, 1.0]).to_uniform();
        assert_eq!(&packed[0..4], &[0.25, 0.5, 0.75, 1.0]);
        assert_eq!(packed[layout::PARAMS], 1.0, "stop count");
        assert_eq!(packed[layout::PARAMS + 1], kind::SOLID);
    }

    #[test]
    fn a_linear_gradient_packs_its_stops_axis_and_count() {
        let packed = Material::LinearGradient {
            axis: [2.0, 0.0],
            to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
            stops: two_stops(),
            tile: TileMode::Clamp,
            ramp: None,
        }
        .to_uniform();

        assert_eq!(&packed[0..4], &[1.0, 0.0, 0.0, 1.0], "first stop");
        assert_eq!(&packed[4..8], &[0.0, 0.0, 1.0, 1.0], "second stop");
        assert_eq!(packed[layout::OFFSETS], 0.0);
        assert_eq!(packed[layout::OFFSETS + 1], 1.0);
        assert_eq!(
            &packed[layout::GEOMETRY..layout::GEOMETRY + 4],
            &[0.0, 0.0, 2.0, 0.0],
            "the axis rather than the end point, and nothing in the half the \
             start used to occupy before it moved inside the mapping"
        );
        assert_eq!(
            &packed[layout::TO_LOCAL..layout::TO_LOCAL + 12],
            &linear_to_local([1.0, 0.0, 0.0, 1.0])
        );
        assert_eq!(packed[layout::PARAMS + 1], kind::LINEAR);
    }

    #[test]
    fn a_radial_gradient_packs_its_mapping() {
        let packed = Material::RadialGradient {
            to_local: linear_to_local([2.0, 0.0, 0.0, 4.0]),
            stops: two_stops(),
            tile: TileMode::Clamp,
            ramp: None,
        }
        .to_uniform();

        // A radial gradient states nothing in the geometry slot at all now: its
        // center rode there, and rides inside the mapping instead.
        assert_eq!(&packed[layout::GEOMETRY..layout::GEOMETRY + 2], &[0.0, 0.0]);
        // The mapping is what makes a circle circular on a non-square target,
        // so it has to survive packing intact.
        assert_eq!(
            &packed[layout::TO_LOCAL..layout::TO_LOCAL + 12],
            &linear_to_local([2.0, 0.0, 0.0, 4.0])
        );
        assert_eq!(packed[layout::PARAMS + 1], kind::RADIAL);
    }

    #[test]
    fn a_sweep_gradient_packs_its_angles_alongside_its_center() {
        let packed = Material::SweepGradient {
            to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
            start_angle: 0.5,
            end_angle: 2.5,
            stops: two_stops(),
            tile: TileMode::Clamp,
            ramp: None,
        }
        .to_uniform();

        // Angles share the geometry slot with the center, which is why a linear
        // gradient's endpoints and a sweep's angles cannot both be present.
        assert_eq!(
            &packed[layout::GEOMETRY..layout::GEOMETRY + 4],
            &[0.0, 0.0, 0.5, 2.5]
        );
        assert_eq!(packed[layout::PARAMS + 1], kind::SWEEP);
    }

    #[test]
    fn every_gradient_kind_falls_back_to_solid_with_one_stop() {
        // Interpolating needs two points. Selecting a gradient path with one
        // would have the shader read an entry nothing wrote.
        let one = vec![Stop::new([1.0, 1.0, 1.0, 1.0], 0.0)];
        let materials = [
            Material::LinearGradient {
                axis: [1.0 - 0.0, 0.0 - 0.0],
                to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
                stops: one.clone(),
                tile: TileMode::Clamp,
                ramp: None,
            },
            Material::RadialGradient {
                to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
                stops: one.clone(),
                tile: TileMode::Clamp,
                ramp: None,
            },
            Material::SweepGradient {
                to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
                start_angle: 0.0,
                end_angle: 1.0,
                stops: one,
                tile: TileMode::Clamp,
                ramp: None,
            },
        ];
        for material in materials {
            let packed = material.to_uniform();
            assert_eq!(packed[layout::PARAMS + 1], kind::SOLID, "{material:?}");
            assert_eq!(&packed[0..4], &[1.0, 1.0, 1.0, 1.0]);
        }
    }

    #[test]
    fn stops_beyond_the_limit_are_dropped_rather_than_overflowing() {
        let stops: Vec<Stop> = (0..8)
            .map(|i| Stop::new([i as f32 / 8.0, 0.0, 0.0, 1.0], i as f32 / 7.0))
            .collect();
        let packed = Material::LinearGradient {
            axis: [1.0 - 0.0, 0.0 - 0.0],
            to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
            stops,
            tile: TileMode::Clamp,
            ramp: None,
        }
        .to_uniform();
        // The count is what stops the shader reading past what was written.
        assert_eq!(packed[layout::PARAMS], MAX_STOPS as f32);
    }

    #[test]
    fn visibility_accounts_for_every_stop_of_every_kind() {
        assert!(Material::solid([1.0, 1.0, 1.0, 0.0]).is_invisible());
        assert!(!Material::solid([0.0, 0.0, 0.0, 1.0]).is_invisible());

        let clear = vec![
            Stop::new([1.0, 0.0, 0.0, 0.0], 0.0),
            Stop::new([0.0, 0.0, 1.0, 0.0], 1.0),
        ];
        assert!(Material::RadialGradient {
            to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
            stops: clear,
            tile: TileMode::Clamp,
            ramp: None,
        }
        .is_invisible());

        let partly = vec![
            Stop::new([1.0, 0.0, 0.0, 0.0], 0.0),
            Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
        ];
        assert!(
            !Material::SweepGradient {
                to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
                start_angle: 0.0,
                end_angle: 1.0,
                stops: partly,
                tile: TileMode::Clamp,
                ramp: None,
            }
            .is_invisible(),
            "one visible stop is enough"
        );
    }

    #[test]
    fn every_gradient_kind_reports_the_same_variant() {
        // They share one program, selected by a uniform, so keying a pipeline
        // on the kind would create three identical pipelines.
        assert_eq!(Material::solid([0.0; 4]).variant(), MaterialVariant::Solid);
        for material in [
            Material::LinearGradient {
                axis: [1.0, 0.0],
                to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
                stops: two_stops(),
                tile: TileMode::Clamp,
                ramp: None,
            },
            Material::RadialGradient {
                to_local: linear_to_local([1.0, 0.0, 0.0, 1.0]),
                stops: two_stops(),
                tile: TileMode::Clamp,
                ramp: None,
            },
        ] {
            assert_eq!(material.variant(), MaterialVariant::Gradient);
        }
    }
}

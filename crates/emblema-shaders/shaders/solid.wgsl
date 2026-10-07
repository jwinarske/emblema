// Solid color and gradients.
//
// Positions arrive in normalized device coordinates, in the WGSL convention:
// Y increases upward. Translation to a backend whose framebuffer runs the
// other way is naga's job, which is what lets this one source produce matching
// output on every target rather than mirrored output on some of them.
//
// The fragment stage needs to know where it is in order to evaluate a
// gradient, and it learns that from an interpolated clip position rather than
// from the fragment coordinate builtin. That builtin's origin differs between
// the two APIs, so using it would make gradients run in opposite directions on
// each; clip space is normalized by the translator and agrees everywhere.
// Passing it as a varying also avoids a second vertex attribute, which every
// solid draw would otherwise pay for.

// One paint per draw, in a uniform buffer bound at a different offset for each
// draw. It traveled in push constants for a long time, which suited data this
// small and this changeable -- but 128 bytes is all a device guarantees there,
// the material had grown to occupy exactly that, and a material that has to
// sit on top of another one does not fit at any arrangement. A color filter is
// that case.
//
// Every member is a four-component vector on purpose. Under the std140 rules
// this block is declared with, a vec4 and an array of them sit at exactly the
// offsets a flat array of floats would, so the two backends copy the packed
// material in without writing padding around anything.
struct Paint {
    // Up to four stops. Unused entries are ignored rather than blended.
    stops: array<vec4<f32>, 4>,
    // Position of each stop along the gradient, in order.
    offsets: vec4<f32>,
    // What the kind decides, in zw. Linear: the axis. Sweep: two angles.
    // Conical: two radii. A rounded rectangle or an ellipse: half its size, in
    // its own space. A blur or a morphology: the step between taps.
    //
    // The xy half is unclaimed for every kind that carries a mapping, because
    // the paint's origin used to live there and now rides inside `to_local`. A
    // mesh, which has no mapping, still reads x and y for its alpha and its
    // tile mode.
    geometry: vec4<f32>,
    // Clip space to the paint's own space, as three columns of a three by three
    // in column order, each padded to four.
    //
    // Clip space is anisotropic whenever the target is not square, and a
    // transform may rotate, skew, or carry perspective, so a circle in user
    // space is some other conic here. Measuring distance or angle directly in
    // clip space would therefore distort every radial and sweep gradient by the
    // aspect ratio. Mapping back first is what makes them correct under any
    // transform.
    //
    // Three by three rather than two by two because the paint's origin is
    // inside it. Subtracting an origin before a mapping is only the same as
    // mapping and then subtracting when the mapping is affine, and this one may
    // not be. An array of vectors rather than a matrix type because every
    // member of this block is a four-component vector on purpose: it is what
    // lets both backends copy the packed material in without writing padding.
    to_local: array<vec4<f32>, 3>,
    // x: number of stops in use. y: 0 solid, 1 linear, 2 radial, 3 sweep.
    params: vec4<f32>,
    // A color filter's matrix, by column: recolor[j] scales the input's jth
    // channel. Named for what it does rather than what it is, because `filter`
    // is a reserved word in this language. Stored by column rather than by row because that makes the
    // application four multiply-adds of vectors, and because a column is a
    // vec4 where a row of the five-wide form a caller writes is not.
    recolor: array<vec4<f32>, 4>,
    // The constant the filter adds.
    filter_offset: vec4<f32>,
    // x: 0 no filter, 1 a matrix on premultiplied color, 2 on straight color.
    filter_params: vec4<f32>,
};

// In its own group so that the texture bindings below, which are rebuilt per
// submission and include a long-lived placeholder set, are untouched by a
// buffer that changes with every draw.
@group(1) @binding(0) var<uniform> paint: Paint;

// One texture per draw. Always bound, even for a paint that does not sample it:
// the alternative is a pipeline variant per material kind, and a binding a
// pipeline declares but no draw supplies is invalid however unreachable the
// branch that would have read it.
@group(0) @binding(0) var image_texture: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) clip: vec3<f32>,
    // Where this vertex reads from a sampled texture. Zero for geometry that
    // samples nothing, which costs an interpolation nobody looks at.
    @location(1) uv: vec2<f32>,
    // A premultiplied color multiplied into whatever the material produced.
    // Opaque white for everything a caller did not color per vertex, and white
    // is the identity.
    @location(2) tint: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    // Depth is zero for every vertex here, and that is not merely because
    // nothing is stacked in Z. Both APIs test a primitive against the near
    // plane as part of their clip volume -- Vulkan against 0 <= z <= w, GL
    // against -w <= z <= w -- and with z pinned to zero each of those reduces
    // to w >= 0. So the rasterizer discards whatever fell behind the vanishing
    // line and cuts a triangle that straddles it, interpolating the new
    // vertices itself. Nothing on the processor has to find that plane.
    out.position = vec4<f32>(position.xy, 0.0, position.z);
    // Undivided, so that whatever scale perspective-correct interpolation
    // leaves on it arrives in every component alike. `to_gradient_space` is
    // where it comes back out.
    out.clip = position;
    out.uv = uv;
    out.tint = tint;
    return out;
}

/// Color at `t` along the stop list, with `count` stops in use.
fn sample_stops(t: f32, count: i32) -> vec4<f32> {
    var result: vec4<f32> = paint.stops[0];
    // Walk the stops rather than searching: at most four, and a loop with a
    // data-dependent exit costs more than the comparisons on the hardware this
    // targets.
    for (var i: i32 = 1; i < count; i = i + 1) {
        let lower = paint.offsets[i - 1];
        let upper = paint.offsets[i];
        let span = max(upper - lower, 1e-6);
        let local = clamp((t - lower) / span, 0.0, 1.0);
        // Only segments the parameter has reached contribute, so the last one
        // to apply is the segment it lies in.
        if (t >= lower) {
            result = mix(paint.stops[i - 1], paint.stops[i], local);
        }
    }
    return result;
}

/// Fold a gradient parameter into the ramp according to a tile mode.
///
/// Returns the parameter to sample with in `x`, and in `y` whether the sample
/// counts at all: decal draws nothing outside the ramp, and carrying that as a
/// multiplier keeps a second branch out of every caller.
///
/// The same three codes an image tiles by, because a caller sets both with one
/// `TileMode` and two meanings for one word would be worse than either.
fn tile_gradient(t: f32, tile: f32) -> vec2<f32> {
    if (tile > 0.5 && tile < 1.5) {
        // Repeat. `t - floor(t)` rather than `fract`, which is the same thing
        // for a finite input and says plainly what happens to a negative one:
        // -0.25 lands at 0.75 and the ramp runs on backwards without a seam.
        return vec2<f32>(t - floor(t), 1.0);
    }
    if (tile > 1.5 && tile < 2.5) {
        // Decal. Tested against the parameter as given, since the clamped one
        // is inside by construction.
        let inside = select(0.0, 1.0, t >= 0.0 && t <= 1.0);
        return vec2<f32>(clamp(t, 0.0, 1.0), inside);
    }
    if (tile > 2.5) {
        // Mirror: a period of two, folded in half. The parameter is reduced
        // modulo two and then reflected about one, so 1.25 comes back as 0.75
        // and the ramp runs backwards through the second half of every period.
        // The two ends of a copy meet the two ends of its neighbors, which is
        // the seam that repeating leaves behind.
        return vec2<f32>(1.0 - abs(1.0 - (t - 2.0 * floor(t * 0.5))), 1.0);
    }
    return vec2<f32>(clamp(t, 0.0, 1.0), 1.0);
}

/// The gradient's color at `t`, from wherever this paint keeps its colors.
///
/// Four stops or fewer travel in the material and are walked; more than that
/// were tabulated into a texture by the recorder and are read from it. The two
/// have to agree, so the walk above and the bake that produced the texture
/// implement the same rule, and this is the only place that chooses between
/// them.
///
/// The ramp is sampled at `t` directly rather than at a texel center: the
/// tabulation already placed its samples at centers, so the filter reading
/// between them reconstructs the function rather than shifting it.
///
/// A count of zero is what says the colors were tabulated. It used to be a
/// flag in the fourth parameter slot, which a conical gradient needed for the
/// distance between its two centers -- and a material carrying a ramp has no
/// stop count to report anyway, so the two never wanted separate words.
fn gradient_color(t: f32, count: i32) -> vec4<f32> {
    if (count <= 0) {
        // Straight color, not premultiplied, and already linear: the table
        // holds what the walk produced rather than an encoding of it, so the
        // sampler returns it unchanged. That is the same shape `sample_stops`
        // returns, so what follows does not need to know which path produced
        // it -- and now they agree about components the sRGB primaries cannot
        // hold as well, which an eight-bit encoded table could not carry.
        // See the note in `gradient_space`: a load, not the parameter.
        var at = t;
        return textureSampleLevel(image_texture, image_sampler, vec2<f32>(at, 0.5), 0.0);
    }
    return sample_stops(t, count);
}

/// Map a clip-space position into the paint's own space.
///
/// `to_local` is the whole inverse of what placed this draw's geometry, with the
/// paint's origin folded in, so what comes back is already measured from that
/// origin and the paint has nothing left to subtract. Folding is not tidiness:
/// under a mapping with perspective the image of an offset is not the offset of
/// the images, so an origin subtracted in clip space beforehand would be
/// subtracted in the wrong space -- correct only for the affine case that made
/// it look correct.
///
/// The argument is homogeneous rather than divided, and that is what keeps this
/// to a single reciprocal. Perspective-correct interpolation hands the fragment
/// stage the clip position scaled by some factor it chose; that factor lands in
/// the numerator and the denominator alike and divides straight back out. So
/// nothing has to undo it, and an affine draw -- whose bottom row is (0, 0, 1)
/// -- comes back exact whatever the hardware picked.
/// Where a gradient measures from, for a draw that may state its own
/// coordinates.
///
/// The same mapping either way, on a different homogeneous point: a mesh that
/// states texture coordinates has already placed each vertex in the caller's
/// space, and the material was built without the geometry's transform to match.
///
/// The select is on the *input* rather than on the result, and that is not
/// style. `select` evaluates both of its operands, so choosing between two
/// calls to the mapping below would run it twice for every fragment -- and this
/// is called from a gradient's arm rather than hoisted above the branch chain,
/// so a solid fill runs it no times rather than one. Both of those were wrong
/// in the first version of this, and together they cost three per cent of a
/// frame on a Raspberry Pi 5.
/// Where a rounded rectangle reads its clip-to-local mapping from.
///
/// Split out of `shade`'s arm so the `var` below has somewhere to live. See
/// the note in `gradient_space` for why it is a `var`.
fn rounded_rect_space(in: VertexOutput) -> vec3<f32> {
    var uv = in.uv;
    return select(in.clip, vec3<f32>(uv, 1.0), paint.geometry.x > 0.5);
}

fn gradient_space(in: VertexOutput) -> vec2<f32> {
    // Through a `var` so the operand is a load rather than the parameter's own
    // value. Vivante's SPIR-V decoder segfaults in
    // `vkCreate*Pipelines` on an `OpCompositeConstruct` in a called function
    // whose operand is a value parameter or a component extracted from one;
    // a load is not. `docs/on-a-board.md` has the rule and the probe that
    // established it, and `the_shader_builds_no_vector_from_a_parameter`
    // fails if one comes back.
    var uv = in.uv;
    let source = select(in.clip, vec3<f32>(uv, 1.0), paint.geometry.x > 0.5);
    return to_gradient_space(source);
}

fn to_gradient_space(clip: vec3<f32>) -> vec2<f32> {
    let mapped = paint.to_local[0].xyz * clip.x
        + paint.to_local[1].xyz * clip.y
        + paint.to_local[2].xyz * clip.z;
    // Beyond the vanishing line the divisor passes through zero, and a fragment
    // there has no position in the paint's space to report. Whole primitives
    // that fell behind it are already gone, the vertex stage having written a
    // depth of zero -- so what reaches here is the sliver a clipped edge
    // leaves. Clamping sends it far from the origin, which every material
    // downstream already has an answer for: coverage saturates to nothing, a
    // tile mode decides, an image decals or clamps. Dividing by zero would
    // answer with a NaN, and a NaN survives the blend and spreads.
    return mapped.xy / max(mapped.z, 1e-6);
}

/// Sample the bound texture at a clip-space position, premultiplied.
///
/// Unlike every other path here, this returns premultiplied color rather than
/// straight, because that is what it read: render targets store premultiplied
/// and an uploaded image is required to. Scaling the whole vector by the
/// paint's alpha keeps it premultiplied and is exact, where converting to
/// straight alpha and back would divide by an alpha that may be zero and lose
/// precision where it is merely small.
///
/// Getting this wrong is invisible until something samples a translucent
/// texture: with an opaque one the two conventions agree, so a layer nested in
/// another layer is the first thing that shows it.
/// Bring a texture coordinate inside the unit square, the way this paint asks.
///
/// Shared by the two materials that sample a texture at a coordinate -- an
/// image, which computes one from the fragment's position, and a mesh, which
/// is handed one per vertex. Two copies of this would be two chances for a
/// tiled image and a tiled mesh to disagree about what mirroring means.
///
/// Done here rather than through the sampler's address mode so that one
/// sampler serves every draw: the modes are a property of the paint, and
/// baking them into samplers would mean one sampler per combination.
/// Snap a coordinate to the center of the texel it falls in, if this paint
/// asked for nearest sampling.
///
/// The sampler stays linear for every draw -- see the note on tile modes for
/// why one sampler is worth keeping -- and a linear read taken exactly at a
/// texel's center has all its weight on that texel. So nearest sampling is a
/// coordinate adjustment rather than a second binding.
///
/// Sized from the texture rather than passed in, because the recorder that
/// built this material has never seen the texture and cannot know how large it
/// is.
fn snapped(coord: vec2<f32>) -> vec2<f32> {
    // Bounded on both sides. Written as `>= 0.5` it would be correct only
    // because the cubic path returns before reaching here, and the next
    // quality added would silently be snapped as well -- which is the mistake
    // the material kinds in this file already made once.
    if (paint.params.z < 0.5 || paint.params.z > 1.5) {
        return coord;
    }
    let size = vec2<f32>(textureDimensions(image_texture));
    return (floor(coord * size) + vec2<f32>(0.5)) / size;
}

/// One axis of the Mitchell-Netravali curve, `x` texels from the sample.
///
/// The family is stated with its two parameters rather than folded into
/// constants, because which member of it this is has a reason: `B` and `C` both
/// a third is what Skia's high quality means, and a caller porting from Flutter
/// is expecting that curve and not the sharper Catmull-Rom or the softer pure
/// B-spline. Reading the coefficients back out of six folded numbers would be
/// no way to check that.
fn cubic_weight(x: f32) -> f32 {
    let b = 1.0 / 3.0;
    let c = 1.0 / 3.0;
    let t = abs(x);
    if (t < 1.0) {
        let cubic = 12.0 - 9.0 * b - 6.0 * c;
        let square = -18.0 + 12.0 * b + 6.0 * c;
        return ((cubic * t + square) * t * t + (6.0 - 2.0 * b)) / 6.0;
    }
    if (t < 2.0) {
        let cubic = -b - 6.0 * c;
        let square = 6.0 * b + 30.0 * c;
        let linear = -12.0 * b - 48.0 * c;
        return (((cubic * t + square) * t + linear) * t + (8.0 * b + 24.0 * c)) / 6.0;
    }
    return 0.0;
}

/// A bicubic read of the bound texture, over the sixteen texels around `coord`.
///
/// The weights are a partition of unity -- they sum to one wherever the sample
/// falls -- so nothing is normalized afterward and a flat image stays exactly
/// flat. What they are not is non-negative: between one and two texels out the
/// curve dips below zero, which is what sharpens an edge and what makes a
/// bright edge overshoot into the dark side of it.
///
/// That overshoot is suppressed at the end, and this is the one place in this
/// shader where a color is still held inside the sRGB primaries' triangle. The
/// reason is that here, and only here, the thing exceeding the range is an
/// artifact rather than a color: a reconstruction kernel with negative lobes
/// invents values no texel it read contains. Everywhere else a component past
/// one is a color a caller asked for, and is carried.
///
/// The two cannot be told apart by looking. At this point a ringing overshoot
/// and a component describing a color outside the triangle are the same number,
/// and the source's own range is not knowable here -- so suppressing the first
/// means suppressing the second with it. What that costs is stated rather than
/// hidden: an image whose colors leave the sRGB primaries, read at this
/// sampling quality, is brought back inside them. `Sampling::Linear` has no
/// negative lobes, needs no such clamp, and carries the full range.
///
/// Clamping instead to the range of the sixteen texels read was the obvious
/// narrower answer and is worse: it removes exactly the overshoot that does the
/// sharpening, which is this filter's whole reason for existing.
fn cubic(coord: vec2<f32>, low: vec2<f32>, high: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(image_texture));
    // Texel centers sit at half-integers, so the coordinate is shifted by half
    // a texel before flooring. Without the shift the sixteen texels chosen are
    // the ones around a corner rather than around the sample.
    let position = coord * size - vec2<f32>(0.5);
    let base = floor(position);
    let offset = position - base;

    var total = vec4<f32>(0.0);
    for (var j = -1; j <= 2; j = j + 1) {
        let wy = cubic_weight(f32(j) - offset.y);
        for (var i = -1; i <= 2; i = i + 1) {
            let weight = cubic_weight(f32(i) - offset.x) * wy;
            // Held inside whatever selection the caller made, exactly as the
            // linear path is: a cubic read four texels wide would otherwise
            // reach two texels into a sprite's neighbors where a linear one
            // reaches half of one.
            let texel = clamp(
                (base + vec2<f32>(f32(i), f32(j)) + vec2<f32>(0.5)) / size,
                low,
                high,
            );
            total = total + textureSampleLevel(image_texture, image_sampler, texel, 0.0) * weight;
        }
    }
    let alpha = clamp(total.a, 0.0, 1.0);
    return vec4<f32>(clamp(total.rgb, vec3<f32>(0.0), vec3<f32>(alpha)), alpha);
}

/// The mip level a coordinate moving this fast across the screen should read.
///
/// `texels` is the coordinate in the texture's own texels rather than in the
/// zero-to-one space the sampler takes, because the question is how many texels
/// one screen pixel covers and the answer is that derivative. The larger of the
/// two screen axes decides it, which is the isotropic choice every fixed
/// function pipeline makes: an image minified hard along one axis and not the
/// other comes out blurrier than it needs to be, and the alternative is
/// anisotropic filtering, which is a sampler feature rather than something a
/// fragment can reconstruct from two derivatives.
///
/// Magnifying gives a negative number, which is not a level. Both APIs clamp it
/// to the largest one and would do the right thing with it, so the clamp here
/// is not observable and no test can distinguish it -- it is one instruction
/// spent on not depending on two different specifications agreeing about a
/// value out of range. The floor inside the logarithm is load-bearing in a way
/// the outer clamp is not: a coordinate that does not move at all, which is
/// what a degenerate triangle gives, would otherwise take the logarithm of
/// zero.
fn level_of(texels: vec2<f32>) -> f32 {
    let per_pixel = max(length(dpdx(texels)), length(dpdy(texels)));
    return max(log2(max(per_pixel, 1e-6)), 0.0);
}

/// One texture read, at whatever quality this paint asked for.
///
/// `low` and `high` bound the coordinates any tap may use, which is how a
/// sprite selection keeps its neighbors out. The linear and nearest paths were
/// already inside by construction and pass their own bounds through unused.
///
/// `texels` is the same place expressed in the texture's own texels and taken
/// *before* any tiling was applied. Only the mipmapped path reads it, and it
/// has to be the untiled one: a repeat wraps with `fract`, whose derivative at
/// the seam is the width of the whole image, and a level chosen from that is
/// the smallest one in the chain -- a blurred line down every seam, which is
/// the classic way this is got wrong.
fn sampled(coord: vec2<f32>, texels: vec2<f32>, low: vec2<f32>, high: vec2<f32>) -> vec4<f32> {
    if (paint.params.z > 2.5) {
        return textureSampleLevel(image_texture, image_sampler, coord, level_of(texels));
    }
    if (paint.params.z > 1.5) {
        return cubic(coord, low, high);
    }
    // Linear and nearest are one read between them: the sampler stays linear,
    // and nearest is the coordinate snapped to a texel center, where a linear
    // filter has all its weight on one texel.
    return textureSampleLevel(image_texture, image_sampler, snapped(coord), 0.0);
}

fn tile_uv(uv: vec2<f32>, tile: f32) -> vec2<f32> {
    if (tile > 0.5 && tile < 1.5) {
        // Repeat.
        return fract(uv);
    }
    if (tile > 2.5) {
        // Mirror, per axis: the same fold a gradient uses, applied to each
        // component so a tiled image meets its neighbors on both edges.
        return vec2<f32>(1.0) - abs(vec2<f32>(1.0) - (uv - 2.0 * floor(uv * 0.5)));
    }
    return clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
}

/// A texture read at coordinates the vertices carried, tinted and scaled.
///
/// What a sprite batch draws with. There is no mapping to undo and no source
/// rectangle to apply -- a caller who states a coordinate per vertex has
/// already said which part of the sheet each corner reads -- so this is the
/// image path with everything the vertices already answered taken out.
fn sample_mesh(uv: vec2<f32>) -> vec4<f32> {
    let tile = paint.geometry.y;
    var texel = sampled(
        tile_uv(uv, tile),
        uv * vec2<f32>(textureDimensions(image_texture)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    if (tile > 1.5 && tile < 2.5) {
        // Decal, tested against the coordinate as given, since the tiled one
        // is inside by construction.
        if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
            texel = vec4<f32>(0.0);
        }
    }
    let tint = paint.stops[0];
    let premultiplied = vec4<f32>(tint.rgb * tint.a, tint.a);
    return texel * premultiplied * paint.geometry.x;
}

fn sample_image(clip: vec3<f32>) -> vec4<f32> {
    // The same mapping a radial gradient uses, so an image lands correctly on a
    // target that is not square and under a transform that rotates or scales.
    let uv = to_gradient_space(clip);
    let tile = paint.geometry.w;

    var coord = tile_uv(uv, tile);

    // Into the piece of the texture this paint selected. Applied after the
    // tiling above, so `coord` has already been wrapped within one repetition
    // of the sprite and this maps that repetition onto the sprite's own texels.
    // Mapping first and tiling afterwards would wrap across the whole sheet and
    // draw the neighbors.
    let source = paint.stops[0];
    coord = source.xy + coord * (source.zw - source.xy);

    // Held half a texel inside the selection, because the filter reads two
    // texels and blends them. Without this a sprite shows a seam of whatever
    // sits next to it on the sheet: the edge of a selection falls exactly on a
    // texel boundary, and every sample within half a texel of it mixes in the
    // neighbor. Clamping to texel centers is what makes a selection actually
    // select.
    //
    // For the whole image this changes nothing, because the sampler already
    // clamps to the edge and reading the border texel twice is what it does.
    let half_texel = 0.5 / vec2<f32>(textureDimensions(image_texture));
    let low = min(source.xy + half_texel, source.zw - half_texel);
    let high = max(source.xy + half_texel, source.zw - half_texel);
    coord = clamp(coord, low, high);

    // In texels, from the coordinate before it was tiled and after the source
    // rectangle scaled it: a sprite selected from a sheet covers as many texels
    // as the selection is wide, not as many as the sheet is.
    let size = vec2<f32>(textureDimensions(image_texture));
    let texels = (source.xy + uv * (source.zw - source.xy)) * size;
    var texel = sampled(coord, texels, low, high);
    if (tile > 1.5 && tile < 2.5) {
        // Decal: nothing outside the image's own bounds. Tested against the
        // unclamped coordinate, since the clamped one is inside by
        // construction.
        let outside = any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0));
        if (outside) {
            texel = vec4<f32>(0.0);
        }
    }
    // The tint, stated straight and premultiplied here so it can scale a texel
    // that already is: the color is multiplied by the tint's alpha as well as
    // by its color, which is what keeps the product premultiplied instead of
    // merely close. A white opaque tint is the identity, which is what every
    // image that never mentions one carries.
    let tint = paint.stops[1];
    let premultiplied = vec4<f32>(tint.rgb * tint.a, tint.a);
    return texel * premultiplied * paint.geometry.z;
}

/// Narrow a field to its own outline, where the paint asked for one.
///
/// An outline is the band where the distance is small, so tracing one costs a
/// field nothing that it had not already computed. Tessellating an outline
/// means building a second shape -- offset inward and outward with the corners
/// resolved -- which is where a stroked path gets its vertices and its joins.
///
/// Shared by every shape here so the three cannot drift apart on what a width
/// means: it is the full width, centered on the edge, in the shape's own space.
/// Narrow a field to its own outline, where the paint asked for one.
///
/// An outline is the band where the distance is small, so tracing one costs a
/// field nothing it had not already computed. Tessellating one means building a
/// second shape -- offset inward and outward with the corners resolved -- which
/// is where a stroked path gets its vertices and its joins.
///
/// Both arguments are in the shape's own space, which is where a width is
/// stated. Taking the absolute value here rather than after normalizing is
/// deliberate: the gradient is taken of the field before this, because `abs`
/// puts a crease exactly on the curve and a derivative across it would be the
/// one place the edge is measured wrongly.
fn coverage_of(distance: f32, per_pixel: f32, width: f32) -> f32 {
    // Everything inside the edge, which is the whole answer for a fill.
    let inside = clamp(0.5 - distance / per_pixel, 0.0, 1.0);
    if (width <= 0.0) {
        return inside;
    }
    // An outline is a band, and a band is the difference of two half-planes:
    // what is inside its outer edge, less what is inside its inner one. Taking
    // the absolute value of the distance and subtracting half the width says
    // the same thing for a wide band and the wrong thing for a narrow one --
    // once the two ramps overlap, `abs` folds them into a plateau where this
    // subtracts them into the partial coverage they actually make. It also
    // avoids a crease exactly on the curve, which is where two devices'
    // derivative estimates diverge most.
    let outer = clamp(0.5 - (distance - width * 0.5) / per_pixel, 0.0, 1.0);
    let inner = clamp(0.5 - (distance + width * 0.5) / per_pixel, 0.0, 1.0);
    return outer - inner;
}

/// Signed distance from a rounded rectangle, negative inside.
///
/// The standard formulation: fold into one quadrant by symmetry, measure to
/// the inset box, and subtract the radius. `max(q, 0)` is the distance outside
/// along each axis and `min(max(q.x, q.y), 0)` the distance inside along the
/// nearer one, so the two cases share an expression rather than a branch.
fn rounded_rect_distance(point: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    // See the note in `gradient_space`: a load, not the parameter.
    var r = radius;
    let q = abs(point) - half_size + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - radius;
}

/// The error function, to about seven digits, as Raph Levien's method uses it.
///
/// `erf` is not in WGSL and the exact function has no elementary form, so this
/// is the rational approximation upstream's `computeErf7` uses -- an odd
/// polynomial in `x` normalized by `sqrt(1 + x^2)`, which is odd, saturating at
/// plus and minus one, and exact at zero, all of which the real `erf` is too.
///
/// The name is upstream's: seven terms, not seven digits.
fn erf7(value: f32) -> f32 {
    let two_over_sqrt_pi = 1.1283791670955126;
    let x = value * two_over_sqrt_pi;
    let xx = x * x;
    let series = x + (0.24295 + (0.03395 + 0.0104 * xx) * xx) * (x * xx);
    return series / sqrt(1.0 + series * series);
}

/// The length formula with an exponent other than two.
///
/// At two this is the distance to a circular corner. Larger, the level sets
/// square off, which is what makes a blurred corner's profile match a
/// Gaussian's -- a blurred corner is not a blurred circle.
fn power_distance(point: vec2<f32>, exponent: f32, exponent_inv: f32) -> f32 {
    let xp = pow(point.x, exponent);
    let yp = pow(point.y, exponent);
    return pow(xp + yp, exponent_inv);
}

/// A blurred rounded rectangle, evaluated rather than blurred.
///
/// The exact convolution of a Gaussian with a rounded rectangle has no closed
/// form. This is Raph Levien's approximation, which upstream evaluates too: the
/// blur along one axis is the difference of two error functions -- the shape's
/// two edges seen through the Gaussian -- and the corners are folded in by
/// measuring the distance to them with an exponent other than two.
///
/// Every constant it needs was computed on the CPU, in the shape's own space,
/// which is why there is so little arithmetic here for what it draws. The one
/// thing left is `exponent_inv`: a reciprocal costs less than the float it
/// would take to carry, and the block is already borrowing a slot from the
/// stop positions.
fn rrect_blur_coverage(clip: vec3<f32>) -> vec4<f32> {
    let point = to_gradient_space(clip);
    let adjust = paint.geometry.xy;
    let s_inv = paint.geometry.z;
    let min_edge = paint.geometry.w;
    let scale = paint.offsets[0];
    let r1 = paint.params.z;
    let exponent = paint.params.w;

    // Folded into one quadrant: the shape is symmetric about both axes of its
    // own space, and `to_local` already put its center at the origin.
    let centered = abs(point);

    // Distance to the rounded rectangle, with the corner measured by the
    // power distance and the straight edges by the nearer one. Outside the
    // corner's quadrant `adjusted` goes negative on an axis and that axis
    // drops out, which is what makes one expression serve edges and corners.
    let adjusted = centered - adjust;
    let outside = power_distance(max(adjusted, vec2<f32>(0.0)), exponent, 1.0 / exponent);
    let inside = min(max(adjusted.x, adjusted.y), 0.0);
    let distance = outside + inside - r1;

    // The Gaussian's integral between the two edges, which is the blur.
    let coverage = scale * (erf7(s_inv * (min_edge + distance)) - erf7(s_inv * distance));

    // The clamp is a guard rather than part of the expression, and upstream
    // does without it. In the domain the CPU side produces it cannot bite:
    // `min_edge` is positive and `erf7` is monotonic, so the difference is
    // never negative, and `scale` is half an `erf7` and so never above a half
    // while the difference is never above two. It is here because this writes
    // a premultiplied color, where a coverage above one would put color past
    // alpha and show up as a bright fringe rather than as an obviously wrong
    // picture. If it ever does bite, something on the CPU side is wrong and
    // this hides it -- so it is worth removing to look, rather than trusting.
    return paint.stops[0] * clamp(coverage, 0.0, 1.0);
}

/// A rounded rectangle evaluated here rather than built out of triangles.
///
/// Two triangles cover it whatever the radius, where tessellating costs
/// vertices in proportion to how round it is. The edge antialiases from the
/// distance the shape already computes, which is a better edge than
/// multisampling gives and costs one sample rather than four.
fn rounded_rect_coverage(clip: vec3<f32>) -> vec4<f32> {
    let point = to_gradient_space(clip);
    let half_size = paint.geometry.zw;
    let radius = clamp(paint.params.z, 0.0, min(half_size.x, half_size.y));
    let stroke = max(paint.params.w, 0.0);

    // A fill and an outline are one expression, differing in what the shape is
    // and whether a second one is taken out of it.
    //
    // An outline is the difference of two offset shapes rather than a band
    // around one, and the difference is the corner. A band takes the distance
    // to the outline and asks for the points within half a width of it;
    // outside a square corner that distance is the Euclidean distance to the
    // vertex, so the band's outer edge there is an *arc*. `dart:ui`'s default
    // join is a miter, which is sharp -- and that is why a square-cornered
    // stroke used to be refused here and sent to the tessellator.
    //
    // Offsetting the shape says the other thing. A rectangle grown by half a
    // width is a rectangle, its corner still square, and the zero set of its
    // own field is exactly the miter. For a rounded corner the two agree --
    // growing a rounded rectangle by `d` grows its radius by `d` -- so nothing
    // that was already analytic changes shape. At a stroke of zero the growth
    // is zero and this is the shape itself, which is the fill.
    let half = stroke * 0.5;
    // The outer radius is carried rather than derived for a stroke, because
    // the join decides it and the shader cannot see the join; see the field's
    // note on `Material::RoundedRect`. Both operands are values already in
    // hand, so selecting between them costs nothing -- `select` evaluates both
    // sides, which is only a trap when a side is work.
    let grown = select(radius, paint.geometry.y, stroke > 0.0);
    let outer = rounded_rect_distance(point, half_size + half, grown);

    // How much the distance changes across one pixel, which is what turns a
    // distance into a coverage. Taken from the derivative rather than passed
    // in, so it stays right under a transform the paint never sees -- a shape
    // scaled up antialiases over the same pixel, not the same unit.
    //
    // The gradient's magnitude, not `fwidth`. `fwidth` sums the two partial
    // derivatives, which is their L1 norm and over-estimates the true rate by
    // up to the square root of two -- and by much more under a transform that
    // scales the axes differently, where it adds a large derivative to a small
    // one. The result is an edge softer than a pixel, which reads as a blurry
    // shape rather than an antialiased one.
    let gradient = vec2<f32>(dpdx(outer), dpdy(outer));
    let per_pixel = max(length(gradient), 1e-6);
    // Half a pixel each way. A pixel whose center sits on the edge is half
    // covered, which is what the linear ramp says at distance zero.
    var coverage = clamp(0.5 - outer / per_pixel, 0.0, 1.0);

    // And for an outline, what is inside the inner shape comes back out. The
    // inner shape vanishes once the stroke is wider than the shape it outlines,
    // and a vanished shape has no inside: left to `max` alone the degenerate
    // box would be a point, and a point half a pixel across is a dot in the
    // middle of a thick outline.
    let shrunk = half_size - half;
    if (stroke > 0.0 && min(shrunk.x, shrunk.y) > 0.0) {
        let inner = rounded_rect_distance(point, shrunk, max(radius - half, 0.0));
        coverage -= clamp(0.5 - inner / per_pixel, 0.0, 1.0);
    }

    let tint = paint.stops[0];
    let alpha = tint.a * coverage;
    return vec4<f32>(tint.rgb * alpha, alpha);
}

/// An ellipse evaluated here, with coverage from an approximate distance.
///
/// There is no closed form for the exact distance to an ellipse. There is one
/// for a good approximation of it: the implicit function divided by the length
/// of its own gradient, which is the first-order estimate of how far away the
/// curve is. It costs two lengths and a divide, no iteration, and it is nearly
/// exact where it matters -- within a pixel of the boundary, which is the only
/// place coverage is between nothing and all of it. Measured against a densely
/// sampled true distance it is within a thousandth of a unit there, and drifts
/// only well inside, where coverage has saturated and nothing can see it.
fn ellipse_coverage(clip: vec3<f32>) -> vec4<f32> {
    return disc_coverage(to_gradient_space(clip), max(paint.geometry.zw, vec2<f32>(1e-6)));
}

/// Every point of a field, from one draw.
///
/// The same field an ellipse uses and the same arithmetic, with the position
/// coming from the vertices rather than from the paint. A field of points is
/// one shape repeated at many centers, and the center is the only thing that
/// differs between them -- so an ellipse's mapping, which carries its center
/// in `to_local` and therefore differs per point, is exactly what stops one
/// such draw from merging with the next.
///
/// Each point's quad carries the unit circle's corners as its texture
/// coordinates, so the interpolated value *is* the position in the shape's own
/// space and this reads it without knowing which point it belongs to. That is
/// what turns a thousand draws into one while keeping the edge each of them
/// had: `disc_coverage` never forms a distance, it differentiates the implicit
/// function across the pixel, and the derivative of an interpolated value is
/// exactly as available as that of a computed one.
fn point_field_coverage(uv: vec2<f32>) -> vec4<f32> {
    return disc_coverage(uv, vec2<f32>(1.0, 1.0));
}

/// The body both share: an implicit ellipse, differentiated into coverage.
fn disc_coverage(point: vec2<f32>, axes: vec2<f32>) -> vec4<f32> {
    // The implicit function itself: zero on the curve, negative inside and
    // positive outside, in no particular units.
    let implicit = length(point / axes) - 1.0;
    // How fast it changes across a pixel, which is what turns it into pixels
    // without ever forming a distance. Taken before the outline, since `abs`
    // creases on the curve.
    let gradient = vec2<f32>(dpdx(implicit), dpdy(implicit));
    let per_pixel = max(length(gradient), 1e-6);

    var stroke = paint.params.w;
    if (stroke > 0.0) {
        // A width is stated in the shape's own space and this function is in
        // none, so it is converted by how fast the function changes per unit
        // there -- which for this one is the ratio of the two lengths below.
        let k1 = max(length(point / axes), 1e-6);
        let k2 = length(point / (axes * axes));
        stroke = stroke * k2 / k1;
    }
    let coverage = coverage_of(implicit, per_pixel, stroke);
    let tint = paint.stops[0];
    let alpha = tint.a * coverage;
    return vec4<f32>(tint.rgb * alpha, alpha);
}

/// One axis of a separable Gaussian blur of the bound texture.
///
/// Two passes of this give the same result as a square of taps, because a
/// two-dimensional Gaussian is the product of two one-dimensional ones -- at a
/// radius of sixteen, thirty-three taps against a thousand and eighty-nine.
///
/// The weights are computed rather than looked up. A table would need a size
/// chosen in advance and a branch per size; an exponential per tap is a handful
/// of instructions on hardware that has one, and this is bandwidth-bound long
/// before it is arithmetic-bound.
fn blur_along_axis(clip: vec3<f32>) -> vec4<f32> {
    let uv = to_gradient_space(clip);
    let step = paint.geometry.zw;
    let sigma = max(paint.params.z, 1e-4);
    // How far the taps go, and it is upstream's rule rather than a choice made
    // here. `CalculateBlurRadius` is `Radius(Sigma(sigma))`, which is
    // `(sigma - 0.5) * sqrt(3)` -- about 1.732 deviations, where three would
    // cover better than four nines of the curve and this covers about 91.67
    // percent of it.
    //
    // That truncation is not invisible and is not meant to be: renormalizing
    // what is left of the curve leaves an effective deviation of 0.8146 rather
    // than 0.9866, so a blur asked for by deviation is narrower than a true
    // Gaussian of that deviation. Matching the number upstream produces for a
    // given sigma is the point; producing the best Gaussian for that sigma is
    // not, and doing the latter made every blur here about seventeen percent
    // wider than the same request gives upstream.
    //
    // Below half a deviation upstream's radius goes to zero rather than
    // negative, which leaves the single center tap and no blur.
    let reach = max((sigma - 0.5) * 1.7320508, 0.0);
    // A shader loop must be bounded, and sixty-five taps is already a lot of
    // bandwidth per pixel per pass.
    let max_taps = 32.0;
    // One tap per texel while that fits within the budget, and further apart
    // once it does not. Spreading rather than truncating is what keeps a large
    // blur soft: cut off at the budget, the taps stop covering the curve and
    // the result stops getting softer however large sigma grows -- the shape
    // creeps toward a box the wider it is asked to be. Spread, the taps still
    // span the whole radius and the sampler's bilinear filter averages the
    // texels each one falls between, which is what makes the gaps cost quality
    // rather than correctness.
    //
    // Upstream reaches the same budget differently, by downsampling the source
    // for a large blur rather than by spreading taps across it. That is a
    // separate divergence and is recorded as one; what is matched here is the
    // width of the kernel, which is what decides how wide the blur looks.
    let spread = max(reach / max_taps, 1.0);
    let taps = min(ceil(reach / spread), max_taps);

    // Hoisted out of the loop, which the compiler may or may not do for a
    // divide by a uniform expression.
    let denominator = -0.5 / (sigma * sigma);
    var total = vec4<f32>(0.0);
    var weight_sum = 0.0;
    var i = -taps;
    loop {
        if (i > taps) { break; }
        // Weighted by where the tap actually lands, not by which tap it is.
        let offset = i * spread;
        let weight = exp(offset * offset * denominator);
        // Clamped, so the edge extends rather than the blur pulling in
        // transparent black from outside and darkening the border. The target
        // is sized to the content, so there is nothing outside worth reading.
        let coord = clamp(uv + step * offset, vec2<f32>(0.0), vec2<f32>(1.0));
        total = total + textureSampleLevel(image_texture, image_sampler, coord, 0.0) * weight;
        weight_sum = weight_sum + weight;
        i = i + 1.0;
    }
    // Normalized by what was actually summed rather than by the analytic
    // integral, so a truncated tail does not darken the result.
    return total / max(weight_sum, 1e-6);
}

/// The bound texture at `uv`, or transparent black outside it.
///
/// The blur clamps to the edge instead, and is right to: it is a weighted
/// average, so reading transparent black from outside would darken every
/// border pixel. A morphological filter is an extremum rather than an average,
/// and the two choices differ only for erosion -- the largest of a value and
/// transparent black is the value, so a dilation cannot tell them apart. For
/// erosion the choice is the whole behavior: clamped to the edge, a shape
/// sitting against the bound would never be eaten into from that side, because
/// the samples reaching past it would come back as more of the shape.
fn sample_or_nothing(uv: vec2<f32>) -> vec4<f32> {
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(image_texture, image_sampler, uv, 0.0);
}

/// One axis of a separable morphological filter of the bound texture.
///
/// Per channel on premultiplied color, which is what makes a dilation of a
/// translucent shape spread the color and the coverage together rather than
/// pulling color out from under an alpha that did not follow it.
///
/// Every tap is a whole texel away. There is no weighting to interpolate
/// between them -- a structuring element is a set of positions, not a curve --
/// so a fractional step would only sample the bilinear blend of two texels and
/// call the result a maximum of them, which it is not.
fn morphology_along_axis(clip: vec3<f32>) -> vec4<f32> {
    let uv = to_gradient_space(clip);
    let step = paint.geometry.zw;
    let taps = paint.params.z;
    let dilate = paint.params.w > 0.5;

    var best = sample_or_nothing(uv);
    var i = 1.0;
    loop {
        if (i > taps) { break; }
        let ahead = sample_or_nothing(uv + step * i);
        let behind = sample_or_nothing(uv - step * i);
        if (dilate) {
            best = max(best, max(ahead, behind));
        } else {
            best = min(best, min(ahead, behind));
        }
        i = i + 1.0;
    }
    return best;
}

/// Apply this paint's color filter, if it has one.
///
/// Takes and returns premultiplied color, because that is what every path
/// through `shade` produces and what the blend equations expect. A filter
/// stated on straight color -- which is how a color matrix is written, and how
/// `dart:ui` defines one -- has the alpha divided out first and put back after.
///
/// Only affine filters exist here, so this is one matrix and no branching on
/// what the filter means. What that costs is that the blend modes offered as
/// filters are the ones affine in their other operand; deciding which those
/// are happens on the processor, where it is a table rather than a branch per
/// fragment.
/// Linear light encoded into sRGB, the standard piecewise curve, extended below
/// zero by odd symmetry.
///
/// The standard states the curve on zero to one and states the split between
/// its two segments in terms of the value, which reads as a signed comparison
/// and is not one: which segment applies is decided by how far from zero a
/// value is, not by which side of zero it falls. Compared signed, every
/// negative component took the near-black linear segment, which sends -0.042 to
/// -0.543 where the curve's own odd extension sends it to -0.227.
///
/// Splitting on the magnitude also removes the floor this needed before. `pow`
/// of a negative base is undefined, and the base here is now a magnitude, so
/// there is nothing left to guard against.
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let m = abs(c);
    let low = m * 12.92;
    let high = 1.055 * pow(m, vec3<f32>(1.0 / 2.4)) - 0.055;
    return sign(c) * select(high, low, m <= vec3<f32>(0.0031308));
}

/// sRGB decoded back to linear light, the inverse of the curve above.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let m = abs(c);
    let low = m / 12.92;
    let high = pow((m + 0.055) / 1.055, vec3<f32>(2.4));
    return sign(c) * select(high, low, m <= vec3<f32>(0.04045));
}

fn filtered(premultiplied: vec4<f32>) -> vec4<f32> {
    let kind = paint.filter_params.x;
    if (kind < 0.5) {
        return premultiplied;
    }

    // A blend against a constant, which the compositing specification states
    // on premultiplied operands and `blend_tint` takes that way. Handled before
    // the straight/premultiplied split below because it is on the far side of
    // it: it is the one filter above the matrix that does not want straight
    // color, which is what `filter::is_straight` says by bounding its range.
    if (kind > 4.5) {
        return blend_tint(
            i32(paint.recolor[0].x + 0.5),
            paint.filter_offset,
            premultiplied,
        );
    }

    // Everything past the premultiplied matrix reads straight color -- the
    // gamma pair could not mean anything else -- so the test is a threshold
    // rather than a list. `filter::is_straight` draws the same line.
    let straight = kind > 1.5;
    var color = premultiplied;
    if (straight) {
        // A fully transparent pixel has no color to recover -- every channel
        // is zero whatever it was -- so the divisor is floored rather than
        // guarded, which gives zero and is the right answer.
        let alpha = max(color.a, 1e-6);
        color = vec4<f32>(color.rgb / alpha, color.a);
    }

    var out: vec4<f32>;
    if (kind > 2.5) {
        // The transfer function moves each channel along a curve and leaves
        // alpha where it is. Alpha is a coverage, not a color, and running it
        // through a color's encoding would make a half-covered pixel a
        // differently-covered one.
        if (kind < 3.5) {
            out = vec4<f32>(linear_to_srgb(color.rgb), color.a);
        } else {
            out = vec4<f32>(srgb_to_linear(color.rgb), color.a);
        }
    } else {
        out = paint.recolor[0] * color.r
            + paint.recolor[1] * color.g
            + paint.recolor[2] * color.b
            + paint.recolor[3] * color.a
            + paint.filter_offset;
    }

    if (straight) {
        // Alpha is coverage and cannot mean anything outside the unit range, so
        // it is still bounded. Color is not: a component outside that range
        // describes a color the sRGB primaries cannot hold rather than an
        // invalid one, and a color matrix is exactly the operation a caller
        // reaches for to produce one deliberately.
        let a = clamp(out.a, 0.0, 1.0);
        return vec4<f32>(out.rgb * a, a);
    }
    // Premultiplied already, so the invariant to keep is the one that form
    // actually carries. `rgb <= alpha` was two claims sharing a clamp: that a
    // straight component lies between zero and one, which is a statement about
    // the sRGB primaries' triangle and is the thing an extended range
    // deliberately gives up; and that a component is alpha times something
    // finite, which is still true and has exactly one visible consequence --
    // where alpha is zero the color is zero, because a finite number times zero
    // is zero. A pixel that is not covered at all still cannot be lit from
    // nowhere.
    let alpha = clamp(out.a, 0.0, 1.0);
    return vec4<f32>(select(out.rgb, vec3<f32>(0.0), alpha <= 0.0), alpha);
}

/// Hard light, which overlay is also built from.
fn hard_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) {
        return cb * (2.0 * cs);
    }
    let d = 2.0 * cs - 1.0;
    return cb + d - cb * d;
}

/// One channel of the separable blend function `B(Cb, Cs)`, unpremultiplied.
///
/// The compositing specification's definitions, and a transcription of the
/// same statement `separable_blend` makes on the CPU rather than a second
/// opinion about what the modes mean. The two are checked against each other,
/// which is only worth doing because neither was derived from the other.
fn separable_b(mode: i32, cb: f32, cs: f32) -> f32 {
    switch mode {
        // Multiply
        case 14: { return cb * cs; }
        // Screen
        case 15: { return cb + cs - cb * cs; }
        // Overlay is hard-light with the two sides exchanged.
        case 16: { return hard_light(cs, cb); }
        // Darken
        case 17: { return min(cb, cs); }
        // Lighten
        case 18: { return max(cb, cs); }
        // ColorDodge. The order of the cases is load-bearing: a black backdrop
        // stays black under a full-strength source, and only then does a
        // full-strength source saturate.
        case 19: {
            if (cb <= 0.0) { return 0.0; }
            if (cs >= 1.0) { return 1.0; }
            return min(cb / (1.0 - cs), 1.0);
        }
        // ColorBurn
        case 20: {
            if (cb >= 1.0) { return 1.0; }
            if (cs <= 0.0) { return 0.0; }
            return 1.0 - min((1.0 - cb) / cs, 1.0);
        }
        // HardLight
        case 21: { return hard_light(cb, cs); }
        // SoftLight
        case 22: {
            let d = select(
                (16.0 * cb - 12.0) * cb + 4.0,
                inverseSqrt(max(cb, 1e-8)),
                cb > 0.25,
            );
            let dd = select(d * cb, sqrt(max(cb, 0.0)), cb > 0.25);
            if (cs <= 0.5) {
                return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb);
            }
            return cb + (2.0 * cs - 1.0) * (dd - cb);
        }
        // Difference
        case 23: { return abs(cb - cs); }
        // Exclusion
        case 24: { return cb + cs - 2.0 * cb * cs; }
        default: { return cs; }
    }
}

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.3, 0.59, 0.11));
}

/// Move a color to a given luminosity, clipping back into range.
fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    let shifted = c + vec3<f32>(l - lum(c));
    let low = min(shifted.r, min(shifted.g, shifted.b));
    let high = max(shifted.r, max(shifted.g, shifted.b));
    let luma = lum(shifted);
    var out = shifted;
    // Clipping toward the luminosity rather than to the unit range, so a color
    // pushed out of gamut desaturates instead of shifting hue.
    if (low < 0.0) {
        out = luma + (out - luma) * luma / max(luma - low, 1e-8);
    }
    if (high > 1.0) {
        out = luma + (out - luma) * (1.0 - luma) / max(high - luma, 1e-8);
    }
    return out;
}

fn sat(c: vec3<f32>) -> f32 {
    return max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b));
}

/// Rescale to a saturation, keeping which channel is which.
///
/// By index rather than by sorting the components, because the result has to
/// go back where it came from: the middle channel of the input stays the
/// middle channel of the output.
fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let low = min(c.r, min(c.g, c.b));
    let high = max(c.r, max(c.g, c.b));
    if (high <= low) {
        // A flat color has no saturation to scale, so it stays flat at zero
        // rather than being given one arbitrarily.
        return vec3<f32>(0.0);
    }
    return (c - vec3<f32>(low)) * s / (high - low);
}

/// The non-separable blend functions, which mix the channels.
fn nonseparable_b(mode: i32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        // Hue
        case 25: { return set_lum(set_sat(cs, sat(cb)), lum(cb)); }
        // Saturation
        case 26: { return set_lum(set_sat(cb, sat(cs)), lum(cb)); }
        // Color
        case 27: { return set_lum(cs, lum(cb)); }
        // Luminosity
        case 28: { return set_lum(cb, lum(cs)); }
        default: { return cs; }
    }
}

/// Combine two colors this shader already holds, both premultiplied.
///
/// This is not the blending a target does. A fragment stage cannot read the
/// framebuffer, which is why the advanced modes against a *destination* need
/// an extension -- but neither of these colors is in the framebuffer. What a
/// caller attaches to a vertex or a sprite and what the paint produced are
/// both here, so combining them is arithmetic and every mode is available.
fn blend_tint(mode: i32, src: vec4<f32>, dst: vec4<f32>) -> vec4<f32> {
    // See the note in `gradient_space`. These two feed the splats in
    // `src.rgb / sa` and `dst.rgb / da`, which are the constructs.
    var sa = src.a;
    var da = dst.a;
    switch mode {
        case 0: { return vec4<f32>(0.0); }
        case 1: { return src; }
        case 2: { return dst; }
        case 3: { return src + dst * (1.0 - sa); }
        case 4: { return dst + src * (1.0 - da); }
        case 5: { return src * da; }
        case 6: { return dst * sa; }
        case 7: { return src * (1.0 - da); }
        case 8: { return dst * (1.0 - sa); }
        case 9: { return src * da + dst * (1.0 - sa); }
        case 10: { return dst * sa + src * (1.0 - da); }
        case 11: { return src * (1.0 - da) + dst * (1.0 - sa); }
        // Plus, clamped here rather than left to the target, which is a
        // deviation and is measured rather than assumed. The hardware path
        // reaches this mode as `One, One` and leaves the saturation to the
        // attachment, so removing this `min` is what makes the tint route agree
        // with the paint route on a floating-point target.
        //
        // It was removed for exactly that reason and put back after benching a
        // Pi 5: the line costs 2.4 per cent on the Vulkan distance-field row and
        // 1.2 on the GLES full frame, state for state, and buys agreement only
        // where a float target is in play -- which is tests, since nothing here
        // presents one. Paying that on every frame for a difference no shipped
        // configuration can see is the wrong way round. §13 of
        // `docs/non-parity.md` records it with the numbers.
        //
        // Removing an instruction made the shader *slower*, which is V3D's
        // register allocation rather than anything arithmetic; the same commit
        // is why `docs/architecture.md` says to bench both backends after
        // shader work.
        case 12: { return min(src + dst, vec4<f32>(1.0)); }
        case 13: { return src * dst; }
        default: {}
    }

    // Everything past here is an advanced mode, whose formulas the compositing
    // specification states on unpremultiplied color between zero and one and
    // does not define outside it. Overlay and hard light branch on a midpoint
    // that only means something there; color dodge and burn saturate at one;
    // soft light uses a curve defined on the unit interval; and the four
    // non-separable modes clip against a gamut their own definition states as
    // the unit cube.
    //
    // That domain is the specification's rather than this shader's, and the
    // hardware unit a paint's own blend mode reaches has the same one and
    // cannot be extended. So it is stated here, at the place it applies, rather
    // than enforced three functions upstream by a clamp that also had to gate
    // colors nothing was wrong with. An operand outside the sRGB primaries'
    // triangle is brought to the triangle's edge before the mode is evaluated.
    // The fourteen modes above are linear, have no such domain, and never reach
    // this line.
    let cs = clamp(
        select(src.rgb / sa, vec3<f32>(0.0), sa <= 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    let cb = clamp(
        select(dst.rgb / da, vec3<f32>(0.0), da <= 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    var mixed: vec3<f32>;
    if (mode >= 25) {
        mixed = nonseparable_b(mode, cb, cs);
    } else {
        mixed = vec3<f32>(
            separable_b(mode, cb.r, cs.r),
            separable_b(mode, cb.g, cs.g),
            separable_b(mode, cb.b, cs.b),
        );
    }
    // The general compositing formula: the source where the backdrop is not,
    // the backdrop where the source is not, and the blend where both are.
    let rgb = sa * (1.0 - da) * cs + sa * da * mixed + (1.0 - sa) * da * cb;
    return vec4<f32>(rgb, sa + da * (1.0 - sa));
}

/// An eight-by-eight ordered dither, in (-63/128, +63/128).
///
/// The matrix upstream Impeller uses, which took it from Skia, transcribed
/// rather than reinvented so that a gradient banded the same way comes apart
/// the same way. The bound stops just short of a half in either direction on
/// purpose: at exactly a half an already-representable value -- black, white --
/// would round to its neighbor instead of staying put.
fn ordered_dither(frag: vec2<f32>) -> f32 {
    let x = u32(frag.x) % 8u;
    let y = u32(frag.y) ^ x;
    let m = ((y & 1u) << 5u) | ((x & 1u) << 4u) | ((y & 2u) << 2u) | ((x & 2u) << 1u) |
            ((y & 4u) >> 1u) | ((x & 4u) >> 2u);
    return f32(m) * (2.0 / 128.0) - (63.0 / 128.0);
}

/// Perturb a color by a fraction of the target's quantization step.
///
/// Banding is what a smooth ramp becomes when neighboring pixels round to the
/// same representable value: the picture gains an edge the gradient does not
/// have. Offsetting each pixel by a fraction of a step first, by a rule that
/// varies across an eight-by-eight tile, makes the rounding land on both sides
/// along what would have been the edge, and the eye reads the mixture rather
/// than the boundary.
///
/// The amplitude and the space come from the target and are handed in through
/// the paint block, because a step is not one quantity. Into a linear surface
/// it is a fixed amount of light. Into an sRGB one the hardware encodes on
/// write, so the step is a step of encoded value, and the light it stands for
/// runs from about a thirtieth of that near black to twice it near white -- so
/// the offset is applied on the encoded side there, which is the side the
/// rounding happens on. Doing it in light with one amplitude would dither the
/// shadows thirty times too hard and the highlights not at all, which is
/// backwards: the shadows are where eight bits band.
///
/// Every gradient, whichever of this renderer's two paths drew it, which is
/// what upstream does on a device with shader storage buffers -- and that is
/// every device its Vulkan and Metal backends run on. Upstream has a stop
/// table in uniforms and a baked ramp texture as well, and neither of those
/// dithers, but both are fallbacks it reaches only without storage buffers or
/// past two hundred and fifty-six stops. Reading its ramp path as this
/// renderer's ramp path gets the mapping backwards: this one tabulates past
/// four stops, so matching that would leave almost every gradient here on the
/// side upstream almost never uses.
///
/// A gradient is the one thing here that asks a target for a long run of
/// nearly equal values, which is why it is the only thing dithered.
///
/// Applied last, to the premultiplied result, because premultiplied is what
/// the target stores and rounds. Alpha is left alone -- perturbing coverage
/// would move an edge rather than break a band.
fn dithered(color: vec4<f32>, frag: vec2<f32>) -> vec4<f32> {
    // The amplitude carries the whole decision: zero for a target with no
    // quantum to bridge and for every material that asks for no dither, which
    // the recorder decides because it knows the material. This used to test
    // `params.y` for a gradient kind as well, which made the route and not the
    // intent the thing being asked about -- see `Material::dithers`.
    let amplitude = paint.filter_params.z;
    if (amplitude <= 0.0) {
        return color;
    }
    let offset = ordered_dither(frag) * amplitude;
    // See the note in `gradient_space`: a load, not an extract of a parameter.
    var alpha = color.a;
    return vec4<f32>(color.rgb + vec3<f32>(offset), alpha);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The vertex color multiplies the material, and the filter applies to what
    // that produced -- which is the order `dart:ui` states: a color filter acts
    // on the paint's result, and a mesh's own colors are part of producing it.
    //
    // Both sides are premultiplied, so this is a componentwise product and
    // stays premultiplied. Multiplying by a straight color instead would leave
    // the alpha applied once to the color and twice to itself.
    // The caller's color is the source and the paint's result the backdrop,
    // which is the order `dart:ui` states for both of the calls that carry one.
    let shaded = shade(in);
    let tint_mode = i32(paint.filter_params.y + 0.5);
    // `Modulate` is the mode a draw that asked for nothing gets, white being
    // its identity, and it is `src * dst` outright -- the same line the switch
    // in `blend_tint` reaches for it. Written here as well so the common case
    // does not enter that function at all: it carries twenty-nine modes and the
    // non-separable tail behind them, and every fragment of every draw was
    // going through it to arrive at one multiply.
    var tinted: vec4<f32>;
    if (tint_mode == 13) {
        tinted = in.tint * shaded;
    } else {
        tinted = blend_tint(tint_mode, in.tint, shaded);
    }
    return dithered(filtered(tinted), in.position.xy);
}

/// The color this paint produces, premultiplied, before any filter.
fn shade(in: VertexOutput) -> vec4<f32> {
    var color: vec4<f32> = paint.stops[0];
    let kind = paint.params.y;
    // A solid color first, because it is the commonest material by a long way
    // and because the chain below is linear: falling through every arm to reach
    // the return at the bottom cost it two comparisons per kind, and the count
    // of kinds grows.
    if (kind < 0.5) {
        return vec4<f32>(color.rgb * color.a, color.a);
    }
    let count = i32(paint.params.x);
    // Where a gradient measures from. Almost always the fragment's position
    // carried back into the paint's own space; on a mesh that states texture
    // coordinates, the coordinates themselves.
    //
    // `dart:ui` reads any color source at a mesh's coordinates, not only an
    // image, and this is what makes a gradient one of them: the caller has said
    // where each vertex sits in the paint's space, so there is nothing to
    // derive. The flag lives in `geometry.x`, which no gradient kind writes --
    // slots in this block mean different things per kind, and `params.z` being
    // a tile mode here and a radius on an ellipse is the same arrangement.

    if (kind > 0.5 && kind < 1.5) {
        // Linear: project onto the axis in the gradient's own space rather than
        // in clip space. Clip space is normalized to the target, so projecting
        // there weights the two axes by the target's shape and runs a diagonal
        // gradient in the wrong direction on anything that is not square.
        //
        // The parameter is taken unclamped and folded afterwards, because what
        // happens past either end is the paint's to say.
        let axis = paint.geometry.zw;
        let length_squared = max(dot(axis, axis), 1e-6);
        let t = dot(gradient_space(in), axis) / length_squared;
        let tiled = tile_gradient(t, paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y;
    } else if (kind > 1.5 && kind < 2.5) {
        // Radial: distance in gradient space, where the radius is one.
        let tiled = tile_gradient(length(gradient_space(in)), paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y;
    } else if (kind > 2.5 && kind < 3.5) {
        // Sweep: angle about the center, measured in gradient space so an
        // anisotropic target does not bunch the stops on two sides.
        let local = gradient_space(in);
        let angle = atan2(local.y, local.x);
        let start_angle = paint.geometry.z;
        let sweep = max(paint.geometry.w - start_angle, 1e-6);
        // The angle is brought into one turn ahead of the start, which removes
        // the atan2 branch cut: that discontinuity is a property of how the
        // angle was computed and not of the gradient, so a sweep beginning at
        // any angle runs forward from there without a seam.
        //
        // Dividing by a full turn rather than by the sweep is the distinction
        // that makes a tile mode possible at all. Folding by the sweep would
        // send every direction back inside the arc, which *is* repeating, and
        // would leave clamp and decal with nothing outside to act on. Past the
        // end of a partial sweep the parameter now exceeds one, and what
        // happens there is the paint's to say.
        let delta = angle - start_angle;
        let turn = 6.28318530717958647692;
        let ahead = delta - turn * floor(delta / turn);
        let tiled = tile_gradient(ahead / sweep, paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y;
    } else if (kind > 8.5 && kind < 9.5) {
        // Conical: the parameter is where a point sits between two circles,
        // which is the general gradient the other two are special cases of. A
        // point lies on the circle interpolated at `t` when
        //
        //     |p - t*d| = r0 + t*dr
        //
        // and squaring that is a quadratic in `t`, which is what this solves.
        // The mapping put the first center at the origin and the second on the
        // positive x axis, so `d` is `(separation, 0)` and the dot products
        // below lose a term rather than needing one.
        let point = gradient_space(in);
        let separation = paint.params.w;
        let r0 = paint.geometry.z;
        let dr = paint.geometry.w;

        let a = separation * separation - dr * dr;
        let b = point.x * separation + r0 * dr;
        let c = dot(point, point) - r0 * r0;

        // Relative, because everything here is in the gradient's own units and
        // a fixed threshold would call a small gradient degenerate and a large
        // one well behaved for the same shape.
        let magnitude = max(separation * separation, dr * dr);

        var t = 0.0;
        var covered = false;
        if (abs(a) > magnitude * 1e-5) {
            let disc = b * b - a * c;
            if (disc >= 0.0) {
                let root = sqrt(disc);
                // Both roots are circles through the point; the larger `t` is
                // the one nearer the second circle and is what the gradient
                // means. A circle of negative radius is not one, so a root
                // producing one is rejected before the other is tried.
                let far = max((b + root) / a, (b - root) / a);
                let near = min((b + root) / a, (b - root) / a);
                if (r0 + far * dr >= 0.0) {
                    t = far;
                    covered = true;
                } else if (r0 + near * dr >= 0.0) {
                    t = near;
                    covered = true;
                }
            }
        } else if (abs(b) > 1e-6) {
            // The quadratic term vanishes when the two circles are tangent
            // internally -- the cone becomes a half-plane -- and what is left
            // is linear. Solving the quadratic here would divide by zero and
            // lose the one root that exists.
            let only = c / (2.0 * b);
            if (r0 + only * dr >= 0.0) {
                t = only;
                covered = true;
            }
        }

        // Outside the cone there is no circle through the point at all, and no
        // tile mode changes that: repeating a parameter that does not exist
        // would invent one. Drawing nothing is what the two circles say.
        let tiled = tile_gradient(t, paint.params.z);
        color = gradient_color(tiled.x, count) * tiled.y * select(0.0, 1.0, covered);
    }
    // Checked after the gradient chain rather than inside it, because the
    // sweep arm tests only a lower bound and would otherwise claim this kind
    // as well. Returned directly, since a sampled texel is premultiplied
    // already and the conversion below would apply alpha a second time.
    switch i32(kind + 0.5) {
        // 4
        case 4: { return sample_image(in.clip); }
        // 7, 8 -- the analytic shape fields
        // The one arm that takes its space from the vertex when asked. A
        // rounded rectangle carried its clip-to-local mapping in the material,
        // which made two of one shape in different places compare unequal and
        // cost a draw each; the mapping rides on `uv` instead when
        // `geometry.x` says so, and then they merge. The select is on this arm
        // rather than hoisted, so every other material still runs it no times.
        case 7: { return rounded_rect_coverage(rounded_rect_space(in)); }
        case 8: { return ellipse_coverage(in.clip); }
        case 12: { return rrect_blur_coverage(in.clip); }
        case 13: { return point_field_coverage(in.uv); }
        // Already premultiplied, like anything else sampled from a target, and
        // a weighted average of premultiplied colors is premultiplied.
        case 6: { return blur_along_axis(in.clip); }
        // Premultiplied for the same reason, and an extremum of premultiplied
        // colors taken channel by channel is one too: no channel can come out
        // above an alpha that no sample had.
        case 11: { return morphology_along_axis(in.clip); }
        // Already premultiplied, like anything else sampled from a texture.
        case 10: { return sample_mesh(in.uv); }
        default: {}
    }
    // Every one of these is bounded on both sides, so each arm claims its own
    // kind and nothing else. Written as descending open-ended tests they were
    // correct only because of the returns above them, which meant a kind added
    // later was claimed by all four: the conical gradient, kind nine, came out
    // as a rounded rectangle the first time it ran.
    if (kind > 4.5 && kind < 5.5) {
        // Coverage rather than color: one channel scaling a solid, which is
        // what an antialiased glyph is. The coordinates are the vertex's own,
        // so a run of glyphs reading different parts of one atlas needs one
        // draw and one paint between them.
        //
        // Read from the red channel, which is where a single-channel atlas
        // puts it and where a four-channel one repeats it.
        let coverage = textureSampleLevel(image_texture, image_sampler, in.uv, 0.0).r;
        let tint = paint.stops[0];
        let alpha = tint.a * coverage;
        return vec4<f32>(tint.rgb * alpha, alpha);
    }

    // Colors are linear here. Conversion to the target's transfer function is
    // the attachment format's job, not this shader's. Premultiplying is this
    // shader's job, because the blend equations expect it.
    return vec4<f32>(color.rgb * color.a, color.a);
}

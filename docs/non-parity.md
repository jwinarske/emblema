# Where this renderer knowingly differs from upstream Impeller

Upstream Impeller is the authority on *what* this renderer must express — the
`dart:ui` operations, their semantics, and the composition rules behind them. It
is not the authority on *how* that is reached. Rasterization strategy, sample
count and batching are answered against the devices this renderer targets, which
are not the devices upstream targets; and where upstream's own choice is an
artifact of its hardware or a known infelicity, this one may choose differently
and say so here.

That distinction is what separates the entries below from a list of debts. The
fill entry is the clearest case: upstream reaches a filled path through
stencil-then-cover and this renderer triangulates, because every pipeline here
compiles ahead of time and nothing requires compute, which is what lets one
binary serve any Vulkan 1.1 or GLES 3.0 device. Neither is a defect in the other.
Linear light is the other shape the distinction takes — a deliberate improvement
rather than a device constraint, and the reason a comparison of mixed colors can
only be a comparison of shape.

This file is the list of places the two differ, why, and what the difference
costs — so that a divergence is a decision somebody made and can find, rather
than something discovered later by whoever compares two pictures.

Two things this file is not. It is not the list of what is *unbuilt*: that is
[`parity.md`](parity.md) for the `dart:ui` surface and
[`playground-parity.md`](playground-parity.md) for the scenes. And it is not a
list of bugs — everything here is deliberate, and a difference that turns out
not to be deliberate belongs in a commit that removes it.

A fourth left it when the last of upstream's seven image filter kinds was
built. That entry had grown two paragraphs of scoping, and both turned out to
be arguing the job was larger than it was: the contract it worried about --
that a caller's program is handed its input as a texture with an identity
transform -- is one a filter pass here satisfies by construction, and the
storage question that looked like the real obstacle was answered by keeping the
program beside the layer rather than inside it. `Layer` is still `Copy` and
still a hundred and fifty-two bytes.

A third left it when a rounded rectangle here stopped having one radius. That
entry said the limit had never been a decision, only a generalization nobody had
written, and named where writing it would cost something -- the analytic route
is a signed distance to a shape with one circular radius, and eight numbers is a
different function rather than that one with more arguments. It was written the
way the entry said it should be: unequal corners tessellate, and a uniform
rounded rectangle still reaches the shader, including when a caller spells it
the general way.

Two entries left this file when the pipeline stopped working in light. Color was
linear here and encoded upstream, which was the deepest difference recorded and
the one most of the others followed from; and the dither's amplitude had to be
derived from the target because a step of the target was worth a different
amount of light at every brightness. Both are gone: the pipeline carries
sRGB-encoded components from the API boundary to the target write, as upstream's
does, and the dither is upstream's single `1.0 / 64.0`. What remains below is
what did not follow from that.

**Every upstream claim below was read at tip of tree**, in the `flutter/flutter`
monorepo under `engine/src/flutter/impeller`, not from a checkout. A parity
decision is worth exactly as much as the source it was read from, and a local
clone of unknown vintage can encode behavior upstream has since changed. Where
a claim names a symbol or a file, that is what to re-read when checking whether
this file has gone stale.

**Last re-read: 2026-09-16**, and the date is here because the sentence above it
is worthless without one. "It was checked" is not a fact a later reader can act
on; "it was checked on this day, and these are the symbols that were still
saying what this file says they say" is. The same lesson is written out at
length beside the timing baseline, which went eight commits pointing at a state
no run had passed against, for want of exactly this.

Six entries name something in upstream specific enough to re-read, and all six
were, on that date and at tip:

| | claim | what was read | still true |
|---|---|---|---|
| §1 | 256 uniform stops | `gradient_generator.h`, `kMaxUniformGradientStops = 256u` | yes |
| §3 | the GLES shading language floors at 1.00 | `compiler.cc`, `sl_options.version = ... : 100`, and the `#ifndef IMPELLER_TARGET_OPENGLES` around `IPOrderedDither8x8` in `fast_gradient.frag` | yes |
| §5 | elevation is in logical pixels | `dl_dispatcher.cc`, `Scalar occluder_z = dpr * elevation` | yes |
| §6 | a blur reduces in one step | `gaussian_blur_filter_contents.cc`, `kMaxSigma = 500.0f` and one `downsample_scalar` through `texture_downsample.frag` | yes |
| §8 | the blurred rectangle's asymmetric term | `solid_rrect_like_blur_contents.cc`, `NegPos` and `1.25 * sigma * (eccentricV.x - eccentricV.y)` under the comment "Pull in long end" | yes |
| §10 | the squircle's conic-weight sawtooth | `round_superellipse_param.cc`, `frac * kPrecomputedVariables[left + 1][0] * sqrt(n)` | yes |

Two of those six are upstream defects rather than differences of design -- §8's
asymmetry and §10's sawtooth -- and both are still there. §10 is the sharper
case now that the file has been read again: the comment four lines above the
line in question states the intended relation as `weight1 = factor1 * sqrt(n)`,
for the whole factor, which is what the code does to one term of the
interpolation and not the other. Neither has been reported.

## 1. Four stops fit in the paint block; upstream carries 256

**What differs.** Past `MAX_STOPS` — four — the recorder tabulates a gradient
into a 256-texel ramp texture and the shader samples it. Upstream's
`kMaxUniformGradientStops` is 256, and its storage-buffer path is bounded only
by the buffer, so it walks stops in the shader for effectively every real
gradient and reaches a texture only past 256 stops or on a device without either
facility.

**Why.** The paint block is one uniform block per draw and every member of it is
a four-component vector; carrying 256 colors and 128 stop pairs would mean the
dedicated secondary blocks upstream uses, which is a different design for the
material rather than a larger one.

**Impact.** A five-stop gradient allocates and samples a texture here where
upstream would walk uniforms. The picture is meant to be the same, and is
tested to one level per channel: the ramp holds exactly what the four-stop walk
produces, so no quantization enters that the walk does not also have. The cost
is an upload and a sampler binding per gradient past four stops, on a path
upstream would not have taken.

Note the trap this sets, because it is easy to fall into and one commit here
already did. Upstream's *texture* path does not dither, and reading that across
to this renderer's ramp looks obviously right. It is backwards: upstream reaches
its texture past 256 stops and this renderer reaches its ramp past four, so
matching the mechanism would leave nearly every gradient here on the side
upstream nearly never uses. Both paths are dithered for that reason.

## 2. The gradient ramp is half-float; upstream's is eight-bit

**What differs.** `CreateGradientTexture` builds a
`PixelFormat::kR8G8B8A8UNormInt` texture. This renderer's ramp is
`Rgba16Float`. Both hold sRGB-encoded components; what differs is the precision
they hold them at.

**Why.** Range rather than precision. An eight-bit table cannot hold a component
outside the sRGB primaries at all, and a wide-gamut gradient has them — a
Display P3 red restated against sRGB is `1.093` in red and negative in the other
two. Upstream's table cannot carry that either, and reaches a table so rarely
that it has not had to.

**Impact.** Two kilobytes against one, per gradient past four stops. In exchange
a gradient stated in Display P3 survives being tabulated, and the two gradient
paths agree to a level rather than to twenty-four. It follows §1: upstream's
texture path is a fallback past 256 stops where this one is the ordinary path
past four, so a limitation upstream can live with is one this cannot.

## 3. Gradients are dithered on GLES

**What differs.** Upstream does not dither on OpenGL ES at all. Its fast path
guards the call with `#ifndef IMPELLER_TARGET_OPENGLES`; its storage-buffer path
is the only other one that dithers and needs storage buffers, which are ES 3.1;
and its uniform and texture paths never dither. Here both backends dither.

**Why.** The guard exists for a constraint this project does not have, and the
constraint is worth stating exactly rather than from the comment beside it. The
shader compiler defaults its GLES target to GLSL ES 1.00 —
`sl_options.version = ... : 100` in `impeller/compiler/compiler.cc` — which is
the OpenGL ES *2.0* shading language. It has no `uint`, no bitwise operators and
no `%`, and `IPOrderedDither8x8` is built from all three, so on that target the
function cannot compile at all. The comment beside the guard says "mod operator"
and understates it.

Two things follow. Upstream's GLES users lose dithering because the shader is
compiled once at that floor, not because anybody decided a gradient should band
there — a modern ES 3.0 device gets the undithered shader along with everything
else. And this backend's floor is GLES 3.0, with 2.0 permanently out of scope,
so `uint` and `%` are present and the same shader compiles and runs.

The second reason is load-bearing on its own. The cross-backend comparison holds
the two backends to `Tolerance::ROUNDING`, one unit per channel with no
outliers, while a dither reaches two — so importing the guard would fail the L3
lane on every gradient scene in the corpus, trading a real invariant for a
copied workaround to a limitation this renderer does not have.

**Impact.** A gradient drawn through this renderer's GLES backend is smoother
than the same gradient through upstream's. Nothing a caller can be harmed by,
but a direct comparison against upstream on a GLES device would differ by up to
two levels across the gradient, and would differ *only* there.

If upstream ever raises its GLES floor past 2.0, this entry should disappear
rather than be re-argued: the divergence is entirely downstream of that one
number.

## 4. Wide gamut is `Rgba16Float`, and is not presented

**What differs.** Upstream renders wide-gamut content into `BGRA10_XR`, a Metal
format that is extended-range ten-bit fixed point. Here the wide format is
`Rgba16Float`. And nothing here presents in a wide-gamut color space: the
swapchain format list, the color space it asks for, and the DRM scanout list are
all untouched.

**Why.** `BGRA10_XR` has no portable equivalent — the property that matters is
extended range rather than depth, and `Rgb10A2Unorm`, which does exist on both
backends, is unsigned and so cannot hold the negative component a Display P3 red
needs. On presentation: the devices available to this project are llvmpipe and
vkms, so a wide-gamut presentation path could not be checked, and would be code
whose correctness rested on having read a specification.

**Impact.** Eight bytes per pixel against upstream's eight, so no memory
difference. The pipeline carries the gamut and can be read back through it, but
a caller cannot get a wide-gamut image onto a display through this renderer, and
should not read the parity tables as saying otherwise.

## 5. A shadow's elevation is in device pixels

**What differs.** One thing, and it is not the blur's width, its color, or what
it does with an occluder — those were all on this list and none is now.
`DlDispatcherBase::drawShadow` takes a `dpr` and computes
`occluder_z = dpr * elevation`, so its elevation is in logical pixels. There is
no such parameter here and an elevation is in device pixels.

**Why.** An API difference rather than an omission. `dpr` is supplied by the
engine upstream and does not appear on `dart:ui`'s `Canvas.drawShadow` at all,
and this renderer has no notion of logical pixels to convert from — so an
elevation here means what it says.

**Impact.** A caller working in logical pixels has to scale the elevation
themselves, by the same factor they scale everything else.

Three things that were on this list and are not now, each removed by checking
rather than by deciding. The tonal color remap is ported. The occluder punch-out
is gone: upstream takes `transparent_occluder` and never reads it, and in the
arrangement the flag describes — an opaque caster drawn over its own shadow —
the punched and unpunched pictures were byte-identical while the punch cost a
layer, so every shadow was five passes where four will do. And
`drawShadow` divides its radius by `GetCurrentTransform().GetScale().y`, which
read as a divergence until both sides were measured: upstream's blur sigma is in
*local* space — `gaussian_blur_filter_contents.cc` multiplies it by
`ExtractScale(entity.GetTransform().Basis())` — so that division exists to
cancel the multiplication and leave the shadow's softness fixed in device
pixels.

That entry used to end here by saying this renderer's sigma was already in
device space and reached the same behavior without dividing, and that copying
the division would break parity rather than add it. Both halves were true of
the convention then in force and neither is now: the sigma is in the space the
drawing is in, as `dart:ui` states it and upstream honors it, so the
multiplication the division exists to cancel is here too and the division is
here with it. `docs/architecture.md` has the change and what caught the half of
it that was not designed. The behavior a caller sees is unchanged — a shadow's
softness is fixed in device pixels, measured at a five-pixel tail under a unit
scale and a doubled one — which is the point of both arrangements and the
reason this paragraph is a correction rather than a new entry.

Worth recording how the blur width was wrong, since the shape of the mistake is
more useful than the number. Elevation gives a kernel *radius*, and the blur
takes a *deviation*; upstream converts with `radius / sqrt(3) + 0.5`, and that
conversion was simply missing. Compounding it, the light ratio was read as
`800.0 / 600.0`. Upstream's dispatcher writes `constexpr Scalar kLightRadius =
800 / 600` with integer literals, so its value is one — while `DlCanvas` has a
*second* pair, `kShadowLightRadius` over `kShadowLightHeight`, which are floats
and do give one and a third, and which size the shadow's bounds rather than draw
it. Reading the wrong pair and skipping the conversion together made every
shadow here about twice as soft as the same elevation gives upstream.

## 6. A large blur is reduced by halving; upstream reduces in one step

**What differs.** Both shrink the image rather than spreading the taps once the
kernel outgrows its budget, and both clamp the deviation at five hundred. The
reduction is reached differently: upstream computes a downsample scalar and
resamples once through `texture_downsample.frag`, where this halves repeatedly
until the radius fits.

**Why.** A linear sample taken at the center of a two-by-two block averages
exactly those four texels, so halving *is* a box filter and a chain of halvings
needs no kernel of its own. Reducing by eight in one step with a single
bilinear tap would read four texels of every sixty-four and call the rest
absent, which is how a downsample turns a smooth image into a crawling one —
so a single-step reduction needs the dedicated shader upstream wrote for it,
and the chain does not.

**Impact.** Passes, and only past the threshold. Under a deviation of about
nineteen there is no reduction on either side and nothing differs. Above it this
spends one pass per halving where upstream spends one in total, so a very wide
blur costs two or three passes more — each on an image already a quarter or a
sixteenth of the size, which is why it was worth having the reduction at all.
The pictures agree: the reduction preserves light, checked at a deviation of
twenty-four by the energy test, which takes this path.

## 7. A blurred path that is not a rounded rectangle is blurred

**What differs.** Upstream has two ways of not running a blur pass. One is
built here and one is not.

- **A rounded rectangle**, all four corners sharing one circular radius:
  upstream's `AttemptDrawBlurredRRect` evaluates the blur in the fragment
  stage. **Built.** `Material::RoundedRectBlur` is Raph Levien's
  approximation, the method `SolidRRectBlurContents` evaluates, and a `Path`
  carries the shape that built it so a shadow reaches it too — `draw_shadow`
  takes a path, as `dart:ui` does, and upstream's `DlPath` answers the same
  question for the same reason.
- **Any other shape**: upstream's `DrawPath` sends a filled, solid-colored,
  positively-blurred path to `AttemptDrawBlurredPathSource`, which tessellates
  a **shadow mesh** whose vertices carry the falloff. **Not built.** Here it
  draws the shape into a layer and runs a separable Gaussian over it: one pass
  for the content and two for the blur.

**Impact, measured on a Raspberry Pi 5's V3D, release build.** The bench frame's
three shadows fall on rounded cards, so they now take the analytic route. The
frame costs **21.224 ms through Vulkan and 20.409 through GLES**, against
26.757 and 24.318 when they were blurred, and 18.928 and 17.676 with them left
out entirely. So three shadows cost 7.8 ms as passes and 2.3 ms as draws, and
the frame is five passes rather than fourteen.

What is left is the shape this does not cover. A shadow under anything that is
not a rounded rectangle — a rounded superellipse, a caller's outline, a glyph —
still costs three passes, and the mesh is what upstream answers that with.

The pictures agree either way, which is why [`parity.md`](parity.md) lists
`maskFilter` and `drawShadow` as built. This is a difference in what they cost.

## 8. A blurred rectangle is symmetric here; upstream's is not

**What differs.** One term, in the analytic blurred rounded rectangle. The
approximation shortens the longer axis by an amount that falls away as either
side grows past the deviation — a rectangle much longer than it is wide
otherwise blurs to something the axis-wise expression makes too eccentric.
Upstream writes that as

```c++
double delta = 1.25 * sigma * (eccentricV.x - eccentricV.y);
rSize += NegPos(delta);            // NegPos(v) = {min(v, 0), max(v, 0)}
```

which shortens x when x is the long axis and *lengthens* y when y is. This
renderer shortens whichever axis is longer: `{min(delta, 0), min(-delta, 0)}`.

**Why.** Upstream's own comment on that line reads "Pull in long end (make less
eccentric)", which is what it does in one orientation and the opposite of what
it does in the other. The consequence is visible: at a deviation of five, a
100×20 rectangle blurs as though it were 98.8 long and a 20×100 one as though
it were 101.2 — the same shape, turned, coming out two and a half texels
different. `a_blurred_rectangle_is_the_same_turned_either_way` fails by
twenty-four levels against upstream's form and passes against this one. The
sampled route passes either way, which is what placed the asymmetry in the
approximation rather than in the rasterizer.

Deviating rather than matching, because a blur whose width depends on which way
the rectangle is turned is a defect rather than a convention, and because
matching it would mean keeping a test that asserts the wrong thing. Reported as
flutter/flutter#192189.

**Impact.** None on agreement with the sampled blur, which is the check that
matters for the approximation as a whole: the seven shapes in
`an_analytic_blurred_rectangle_agrees_with_the_blur_it_replaces` come to 13,
22, 17, 16, 18, 18 and 9 levels either way. The error was symmetric about the
sampled result — one orientation short, the other long — and is now the same
shortening in both.

## 9. Operations that are absent

These are listed in [`parity.md`](parity.md) with their reasoning and are
summarized here only so that this file is the one place to look.

- **Text shaping and font parsing.** Out of scope by design; `draw_glyphs` takes
  a positioned run and an atlas. *Impact:* a caller brings their own shaper.
- **`drawPicture` is composed as an image rather than replayed.** This entry
  used to say the geometry was re-walked and that the cost was recording time,
  which is not what `draw_recording` does and understates it twice over. A
  recording arrives with its passes already made: they are appended, its root
  becomes a texture this canvas samples, and the picture is placed by mapping a
  fragment back through the transform. So a picture costs a target and a pass of
  its own -- `picture-drawn-into-a-picture` in the corpus is three passes for two
  nested ones, one each and one for the frame, and `cost-baseline.txt` is where
  that is visible.

  *Impact:* two, and the second is the one a caller would notice. A pass per
  picture, where upstream dispatches the sub-picture's ops into the canvas it is
  already recording and spends none. And a picture is rasterized at its own
  extent before it is placed, so magnifying one resamples the picture it became
  rather than re-flattening its curves at the new scale --
  `dl/draw-picture-magnified` in the catalog draws exactly that, and a circle's
  edge is where it shows.

## 10. An upstream artifact carried on purpose

**The squircle's outline snaps at twelve corner radii, and it does here too.**

`draw_rsuperellipse` approximates each superellipse arc with two conics, and
the conic weights come from a fitted table interpolated on the curve's degree.
Upstream's interpolation multiplies `sqrt(n)` into only the right-hand term, so
the weight climbs across each interval and drops back at the next whole degree
-- a sawtooth with a forty percent step, twelve times over the table's range.

It shows. Sweeping the ratio of side to corner radius and measuring the drawn
outline against the analytic curve, a ratio of 2.700 lands within 0.005 of the
true shape and 2.705 lands 0.042 away. Two tenths of a percent of corner
radius, a ninefold change in how faithful the outline is, at a place where the
shape itself is perfectly continuous. A control animating its corner radius
crosses several of these.

The obvious repair -- applying the factor to the whole interpolation, which is
what upstream's own comment describes -- was implemented here and measured, and
it is worse everywhere: 0.056 at its worst against 0.046, and two to five times
the error past a ratio of five. The table was fitted against the formula as
written, so correcting the formula without refitting the table moves the shape
further from the curve it is approximating rather than closer.

So this is carried rather than fixed. Smoothing it would put this renderer's
squircle where Flutter's is not, which is the substitution refused everywhere
else here; refitting the table would be inventing a shape rather than matching
one. Reported as flutter/flutter#192190, including the measurement that says the
one-line fix is worse than the bug. *Impact:* none against upstream, which is the point -- the outline is
wrong in exactly the way Flutter's is. It is written down because the next
person to measure this shape will find the jump and reasonably think it is a
local mistake.

## 11. One thing that looks like a difference and is not

Worth stating because a reviewer raised it as a hole. **The advanced blend modes
are defined on `[0, 1]` here and clip in `set_lum`,** which looks like an
eight-bit assumption surviving into a wide-gamut pipeline. It is not: that clip
is the W3C compositing specification's `ClipColor`, part of the *definition* of
the non-separable modes, and upstream implements the same specification.
Matching it is parity. Extending those modes past the unit range would be
inventing behavior upstream does not have.

**Impact.** None, which is the reason for the entry. It is here so that the
next reader who notices the clip finds the answer rather than filing it, and so
that anyone tempted to "fix" it sees that doing so would *create* a divergence
rather than remove one.

## 12. A layer's matrix widens what it records, but not without limit

**What differed, and what was done.** `Layer::with_matrix` is `dart:ui`'s matrix
image filter on a save layer, and it resamples what the layer captured. What a
layer captured was bounded by its parent's target -- so a shape drawn outside
the frame was gone before the matrix ran, and a translation that would have
brought it into view brought in nothing. Upstream's
`MatrixImageFilterDoesntCullWhenTranslatedFromOffscreen` is that case by name,
and it drew nothing here.

A layer whose matrix will move its result now records over the *pre-image*: the
region that lands where the parent can see it once the matrix has been applied,
unioned with the parent for the content the matrix leaves where it was.

**Opening wide costs no memory, and that is what made it safe to do.** The entry
that stood here said the fix was a memory decision as much as an arithmetic one,
because the inverse of a minifying matrix is a magnifying one and layer
allocation is where this project has already run a machine out of texture
memory. That is true of the region a draw may *land* in and false of the region
that is *allocated*: the pass's extent comes from the narrowed target in
`finish_layer`, which is the content's own bounds. A matrix that magnifies its
pre-image a hundredfold widens where a draw may go without allocating for
anywhere no draw reached, so what is allocated stays bounded by what the caller
drew rather than by the matrix.

**What is left.** The widening is computed from the matrix alone, so it is exact
for the affine cases and refuses the rest: a matrix that folds the plane has no
inverse and one that carries the region across the vanishing line has no finite
pre-image, and both keep the old behavior of capturing what the parent holds.
A layer given explicit bounds is also unchanged -- the caller has said where the
content is, and a matrix does not make that statement wrong.

**Impact.** A caller who draws deliberately off-target and translates it in now
gets the picture, where before there was nothing. A caller who moves a layer
within the frame sees no difference, which was always nearly every use.

## 13. A translucent bevelled stroke covers a pixel twice

**What differs.** A stroke sent to the tessellator is a run of quads with a
join between each pair and a cap on each end, and where those quads land on the
same pixel the outline covers it more than once. At full opacity that is
invisible. At half it is not: each cover blends over the last, so the pixel
comes out darker than a stroke of that alpha should ever be.

Upstream draws exactly this picture to say it does not happen. Its
`CanRenderWideStrokedRectWithoutOverlap` and its `...RectPath...` twin are the
same six outlines, translucent blue, three joins where the stroke leaves a gap
down the middle and three where it is wider than the shape it outlines.

**Two of the three joins are fixed.** An evaluated distance field covers each
pixel exactly once, and a stroked rectangle now takes that route for a round
join and for a miter — the outline having become the difference of two offset
shapes rather than a band around one, which is what let a square corner stay
square. `docs/architecture.md` has the geometry. Measured on the plate's lower
row, where the stroke is twice the width of the rectangle: the round column
carries one cover over 776 pixels and the mitered column over 897, with nothing
above it in either.

What is left is the bevel, and it is left for a reason rather than pending. A
bevel cuts the corner off, which is neither the arc an offset gives nor the
point a miter does; no offset of a rounded rectangle is a bevelled one, so
there is no field to evaluate and the tessellator is the only route. Its column
still reads three covers and six where one is 140 in blue, 226 and 251.

The same is true of any stroked shape with no analytic form — a polygon, a
curve — which is the larger part of what remains. Fixing that is not a change
to the stroker: the quads have to overlap, that being how a join covers the
wedge between two segments, so what would have to change is that the whole
outline is resolved to coverage before the paint's alpha is applied. That is a
stencil pass or an offscreen per stroke, a cost every stroke would pay for a
case only a translucent self-overlapping one has.

**Impact.** Confined to a translucent stroke wide enough to reach across the
shape it outlines, or one whose path doubles back on itself inside a stroke
width, *and* drawn either with a bevel join or on a shape with no analytic
form. An opaque stroke of any width is unaffected, and so is a translucent one
narrow relative to its geometry, which is nearly every stroke drawn. Where it
shows, it shows as a darker patch at the joins rather than as anything
structural, and it is the same on both backends.

## 14. A blur turns with its caller, but by turning the passes rather than the space

**What differs, and it is now a mechanism rather than a result.** `dart:ui`
states a deviation per axis in the space the caller was drawing in. Where that
space is turned relative to the target, the blur turns with it here as it does
upstream -- a quarter turn transposes the picture exactly, pixel for pixel --
but the two get there by different routes, and the difference is worth keeping
written down.

**Upstream removes the rotation.** `GaussianBlurFilterContents` re-renders its
input into what its comment calls "un-rotated local space", scaled by the
transform but not turned by it:

    // Source space here is scaled by the entity's transform. [...] You can
    // think of this as "scaled source space" or "un-rotated local space". The
    // entity's rotation is applied to the result of the blur as part of the
    // result's transform.

`ExtractScale` takes the lengths of the transformed basis vectors, so a rotation
contributes nothing to it; the blur then runs along that space's own axes and
the finished image is drawn back under the full transform. An `FML_DCHECK` that
the snapshot's transform is translation-and-scale only holds the invariant in
place. The stated reason is quality rather than correctness: the comment says
the un-rotated space "is a requirement for text to be rendered correctly",
because taps landing on texel centers is what keeps a glyph sharp.

**This turns the passes instead.** That arrangement is not available here. A
layer is a recorded pass with a device-space target, a device-space scissor and
a stencil buffer to match, so its content cannot be re-rendered into a space of
its own choosing after the fact. What was available is the blur pass's `step`,
which was already a free two-vector rather than an axis flag -- the shader walks
its taps along whatever direction it is given. So `BlurBasis` takes the
directions the caller's axes point in once the transform has been applied, and
the two passes run along those.

The two are the same Gaussian. A blur with deviations along orthogonal
directions is separable along exactly those directions, so the picture is
upstream's. What differs is that a tap here lands between texels and is resolved
by the sampler, which costs a little sharpness upstream's arrangement does not
pay. Nothing in this repository renders text through a blur, which is the case
upstream's comment is about.

**What is refused, and it is the same set upstream loses.** A transform with
perspective has no single basis -- the directions would differ per fragment,
which a pass walking a constant step cannot express. And a transform whose image
axes are not perpendicular, which is a shear, leaves a Gaussian that two
separable passes cannot state at all: separability is a property of orthogonal
directions. Both fall back to the target's own axes. Upstream is no better off
here, its `ExtractScale` taking the lengths of the image axes and dropping the
shear entirely.

**Impact.** A blur under a rotation now smears the way the caller asked, which
is visible only where the two deviations differ -- an isotropic blur was always
correct under a rotation, a circular kernel being circular whichever way it is
turned. Under a shear or a perspective transform, an anisotropic blur still
runs along the target's axes.

## 15. A mask blur under a mode that ignores coverage erases its whole bounds

**What differs.** A mask blur that cannot be evaluated in the fragment stage is
drawn as a layer: the shape goes into a target, the target is blurred, and the
layer is composited onto the frame with the caller's blend. That composite
covers the layer's *bounds*, and a mode that writes where its source is
transparent writes across all of them -- so the shape becomes its bounding
rectangle.

Seven of `dart:ui`'s modes ignore coverage in that sense: the ones whose
destination factor is neither `One` nor `OneMinusSrcAlpha` -- `Clear`, `Src`,
`SrcIn`, `SrcOut`, `DstIn`, `DstATop` and `Modulate`. Every other mode,
`SrcOver` and all the advanced ones included, is unaffected at any deviation.

**`Clear` is fixed, and the other six are not.** That split is upstream's and
not an arbitrary stopping point. `Clear` is the one mode that can be admitted to
the *evaluated* blur anyway, because on a coverage it is not what its factors
say: clearing by an amount `c` is `dst * (1 - c)`, which is `DstOut` against a
white source -- and white is exact rather than approximate, since `Clear`
discards the source color by definition and cannot care which one it had. So the
guard that refuses a coverage-ignoring mode admits `Clear`, substituting white
and `DstOut`, and a blurred circle drawn to clear now erases by its falloff:
alpha climbs monotonically out of the hole, and a corner of what the bounds
would have been is untouched.

Upstream does exactly this and no more.
`SolidRRectLikeBlurContents::Render` checks for `BlendMode::kClear`, forces the
color to white, and sets a flag that turns the pipeline's blend into a reverse
subtraction -- destination factor `One`, source factor `DestinationColor`, so
`dst - src * dst`. Same arithmetic; upstream reaches it by subtraction because
its fragment writes coverage directly, and this reaches it by naming the mode
that already means it. The other six have no such reading, upstream does not
generalize the case, and neither does this.

**What is left.** Two residues, and both are the layer route rather than the
evaluated one. The other six modes over any mask blur. And `Clear` over a shape
that is not rounded-rectangle-like -- a polygon, a curve -- which has no
evaluated blur to be admitted to and falls through to the layer as before.
Fixing either means treating a layer's alpha as coverage rather than as an
image, `mix(dst, M(src, dst), src_alpha)` per mode, which is a table nobody
upstream has derived either.

**Impact.** Confined to a mask blur combined with one of those six modes, or to
`Clear` on a shape with no analytic form. `SrcOver` is what nearly every blurred
draw uses. Where it does bite it is loud rather than subtle: a rectangle appears
where a soft shape was asked for.

## 16. A tint blend in `Plus` saturates in the shader, not at the target

**What differs.** `Plus` reaches three different places here, and one of them
clamps. A paint's own blend mode goes to the hardware as `One, One`, and a `Plus`
color filter is affine in the destination so it becomes a color matrix; both leave
the saturation to the attachment, which means an eight-bit target clips the sum
and a floating-point one keeps it. A per-sprite *tint* on `draw_atlas`, and the
same field on a mesh, go through the shader's `blend_tint`, whose arm for the mode
is `min(src + dst, 1)`. So a tint sum stops at one wherever it is written.
Upstream's `DrawAtlasPlusWideGamut` is the scene that can see the difference: it
requires an extended-range default format and adds to a bright texel.

**Why.** The clamp was removed to make the three agree, and put back after
measuring what that cost. On a Raspberry Pi 5, against a baseline the commit
before it reproduced to within three tenths of a per cent on all eight rows over
three runs:

| row | with the clamp | without it | shift |
|---|---|---|---|
| Vulkan distance field | 8.85 ms | 9.06 ms | +2.4% |
| Vulkan tessellated, either sample count | — | — | +0.9% |
| Vulkan full frame | 13.96 ms | 14.18 ms | +1.6% |
| GLES full frame | 14.84 ms | 15.01 ms | +1.2% |
| GLES distance field, tessellated | — | — | flat |

Read state for state: that board's four Vulkan rows are bimodal and settle at
process start, so the fast state is compared with the fast state. The raw
`--check` output says +6.4%, which is a slow state against a fast one and
overstates it.

Removing one instruction made the shader slower, which is register allocation on
V3D rather than anything arithmetic, and is the same step-function behavior
`docs/architecture.md` records for shader work. Attribution is exact rather than
inferred: rebuilding the tree with only that line restored produces a
byte-identical binary to the commit that measured clean, because everything else
in the two commits between them is test and document text.

What the removal bought was agreement on a floating-point target. Nothing here
presents one -- §4 above -- so the only place the disagreement can be observed is
a test that creates such a target itself. Paying one to two and a half per cent on
every frame of the configurations that do ship, for a difference none of them can
show, is the wrong way round.

**Impact.** A tint or a mesh's per-vertex color combined with `Plus`, and only
where the sum would pass one, and only on a target that could have held it. On
every eight-bit target -- which is every target this renderer can present to --
the clamp is invisible, because the attachment would have clipped the sum anyway.
`an_atlas_tint_in_plus_is_clipped_where_the_other_routes_are_not` pins it, and is
written to fail if the clamp comes out again so that whoever notices the
inconsistency finds the cost recorded rather than rediscovering it.

## 17. A morphology radius is in device pixels; upstream's is a local length

**What differs.** `ImageFilter.dilate` and `ImageFilter.erode` take a radius, and
the two renderers disagree about what space it is in. Upstream's is local: read at
master on 2026-09-23, `DirectionalMorphologyFilterContents::RenderFilter` builds
`entity.GetTransform() * effect_transform.Basis()`, applies it to the radius, and
rounds the length of the result to whole texels for the shader. So a dilated layer
under `canvas.scale(3.0)` spreads three times as far. Here the radius is device
pixels and nothing scales it: `Layer::scaled_by` multiplies `blur` and
`backdrop_blur` by the layer's scale and leaves `morphology` alone, deliberately.

**Why.** Not a decision so much as a claim that turned out to be false.
`architecture.md` records the conversion of every blur from device space to local,
done because a card lifting under a scale kept a blur the same size while its
content grew -- and it exempted morphology on the stated grounds that "upstream has
no morphology to be in parity with". Upstream has both filters, so the exemption
rested on nothing, and the one filter left in the old convention is the one the
section was written to fix.

It was not simply flipped along with the blur, and the reason is worth stating
rather than leaving as an omission. The radius is rounded to whole texels at
construction, in device space, and `architecture.md` explains why that rounding
must happen in the one place both the shader and the layer's bounds read the
radius from: a structuring element is a set of sample positions, and a radius
rounded for one reader and not the other grows the picture past what the target
has room for. Making the radius local moves the rounding after the scale, which
means it no longer happens where the value is stored, and the bounds and the pass
have to be shown to still agree. That is a change with a correctness argument
attached, not a multiplication.

**Impact.** A dilate or erode inside a scaled layer reaches the wrong distance
compared with upstream -- unchanged by the scale where upstream's grows with it --
and the error is proportional to the scale, so it is invisible at one and total at
ten. Nothing noticed until this entry was written: every morphology scene in the catalog
was drawn without a scale, rotation or concat, all six of them, so the corpus
comparison agreed with itself across backends and devices while both differed from
upstream.

The pair that notices now is `layer-dilated` and `layer-dilated-under-scale` --
the same cross at half the size under a scale of two, so it lands on exactly the
pixels the unscaled one covers and the dilation distance is the only thing left
that can separate them. `the_dilation_under_a_scale_reaches_the_same_distance`
reads the radius out of the recording and asserts they agree, which they do here
and would not upstream. It is written to fail when the convention is flipped, and
says in its own message to replace it with the doubled assertion rather than delete
it.

Worth knowing what does *not* catch it, since it looks as though it should: the
cost baseline records identical rows for the two scenes. A pass is emitted per
`MORPHOLOGY_TAPS` texels and that constant is thirty-two, so eight and sixteen both
fit in one pass each way and the pass count never moves. The same arithmetic means
the two scenes are very likely pixel-identical, so the image comparison is not the
detector either. The radius has to be read, not counted.

## 18. A filled path is triangulated here; upstream stencils and covers it

**What differs.** This is the deepest divergence in the file, and it is about how a
filled path becomes pixels at all rather than about any one operation.

Upstream, read at `flutter/flutter` master `fab99153` on 2026-09-29: a filled
path's winding is resolved by the *stencil buffer*, not by a triangulator.
`FillPathSourceGeometry::GetResultMode` returns `Mode::kNormal` for a convex path
and `kNonZero` or `kEvenOdd` otherwise, and
`ColorSourceContents::DrawGeometry` turns the latter into two draws -- a
preparation pass under `StencilMode::kStencilNonZeroFill`, which increments the
stencil on front faces and decrements on back, then a cover draw under
`kCoverCompare` over the geometry's bounds. What runs on the CPU is curve
*flattening* plus fan or strip index building: `Tessellator::TessellateConvex`,
documented as "Given a convex path, create a triangle fan structure", and it
produces the same vertex buffer for the convex direct draw and for the stencil
pass. A true arbitrary triangulator is in the tree and the renderer does not use
it -- `tessellator_libtess.h` wraps libtess2 and its only non-test consumer is the
Dart FFI shim. One comment states the purpose plainly: "for sufficiently complex
paths we may opt to use stencil-then-cover to avoid tessellation".

Here a filled path is triangulated on the CPU, by lyon, and those triangles *are*
the fill. `architecture.md` says it in a line: a recording is tessellated
geometry. No fill takes a stencil pass; the stencil serves clipping only.

**Where the two agree, which this framing makes easy to lose.** Both flatten
curves on the CPU. Both build stroke outlines on the CPU and neither expands a
stroke on the GPU -- upstream's `StrokePathSegmentReceiver` emits a triangle strip
carrying `Mode::kPreventOverdraw`, which is resolved by the depth buffer rather
than the stencil, and this one emits quads from lyon's stroker.

**And the analytic routes run closer than the entry's title suggests, in both
directions.** Upstream computes circle coverage from a distance function by
default -- `circle.frag`'s `distanceFromCircle`, shading a polygon mesh padded for
antialiasing -- and absorbs a symmetrically mask-blurred rounded rectangle into a
closed form in `rrect_blur.frag`, which is the same approximation this renderer
cites where it does the same thing. Its wider signed-distance family
(`uber_sdf.frag`, covering rect, oval, rounded rect and symmetric round
superellipse, filled and stroked) is **off by default**: `impeller::Flags`
declares `bool use_sdfs = false`, every one of the six call sites in
`display_list/canvas.cc` is gated on it, and only `--impeller-use-sdfs` turns it
on. So an unblurred rounded rectangle is a CPU polygon upstream today, where here
it is a field -- under a gate of nearly the same shape, both requiring
antialiasing, a solid color and no perspective.

**Why.** Reach, and it is stated in the first line of the README rather than
discovered here: every pipeline compiles ahead of time and nothing requires
compute, which is what lets one binary serve every Vulkan 1.1 and GLES 3.0 device
including embedded parts whose compute support is weak or immature. A triangulated
fill needs a vertex buffer and one draw. Stencil-then-cover needs a stencil
attachment on every pass that might fill a concave path, two draws where there was
one, and a cover whose bounds must be right.

What this project can say about the trade is narrower than it would like, and the
distinction matters because it is easy to overstate. It has never implemented
stencil-then-cover and has therefore never timed it. **There is still no
measurement here of stencil-then-cover against triangulation, and no claim about
which is faster** -- what is bounded below is the largest CPU saving it could
possibly offer, which is not the same thing. What is measured beside it is the
neighboring question -- moving coverage from
vertices into a fragment shader -- because this renderer has both routes for a
rounded rectangle and times all four. On a Raspberry Pi 5's V3D, from the
committed baseline:

| route | recording (CPU) | execute (GPU, Vulkan) |
|---|---|---|
| distance field | 0.099 ms | 8.852 ms |
| tessellated, 1 sample | 0.240 ms | 3.688 ms |
| stroked field | 0.098 ms | 9.431 ms |
| stroked path | 0.739 ms | 1.184 ms |

The field is two and a half times cheaper on the CPU for a fill and seven times
cheaper for a stroke, and on this hardware it costs two and a half times more GPU
for the fill and eight times more for the stroke. That is why the analytic routes
here are narrow rather than universal: on a tile-based part the saving reverses,
and a renderer aimed at such parts cannot take the CPU win as free. It says
nothing about the stencil, which costs a pass rather than a shader.

**What the stencil could save, as a ceiling.** The two designs do the same
flattening; they differ in what follows it, and only for concave paths, because
`fill` sends a single convex subpath to a fan and upstream sends one to
`TessellateConvex`. So the CPU stencil-then-cover could save is bounded above by
the time between flattening a path and filling it, and nothing in the rounded
rectangle above reaches that route at all -- it tessellates `fan=1, general=0`,
which is why the rows above cannot show this cost.

Measured on real map geometry rather than a synthetic shape, since a map is the
workload where concave fills dominate: one Berlin z14 vector tile, 527 polygon
features, 1,185 rings, 13,795 points, 38 of them multi-ring.

| device | flatten | flatten + fill | triangulation | share |
|---|---|---|---|---|
| x86-64 workstation | 0.102 ms | 1.585 ms | 1.483 ms | 93.6% |
| SA8155P, Kryo | 0.510 | 5.294 | 4.784 | 90.4% |
| Raspberry Pi 5, A76 | 0.479 | 5.668 | 5.189 | 91.6% |
| VisionFive 2, U74 | 2.291 | 21.300 | 19.010 | 89.2% |

Two readings, and the second is the one that matters. The *share* barely moves --
about nine tenths of the phase on every device from an x86-64 desktop to an
in-order U74 -- so the ceiling is a property of the work rather than of the part.
The *absolute* does move, by 3.7x between the two extremes, and nineteen
milliseconds for one tile is past a whole frame at sixty hertz.

Four things that keep this from being a case for stencil-then-cover on its own.
It is a ceiling and not a saving: the stencil trades that CPU for a second draw
and an attachment, which a tiler does not give away. Vector tile geometry carries
no curves, so flattening here is line-segment work and cheap, and a curve-heavy
path would shift the share down. A tile is tessellated once and retained, so this
is a load cost amortized over frames rather than a per-frame one -- it would
appear as a stall when a pan lands many tiles at once, not as a lower frame rate.
And these are static-musl builds, which run about fourteen per cent slower than
glibc on the workstation where both were timed; the shares are unaffected.

**Impact.** A concave fill costs a CPU triangulation here and two draws plus a
stencil attachment upstream, and which is dearer is unmeasured on any hardware
this project has. Fill rules are not affected -- lyon resolves non-zero and
even-odd as the stencil does, so a self-intersecting path fills the same either
way, which is why nothing in `parity.md` or the corpus shows this. The visible
consequences are elsewhere: a concave path's cost here scales with its vertex
count on the CPU rather than with its area on the GPU, and a pathological path is
a CPU problem here and a bandwidth one upstream.

The honest reading of the whole entry: the two renderers agree closely on what a
picture should look like, and this file is the record of where they do not, but
they do not agree on the most basic question of how a fill is rasterized. Nobody
should infer the C++ design from this one, which is the reason this entry leads
with the mechanism instead of the consequence.

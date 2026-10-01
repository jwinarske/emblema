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

Ten entries name something in upstream specific enough to re-read. Six were read on
the date above. §1 and §19 were read at tip on **2026-09-29**, and §17, §20 and §21 on
**2026-09-30**, which is why five rows carry their own date.

§21's row says something the others do not, and the distinction is worth keeping in
this column: the code it cites exists and a search says nothing calls it. "Still true"
for that row means the mechanism is still there, not that upstream still uses it.

Two things about how those four arrived, because both are the failure this table
exists to catch. §17 quoted upstream code with a commit in its own text for as long
as it existed and was never listed here, so nothing said when it had last been
checked. And §20's row was written from §14's citation of `ExtractScale` rather than
from the file -- a row asserting a re-read that had not happened. Both were read
before this sentence was written:

| | claim | what was read | still true |
|---|---|---|---|
| §1 | 256 uniform stops | `gradient_generator.h`, `kMaxUniformGradientStops = 256u` | yes, again 2026-09-29 |
| §3 | the GLES shading language floors at 1.00 | `compiler.cc`, `sl_options.version = ... : 100`, and the `#ifndef IMPELLER_TARGET_OPENGLES` around `IPOrderedDither8x8` in `fast_gradient.frag` | yes |
| §5 | elevation is in logical pixels | `dl_dispatcher.cc`, `Scalar occluder_z = dpr * elevation` | yes |
| §6 | a blur reduces in one step | `gaussian_blur_filter_contents.cc`, `kMaxSigma = 500.0f` and one `downsample_scalar` through `texture_downsample.frag` | yes |
| §8 | the blurred rectangle's asymmetric term | `solid_rrect_like_blur_contents.cc`, `NegPos` and `1.25 * sigma * (eccentricV.x - eccentricV.y)` under the comment "Pull in long end" | yes |
| §10 | the squircle's conic-weight sawtooth | `round_superellipse_param.cc`, `frac * kPrecomputedVariables[left + 1][0] * sqrt(n)` | yes |
| §19 | a vertex-interpolated gradient path | `linear_gradient_contents.cc`, `CanApplyFastGradient` and `FastLinearGradient`, reached before the uniform path in `Render` | yes, read 2026-09-29 |
| §20 | a blur's deviations scale per axis | `filters/gaussian_blur_filter_contents.cc`, `Vector2 ExtractScale(...)` and the `Vector2 scaled_sigma` it feeds, which §14 cites for the same call | yes, read 2026-09-30 |
| §17 | a morphology radius per transformed direction | `filters/morphology_filter_contents.cc`, `transform.TransformDirection(direction_ * radius_.radius)` and `std::round(transformed_radius.GetLength())` | yes, read 2026-09-30 |
| §21 | opaque draws reordered to cull one another | `draw_order_resolver.h`, "reverse painter's order so that they cull one another"; `color_source_contents.h`, `depth_write_enabled = options.blend_mode == BlendMode::kSrc` | the machinery yes, the wiring no -- read 2026-09-30 |

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

**That was written as a debt, and the measurement says it is the opposite.** The
sentence above and the paragraph that used to follow it read the texture as the
expensive thing. The bench's frame draws a full-screen five-stop gradient, so it is
on this path, and that draw is 75 per cent of the frame on a Pi 5 and 67 on a
VisionFive 2 -- which made it look as though a filtered fetch per fragment were the
largest item in a frame.

It is not. Measured on the Pi 5 on 2026-09-30, the same ground drawn three ways in
one run ([`on-a-board.md`](on-a-board.md) has the conditions):

| route | flat fill | 5 stops, ramp texture | 4 stops, paint block |
|---|---|---|---|
| Vulkan | 8.494 ms | 10.424 | **14.958** |
| GLES | 8.843 | 11.767 | **16.308** |

Two things follow, and neither was the expected one.

**Most of that draw is fill.** Evaluating the ramp adds 1.930 ms under Vulkan, so of
a 13.911 ms frame the whole business of deciding a gradient's color is 13.9 per
cent; the rest is covering two million pixels once, which no gradient path changes.

**The ramp texture is the cheaper way to evaluate it, by a factor of three.** The
four-stop walk costs 6.464 ms of evaluation against the ramp's 1.930 -- 3.3 times
as much on Vulkan and 2.6 on GLES. A filtered fetch from a 256-texel table that
fits in any texture cache beats a per-fragment walk with comparisons and
interpolation on this hardware.

**Impact.** The picture is the same either way, and is tested to one level per
channel: the ramp holds exactly what the four-stop walk produces, so no
quantization enters that the walk does not also have. The cost is an upload and a
sampler binding per gradient past four stops, which is a per-draw overhead, against
a per-fragment saving of 1.9 ms on a full-screen draw. On the devices this renderer
targets the trade is favorable, and **closing this gap would make them slower**:
raising `MAX_STOPS` toward upstream's 256 would move every gradient between five
and two hundred and fifty-six stops onto the path that costs 3.3 times more to
evaluate. By the rule at the top of this file that is a question about how the
pixels get there, answered against the target devices, and the answer is now
measured rather than assumed.

What remains unmeasured is the crossover: a gradient small enough that one upload
and one sampler binding outweigh the per-fragment saving. Every figure here is a
full-screen draw, where the per-fragment side dominates by construction.

Note the trap this sets, because it is easy to fall into and one commit here
already did. Upstream's *texture* path does not dither, and reading that across
to this renderer's ramp looks obviously right. It is backwards: upstream reaches
its texture past 256 stops and this renderer reaches its ramp past four, so
matching the mechanism would leave nearly every gradient here on the side
upstream nearly never uses. Both paths are dithered for that reason.

The same shape of mistake produced the paragraph this entry used to carry. Reading
a mechanism across from upstream implies upstream's is the one to want; here the
measurement says the ramp is the better of the two on this hardware, and the thing
to carry across was the dithering rather than the threshold.

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

## 17. A morphology radius turns and stretches with the transform, and the shape of that is now upstream's

**Closed, in two halves, and kept because the second half is a worked example of
how this file is meant to be used.** A dilate or erode radius used to be device
pixels here while upstream's was a local length, so a dilated layer under a scale
of two spread twice as far there and the same distance here. That was the first
half, fixed 2026-09-29: `Layer::scaled_by` converts it, `ImageFilter::scaled_by`
converts a dilation handed over as a filter, and `Morphology::applied_radius`
rounds to whole device texels where the device radius is known.

The second half was the *shape* of the conversion, and it is fixed too. Read at
`flutter/flutter` master `fab99153`, upstream builds two directional passes -- X
carrying `radius_x` with direction `Point(1, 0)`, Y carrying `radius_y` with
`Point(0, 1)` -- and each transforms its own direction vector:

```
transform          = entity.GetTransform() * effect_transform.Basis()
transformed_radius = transform.TransformDirection(direction_ * radius_.radius)
frag_info.radius   = round(transformed_radius.GetLength())
frag_info.uv_offset = ...TransformDirection(transformed_radius).Normalize() / extent
```

So upstream takes a *length per axis* from the transformed vector and walks each
pass along the transformed *direction*. This renderer now does both:
`axis_scales_of` takes the lengths of the transformed basis vectors, which is what
upstream's `ExtractScale` takes, and `morphology_passes` walks `BlurBasis`'s
directions the way `blur_passes` does.

**What is left, and it is shared with the blur.** Under a shear or a perspective
transform `BlurBasis` refuses to decompose and falls back to the target's own
axes, so a sheared dilation does not turn. Entry 14 records the same limit for the
blur and why: separability is a property of orthogonal directions, and running two
passes along oblique ones is not a wrong dilation so much as not a dilation.
Upstream's morphology has no such fallback -- `TransformDirection` takes the shear
-- so this is a real remaining difference, narrower than the one this entry started
with and bounded to transforms the corpus does not contain.

**Impact.** Under an anisotropic scale the two now agree: `scale(2, 5)` on radii of
eight and three reaches sixteen along x and fifteen along y in both. Before, one
factor -- the larger -- multiplied both, so x reached forty. Under a rotation a
dilation now spreads along the caller's axes, so a rotated square dilates to a
square in the right orientation, where before it dilated along the screen's.

Three tests hold it, and the way they were written is the point. Each asserted the
*old* behavior first, so the fix could not land quietly:

- `the_dilation_under_a_scale_reaches_twice_as_far` pins the units, over the corpus
  pair `layer-dilated` and `layer-dilated-under-scale`.
- `the_dilation_under_an_anisotropic_scale_converts_per_axis` pins the per-axis
  length, over `layer-dilated-under-anisotropic-scale`. It asserted forty and
  fifteen until the conversion landed, then failed with sixteen and fifteen and
  named its own replacement.
- `a_dilation_walks_the_callers_axes_after_a_quarter_turn` pins the direction, and
  is built from a `Canvas` rather than a scene because no picture could catch it:
  a rotation changes nothing a material carries except `step`.

`a_morphology_radius_is_the_same_length_either_way_and_scales` pins that both
spellings agree, including which factor each takes.

**The anisotropic scene is pinned twice, and the second pin was not designed.** A
pass covers `MORPHOLOGY_TAPS` texels, which is thirty-two, so the inflated forty
arrived as two passes where sixteen arrives as one. The scene's cost row went from
five passes to four when the conversion landed. The corpus pair could not have
shown that, eight and sixteen both fitting one pass -- which is why this file used
to say a cost table cannot catch a morphology convention. For that scene it can.

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
superellipse, filled and stroked) is **off by default**: `emblema::Flags`
declares `bool use_sdfs = false`, every one of the six call sites in
`display_list/canvas.cc` is gated on it, and only `--emblema-use-sdfs` turns it
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

## 19. An axis-aligned gradient is shaded per fragment here; upstream interpolates it across vertices

**What differs.** Upstream has three linear-gradient paths and this renderer has
two. `LinearGradientContents::Render` tries `CanApplyFastGradient()` *first*: if
the effect transform inverts to identity, the geometry has coverage, and the
gradient is axis-aligned with its endpoints on the covered rect's edges — start
and end sharing an x for a vertical wash, sharing a y for a horizontal one — it
takes `FastLinearGradient`, which computes no gradient in the fragment shader at
all. It divides the rect into one section per pair of stops, emits six vertices a
section carrying the two stop colors, and lets the rasterizer interpolate between
them. `fast_gradient.frag` is left applying alpha and, off GLES, a dither.

Only when that fails does upstream reach the uniform path this file's §1 is
about, and only past 256 stops the texture path. This renderer's fastest path is
that uniform walk: four stops in the paint block, evaluated per fragment, and a
sampled ramp beyond. There is no vertex-interpolated path and no predicate that
would select one.

**Why.** No reason on record, which is what distinguishes this entry from its
neighbors. It is not a device constraint — vertex color interpolation is the one
thing every target here does in fixed-function hardware — and not a semantic
difference, since the two produce the same picture up to interpolation
precision. It is a missing optimization, listed here rather than in
[`parity.md`](parity.md) because the `dart:ui` operation is built and behaves;
what is absent is a route through it.

The honest account of how it stayed absent: the symbol was already cited in this
file, in §3's evidence column, as the file that dithers under an `#ifndef`.
Something can be read for one property and not seen for another.

**Impact, and it is bounded now rather than open.** For the most ordinary gradient
in an interface — a vertical or horizontal wash behind a card or a bar — upstream
does zero per-fragment gradient work where this renderer does a four-stop walk or a
filtered texture fetch per fragment.

What that is worth has a ceiling, and §1's later measurement supplies it. A
vertex-interpolated path removes *all* per-fragment gradient evaluation and nothing
else, so the most it can recover is the evaluation share of the draw: on the Pi 5,
1.930 ms of a 13.911 ms frame under Vulkan and 2.924 of 14.866 under GLES. That is
**13.9 and 19.7 per cent of a frame** — worth building, and not the three quarters
that the gradient ground's share of the frame invites you to read. Four fifths of
that draw is the fill underneath, which no gradient path touches.

The earlier wording here pointed at the ground's 43.1 ms of 64.0 on the VisionFive 2
as the reason to expect this to matter, which overstated it in exactly that way.

**The measurement cannot be taken from anything here as it stands**, and the
bench is the reason. Its only gradient is the ground's, `[0, 0]` to `[w, h]` —
diagonal, so not a case this path would catch even if it existed. The corpus is
the other way round: of its thirty-eight linear gradients, nine share a y --
`[8, 0]` to `[120, 0]` among them -- so axis-aligned cases are not missing there,
though none share an x, and upstream's predicate takes a vertical wash as readily
as a horizontal one. What is unchecked is whether any of the nine satisfies the
stricter half of that predicate — endpoints landing on the *covered rect's* edges
rather than anywhere along the axis — because that depends on each scene's
geometry and was not worth resolving before a path exists to select. The corpus
checks pictures in any case, and a vertex-interpolated wash is meant to produce
the same one.

So: an axis-aligned rect gradient in the bench comes first, because without one
there is nothing to time a change against. Then the path, whose predicate is cheap
and whose geometry is the sections upstream already describes. The paint block's
four stops do not bound it — the sections are geometry, so a vertex-interpolated
wash is not limited the way the fragment walk is.

It would not narrow §1, though, which is what an earlier draft of this paragraph
claimed. §1 is about which of two *per-fragment* evaluations a gradient past four
stops takes, and the measurement there says this renderer already takes the cheaper
one. A vertex-interpolated path sidesteps both rather than improving either, and
only for the axis-aligned rect case the predicate admits.

## 20. A blur's deviations scale per axis, and so does a backdrop blur's single number

**Closed for the pair, open for the one number, and the pair is why this entry
exists.** `dart:ui` states a deviation per axis in the caller's space, and something
has to convert the pair into device pixels. Upstream's
`GaussianBlurFilterContents` uses `ExtractScale`, which takes the lengths of the
transformed basis vectors -- one length per axis. `Layer::scaled_by` multiplied both
deviations by `max_scale_of`, the *larger* of those two, until 2026-09-30.

**What that cost, measured before it was fixed.** A layer blurred with deviations of
six and two recorded eighteen and six under `scale(2, 3)` -- and the same eighteen
and six under `scale(3, 2)`. Two transforms that transpose each other produced an
identical blur, because one number cannot tell them apart. It now records twelve and
six, and eighteen and four, which transpose as the transforms do.

By the rule in this file's opening that was never a divergence to keep: it is not a
question about how the pixels get there but about what the picture is, and two
different transforms giving one picture is not an answer to it. It was recorded
rather than fixed for one commit because no corpus scene blurred a layer under an
anisotropic scale, and the scene comes before the fix here or the change is
unmeasured.

**`Layer::backdrop_blur` was the last of it, and closing it needed no API change
after all.** This entry said it did: the field is one `f32` and `with_backdrop_blur`
takes one sigma, so the reasoning was that an anisotropic backdrop blur is not
expressible and making it so means making the field a pair. That conflated what the
caller states with what the renderer converts it to.

One number is the right thing for a caller to state -- it means a blur that is round
*in their own space*, which is what nearly every caller wants. A round blur under a
transform whose axes scale differently is an oval in device space, and the mistake was
converting it to one number of device pixels: `Layer::scaled_by` multiplied it by the
single largest factor, so a frosted panel came out round at the larger scale on both
axes while the same deviations on the layer itself stretched, and `scale(2, 3)` and
`scale(3, 2)` were again indistinguishable.

The field now stays in the caller's space and `open_layer` converts it where it becomes
an `ImageFilter::Blur`, which has two components to put the answer in. Six under
`scale(2, 3)` reaches twelve across and eighteen down, and transposing the transform
transposes the blur. Nothing between the two points reads it as a length -- its only
other uses are `> 0.0` tests asking whether a backdrop is wanted at all -- and the
public API is untouched. `a_backdrop_blur_is_converted_per_axis` pins it.

**Impact.** None against upstream, under every transform `BlurBasis` can decompose:
the layer's deviations, a dilation's radii and a backdrop blur all take the length of
each transformed basis vector, as upstream's `ExtractScale` does. What is left is the
shear and perspective fallback entry 17 records, which both filters share.

Three tests hold it, written the way entry 17's were:

- `a_blur_under_an_anisotropic_scale_converts_per_axis` asserted eighteen and six
  until the conversion landed, then failed with twelve and six and named its
  replacement, over `layer-blurred-under-anisotropic-scale`.
- `transposing_the_scale_transposes_the_deviations` is the property the single factor
  destroyed, and is built from a `Canvas` because the two halves have to be recorded
  in one test to be compared.
- `a_backdrop_blur_is_converted_per_axis` pins the last of it, and the transposition
  as well: twelve and eighteen one way, eighteen and twelve the other.

The scene's deviations are small deliberately, and entry 6 is why: a deviation drives
the reduction as well as the blur, so a scene crossing a halving threshold would move
its own pass count for a second reason and leave a reader unable to tell which one
did it. Eighteen was the largest in play against a threshold a little over nineteen.
`the_scene_does_not_reach_the_reduction_either_way` pins that, on the whole
recording's pass count rather than on the blur materials -- a halving is a pass of its
own, so counting blurs would read two either way and prove nothing.

## 21. Draws go out in painter's order here; upstream has machinery to reorder opaque ones, mostly unwired

**Why this entry is here.** [`on-a-board.md`](on-a-board.md) measured a full-screen
gradient at three quarters of a frame on V3D and then measured four fifths of *that*
to be fill rather than gradient evaluation. Fill is now the largest identified cost
in the bench's frame and no entry in this file addressed it, so the question was what
upstream does about covering a pixel more than once.

**What differs.** This renderer submits draws in the order they were recorded.
`Batch`'s own documentation states the constraint: "2D drawing is painter's-algorithm
ordered: reordering two overlapping draws changes which one ends up on top. Deciding
when a reorder is safe needs either overlap analysis or a depth buffer, and that
belongs to the layer that knows what the draws represent." Nothing above it does that
yet, and no draw here writes or tests depth.

Upstream has the pieces for it, read at tip on 2026-09-30:

- `color_source_contents.h` sets `options.depth_write_enabled = options.blend_mode ==
  BlendMode::kSrc`, under the comment "Enable depth writing for all opaque entities in
  order to allow reordering."
- `draw_order_resolver.h` separates opaque from translucent and keeps the opaque set
  "order independent, and so we render these elements in reverse painter's order so
  that they cull one another." `GetSortedDraws` also takes `opaque_skip_count` and
  `translucent_skip_count` "used for the 'clear color' optimization", which hoists
  leading full-coverage draws out of the pass entirely.
- `GeometryResult::Mode::kPreventOverdraw` turns on `depth_write_enabled` with
  `CompareFunction::kGreater`, and `Entity::GetShaderTransform` puts each element's
  depth in z, scaled by `kDepthEpsilon`, so z grows with paint order.

**But the scene-level half of that is not wired at tip, and the entry would be wrong
to imply otherwise.** A code search for `DrawOrderResolver` across `flutter/flutter`
returns its own header, its own implementation and its own unit tests, and nothing
else. The comment justifying the opaque depth writes names `EntityPass::AddEntity`,
and there is no `class EntityPass` upstream any more -- only `EntityPassTarget` and
`EntityPassClipStack`. So the reordering that would exploit those depth writes looks
either staged or left behind by a refactor. What is demonstrably live is the narrower
`kPreventOverdraw`, whose stated purpose is a stroke not painting its own pixels
twice rather than one draw occluding another.

**Why, on this side.** No reason on record beyond `Batch`'s note, which frames
reordering as a way to cut pipeline binds rather than as a way to avoid overdraw --
a smaller prize, and it is the one the comment weighs. The attachment is not the
obstacle: `emblema-hal-vulkan`'s `stencil.rs` already allocates a combined
`D24_UNORM_S8_UINT` or `D32_SFLOAT_S8_UINT` image for the clip, so the depth half is
already allocated and already paid for on every target that clips. What is missing is
depth state on the pipelines, which every one of them compiles ahead of time, and a
pass over recorded draws that knows which are opaque.

**Impact, estimated rather than measured, and small for the scene that prompted it.**
In the bench's frame the only opaque draws are the gradient ground and the three
cards; both shadows and the blurred highlight are translucent and cull nothing. Three
rounded rectangles of side 216 with radius 38.88 cover 136,075 of 2,073,600 pixels,
which is 6.6 per cent of the frame. Culling that much of the ground saves about 0.68
ms of a 13.911 ms Vulkan frame -- **near five per cent**. The clear-color hoist does
not apply at all, because the ground is a gradient and a clear is one color.

So this is worth recording and is not worth building for this frame. The prize scales
with how much opaque content a scene stacks, and nothing here measures a scene that
stacks much: an interface with opaque panels over opaque backgrounds is where the
number would be larger, and this repository has no such scene to measure. That is the
same shape as entry 19 -- a real upstream mechanism whose value here is bounded by
what the corpus and the bench actually draw.

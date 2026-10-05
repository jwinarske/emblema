# Changelog

This file starts at the point there was a version on a registry to anchor it
to. Everything before that is in the commit history, which is written to be
read: a commit here explains why a change is shaped the way it is rather than
restating what moved.

Dates are the day a version reached crates.io.

## 0.2.0 — 2026-10-05

The first release under the `emblema` names. `impeller-rs 0.1.0` and its thirteen
`impeller-*` crates stay on the registry as the record of what shipped on
2026-09-21; this is a hundred and two merges later and the names it publishes
under are new, so the number starts again rather than continuing theirs.

### The rename, and the filters

**The project is `emblema`.** It was `impeller-rs`, a name that read as a port of
upstream Impeller and set the expectation this renderer is measured against
upstream's output. It is not one: it targets devices where upstream's
stencil-then-cover route is not the fastest available, and `docs/non-parity.md`
records where it diverges on purpose. Every crate is `emblema-*`, the facade is
`emblema`, and the three environment variables this workspace owns are
`EMBLEMA_DRM_CARD`, `EMBLEMA_SHADER_SNAPSHOTS` and `EMBLEMA_COST_BASELINE`.

What keeps the upstream name is what upstream names: `emblema-capi` still builds
`libimpeller.so` against `impeller.h`, because the C ABI is upstream's and a
consumer loads it by that name. `ImpellerGetVersion` and `IMPELLER_VERSION` are
part of that ABI and are unchanged.

**A morphology radius is a length in the caller's space, as upstream's is.** It
was device pixels, alone among this renderer's lengths, on the recorded grounds
that upstream had no morphology to be in parity with -- and upstream has `dilate`
and `erode`, whose radius `entity.GetTransform() * effect_transform.Basis()`
scales at the pass. `Layer::scaled_by` now converts it with the sigmas and
`ImageFilter::scaled_by` converts a dilation handed over as a filter, so the two
spellings agree. **This changes what a scaled dilate or erode draws**: under a
scale of two a radius of eight now reaches sixteen device pixels rather than
eight.

The rounding moved with it. A structuring element is a set of sample positions
and there is no half of one, but the positions are device texels and the caller
states a local length, so rounding at construction rounded the wrong quantity --
1.5 at a scale of three became two and then six, naming positions at four and a
half. `Morphology::applied_radius` rounds where the device radius is known, and
is now the one place the value is finalized.

**And the conversion is now per axis, which closes the rest of it.** That radius
was converted by one factor -- `max_scale_of`, the larger of the two transformed
basis lengths -- and the passes walked the target's own axes. Upstream transforms
one direction vector per pass and takes a length from each. `axis_scales_of` now
takes the lengths separately, as upstream's `ExtractScale` does, and
`morphology_passes` walks `BlurBasis`'s directions the way `blur_passes` already
did. **This changes what a dilate or erode draws under an anisotropic scale or a
rotation**: radii of eight and three under `scale(2, 5)` reach sixteen and fifteen
rather than forty and fifteen, and a rotated dilation spreads along the caller's
axes rather than the screen's. Under a shear or perspective the basis does not
decompose and both fall back to the target's axes, which `docs/non-parity.md` 17
records as what remains.

**A blur's deviations scale per axis too, which closes the pair.** They took the
single largest factor, so a layer blurred with deviations of six and two recorded the
same sigmas under `scale(2, 3)` as under `scale(3, 2)` -- two transforms that
transpose each other producing one picture, because one number cannot tell them
apart. Each component now takes the length of its own transformed basis vector, as
upstream's `ExtractScale` does: twelve and six one way, eighteen and four the other.
**This changes what a blurred layer draws under an anisotropic scale.**

**A backdrop blur is converted per axis too, and the field did not have to change.**
`Layer::backdrop_blur` is one `f32` and `with_backdrop_blur` takes one sigma, which is
the right thing for a caller to state: it means a blur round in *their* space. A round
blur under a transform whose axes scale differently is an oval in device space, and the
defect was converting it to a single number of device pixels -- so a frosted panel came
out round at the larger factor on both axes while the same deviations on the layer
stretched. The field now stays in the caller's space and `open_layer` converts it where
it becomes an `ImageFilter::Blur`, which has two components to hold the answer. **This
changes what a backdrop blur draws under an anisotropic scale**: six under `scale(2, 3)`
reaches twelve across and eighteen down rather than eighteen on both. The public API is
unchanged, and `docs/non-parity.md` 20 is closed.

`emblema_present_drm::pacing` counts the vertical blanks a frame loop did not land
on, from the sequence the kernel reports with each completed flip -- which the event
loop previously discarded. `KmsOutput` gains `pacing`, `exact_frame_nanos` and
`framebuffer_count`, all inherent so the published `OutputEvent` keeps its shape, and
the panel example reports the ledger and can be made to miss on demand.

What that measures on a board is now settled at both ring depths. A Pi 5 latches every
vertical blank at sixty a second on both display controllers at `DEPTH=2`, where the
ring cannot hold a finished buffer back to absorb a frame that overran -- so the figure
is a statement about the renderer and not about the ring. Growing the scene separates
the two depths sharply, which is what says the shallower ring was a real constraint
rather than an inert knob. `docs/on-a-board.md` has the tables and the preconditions.

Two fixes came with it. The panel example destroyed a frame's layer targets while the
GPU could still be sampling them, which the validation layer reports as
`VUID-vkDestroyImage-image-01000` and which `submit_recording` exists to prevent. And
a comment in the atomic-commit path called the fence-on-commit path unverified, when
two devices demonstrate it.

**A composed image filter nested deeper than 256 is refused rather than fatal.**
Everything that reads a composition walks it recursively -- `peel`, `covering`,
`is_identity`, `scaled_by` -- as does the pass building after it, so a deep enough
`ImageFilter::Compose` took the process down instead of returning an error: a debug
build serviced two thousand and forty-eight levels and died at three thousand and
seventy-two. The limit is checked iteratively, so the check cannot overflow on the
input it exists to refuse, and it costs the limit rather than the filter's size.
Each level also costs a pass, so the accepted depth is already a recording of two
hundred and sixty passes. Not covered, and documented as such: a composition is a
chain of `Box`es, so building one deep enough overflows the stack when it is
*dropped*, in the caller's own code, before anything here sees it.

**A dash pattern finer than the curve is flattened to no longer hangs.** `walk`
counts intervals rather than distance, and `Dash::is_usable` cannot see how the
intervals compare with the path -- so a period of `f32::MIN_POSITIVE` over a
hundred-unit line asked for around ten to the fortieth dashes. It never got
there: the position the intervals accumulate into stops advancing at about
`2e-31`, where adding one falls below the last bit of an `f32`, and from there the
loop emitted geometry forever without moving. `dash_path` now returns such a path
unchanged, on the same terms as an unusable pattern, and `walk` refuses an
interval that cannot move the position it is added to so that termination does not
depend on the caller's tolerance.

The same loop had a second way not to end, and `is_usable` is why it was missed:
it sums the intervals as the caller gave them, while `Dash::cycle` doubles an
odd-length pattern so it alternates, so the total the walk runs against can be
twice the total that was checked. `[f32::MAX, 118.0, 370.0]` sums to `f32::MAX`
and doubles to infinity; `phase.rem_euclid(inf)` is infinity, and the loop that
normalizes the phase subtracted intervals from it forever. A non-finite period is
now refused, and that loop guards its own progress rather than trusting a check
made against a different sum. Found by generating a `Paint` field by field,
which is also new: nothing generated one before, and three of its fields are
sanitized by a builder and public anyway.

**Every spelling of "filter this" now measures its lengths in the same space.**
A caller's blur sigma is stated in the space they were drawing in, and
`Layer::scaled_by` converted the fields a `Copy` layer carries while nothing
converted the `ImageFilter` handed alongside them. So under a scale of two an
eight-pixel blur came out as sixteen device pixels through `Layer::with_blur`,
`Paint::with_image_filter` and `Layer::backdrop_blur`, and as eight through
`Canvas::save_layer_filtered` and `Canvas::save_layer_backdrop`. The third of
those against the fourth is what makes it a defect rather than two conventions:
both hand an `ImageFilter::Blur` to a layer, and only the one that becomes a
layer on the way was scaled. `ImageFilter::scaled_by` closes it, reaching through
a `Compose` into both halves. A morphology radius is still device pixels in every
spelling, which is non-parity 17 and is now asserted to be the same in both
rather than merely divergent in one.

A corpus scene pins what space a morphology radius is in. `dilate` and `erode`
measure in device pixels here and nothing scales them, where upstream's radius is
a local length the transform scales at the pass -- so a dilated layer under a
scale of two spreads twice as far there. `layer-dilated-under-scale` is the same
cross at half the size under that scale, landing on the pixels the unscaled scene
covers, so the dilation distance is all that can separate the pair, and
`the_dilation_under_a_scale_reaches_twice_as_far` reads the radius out of the
recording. The divergence is recorded as non-parity 17; the test is written to
fail if the convention is flipped rather than to endorse it.

**A morphology radius no longer decides how many passes a recording holds.**
`morphology_passes` emits one pass per `MORPHOLOGY_TAPS` texels of radius and
nothing bounded the radius above, so `Morphology::dilate(1e6, 1e6)` recorded
sixty-two thousand passes and `1e20` exhausted memory -- through the documented
constructor, whose sanitizing rejects only what is negative or not finite. A
`Morphology` built as a struct literal could also carry infinity, which the pass
loop never came back from. `Morphology::applied_radius` clamps to the extent of
the target axis, which cannot change a picture: a window reaching as far as the
target is wide already spans it. Found by a generated `Layer`.

Generated canvas operation sequences and generated `Layer`s in
`emblema`'s hostile-input suite, which is what found the above. Up to forty
calls whose order is hostile -- unbalanced restores, clips under a degenerate
transform, layers left open at `finish` -- and every `Layer` field, built as a
struct literal so a new field breaks the build rather than going quietly
uncovered.

dma-buf negotiation gets generated input on both halves. The `IN_FORMATS` parser
is put through blobs built the way the kernel lays one out and then corrupted --
truncated anywhere, given a version that does not exist, and given a header
claiming arrays the bytes do not hold -- asserting it advertises only formats and
modifiers those bytes carry. `negotiate` is put through generated pairs of
advertised sets, asserting an agreed layout is one both sides listed and that a
refusal means nothing asked for was shared. `proptest` rather than a fuzzer, for
the toolchain reason `docs/architecture.md` already records.

`Capability` and `Withheld` in `emblema-hal`, and a `withheld` field on
`ContextConfig` and `GlesConfig`, so a context can be built lacking a capability
the device has. Test support: the refusal paths and the branches that decide
whether to skip could previously only run on a machine whose device lacked the
thing, and one test said as much about itself. `Validated::without` is the call a
test makes.

Withholding only, which is soundness rather than preference -- a Vulkan device is
created without the advanced-blend features structure when that capability is
false, so granting one would mean pipelines built against a device that never
enabled the feature. `docs/architecture.md` has the rest, including why a
restricted context must not reach the corpus.

Additive, so nothing a caller has written changes. `Capability` is not
`#[non_exhaustive]`: the backends are separate crates and a wildcard arm would let
one honor a new variant while the other ignored it, which means adding a variant
later is a breaking change. That is written down beside the type.

### What a frame costs

**A draw that a later opaque draw covers is no longer drawn.** `Batch` confines
each draw to the pixels nothing above it replaces, by scissor rather than by
reordering — depth was measured first and ruled out, which
`docs/non-parity.md` 21 records along with why. On a Pi 5, three runs a side:
`stacked interface` goes from 16.074 ms to **5.198** under Vulkan and 17.304 to
**6.134** on GLES; `stacked, plus panels` from 13.686 to **4.478** and 14.977 to
**5.195**. Two thirds of the frame, sixty-two frames a second to a hundred and
ninety-three. `full frame, mixed content` is the control and does not move, its
cards being rounded and so not occluders. Nothing about the API changes and no
picture changes; `culling_hidden_pixels_changes_no_pixel` is what says so, and
`Canvas::keep_hidden_draws` turns it off for that comparison.

**A small simple contour is triangulated by ear clipping rather than by a
sweep.** Verified simple first — a quadratic edge-pair test — and sent to `lyon`
when it is not, so fill rules are untouched. The bench's twelve-point concave
recording goes 0.175 ms to 0.118, a third off. The cap is sixteen points, taken
from where proving a contour simple overtakes what the sweep costs on this
project's reference A76 rather than on a workstation, which puts it at twenty
there and thirty on x86-64.

**A rectangle's fill is tessellated rather than evaluated from a distance
field.** Upstream's `FillRectGeometry` emits a four-vertex strip for the same
reason, and the measurement is the one that decided it: multisampling a rect
fill costs 1.23x on V3D and 2.62x on PowerVR, not the four an analytic route was
avoiding. Strokes stay analytic.

**An axis-aligned gradient on a rectangle is interpolated across vertices.**
Upstream tries the same path before its uniform walk. One quad per pair of stops
carries the stop colors and the rasterizer interpolates, so no fragment
evaluates a gradient and a five-stop wash bakes no 256-texel ramp. It applies
where the endpoints are the rectangle's own edges, the stops share one alpha, and
the tile mode is `Clamp` — the last two because the two routes genuinely differ
otherwise, by 58 and 56 of 255 respectively, both measured. Elsewhere the
fragment walk runs as before.

### Fixed

**A translucent vertex-colored mesh was treated as an occluder, and the picture
was wrong by 128 of 255.** `draw_vertices` builds an opaque `Material::Solid`
under `Modulate` with per-vertex colors that multiply it, which the occluder
predicate admitted on the reasoning that a solid fill carries no per-vertex
color. An indexed quad of translucent vertices culled the draw beneath it. The
predicate reads the vertex colors now and refuses any that is not white.

**Dithering is a property of the material rather than of the shader's reading of
it.** It was decided in the fragment stage by testing the material kind against
the four gradient kinds, which made the route and not the intent the thing being
asked — and a second route to a gradient under another kind stopped dithering
without saying so. No picture changes.

### What the suite checks

The corpus is pinned as images now (`tests/golden/`), which is the first
comparison here against something other than another render of this renderer.
Generated scenes are executed on a device rather than stopping at the recording,
and four comparisons that were written by hand for one change each are
properties over generated input: that culling changes no pixel, that the two
gradient routes agree, that the two backends agree, and that a scene renders the
same after other frames have run. Flattening's tolerance is asserted as a bound
for the first time, in device space as well as path space.

### Known, and recorded rather than worked around

`cargo xtask gate --software` is clean again on a machine whose lavapipe reports
advanced blending. Two driver defects are why it needs probes to stay that way:
a group composited under an advanced blend writes nothing on lavapipe, which is
reduced in bare `ash` and filed as mesa/mesa work item 16243, and a multisampled
layer holding a mask-blurred advanced-blend draw renders differently on GLES
depending on what preceded it, which is reproduced and open.
`docs/on-a-board.md` has both. A Vivante GC7000UL segfaults in its own SPIR-V
front end on one of this renderer's fragment shaders, reduced to a 192-word
module with no project code in it.

### Also

`cargo xtask release` prints the order the fourteen crates publish in and what
each step needs, with tests holding the two ways a manifest can make a crate
unpublishable while the workspace still builds.

## 0.1.0 — 2026-09-21

Everything, because this is the first version with an API. So this section holds the
whole project rather than a delta, and `docs/parity.md` and
`docs/playground-parity.md` describe its state far better than a list could — both
are checked by tests, so neither can drift from the code without failing the build.

Fourteen of the workspace's seventeen crates go. `xtask` is this repository's
tooling; `emblema-testkit` exists to test this workspace and its API is shaped by
that; `emblema-capi` produces a shared library that C consumers obtain from a build
rather than from cargo. Each says so in its own manifest, and each is one line from
changing its mind.

This section used to carry a list of what stood between here and a first release
with an API. It had one entry, `drawRSuperellipse` and `clipRSuperellipse`, and
it stayed on the list for a while after both were written -- which is the
duplication the paragraph above rules out. `docs/parity.md` says what is built,
a test checks that it says so truly, and a copy of that claim kept by hand here
is the same claim without the check. So there is no list.

An atlas tint in `Plus` saturates in the shader rather than at the target, and that
is now a recorded deviation rather than an accident. `blend_tint`'s arm for the mode
is `min(src + dst, 1)` while the hardware path reaches it as `One, One`, so the same
mode answers differently depending on whether a draw carries the blend on its paint
or as a per-sprite tint. The clamp was removed to make the two agree and put back
after benching a Pi 5: it costs 2.4 per cent on the Vulkan distance-field row and
1.2 on the GLES full frame, and buys agreement only on a floating-point target,
which nothing here presents. §13 of `docs/non-parity.md` records it with the
measurements.

A hairline drawn through `draw_line` lands on a pixel rather than between two. A
line one pixel wide whose center sits on a pixel boundary covers half of each row
it straddles, so it drew gray and two pixels soft where upstream draws it crisp:
`LineGeometry::GetPositionBuffer` carries the endpoints into device space, drops
the transform and rounds the constant coordinate to a pixel's middle. Gated as
upstream gates it, on a width of exactly zero and a transform that is a
translation and a scale, and narrower in one way -- a paint wanting a layer keeps
the ordinary path, since a mask blur's sigma is stated in user space and this
draws in device space. A two-point path is not snapped, there or here.

A point smaller than a pixel covers one, and a width of zero is a point. Upstream
widens a point field's radius to `max(radius, 0.5 / max_basis)` and refuses only a
negative one, so the smallest point it draws covers a whole pixel; this renderer
drew nothing at zero and drew a sub-pixel point at whatever the sample grid gave
it. Nothing is dimmed to pay for the widening, which is upstream's rule as well
and the opposite of what a thin stroke gets: a point's area already falls away as
the square of its radius.

A stroke width of zero is a hairline. `dart:ui` documents `Paint.strokeWidth` as
defaulting to zero and zero as "a hairline width", and Impeller widens it to the
thinnest line the device can draw at full coverage -- an explicit exception at the
top of `ComputeStrokeAlphaCoverage` rather than something falling out of the
dimming. This renderer read it as no stroke, on the reasoning that a width
animating to nothing should fade out rather than jump back to full at the end. The
default settled it: a Flutter app that strokes without setting a width drew a
hairline there and nothing here, which is not an edge a caller opts into. So the
jump comes with the rule. `Paint::is_visible` answers true for a zero-width
stroke now, and `StrokeStyle` grew `can_draw` beside `is_visible` to keep the two
questions apart -- the tessellator still cannot build a stroke of no width, and a
caller's zero is widened before it gets there.

A color filter that recolors nothing no longer costs a pass. `ImageFilter::Color`
reported itself the identity only when it wrapped `ColorFilter::None`, so a blend
in `Dst` mode -- return the destination untouched -- and an identity color matrix
each routed the draw through an offscreen and resampled it coming back, to arrive
at what they were handed. It asks `ColorFilter::is_identity` now, which knows all
three.

`ColorFilter::blend` no longer answers a `Result`. It never failed: a mode that
is affine in the destination becomes a matrix and every other mode becomes
`ColorFilter::Blend`, evaluated per fragment against the constant. Its own
documentation said the advanced modes were "refused rather than approximated",
which the code has never done, and eighteen call sites carried an `expect` that
could not fire -- several with messages stating the opposite of what the branch
they were on does. A caller drops the `expect`.

A stencil clip survives the pass a backdrop filter cuts. A backdrop cannot
sample the attachment it is writing, so the pass stops there and what follows
begins by drawing it back in -- which restored the color and not the stencil. A
stencil belongs to a pass, so with a clip of a shape a scissor cannot express in
force, every draw after a backdrop filter tested for a depth no pixel in the new
pass held and landed nowhere. The narrowings are made again in the pass that
follows the cut, at the depths they were made at. Fixed alongside it: on GLES a
pass that does not clear its color did not clear its stencil either, so a
rebuilt clip tested against whatever the renderbuffer was allocated with and the
same frame drew differently between runs. Every pass clears its stencil now,
which is what the Vulkan render pass already did.

A backdrop filter may be a matrix. `save_layer_backdrop` refused one, on the
reasoning that a filter is a pass and a pass that moves its image needs a target
sized for where the image went. The first half holds of every other filter and
the second does not follow: a backdrop is seeded rather than composited, and the
target it is seeded into is the layer's own -- fixed before the filter is
consulted, and the same size whatever the matrix says. So the matrix folds into
the mapping that seed already draws through, and costs no pass. Outside the
moved image the seed names no texel and the target shows through, rather than
the edge smeared across the gap. A matrix inside a composition is still refused,
having no seed to fold into, and so is one with no inverse or with a projection.

A layer whose matrix will move its result records over the pre-image of what the
frame can see, not only over the frame. A shape drawn outside the frame and
translated back into view used to be gone before the matrix ran; it arrives now.
It costs no memory: the pass's extent still comes from the content's own bounds,
so a magnifying inverse widens where a draw may land without allocating where no
draw went. A matrix with no inverse, or one carrying the region across the
vanishing line, keeps the old behavior, and a layer given explicit bounds is
unchanged.

A stroked rectangle with square corners is drawn by the distance field rather
than by the tessellator, where before only a round join was. An outline is the
difference of two offset shapes now instead of a band around one: a rectangle
grown by half a width is a rectangle, its corner still square, where the band's
outer edge at a vertex is an arc. `Material::RoundedRect` carries an
`outer_radius`, which the join decides and the shader cannot. A bevel is still
tessellated -- it cuts the corner off, which no offset of this shape does.

Two consequences worth stating. A translucent wide stroke with a miter no longer
covers a pixel twice, which is most of §13 of `docs/non-parity.md`. And every
pixel of the catalog and corpus is unchanged, since every stroke that was
already analytic had a radius, and for a radius the two formulations agree.

A mesh's texture coordinates are read by any shader, not only an image. A
gradient on a textured mesh takes its coordinate from the vertices, which is
what `dart:ui` means by them; the material is built without the geometry's
transform in that case, since the vertices have already applied it, and the
shader runs the same mapping on the coordinate that it runs on a fragment's
position. A caller's program is still refused, with a message saying why: a
program replaces the fragment shader and takes its coordinate from the fragment,
so there is nowhere for a per-vertex one to arrive. A solid paint now accepts
coordinates it cannot show, as upstream does.

A layer given both explicit bounds and an image filter sizes its target for the
filter's spread. It was sized for the layer's own blur and morphology only, so a
blur handed over as the layer's filter stopped dead at a stated bound where the
same blur set on the layer carried ten pixels past it -- two spellings of one
thing giving two pictures. A layer without stated bounds was always right, being
sized by a narrowing that already asks the filter how far it reaches.

`ColorFilter::blend` accepts every mode `ColorFilter.mode` takes, where it
refused the advanced ones. The affine modes still become a `ColorFilter::Matrix`
and cost the shader nothing beyond the multiply it was already doing; the rest
become a new `ColorFilter::Blend { color, mode }`, which the shader evaluates
per fragment against the constant using the same function a mesh's per-vertex
tint goes through. It blends against a constant rather than against the frame,
so unlike the same mode set on the paint it needs no framebuffer fetch and no
extension, and is available wherever the shader compiles.

A blur turns with the transform it was stated under. `dart:ui` states a
deviation per axis in the caller's own space, and the two separable passes now
run along the directions that space's axes point in once the transform has been
applied rather than along the target's -- so a quarter turn transposes the
picture exactly. Upstream reaches the same Gaussian by removing the rotation and
blurring in an un-rotated space, which is not available to a recorded pass with
a device-space target; §15 of `docs/non-parity.md` keeps the difference and what
it costs. A transform with perspective, or a shear, still falls back to the
target's axes.

A mask blur drawn with `BlendMode::Clear` erases by the blur's falloff rather
than clearing the layer's bounding rectangle, where the shape is a rounded
rectangle, a circle or an oval. `Clear` is the one coverage-ignoring mode the
evaluated blur admits, because on a coverage it means `dst * (1 - c)` --
`DstOut` against a white source, exact because `Clear` discards the source color
by definition. Upstream makes the same special case in the same one place. The
other six such modes, and `Clear` over a shape with no evaluated blur, still
take the layer route and still clear their bounds: §15 of
`docs/non-parity.md`.

`ImageFilter::Blur` carries `sigma_x` and `sigma_y` where it carried one
`sigma`, because `dart:ui`'s `ImageFilter.blur` takes both and upstream passes
both through. `ImageFilter::blur(sigma)` is the isotropic case and is what every
existing call site became; `ImageFilter::blur_xy` states the two. `Layer::blur`
is a `Vec2` for the same reason, with `Layer::with_blur` setting both and
`Layer::with_blur_xy` stating them apart. A pass whose deviation is zero is
skipped rather than run as an identity, which is what keeps a blur along one
axis from resampling the other. The passes run along the target's own axes, so
a blur whose deviations differ does not turn with a rotation -- §14 of
`docs/non-parity.md` has the measurement and what fixing it would take.

`Color` stores sRGB-encoded components rather than linear light. `Color::srgb`
keeps what it is given, `Color::linear` encodes on the way in, and `to_array`
hands back what is stored. Anything that decomposed a color and rebuilt it needs
`Color::srgb` on the way back, not `Color::linear` -- rebuilding with the latter
now applies the transfer function a second time. Render targets are plain
unsigned normalized formats for the same reason: an sRGB target would encode
what is already encoded.

`BatchDraw::to_uniform` takes the format it is about to be drawn into. Only the
backend knows that, and it is what decides a gradient's dither; the same
recording drawn into an eight-bit surface and a float one wants different
answers, so it could not have been a property of the recording.

`transform` was on that list too, taking a 2D affine where `dart:ui` takes a 4×4
and so admits perspective, and was described as a deliberate design limit
rather than an omission. That was an assertion with no argument behind it,
unlike the superellipse decision beside it, and it is no longer true either way:
`concat_4x4` takes the same matrix `dart:ui` does.

Two things that a caller writing a runtime effect has to know about, and the
only reason they are not compatibility breaks is that nothing has been
published. An effect declares the paint's uniform block by hand, and `to_local`
grew from one vector to three so that a paint's mapping can carry perspective --
so `params` and everything after it moved. And an effect's program is linked
from this renderer's own vertex stage, whose varying at location zero is now a
three-component homogeneous clip position rather than a two-component one; an
effect reading it divides by the third component to get where it used to be.

Color states which primaries it is against. `Color` carries a `ColorSpace` --
sRGB, extended sRGB, or Display P3 -- and converts between them, matching what
`dart:ui` does; `Color::display_p3` states one exactly, which for a saturated
red means components outside zero to one, because the sRGB primaries describe a
smaller triangle than P3's. Nothing between the paint and the target clamps, so
those components reach a floating-point surface intact, through layers,
gradients and filters.

The GLES backend has the advanced blend modes. It reports
`GL_KHR_blend_equation_advanced` as the capability now rather than refusing the
fifteen modes outright, so the same picture reaches both backends -- 29 of 29
modes are checked against the compositing equations on GLES where 14 were.
Nothing about the API changed; a caller that was refused is not.

A blur's sigma is in the space the drawing is in and scales with the transform,
where it used to be in device pixels. `Paint::with_mask_blur`,
`Layer::with_blur` and `Layer::with_backdrop_blur` all change meaning: the same
number under a scale of three is now three times the blur, which is what
`dart:ui` states and what upstream does. `draw_shadow` divides by the scale to
cancel it, so a shadow's softness is unchanged -- also upstream's arrangement.

`drawPoints` in `PointMode::Points` records one draw where it recorded one per
point. The dots carry their centers on the vertices rather than in the paint, so
they share a material; the edge is unchanged, to the byte. A paint that is not a
single solid color, or one carrying a mask blur or an image filter, keeps the
per-point route.

`drawImageNine` records one draw where it recorded nine. The patches carry
their texture coordinates on the vertices rather than each carrying a source
rectangle of its own, with a half-texel inset standing in for the clamping a
per-draw source rectangle used to give. The picture is unchanged, to the byte.

A rounded superellipse costs a quarter of the geometry it did. The
conic-to-quadratic conversion subdivided until the curve's *weight* was near
one, which is a proxy that doubles its output per step; it now stops when the
approximation error itself is small enough, at the same relative tolerance. The
outline moves by a few thousandths at its tangent extremes, which is four
pixels of a hundred-and-twenty-eight-square frame.

A stroke of any width tessellates. `Paint::stroke(color, 1e30)` with a round
join used to overflow the stack inside the tessellation dependency, which no
caller can catch, and a `MAX_STROKE_WIDTH` bound refused such widths rather than
hand them over. That defect is fixed upstream in `lyon_tessellation` 1.0.21,
which the workspace now requires, so the bound is removed and `MAX_STROKE_WIDTH`
is gone from the public API — `dart:ui` states no maximum stroke width and
neither does upstream, and a limit nobody else has needs a reason that outlived
the crash. Widths past about a thousand million lose precision rather than being
refused, which `docs/architecture.md` measures.

A path is bounded before it is tessellated. A coordinate past two to the
twenty-fourth is refused rather than drawn, because past that a float cannot
name a pixel and because a stroke at a larger one grows without bound -- three
verbs at `1e15` used to stroke to thirty-one million vertices. A tolerance that
is not a positive length falls back to the default rather than being passed to
a dependency that asserts on it. `Path::is_within_tessellation_range` and
`MAX_COORDINATE` are new and public.

The workspace denies `unwrap`, `panic!`, `todo!` and `unimplemented!` in
shipping code, and `unsafe_op_in_unsafe_fn` everywhere. Nothing in the public
API changed; a handful of internal `unwrap`s became `expect`s that say what
they are relying on.

A stroked rectangle keeps its join. `draw_rect` and `draw_rrect` at a radius of
zero handed a stroked shape to the fragment-evaluated route, whose stroke band
rounds a vertex whatever the join says, so a rectangular border came out with
rounded corners. They tessellate now unless the caller asked for a round join.

A stroke narrower than a device pixel draws differently. It is widened to one
pixel and dimmed by `clamp(2 * scaled_width, 0, 1)`, which is upstream's
arithmetic with upstream's constants, so a thin line fades with its width
instead of quantizing against the sample grid -- before this a stroke of 0.18
device pixels and one of 0.3 laid down the same ink and one of 0.15 laid down
none. A width of zero still draws nothing, which is the one part of upstream's
rule this renderer does not take and is `docs/parity.md`'s `strokeWidth` row.
`Material::with_opacity` and `StrokeStyle::with_width` are new and public.

A layer says which backdrop it filters. `Layer::with_backdrop_id` is `dart:ui`'s
`backdropId`: layers naming one filter the image captured the first time it was
used rather than each capturing afresh, which is a different picture wherever
they overlap and one capture instead of one per layer. `Layer` gains a public
field for it, so a struct literal naming every field by hand needs the new one;
`Layer::default` and the builders do not change.

Presenting a wide gamut is not built and is not claimed. The swapchain and the
scanout path are untouched, because the devices available for testing are a
software rasterizer and a virtual display controller and a Display P3 surface
cannot be exercised on either.

Two more signature changes, again breaks only because nothing has been
published. `execute_layers` takes the format its intermediates should use, so
that a layer can hold what the frame it composites into holds; `execute` derives
it and none of *its* callers change. And `Context::read` was documented as
returning tightly packed RGBA8 when it has always returned whatever the surface
format packs -- eight bytes per pixel for a floating-point surface, as four
half-floats.

Four defects were found and fixed on the way, none of which needed wide gamut to
be worth fixing. Both sRGB transfer functions compared the signed value against
the knee where the standard means the magnitude, so every negative component
took the near-black linear segment. The GLES backend built a color attachment
for every texture it created regardless of what the caller asked for, which
turns a sampling-only texture into a creation failure for any format that is
filterable and not renderable. Its transfer paths named the channel layout from
the format and the component type from a literal. And a gradient of more than
four stops was tabulated through eight bits, so adding a stop that changed
nothing about a gradient changed the picture by twenty-four levels once a color
filter brought the difference back into view.

## 0.0.0 — 2026-08-22

A placeholder holding the name, containing no API.

Published because `impeller` on crates.io belongs to an unrelated crate that
got there first, so the name this project actually builds under needed
claiming before it went the same way. The version is `0.0.0` so that it sorts
below anything real and `cargo add impeller-rs` will not resolve to it once a
release exists.

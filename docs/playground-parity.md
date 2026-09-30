# The playground, against Impeller's

Impeller's playground is not a separate program. It is a mode of its test
suite: a `TEST_P` that calls `OpenPlaygroundHere` renders one frame, and with
`--enable_playground` set it opens a window instead of running headless. Its
scenes are therefore its tests, grouped by topic across
`impeller/display_list/aiks_dl_*_unittests.cc`.

This document is the inventory. What that suite contains, what is reproduced
here, and what is not — with the reason named, so a row is either work to do or
a decision that has been made rather than an unexplained gap.

## How this one is arranged

Two collections, both data rather than code, so both reach the window and the
test suite from one list.

The **corpus** is small and tuned. Each scene is there because it exercises
something no other scene does, its tolerance is argued about individually, and
it is compared against a software reference as well as across backends. A
failure in the corpus names a capability.

The **catalog** is this document's subject: the same pictures Impeller's
playground offers, as far as this renderer can draw them. Broad rather than
minimal, one tolerance for the whole collection rather than one per scene. A
failure in the catalog names a picture.

Keeping them apart is deliberate. Several hundred scenes organized around
another project's test suite would destroy what makes the corpus useful, and
folding the corpus into the catalog would lose the per-scene rigor.

## The count

Counts are from the files as fetched, and are the number of `TEST_P` cases
rather than of playground scenes — most call `OpenPlaygroundHere`, a few only
assert. The blend count includes the cases a macro generates per blend mode,
which are the majority of that file. Nothing here checks any of them
mechanically: this repository has no copy of that source, and a number in a
document that nothing verifies is a number to treat as approximate.

The one number describing *this* repository is checked. A scene added to the
catalog without the count following it fails a test, because an inventory that
drifts from what it inventories is worse than none.

The last column says what stops the rest, and it is worth reading with some
suspicion: an entry there is a claim about this renderer, and a claim nobody
tests goes stale quietly. Six of them have now, and the two most recent
were written by whoever was checking the others -- which is the argument for
the suspicion rather than against it. Blurs under rotation and
clipping together were listed as blocked and were merely unwritten -- both
render, and both agree across the backends. Mipmapped cases were genuinely
blocked and are not any more. Runtime effects were listed as blocking the
vertices file and never did: a mesh without texture coordinates takes its
material from the paint's shader like any other geometry, so a caller's
program reached it through the ordinary path and nothing had to be built.

That row had a blocker again for a while, and it was half real. A mesh *with*
texture coordinates was refused unless the paint was an image, and the entry
recording it said plainly that this was unbuilt rather than decided -- the
refusal was a bare `return Err` with no comment beside it, in a file where every
refusal around it carries a paragraph. It is built now for every shader but one,
and the gradient scene it blocked is mirrored.

What remains is a caller's program, and that is a real obstacle rather than an
unwritten one. Every other shader is a branch in this renderer's own fragment
shader, which sees the interpolated coordinate as an attribute; a program
replaces that shader outright, so the attribute reaches nothing. `docs/non-parity.md`
section 14 has what was built, including why the obvious version of it draws a
wrong picture that looks like a right one.

Difference-of-rounded-rects was the interesting one, because it was half
true. `draw_drrect` existed and had a test, so the renderer was not what
stopped it -- the scene format was, having no shape to say it with. A
capability the catalog cannot express is not covered by the catalog whatever
the API can do, and the cross-backend comparison is the thing being missed:
the only bug found so far in a backend's sampler bindings passed every test
on the other backend and was caught by a plate.

The two recent ones were of different kinds. "Wide gamut" went stale twice
over: the pipeline gained one, and while that was being written this document
said in the same breath that dithering was the only capability missing, which
was a contradiction no test here could see. "Mask blurs over a gradient" was
true when written and was then declared blocked on a decision that turned out
not to exist -- a mesh is refused elsewhere, a glyph run tints one color
whatever is done to it, and what was left was a shape with a fill that varies,
which is well defined and is now built.

Three of the four primitive-shape scenes are written --
`primitive/can-render-skewed-circle-hairline`,
`primitive/can-render-skewed-rect-hairline` and
`primitive/can-render-transformed-rect-with-near-vertical-edge-hairline` -- and
`a_hairline_holds_its_weight_under_a_transform` states what the chapter is for as
a pair of numbers: the outline has to be there, and the widest unbroken run of it
on any row has to stay small. The fourth is `PrimitiveShapePlayground`, sliders
and a shape chooser, which is a harness rather than a picture.

"A hairline skew" was never an obstacle and is not one now. A shear is no
difficulty: half a pixel of width under one draws exactly as it should, and a
test says so. The other half, a stroke width of zero meaning the thinnest line a
device can draw, is what `dart:ui` documents and is now what it means here as
well -- so nothing in that scene is out of reach.

This used to add that upstream's own handling was unsettled enough that copying
it was not the obvious move. Read at tip of tree, that is not so. Upstream's
rule is one rule and it covers every stroked geometry -- line, circle, arc and
path all widen to `max(width, kMinStrokeSize / max_basis)` and all dim by
`clamp(2 * scaled_width, 0, 1)` -- with zero falling out of it as a full-opacity
hairline, plus a half-pixel snap for an axis-aligned line under a
translate-and-scale.

Reading it that way found that this renderer had the *thin* half wrong and not
only the zero. A sub-pixel stroke was drawn at the width it asked for, which
against a four-sample grid meant a stroke of 0.18 device pixels and one of 0.3
laid down identical ink and one of 0.15 laid down none. Upstream's widening and
dimming were copied constant for constant, which left the deviation at zero and
nowhere else.

Both remaining pieces are taken now, and the argument that settled the first is
worth keeping because it is not the one that was being had. The case for reading zero
as no stroke was continuity: a width animating to nothing fades out rather than
jumping back to full at the end, and upstream jumps. The case against it is that
zero is the *default* value of `Paint.strokeWidth` and is documented as "a
hairline width" -- so a Flutter app that strokes without setting a width drew a
hairline there and nothing here, which is not an edge a caller opts into. The
discontinuity is upstream's on purpose, spelled as an exception at the top of
`ComputeStrokeAlphaCoverage`, and it comes along with the rest of the rule. So
the row is no longer short by a decision.

The snap went with it, and it is the piece that decides whether a hairline *looks*
like one. A line one pixel wide centered on a pixel boundary covers half of each
row it straddles and draws gray and two pixels soft; upstream carries an
axis-aligned hairline into device space and rounds its constant coordinate to a
pixel's middle, so it covers one row fully. That is `LineGeometry` alone upstream,
so a two-point path is not snapped there and is not snapped here -- which is what
the four `path/draw-lines-with-*` plates are for, and
`the_same_thin_line_said_four_ways_says_what_it_should` now requires the line and
the path forms to differ in the hairline column and nowhere else. It used to
require them identical, with a note that if the line ever gained a route of its
own this is what would say so. It did, and it did.

So the whole of upstream's minimum-size rule is here: the widening, the dimming,
the exception at zero, the point field's own constant, and the snap.

All fourteen of the rows below have now been read against the file they name, at
tip of tree, and the results are set out after the table. One named its obstacle
correctly: text, where nearly all forty of that file's scenes shape a real font
and mirroring them would mean shipping a shaper.

Two more came close and are the ones worth reading. Shadows named a real
absence, and drew the wrong conclusion from it -- the optimization is indeed not
here, but what upstream's scenes check between them survives it, and the section
on that row is where the second look is recorded. Paths named nothing blocking
the nine it was short, which was right; its count was still one too high, for
the reason the atlas row's was four too high -- a test in the file that draws no
picture -- so it was right about the only thing the column claims to be about.

That makes three right and eleven wrong, in eight distinguishable ways, and the
ways matter more than the count.

An obstacle that had been removed and the row not updated (gradients). One
stated far more broadly than it held, two scenes rather than a chapter (blurs).
One resting on a precedent that a later change took away (meshes). One read off
the shape of a scene rather than off anything standing in the way (nested
opacity). One naming an upstream uncertainty that upstream has since resolved
(primitive shapes). One naming real obstacles but for three of twenty-one
tests, with a dozen unwritten (blends). One understating badly and missing the
only real obstacle in its chapter, which was not recorded anywhere -- that a
rounded rectangle here has one radius where `dart:ui` has eight (basics). One whose "see below" pointed at nothing, and whose own count turned out to be
the thing that was wrong (atlases). One that said its file was covered when
it was not, by a single scene that needed a capability the preferred device here
lacks -- which is how it came to be overlooked (clips). One that said its
picture cases were all present when two thirds of them were not (miscellany).
And one that said its chapter was bounded by test fixtures rather than by the
renderer, when the renderer is exactly what bounds it (runtime effects).

Three of the eight turned into renderer changes rather than document ones, and
more have since. The pattern has held every time it has been tested: on the
evidence above, the likeliest thing wrong with an entry in this column is not
that it names the wrong obstacle but that it names a real one governing far
less than the gap it is offered to explain -- or, twice now, one that is real
about upstream and not about this renderer.

Both of those turned up again in one sitting. The shadow row named an
optimization that genuinely is not here and concluded that mirroring its scenes
would check nothing, which is true of any one of them and false of the pairs
they come in. The vertices row named a refusal whose stated reason -- that a
caller's program has nowhere for a per-vertex coordinate to arrive -- was about
a fragment stage, when the vertex stage behind it is this renderer's own and
hands the coordinate on. The first cost ten plates and a shape; the second cost
a fixture and a refusal being deleted.

So the column is short again, and what is in it now is mostly of a third kind:
not an obstacle at all, but a test of machinery upstream has and this renderer
does not, which no amount of building here would reach.

One thing the checking did *not* find, and it is worth saying because the
opposite was nearly recorded: the "Scenes there" counts are in scenes and not
in tests. Two of them look badly stale against a count of `TEST_P` -- the blend
file holds twenty-one and the row says about eighty -- and they are not, because
`IMPELLER_FOR_EACH_BLEND_MODE` expands one test over every blend mode. The
column is right and the obvious way to check it is wrong.

| File | Scenes there | Here | Blocked on |
|---|---|---|---|
| `aiks_dl_basic_unittests.cc` | ~85 | 91 | nothing; see below |
| `aiks_dl_path_unittests.cc` | ~30 | 37 | nothing; see below |
| `aiks_dl_gradient_unittests.cc` | ~40 | 46 | nothing; see below |
| `aiks_dl_clip_unittests.cc` | ~6 | 8 | nothing; this file is covered |
| `aiks_dl_opacity_unittests.cc` | ~3 | 3 | nothing; this file is covered |
| `aiks_dl_blend_unittests.cc` | ~79 | 77 | a callback for one and a second implementation this renderer does not have for one; see below |
| `aiks_dl_blur_unittests.cc` | ~64 | 64 | nothing; see below |
| `aiks_dl_vertices_unittests.cc` | ~16 | 22 | nothing; see below |
| `aiks_dl_atlas_unittests.cc` | ~11 | 12 | nothing; see below |
| `aiks_dl_shadow_unittests.cc` | 30 | 24 | twelve casters the scene model cannot spell or cannot wind, measured as near-duplicates of ones it can; see below |
| `aiks_dl_primitive_shape_unittests.cc` | 4 | 3 | one is a playground harness with sliders; see below |
| `aiks_dl_text_unittests.cc` | — | 7 | shaping and font parsing, which are out of scope; glyph rendering is not, and these use synthetic coverage |
| `aiks_dl_runtime_effect_unittests.cc` | — | 15 | nothing; two of that file's tests exercise machinery this renderer does not have, which is not the same as a gap; see below |
| `aiks_dl_unittests.cc` | 40 | 29 | nine that are texture or dispatcher machinery rather than pictures, two the scene model cannot say, and two that cannot show what they are for; see below |

Three rows now carry an exact number rather than an approximate one.
`aiks_dl_unittests.cc` holds forty `TEST_P`, `aiks_dl_shadow_unittests.cc`
thirty and `aiks_dl_primitive_shape_unittests.cc` four, and none expands one over
a macro, so for those three the count in tests and the count in scenes are the
same number and there is nothing to approximate.

The primitive-shape row said two, and it had said two since it was written. It
was never checked against the file, because nothing in it could be written while
a stroke width of zero drew nothing -- all four of its tests stroke at zero -- so
the row was a placeholder that looked like a count. That is the fourth wrong
number in this column and the first that was wrong for having nothing to be right
about.

Both of those numbers were wrong when first written -- thirty-nine and
twenty-nine -- and the reason is worth more than the correction. They came from
matching `TEST_P(AiksTest, Name)` on one line, and upstream wraps a declaration
whose name is long enough onto a second. Counting the `TEST_P(` lines and
counting the names extracted from them give different answers, which is what
caught it: forty against thirty-nine here, thirty against twenty-nine in the
shadow file, and eighty-five against eighty-three in the basic one. The rest keep their
tildes and should: the paragraph below on `IMPELLER_FOR_EACH_BLEND_MODE` is why
counting them that way is wrong.

`aiks_dl_basic_unittests.cc` is the reason the others keep theirs, and not for
the macro reason. It holds eighty-five `TEST_P` and two of them draw nothing at
all: `FormatSRGB` asserts the context's default color format is one of the two
eight-bit ones and `FormatWideGamut` asserts it is `kB10G10R10A10XR`, both
`EXPECT` and no playground. So that file is eighty-three scenes and eighty-five
tests, and its tilde is carrying a real difference rather than an unchecked
one. Of the rest, two more are named rather than counted, because they are a
capability question and not an arithmetic one.
`CanRenderColorFilterWithInvertColors` and its `drawPaint` twin put a *blend*
color filter and an inversion on the same paint, and a `ColorFilter` here is
`None`, `Matrix` or `Blend` with no composition among them --
`docs/parity.md` reaches `invertColors` through a matrix, which is the other
half of the same paint. Folding the two into one matrix by hand would draw the
right picture and test something else: upstream's filter is yellow over
anything, so the composition collapses to a constant and a plate stating the
answer would agree with itself. That is the scene model's gap rather than an
unwritten scene.

Two more were never adjudicated because the name match never saw them, both
declarations that wrap. `MatrixImageFilterDoesntCullWhenScaledAndTranslatedFromOffscreen`
and `CanPerformSaveLayerWithBoundsAndLargerIntermediateIsNotAllocated` are
ordinary pictures, and they are written:
`basic/matrix-image-filter-doesnt-cull-when-scaled-and-translated-from-offscreen`
and `basic/save-layer-with-bounds-larger-than-the-frame`. The third the wrap
hid, `TranslucentSaveLayerWithColorFilterAndImageFilterDrawsCorrectly`, is in
the `dl` chapter and written too.

The seven that had never been adjudicated are adjudicated now: five written and
two covered from elsewhere. Reading each of them is what settled it, in the order
below, and the criterion each was settled on is at the end.

`CanRenderClippedBackdropFilter` states the layer's
bounds as the *bounding rectangle* of a rounded clip -- "the clip coverage and
SaveLayer size are the same", in its own comment -- so the corners have to come
off the composite because the clip takes them and not because the layer is
smaller. That is the arrangement a backdrop layer's target is now sized from,
which made it a guard rather than a fifth picture of the same thing, and it is
written: `basic/can-render-clipped-backdrop-filter`. The plate turned up two
defects of its own, neither about clipping: `filters_its_backdrop` counted only
the sigma spelling of a backdrop filter and not the general one, and
`ColorFilter::blend` answered a `Result` it never returned an error from.

`BackdropFilterOverUnclosedClip` pins the other direction of the same clip
stack. The superellipse sheet above fails if a parent clip is popped too early;
this one pushes two clips around a corner, pops them, and *then* opens an
unbounded backdrop blur -- so what the blur reads is everything the outer clip
admits, and a rebuild that replayed the batch's narrowings rather than the stack
would put the popped pair back and confine the blur to the corner. Nothing else
asked that. Written as `basic/backdrop-filter-over-unclosed-clip`, with
`a_clip_popped_before_a_backdrop_cuts_does_not_come_back` sampling six pixels
outside the corner clip, where a confined blur leaves white.

Three are adjudicated the other way, and saying which and why is the point of
keeping the list. `CanPerformFullScreenMSAA` draws one red circle and
is named for the target it draws into; a dozen plates here are circles at four
samples, so it is covered several times over.
`CoordinateConversionsAreCorrect` draws an image under a translation and a
scale, then an unbounded half-alpha group over three overlapping rectangles,
and asks that the offscreen land where the direct draw did --
`opacity/can-render-group-opacity-to-savelayer` is the second half exactly and
the image plates are the first. `PipelineBlendSingleParameter` cannot be a
plate: its feature is a blend color filter in `Dst` mode, which is the identity,
and a scene whose feature leaves the picture unchanged is rejected by the check
that every feature has to reach the picture. So it is a test instead,
`a_color_filter_that_recolors_nothing_still_draws_the_shape`, which holds the
picture its comment describes -- a green square in the middle of a blue circle
-- and the pass count, since the filter used to cost one.

`PerspectiveRectangle` states its projection first and its
clip after it -- so the clip is a region in projected space, its own edges go
through the divide, and it is in force over a run rather than narrowing one
draw. `clip/a-rectangular-clip-under-perspective` puts the projection on the item
instead, where the clip travels with that one draw, so this is a different
arrangement rather than the same one restated. Its term is on Y as well, where
every other perspective plate here puts it on X. Written as
`basic/perspective-rectangle`, with `a_projected_clip_narrows_across_the_plate`
requiring the lit span to be wider near the top than the bottom -- a clip taken
in frame axes draws the rectangle that was written down and agrees with itself
perfectly.

Writing it needed a defect fixing first, of the same kind the clipped backdrop
filter turned up. `asks_for_perspective` never tested a clip's *own* transform
while `plain` stripped it, which is the worst way round: this plate would have
reported no feature, been left out of the comparison that says a feature reaches
the picture, and passed as covered. `flatten_perspective` did not reach a clip's
children either, which `plain` hid by recursing through a clip itself.
`a_projection_on_a_clip_is_a_feature_and_comes_off` holds both halves.

`CanDrawPerspectiveTransformWithClips` is the largest of them, and three
things in it appear in no other plate: an image read through a projection, a
stencil clip stated in frame axes with projected content inside it, and a clip
whose geometry is drawn and popped before the draw it is meant to sit behind.
Upstream's own comments name that third one -- a clip drawn and restored "will
get drawn to the depth buffer behind the image", the oval that scopes the image
sits "in front of the image on the depth buffer" -- so it puts one on each side
of the image to say which depth a clip's geometry occupies. Written as
`basic/can-draw-perspective-transform-with-clips`, and both clips are checked
from the picture by `a_clip_drawn_before_an_image_does_not_bound_it`: the middle
has to hold the image, since the difference clip was popped before it drew, and a
point inside the image's projected quad but outside the oval has to come back as
ground. Turning the oval into a rectangle fails the second.

The projection stands in for upstream's rotation about Y, which this scene model
has no term for -- a plane turned about the vertical axis projects to a
horizontal divide, which is what the perspective term is. Said in the plate
rather than left implied, so a reader comparing the two does not go looking for a
rotation.

So the seven are adjudicated, and none is a
picture, and the catalog covers each subject from some other direction -- there
are backdrop filters under clips, perspective under a clip and under a curve,
images under a transform, and a great many multisampled circles. Whether that
counts as mirroring them is a judgment about what a plate is for, not a count,
and the criterion that settled them is worth keeping for the next inventory row
that stalls: not whether the subject appears elsewhere, but whether the test pins
an arrangement nothing else does. Five were yes and two were no, and the two are
named above with what covers them instead. Four defects came out of asking, none
of them about the subjects the scenes are named for, which is the argument for
asking rather than counting.

The catalog holds four hundred and thirty-eight scenes against a column totalling
about four hundred, and the two are not a ratio: five chapters hold more than
the file they mirror, because a scene here is one picture where a test there can
be a loop over every blend mode or a family drawn twice. What the totals meeting
does say is that the gaps left are small and named: every row is now short by
single scenes, by casters or programs the scene model cannot describe, or by
ordinary pictures nobody has written.

## What blocks the rest

Three different kinds of obstacle, worth separating because only one of them is
about the renderer.

**Capabilities this renderer lacks.** None, now that gradients are dithered.
It never was a row of `docs/parity.md` and should not be looked for as one: it
is not a `dart:ui` method but a quality behavior inside gradient rendering,
which is where the banding it exists to break up appears.

That sentence was written while two cells above it still said "wide gamut",
which made this document contradict itself for the length of one commit. Worth
recording rather than quietly fixing, because it is the failure mode this
column already has a warning about: prose no test reads goes stale, and the
person who wrote both halves is not exempt.

It went stale a second time, in the other direction and for much longer. The
gradient row went on saying "dithering" after the sentence above said there was
nothing left, so the two halves of the same claim sat nine lines apart
disagreeing, and the count under that row stayed where the blocker had left it.
Nobody had counted the chapter against the source -- this document said so
about wide gamut and the admission applied here too -- and counting it found
that nothing in the file was blocked at all. Eight scenes went in: the four
dithering plates the cell was named for, the incomplete-stops scene for the
three gradient kinds that were missing the one the linear kind already had, and
a gradient under a mask blur.

Counting it again, name by name against the file at tip of tree, found fifteen
more and no obstacle among them either. Eight are one family: upstream's
seven-stop ramp under each of the four tile modes, drawn linearly and then
swept. Seven stops is the number that matters -- four fit in the paint block and
are interpolated by the shader, a fifth sends the gradient to an uploaded ramp
texture instead, and `docs/non-parity.md` records that fork as a difference from
upstream worth watching. So these are the scenes that take the second route, and
they take it under every tiling rule.

The ramp covers a third of the shape in all eight, so what fills the rest is the
tile mode and nothing else, and the twelve pairs are asserted to be twelve
different pictures. They differ by between forty-four and sixty-nine per cent of
the frame. The mistake that would make: treating a decal as a clamp, which is
easy because both leave the ramp's end color at the boundary, and which every
other test here would let through.

The plate that was already under the clamp name has been given the same
geometry as its three new siblings. It spanned the whole shape, so there was
nothing outside the ramp for the mode to act on and it could not have shown
clamping if it had wanted to.

Five more are upstream's fast-gradient set: the same axis-aligned ramp
horizontally, vertically, and each of those reversed, drawn on a rectangle and
on a rounded rectangle beside it -- both shapes, because a per-draw
optimization is chosen per draw, and a renderer that took it for the rectangle
and not for its rounded neighbor would show the difference here and nowhere
else. The fifth pulls the endpoints inside the shape and repeats, which is
upstream's way of saying the condition has to be tested rather than assumed.
The stops are bunched at one end, so reversing is not a symmetry and a fast
path keyed on the axis that forgot the direction is visible.

The last two are a decal gradient recolored by a quarter of green -- where the
assertion is upstream's own comment, that the green covers the border outside
the ramp too, because outside a decal what the shader produced is transparent
rather than absent -- and a gradient blurred as an image, which is the other
order and a different picture.

A note on that chapter before the rest of it, because it was drawn wrong for
longer than any of what follows was missing. Ten of its plates put a mask blur
under `BlendMode::Src`, which is not a choice any of them made: `Item::fill`
defaulted to it at the time, upstream's paint defaults to `kSrcOver`, and `Src`
is one of the seven modes `docs/non-parity.md` section 15 names as erasing a
mask blur's whole bounds. So ten plates named after upstream's blur scenes were drawing that
deviation instead of the picture -- a blurred circle inside a hard-edged
rectangle of erased ground, which is what the section describes and none of what
upstream draws.

Nothing could see it. The cross-backend comparison saw both backends agree,
which they did; and `every_plate_that_asks_for_a_mask_blur_can_show_one` saw a
plate differ from itself with its blur taken away, which it did -- by the size
of the erased rectangle. That check was being satisfied by the artifact. Giving
the draws upstream's blend broke it on the one plate whose blur is genuinely too
small to see, `SolidColorOvalsMaskBlurTinySigma`, which had been written as one
oval at a made-up deviation where upstream draws three at nothing, a hundredth,
and one. It is the three now, and the check passes on the blur.

That fix stopped at the mask blurs and left the rest open. It is closed now, and
the way it closed is worth the paragraphs because the reason it stayed open was
a cost that turned out to be avoidable.

`Item::fill`, `Item::filled` and `Item::stroke` defaulted to `Src`; `dart:ui`'s
paint defaults to `srcOver`. Wherever a plate draws over something the two
differ -- at an antialiased edge, under a translucent color, and anywhere a fill
is transparent by design, which is what a decal tile mode makes it. Measured, by
changing the default and rendering the catalog either way: 65 of 418 plates
move. The largest are the ones where a fill is transparent on purpose -- a tiled
texture with a decal mode at 86 per cent of the frame, a linear gradient with
one at 66 -- because `Src` writes that transparency over the ground instead of
leaving the ground alone. Two translucent stroked-arc plates move by 40 per
cent, for the same reason with a different cause. Every one of those movements
is a correction: what they drew before is a picture upstream never draws.

What held it up was the second cost, and it was real as stated.
`Scene::tolerance` read `blend == SrcOver` as saying the fragment was computed
rather than replaced, and gave a scene that says so a unit of slack per store.
A solid `Src` draw was therefore compared *byte for byte* across devices, and
that strictness is what the default bought.

The way out is that the premise was too coarse. `SrcOver` is named as the mode
that combines the fragment with what is already there, but an opaque source
leaves nothing to combine with: `src + dst * (1 - 1)` is `src`, exactly, in the
shader and in the fixed-point store alike. So the question the derivation asks
is now about the *color* as well as the mode, and an opaque solid draw keeps its
byte-for-byte comparison whichever mode it names. Two derivation tests had been
leaning on the old imprecision and say what they mean now: the one that counts
stores through a group uses a translucent color, since counting stores that do
not happen counts nothing, and the one that checks the multisample budget uses a
rectangle, since a multisampled circle is answered by the analytic branch above
it.

`EXACT` also had to grow a second condition. A scene can be made entirely of
draws that store and still be composited out of a target, and the composite is
arithmetic whatever it composites. `picture-drawn-into-a-picture` is the scene
that says so: every draw in it writes a solid color and the two backends still
land two levels apart on a tenth of the frame.

Two things fell out of the change that are worth more than it cost. Two corpus
scenes -- `circle-antialiased` and `rounded-rect-stroked` -- drop from 114 and
216 vertices to 6, because `Src` does not respect coverage and was pushing both
onto the tessellated path; they are drawn from their distance fields now, which
is what upstream does and what this renderer prefers. And
`blur/composed-filters` turned out to be a second plate drawing the deviation
instead of its subject: its closing used a radius of nine against a notch
twenty-four across, so it filled nothing, and the only thing distinguishing it
from the same shape unfiltered was the ground `Src` erased.
`a_feature_a_scene_asks_for_has_to_change_the_picture` caught it the moment the
erasure stopped. The radius is fourteen.

The blur row was wrong in a more interesting way, and checking it changed the
renderer rather than the document. It said "backdrop identity keys", which was
real -- upstream's `SaveLayer` takes a `backdrop_id` so several layers can share
one capture of what is behind them, and this renderer's `Layer` had no such
key. It was right about the obstacle and wrong about the size of it: it named
two of the file's scenes and there are four, all of them now built along with
the key itself, so the row says nothing. It was also not what stopped the other
twenty-one, and writing those scenes is what found the thing that was: a mask
blur here took a solid color for three of its four styles.

Counting the file again found a second thing it was short of, and this one was
in the API rather than in the renderer. `dart:ui`'s `ImageFilter.blur` takes
`sigmaX` and `sigmaY`; this one took a single `sigma`, and `docs/parity.md` said
of the image filters that all seven kinds upstream offers are here -- true of the
kinds and not of that one. A blur is two separable passes here already, one
along each axis, so the second deviation was a field rather than a mechanism:
each pass uses its own, a pass whose deviation is zero is skipped rather than
run as an identity that would resample for nothing, and the reduction that makes
a wide blur affordable became per axis so that a hard blur along x does not
shrink and enlarge a sharp y.

Nothing in either collection moved, which is what says the reduction change was
safe: every blur here was isotropic on a square target, where per-axis and
shared reductions agree.

Two plates went in for it, a square smeared along x and the same square smeared
along y, asserted to be each other transposed -- either alone would say a
deviation arrived, and only the pair says which of the two it was. Replacing one
with an isotropic blur of the same deviation spreads the sharp axis from
thirty-two pixels to fifty-nine, which is what the assertion was written
against.

Upstream's own scene for this, `GaussianBlurRotatedNonUniform`, went unmirrored
for a while: the passes ran along the target's axes, so a layer turned
forty-five degrees with a blur along x alone smeared along the screen's x rather
than along the axis the caller stated. Thirty-nine by fifteen upright,
forty-five by twenty-one turned, which is the same horizontal smear applied to a
diamond.

It is built now, and by a different mechanism from upstream's. Upstream removes
the rotation -- it re-renders its input into an un-rotated space, blurs
axis-aligned there, and applies the rotation to the result, for a reason its own
comment gives as text quality. That is not available here, a layer being a
recorded pass with a device-space target and a stencil to match. What was
available is the blur pass's step, which was already a free two-vector: the
passes turn instead of the space. Same Gaussian, one tap landing between texels
rather than on one. `docs/non-parity.md` section 15 keeps the difference and
what it costs; the row above says nothing because there is nothing left of that
file this renderer cannot draw.

Four more plates from that file, and a second defect found by writing a fifth.
The four: a blurred circle whose clip cuts it *after* the blur, so the halo
stops dead at the clip's edge while the shape's own edge stays soft -- the
opposite picture from clipping first and blurring what is left; a color filter
over a mask blur, which upstream keeps because the two are easy to apply in the
wrong order; and upstream's channel swap composed with a blur, in both orders.

The compose pair is worth a sentence because the two are the same picture on
purpose. A permutation matrix and a weighted sum are both linear, so they
commute, and the plates agree to the byte. That agreement is the assertion
rather than a redundancy: a composition that applied only the outer filter would
leave one plate a recolored sharp circle and the other a blurred green one, and
nothing else here would notice. Dropping the inner one instead is caught by the
rule that a scene carrying a filter has to render differently without it.

The fifth was `ClearBlendWithBlur`, and writing it found a defect that has since
been fixed. A mask blur that cannot be evaluated in the fragment stage is drawn
as a layer, and the layer is composited with the caller's blend -- across the
layer's *bounds*. A mode that writes where its source is transparent writes
across all of them, so a blurred circle drawn with `Clear` cleared a rectangle:
9216 pixels, 96 by 96 to the pixel, where the sharp version correctly erased
4052 against a disc's 4071.

Reading upstream settled what to do, and the answer was narrower than either
option first written down. `Clear` is the one coverage-ignoring mode that can be
admitted to the *evaluated* blur anyway, because on a coverage it is not what
its factors say: clearing by an amount is `dst * (1 - c)`, which is `DstOut`
against a white source -- and white is exact rather than approximate, since
`Clear` discards the source color by definition.
`SolidRRectLikeBlurContents::Render` does exactly that, forcing white and
switching the pipeline to a reverse subtraction, and does no more. The plate is
here now, and the hole's alpha climbs monotonically out of the middle with a
corner of its old bounding box untouched -- which is the assertion that tells
the fix from the defect.

Four more after that. The periphery plate's twin, turned: a strip down the
middle running to the top and bottom edges, so the kernel reaches past the
target along the other axis. Its ground turns with it, and that is not
decoration -- a blur reading past the edge has to answer with the clamped edge
texel, and the way to see that it did is to blur *across* the stripes. Stripes
parallel to the blur would look the same either way.

A blurred layer under a mirrored transform, which is the one of the four that
could have drawn nothing. A mirror has a negative determinant, so a renderer
deriving the layer's extent by transforming its corners and subtracting gets a
negative width and a target of no size. The content sits left of center in the
layer's own space and has to land right of center on the frame; symmetric
content would have made the mirror invisible, and taking the flip out of the
transform fails the assertion, which is what it was written against.

A gradient under a mask blur inside a group at half opacity -- three layers deep
before the group's own opacity applies, since a mask blur over a varying fill
draws the fill across everything the blur reaches and masks it with a blurred
coverage. And an image under a mask blur, which is the same route again with the
fill being a sheet: what comes out is the image with soft edges rather than a
blurred image, because a mask blur acts on coverage and an image's coverage is
the rectangle it is drawn into.

Three more, and the chapter's remainder is now named rather than merely
uncounted. Upstream's clipped pair -- the same blurred image scaled into a
window, and scaled and turned -- goes in with both halves, because the clip is
stated outside the transform: the window stays put on the frame while what is
drawn into it moves, so the blur's target is decided in one space and its
contents in another. Two things are asserted and they pull opposite ways. The
window is filled edge to edge in both, which a clip applied in the wrong space
would break; and the two are different pictures, which a dropped rotation would
break while still drawing a plausible blurred window.

The third is a shape larger than the frame, moved so most of it is off the top,
under a blur wide enough that the halo alone fills the picture. Its second
contour is upstream's and is why the scene exists: a path holding more than one
contour cannot be recognized as a rounded rectangle, so the blur cannot be
evaluated in the fragment stage and takes the general route -- on a target sized
for a shape most of which is not there.

Upstream added six to this file since the last count. Five are the same
gradient oval stroke at each blur style translated and turned, and they are
mirrored below. The sixth is `BackdropGroupUsesCoverageUnionSnapshot`, which
draws nothing anyone looks at: it renders two grouped backdrop blurs sharing a
key, then reads upstream's render target cache and requires the intermediates to
be sized to the union of the two groups rather than to the canvas.

That is a claim about allocation rather than about a picture, and this renderer
does not satisfy it. Nothing is wrong on screen -- the plates sharing a backdrop
key draw what they should -- so it is a cost, which is why it is here and not in
`docs/non-parity.md`.

Half of the gap was a clip being ignored rather than anything to do with
sharing, and that half is closed: a backdrop layer opened under a clip used to
take a target the size of its parent, so a frosted panel a fortieth of the
frame's area recorded a frame-sized one. It takes the clip's size now, which is
exactly what a caller writing the clip out as the layer's bounds always got --
`narrowing_to_the_clip_records_what_stating_it_would` holds the two spellings to
the same recording.

The blur that filters the capture is narrowed too, which turned out to need no
padding at all: those passes read the whole capture and only have to *write* the
region the layer will hold, so what shrinks is the writing. A frosted panel
forty pixels across in a frame of a hundred and twenty-eight went from five
frame-sized passes to two, the other three being the clip's size -- 37568 pixels
of pass area against 81920.

A layer naming a key is narrowed on the same terms, which took reading the
sharing to see. It was left out of the first pass on the reasoning that a shared
capture has to cover every group naming the id -- true, and about the capture
rather than about the layer. The layer's own target holds only its own content,
which its clip binds whether or not the key is shared, so the exclusion was
protecting the wrong region. Two keyed panels in opposite corners now record
their own targets at the clip's size and read one capture and one filtered image
between them: six passes, of which two are narrowed, where before none were.

What is left is the capture and the filter over it. Both stay the parent's size
because both serve every group that names the id, and this renderer meets those
groups one at a time -- the first capture is made before the last sharer is
known, where upstream takes the coverage union at display-list build time and
sizes one snapshot from it. Filtering per group instead was tried and is worse
for the case the id exists for: a list of frosted rows would take one blur each
where upstream takes one in total, and the blur is where nearly all of the cost
is. The capture itself is the pass being cut, so it is the parent's size whatever
happens -- that one is not an allocation a layer asks for.

What is left of the file is eight, and each has a reason. Four drive a
callback rather than building one picture -- three named `Interactive` with
sliders, and `GaussianBlurAnimatedBackdrop`, which is the same shape without the
name. Two are unit tests that build a texture and assert it exists rather than
opening a playground, in the way four of the atlas file's are:
`GaussianBlurSolidColorTinyMipMap` and `GaussianBlurBackdropTinyMipMap`. And one
is `MaskBlurOnZeroDimensionIsSkippedWideGamut`, which needs a wide-gamut target
to present and is §4.

The two that were merely unwritten are written, each short of upstream in one
stated way. `BlurredRectangleWithShader` shades with a checkerboard that
upstream renders into a texture first, which is `Picture.toImage`; the fixture
sheet stands in, tiled the same way, since what the scene is about is an
image-shaded paint drawn plain and through a blur on rectangles and strokes.
`CanRenderBoundedBlurWithTranslation` blurs two bounded backdrops, one moved and
one moved, scaled and turned. Its turned band came out upright at first, and the
reason was the scene format rather than the renderer: a layer's target is an
axis-aligned box in device space on both sides, so upstream's tilted edge comes
from the clip it states under the same transform, and a clip could be put on one
draw here but not around a group. `Node::Clip` takes a shape and a transform
now, so the plate states the turn as upstream does and the band is tilted. The
plate checks the rest:
`a_bounded_backdrop_blur_moves_with_the_transform_it_was_opened_under` moves the
first band and unturns the second, and requires the picture to change both
times.

This paragraph said six and listed seven, and one of the seven --
`CanRenderForegroundAdvancedBlendWithMaskBlur`, whose color filter is a blend in
a non-separable mode -- has since been built and is
`blur/can-render-foreground-advanced-blend-with-mask-blur`. A list that is one
longer than the number in front of it is the cheapest thing to check in a
document like this one and nothing checks it.

The default style blurs coverage and fills through it, which works for any
fill. The three that combine a blurred mask with a sharp one -- solid, outer,
inner -- were built for a color only, and refused a gradient rather than
guessing at it. The refusal was right and the limit was not necessary: the
combination is the same rules whichever is being combined, so it now happens
once, over color where the fill is constant and over coverage where it varies,
and every style takes any fill. Eight scenes followed, five of them upstream's
stroked gradient oval under each style.

The last two rows are the two whose obstacle is a boundary this project drew,
and they came out opposite ways.

The text row is right. Nearly all forty of that file's scenes shape a real font
-- emoji, italics, subpixel alignment, a shadow cache keyed on glyph identity --
and a font file decides those pictures, so mirroring them would mean shipping a
shaper. Two that are about glyph *rendering* rather than about text were
mirrorable with synthetic coverage and are here now: a run inside a save layer,
where the atlas is resampled once more than it otherwise would be, and a run
that leaves the frame on both sides, which is where a coordinate computed from a
quad's own corners rather than from its visible part goes wrong. One that looked
mirrorable is not -- `TextForegroundShaderWithTransform` puts a gradient over a
run, and a run here takes a solid color, which `docs/parity.md` says in the
`maskFilter` row and `docs/non-parity.md` explains.

Four more of the blend file went in once an advanced mode became available as a
color filter, and writing them found something wider than the four. Upstream's
`EmulatedAdvancedBlendRestore` draws an advanced blend inside a clip and then a
shape the clip must still cut, and the plate rendered as a blank white frame:
the blend had drawn nothing at all.

It was not the clip and it was not this renderer. Measured on both backends at
both sample counts: on this machine's Vulkan software rasterizer an advanced
blend under multisampling produces no output, and at one sample it is correct;
on GLES, which is the same Mesa through a different extension, it is correct at
both. So it is the driver.

What made it worth chasing is what it had been hiding. Sweeping the whole
catalog -- every scene carrying an advanced blend, rendered again with those
draws *deleted* -- found seventeen plates rendering identically either way. The
catalog counted every one of them as coverage of a mode, and nothing could have
noticed: the cross-backend comparison skips them, because the device it prefers
has no advanced-blend extension at all, and "draws something" passes on the
gradient underneath.

Deleting the draw rather than substituting a plain mode is the part that took a
second try. Substituting source-over draws the shape, so a dropped advanced
draw and a working one both differ from it, and the first sweep reported one
affected plate instead of seventeen.

Two causes, and only one was the driver. Fifteen were multisampling and are
single-sampled now, which costs those plates nothing -- their subject is what a
blend computes, not where an edge falls, and the equation suite checks the
arithmetic separately. The other two could not have shown their mode on any
device: hue and saturation over a gray backdrop collapse to the backdrop, gray
having no saturation to exchange. The family's gradient runs through color now
as well as through value.

All twenty-one are asserted, and putting the sample count back fails the
assertion.

The other half of that file's macro output is here now, and it is not the same
family twice. Upstream generates every blend mode over a *draw* and again over a
*group* at half alpha, and the two cover different mechanisms: a draw's mode
combines one shape's color with the frame, a group's combines a finished image
with it after the group's own alpha has scaled what it holds. A renderer
applying the alpha after the mode would agree with the first family and disagree
with the second for every mode that is not linear.

Writing them found a second driver failure of the same shape as the first, in a
different draw. A group is composited as an image quad drawn with its mode, and
on this machine's Vulkan software rasterizer that draw produces nothing for an
advanced mode -- at any alpha, at any sample count -- while GLES, the same Mesa
through a different extension, is correct. The draw family works on both. So the
sweep that asks whether a plate can show its mode now asks *every* device here
and passes if any can, which is the honest claim rather than a device-specific
one, and the reason is written beside it.

The rule that found none of that has been widened since, because it should have
found some of it. A scene is rendered again with its features stripped and has
to come out different, and "features" meant image filters, mask blurs, tint
blends and morphologies -- not color filters, which are exactly as capable of
being asked for and not delivered. They count now.

Widening it caught one scene, and it was one written that same afternoon.
Upstream's `AdvancedBlendColorFilterWithDestinationOpacity` puts a `Saturation`
blend against a *transparent* color on a group, and that filter is the identity
by arithmetic rather than by accident: the compositing formula is
`cs·(1−ab) + cb·(1−as) + B(cb,cs)·as·ab`, and at a source alpha of zero every
term but `cb` vanishes. No renderer can draw anything from it. Upstream's own
comment says the picture "should be solid red as the destructive color filter
floods the clip", which is not what the formula gives and reads like it was
copied from the scene above it -- the flood case, which is already here as
`destructive-blend-color-filter-floods-clip`. So the scene is not mirrored, and
this is the reason rather than a capability.

The two that wanted a wide-gamut target no longer want anything.
`BlendModePlusAlphaWideGamut` and its color-filter twin both open by requiring
the default color format to be `kB10G10R10A10XR` and skipping otherwise, and
then add red to a bright ground through `Plus`: the sum passes one, so an
eight-bit target clips it and an extended-range one does not, and that difference
is the whole picture. The pipeline reaches a floating-point target now, so the
obstacle is gone -- what the column was really deferring to was §4 of
`docs/non-parity.md`, which is about *presenting* a wide gamut and says nothing
about rendering into one.

They are tests rather than plates, for the reason `PipelineBlendSingleParameter`
is: a catalog scene cannot opt into a floating-point target, because every test
binary here runs concurrently and peak allocation is their sum.
`plus_saturates_in_eight_bits_and_does_not_in_a_float_target` takes both routes
upstream takes -- the blend on the draw and the same arithmetic as a color filter
on a group -- and each is checked twice, once into a float target where it has to
pass one and once into eight bits where it has to clip.

Mirroring them found a difference in a third route neither of them takes, and the
difference is kept on purpose. The atlas chapter's `DrawAtlasPlusWideGamut` is the
same claim through a per-sprite tint, and that path clamps: `blend_tint`'s `Plus`
arm is `min(src + dst, vec4(1.0))` where the hardware path reaches the mode as
`One, One` and leaves the saturation to the target. So the same mode gives
different answers depending on which route a draw takes, on exactly the targets
that can tell them apart -- invisible in eight bits, which is why nothing had
caught it.

The clamp came out and went back in. Removing it cost 2.4 per cent on the Vulkan
distance-field row and 1.2 on the GLES full frame on a Pi 5, against a baseline the
commit before it reproduced to within three tenths of a per cent, and what it
bought was agreement on a floating-point target -- which nothing here presents. §13
of `docs/non-parity.md` has the numbers and the reasoning, and
`an_atlas_tint_in_plus_is_clipped_where_the_other_routes_are_not` pins the
deviation so that the next reader of `blend_tint` finds the cost recorded rather
than rediscovering it.

The scene that widening caught is mirrored now too, as a test rather than a
plate and for the reason it was caught: its feature leaves the picture unchanged,
so the catalog rejects it and a test is where the claim belongs.
`an_advanced_color_filter_against_a_transparent_color_is_the_identity` builds the
group twice, once with the filter and once without, and requires them identical
-- and separately requires the picture to be a picture, since two blank frames
would satisfy the first line and say nothing.

What it guards is the weighting rather than the arithmetic of the mode. A shader
that applied the blended color without weighting it by the source alpha would
flood the group with the saturation result, which is what upstream's "solid red"
would look like; dropping `sa` from that term fails the test. It does not guard
the unpremultiply, and that was checked rather than assumed -- opening the divide
to a source alpha of zero leaves it passing, because the clamp around it takes
the NaN to zero and the term it feeds is multiplied by that same zero.

Widening the rule also retired an exemption. Two atlas plates carry an identity
matrix image filter whose whole claim is that it changes nothing, and they had
to be named and checked backwards. They carry a color filter as well, which the
rule now sees, so they pass it like anything else -- and the claim about the
identity filter is asserted where it belongs, by comparing the two panels within
one frame rather than the frame against itself.

The runtime-effect row is wrong, and wrong in the way that matters most: it said
the chapter was "bounded by having two fixture programs rather than by the
renderer", and it is bounded by the renderer. Upstream's `DlImageFilter` offered
seven kinds where `ImageFilter` offered five. Both missing kinds have since been
built, so the row's obstacle is gone. Three of the twelve are mirrored now: a
program used as an image filter, and the pair that composes one with a blur
both ways round. The pair is the point rather than either half -- composing is
the one thing a program used as a filter can do that a program used as a paint
cannot, and the two orders come out thirty-six per cent apart, which is what
says the order is carried rather than collapsed.

Two more since, and the file's remainder is named. A program filling a rectangle
cut to a rounded one: a program replaces this renderer's fragment shader
outright, so the clip cannot be arithmetic inside it and has to come from the
stencil -- which makes the plate say that a program's draw is clipped like any
other rather than being a special case that escaped the machinery around it. And
a program used as an image filter on content that is turned, where the filter
acts on what the draw produced and what the draw produced is already in the
frame's space, so the tint does not turn with the shape.

`DrawPaintTransformsBounds` was the third of these and is here now. It fills
with a program through `drawPaint`, and what stopped it was the scene format
rather than the renderer: `Canvas::draw_paint` takes a paint like every other
call and a paint carries a shader, but a scene said `drawPaint` as a node
carrying a color. It carries a fill now, which is what the call takes, and the
two derivations that read a node's textures were told about it -- a flood fill
had been filed under "solid color; samples nothing", which stopped being true
in the same commit.

The plate cannot show the transform and does not claim to. The fixture program
splits on the clip-space coordinate, so nothing in the picture leans; what it
shows is coverage under a rotation, which is where getting the bounds wrong
would leave the frame's corners empty. That a paint fills the clip carried back
through the transform is asserted where it can be, in
`drawing_the_paint_fills_the_clip_rather_than_the_target` -- under a scale that *shrinks*,
which is the direction that catches it.

Two of upstream's are left, and both were read again once the third had turned
out to be buildable. Neither is unbuilt. Both are tests of machinery this
renderer does not have, which is a different sentence and the one this column
should have been carrying.

`RuntimeEffectWithInvalidSamplerDoesNotCrash` binds a gradient where a sampler
is expected, and asserts the engine survives it. Upstream can be handed that
because a Dart caller hands it `List<ImageShader>` and the list is checked at
run time. Here a program names its images as slots into a table the executor
uploads -- `images: Vec<u32>` -- so there is no bad binding to survive: the
type does not admit one. Mirroring it would mean weakening the interface in
order to test the weakness.

`RuntimeEffectVectorArray` declares `uniform vec4 iValues[1]` and reads its four
components in four bands. What that exercises upstream is uniform *reflection*:
its compiler walks a program's declarations and works out where each one sits.
This renderer has none to get wrong. A program declares the same block the
renderer packs -- that is the whole interface, and `docs/architecture.md` says
why -- and that block is already made of arrays of `vec4`. Every runtime-effect
plate here reads `paint.stops[1]` on every fragment, so an array-typed uniform
in a caller's program is not a case to be built; it is the only case there is.

The plates fill with a gradient where a flat color would have been easier, and
the reason is a mutation that passed. Where a texture binding names nothing the
backend binds a placeholder, so a program handed no input still samples
something -- and against a flat fill, tinting the placeholder is the same
picture as tinting the layer. A gradient is not reproducible that way.

Four more followed once a backdrop could be filtered by something other than a
blur, which is the other half of what those scenes need: a group whose backdrop
is a program, composed with a blur, bounded and unbounded. They are drawn at one
sample, for the reason the blur chapter's backdrop plates are -- a backdrop cuts
the pass to read what it was writing, and a multisampled pass cannot be resumed.

Their ground is a gradient with hard-edged bars over it, and the bars were added
after measuring. On a smooth ramp a blur does so little that the two sigmas came
out four levels apart and the inner half of the composition was barely tested;
with edges to work on they are a hundred and thirty-three apart over
ninety-seven per cent of the frame.

Writing them found a bug that a catalog cannot show. The fixture programs were
registered by a derivation that walked a scene for anything naming one, and a
group whose *backdrop* is a program has nothing inside it that does -- so they
went unregistered, and the plates drew correctly anyway because an earlier scene
had registered them and registration is idempotent. Alone they failed every
time. The derivation had been wrong four times by then and is deleted rather
than corrected: registering stores a program's SPIR-V against an index and
pipelines are built lazily from it, so the gate was buying three clones of a
byte vector per context at the price of having to be right. There is a test now
that renders one scene per context for each way a scene can depend on something
the executor arranges.

Seven of the twelve rested on the runtime effect
-- every `ComposePaintRuntime` and `ComposeBackdropRuntime` variant,
`CanRenderRuntimeEffectFilter`, `RuntimeEffectImageFilterRotated` and
`ClippedBackdropFilterWithShader`. A fragment program is a paint here, so it can
fill a shape and cannot filter what a layer already drew. That is now
`docs/non-parity.md` §7, and the `imageFilter` row of `docs/parity.md` names
both absences rather than only listing what it has.

The miscellany row said "mostly internal optimizations; the picture cases are
here now", and the second half was the false one. Thirty-eight tests, two of
which are unit tests on a texture -- `EXPECT_EQ(texture, nullptr)` and
`EXPECT_FALSE(texture->NeedsMipmapGeneration())` -- so thirty-six pictures
against twelve here. Five of the twenty-four missing were named for subpass
collapse and are here now, for the reason at the foot of this section; the rest
are pictures, and the largest family among them is
eight translucent save layers, each with a different filter on it.

The points family went in since, three of it: upstream's set of seven drawn
twice, once with a round cap and once with a square one -- which is the whole of
what a cap means to a point, there being no direction for one to extend along --
and its pair of scale extremes.

Two matrix backdrops followed, once a matrix could be one. The second of them,
`MatrixBackdropFilterWithTranslation`, is what a clip on a group is for: the
group is unbounded, so without the clip the halved copy would cover the frame
and with it the copy is a panel in the corner. A scene had no way to say that
until it did -- an item's clip narrows one draw, and a clip is state on the
canvas -- and saying it found a defect, recorded below.

Two scenes here are deliberately left unwritten because of what they look like
rather than what they need, and both are backdrop filters covered by an opaque
draw. `CollapsedDrawPaintInSubpassBackdropFilter` was written and then removed:
a paint fills the frame, the group filters all of it, and a second opaque paint
covers the result, so the plate is one flat color and is the same flat color
with the filter taken away. It passed here for a while because this machine's
GPU and its software rasterizer disagreed by a pixel at the frame's edge, which
is not evidence of anything; CI, which has only the one device, said so.

`BackdropRestoreUsesCorrectCoverageForFirstRestoredClip` is the other. It draws a red rectangle, filters a backdrop under a clip, and then
covers the whole thing with green -- so the picture is the green and the notch,
and it is the same picture with the backdrop filter taken away.
`a_feature_a_scene_asks_for_has_to_change_the_picture` says a plate like that is
worse than no plate, and it is right: what the scene is for cannot be seen in
it.

The cost lane took it instead, which is the half a picture could not carry.
`layer-backdrop-under-a-difference-clip` is in the corpus: the same arrangement,
with nothing drawn over the evidence, and what it is there for is the number
rather than the look. Rebuilding a clip costs one draw per clip in force per
cut, and that scene is the only one in either collection that can say so --
`cost.rs` counts draws on every commit, and removing the rebuild takes its row
from eleven draws to ten.

It was still worth writing once. Doing so found that a stencil clip did not
survive the pass a backdrop filter cuts -- every draw after the filter tested
for a depth no pixel in the new pass held, and landed nowhere -- and that on
GLES the new pass's stencil was not cleared at all, so the same frame drew
differently between runs. Both are fixed, and what checks them is
`a_stencil_clip_survives_the_pass_a_backdrop_filter_cuts`, which is a test
rather than a plate for exactly the reason the scene is not one.

That pair is the one worth the words, and it is a test about where a
multiplication happens rather than about points. One plate is a point a
ten-thousandth of a unit wide under a millionfold scale; the other is a hundred
million units wide under a millionth of one. The product is the same hundred
device pixels both times, so the two have to draw the same disc, and the two
ways of getting it wrong are opposite -- sizing the point in device pixels from
the width alone draws nothing in the first and fills the frame in the second.
Either plate alone would be passed by a renderer that had the arithmetic
backwards. They measure eight thousand and thirteen pixels against eight
thousand, on a disc of about seven thousand nine hundred.

Three more went in with the points, and one of them found a defect that had
been sitting under the other two. Upstream keeps a pair on unbounded contents --
a group whose contents are a paint, which covers the clip rather than any shape,
once with bounds stated and once without -- and it draws registration marks
outside the bounds so a reader can see where the edge should be.

The bounded one came out with a hard edge exactly at the bound where the blur
should have carried past it. `Layer::with_blur` and an `ImageFilter::Blur`
handed to `save_layer_filtered` are two spellings of one thing -- `open_layer`
turns the first into the second so there is one path below it -- and they were
not one picture. A layer's own sigma is carried into the target's size; a
filter's spread was not, so with bounds stated the filter's output stopped dead
at them. Measured: the same blur reached ten pixels past the bound on the layer
and none as a filter.

Only where bounds were stated, which is why nothing had noticed. An unbounded
layer is sized by a narrowing that already asks the filter how far it reaches,
so it was right all along. The two are now asserted against each other, and the
assertion also checks that both actually spread -- otherwise it is satisfied by
two pictures each cut off at the bound.

The line-mode depth scene went in with them, and needed the one thing a points
run could not say: a clip. `Lines` and `Polygon` make several draws out of a
single call, and upstream keeps the scene because they all have to carry the
same depth -- a clip is tested against depth, so draws given different ones
would be cut differently, some segments surviving a circle and some not, from
one call. `PointsSpec` carries a `clip_shape` now, on the run rather than on
each point, which is the distinction the scene is about.

This one found nothing, which is worth saying: 2082 pixels of the run inside the
circle and none outside it. Removing the clip puts 1219 outside, so the
assertion is measuring what it claims to.

`CanDrawPointsWithTextureMap` is the neighbor it cannot join. A points run here
takes a color and the field that draws it is a solid-color material, so a run
filled by an image is not expressible -- the scene format and the material would
both have to grow, and it is the material that decides it.

Five of the eight translucent save layers are here now, and the family divides
in two once a group can be filtered as a whole. Two recolor the group on its way out, two run a
filter over the finished group, and one does both -- and the two halves are not
two spellings of one thing: the same blend against a constant, put one way and
then the other, differs across the whole frame.

The family is the point rather than any one of
them: a translucent group has two things that have to happen in the right
order, the alpha it composites with and whatever recolors it on the way out, and
only a filter that is not a plain scale can tell the orders apart. So neither of
the two is one. Destination-over against a constant reads the group's own alpha,
so what the filter contributes is strongest where the group is thinnest; and
upstream's alpha-doubling matrix is chosen to fight the layer's alpha rather
than to look like anything.

Checked by rendering all three of the family that now exist and comparing them,
because a filter that was accepted and dropped would leave the plates identical
and passing. They differ across the whole frame and across a third of it
respectively, which is the shape each filter should have.

A count that was nearly wrong the other way, worth recording beside the two
that were: five of that file's tests read like resource tests from their names
-- mipmap generation, releasing a texture on teardown, setting contents with a
region, two about depth values -- and all five open a playground and are
scenes. Only the two named above do not.

The thirteen of that file still unmirrored, read one body at a time rather than
sorted by their names, because the names mislead in both directions and so does
grepping for `OpenPlaygroundHere` -- `ToImageFromImage` opens one and is a test
of a round trip through a host buffer, not of a picture.

Eight are about a recording rendered into a texture the caller then owns, and
what becomes of that texture: an empty picture and a full one converted to an
image, the round trip out to a host buffer and back, the null a zero-sized
target returns, mip generation on that path and on a texture built by hand,
releasing on teardown, and replacing a region of one's contents. What they draw
is incidental to the machinery under test. One more is the dispatcher's own
culling, reached through a render-target cache.

Two the scene model cannot say. One is a run of points painted from a texture,
where a run here carries a single color. The other is a stroked path whose
`MoveTo` is followed by a `Close`: a `Shape` states its contours all-open or
all-closed -- `Contours` opens every one and `Polygons` closes every one -- and
upstream's path is one of each. The invariant behind it did not need a picture
and is checked where it belongs, in
`a_close_after_a_lone_move_to_adds_no_contour`: the builder keeps the trailing
`MoveTo` and `Close`, and flattening walks one contour rather than two. That is
what upstream's comment asks for, reached by dropping the empty contour instead
of declining to add one.

One is `CollapsedDrawPaintInSubpassBackdropFilter`, which was built and then
removed -- it drew a uniform picture with its features stripped as with them, so
nothing could tell whether the optimization it names had happened.

That leaves one picture nobody has written rather than anything blocked: the
coverage the first restored clip gives a backdrop filter.

The clip row said "nothing; this file is covered" and the file was not covered.
Six scenes against five tests reads as a surplus, and two of the six are this
chapter's own, so four of upstream's five were mirrored and the fifth was not.
`FramebufferBlendsRespectClips` is now here and the row's claim is now true.

It is worth a plate rather than being folded into the blend chapter, because
what it is about is the clip and not the mode. A separable blend is applied by
the hardware as it writes, so the clip has already decided which pixels are
written and there is nothing further to respect. An advanced mode reads its
destination -- through a framebuffer fetch, or through a copy of the target --
and a reader that ignores the clip blends happily into pixels no draw should
have touched. The plate multiplies a red square across a region much larger
than the circle it is clipped to, and its corners are the answer: pure white on
a software device, which is the ground exactly as it was.

That plate is also the first thing to be drawn by the device fix two commits
ago rather than in spite of it. `Multiply` is an advanced mode and the
preferred device here has no extension for it, so before that change this
scene would have been skipped by the only test that draws every plate, and
skipped silently.

The path row is the second whose count was the thing that was wrong, and by one
rather than by four. `ArcWithZeroSweepAndBlur` builds a display list and stops,
with a comment saying that an empty picture has to be creatable without
crashing; it opens no playground and is not a scene. Thirty pictures, twenty-one
here, nine short -- and nothing blocking any of the nine, which are strokes,
lines drawn four different ways, multi-contour paths and a fat stroked arc.

That crash test is worth having and is not worth a plate, so it is a test. An
arc of zero sweep is a single point, a sweep gradient divides by an angle, and a
mask blur opens layers around whatever coverage the point produced -- three
things with degenerate cases meeting on one draw. It records here without
complaint and reaches a device, and the frame comes back untouched, which is
what says the point produced no coverage rather than coverage nobody looked at.

The atlas row said "nothing named; see below" and there was nothing below --
the only row whose pointer went nowhere, which is its own kind of stale.
Checking it moved the other column instead.

That file holds fifteen tests and eleven pictures. Four of them --
`DlAtlasGeometryNoBlendRenamed`, `DlAtlasGeometryBlend`,
`DlAtlasGeometryColorButNoBlend` and `DlAtlasGeometrySkip` -- never open a
playground at all: they build an atlas geometry and assert on its flags and its
vertex buffer, `EXPECT_TRUE(geom.ShouldSkip())` and the like. They are unit
tests that happen to live in the playground file, and counting them as scenes
made the chapter look a third emptier than it is. So the count beside it is
eleven.

Of the rest: one is `Plus` into a wide-gamut target, which is a test now --
`an_atlas_tint_in_plus_is_clipped_where_the_other_routes_are_not`, and what it
found is the tint path clamping a mode the hardware path does not, kept for the
reason §13 of `docs/non-parity.md` gives -- and one is a four-color modulate that is the plate
already here under `draw-atlas-with-color-simple` -- the same four sprites, the
same mode, a different name upstream. The other three went in.

Two of those three are the pair that compares upstream's conversion of
`drawImageRect` into a `drawAtlas` against the unconverted path, and the
reasoning that had them declined was wrong in an instructive way. It said the
picture the pair would contribute is already elsewhere -- an image under a color
filter -- and that is true and beside the point. The pair is not there for the
picture. It is there because the two draws must agree, and upstream takes the
left one off its fast path by putting an identity matrix image filter on it.

There is no such fast path here, so what survives the translation is the other
half of the same claim: an identity matrix filter routes a draw through an
offscreen and resamples it on the way back, and has to come out the same
anyway. That is not free of ways to fail -- a half-texel offset in the resample,
a target sized to the wrong bounds, a color filter applied on the way in rather
than on the way out -- and none of them are checked anywhere else. Asserted
rather than looked at, and checked by moving the filter two pixels, which puts
seventy-one per cent of the panel out.

The pair collided with a rule this suite already had, which is the interesting
part. Every scene carrying a feature is rendered again with the feature stripped
and has to come out different, on the reasoning that a feature which changes
nothing is a feature that is not reaching the picture. An identity matrix filter
is the one feature for which that is exactly backwards. So the two are exempt by
name -- and the exemption asserts the opposite rather than skipping them, since
an exemption that asserts nothing is a hole with a comment on it.

The third is `ColorBurn` over four grays running black to white. The plate
beside it uses `Difference` with one color and says the tint setting is read at
all; this says the arithmetic is right, because a burn done as a multiply would
still darken and would darken wrong.

This is the one place the counting caveat above cuts the other way. The "Scenes
there" column is in scenes and not tests, which is why the blend row's eighty
is right against twenty-one tests; here the same distinction makes fifteen
wrong against eleven pictures. Both directions have now been checked once each.

The basic row named superellipses and subpass optimizations, and understated
what it was covering. Superellipses account for seven of that file's
eighty-five, and the file is short by forty-one -- so the two names between
them explain a fraction of the gap, and the rest of the missing scenes are
`CanDrawPaint`, `CanPerformSkew`, `CanRenderSimpleClips`, a dozen `StrokedArcs`
variants and other ordinary pictures that are unwritten rather than blocked.

Checking it did find one obstacle nobody had written down, and it is a real
one. A rounded rectangle here has a single circular radius; `dart:ui`'s `RRect`
and upstream's `RoundRect` both carry four corners with independent x and y
radii, eight numbers against one. Five of the basic chapter's scenes build a
rounded rectangle this cannot describe. That was not in this column and, worse,
was not in `docs/parity.md` either, whose `drawRRect` row said "yes" with
nothing beside it.

Four of those forty went in as the stroked-arc family, which is the one cluster
among them that is a mechanism rather than a picture: an arc has ends, where a
cap shows, and through its center a vertex, where a join does. Butt and round
ends were already here; square ends, the two joins and a full turn were not.

The joins needed their sweep narrowed before the pair meant anything. At a
hundred and thirty-seven degrees a miter and a round join agree to within
twenty-five pixels, which is two plates that look identical to anyone comparing
them; at forty the miter runs out to a point and they are three times as far
apart. Small either way, and now small about something.

It was recorded as a non-parity entry and then, a commit later, built instead --
which is what the entry had said should happen to it, having named the limit as
a generalization nobody had written rather than a decision anybody took. So the
five scenes are no longer blocked. The superellipses have since been built
too, which is the second entry in this column to have gone that way. The subpass
optimizations went the same way as a group, for the reason at the foot of this
section, and the row is now level with the file.

Five more of those are written now, and they were chosen for covering something
no other plate does rather than for being next in the file. Two draw a paint
with no shape at all, once and then twice with the second translucent -- what a
paint covers is the clip, so a renderer that took its extent from a shape's
bounds would be right everywhere else and wrong here. One shears, which nothing
else in the catalog does. One puts five clip shapes against three fills, a
color, a mirror-tiled radial and a repeated image. And one steps three squares
down the diagonal with the middle one inside a layer, which is the only plate
that says when a layer composites: it goes behind the square drawn after it and
in front of the one drawn before, and a renderer that composited layers last
would put it in front of both.

Three more went in after that, and they are the ones this column's own history
points at. The eight-radius rounded rectangle was found as an obstacle here,
recorded, and built -- and then the scenes it unblocked stayed unwritten, which
is the ordinary way a built capability goes unexercised: nothing fails when a
picture nobody drew is missing. So there is a rectangle whose four corners are
four different ellipses, one whose radii overrun the side they share by three
times and are scaled by `dart:ui`'s rule, and one that draws a ring twice --
once as a difference of two rounded rectangles and once as the outer one with
the inner cleared out of it. That last pair agrees to the byte in color and
differs only in alpha, which is `Clear` doing its job; the agreement is asserted
rather than left to a reader, and it is sensitive to a two-pixel change in one
corner's radius.

Four after those, and they are one question asked four ways: where does a
layer's target sit and what does it cover. Three siblings each bounded to a
small square and each filling the whole frame inside it, so nothing but the
bounds decides what shows. One layer bounded to a quarter of what is drawn into
it, where the bounds cut three overlapping squares and the order among them
survives being cut. One layer with nothing in it at all, composited with a mode
that discards its destination -- empty is not absent, so it cuts a hole the size
of its bounds in the image behind it. And one that is about the origin rather
than the extent: a bounded layer's target starts where the bounds start, so its
contents are placed against that origin, and a renderer that forgot would slide
them by the offset and still draw three squares. That last one has the bounds
drawn as a yellow outline so the two can be read against each other.

The arc family went next, and writing it found that two of its existing plates
were the wrong picture. Upstream splits every arc farm in two -- one closed by
the chord between the ends, one closed through the center -- and both of the
plates here named for the *open* half were drawing the closed one, with the
closed half not present at all. So the pair upstream draws was one plate showing
the wrong side of it. The two are corrected and their siblings added, along with
the third join a bevel makes, the two cap plates that read a square end against
a butt and then against a round one, and the pair upstream draws translucently.

That last pair is the one worth the words. A stroke is a run of overlapping
quads with a cap on each end, so an outline covering a pixel twice is invisible
at full opacity and darker at half. The plate closes its arc to within twenty
degrees at a stroke wider than the diameter, so the two caps land on top of each
other and that patch is darker -- correctly, being two shapes over one pixel.
What must not darken is the run between them, and that is asserted rather than
looked at: three windows away from the ends carry a single cover and no pixel
darker, and a fourth window over the caps has to find the doubling, so the first
three cannot pass by the arc having missed them.

Two stroke plates went in after those, and they are the two that put a
circle's outline somewhere extreme: a stroke half a device pixel wide under a
twentyfold zoom, and one five times wider than the shape it outlines. Each is
upstream's four quadrants -- filled, stroked, both, and a filled circle of the
outer radius beside them -- and that fourth quadrant is the assertion, made by
eye: a stroke's outer edge is a circle of exactly `radius + width / 2`.
Measured while writing them, the zoomed pair agree exactly at twenty pixels of
reach and the wide pair to within one, which is an antialiased edge falling
either side of a threshold rather than a difference in the geometry.

Upstream builds its circle as a path of four cubics so the general stroker sees
it; here the same shape takes the fragment-evaluated route. That is the reason
to have them rather than an obstacle to it -- the tessellated stroker is
already covered by the corpus, and what these say is that the *field* holds up
where the numbers are hard.

The wide stroked rectangle went in next, as the pair upstream keeps, and it
is the first plate in this chapter to have been added because the one already
here could not fail. Upstream draws six outlines in translucent blue -- three
joins where the stroke leaves a gap down the middle and three where it is
wider than the shape it outlines -- and it draws them twice, once saying the
rectangle directly and once handing over its path, so that neither route may
cover a pixel twice. What was here was one opaque rectangle with a gap in it,
under upstream's name. Opaque is the part that matters: an outline covering a
pixel twice is invisible at full opacity, so a plate named for not overlapping
was drawn in the one color that could not show an overlap.

Saying the path form at all needed something the catalog did not have. Five
shapes have their own entry point here and a scene naming one gets that call,
which is deliberate -- it keeps the choice each makes between an analytic field
and a tessellation under test -- but it left no way to ask for the other route,
so every rectangle in the catalog was an analytic one. An item can now say
`as_path`, and the two plates differ in that field and nothing else.

They do not agree, which is the point of having both. Through `draw_rect` with
a round join, answered analytically, the middle of a rectangle whose stroke is
twice its width carries exactly one cover. Through the tessellator -- what the
square-cornered joins get, since a stroked rectangle with a miter has no
analytic form here, and what all three get from the path form -- the same
pixels carry three covers and six. The analytic half is asserted; the other is
`docs/non-parity.md` section 13, with the numbers.

Then upstream's two mask-blur grids, which are the same question asked at two
resolutions: whether a blurred shape is still that shape. One runs five kinds
of shape down its rows -- rectangle, circle, oval, a rounded rectangle with
circular corners and one whose corners are ellipses -- and sweeps each row from
a sliver one way to a sliver the other. The other holds the shape still and
sweeps the corner instead, five radii each way, so its grid runs from a plain
rectangle in one corner to a stadium in the opposite one and the two edges are
the cases a rounded rectangle usually never meets: one radius zero and the other
not.

Both draw on white rather than on this catalog's ground, which upstream also
does and which is not decoration. A blur spreads coverage outward; against a
dark ground the spread edge fades toward what it is already nearest, and against
white it fades the other way, where it can be read.

The sigma is the one number not taken from upstream. These plates are a fifth
of upstream's size, so a faithful sigma would be a fifth of a pixel -- a blur no
comparison could fail on. One pixel against a twelve-pixel shape is upstream's
picture at this scale rather than upstream's number, and the picture is what the
plate is for.

Writing them found that nothing in this suite checked that a mask blur reaches
the frame at all, which is the same trap the backdrop-blur plates fell into
once: a plate that asks for a filter and renders identically without it counts
as covered while showing nothing. Every plate holding a mask blur -- twenty-five
of them -- is now rendered a second time with its sigma set to zero and has to
come out different. All twenty-five do.

Four more after those, and between them they take the chapter to eighty-two of
its eighty-five. Upstream's rounded rectangles drawn as paths, which the
`as_path` field above had just made sayable and which pair with the analytic
trio already here. A rectangle in one color, which is upstream's whole scene and
is what every other plate in the chapter rests on. A layer with nothing in it,
opened over a red frame and closed at once, which asks whether an empty layer's
target was cleared before it was composited -- an uncleared one would show as a
rectangle of whatever the allocation held, exactly where the bounds are, and the
frame is asserted to be one color end to end.

And the same layer twice at two scales, each blurring what it captured, which
is the one plate in this chapter that checks a decision rather than a picture.
A filter on a save layer is stated in the space of the caller rather than in
device pixels, so the copy drawn at three times the scale blurs three times as
wide on screen. A renderer holding the sigma in device pixels would draw both
panels with the same soft edge and differ only in size, which is not a thing
anyone comparing them by eye would reliably catch -- so the edge is measured.
It falls from covered to ground over six pixels in one panel and nineteen in
the other, and setting the second layer's sigma to a third of the first's puts
that ratio at exactly one, which is what the assertion is written against.

Four scenes were filed under the wrong chapter, which is worth recording
because of how it was found rather than because of the four. Writing the
save-layer plates, three of them turned out to already exist under `dl/` --
mirrors of basic-chapter tests sitting in the miscellany chapter, so the
inventory counted them against the wrong file and a name-by-name diff of the
basic chapter reported them missing. One went the other way: a matrix-filter
plate under `basic/` mirrors a test in the miscellany file.

They cannot be caught by a test. The topic a scene is filed under has to match
the file its name comes from, and this repository keeps no copy of those files
-- so the check is a comparison against sources fetched at tip of tree, run by
hand, and the four are what it found. Where the two mirrors of one test
differed, the closer one was kept: upstream's sibling-bounds test is three
layers each filling the frame and the incumbent had two at six-tenths alpha,
its standalone layer is half-transparent where the incumbent was opaque, and
its bounded layer holds three overlapping squares where the incumbent held the
chapter's generic pair.

The blend row named two obstacles and one of them was misread, for three of
that file's twenty-one tests. Two are named for framebuffer fetch and one is
the subpass collapse optimization itself, which is out of scope for the reason
below. Of the two, only `ColorFilterAdvancedBlendNoFbFetch` is absent, and the reason
this column gave for it was wrong in a way worth correcting: it said the test
needs capability injection, which reads as a mechanism someone could build. It is
a Metal-only test that installs a mock capabilities object to force
`SupportsFramebufferFetch()` false, and what it then exercises is upstream's
*other* advanced-blend implementation -- the one that does not read the
destination through a fetch.

There is no such implementation here to select. This renderer does advanced
blending with the hardware extension or refuses the batch:
`Capabilities::check_blend_modes` returns an error rather than substituting, the
word `framebuffer_fetch` appears nowhere in the tree, and `solid.wgsl` binds one
texture and one uniform -- so nothing in the fragment stage can read the target
at all, and `blend_tint`'s advanced arm blends against a tint carried in the
paint rather than against what is already there. Forcing a capability false would
produce that refusal and not a different picture, which is also why the scene
could not satisfy the rule that a scene carrying a feature has to render
differently without it.

So it belongs with the subpass collapse rather than with the unwritten: out of
scope because it tests a second implementation this renderer deliberately does
not have. Building capability injection would be worth doing for other reasons
and would not bring this scene any nearer. `FramebufferAdvancedBlendCoverage`
is named for upstream's implementation and not for anything it requires -- it
draws an image with `kMultiply` under a scale, and asserts the scale reached the
image. That is a picture this renderer can draw, and it is here now. Twelve of the rest were unwritten rather than
blocked, and three of those are here now -- `drawPaint` twice under a
non-separable mode, an advanced blend clipped so its destination runs out, and
an empty group whose color filter floods what its bounds admit. That last is
the interesting one: nothing is drawn inside the group, so it is transparent,
and a filter that ignores its input turns transparent into opaque red. It comes
out solid red on both devices here, which is what upstream's comment says it
should be.

Checking that row turned up something larger than the row. Two of the three new
scenes need advanced blending, which the preferred device on the machine this
was written on does not have -- and `every_catalog_scene_draws_something` asked
only the preferred device and skipped what it could not render, without saying
so. Nineteen plates were never drawn at all, and the test reported a pass. The
software rasterizer beside it has the extension and renders every one of them.

So that test now asks every Vulkan device on the machine, in the order the
conformance suite already asks them, and says how many of the catalog it drew.
It draws all of them, and the number is left out here on purpose: it is the 
size of the catalog, which is stated once above under a test that checks it. The cross-backend comparison beside
it was already honest -- it reports its nineteen gaps by name -- and those stay
gaps, because the GLES driver here lacks the extension too, so there is no pair
to compare them on. Reported, not hidden, which was the whole difference.

The shadow row was the first of these to survive being checked, and then did
not survive the second look. Twenty-six of that file's thirty scenes are named
`DrawShadowCanOptimize` or `DrawShadowDoesNotOptimize` something, and what they
are for is deciding which paths upstream's convex-shadow optimization applies
to. That optimization is not here. The row's argument followed: the scenes would
still *draw*, since an optimization that changed the picture would be a bug, so
mirroring them would produce pictures that check nothing.

The premise is right and the conclusion does not follow, because it looks at one
scene at a time. Upstream writes them in pairs -- the same caster wound each way
-- and the pair checks something the single scene cannot: that a shadow does not
depend on which way its caster is wound. That property has to hold here too, it
holds by a different mechanism than upstream's, and nothing was checking it.

So the polygonal casters are here in both windings, and
`a_shadow_does_not_care_which_way_its_caster_is_wound` reads them as pairs and
requires the two pictures to be identical -- not within a budget, since two
windings of one polygon are one region and anything that reads the direction
differs by whole triangles rather than by an edge sample. The three casters
upstream declines to optimize are here too, for the other side of the same
question.

Twelve of upstream's thirty are left out, and they divide by what stops
them. Eight want a closed contour of several curve segments -- a circle and an
oval built from conics tweaked off the exact weight, a round rectangle with four
different corner radii built the same way, and quadratic, conic and cubic
casters -- where `Shape::Conic` and `Shape::Cubic` are each one open segment and
`Contours` opens every contour it is given. Three want a *winding* the model
cannot state: the counter-clockwise circle, oval and uniform round rectangle,
where the clockwise ones here are `Shape::Circle`, `Shape::Oval` and
`Shape::RoundedRect` and a primitive has no direction to reverse. The last mixes
stray `MoveTo`s with a closed contour, which is the same open-or-closed
limitation the file beside it ran into.

Worth stating what that costs, because the obvious repair is a general curve
path and it would buy very little. Upstream's tweak is there to defeat *its*
shape recognition, so that the general convex path is exercised rather than an
oval fast path. This renderer has no such recognition for a shadow: a circle, a
sixty-four-sided polygon approximating one, and a rounded rectangle all record
as four passes and four draws, differing only in the vertex count of the shape
itself. So every one of the twelve would take the same route as the octagon and
the triangle that are here, with its curves flattened first -- which the path
chapter already covers from several directions. The variant is worth adding when
something else wants it, and not for these. The mesh
comparison upstream makes stays out as well, and it is the only part of that
file that was ever about the optimization rather than about the shadow.

A third was on that list and is not now. It holds two closed triangles, which
`Shape::Contours` cannot say -- its contours are open, deliberately, and the
note beside that variant said the closed case should be written when a use for
it appeared rather than in advance. This was the use, and `Shape::Polygons` is
the shape it asked for: several closed contours in one path, which is what a
fill rule acts on and what two separate draws are not.

One of the four that are not about the optimization was missing and is now
here. `CanDrawPerspectiveConvexShadow` is filed with the others and is not one
of them: it draws a mask blur under a three-dimensional rotation and a
perspective matrix, where its two siblings rotate and scale. Its siblings were
already translated as a shadow under a transform, and perspective is a field of
that transform, so the third followed once perspective existed. It did not
exist when the row was written, which is the other way one of these goes stale.

The opacity row said "subpass collapse", and its one missing scene needed
nothing of the sort. Subpass collapse is upstream's optimization for a save
layer whose contents let it be folded into its parent, which is what that
scene's nested layers are shaped like -- so the name was read off the scene
rather than off any obstacle. It decides how many passes a picture costs, not
what the picture is. The scene draws, the file is covered, and what the scene
is actually about turned out to be unchecked anywhere: that a group's alpha
applies to the finished group rather than to each draw in it, and that nesting
two of them multiplies. There is a test for that now.

Its first draft could not have failed. It compared a nested pair against a
single layer worth their product, and every such comparison survives
compositing each layer at the square root of its alpha -- the root of
forty-nine hundredths is seven tenths, and seven tenths squared is back where
it started. So it now also asserts what the alpha is worth against a value
derived rather than measured, and both mutations fail it.

The vertices row said "mask filters on a mesh", and that was true: a mask blur
over one was refused outright. The reason given was that a mesh carries a color
per vertex and so varies by construction, and it does not have to -- built from
positions alone it is filled by the paint, exactly as a path covering the same
area is. Upstream's own scene for this passes no vertex colors at all.

What the refusal was protecting is real and is narrower than it was written.
A mask blur fills through blurred coverage, so the fill must have a value
everywhere that coverage reaches, halo included. A paint has one, being a
function of position. Per-vertex colors are defined on the triangles and nowhere
else, so the halo has nothing to take its color from, and that case is still
refused rather than extrapolated -- the message now says so instead of saying
meshes. A mesh the paint fills now blurs, and is required to come out identical
to the same square drawn as a path.

Fifteen of the gradient file's forty are still not mirrored, and none is
blocked either.
Four are upstream's fast-gradient scenes, which exist to check an optimization
that decides whether a two-stop gradient can be drawn as a full-screen quad --
an internal choice with no `dart:ui` surface, and one this renderer does not
make. The rest are tile modes crossed with many-colors, which is the one shape
of gap where writing the next scene is arithmetic rather than coverage: the
tile modes are each mirrored once already, and the many-colors ramp is mirrored
once already, and crossing them tests the crossing and nothing else.

The pipeline carries a wide gamut now. Colors state which primaries they are
against, a color outside the sRGB primaries' triangle keeps the components
below zero that say so, and it reaches a floating-point target through layers,
gradients and filters intact. **What is not built is presenting one.** The
swapchain still negotiates `SRGB_NONLINEAR` -- which is the presentation engine
being told the image holds encoded values, and it does -- and neither it nor the
scanout path asks for a wide-gamut color space, because the devices available
here are a software rasterizer and a virtual display controller: a Display P3
surface cannot be exercised, and this project does not ship what it cannot
check. So the two cells above no longer name wide
gamut, and nobody has counted against the source how many of those scenes
needed it rather than one of the other things listed.

Perspective transforms were the other, and were described here as a design
limit -- the transform type being affine and two-dimensional -- with the note
that the scenes needing it would arrive when the parity row changed. The row
changed, and three of them arrived: a curve under perspective, a receding
plane, and a rectangular clip under one. What that promise did not say, and
what is worth recording in its place, is that the count above moved by three
and not by the nine the path chapter is still short -- eleven when that was
written, and the number has been checked since rather than carried forward.
The blocked-on column
was per-file prose that no test reads, and it named one obstacle where there
may have been several; nobody has since counted, against the source, how many
of those scenes actually build a perspective matrix. So the path row now says
nothing is named rather than that nothing is left, which is the honest state of
it: the obstacle that was written down is gone, and what holds up the rest has
not been examined.

It has been examined since, against the file at tip of tree, and the chapter is
four longer for it. Upstream draws the same thin-line grid four times -- as a
line, as a stroked path, as a filled rectangle and as a filled rounded one --
and it is the one family in that file worth having whole, because what it is
for is the disagreement between the four rather than any one picture. Three
columns of stroke width against five rows of angle, four parallel lines to a
cell, each a quarter of a pixel further off the sample grid than the last.

Saying the first of the four needed a `Shape::Line`, which the catalog did not
have: `Canvas::draw_line` is a public entry point and nothing in either
collection reached it. Having it says something immediately. The line and the
stroked path come out identical to the byte, because `draw_line` builds a
two-point path and hands it to `draw_path` -- so upstream's pair, which exists
to catch a specialized line renderer disagreeing with the general one, is one
picture here. That is pinned rather than assumed, so a route of its own would
be visible the day it appears.

The filled forms are a different picture, and should be. A stroke narrower than
a device pixel is widened to one and dimmed to match, which is upstream's rule
copied constant for constant; a filled rectangle a third of a pixel tall gets a
third of a pixel of coverage and nothing more. The middle column is where the
two families part, by eleven per cent of the frame, and that difference is
asserted -- a renderer that had quietly dropped the thin-stroke rule would make
all four agree and would look, from every other test here, entirely well.

The first column asks for a width of zero. Upstream widens that to the thinnest
line a device can draw; this renderer refuses, which is the one part of the rule
not taken and is recorded as a row of `docs/parity.md`. So the column is empty
in all four plates, and they are the only place that decision can be seen rather
than read about.

Five more after those. Four are shapes that had no
plate and needed no new capability: a quadratic whose ends are the same point,
so the stroke goes out and comes straight back and should be a pill rather than
anything flat-ended; a cubic whose controls lie outside the band its ends
define, so the curve crosses its own chord twice; and a quadrilateral closed
into something no rectangle route could mistake for one. The fifth pair needed
a `Shape::Contours`, because a path may hold more than one contour and nothing
in either collection could say so. Both of upstream's scenes on that are cases
where the difference is easy to lose: two contours meeting at a point, where a
renderer that ran them together would draw a mitered corner instead of two round
caps, and a contour holding a single point, which has no direction and is drawn
only because a round cap has a shape without one.

That leaves the file's `ArcWithZeroSweepAndBlur`, which is not a catalog scene
and should not become one. Upstream builds it and stops, with the comment that
an empty picture has to be creatable without crashing, and it draws nothing --
so it fails this collection's own rule that every scene draws something, and it
would fail the mask-blur check above for the same reason. It is already covered,
by `an_arc_that_sweeps_nothing_survives_a_mask_blur_and_a_sweep_gradient`, which
asserts the whole frame is untouched.

Writing it up found the one thing in this round that looked like a defect and
was not. A degenerate shape given a mask blur appeared to draw a disc of
transparent pixels where it should have drawn nothing. It is the blend: an empty
layer still has bounds, and this collection's items replace by default, so `Src`
clears those bounds to transparent -- correct, and the same behavior a plate in
the basic chapter exists to show. Composited as upstream composites, the frame
comes back untouched.

Calling that the end of the chapter was wrong, and the way it was wrong is worth
the sentence. The count had reached the number in the table, and the number in
the table is a count of scenes filed under `path/` rather than of upstream tests
mirrored -- nine of the scenes there are this repository's own. So the two
numbers met while nine of upstream's were still missing. The diff that had said
otherwise was splitting runs of capitals, which made `UVPositionData` into
`u-v-position-data` and reported three scenes missing that were already there
under `uv`. Both halves of that mistake are the same mistake: a count is not a
name-by-name check, and a name-by-name check is only as good as the rule that
makes the names.

Seven more went in on the second pass. A plain thick segment; a rectangle
stroked through its path with a miter and then with a bevel, where the
difference is four small triangles and is exactly the size a join that fell back
to the default would hide; a stroked circle under a mask blur, which is the case
where a renderer has to decide whether it is drawing a field or a ring and the
answer is neither; and an arrow stroked, recolored by a filter that keeps only
the filter's color, and turned a quarter turn -- where the paint underneath is
black, so a filter dropped on a transformed draw loses the arrow into the
ground.

The last two are measured rather than looked at. Upstream's fat stroke draws an
arc at a width wider than the shape's radius, with a line at the frontier its
outer edge must reach and not pass, and leaves a reader to check. The stroke's
inner offset has crossed the center there, so the outline self-intersects, and
that is where a stroker either clamps and falls short or runs away. It reaches a
full cover one pixel inside the frontier, touches the pixel the frontier passes
through, and puts nothing beyond -- which took one correction to establish, since
the marker line was two pixels wide and covering the edge it marked.

And a clip whose corner is the circle's center, so one quarter survives: two
straight edges meeting where a curve used to be. Said as a shape rather than as
a rectangle, which is the stencil route and is what upstream's `ClipPath` means;
a scissor could express this one and would be exercising something else.

Dithering was not the small feature its one-word entry suggested, and the reason
is worth keeping even though the difficulty has since evaporated. Breaking up a
band means perturbing a color by a fraction of one quantization step before it
is quantized, and a step was not a fixed quantity while the pipeline carried
light: into a linear eight-bit surface it was a flat 1/255 of light, and into an
sRGB one the hardware encoded on write with a slope running from 12.92 at black
to roughly 0.44 at white -- so one offset was many steps in shadow and a
fraction of one in highlight. Measured rather than argued: dithering a dark
gradient into an sRGB target in *light* tracked the ideal about six times worse
than not dithering at all.

The pipeline carries encoded components now, and a target stores them without
transforming them, so a step is a flat 1/255 wherever it stands and the rate is
upstream's single `1.0 / 64.0`. The amplitude and the space it was applied in
are both gone from the paint block.

The lesson that outlives all of it is the one about the blocker. This was filed
as waiting on a decision about the color policy -- the conversion belonging to
the target format rather than to a shader -- and what was missed is that nothing
required the *shader* to be what knew the format. A backend fills two floats at
submission and the shader is handed numbers. The decision dissolved on being
looked at, which is the same lesson as the blocked column above.

Dithering the gradient's parameter rather than its color was the obvious way
around it and would not have worked: jittering where a band edge falls, rather
than what value it steps to, needs no knowledge of the target, but the jitter is
half a pixel wide while the band is widest exactly when the gradient changes
slowest. It is weakest where banding is worst.

The scope is upstream's, not a local judgment about where banding is worst.
Checked at tip of tree rather than from a checkout: `IPOrderedDither8x8` occurs
in six files there, its own definition plus the four SSBO gradient fills and the
two-stop fast path. The variants that sample a baked ramp do not call it. So a
gradient of four stops or fewer is dithered here and one tabulated into a ramp
is not, which costs the exact agreement between this renderer's two gradient
paths -- they now differ by what a dither reaches. That is a real price and
`docs/architecture.md` records it beside the claim it weakens.

One thing upstream does that is not followed: it compiles the dither out on
OpenGL ES, and the guard says why -- `mod` does not exist in GLES 2.0. This
backend's floor is GLES 3.0, so the limitation is not present, and copying the
workaround would make the two backends draw different pixels for no reason.

An effect's own textures used to head this list, on the grounds that a caller's
program got the material's uniform block and nothing else. That stopped being
true when the effect material grew texture slots, and the paragraph outlived the
limitation by some days -- which is the same failure the blocked column above is
warned about, in the prose that does the warning.

**Things the scene model could not describe.** This category is empty now, and
what was in it is worth keeping because of how it was closed rather than that
it was.

A scene has to be writable without a device — that is what lets one list serve
a window, a headless comparison and a board at the end of a cable — so a scene
cannot hold a texture. It does not need to: an item says it samples an image,
and the executor uploads a single fixture and binds it, for the scenes that ask
and allocating nothing for the ones that do not. A mesh and a sprite batch were
the other two, and they are their own kinds of node rather than kinds of item,
because an item is a shape with a fill and everything that follows from that —
a stroke, a clip built from its outline, a transform applied to its path — and
a mesh has none of them.

A caller's fragment program joined on the same terms and for the same reason. A
scene says it uses the fixture effect and the executor registers it, because a
program cannot be written down without a device any more than a texture can.
Registering the same payload twice gives the same name back, which is what lets
a caller with nowhere to keep an index — every caller that renders a list of
scenes — register before each one without building a pipeline per scene.

The lesson worth carrying is about the derivations. What a scene needs from a
device, what tolerance it earns, whether it samples the fixture: all three are
derived from what the scene contains rather than declared beside it, and all
three walked its *items*. A node kind that was not an item would have been
missed silently — a mesh using an advanced blend reported as a backend
regression rather than as a known gap. They ask the node now, exhaustively, so
the compiler will not let the next kind be added without a decision for each.

This category was larger when it was first written, and wrongly so: it claimed
a color filter stated as a blend was among them. It was not. A filter is a
matrix by the time a scene carries one, and the constructor that builds one
from a blend mode had existed for days. Thirty-four blend scenes came across as
soon as somebody checked the claim instead of repeating it, which is the
argument for writing an inventory down rather than carrying it in one's head.

**A capability that was declined and then built.** Round superellipses. This
column carried them as a decision: that matching Flutter's curve means
transcribing a fitted table nothing here could check, so drawing a different
curve under the same name would be worse than drawing none. The premise was
wrong -- upstream publishes points on the boundary, which is exactly the
reference the entry said did not exist -- and the shape is now built and its
seven plates are here. `docs/parity.md` has what checks the transcription, and
`docs/non-parity.md` has the one upstream artifact it carries.

**Things that are deliberately out of scope.** Text, which needs shaping and
font parsing that `docs/architecture.md` places outside this project. That is
the whole of the list, and it used to be longer.

The rest of it read: the tests that check an optimization rather than a picture
— subpass collapse, clear-color elision, peepholes — on the reasoning that they
assert about how a frame was rendered rather than what it looks like, and this
renderer does not implement the optimizations. That was eight scenes across
three rows, and the reasoning does not survive being checked. Every one of them
opens a playground and shows a picture; none of them assert anything about a
pass count, because a playground test cannot. The optimization is what the test
is *named* for, and what it does is draw the shape the optimizer was getting
wrong.

So the picture is the picture with the optimization or without it, and this
renderer can draw all eight: two clear-color scenes in basic, one foreground
blend in blend, and five in the miscellany. They are here, and what would still
be out of scope — an assertion that a frame took one pass rather than two —
is not something any of them ever made.

## The scenes

Named for the test each mirrors, in the form `topic/CppTestName` reduced to
kebab case, so the original can be found by its name and a scene here with no
counterpart there would be visible as one. `catalog()` in
`crates/emblema-testkit/src/catalog.rs` is the list; the playground shows it
after the corpus, and `cargo test -p emblema-testkit --test catalog` renders
every one of them on both backends and compares.

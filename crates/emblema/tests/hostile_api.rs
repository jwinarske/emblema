//! What the public API does with data a caller did not sanitize.
//!
//! `emblema-geometry`'s `hostile.rs` covers the path and the tessellator,
//! which is where the geometry arrives. This covers the rest of the front
//! door: the calls that take a caller's *arrays* -- a mesh's positions and
//! indices, an atlas's sprites and source rectangles, a glyph run's placements,
//! a gradient's stops. Those are the arguments most likely to come from a file
//! or a wire rather than from a literal, and each has a shape the renderer
//! relies on.
//!
//! Recording only, with no device. Everything asserted here is decided while
//! building a `Recording`, so the whole file runs anywhere and in a
//! millisecond -- and a defect in it is a defect on every backend rather than
//! on the one that happened to be present.
//!
//! The claim is the same one the geometry file makes and is worth repeating
//! because it is easy to weaken by accident: *garbage in may give garbage out,
//! but it must not give a panic, and it must not give a recording that names
//! something that is not there.* A draw whose vertex indices run off the end
//! of its buffer is not a wrong picture; it is a read the driver performs on
//! this process's behalf.
//!
//! # These passed the first time they were run, which is not evidence
//!
//! All four did, and a property test that has never failed has not been shown
//! to be able to. So the mesh one was checked against the guard it is about:
//! deleting the index bounds check in `Vertices::build` did *not* fail it, and
//! the reason turned out to be worth knowing rather than a flaw. The invariant
//! has two independent guards -- `Vertices` refuses the index, and
//! `Batch::push` refuses it again with its own message -- so removing either
//! leaves the other holding, and the recording stays addressable. Removing
//! both fails the property, on `index 0 against 0 vertices`.
//!
//! That is the shape of thing worth writing down: the property is sound, the
//! codebase is belt-and-braces here, and a mutation that removes one belt
//! proves nothing about the test.

use emblema::*;
use proptest::prelude::*;

/// Floats chosen to break arithmetic rather than to describe a picture.
fn hostile_coord() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -500.0f32..500.0,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::NEG_INFINITY),
        1 => Just(0.0f32),
        1 => Just(-0.0f32),
        1 => Just(f32::MIN_POSITIVE),
        1 => Just(f32::MAX),
        1 => Just(1e20f32),
        1 => Just(-1e20f32),
    ]
}

fn hostile_point() -> impl Strategy<Value = [f32; 2]> {
    (hostile_coord(), hostile_coord()).prop_map(|(x, y)| [x, y])
}

fn hostile_color() -> impl Strategy<Value = Color> {
    (
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
    )
        .prop_map(|(r, g, b, a)| Color::srgb(r, g, b, a))
}

/// Every draw in every pass names vertices that exist, and every texture slot
/// it samples was declared by the pass it sits in.
///
/// The one invariant that is not about the picture. Everything below builds a
/// recording out of hostile input and then asks this of it.
fn recording_is_addressable(recording: &Recording) -> Result<(), String> {
    for (n, pass) in recording.passes.iter().enumerate() {
        let vertices = pass.batch.vertices().len();
        for index in pass.batch.indices() {
            if *index as usize >= vertices {
                return Err(format!(
                    "pass {n}: index {index} against {vertices} vertices"
                ));
            }
        }
        if pass.batch.indices().len() % 3 != 0 {
            return Err(format!(
                "pass {n}: {} indices is not whole triangles",
                pass.batch.indices().len()
            ));
        }
        for slot in pass.batch.texture_slots() {
            if slot as usize >= pass.sources.len() {
                return Err(format!(
                    "pass {n}: slot {slot} against {} sources",
                    pass.sources.len()
                ));
            }
        }
    }
    Ok(())
}

const SIZE: Extent2D = Extent2D {
    width: 128,
    height: 128,
};

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// A mesh built from arbitrary positions, indices and colors.
    ///
    /// `Vertices` refuses an index that names a position it does not have, and
    /// that refusal is the thing under test as much as the drawing is: this
    /// asks for the refusal by generating indices against a shorter list than
    /// they address, over and over.
    #[test]
    fn a_mesh_of_any_shape_records_something_addressable(
        positions in prop::collection::vec(hostile_point(), 0..12),
        indices in prop::collection::vec(0u32..16, 0..24),
        colors in prop::collection::vec(hostile_color(), 0..12),
    ) {
        let positions: Vec<Vec2> = positions.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        for mode in [VertexMode::Triangles, VertexMode::TriangleStrip, VertexMode::TriangleFan] {
            let mesh = Vertices::indexed(mode, positions.clone(), Vec::new(), indices.clone());
            let _ = &colors;
            let Ok(mesh) = mesh else { continue };
            let mut canvas = Canvas::new(SIZE);
            canvas.clear(Color::BLACK);
            let _ = canvas.draw_vertices(&mesh, &Paint::fill(Color::WHITE));
            let recording = canvas.finish();
            prop_assert!(
                recording_is_addressable(&recording).is_ok(),
                "{:?}",
                recording_is_addressable(&recording)
            );
        }
    }

    /// A gradient whose stops are in no particular order, at no particular
    /// offsets, in any number.
    ///
    /// Offsets are what the shader walks to find a color, and a walk over
    /// unsorted or non-finite offsets is the sort of thing that reads past the
    /// end of a stop table. More than `MAX_STOPS` takes the tabulated route
    /// instead, which is a second code path with the same question.
    #[test]
    fn a_gradient_of_any_stops_records_something_addressable(
        stops in prop::collection::vec((hostile_color(), hostile_coord()), 0..12),
        start in hostile_point(),
        end in hostile_point(),
    ) {
        let stops: Vec<GradientStop> = stops
            .into_iter()
            .map(|(color, offset)| GradientStop::new(color, offset))
            .collect();
        let paint = Paint::default().with_shader(Shader::LinearGradient {
            start: Vec2::new(start[0], start[1]),
            end: Vec2::new(end[0], end[1]),
            stops,
            tile: TileMode::Clamp,
        });
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let _ = canvas.draw_rect(Rect::new(8.0, 8.0, 120.0, 120.0), &paint);
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{:?}",
            recording_is_addressable(&recording)
        );
    }

    /// A run of glyphs placed anywhere, at any size, naming any atlas cell.
    #[test]
    fn a_glyph_run_of_any_placement_records_something_addressable(
        placements in prop::collection::vec((hostile_point(), hostile_point(), 0u32..64), 0..10),
    ) {
        let glyphs: Vec<PositionedGlyph> = placements
            .into_iter()
            .map(|(position, size, id)| PositionedGlyph {
                key: GlyphKey { font: 0, glyph: id as u16, size: 12 },
                position,
                size,
            })
            .collect();
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        // An empty atlas, which is the hostile case for a run: every glyph
        // names a cell the atlas does not hold.
        let atlas = Atlas::new(64);
        let _ = canvas.draw_glyphs(&glyphs, &atlas, 0, &Paint::fill(Color::WHITE));
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{:?}",
            recording_is_addressable(&recording)
        );
    }

    /// Sprites whose source rectangles need not lie inside any image.
    #[test]
    fn an_atlas_of_any_sprites_records_something_addressable(
        sprites in prop::collection::vec(
            (hostile_coord(), hostile_coord(), hostile_coord(), hostile_coord(), hostile_point()),
            0..10,
        ),
    ) {
        let sprites: Vec<Sprite> = sprites
            .into_iter()
            .map(|(x, y, w, h, at)| Sprite {
                source: SourceRect::new(x, y, w, h),
                color: Color::WHITE,
                transform: Affine2::from_translation(Vec2::new(at[0], at[1])),
            })
            .collect();
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let _ = canvas.draw_atlas(&sprites, SIZE, &Paint::fill(Color::WHITE));
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{:?}",
            recording_is_addressable(&recording)
        );
    }
}

/// A stack of layers deeper than any interface needs is recorded, not refused and
/// not fatal.
///
/// `save_layer` is the one call a caller can nest without bound, and each nesting
/// costs a pass and a target for it. So the arithmetic a hostile caller reaches is not
/// a big array but a deep stack, and the two failures worth ruling out are a recursion
/// that runs out of stack and a limit that was never written down.
///
/// Recorded here rather than executed, like the rest of this file, and the numbers
/// below say why that is not a dodge: a thousand nested layers at sixty-four pixels
/// square draw in a tenth of a second, and what a device would then do with a
/// thousand targets is a question about memory on that device rather than about this
/// API. The recording is where a stack overflow would happen, and it does not.
///
/// What this does not claim is that a deep nest is *useful*. Each layer composites
/// through its own opacity, so a thousand at nine tenths each leaves the drawing
/// multiplied by nine tenths to the thousandth, which is nothing. Arriving at
/// nothing by arithmetic is a correct picture; arriving at it by aborting is not.
#[test]
fn a_stack_of_layers_deeper_than_anything_needs_is_still_a_recording() {
    for depth in [1usize, 64, 1024] {
        let mut canvas = Canvas::new(Extent2D::new(64, 64));
        canvas.clear(Color::BLACK);
        for _ in 0..depth {
            canvas.save_layer(Layer::opacity(0.9));
        }
        assert_eq!(
            canvas.layer_depth(),
            depth,
            "the canvas lost count of the layers it has open"
        );
        canvas
            .draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE))
            .expect("a rectangle inside a deep stack is still a rectangle");
        for _ in 0..depth {
            canvas.restore();
        }
        assert_eq!(canvas.layer_depth(), 0, "a balanced stack did not unwind");

        // One pass per layer and one for the root, which is what says the nesting
        // reached the recording rather than being flattened away somewhere.
        let recording = canvas.finish();
        assert_eq!(
            recording.passes.len(),
            depth + 1,
            "a nest of {depth} produced {} passes",
            recording.passes.len()
        );
    }
}

/// Unbalanced nesting neither panics nor invents a pass.
///
/// A caller who opens layers and forgets to close them is the ordinary way a deep
/// stack arrives -- a loop with an early return in it. `finish` has to cope, and what
/// it must not do is drop the drawing on the floor without saying so.
#[test]
fn layers_left_open_are_finished_rather_than_lost() {
    let mut canvas = Canvas::new(Extent2D::new(64, 64));
    canvas.clear(Color::BLACK);
    for _ in 0..32 {
        canvas.save_layer(Layer::opacity(0.9));
    }
    canvas
        .draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE))
        .expect("a rectangle");
    assert_eq!(canvas.layer_depth(), 32, "the layers did not open");

    // No `restore` at all: thirty-two layers still open when the recording is asked
    // for.
    let recording = canvas.finish();
    assert_eq!(
        recording.passes.len(),
        33,
        "an unbalanced stack lost the passes its layers opened"
    );
    assert!(
        recording.draw_count() > 0,
        "the drawing inside an unclosed layer went nowhere"
    );
}

/// Every field of a [`Layer`], generated.
///
/// Written as a struct literal on purpose, and that is the whole reason this is a
/// function rather than a chain of `with_` calls. `Layer` has eight fields and is
/// not `#[non_exhaustive]`, so a literal makes adding a ninth a compile error here
/// -- which is the only mechanism that keeps a generator honest as a type grows.
/// A builder chain would keep compiling and would silently stop covering the new
/// field, and this is a type that has grown twice: `morphology` and
/// `backdrop_blur` both arrived after the first pass over it, and both were missed
/// by an enumeration written out by hand.
fn hostile_layer() -> impl Strategy<Value = Layer> {
    (
        // Grouped in threes because `prop_oneof!` and tuple strategies both stop
        // at twelve, and eight fields plus their inner parts exceed it.
        (hostile_coord(), hostile_coord(), hostile_coord()),
        (
            prop::sample::select(BlendMode::ALL),
            prop::option::of((hostile_coord(), hostile_coord(), hostile_coord())),
            prop::option::of((hostile_coord(), hostile_coord(), any::<bool>())),
        ),
        (
            prop_oneof![
                4 => Just(ColorFilter::None),
                1 => (hostile_coord(), prop::sample::select(BlendMode::ALL))
                    .prop_map(|(c, mode)| ColorFilter::Blend {
                        color: [c, c, c, c],
                        mode,
                    }),
                1 => Just(ColorFilter::Gamma { direction: Gamma::LinearToSrgb }),
                1 => Just(ColorFilter::Gamma { direction: Gamma::SrgbToLinear }),
            ],
            prop::option::of(any::<i64>()),
        ),
    )
        .prop_map(
            |(
                (blur_x, blur_y, alpha),
                (blend, matrix, morphology),
                (color_filter, backdrop_id),
            )| Layer {
                blur: Vec2::new(blur_x, blur_y),
                alpha,
                blend,
                matrix: matrix.map(|(x, y, r)| {
                    Transform2D::from(Affine2::from_scale(Vec2::new(x, y)))
                        * Transform2D::from(Affine2::from_angle(r))
                }),
                backdrop_blur: blur_y,
                backdrop_id,
                morphology: morphology.map(|(x, y, dilate)| Morphology {
                    radius: [x, y],
                    dilate,
                }),
                color_filter,
            },
        )
}

/// One call a caller could make, in a sequence of them.
///
/// The arrays a draw carries are covered above. This is the other axis and the one
/// nothing reached: the *order* calls arrive in, and in particular a stack that
/// does not balance. `Restore` is generated freely, including more often than
/// `Save`, because restoring past the bottom is a thing a caller does and must not
/// be a panic.
#[derive(Debug, Clone)]
enum Op {
    Save,
    Restore,
    SaveLayer(Layer),
    SaveLayerBounds(Layer, [f32; 4]),
    ClipRect([f32; 4]),
    ClipOutRect([f32; 4]),
    Translate(f32, f32),
    Scale(f32, f32),
    Rotate(f32),
    DrawRect([f32; 4], Color),
    DrawCircle([f32; 2], f32, Color),
    DrawPaint(Color),
    Clear(Color),
}

fn hostile_rect() -> impl Strategy<Value = [f32; 4]> {
    (
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
        hostile_coord(),
    )
        .prop_map(|(a, b, c, d)| [a, b, c, d])
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        3 => Just(Op::Save),
        4 => Just(Op::Restore),
        3 => hostile_layer().prop_map(Op::SaveLayer),
        2 => (hostile_layer(), hostile_rect())
            .prop_map(|(l, r)| Op::SaveLayerBounds(l, r)),
        2 => hostile_rect().prop_map(Op::ClipRect),
        1 => hostile_rect().prop_map(Op::ClipOutRect),
        2 => (hostile_coord(), hostile_coord()).prop_map(|(x, y)| Op::Translate(x, y)),
        2 => (hostile_coord(), hostile_coord()).prop_map(|(x, y)| Op::Scale(x, y)),
        1 => hostile_coord().prop_map(Op::Rotate),
        4 => (hostile_rect(), hostile_color()).prop_map(|(r, c)| Op::DrawRect(r, c)),
        2 => (hostile_point(), hostile_coord(), hostile_color())
            .prop_map(|(at, radius, c)| Op::DrawCircle(at, radius, c)),
        1 => hostile_color().prop_map(Op::DrawPaint),
        1 => hostile_color().prop_map(Op::Clear),
    ];
    prop::collection::vec(op, 0..40)
}

/// Apply one operation, ignoring the errors a hostile argument earns.
///
/// A refusal is a correct outcome here and is not what is under test -- the
/// invariants below are about what the canvas is left holding, which has to be
/// sound whether a call was taken or declined.
fn apply(canvas: &mut Canvas, op: &Op) {
    let rect = |r: &[f32; 4]| Rect::new(r[0], r[1], r[2], r[3]);
    match op {
        Op::Save => {
            canvas.save();
        }
        Op::Restore => {
            canvas.restore();
        }
        Op::SaveLayer(layer) => {
            canvas.save_layer(*layer);
        }
        Op::SaveLayerBounds(layer, r) => {
            canvas.save_layer_bounds(*layer, rect(r));
        }
        Op::ClipRect(r) => {
            let _ = canvas.clip_rect(rect(r));
        }
        Op::ClipOutRect(r) => {
            let _ = canvas.clip_out_rect(rect(r));
        }
        Op::Translate(x, y) => {
            canvas.translate(*x, *y);
        }
        Op::Scale(x, y) => {
            canvas.scale(*x, *y);
        }
        Op::Rotate(r) => {
            canvas.rotate(*r);
        }
        Op::DrawRect(r, color) => {
            let _ = canvas.draw_rect(rect(r), &Paint::fill(*color));
        }
        Op::DrawCircle(at, radius, color) => {
            let _ = canvas.draw_circle(Vec2::new(at[0], at[1]), *radius, &Paint::fill(*color));
        }
        Op::DrawPaint(color) => {
            let _ = canvas.draw_paint(&Paint::fill(*color));
        }
        Op::Clear(color) => {
            canvas.clear(*color);
        }
    }
}

/// How many of a sequence's operations open a stack entry.
fn opens(ops: &[Op]) -> usize {
    ops.iter()
        .filter(|op| matches!(op, Op::Save | Op::SaveLayer(_) | Op::SaveLayerBounds(..)))
        .count()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// Any sequence of calls, in any order, leaves a recording that names only
    /// what it holds.
    ///
    /// The same invariant the four properties above assert, reached along the axis
    /// they do not touch. Each of those makes one well-formed call carrying a
    /// hostile array; this makes up to forty calls whose *order* is hostile --
    /// unbalanced restores, clips under a degenerate transform, layers left open
    /// at `finish`.
    #[test]
    fn a_sequence_of_any_operations_records_something_addressable(ops in ops()) {
        let mut canvas = Canvas::new(SIZE);
        for op in &ops {
            apply(&mut canvas, op);
            // Checked inside the loop rather than only at the end, because the
            // two counts are a property of every intermediate state and a
            // violation the next operation repairs would otherwise go unseen.
            prop_assert!(
                canvas.layer_depth() <= canvas.save_depth(),
                "{} layers open against {} saves, so a layer is not on the stack \
                 it is counted from",
                canvas.layer_depth(),
                canvas.save_depth()
            );
            prop_assert!(
                canvas.save_depth() <= opens(&ops),
                "{} saves outstanding after at most {} operations that open one",
                canvas.save_depth(),
                opens(&ops)
            );
        }
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{}",
            recording_is_addressable(&recording).unwrap_err()
        );
    }

    /// The stack unwinds in exactly as many restores as it says are outstanding.
    ///
    /// `save_depth` is documented as being for "a caller checking its own
    /// balance", which is only true if it is the number of restores that balance
    /// it. That makes it a round trip rather than a reading: ask, restore that
    /// many times, and nothing may be left -- including no open layer, since
    /// `restore` pops whichever kind of entry is on top and a layer left behind
    /// would mean the two counts disagree about the same stack.
    #[test]
    fn the_stack_unwinds_in_as_many_restores_as_it_says(ops in ops()) {
        let mut canvas = Canvas::new(SIZE);
        for op in &ops {
            apply(&mut canvas, op);
        }
        let outstanding = canvas.save_depth();
        for _ in 0..outstanding {
            canvas.restore();
        }
        prop_assert_eq!(
            canvas.save_depth(),
            0,
            "{} restores left the stack un-emptied",
            outstanding
        );
        prop_assert_eq!(
            canvas.layer_depth(),
            0,
            "the saves are balanced but {} layers are still open",
            canvas.layer_depth()
        );
        // And one more past the bottom is a no-op rather than a panic, which is
        // what a caller over-restoring in an error path does.
        canvas.restore();
        prop_assert_eq!(canvas.save_depth(), 0);
    }

    /// A layer configured any way at all still records addressably.
    ///
    /// Reached through `save_layer` directly rather than through a sequence, so a
    /// failure names the layer rather than the forty calls around it. Every field
    /// is generated, including the two an earlier hand-written enumeration of this
    /// type missed.
    #[test]
    fn a_layer_of_any_configuration_records_something_addressable(
        layer in hostile_layer(),
        bounds in prop::option::of(hostile_rect()),
    ) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        match bounds {
            Some(r) => {
                canvas.save_layer_bounds(layer, Rect::new(r[0], r[1], r[2], r[3]));
            }
            None => {
                canvas.save_layer(layer);
            }
        }
        let _ = canvas.draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE));
        canvas.restore();
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{}",
            recording_is_addressable(&recording).unwrap_err()
        );
    }
}

/// The sequences reach the states the properties above are about.
///
/// Three of them could pass on a generator that never opened a layer, never left
/// one open at `finish`, and never restored past the bottom -- the three states
/// that distinguish this from the single-call properties. So this counts them.
/// Written as an ordinary test because it is a claim about the generator.
#[test]
fn the_generated_sequences_reach_the_states_worth_reaching() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = ops();
    let mut opened = 0usize;
    let mut left_open = 0usize;
    let mut over_restored = 0usize;
    let mut drew = 0usize;
    const TRIES: usize = 512;
    for _ in 0..TRIES {
        let ops = strategy
            .new_tree(&mut runner)
            .expect("the generator above is infallible")
            .current();
        let mut canvas = Canvas::new(SIZE);
        let mut bottomed = false;
        for op in &ops {
            if matches!(op, Op::Restore) && canvas.save_depth() == 0 {
                bottomed = true;
            }
            apply(&mut canvas, op);
        }
        if ops
            .iter()
            .any(|op| matches!(op, Op::SaveLayer(_) | Op::SaveLayerBounds(..)))
        {
            opened += 1;
        }
        if canvas.layer_depth() > 0 {
            left_open += 1;
        }
        if bottomed {
            over_restored += 1;
        }
        if canvas.finish().draw_count() > 0 {
            drew += 1;
        }
    }
    for (what, count) in [
        ("opened a layer", opened),
        ("left a layer open at finish", left_open),
        ("restored past the bottom", over_restored),
        ("recorded a draw", drew),
    ] {
        assert!(
            count * 10 >= TRIES,
            "only {count} of {TRIES} generated sequences {what}, so the \
             properties over that state are close to vacuous"
        );
    }
}

/// Every field of a [`Paint`], generated.
///
/// A struct literal for the reason `hostile_layer` is one: `Paint` has ten public
/// fields and is not `#[non_exhaustive]`, so a literal makes an eleventh a compile
/// error here rather than a field the generator quietly stops covering.
///
/// This exists because `Paint` has the shape that has already produced two
/// defects. Three of its fields are sanitized by a builder and public anyway --
/// `with_mask_blur` maps a non-finite sigma to zero, and `mask_blur` is a field --
/// which is exactly how `Layer::blur` carried an infinity into the blur reduction
/// and `Morphology`'s radius carried one into the pass loop. Nothing generated a
/// paint at all until this, so the whole family was reachable only through the
/// builders that clean it.
///
/// Probing it by hand first turned up no panic but plenty of incoherence: a mask
/// blur of infinity records a blur where the builder would record none, `1e20`
/// loses the draw entirely while the larger `f32::MAX` keeps it, and NaN and a
/// negative both correctly record nothing. Non-monotonic, and none of it a defect
/// by the contract at the top of this file -- which is the contract this asserts
/// rather than the coherence, because "garbage out" is allowed and a lost index is
/// not.
fn hostile_paint() -> impl Strategy<Value = Paint> {
    (
        // Threes again, because a tuple strategy stops at twelve and ten fields
        // with their innards exceed it.
        (
            hostile_color(),
            prop_oneof![
                3 => Just(Style::Fill),
                1 => (hostile_coord(), hostile_coord()).prop_map(|(width, miter_limit)| {
                    Style::Stroke(StrokeStyle {
                        width,
                        cap: LineCap::Butt,
                        join: LineJoin::Miter,
                        miter_limit,
                    })
                }),
            ],
            prop::sample::select(
                [
                    MaskBlurStyle::Normal,
                    MaskBlurStyle::Solid,
                    MaskBlurStyle::Outer,
                    MaskBlurStyle::Inner,
                ]
                .as_slice(),
            ),
        ),
        (
            prop_oneof![
                4 => Just(ImageFilter::None),
                1 => (hostile_coord(), hostile_coord()).prop_map(|(x, y)| ImageFilter::Blur {
                    sigma_x: x,
                    sigma_y: y,
                }),
                1 => (hostile_coord(), hostile_coord()).prop_map(|(x, y)| ImageFilter::Dilate {
                    radius_x: x,
                    radius_y: y,
                }),
            ],
            prop_oneof![
                4 => Just(ColorFilter::None),
                1 => Just(ColorFilter::Gamma { direction: Gamma::SrgbToLinear }),
            ],
            prop::option::of(
                (
                    prop::collection::vec(hostile_coord(), 0..4),
                    hostile_coord(),
                )
                    .prop_map(|(intervals, phase)| Dash::new(intervals, phase)),
            ),
        ),
        (
            hostile_coord(),
            prop::sample::select(BlendMode::ALL),
            prop::sample::select(BlendMode::ALL),
            any::<bool>(),
        ),
    )
        .prop_map(
            |(
                (color, style, mask_blur_style),
                (image_filter, color_filter, dash),
                (mask_blur, blend, tint_blend, anti_alias),
            )| Paint {
                shader: Shader::Solid(color),
                style,
                mask_blur_style,
                image_filter,
                color_filter,
                dash,
                mask_blur,
                blend,
                tint_blend,
                anti_alias,
            },
        )
}

/// Layers a builder would not produce, written out rather than generated.
///
/// See `a_paint_inside_a_filtered_layer_records_something_addressable` for why
/// these are a list and not a strategy.
fn hostile_layers() -> Vec<Layer> {
    let base = Layer::opacity(1.0);
    vec![
        base,
        // An infinite blur, which `with_blur_xy` maps to zero and a literal does
        // not. This is what carried infinity into the blur reduction.
        Layer {
            blur: Vec2::new(f32::INFINITY, 0.0),
            ..base
        },
        Layer {
            blur: Vec2::new(f32::MAX, f32::MAX),
            ..base
        },
        // A backdrop blur past its own builder, which is the same bypass again.
        Layer {
            backdrop_blur: f32::INFINITY,
            ..base
        },
        // A morphology radius past `sane`, which is what asked for six quintillion
        // passes before `applied_radius` clamped it.
        Layer {
            morphology: Some(Morphology {
                radius: [1e20, 1e20],
                dilate: true,
            }),
            ..base
        },
        // A matrix whose axes differ by seventy-six orders of magnitude, so the
        // preimage it widens to is degenerate one way and enormous the other.
        Layer {
            matrix: Some(Transform2D::from(Affine2::from_scale(Vec2::new(
                1.1754944e-38,
                3.4028235e38,
            )))),
            ..base
        },
        // Alpha that is not a fraction, under a blend that ignores it.
        Layer {
            alpha: f32::NAN,
            blend: BlendMode::Clear,
            ..base
        },
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// A paint assembled field by field still records addressably.
    ///
    /// Two shapes, because the paint's style decides which geometry path runs: a
    /// rectangle reaches the fill and the analytic routes, and a path with a
    /// curve reaches the tessellator and the dasher. A stroke width or a dash
    /// interval that is not a length has to be refused or survived, not trusted.
    #[test]
    fn a_paint_of_any_configuration_records_something_addressable(
        paint in hostile_paint(),
        curved in any::<bool>(),
    ) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        if curved {
            let mut path = PathBuilder::new();
            path.move_to(Vec2::new(16.0, 16.0));
            path.cubic_to(
                Vec2::new(96.0, 8.0),
                Vec2::new(8.0, 96.0),
                Vec2::new(112.0, 112.0),
            );
            let path = path.build();
            let _ = canvas.draw_path(&path, &paint);
        } else {
            let _ = canvas.draw_rect(Rect::new(16.0, 16.0, 112.0, 112.0), &paint);
        }
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{}",
            recording_is_addressable(&recording).unwrap_err()
        );
    }

    /// The same paint on a layer's content, where a filter pass sits above it.
    ///
    /// A paint's own image filter opens a layer per draw, so this nests one inside
    /// another and is the arrangement where a pass count multiplies rather than
    /// adds -- which is how the blur reduction's overflow was reached.
    ///
    /// The layer is *chosen* rather than generated, and that is a limit of the
    /// tooling rather than of the interest. Combining `hostile_layer` with
    /// `hostile_paint` overflows the stack inside `proptest`'s own `new_tree`
    /// before any case runs -- each generator is fine alone and the pair is not,
    /// because the strategy tree a nest of tuples and `prop_oneof` builds is walked
    /// recursively and a debug build's test thread has two mebibytes. It looked
    /// exactly like a renderer defect and was not, which is worth leaving written
    /// down here: the fix was to make the test shallower.
    ///
    /// So the layers below are the interesting ones written out. Each is a value a
    /// builder would refuse and a struct literal carries anyway, which is the shape
    /// that has already produced two defects.
    #[test]
    fn a_paint_inside_a_filtered_layer_records_something_addressable(
        paint in hostile_paint(),
        layer in prop::sample::select(hostile_layers()),
    ) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.save_layer(layer);
        let _ = canvas.draw_rect(Rect::new(16.0, 16.0, 112.0, 112.0), &paint);
        canvas.restore();
        let recording = canvas.finish();
        prop_assert!(
            recording_is_addressable(&recording).is_ok(),
            "{}",
            recording_is_addressable(&recording).unwrap_err()
        );
    }
}

/// A composition deeper than anything needs is refused, not fatal.
///
/// The companion to `a_stack_of_layers_deeper_than_anything_needs_is_still_a_recording`
/// and the case it did not cover. Layers nest through the canvas's own stack, which
/// is a `Vec`; a composed image filter nests through `Box`, and everything that
/// reads one walks it recursively -- `peel`, `covering`, `is_identity`, `scaled_by`
/// -- as does the pass building that follows. So a deep enough composition took the
/// process down rather than returning an error.
///
/// Measured before the limit existed: a debug build serviced two thousand and
/// forty-eight levels and died at three thousand and seventy-two. That boundary is
/// a property of the stack and the profile rather than of the picture, which is why
/// the limit is far below it.
///
/// Each level also costs a pass, so the accepted depth is already a recording of
/// two hundred and sixty passes over a picture whose shape nobody can see.
#[test]
fn a_composition_deeper_than_anything_needs_is_refused_rather_than_fatal() {
    fn nested(depth: usize) -> ImageFilter {
        let mut filter = ImageFilter::Blur {
            sigma_x: 1.0,
            sigma_y: 1.0,
        };
        for _ in 0..depth {
            filter = ImageFilter::Compose {
                outer: Box::new(ImageFilter::Color(ColorFilter::Gamma {
                    direction: Gamma::SrgbToLinear,
                })),
                inner: Box::new(filter),
            };
        }
        filter
    }

    let draw = |filter: &ImageFilter| -> (bool, usize) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let taken = canvas
            .save_layer_filtered(Layer::opacity(1.0), None, filter)
            .is_ok();
        let _ = canvas.draw_rect(Rect::new(8.0, 8.0, 56.0, 56.0), &Paint::fill(Color::WHITE));
        canvas.restore();
        (taken, canvas.finish().passes.len())
    };

    // Shallow compositions are what this is for, and they still work.
    for depth in [1usize, 16, 64] {
        let (taken, passes) = draw(&nested(depth));
        assert!(taken, "a composition {depth} deep was refused");
        assert!(
            passes > depth,
            "a composition {depth} deep produced only {passes} passes"
        );
    }

    // And a deep one is refused rather than fatal. Reaching these assertions at
    // all is most of what they say.
    //
    // Four and eight thousand rather than something enormous, and the ceiling is
    // this test's rather than the renderer's: a composition is a chain of `Box`es,
    // so *dropping* one recurses as deeply as it nests, and a sixty-five-thousand
    // deep filter overflowed the stack being freed at the end of the statement that
    // built it. That is a property of a recursively boxed type in the caller's own
    // code -- it happens before the filter is handed to anything here, and no
    // refusal on this side can prevent it. What this file can say is that the
    // renderer refuses what it is given rather than crashing on it, and the
    // refusal costs the limit rather than the filter's own size, because
    // `nests_deeper_than` walks iteratively and stops as soon as the limit is
    // passed.
    for depth in [4096usize, 8192] {
        let (taken, passes) = draw(&nested(depth));
        assert!(
            !taken,
            "a composition {depth} deep was accepted, which is the crash this \
             refusal replaced"
        );
        assert_eq!(
            passes, 1,
            "a refused filter still built {passes} passes, so the layer was opened \
             after all"
        );
    }

    // The same refusal on the backdrop path, which reads the filter with the same
    // recursive walks.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    assert!(
        canvas
            .save_layer_backdrop(Layer::opacity(1.0), None, &nested(4096))
            .is_err(),
        "a backdrop filter deeper than the limit was accepted"
    );

    // And on a paint, which reaches the recursion through `peel` instead.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    assert!(
        canvas
            .draw_rect(
                Rect::new(8.0, 8.0, 56.0, 56.0),
                &Paint::fill(Color::WHITE).with_image_filter(nested(4096)),
            )
            .is_err(),
        "a paint's filter deeper than the limit was accepted"
    );
}

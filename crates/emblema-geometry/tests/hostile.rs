//! What the geometry does when it is handed numbers nobody meant.
//!
//! [`property.rs`](property.rs) checks the invariants the tessellator relies
//! on over coordinates a real viewport might see, and says in its own header
//! that the degenerate cases are deliberately kept out of it. This is those
//! cases, generated rather than chosen: NaN, both infinities, subnormals, the
//! largest finite float, and structures no caller would write on purpose --
//! subpaths that never move, closes with nothing to close, curves whose
//! control points coincide, thousands of verbs in a row.
//!
//! The claim is not that any of it draws something sensible. Garbage in is
//! allowed to give garbage out. The claim is narrower and is the one that
//! matters to a caller who does not control the data:
//!
//! - it returns rather than panicking, and
//! - the buffers it returns are addressable.
//!
//! The second is the one with teeth. A vertex buffer's indices are handed
//! straight to the GPU and used to index the vertex array, so an index past
//! the end of it is not a wrong picture -- it is a read the driver performs on
//! this process's behalf out of whatever follows the buffer.
//!
//! # Why this and not `cargo-fuzz`
//!
//! libFuzzer needs a nightly toolchain and an LLVM runtime, and this workspace
//! must compile from pure Rust with no C toolchain -- a requirement
//! `deny.toml` bans two crates to protect and `xtask/tests/dependencies.rs`
//! enforces without needing anything installed. A fuzz target would have to
//! sit outside the workspace and run on a toolchain nothing else here uses.
//! `proptest` is already a dependency, shrinks a failure to its smallest form,
//! and runs in the ordinary gate on every commit, which is worth more than
//! coverage-guided depth for input this shallow.

use emblema_geometry::stroke::{LineCap, LineJoin, StrokeStyle};
use emblema_geometry::tessellate::Tessellator;
use emblema_geometry::{FillRule, Path, PathBuilder, MAX_COORDINATE};
use glam::Vec2;
use proptest::prelude::*;

/// Floats chosen to break arithmetic rather than to describe a picture.
///
/// Weighted toward the ordinary, because a path made entirely of NaN exercises
/// the first guard and nothing after it. What finds a defect is one bad number
/// among good ones -- a subpath that is fine until its third point.
fn hostile_coord() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -2_000.0f32..2_000.0f32,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::NEG_INFINITY),
        1 => Just(0.0f32),
        1 => Just(-0.0f32),
        1 => Just(f32::MIN_POSITIVE),
        1 => Just(-f32::MIN_POSITIVE),
        1 => Just(f32::MAX),
        1 => Just(f32::MIN),
        1 => Just(1e30f32),
        1 => Just(-1e30f32),
    ]
}

fn hostile_point() -> impl Strategy<Value = Vec2> {
    (hostile_coord(), hostile_coord()).prop_map(|(x, y)| Vec2::new(x, y))
}

/// One instruction to a path builder.
#[derive(Debug, Clone)]
enum Verb {
    Move(Vec2),
    Line(Vec2),
    Quad(Vec2, Vec2),
    Conic(Vec2, Vec2, f32),
    Cubic(Vec2, Vec2, Vec2),
    Arc(Vec2, Vec2, f32, f32),
    Close,
}

fn verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        hostile_point().prop_map(Verb::Move),
        hostile_point().prop_map(Verb::Line),
        (hostile_point(), hostile_point()).prop_map(|(c, p)| Verb::Quad(c, p)),
        (hostile_point(), hostile_point(), hostile_coord())
            .prop_map(|(c, p, w)| Verb::Conic(c, p, w)),
        (hostile_point(), hostile_point(), hostile_point())
            .prop_map(|(a, b, c)| Verb::Cubic(a, b, c)),
        (
            hostile_point(),
            hostile_point(),
            hostile_coord(),
            hostile_coord()
        )
            .prop_map(|(c, r, s, w)| Verb::Arc(c, r, s, w)),
        Just(Verb::Close),
    ]
}

/// A path built from a run of them, in whatever order they came.
///
/// Deliberately not made well-formed first: a `close` before any `move`, a
/// curve with no current point, and a run of `move`s with nothing between them
/// are all cases a caller can produce and all are in here.
fn build(verbs: &[Verb], rule: FillRule) -> Path {
    let mut b = PathBuilder::new().with_fill_rule(rule);
    for v in verbs {
        match *v {
            Verb::Move(p) => {
                b.move_to(p);
            }
            Verb::Line(p) => {
                b.line_to(p);
            }
            Verb::Quad(c, p) => {
                b.quad_to(c, p);
            }
            Verb::Conic(c, p, w) => {
                b.conic_to(c, p, w);
            }
            Verb::Cubic(a, c, p) => {
                b.cubic_to(a, c, p);
            }
            Verb::Arc(c, r, s, w) => {
                b.arc(c, r, s, w);
            }
            Verb::Close => {
                b.close();
            }
        }
    }
    b.build()
}

fn fill_rule() -> impl Strategy<Value = FillRule> {
    prop_oneof![Just(FillRule::NonZero), Just(FillRule::EvenOdd)]
}

/// Tolerances including the ones that are not lengths.
fn hostile_tolerance() -> impl Strategy<Value = f32> {
    prop_oneof![
        6 => 0.01f32..4.0f32,
        1 => Just(0.0f32),
        1 => Just(-1.0f32),
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(1e-30f32),
        1 => Just(1e30f32),
    ]
}

fn hostile_stroke() -> impl Strategy<Value = StrokeStyle> {
    (
        prop_oneof![
            6 => 0.1f32..40.0f32,
            1 => Just(0.0f32),
            1 => Just(-4.0f32),
            1 => Just(f32::NAN),
            1 => Just(f32::INFINITY),
            1 => Just(1e30f32),
        ],
        prop_oneof![
            Just(LineCap::Butt),
            Just(LineCap::Round),
            Just(LineCap::Square)
        ],
        prop_oneof![
            Just(LineJoin::Miter),
            Just(LineJoin::Round),
            Just(LineJoin::Bevel)
        ],
        prop_oneof![
            4 => 1.0f32..20.0f32,
            1 => Just(0.0f32),
            1 => Just(-1.0f32),
            1 => Just(f32::NAN),
            1 => Just(f32::INFINITY),
        ],
    )
        .prop_map(|(width, cap, join, limit)| {
            StrokeStyle::new(width)
                .with_cap(cap)
                .with_join(join)
                .with_miter_limit(limit)
        })
}

/// Coordinates that are extreme but inside what the tessellator accepts.
///
/// The interesting values are the edges of the admitted range and the places
/// float arithmetic changes character -- the largest exactly-representable
/// integer, subnormals, both zeros -- rather than the ones outside it, which
/// are refused at the boundary and tell this property nothing.
fn in_range_coord() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -2_000.0f32..2_000.0f32,
        1 => Just(0.0f32),
        1 => Just(-0.0f32),
        1 => Just(f32::MIN_POSITIVE),
        1 => Just(-f32::MIN_POSITIVE),
        1 => Just(MAX_COORDINATE),
        1 => Just(-MAX_COORDINATE),
        1 => Just(MAX_COORDINATE - 1.0),
    ]
}

fn in_range_point() -> impl Strategy<Value = Vec2> {
    (in_range_coord(), in_range_coord()).prop_map(|(x, y)| Vec2::new(x, y))
}

fn in_range_verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        in_range_point().prop_map(Verb::Move),
        in_range_point().prop_map(Verb::Line),
        (in_range_point(), in_range_point()).prop_map(|(c, p)| Verb::Quad(c, p)),
        (in_range_point(), in_range_point(), -8.0f32..8.0f32)
            .prop_map(|(c, p, w)| Verb::Conic(c, p, w)),
        (in_range_point(), in_range_point(), in_range_point())
            .prop_map(|(a, b, c)| Verb::Cubic(a, b, c)),
        (
            in_range_point(),
            in_range_point(),
            -20.0f32..20.0f32,
            -20.0f32..20.0f32
        )
            .prop_map(|(c, r, s, w)| Verb::Arc(c, r, s, w)),
        Just(Verb::Close),
    ]
}

fn in_range_stroke() -> impl Strategy<Value = StrokeStyle> {
    (
        0.01f32..400.0f32,
        prop_oneof![
            Just(LineCap::Butt),
            Just(LineCap::Round),
            Just(LineCap::Square)
        ],
        prop_oneof![
            Just(LineJoin::Miter),
            Just(LineJoin::Round),
            Just(LineJoin::Bevel)
        ],
        0.0f32..40.0f32,
    )
        .prop_map(|(width, cap, join, limit)| {
            StrokeStyle::new(width)
                .with_cap(cap)
                .with_join(join)
                .with_miter_limit(limit)
        })
}

proptest! {
    // Five hundred and twelve was too few, and finding that out is the point.
    // The stack overflow below took eight thousand cases to appear reliably --
    // at five hundred it surfaced perhaps one run in three, which is how the
    // gate stayed green over a bug that aborts the process. A property test
    // that finds a defect intermittently is a property test that reports the
    // absence of one intermittently.
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    /// Filling anything at all leaves buffers the GPU can index.
    #[test]
    fn a_filled_path_of_any_shape_leaves_addressable_buffers(
        verbs in prop::collection::vec(verb(), 0..24),
        rule in fill_rule(),
        tolerance in hostile_tolerance(),
    ) {
        let path = build(&verbs, rule);
        let mut tess = Tessellator::new();
        let buffers = tess.fill(&path, tolerance);
        prop_assert!(
            buffers.is_well_formed(),
            "fill produced {} indices over {} vertices",
            buffers.indices.len(),
            buffers.vertices.len()
        );
        // And nothing non-finite reaches a buffer, whatever went in. A path
        // that carries one is refused at the boundary and comes back empty, so
        // this holds for hostile input by being vacuous and for accepted input
        // by being true.
        for v in &buffers.vertices {
            prop_assert!(v.is_finite(), "fill put {v:?} in the buffer");
        }
    }

    /// And stroking it, which has more arithmetic in it: an offset, a join at
    /// every vertex, and a miter that divides by the sine of half an angle.
    #[test]
    fn a_stroked_path_of_any_shape_leaves_addressable_buffers(
        verbs in prop::collection::vec(verb(), 0..24),
        rule in fill_rule(),
        style in hostile_stroke(),
        tolerance in hostile_tolerance(),
    ) {
        let path = build(&verbs, rule);
        let mut tess = Tessellator::new();
        let buffers = tess.stroke(&path, &style, tolerance);
        prop_assert!(
            buffers.is_well_formed(),
            "stroke produced {} indices over {} vertices",
            buffers.indices.len(),
            buffers.vertices.len()
        );
        // And nothing non-finite reaches a buffer, whatever went in. A path
        // that carries one is refused at the boundary and comes back empty, so
        // this holds for hostile input by being vacuous and for accepted input
        // by being true.
        for v in &buffers.vertices {
            prop_assert!(v.is_finite(), "stroke put {v:?} in the buffer");
        }
    }

    /// Input the tessellator accepts must tessellate to finite vertices.
    ///
    /// The two above allow garbage out for garbage in. This one does not,
    /// because there is nothing to blame: every coordinate is a real number
    /// inside the range `Path::is_within_tessellation_range` admits, so a NaN
    /// in the output was made rather than passed through -- and a NaN vertex
    /// reaches the rasterizer as a triangle of no defined extent.
    ///
    /// Generated in range rather than filtered to it. The first version
    /// assumed its way there and proptest gave up: with the hostile
    /// coordinates above, twenty-four verbs are almost never all finite, so
    /// nine in ten cases were thrown away and the run aborted on rejects
    /// before it had sampled anything.
    #[test]
    fn accepted_input_tessellates_to_finite_output(
        verbs in prop::collection::vec(in_range_verb(), 0..24),
        rule in fill_rule(),
        style in in_range_stroke(),
        tolerance in 0.01f32..4.0f32,
    ) {
        // Not asserted to be in range, and the first version of this made
        // exactly that mistake. The generator produces in-range input to the
        // *builder*, and a builder is free to produce points outside it: an
        // arc adds its radii to its center, so a center at the limit leaves
        // the range however small the radii are. What is asserted is what is
        // true either way -- an accepted path tessellates finite, and a
        // refused one tessellates to nothing.
        let path = build(&verbs, rule);
        let mut tess = Tessellator::new();
        for (what, buffers) in [
            ("fill", tess.fill(&path, tolerance).clone()),
            ("stroke", tess.stroke(&path, &style, tolerance).clone()),
        ] {
            for v in &buffers.vertices {
                prop_assert!(
                    v.is_finite(),
                    "{what} put {v:?} in the buffer from a path that is finite"
                );
            }
        }
    }

    /// The builder itself, without a tessellator behind it.
    ///
    /// A path's own accessors are what everything above the geometry reads to
    /// decide bounds, convexity and whether a shape is worth drawing, and they
    /// are reached before any tolerance is applied. They have to answer.
    #[test]
    fn a_path_answers_for_itself_however_it_was_built(
        verbs in prop::collection::vec(verb(), 0..24),
        rule in fill_rule(),
    ) {
        let path = build(&verbs, rule);
        let _ = path.bounds();
        let _ = path.convexity();
        let _ = path.is_finite();
        let _ = path.is_empty();
        let _ = path.as_rounded_rect();
        prop_assert_eq!(path.fill_rule(), rule);
        // Every verb's points are present: the buffers are read in step by
        // `segments`, so a verb without its points would walk off the end.
        let counted: usize = path.segments().map(|(_, points)| points.len()).sum();
        prop_assert!(counted <= path.points().len());
    }
}

/// The three shapes the properties above found, kept as themselves.
///
/// A property that shrank to one case is a case, and the next reader should
/// not have to re-derive it from a generator. Each of these took the process
/// down or allocated without bound before the boundary guards existed.
mod found_by_generation {
    use super::*;
    use emblema_geometry::MAX_COORDINATE;

    /// `debug_assert!(tolerance >= S::EPSILON * S::EPSILON)` inside lyon's
    /// cubic flattener. A tolerance of zero is a number a caller can arrive at
    /// by dividing a device tolerance by a very large scale.
    #[test]
    fn a_tolerance_of_zero_does_not_take_the_process_down() {
        let mut b = PathBuilder::new();
        b.cubic_to(Vec2::ZERO, Vec2::ZERO, Vec2::ZERO);
        let path = b.build();
        let mut tess = Tessellator::new();
        assert!(tess.fill(&path, 0.0).is_well_formed());
        assert!(tess
            .stroke(&path, &StrokeStyle::new(0.1), 0.0)
            .is_well_formed());
        for absurd in [-1.0, f32::NAN, f32::INFINITY, f32::MIN_POSITIVE] {
            assert!(
                tess.fill(&path, absurd).is_well_formed(),
                "fill at {absurd}"
            );
            assert!(
                tess.stroke(&path, &StrokeStyle::new(0.1), absurd)
                    .is_well_formed(),
                "stroke at {absurd}"
            );
        }
    }

    /// `nan_check` inside lyon's path builder. The coordinate handed in is
    /// finite -- it is `f32::MAX` -- and lyon's own arithmetic overflows it.
    #[test]
    fn a_finite_but_enormous_coordinate_does_not_take_the_process_down() {
        let mut b = PathBuilder::new();
        b.arc(Vec2::new(f32::MAX, 0.0), Vec2::ZERO, 0.0, -943.1208);
        let path = b.build();
        assert!(path.is_finite(), "the premise: every coordinate is finite");
        assert!(!path.is_within_tessellation_range());

        let mut tess = Tessellator::new();
        assert!(tess.fill(&path, 0.25).is_empty());
        assert!(tess.stroke(&path, &StrokeStyle::new(4.0), 0.25).is_empty());
    }

    /// A stroke wide enough to have made lyon's round join recurse four billion
    /// deep, which was a stack overflow and could not be caught.
    ///
    /// The mechanism is worth keeping even though the defect is gone. Lyon
    /// computed a round join's subdivision count as
    /// `num_segments.log2().round() as u32`, and Rust's `as` cast *saturates*:
    /// an infinite `num_segments` -- which a zero flattening step produces, and
    /// a huge radius does -- became `u32::MAX` rather than wrapping or
    /// panicking, and that value was used as a recursion depth. It was
    /// reachable in four lines through `Canvas::draw_path`, with no device
    /// involved.
    ///
    /// Reported as <https://github.com/nical/lyon/issues/959> and fixed in
    /// <https://github.com/nical/lyon/pull/961>, which clamps the count to
    /// sixteen subdivisions and does the same at the round cap -- a second site
    /// the report had not found. The workspace requires 1.0.21 or later.
    ///
    /// So this no longer asserts a refusal. It asserts that the widths that
    /// used to abort now tessellate, and that the ones with no geometry in them
    /// produce nothing rather than something malformed. `dart:ui` states no
    /// maximum stroke width and neither does upstream, so neither does this.
    #[test]
    fn a_stroke_wider_than_the_coordinate_range_tessellates() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(10.0, 10.0))
            .line_to(Vec2::new(50.0, 10.0))
            .line_to(Vec2::new(50.0, 50.0));
        let path = b.build();
        let mut tess = Tessellator::new();

        // The widths that used to abort the process. Reaching this line at all
        // is most of the assertion; the rest is that what comes back is finite,
        // since a stroke built from a saturated count would not be.
        for width in [1e30f32, f32::MAX, 1e7] {
            let style = StrokeStyle::new(width)
                .with_cap(LineCap::Round)
                .with_join(LineJoin::Round);
            let out = tess.stroke(&path, &style, 0.25);
            assert!(
                !out.vertices.is_empty(),
                "a stroke {width:e} wide should draw something"
            );
            assert!(
                out.vertices
                    .iter()
                    .all(|p| p.x.is_finite() && p.y.is_finite()),
                "a stroke {width:e} wide produced a vertex that is not a point"
            );
        }

        // And the widths that describe no stroke draw nothing, which lyon
        // handles rather than this: the same release added the non-finite
        // guards that make these safe to hand over.
        for width in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 0.0, -5.0] {
            let style = StrokeStyle::new(width)
                .with_cap(LineCap::Round)
                .with_join(LineJoin::Round);
            assert!(
                tess.stroke(&path, &style, 0.25).is_empty(),
                "a stroke {width:e} wide has no geometry in it"
            );
        }
    }

    /// The one with the widest blast radius, and the one no assertion catches
    /// because it is not an assertion failure. A stroke hands its curves to
    /// lyon, which subdivides them by its own arithmetic with no cap, so the
    /// vertex count grows linearly in the coordinate.
    ///
    /// Measured before the range guard: three verbs at a coordinate of `1e15`
    /// stroked to 31,694,355 vertices and 95,083,059 indices -- six hundred
    /// and thirty megabytes of position and index, in a release build, from a
    /// path a caller can write in four lines.
    #[test]
    fn a_large_coordinate_cannot_allocate_without_bound() {
        let curve = |m: f32| {
            let mut b = PathBuilder::new();
            b.move_to(Vec2::new(m, 0.0))
                .cubic_to(Vec2::new(m, m), Vec2::new(0.0, m), Vec2::new(-m, -m))
                .close();
            b.build()
        };
        let mut tess = Tessellator::new();

        // At the limit, a shape: bounded, and the number is what the constant
        // was chosen against.
        let at_limit = tess
            .stroke(&curve(MAX_COORDINATE), &StrokeStyle::new(4.0), 0.25)
            .vertices
            .len();
        assert!(
            at_limit < 100_000,
            "a curve at the limit strokes to {at_limit} vertices"
        );

        // Past it, nothing at all rather than a hundred times as much.
        for m in [1e8f32, 1e10, 1e12, 1e15, f32::MAX] {
            let out = tess.stroke(&curve(m), &StrokeStyle::new(4.0), 0.25);
            assert!(
                out.is_empty(),
                "a curve at {m:e} strokes to {} vertices",
                out.vertices.len()
            );
        }
    }
}

/// The case that showed lyon's own debug assertions had to come off.
///
/// Found by `accepted_input_tessellates_to_finite_output` and kept here as a
/// case of its own, because a proptest seed says a failure happened once and a
/// test says what it was. Every coordinate is inside
/// `Path::is_within_tessellation_range`, so this is input this renderer accepts
/// on purpose.
///
/// What it does to lyon 1.0.21 is reach `fill.rs`'s `recover_from_error` and
/// trip the assertion there -- `self.active.edges.first().map(|e|
/// !e.is_merge)`, a claim about the sweep line's own state that its recovery
/// path apparently cannot keep. In a release build there is no assertion and it
/// recovers into a real tessellation, a thousand and forty-nine vertices of
/// one; in a debug build it panicked, so the same path drew a shape in release
/// and took the process down under test. The workspace manifest turns that
/// package's debug assertions off and says why; the report is nical/lyon#963,
/// which carries a seven-point reproduction shrunk out of this one.
///
/// Reduced from what proptest handed over, and each of the three curves earns
/// its place: replacing the conic with a line, or shrinking the magnitude to a
/// thousand, makes it tessellate cleanly. The quad is the one piece that does
/// not need to be a curve -- two lines through the same points do it -- and it
/// is left as upstream's generator produced it so the case here and the seed in
/// `hostile.proptest-regressions` are the same case.
#[test]
fn a_path_at_the_coordinate_limit_tessellates_rather_than_panicking() {
    let far = 16_777_215.0f32;
    let mut builder = PathBuilder::new().with_fill_rule(FillRule::NonZero);
    builder.arc(
        Vec2::new(0.0, far),
        Vec2::new(1162.7828, -479.33395),
        7.361_401_6,
        -0.362_522_57,
    );
    builder.close();
    builder.conic_to(Vec2::new(0.0, 0.0), Vec2::new(-1.1754944e-38, far), 0.0);
    builder.move_to(Vec2::new(MAX_COORDINATE, far));
    builder.quad_to(Vec2::new(0.0, 0.0), Vec2::new(0.0, far));
    let path = builder.build();
    assert!(path.is_finite());
    assert!(path.is_within_tessellation_range());

    let mut tess = Tessellator::new();
    let filled = tess.fill(&path, 0.01);
    // Something rather than nothing, which is what says lyon recovered instead
    // of giving up -- and the check that would fail if a later version of it
    // decided to return empty here instead.
    assert!(
        !filled.is_empty(),
        "the path should tessellate to something"
    );
    for v in &filled.vertices {
        assert!(v.is_finite(), "fill put {v:?} in the buffer");
    }
}

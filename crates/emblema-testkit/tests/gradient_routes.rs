//! The two linear-gradient routes paint the same picture, over generated input.
//!
//! A rectangle recorded as one, filled with an axis-aligned gradient whose
//! endpoints are its own edges, is drawn by interpolating stop colors across
//! vertices -- upstream's `FastLinearGradient`, and §19 of
//! `docs/non-parity.md`. Anything else walks the stops per fragment. The two
//! are meant to be the same picture, and nothing else in this workspace can see
//! that they are: every other comparison here renders one route against another
//! *device* or another *backend*, which takes whichever route the geometry
//! selects on both sides.
//!
//! `the_two_gradient_routes_agree` in `emblema`'s public API tests asserts it
//! for one gradient. This asserts it for generated ones, which is where the
//! predicate's edges are: a stop list with repeated offsets, two stops at the
//! same place, a reversed axis, a tile mode that cannot matter, a transform that
//! turns the shape.
//!
//! # How the slow route is forced
//!
//! `Shape::Rect` reaches `Canvas::draw_rect`, which records the rectangle as
//! one; `Shape::Polygon` of the same four corners reaches `draw_path`, which
//! does not. So the pair is the same geometry and the same paint through two
//! routes, with nothing mocked.
//!
//! # Why one level and not zero
//!
//! Both sides dither, and the dither is a function of the fragment's position
//! rather than of the route, so it cancels. What is left is that one side
//! interpolates in the rasterizer's precision and the other evaluates in the
//! shader's. One level is what the hand-written case measures; if a generated
//! one exceeds it, the interesting question is which input did.

use emblema_hal::BlendMode;
use emblema_hal_vulkan::validation::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use emblema_testkit::scene::{Fill, Stop};
use emblema_testkit::{render_scene, Item, Scene, Shape};
use proptest::prelude::*;

/// A color without its alpha, which the stop list supplies.
fn rgb() -> impl Strategy<Value = [f32; 3]> {
    (0.0f32..1.0, 0.0f32..1.0, 0.0f32..1.0).prop_map(|(r, g, b)| [r, g, b])
}

/// The one alpha every stop in a list carries.
///
/// One rather than per stop, because the predicate refuses a list whose alphas
/// differ -- the two routes genuinely disagree there, and
/// `stops_of_differing_alpha_take_the_per_fragment_route` is where that is
/// pinned. Generating it here would make this property assert that a route it
/// does not take agrees with itself.
fn alpha() -> impl Strategy<Value = f32> {
    prop_oneof![
        3 => Just(1.0f32),
        1 => 0.1f32..1.0,
    ]
}

/// Stop offsets: sorted, starting at zero and ending at one.
///
/// Those two ends are what the predicate requires, so a list without them takes
/// the per-fragment route on both sides and the comparison says nothing. The
/// interior is free, including repeats -- a repeated offset is how a caller asks
/// for a hard edge, and it is the case that makes a section zero-width.
fn stops() -> impl Strategy<Value = Vec<Stop>> {
    (
        // At most `MAX_STOPS` all told, so the paint block holds them and the
        // fragment route walks the list rather than sampling a baked ramp. A
        // ramp is 256 texels resampling the stop function, and where a feature
        // is narrow against a texel the two routes differ by more than
        // precision -- measured and pinned in
        // `a_baked_ramp_is_the_coarser_of_the_two_routes`, which is a property
        // of the ramp path rather than of this one.
        prop::collection::vec((rgb(), 0.0f32..1.0), 0..emblema_hal::MAX_STOPS - 1),
        rgb(),
        rgb(),
        alpha(),
    )
        .prop_map(|(interior, first, last, alpha)| {
            let with = |c: [f32; 3]| [c[0], c[1], c[2], alpha];
            let mut offsets: Vec<f32> = interior.iter().map(|(_, o)| *o).collect();
            offsets.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in the range"));
            let mut out = vec![Stop::new(with(first), 0.0)];
            for ((color, _), offset) in interior.iter().zip(offsets) {
                out.push(Stop::new(with(*color), offset));
            }
            out.push(Stop::new(with(last), 1.0));
            out
        })
}

/// A rectangle on the target, with room for a transform to move it.
fn rect() -> impl Strategy<Value = ([f32; 2], [f32; 2])> {
    (4.0f32..40.0, 4.0f32..40.0, 24.0f32..88.0, 24.0f32..88.0)
        .prop_map(|(x, y, w, h)| ([x, y], [x + w, y + h]))
}

/// Clamping only, which is the one mode the interpolated route takes.
///
/// The predicate used to admit all four, on the reasoning that endpoints on the
/// shape's edges leave nothing outside for a tile mode to decide. This test is
/// what disproved it -- at a fractional edge the fragment walk's parameter does
/// leave `[0, 1]` and `Repeat` wraps to the far end, 56 of 255 apart. So the
/// other three are refused now, which
/// `the_interpolated_route_refuses_what_it_cannot_draw` pins, and generating
/// them here would only re-assert that refusal through a failed route check.
fn tile() -> impl Strategy<Value = emblema_hal::TileMode> {
    Just(emblema_hal::TileMode::Clamp)
}

/// Whether the gradient runs along x or y, and in which direction.
///
/// All four, because the predicate takes the endpoints in either order and the
/// section colors are wound differently per axis -- which is a thing I got
/// wrong writing the route and a single orientation would not have shown.
fn axis() -> impl Strategy<Value = (bool, bool)> {
    (any::<bool>(), any::<bool>())
}

#[derive(Debug, Clone)]
struct Case {
    min: [f32; 2],
    max: [f32; 2],
    fill: Fill,
    blend: BlendMode,
}

fn case() -> impl Strategy<Value = Case> {
    (rect(), stops(), tile(), axis(), blend()).prop_map(
        |((min, max), stops, tile, (horizontal, reversed), blend)| {
            let (a, b) = if horizontal {
                ([min[0], min[1]], [max[0], min[1]])
            } else {
                ([min[0], min[1]], [min[0], max[1]])
            };
            let (start, end) = if reversed { (b, a) } else { (a, b) };
            Case {
                min,
                max,
                fill: Fill::LinearGradient {
                    start,
                    end,
                    stops,
                    tile,
                },
                blend,
            }
        },
    )
}

fn blend() -> impl Strategy<Value = BlendMode> {
    prop_oneof![
        3 => Just(BlendMode::SrcOver),
        1 => Just(BlendMode::Src),
        1 => Just(BlendMode::Multiply),
    ]
}

/// The same case drawn both ways: as a rectangle, and as a polygon of its own
/// corners.
fn scenes(case: &Case) -> (Scene, Scene) {
    let item = |shape: Shape| Item::filled(shape, case.fill.clone()).with_blend(case.blend);
    let interpolated = Scene::new(
        "gradient-route-interpolated",
        vec![item(Shape::Rect {
            min: case.min,
            max: case.max,
        })],
    );
    let per_fragment = Scene::new(
        "gradient-route-per-fragment",
        vec![item(Shape::Polygon(vec![
            [case.min[0], case.min[1]],
            [case.max[0], case.min[1]],
            [case.max[0], case.max[1]],
            [case.min[0], case.max[1]],
        ]))],
    );
    (interpolated, per_fragment)
}

fn device() -> Option<Validated> {
    match Validated::new(DevicePreference::Software) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no software reference ({e})");
            None
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Whatever the stops, the axis, the tile mode or the blend, the two routes
    /// agree to a level.
    #[test]
    fn the_routes_agree_over_generated_gradients(case in case()) {
        // No ramp on either side, which is what makes this an equality rather
        // than a comparison against a resampling.
        prop_assume!(
            emblema_testkit::record_scene(&scenes(&case).1)
                .map(|r| r.ramps.is_empty())
                .unwrap_or(false)
        );
        let Some(mut ctx) = device() else { return Ok(()) };
        let (interpolated, per_fragment) = scenes(&case);

        // The interpolated side really took that route, or this compares a
        // picture with itself. Checked per case rather than once, because the
        // predicate is what is under test and a case that silently failed it
        // would pass this comparison.
        let recording = emblema_testkit::record_scene(&interpolated)
            .expect("an axis-aligned gradient on a rectangle records");
        prop_assert!(
            recording
                .passes
                .iter()
                .flat_map(|p| p.batch.draws())
                .any(|d| matches!(d.material, emblema_hal::Material::VertexGradient)),
            "the rectangle did not take the interpolated route"
        );

        let a = render_scene::<VulkanHal>(&mut ctx, &interpolated);
        let b = render_scene::<VulkanHal>(&mut ctx, &per_fragment);
        let (a, b) = match (a, b) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(_), Err(_)) => return Ok(()),
            (a, b) => {
                prop_assert!(
                    false,
                    "the route decided whether the scene renders: {:?} against {:?}",
                    a.err(),
                    b.err()
                );
                unreachable!()
            }
        };

        prop_assert_eq!(a.pixels.len(), b.pixels.len());
        let mut worst = 0i32;
        let mut at = 0usize;
        for (i, (x, y)) in a.pixels.iter().zip(b.pixels.iter()).enumerate() {
            let d = (*x as i32 - *y as i32).abs();
            if d > worst {
                worst = d;
                at = i;
            }
        }
        prop_assert!(
            worst <= 1,
            "the two gradient routes disagree by {} levels at byte {} of {}",
            worst,
            at,
            a.pixels.len()
        );
    }
}

/// Stops whose alphas differ take the per-fragment route, because the two do
/// not agree there.
///
/// A gradient is interpolated in *straight* color -- `dart:ui` and Skia do, and
/// the fragment walk follows: it interpolates the stops and premultiplies the
/// result. A vertex color cannot. What the rasterizer interpolates is what gets
/// multiplied in, so the colors are premultiplied before they reach it, and
/// premultiplying then interpolating is not interpolating then premultiplying
/// unless alpha is constant.
///
/// Found by the property above rather than by reading the code: its first
/// version generated an alpha per stop and failed on the fourth case, at 58 of
/// 255 for a two-stop gradient from alpha one to alpha a tenth. The hand-written
/// case in `public_api.rs` used opaque stops and could not see it.
#[test]
fn stops_of_differing_alpha_take_the_per_fragment_route() {
    let varying = |a: f32, b: f32| Case {
        min: [4.0, 4.0],
        max: [60.0, 40.0],
        fill: Fill::LinearGradient {
            start: [4.0, 4.0],
            end: [60.0, 4.0],
            stops: vec![
                Stop::new([1.0, 0.0, 0.0, a], 0.0),
                Stop::new([0.0, 0.0, 1.0, b], 1.0),
            ],
            tile: emblema_hal::TileMode::Clamp,
        },
        blend: BlendMode::SrcOver,
    };
    let took_it = |case: &Case| {
        emblema_testkit::record_scene(&scenes(case).0)
            .expect("records")
            .passes
            .iter()
            .flat_map(|p| p.batch.draws())
            .any(|d| matches!(d.material, emblema_hal::Material::VertexGradient))
    };
    assert!(
        took_it(&varying(1.0, 1.0)),
        "one alpha is the case it is for"
    );
    assert!(
        took_it(&varying(0.5, 0.5)),
        "translucent is fine so long as it is uniform"
    );
    assert!(
        !took_it(&varying(1.0, 0.1)),
        "differing alphas would be a different picture"
    );
}

/// A baked ramp is the coarser of the two routes, and by how much.
///
/// Past `MAX_STOPS` the fragment route samples a 256-texel ramp rather than
/// walking the stops, so it carries that ramp's own resampling. Where a feature
/// is narrow against a texel the vertex route is the *more* accurate of the two
/// -- which is why the property above excludes ramped gradients instead of
/// widening until they fit.
///
/// Measured here rather than asserted as a bound, because the bound is the
/// ramp's resolution against the stop list and there is no useful general one:
/// a feature narrower than a texel can be lost entirely.
#[test]
fn a_baked_ramp_is_the_coarser_of_the_two_routes() {
    let Some(mut ctx) = device() else { return };
    let black = [0.0f32, 0.0, 0.0, 1.0];
    let blue = [0.0f32, 0.0, 0.48, 1.0];
    let measure = |stops: Vec<Stop>, ctx: &mut Validated| -> (usize, i32) {
        let case = Case {
            min: [4.0, 4.0],
            max: [45.807266, 28.0],
            fill: Fill::LinearGradient {
                start: [4.0, 4.0],
                end: [45.807266, 4.0],
                stops,
                tile: emblema_hal::TileMode::Clamp,
            },
            blend: BlendMode::SrcOver,
        };
        let (interpolated, per_fragment) = scenes(&case);
        let ramps = emblema_testkit::record_scene(&per_fragment)
            .expect("records")
            .ramps
            .len();
        let a = render_scene::<VulkanHal>(ctx, &interpolated).expect("interpolated");
        let b = render_scene::<VulkanHal>(ctx, &per_fragment).expect("per fragment");
        let worst = a
            .pixels
            .iter()
            .zip(b.pixels.iter())
            .map(|(x, y)| (*x as i32 - *y as i32).abs())
            .max()
            .unwrap_or(0);
        (ramps, worst)
    };

    // A spike about one and a half pixels wide, inside the paint block.
    let (ramps, worst) = measure(
        vec![
            Stop::new(black, 0.0),
            Stop::new(blue, 0.167_503),
            Stop::new(black, 0.203_762),
            Stop::new(black, 1.0),
        ],
        &mut ctx,
    );
    assert_eq!(ramps, 0, "four stops fit the paint block");
    assert_eq!(
        worst, 0,
        "both routes walk the same list, so they agree exactly"
    );

    // The same spike, with a fifth stop that forces the ramp.
    let (ramps, worst) = measure(
        vec![
            Stop::new(black, 0.0),
            Stop::new(black, 0.0),
            Stop::new(blue, 0.167_503),
            Stop::new(black, 0.203_762),
            Stop::new(black, 1.0),
        ],
        &mut ctx,
    );
    assert_eq!(ramps, 1, "five stops bake one");
    assert!(
        worst > 1,
        "the ramp should lose a feature this narrow, got {worst}"
    );

    // And where the features are wide the ramp resolves them.
    let (ramps, worst) = measure(
        vec![
            Stop::new(black, 0.0),
            Stop::new(black, 0.0),
            Stop::new(blue, 0.4),
            Stop::new(black, 0.8),
            Stop::new(black, 1.0),
        ],
        &mut ctx,
    );
    assert_eq!(ramps, 1);
    assert!(
        worst <= 1,
        "a ramp resolves a gradient this smooth, got {worst}"
    );
}

/// The per-fragment side is the per-fragment side.
///
/// Beside the per-case check above, because that one would also pass if the
/// polygon took the interpolated route too -- both sides being the same route
/// is the way this comparison can go quiet.
#[test]
fn a_traced_rectangle_takes_the_per_fragment_route() {
    let case = Case {
        min: [8.0, 8.0],
        max: [88.0, 72.0],
        fill: Fill::LinearGradient {
            start: [8.0, 8.0],
            end: [88.0, 8.0],
            stops: vec![
                Stop::new([1.0, 0.0, 0.0, 1.0], 0.0),
                Stop::new([0.0, 0.0, 1.0, 1.0], 1.0),
            ],
            tile: emblema_hal::TileMode::Clamp,
        },
        blend: BlendMode::SrcOver,
    };
    let (_, per_fragment) = scenes(&case);
    let recording = emblema_testkit::record_scene(&per_fragment).expect("records");
    let materials: Vec<_> = recording
        .passes
        .iter()
        .flat_map(|p| p.batch.draws())
        .map(|d| d.material.clone())
        .collect();
    assert!(
        materials
            .iter()
            .any(|m| matches!(m, emblema_hal::Material::LinearGradient { .. })),
        "a traced rectangle should walk the stops per fragment, got {materials:?}"
    );
    assert!(
        !materials
            .iter()
            .any(|m| matches!(m, emblema_hal::Material::VertexGradient)),
        "a traced rectangle took the interpolated route, so the comparison is vacuous"
    );
}

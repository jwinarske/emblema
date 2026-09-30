//! Scenes mirroring the ones Impeller's own playground offers.
//!
//! Impeller's playground is a mode of its test suite: a `TEST_P` that calls
//! `OpenPlaygroundHere` renders a frame, and with the flag set it opens a
//! window instead of running headless. The scenes are therefore its tests,
//! grouped by topic across `aiks_dl_*_unittests.cc` -- basic, blend, blur,
//! clip, gradient, opacity, path, primitive shape, vertices, atlas, shadow,
//! text and runtime effects.
//!
//! This is the same set of pictures, as far as this renderer can draw them.
//! `docs/playground-parity.md` is the inventory: what each file contains, what
//! is reproduced here, and what is blocked and on which missing capability.
//!
//! # Why not the corpus
//!
//! The corpus is deliberately small and varied -- each scene is there because
//! it exercises something no other scene does, and its tolerances are tuned
//! per scene against a software reference. Several hundred pictures organized
//! around somebody else's test suite is a different thing with a different
//! purpose, and folding them together would ruin the property that makes the
//! corpus useful: that a failure in it names a capability rather than a
//! picture.
//!
//! So these are their own collection. They render on both backends and are
//! compared against each other, which is what makes them worth having as
//! tests rather than only as pictures, but they carry no per-scene tolerance
//! and no claim to be minimal.
//!
//! # Naming
//!
//! Every scene is named for the test it mirrors, in the form
//! `topic/CppTestName` reduced to kebab case. A reader who wants to know what
//! a scene is supposed to show can find the original by its name, and a
//! scene here with no counterpart there would be visible as one.

use crate::scene::{
    AtlasSpec, Fill, GlyphRunSpec, Item, LayerSpec, MeshSpec, NinePatchSpec, Node, PaintSpec,
    PictureSpec, PointsSpec, Scene, ShadowSpec, SpriteSpec, Stop, StrokeSpec, Transform,
};
use crate::shape::Shape;
use emblema_core::{Affine2, ImageFilter, MaskBlurStyle, PointMode, Vec2, VertexMode};
use emblema_geometry::stroke::{LineCap, LineJoin};
use emblema_geometry::FillRule;
use emblema_hal::{BlendMode, ColorFilter, Extent2D, Sampling, TileMode};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.2, 0.4, 1.0, 1.0];
/// The same blue at half alpha, for the plate whose stroke overlaps itself.
const BLUE_HALF: [f32; 4] = [0.2, 0.4, 1.0, 0.5];
/// Skia's `kSkyBlue`, which upstream's difference-of-rounded-rects draws in.
const SKY_BLUE: [f32; 4] = [135.0 / 255.0, 206.0 / 255.0, 235.0 / 255.0, 1.0];
const YELLOW: [f32; 4] = [1.0, 0.9, 0.1, 1.0];
/// The ground every plate clears to.
///
/// Stated as exact eight-bit values rather than as round decimals, which is
/// not fussiness. The first version of this was `0.10` in blue, and `0.10`
/// times 255 is 25.5 -- a tie, which the two backends broke in opposite
/// directions. Every pixel of every plate then differed by one, the outlier
/// fraction went to ninety per cent, and the comparison that was supposed to
/// find a diverging backend was drowned by its own background.
const DARK: [f32; 4] = [15.0 / 255.0, 18.0 / 255.0, 26.0 / 255.0, 1.0];

/// Every catalog scene.
pub fn catalog() -> Vec<Scene> {
    let mut scenes = Vec::new();
    scenes.extend(basic());
    scenes.extend(primitive_shape());
    scenes.extend(path());
    scenes.extend(gradient());
    scenes.extend(clip());
    scenes.extend(opacity());
    scenes.extend(blend());
    scenes.extend(image());
    scenes.extend(vertices());
    scenes.extend(atlas_scenes());
    scenes.extend(blur());
    scenes.extend(shadow());
    scenes.extend(blur_variants());
    scenes.extend(backdrop_ids());
    scenes.extend(basic_pictures());
    scenes.extend(rounded_rect_radii());
    scenes.extend(save_layer_pictures());
    scenes.extend(extreme_strokes());
    scenes.extend(layers());
    scenes.extend(runtime_effect());
    scenes.extend(backdrops());
    scenes.extend(image_filters());
    scenes.extend(glyphs());
    scenes.extend(pictures());
    scenes
}

/// A scene on the catalog's own ground, which is dark so a white shape shows.
fn paint_plate(name: &'static str, specs: Vec<PaintSpec>) -> Scene {
    Scene::tree(
        name,
        specs
            .into_iter()
            .map(|s| Node::Paint(Box::new(s)))
            .collect(),
    )
    .with_background(DARK)
    .with_samples(4)
}

fn nine_plate(name: &'static str, specs: Vec<NinePatchSpec>) -> Scene {
    Scene::tree(
        name,
        specs
            .into_iter()
            .map(|s| Node::NinePatch(Box::new(s)))
            .collect(),
    )
    .with_background(DARK)
    .with_samples(4)
}

fn points_plate(name: &'static str, spec: PointsSpec) -> Scene {
    Scene::tree(name, vec![Node::Points(Box::new(spec))])
        .with_background(DARK)
        .with_samples(4)
}

fn plate(name: &'static str, items: Vec<Item>) -> Scene {
    Scene::new(name, items)
        .with_background(DARK)
        .with_samples(4)
}

/// Upstream's registration marks: four white lines just outside a group's
/// bounds, so a reader can see where its edge should have been.
fn registration_marks() -> Vec<Node> {
    [
        ([26.0f32, 44.0], [102.0f32, 44.0]),
        ([26.0, 84.0], [102.0, 84.0]),
        ([44.0, 26.0], [44.0, 102.0]),
        ([84.0, 26.0], [84.0, 102.0]),
    ]
    .into_iter()
    .map(|(from, to)| {
        Node::Draw(Box::new(Item::stroke(
            Shape::Line { from, to },
            StrokeSpec::new(1.0),
            WHITE,
        )))
    })
    .collect()
}

/// Upstream's three filter-collapse scenes, which differ only in the filter.
///
/// A white ground so a filter that inverts has something to invert, and a
/// square turned a third of a turn inside the group, which is what says the
/// group is recolored as a finished image rather than each shape before it is
/// placed.
fn filter_collapse(filter: ColorFilter) -> Vec<Node> {
    vec![
        Node::Paint(Box::new(PaintSpec {
            fill: Fill::Solid(WHITE),
            blend: BlendMode::Src,
            clip: None,
            clip_out: None,
            transform: Transform::default(),
        })),
        Node::Layer {
            layer: Box::new(LayerSpec {
                color_filter: filter,
                ..LayerSpec::default()
            }),
            bounds: None,
            transform: Transform::default(),
            children: vec![Node::Draw(Box::new(
                Item::fill(
                    Shape::Rect {
                        min: [-26.0, -26.0],
                        max: [26.0, 26.0],
                    },
                    BLUE,
                )
                .with_transform(Transform {
                    rotate: 120.0f32.to_radians(),
                    translate: [64.0, 64.0],
                    ..Transform::default()
                })
                .with_blend(BlendMode::SrcOver),
            ))],
        },
    ]
}

/// A plate drawn without multisampling.
///
/// For the scenes whose subject is what a blend computes rather than where an
/// edge falls. See the note at the blend family for why that distinction had to
/// be made rather than assumed.
fn single_sampled_plate(name: &'static str, items: Vec<Item>) -> Scene {
    Scene::new(name, items)
        .with_background(DARK)
        .with_samples(1)
}

/// A plate whose scene is a tree rather than a flat run of items.
///
/// The same background and sample count as [`plate`]; the difference is only
/// that a scene needing a `drawPaint` or a layer cannot say it as an `Item`.
fn plate_tree(name: &'static str, items: Vec<Node>) -> Scene {
    Scene::tree(name, items)
        .with_background(DARK)
        .with_samples(4)
}

/// The three filled rounded rectangles upstream draws, in both of the forms it
/// keeps them in.
///
/// A small radius, a large one, and one asked for more than half the shorter
/// side so it becomes a stadium rather than an outline that crosses itself.
/// Upstream keeps the same trio drawn as paths, and the reason is the corner:
/// the analytic route evaluates a rounded rectangle's field directly where the
/// path route scales each corner's radii to fit and then tessellates, and this
/// repository's `no-dimples-in-r-rect-path` exists because that scaling can be
/// done per corner instead of once and leave a visible notch.
fn filled_round_rects(as_path: bool) -> Vec<Item> {
    [
        (
            Shape::RoundedRect {
                min: [10.0, 10.0],
                max: [60.0, 50.0],
                radius: 4.0,
            },
            RED,
        ),
        (
            Shape::RoundedRect {
                min: [68.0, 10.0],
                max: [118.0, 50.0],
                radius: 20.0,
            },
            GREEN,
        ),
        (
            Shape::RoundedRect {
                min: [10.0, 62.0],
                max: [118.0, 118.0],
                radius: 64.0,
            },
            BLUE,
        ),
    ]
    .into_iter()
    .map(|(shape, color)| {
        let item = Item::fill(shape, color);
        if as_path {
            item.as_path()
        } else {
            item
        }
    })
    .collect()
}

/// Which of the four ways upstream says the same thin line.
#[derive(Clone, Copy)]
enum LineForm {
    /// Through `Canvas::draw_line`.
    Line,
    /// As a stroked two-point path.
    Path,
    /// As a filled rectangle the width of the stroke.
    Rect,
    /// As a filled rounded rectangle, its radius half the stroke width, which
    /// is what a round cap on the line would have drawn.
    RoundRect,
}

/// Upstream's `DrawLinesTest`, which four of its scenes share and differ only
/// in how the line is said.
///
/// Three columns of stroke width against five rows of angle, and in each cell
/// four parallel lines a quarter of a pixel further off the grid than the last.
/// The subpixel offsets are the point and are kept in device pixels rather
/// than scaled with the rest: what the family is for is how a line a third of a
/// pixel wide lands on a sample grid, and upstream says as much in a comment --
/// these are the scenes that deliberately do not scale with the display.
///
/// The first column asks for a width of zero, which upstream's rule widens to
/// the thinnest line a device can draw and this renderer refuses. So that
/// column is empty here, on purpose and by a decision `docs/parity.md` records,
/// and the plate is the one place it can be seen rather than read about.
fn draw_lines_grid(form: LineForm) -> Vec<Node> {
    let mut nodes = vec![Node::Paint(Box::new(PaintSpec {
        // Upstream's own ground for this family, which is darker than the one
        // the rest of the catalog clears to: a line a third of a pixel wide
        // arrives as a few levels of gray, and it needs somewhere dark to
        // arrive on.
        fill: Fill::Solid([
            0x11 as f32 / 255.0,
            0x11 as f32 / 255.0,
            0x11 as f32 / 255.0,
            1.0,
        ]),
        blend: BlendMode::Src,
        clip: None,
        clip_out: None,
        transform: Transform::default(),
    }))];
    const LENGTH: f32 = 24.0;
    for (col, width) in [0.0f32, 0.3, 1.0].into_iter().enumerate() {
        let cx = (col as f32 + 0.5) * (128.0 / 3.0);
        for (row, degrees) in [0.0f32, 3.0, 45.0, 87.0, 90.0].into_iter().enumerate() {
            let cy = (row as f32 + 0.5) * (128.0 / 5.0);
            let transform = Transform {
                rotate: degrees.to_radians(),
                translate: [cx, cy],
                ..Transform::default()
            };
            for i in 0..4 {
                // Four lines three and a half pixels apart, each a further
                // quarter of a pixel off the grid than the one before it.
                let y = i as f32 * 3.5 - 5.25 + i as f32 * 0.25;
                let (half_l, half_w) = (LENGTH / 2.0, width / 2.0);
                let item = match form {
                    LineForm::Line => Item::stroke(
                        Shape::Line {
                            from: [-half_l, y],
                            to: [half_l, y],
                        },
                        StrokeSpec::new(width),
                        WHITE,
                    ),
                    LineForm::Path => Item::stroke(
                        Shape::Polyline(vec![[-half_l, y], [half_l, y]]),
                        StrokeSpec::new(width),
                        WHITE,
                    ),
                    LineForm::Rect => Item::fill(
                        Shape::Rect {
                            min: [-half_l, y - half_w],
                            max: [half_l, y + half_w],
                        },
                        WHITE,
                    ),
                    LineForm::RoundRect => Item::fill(
                        Shape::RoundedRect {
                            min: [-half_l, y - half_w],
                            max: [half_l, y + half_w],
                            radius: half_w,
                        },
                        WHITE,
                    ),
                };
                nodes.push(Node::Draw(Box::new(
                    item.with_transform(transform)
                        .with_blend(BlendMode::SrcOver),
                )));
            }
        }
    }
    nodes
}

/// The warm and cool ends upstream's gradient scenes are drawn between.
const WARM: [f32; 4] = [0.9568, 0.2627, 0.2118, 1.0];
const COOL: [f32; 4] = [0.1294, 0.5882, 0.9529, 1.0];
/// The same cool end, transparent, which is what makes a decal's edge and an
/// image filter's spread visible rather than merely present.
const COOL_CLEAR: [f32; 4] = [0.1294, 0.5882, 0.9529, 0.0];

/// Upstream's seven-stop ramp, which four of its linear scenes and four of its
/// sweep scenes share and which differ only in the tile mode.
///
/// Seven matters. Four stops fit in the paint block and are interpolated by the
/// shader; a fifth sends the gradient to an uploaded ramp texture instead, and
/// `docs/non-parity.md` records that fork as a difference from upstream worth
/// watching. These are the scenes that take the second route.
fn seven_stops() -> Vec<Stop> {
    [
        [0x1f as f32 / 255.0, 0.0, 0x5c as f32 / 255.0, 1.0],
        [0x5b as f32 / 255.0, 0.0, 0x60 as f32 / 255.0, 1.0],
        [
            0x87 as f32 / 255.0,
            0x01 as f32 / 255.0,
            0x60 as f32 / 255.0,
            1.0,
        ],
        [
            0xac as f32 / 255.0,
            0x25 as f32 / 255.0,
            0x53 as f32 / 255.0,
            1.0,
        ],
        [
            0xe1 as f32 / 255.0,
            0x6b as f32 / 255.0,
            0x5c as f32 / 255.0,
            1.0,
        ],
        [
            0xf3 as f32 / 255.0,
            0x90 as f32 / 255.0,
            0x60 as f32 / 255.0,
            1.0,
        ],
        [1.0, 0xb5 as f32 / 255.0, 0x6b as f32 / 255.0, 1.0],
    ]
    .into_iter()
    .enumerate()
    .map(|(i, color)| Stop::new(color, i as f32 / 6.0))
    .collect()
}

/// The seven-stop ramp across a third of the shape, so every tile mode has
/// something outside it to act on.
fn many_colors(tile: TileMode) -> Fill {
    Fill::LinearGradient {
        start: [4.0, 4.0],
        end: [44.0, 44.0],
        stops: seven_stops(),
        tile,
    }
}

/// The same ramp swept through ninety degrees about the middle of the plate,
/// which leaves three quarters of the turn for the tile mode to fill.
fn sweep_many_colors(tile: TileMode) -> Fill {
    Fill::SweepGradient {
        center: [64.0, 64.0],
        start_angle: std::f32::consts::FRAC_PI_4,
        end_angle: std::f32::consts::FRAC_PI_4 * 3.0,
        stops: seven_stops(),
        tile,
    }
}

/// Upstream's fast-gradient plates: the same axis-aligned ramp on a rectangle
/// and on a rounded rectangle beside it.
///
/// Both shapes rather than one, because the fast route a gradient may take is
/// chosen per draw and the two draws are different geometry. A renderer that
/// took it for the rectangle and not for its rounded neighbor would show the
/// difference here and nowhere else.
///
/// The three stops are bunched at one end -- nought, a tenth, and one -- so an
/// implementation that spaced them evenly would be obvious rather than subtle.
fn fast_gradient(start: [f32; 2], end: [f32; 2], tile: TileMode) -> Vec<Item> {
    let fill = |()| Fill::LinearGradient {
        start,
        end,
        stops: vec![
            Stop::new(RED, 0.0),
            Stop::new(BLUE, 0.1),
            Stop::new(GREEN, 1.0),
        ],
        tile,
    };
    vec![
        Item::filled(
            Shape::Rect {
                min: [0.0, 0.0],
                max: [56.0, 120.0],
            },
            fill(()),
        )
        .with_transform(Transform::translate(4.0, 4.0)),
        Item::filled(
            Shape::RoundedRect {
                min: [0.0, 0.0],
                max: [56.0, 120.0],
                radius: 4.0,
            },
            fill(()),
        )
        .with_transform(Transform::translate(68.0, 4.0)),
    ]
}

/// Upstream's `MakeWideStrokedRects`, which two of its scenes draw and differ
/// only in how they say the rectangle.
///
/// Six outlines: three joins in a row where the stroke leaves a gap down the
/// middle, and the same three where it is wider than the shape and the two
/// sides of it land on the same pixels.
///
/// **The half alpha is the whole plate.** An outline that covers a pixel twice
/// is invisible at full opacity and darker at half, so a stroke drawn opaque
/// cannot show whether it overlapped itself -- and this plate, which is named
/// for not overlapping, used to be drawn opaque and could not. That it now
/// draws upstream's translucent blue is the difference between a picture and a
/// picture that can fail.
fn wide_stroked_rects(as_path: bool) -> Vec<Item> {
    let mut items = Vec::new();
    for (i, join) in [LineJoin::Bevel, LineJoin::Round, LineJoin::Miter]
        .into_iter()
        .enumerate()
    {
        let x = i as f32 * 43.0;
        // Twenty-two across with a stroke of eight: the inner edges stop
        // fourteen apart, so the four sides are four separate bands.
        items.push((
            Shape::Rect {
                min: [x + 10.0, 12.0],
                max: [x + 32.0, 34.0],
            },
            8.0,
            join,
        ));
        // Ten across with a stroke of twenty: each side reaches five past the
        // middle, so the left band and the right band want the same pixels and
        // so do the top and the bottom.
        items.push((
            Shape::Rect {
                min: [x + 16.0, 78.0],
                max: [x + 26.0, 88.0],
            },
            20.0,
            join,
        ));
    }
    items
        .into_iter()
        .map(|(shape, width, join)| {
            let item = Item::stroke(
                shape,
                StrokeSpec {
                    join,
                    ..StrokeSpec::new(width)
                },
                BLUE_HALF,
            )
            // Composited rather than replaced: half an alpha written by a mode
            // that replaces leaves a half-transparent hole, and what this plate
            // is read for is how the ink stacks.
            .with_blend(BlendMode::SrcOver);
            if as_path {
                item.as_path()
            } else {
                item
            }
        })
        .collect()
}

/// Skia's crimson, orange and purple, which upstream's mask-blur grid uses.
const CRIMSON: [f32; 4] = [220.0 / 255.0, 20.0 / 255.0, 60.0 / 255.0, 1.0];
const ORANGE: [f32; 4] = [1.0, 165.0 / 255.0, 0.0, 1.0];
const PURPLE: [f32; 4] = [128.0 / 255.0, 0.0, 128.0 / 255.0, 1.0];

/// White under a plate that would otherwise read as a blur against nothing.
///
/// Upstream's two mask-blur grids draw a white ground first, and it is not
/// decoration: a blur spreads coverage outward, and against this catalog's dark
/// ground a spread edge fades toward the ground it is already nearest. Against
/// white it fades the other way, which is the direction a reader can see.
fn white_ground() -> Node {
    Node::Paint(Box::new(PaintSpec {
        fill: Fill::Solid(WHITE),
        blend: BlendMode::Src,
        clip: None,
        clip_out: None,
        transform: Transform::default(),
    }))
}

/// The sigma both mask-blur grids are drawn at.
///
/// Upstream blurs at one against shapes a hundred points across. These plates
/// are a fifth of upstream's size, so the faithful sigma would be a fifth of a
/// pixel -- which is a blur nothing could see and no comparison could fail on.
/// One pixel against a twelve-pixel shape is the same *picture* at this scale
/// rather than the same number, and it is the picture the plate is for.
const GRID_BLUR: f32 = 1.0;

/// The three boxes upstream draws in every rounded-superellipse plate.
///
/// A square, a tall one and a wide one, at a quarter of upstream's
/// coordinates. Between them a corner is built from the shorter side in one
/// shape and the longer in another, which are different branches of the
/// normalize-and-scale the asymmetric case goes through.
fn rse_boxes(y: f32, radius: f32) -> Vec<Shape> {
    vec![
        Shape::RoundSuperellipse {
            min: [12.5, y],
            max: [37.5, y + 25.0],
            radii: [[radius, radius]; 4],
        },
        Shape::RoundSuperellipse {
            min: [50.0, y],
            max: [65.0, y + 35.0],
            radii: [[radius, radius]; 4],
        },
        Shape::RoundSuperellipse {
            min: [77.5, y],
            max: [112.5, y + 15.0],
            radii: [[radius, radius]; 4],
        },
    ]
}

/// The same three boxes at the plate's usual height, wrapped one way or another.
fn rse_trio(radius: f32, wrap: impl Fn(Shape) -> Item) -> Vec<Item> {
    rse_boxes(12.5, radius).into_iter().map(wrap).collect()
}

fn rse_stroke(width: f32) -> StrokeSpec {
    StrokeSpec {
        width,
        cap: LineCap::Butt,
        join: LineJoin::Round,
        miter_limit: 4.0,
        dash: None,
    }
}

/// `aiks_dl_basic_unittests.cc` -- shapes, strokes and arcs.
fn basic() -> Vec<Scene> {
    vec![
        // The seven rounded-superellipse plates. Upstream lays these out
        // across a canvas about five hundred wide; every coordinate here is a
        // quarter of theirs, which leaves each shape the same shape -- the
        // curve depends on the ratio of side to radius, and scaling both
        // leaves it alone.
        //
        // Three boxes recur across them and are worth naming once: a square,
        // a tall one and a wide one, so that a corner built from the shorter
        // side and one built from the longer are both drawn every time.
        plate(
            "basic/can-render-filled-round-superellipses",
            rse_trio(5.0, |shape| Item::fill(shape, BLUE)),
        ),
        // Four corners, all different, which is the only configuration that
        // reaches the code splitting a side between two unequal corners.
        plate(
            "basic/can-render-asymmetric-round-superellipses",
            vec![
                Item::fill(
                    Shape::RoundSuperellipse {
                        min: [6.0, 6.0],
                        max: [61.0, 61.0],
                        radii: [[7.5, 30.0], [25.0, 5.0], [22.5, 2.5], [5.0, 25.0]],
                    },
                    BLUE,
                ),
                Item::fill(
                    Shape::RoundSuperellipse {
                        min: [69.0, 6.0],
                        max: [124.0, 61.0],
                        radii: [[30.0, 5.0], [5.0, 30.0], [5.0, 30.0], [30.0, 5.0]],
                    },
                    RED,
                ),
                Item::fill(
                    Shape::RoundSuperellipse {
                        min: [6.0, 69.0],
                        max: [56.0, 119.0],
                        radii: [[30.0, 30.0], [5.0, 5.0], [5.0, 5.0], [5.0, 5.0]],
                    },
                    GREEN,
                ),
                Item::fill(
                    Shape::RoundSuperellipse {
                        min: [69.0, 69.0],
                        max: [119.0, 119.0],
                        radii: [[30.0, 30.0], [5.0, 5.0], [5.0, 5.0], [30.0, 30.0]],
                    },
                    BLUE,
                ),
            ],
        ),
        plate(
            "basic/can-render-stroked-round-superellipses",
            rse_trio(5.0, |shape| Item::stroke(shape, rse_stroke(1.25), BLUE)),
        ),
        // A radius small enough to sit near the threshold below which a corner
        // is treated as square, which is the branch nothing else here takes.
        plate(
            "basic/can-render-small-radius-round-superellipses",
            rse_trio(0.5, |shape| Item::fill(shape, BLUE)),
        ),
        // A stroke wider than twice the radius, so the inner offset of the
        // corner turns itself inside out if the join is wrong. Half
        // transparent, as upstream has it, so the overlap shows.
        plate(
            "basic/can-render-thick-stroked-round-superellipses",
            rse_trio(7.5, |shape| {
                Item::stroke(shape, rse_stroke(10.0), BLUE_HALF)
            }),
        ),
        // Three radii against the same three boxes: the ratio of side to
        // radius runs from about twenty down to two, which is the whole span
        // the fitted table covers plus the extrapolation past it.
        plate(
            "basic/can-render-round-superellipse-grid",
            [2.5f32, 7.5, 12.5]
                .iter()
                .enumerate()
                .flat_map(|(row, radius)| {
                    let y = 3.0 + row as f32 * 42.5;
                    rse_boxes(y, *radius)
                        .into_iter()
                        .map(|shape| Item::fill(shape, BLUE))
                        .collect::<Vec<_>>()
                })
                .collect(),
        ),
        // Rotated and unevenly scaled, so the curve is sampled off its own
        // axes. A shape flattened before the transform rather than after it
        // shows here as flats along the corner.
        plate(
            "basic/can-render-transformed-round-superellipse",
            vec![Item::fill(
                Shape::RoundSuperellipse {
                    min: [-17.5, -7.5],
                    max: [17.5, 7.5],
                    radii: [[5.0, 5.0]; 4],
                },
                BLUE,
            )
            .with_transform(Transform {
                scale: [1.5, 0.8],
                rotate: std::f32::consts::FRAC_PI_4,
                translate: [64.0, 64.0],
                ..Transform::default()
            })],
        ),
        plate(
            "basic/can-render-colored-rect",
            vec![Item::fill(
                Shape::Rect {
                    min: [32.0, 32.0],
                    max: [96.0, 96.0],
                },
                BLUE,
            )],
        ),
        // Wide enough that a naive stroke would overlap itself at the corners,
        // which is what the original is checking for.
        // A shear, which nothing else in the catalog uses and which is the
        // one transform class here that is not conformal: it leaves a
        // rectangle a parallelogram, so no axis survives it and a clip under
        // one cannot be a scissor. It is also the only transform that tells a
        // packed two-by-two from its transpose -- under a scale, or a rotation
        // of a symmetric shape, the two agree and a backend that swapped them
        // would draw the same picture as one that did not.
        plate(
            "basic/a-receding-plane",
            // What a four-by-four admits and a two-by-three cannot say. Three
            // bars of equal width in their own space, drawn under a divisor
            // that grows with x -- so they come out unequal, and the spacing
            // between them closes rather than staying constant. A transform
            // that merely scaled would keep both.
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [8.0, 24.0],
                        max: [32.0, 104.0],
                    },
                    BLUE,
                ),
                Item::fill(
                    Shape::Rect {
                        min: [48.0, 24.0],
                        max: [72.0, 104.0],
                    },
                    GREEN,
                ),
                Item::fill(
                    Shape::Rect {
                        min: [88.0, 24.0],
                        max: [112.0, 104.0],
                    },
                    YELLOW,
                ),
            ]
            .into_iter()
            .map(|item| {
                item.with_transform(Transform {
                    perspective: [0.006, 0.0],
                    ..Transform::default()
                })
            })
            .collect(),
        ),
        plate(
            "basic/shapes-under-a-shear",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [16.0, 12.0],
                        max: [76.0, 40.0],
                    },
                    BLUE,
                )
                .with_transform(Transform {
                    skew: [0.55, 0.0],
                    translate: [-30.0, 0.0],
                    ..Transform::default()
                }),
                // The analytic paths, which evaluate their shape per fragment
                // through the inverse of this transform rather than from
                // tessellated geometry -- so they are where a skew has to
                // survive an inversion rather than a vertex multiply.
                Item::fill(
                    Shape::RoundedRect {
                        min: [16.0, 50.0],
                        max: [76.0, 78.0],
                        radius: 12.0,
                    },
                    GREEN,
                )
                .with_transform(Transform {
                    skew: [0.55, 0.0],
                    translate: [-30.0, 0.0],
                    ..Transform::default()
                }),
                Item::fill(
                    Shape::Circle {
                        center: [46.0, 104.0],
                        radius: 22.0,
                    },
                    RED,
                )
                .with_transform(Transform {
                    skew: [0.55, 0.0],
                    translate: [-30.0, 0.0],
                    ..Transform::default()
                }),
            ],
        ),
        plate(
            "basic/a-gradient-under-a-shear",
            // A gradient reaches the fragment stage as a mapping back into
            // paint space, which is exactly the inverse this transform makes
            // non-symmetric. The bands have to lean with the shape; bands that
            // stayed square would be a mapping that dropped the off-diagonal,
            // and the shape would look right while the paint did not.
            vec![Item::filled(
                Shape::Rect {
                    min: [20.0, 20.0],
                    max: [108.0, 108.0],
                },
                Fill::LinearGradient {
                    start: [20.0, 20.0],
                    end: [108.0, 108.0],
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )
            .with_transform(Transform {
                skew: [0.0, 0.4],
                translate: [0.0, -26.0],
                ..Transform::default()
            })],
        ),
        plate(
            "basic/an-image-under-a-shear",
            // The same question for a sampled image, where the mapping decides
            // which texel each fragment reads. A sheared image is the case a
            // separable resampling shortcut cannot take, so what this asks is
            // that both backends read through the transform they were given.
            vec![Item::filled(
                Shape::Rect {
                    min: [24.0, 24.0],
                    max: [104.0, 104.0],
                },
                sheet(
                    [24.0, 24.0, 104.0, 104.0],
                    ALL,
                    TileMode::Clamp,
                    Sampling::Linear,
                ),
            )
            .with_transform(Transform {
                skew: [0.35, 0.0],
                translate: [-20.0, 0.0],
                ..Transform::default()
            })],
        ),
        paint_plate(
            "basic/a-color-fills-what-the-clip-admits",
            // `drawColor` fills the clip rather than the target, and the
            // difference only shows where the clip is not the target. Four
            // fills, each admitting a different region, so the picture is the
            // clips rather than the color.
            //
            // The areas were checked once and are exact, which is what says
            // each of the four painted rather than two of them painting and the
            // plate agreeing with itself on the other backend: eighteen
            // hundred and seventy-two for the plain rectangle, fifteen hundred
            // and ninety-two for the one with a hole, and fifteen hundred and
            // eighty-four for the sheared one -- a shear preserving area, so
            // that last is the unsheared rectangle's own.
            vec![
                // A plain rectangle.
                PaintSpec {
                    fill: Fill::Solid(BLUE),
                    blend: BlendMode::SrcOver,
                    clip: Some([6.0, 6.0, 58.0, 42.0]),
                    clip_out: None,
                    transform: Transform::default(),
                },
                // With a hole taken out of it, which is the clip stack rather
                // than one clip.
                PaintSpec {
                    fill: Fill::Solid(GREEN),
                    blend: BlendMode::SrcOver,
                    clip: Some([68.0, 6.0, 122.0, 42.0]),
                    clip_out: Some([84.0, 16.0, 106.0, 32.0]),
                    transform: Transform::default(),
                },
                // Under a rotation, where the region admitted is not a device
                // rectangle and the fill has to cover the turned one rather
                // than a box around it.
                PaintSpec {
                    fill: Fill::Solid(RED),
                    blend: BlendMode::SrcOver,
                    clip: Some([-24.0, -16.0, 24.0, 16.0]),
                    clip_out: None,
                    transform: Transform {
                        rotate: 0.5,
                        translate: [36.0, 78.0],
                        ..Transform::default()
                    },
                },
                // Under a shear, which is the transform no axis survives, and
                // added rather than drawn over so the overlap is visible.
                PaintSpec {
                    fill: Fill::Solid([0.9, 0.75, 0.2, 1.0]),
                    blend: BlendMode::Plus,
                    clip: Some([-22.0, -18.0, 22.0, 18.0]),
                    clip_out: None,
                    transform: Transform {
                        skew: [0.6, 0.0],
                        translate: [92.0, 84.0],
                        ..Transform::default()
                    },
                },
            ],
        ),
        nine_plate(
            "basic/a-nine-patch-stretched-wide-and-tall",
            // The whole of what a nine-patch means is which pieces stretch in
            // which direction, so one destination cannot show it: a square
            // would look the same as a plain image drawn to the same place.
            // These two are stretched along opposite axes from the same center,
            // so a corner that stretched, or an edge that stretched along the
            // wrong axis, differs between them rather than looking merely odd.
            vec![
                NinePatchSpec {
                    center: [3.0, 3.0, 5.0, 5.0],
                    into: [6.0, 8.0, 122.0, 46.0],
                    alpha: 1.0,
                    blend: BlendMode::SrcOver,
                    transform: Transform::default(),
                },
                NinePatchSpec {
                    center: [3.0, 3.0, 5.0, 5.0],
                    into: [8.0, 54.0, 46.0, 122.0],
                    alpha: 1.0,
                    blend: BlendMode::SrcOver,
                    transform: Transform::default(),
                },
                // Smaller than the sheet in both directions, where the middle
                // has to shrink rather than stretch and the corners still may
                // not: the case that separates a nine-patch from a scale.
                NinePatchSpec {
                    center: [3.0, 3.0, 5.0, 5.0],
                    into: [64.0, 60.0, 96.0, 92.0],
                    alpha: 1.0,
                    blend: BlendMode::SrcOver,
                    transform: Transform::default(),
                },
                // Faded, so the alpha the paint carries is shown to reach all
                // nine pieces rather than the first one drawn.
                NinePatchSpec {
                    center: [3.0, 3.0, 5.0, 5.0],
                    into: [98.0, 56.0, 124.0, 124.0],
                    alpha: 0.45,
                    blend: BlendMode::SrcOver,
                    transform: Transform::default(),
                },
            ],
        ),
        points_plate(
            "basic/points-in-all-three-modes",
            // `drawPoints` is marked complete in the parity table and had no
            // plate, so the three modes had never been put in front of the
            // second backend. A point is a segment of no length, so what is
            // drawn is the cap alone -- which makes this a test of cap
            // generation at a degenerate length rather than of geometry.
            PointsSpec {
                mode: PointMode::Points,
                points: (0..6).map(|i| [16.0 + i as f32 * 19.0, 24.0]).collect(),
                stroke: StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(13.0)
                },
                color: BLUE,
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
                clip_shape: None,
            },
        ),
        points_plate(
            "basic/points-as-separate-segments",
            // Pairs, and an odd point at the end that is drawn as nothing --
            // the rule that distinguishes this mode from the run below, and
            // the one a backend can quietly get wrong by drawing the leftover.
            PointsSpec {
                mode: PointMode::Lines,
                points: vec![
                    [14.0, 52.0],
                    [52.0, 74.0],
                    [66.0, 52.0],
                    [110.0, 78.0],
                    [90.0, 100.0],
                ],
                stroke: StrokeSpec {
                    cap: LineCap::Square,
                    ..StrokeSpec::new(9.0)
                },
                color: GREEN,
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
                clip_shape: None,
            },
        ),
        points_plate(
            "basic/points-as-one-open-run",
            // One run through all of them, so every interior position is a
            // join rather than two caps. A butt cap makes the two ends the only
            // place a cap appears, which is what separates this picture from
            // the same points drawn as segments.
            PointsSpec {
                mode: PointMode::Polygon,
                points: vec![
                    [12.0, 108.0],
                    [38.0, 86.0],
                    [58.0, 116.0],
                    [82.0, 84.0],
                    [116.0, 110.0],
                ],
                stroke: StrokeSpec {
                    cap: LineCap::Butt,
                    ..StrokeSpec::new(7.0)
                },
                color: RED,
                blend: BlendMode::SrcOver,
                transform: Transform::default(),
                clip_shape: None,
            },
        ),
        plate_tree(
            "basic/solid-color-circles-ovals-r-rects-mask-blur-correctly",
            {
                let mut items = vec![white_ground()];
                for (row, color) in [CRIMSON, BLUE, GREEN, PURPLE, ORANGE]
                    .into_iter()
                    .enumerate()
                {
                    let cy = row as f32 * 25.0 + 14.0;
                    for col in 0..5 {
                        let cx = col as f32 * 25.0 + 14.0;
                        // Upstream's shapes narrow as they widen, which is what
                        // makes the row a sweep rather than five of the same
                        // thing: the first is a sliver a tenth of its height
                        // and the last is a sliver the other way round.
                        let r = (col + 1) as f32 * 2.0;
                        let shape = match row {
                            0 => Shape::Rect {
                                min: [cx - r / 2.0, cy - (12.0 - r) / 2.0],
                                max: [cx + r / 2.0, cy + (12.0 - r) / 2.0],
                            },
                            1 => Shape::Circle {
                                center: [cx, cy],
                                radius: r,
                            },
                            2 => Shape::Oval {
                                min: [cx - r / 2.0, cy - (12.0 - r) / 2.0],
                                max: [cx + r / 2.0, cy + (12.0 - r) / 2.0],
                            },
                            // A round corner, then the same corner made an
                            // ellipse by holding one radius still: the last row
                            // is the only one whose corners are not circular,
                            // and it is why the eight-radius rounded rectangle
                            // had to exist before this plate could.
                            3 => Shape::RoundedRect {
                                min: [cx - 6.0, cy - 6.0],
                                max: [cx + 6.0, cy + 6.0],
                                radius: (col + 1) as f32,
                            },
                            _ => Shape::RoundedRectWithRadii {
                                min: [cx - 6.0, cy - 6.0],
                                max: [cx + 6.0, cy + 6.0],
                                radii: [[(col + 1) as f32, 1.0]; 4],
                            },
                        };
                        items.push(Node::Draw(Box::new(
                            Item::fill(shape, color)
                                .with_mask_blur(GRID_BLUR)
                                .with_blend(BlendMode::SrcOver),
                        )));
                    }
                }
                items
            },
        ),
        plate_tree(
            "basic/fast-elliptical-r-rect-mask-blurs-render-correctly",
            {
                let mut items = vec![white_ground()];
                // Five radii each way against one shape, so the grid runs from
                // a plain rectangle in one corner to a stadium in the other and
                // every corner between them is a different ellipse. The two
                // edges are the cases a rounded rectangle usually never sees:
                // one radius zero and the other not.
                for row in 0..5 {
                    for col in 0..5 {
                        let (x, y) = (col as f32 * 25.0 + 6.0, row as f32 * 25.0 + 6.0);
                        items.push(Node::Draw(Box::new(
                            Item::fill(
                                Shape::RoundedRectWithRadii {
                                    min: [x, y],
                                    max: [x + 16.0, y + 16.0],
                                    radii: [[col as f32 * 2.4, row as f32 * 2.4]; 4],
                                },
                                BLUE,
                            )
                            .with_mask_blur(GRID_BLUR)
                            .with_blend(BlendMode::SrcOver),
                        )));
                    }
                }
                items
            },
        ),
        Scene::tree(
            "basic/matrix-image-filter-doesnt-cull-when-scaled-and-translated-from-offscreen",
            // The plate below with a scale in the matrix as well, which is
            // upstream's second version of it. A pre-image computed from the
            // translation alone would be the right region at the wrong size,
            // so the circle arrives cropped rather than whole -- and at twice
            // the scale the crop is half of it.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    matrix: Some(Transform {
                        scale: [2.0, 2.0],
                        translate: [128.0, -64.0],
                        ..Transform::default()
                    }),
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(Item::fill(
                    Shape::Circle {
                        center: [-48.0, 64.0],
                        radius: 16.0,
                    },
                    GREEN,
                )))],
            }],
        ),
        Scene::tree(
            "basic/save-layer-with-bounds-larger-than-the-frame",
            // A group given bounds a hundred thousand pixels across. The
            // bounds say where the content is, and the target is what the
            // renderer allocates for it -- so a renderer taking them at their
            // word asks the device for a target it cannot have. They are
            // clamped to the frame here, and the plate is the three rectangles
            // upstream draws through such a group, at half alpha.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                }),
                bounds: Some([0.0, 0.0, 100000.0, 100000.0]),
                transform: Transform::default(),
                children: vec![
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [8.0, 8.0],
                            max: [88.0, 88.0],
                        },
                        RED,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [20.0, 20.0],
                            max: [100.0, 100.0],
                        },
                        GREEN,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [32.0, 32.0],
                            max: [112.0, 112.0],
                        },
                        BLUE,
                    ))),
                ],
            }],
        ),
        Scene::tree(
            "basic/can-render-clipped-backdrop-filter",
            // A backdrop filter whose bounds are the clip's bounding rectangle
            // while the clip itself is rounded. Upstream states the two that
            // way deliberately -- "the clip coverage and SaveLayer size are the
            // same" -- so the corners have to come off the composite because
            // the clip takes them, not because the layer is smaller.
            //
            // That is the arrangement a backdrop layer's target is now sized
            // from: it takes the clip's size, and the clip here is a curve
            // inside the rectangle the caller asked for. A target sized to the
            // curve's bounding box is right and a composite that forgot the
            // curve fills the corners with exclusion-blended ground.
            //
            // The ground is a repeating gradient rather than a flat color for
            // the reason the rest of this chapter's backdrop plates give: a
            // filter over one color is one color, and the bands are what say
            // where the filtered region starts and stops.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::LinearGradient {
                        start: [0.0, 0.0],
                        end: [32.0, 32.0],
                        stops: vec![
                            Stop::new([0.9568, 0.2627, 0.2118, 1.0], 0.0),
                            Stop::new([0.1294, 0.5882, 0.9529, 1.0], 1.0),
                        ],
                        tile: TileMode::Repeat,
                    },
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Clip {
                    rect: None,
                    rect_out: None,
                    shape: Some(Shape::RoundedRect {
                        min: [12.0, 12.0],
                        max: [116.0, 84.0],
                        radius: 26.0,
                    }),
                    transform: Transform::default(),
                    children: vec![Node::Layer {
                        layer: Box::new(LayerSpec {
                            backdrop: ImageFilter::Color(ColorFilter::blend(
                                RED,
                                BlendMode::Exclusion,
                            )),
                            ..LayerSpec::default()
                        }),
                        // The clip's bounding rectangle, as upstream states it.
                        bounds: Some([12.0, 12.0, 116.0, 84.0]),
                        transform: Transform::default(),
                        // Nothing drawn inside it. The filtered backdrop is the
                        // whole of the picture, which is upstream's scene too --
                        // its save paint is default and it draws nothing before
                        // the restore the builder makes for it.
                        children: vec![],
                    }],
                },
            ],
        ),
        Scene::tree(
            "basic/backdrop-filter-over-unclosed-clip",
            // A clip pushed and popped *before* a backdrop filter cuts the
            // pass. The cut takes the batch away and the clips are made again
            // in the pass that follows, so the thing this pins is which ones:
            // the outer clip is still in force and the inner two are not, and a
            // rebuild that replayed the batch's narrowings would resurrect them
            // and blur the corner alone.
            //
            // The other direction is already covered -- the superellipse sheet
            // fails if a parent clip is popped too early -- and this is the one
            // nothing asked: a popped clip coming back.
            //
            // Upstream leaves the layer for the enclosing restore to close
            // rather than closing it itself, which is where its name comes
            // from. A scene is a tree, so the layer being the clip's last child
            // says the same thing; what a tree cannot say is the order of two
            // closes, and nothing here depends on it.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid(WHITE),
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Clip {
                    rect: Some([16.0, 16.0, 128.0, 128.0]),
                    rect_out: None,
                    shape: None,
                    transform: Transform::default(),
                    children: vec![
                        // The corner, under two clips of its own, both gone by
                        // the time the backdrop reads.
                        Node::Clip {
                            rect: Some([96.0, 96.0, 128.0, 128.0]),
                            rect_out: None,
                            shape: None,
                            transform: Transform::default(),
                            children: vec![
                                Node::Paint(Box::new(PaintSpec {
                                    fill: Fill::Solid(RED),
                                    blend: BlendMode::SrcOver,
                                    clip: None,
                                    clip_out: None,
                                    transform: Transform::default(),
                                })),
                                Node::Paint(Box::new(PaintSpec {
                                    fill: Fill::Solid([0.0, 0.0, 1.0, 0.5]),
                                    blend: BlendMode::SrcOver,
                                    clip: None,
                                    clip_out: None,
                                    transform: Transform::default(),
                                })),
                                // Narrower again, and upstream states this one
                                // without a save of its own, so it stands until
                                // the block ends. A clip wrapping the one draw
                                // that follows it is the same region.
                                Node::Clip {
                                    rect: Some([112.0, 112.0, 120.0, 128.0]),
                                    rect_out: None,
                                    shape: None,
                                    transform: Transform::default(),
                                    children: vec![Node::Paint(Box::new(PaintSpec {
                                        fill: Fill::Solid([1.0, 0.0, 0.0, 0.5]),
                                        blend: BlendMode::SrcOver,
                                        clip: None,
                                        clip_out: None,
                                        transform: Transform::default(),
                                    }))],
                                },
                            ],
                        },
                        // Unbounded, so what it filters is everything the outer
                        // clip admits. Blurring flat white gives flat white, so
                        // the corner's edges are the whole of what moves -- and
                        // they are inside the region a resurrected clip would
                        // have kept, which is why the picture tells the two
                        // apart at all.
                        Node::Layer {
                            layer: Box::new(LayerSpec::default().with_backdrop_blur(8.0)),
                            bounds: None,
                            transform: Transform::default(),
                            children: vec![],
                        },
                    ],
                },
                // And a draw after the outer clip, three quarters of it outside.
                // A clip that survived its own restore would cut it square.
                Node::Draw(Box::new(Item::fill(
                    Shape::Circle {
                        center: [16.0, 16.0],
                        radius: 16.0,
                    },
                    [0.0, 1.0, 1.0, 1.0],
                ))),
            ],
        ),
        Scene::tree(
            "basic/perspective-rectangle",
            // Upstream states its projection first and its clip after it, so the
            // clip is a region in projected space and its own edges go through
            // the divide. That is a clip in force over a run rather than one
            // narrowing a single draw, which is the arrangement this collection
            // did not have: `clip/a-rectangular-clip-under-perspective` puts the
            // projection on the item, where the clip travels with that one draw.
            //
            // The term is on Y here, and every other perspective plate puts it
            // on X. A divide by `1 + p*y` is not the transpose of a divide by
            // `1 + p*x` as far as a rasterizer is concerned -- the interpolation
            // runs along the other axis -- so the two are separate pictures and
            // this is the second.
            //
            // The flood at half white is upstream's, and it is inside the clip:
            // it is the draw that says where the projected region actually is,
            // since it covers whatever the clip admits and nothing else.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([0.1, 0.1, 0.12, 1.0]),
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Clip {
                    rect: Some([8.0, 8.0, 72.0, 120.0]),
                    rect_out: None,
                    shape: None,
                    transform: Transform {
                        // Positive, so the divisor grows down the plate and the
                        // region narrows toward the bottom. Upstream's term is
                        // negative, and the sign is not what it is testing: its
                        // divisor carries a constant of 2.2 and its translation
                        // is half the frame, so the region it ends up with is on
                        // screen. A negative term here magnifies instead, and at
                        // the far edge of a plate this size it carries the
                        // bottom of the clip to y = 428 -- a trapezoid nobody
                        // can see is not the picture the test is about.
                        perspective: [0.0, 0.006],
                        translate: [24.0, 0.0],
                        ..Transform::default()
                    },
                    children: vec![
                        Node::Draw(Box::new(Item::fill(
                            Shape::Rect {
                                min: [8.0, 8.0],
                                max: [72.0, 120.0],
                            },
                            [0.1, 0.3, 0.9, 1.0],
                        ))),
                        Node::Paint(Box::new(PaintSpec {
                            fill: Fill::Solid([1.0, 1.0, 1.0, 0.5]),
                            blend: BlendMode::SrcOver,
                            clip: None,
                            clip_out: None,
                            transform: Transform::default(),
                        })),
                    ],
                },
            ],
        ),
        Scene::tree(
            "basic/can-draw-perspective-transform-with-clips",
            // The last of this file's seven, and the one with the most in it.
            // Three things here appear in no other plate: an image read through
            // a projection, a stencil clip stated in frame axes with projected
            // content inside it, and a clip whose geometry is drawn and popped
            // before the draw it is meant to sit behind.
            //
            // Upstream's comments name that third one as the point -- a clip
            // drawn and restored "will get drawn to the depth buffer behind the
            // image", and the oval that scopes the image "in front of the image
            // on the depth buffer". So what it is about is which depth a clip's
            // own geometry occupies relative to the draws around it, and it puts
            // one on each side of the image to say so.
            //
            // The projection stands in for upstream's rotation about Y, which
            // this scene model has no term for: a plane turned about the
            // vertical axis projects to a horizontal squeeze and a horizontal
            // divide, which is what the scale and the perspective term are.
            // Stated rather than implied, because a reader comparing the two
            // would otherwise look for the rotation and not find it.
            vec![
                // The surround, and then the difference clip that cuts the
                // middle out of it. Upstream floods twice, the second time under
                // a difference clip, so what is left of the first flood is the
                // square in the middle -- and the clip is gone by the time the
                // image draws, which is what puts its geometry behind it.
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid(GREEN),
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([0.0, 0.0, 0.0, 1.0]),
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: Some([28.0, 28.0, 100.0, 100.0]),
                    transform: Transform::default(),
                })),
                // The oval, in frame axes, holding an image that is not. Nothing
                // else here scopes projected content with an unprojected clip,
                // and the two cannot be folded together: the clip is a stencil
                // and the image's corners are where the divide put them.
                Node::Clip {
                    rect: None,
                    rect_out: None,
                    shape: Some(Shape::Oval {
                        min: [24.0, 24.0],
                        max: [104.0, 104.0],
                    }),
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(
                        Item::filled(
                            Shape::Rect {
                                min: [20.0, 20.0],
                                max: [108.0, 108.0],
                            },
                            sheet(
                                [20.0, 20.0, 108.0, 108.0],
                                ALL,
                                TileMode::Clamp,
                                Sampling::Linear,
                            ),
                        )
                        .with_transform(Transform {
                            perspective: [0.003, 0.0],
                            translate: [14.0, 0.0],
                            ..Transform::default()
                        }),
                    ))],
                },
                // And the translucent disc over the lot, which upstream draws
                // last and unclipped. It is wider than the oval, so it crosses
                // every region above and says they are all still there.
                Node::Draw(Box::new(Item::fill(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 46.0,
                    },
                    [0.0, 0.0, 1.0, 0.4],
                ))),
            ],
        ),
        Scene::tree(
            "basic/matrix-image-filter-doesnt-cull-when-translated-from-offscreen",
            // A circle drawn well off the left of the frame, inside a group
            // whose matrix carries it back into view. What the group captures
            // used to be bounded by the frame, so the circle was gone before
            // the matrix ran and the plate would have been empty.
            //
            // A group whose matrix will move its result records over the
            // pre-image now -- the region that lands where the frame can see it
            // once the matrix has been applied -- so what arrives is the whole
            // circle rather than a sliver or nothing.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    matrix: Some(Transform::translate(96.0, 0.0)),
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(Item::fill(
                    Shape::Circle {
                        center: [-32.0, 64.0],
                        radius: 30.0,
                    },
                    GREEN,
                )))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        plate(
            "basic/can-render-colored-rect-primitive",
            // Upstream's whole scene: one rectangle in one color, drawn
            // through the call the API offers for it. There is nothing to it,
            // and that is what it is for -- every other plate in this chapter
            // rests on this one working.
            vec![Item::fill(
                Shape::Rect {
                    min: [32.0, 32.0],
                    max: [96.0, 96.0],
                },
                BLUE,
            )],
        ),
        plate_tree(
            "basic/empty-save-layer-ignores-paint",
            // A layer with nothing drawn into it composites nothing, whatever
            // its paint says. Upstream's scene paints the frame red, clips,
            // opens a layer with a blue paint and closes it at once, and the
            // frame stays red.
            //
            // The clip is the half that can fail here. An empty layer still
            // allocates a target and still composites it, and a target
            // composited without its contents having been cleared is the
            // failure this would show -- as a rectangle of whatever the
            // allocation held, exactly where the clip is.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid(RED),
                    blend: BlendMode::Src,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Layer {
                    layer: Box::new(LayerSpec::default()),
                    bounds: Some([32.0, 32.0, 96.0, 96.0]),
                    transform: Transform::default(),
                    children: Vec::new(),
                },
            ],
        ),
        plate_tree(
            "basic/subpass-with-clear-color-optimization",
            // A bounded group at half alpha, composited with `Src`, holding one
            // paint of transparent black -- also `Src`, so the group ends up
            // transparent everywhere. Compositing it replaces what the bounds
            // cover, so the plate is a hole punched in the ground.
            //
            // Upstream names this for an optimization that turns a group whose
            // first draw covers it into the pass's clear color, and its comment
            // says what a failure looks like: the target is composited without
            // having been written, so what shows is whatever the allocation
            // held -- on a real device, often NaNs, which upstream reports as a
            // magenta square. The half alpha and the `Src` are upstream's too,
            // and they are there so nothing mistakes this for the opacity
            // peephole instead.
            //
            // The second group is upstream's as well: empty, unbounded, and
            // composited with `DstOver`, so it must change nothing.
            vec![
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        alpha: 0.5,
                        blend: BlendMode::Src,
                        ..LayerSpec::default()
                    }),
                    bounds: Some([0.0, 0.0, 64.0, 64.0]),
                    transform: Transform::default(),
                    children: vec![Node::Paint(Box::new(PaintSpec {
                        fill: Fill::Solid([0.0, 0.0, 0.0, 0.0]),
                        blend: BlendMode::Src,
                        clip: None,
                        clip_out: None,
                        transform: Transform::default(),
                    }))],
                },
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        blend: BlendMode::DstOver,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: Vec::new(),
                },
            ],
        ),
        plate_tree(
            "basic/clear-color-optimization-when-subpass-is-bigger-than-parent-pass",
            // The same optimization asked the harder question: a group that is
            // larger than the pass it composites into. Its matrix doubles it on
            // the way back, so what it captures reaches twice as far as the
            // frame, and the rectangle inside it is drawn twice that again --
            // deliberately, to make the group's own extent the larger number.
            //
            // The plate is uniform, and that is the assertion. A group sized
            // from the parent rather than from its contents, or one composited
            // at the wrong offset, shows the ground somewhere along an edge.
            vec![
                Node::Draw(Box::new(Item::fill(
                    Shape::Rect {
                        min: [64.0, 64.0],
                        max: [96.0, 96.0],
                    },
                    RED,
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        matrix: Some(Transform {
                            scale: [2.0, 2.0],
                            ..Transform::default()
                        }),
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![
                        Node::Draw(Box::new(Item::fill(
                            Shape::Rect {
                                min: [0.0, 0.0],
                                max: [128.0, 128.0],
                            },
                            GREEN,
                        ))),
                        Node::Draw(Box::new(Item::fill(
                            Shape::Rect {
                                min: [0.0, 0.0],
                                max: [256.0, 256.0],
                            },
                            RED,
                        ))),
                    ],
                },
            ],
        ),
        plate_tree(
            "basic/save-layer-filters-scale-with-transform",
            // The same layer twice, once at unit scale and once at three
            // times it, each blurring what it captured. Upstream draws it to
            // say that a filter on a save layer is stated in the space of the
            // caller rather than in device pixels, so the second copy's blur
            // is three times as wide on screen as the first's.
            //
            // Which is the decision recorded in `docs/architecture.md`: a blur
            // sigma is local, and the conversion to device happens where the
            // layer opens. A renderer that took the sigma as already-device
            // would draw both copies with the same soft edge, and the two
            // panels here would then differ only in size.
            vec![
                Node::Layer {
                    layer: Box::new(LayerSpec::default().with_blur(2.0)),
                    bounds: None,
                    transform: Transform::translate(6.0, 6.0),
                    children: vec![Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [0.0, 0.0],
                            max: [20.0, 20.0],
                        },
                        WHITE,
                    )))],
                },
                Node::Layer {
                    layer: Box::new(LayerSpec::default().with_blur(2.0)),
                    bounds: None,
                    transform: Transform {
                        scale: [3.0, 3.0],
                        translate: [44.0, 44.0],
                        ..Transform::default()
                    },
                    children: vec![Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [0.0, 0.0],
                            max: [20.0, 20.0],
                        },
                        WHITE,
                    )))],
                },
            ],
        ),
        plate(
            "basic/can-render-wide-stroked-rect-without-overlap",
            wide_stroked_rects(false),
        ),
        plate(
            "basic/can-render-wide-stroked-rect-path-without-overlap",
            wide_stroked_rects(true),
        ),
        plate(
            "basic/stroked-rects-render-correctly",
            vec![
                Item::stroke(
                    Shape::Rect {
                        min: [12.0, 12.0],
                        max: [60.0, 60.0],
                    },
                    StrokeSpec::new(4.0),
                    RED,
                ),
                Item::stroke(
                    Shape::Rect {
                        min: [68.0, 12.0],
                        max: [116.0, 60.0],
                    },
                    StrokeSpec::new(12.0),
                    GREEN,
                ),
                Item::stroke(
                    Shape::Rect {
                        min: [12.0, 68.0],
                        max: [60.0, 116.0],
                    },
                    StrokeSpec::new(1.0),
                    BLUE,
                ),
                Item::stroke(
                    Shape::Rect {
                        min: [68.0, 68.0],
                        max: [116.0, 116.0],
                    },
                    StrokeSpec {
                        join: LineJoin::Round,
                        ..StrokeSpec::new(10.0)
                    },
                    YELLOW,
                ),
            ],
        ),
        plate(
            "basic/filled-circles-render-correctly",
            vec![
                Item::fill(
                    Shape::Circle {
                        center: [40.0, 40.0],
                        radius: 28.0,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [88.0, 40.0],
                        radius: 16.0,
                    },
                    GREEN,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [40.0, 92.0],
                        radius: 8.0,
                    },
                    BLUE,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [92.0, 92.0],
                        radius: 2.0,
                    },
                    YELLOW,
                ),
            ],
        ),
        plate(
            "basic/draw-thin-stroked-circle",
            vec![Item::stroke(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 48.0,
                },
                StrokeSpec::new(0.5),
                WHITE,
            )],
        ),
        plate(
            "basic/stroked-circles-render-correctly",
            vec![
                Item::stroke(
                    Shape::Circle {
                        center: [42.0, 42.0],
                        radius: 26.0,
                    },
                    StrokeSpec::new(2.0),
                    RED,
                ),
                Item::stroke(
                    Shape::Circle {
                        center: [90.0, 42.0],
                        radius: 22.0,
                    },
                    StrokeSpec::new(10.0),
                    GREEN,
                ),
                // Wider than the radius, so the band closes over the center --
                // the case a stroke of a small circle has to get right.
                Item::stroke(
                    Shape::Circle {
                        center: [64.0, 96.0],
                        radius: 10.0,
                    },
                    StrokeSpec::new(24.0),
                    BLUE,
                ),
            ],
        ),
        plate(
            "basic/filled-ellipses-render-correctly",
            vec![
                Item::fill(
                    Shape::Oval {
                        min: [8.0, 24.0],
                        max: [120.0, 56.0],
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Oval {
                        min: [48.0, 68.0],
                        max: [80.0, 120.0],
                    },
                    BLUE,
                ),
            ],
        ),
        // Open, which is what `use_center = false` means and what this plate is
        // named for. It drew closed slices for a while, under this name, with
        // the with-center plate below not existing at all -- so the pair that
        // upstream splits into two farms was one plate showing the wrong half
        // of it. A filled open arc is closed by the chord between its ends
        // rather than through the center, and that is the whole difference.
        plate(
            "basic/filled-arcs-render-correctly",
            vec![
                Item::fill(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [52.0, 52.0],
                        start: -1.2,
                        sweep: 2.4,
                        through_center: false,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [34.0, 34.0],
                        start: 1.6,
                        sweep: 2.0,
                        through_center: false,
                    },
                    GREEN,
                ),
            ],
        ),
        // The same two through the center, which is the other farm. A sweep
        // under half a turn is where the two differ most: the chord cuts a
        // segment off where the center makes a wedge.
        plate(
            "basic/filled-arcs-render-correctly-with-center",
            vec![
                Item::fill(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [52.0, 52.0],
                        start: -1.2,
                        sweep: 2.4,
                        through_center: true,
                    },
                    RED,
                ),
                Item::fill(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [34.0, 34.0],
                        start: 1.6,
                        sweep: 2.0,
                        through_center: true,
                    },
                    GREEN,
                ),
            ],
        ),
        plate(
            "basic/non-square-filled-arcs-render-correctly",
            vec![Item::fill(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [56.0, 30.0],
                    start: -0.6,
                    sweep: 4.0,
                    through_center: false,
                },
                YELLOW,
            )],
        ),
        // On an ellipse the center matters more than it does on a circle: the
        // chord between two points of an ellipse is not perpendicular to
        // anything in particular, so the segment it cuts off and the wedge the
        // center makes are different shapes rather than the same shape at two
        // sizes.
        plate(
            "basic/non-square-filled-arcs-render-correctly-with-center",
            vec![Item::fill(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [56.0, 30.0],
                    start: -0.6,
                    sweep: 4.0,
                    through_center: true,
                },
                YELLOW,
            )],
        ),
        plate(
            "basic/stroked-arcs-render-correctly-with-round-ends",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [46.0, 46.0],
                    start: -2.2,
                    sweep: 3.6,
                    through_center: false,
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(14.0)
                },
                WHITE,
            )],
        ),
        plate(
            "basic/stroked-arcs-render-correctly-with-butt-ends",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [46.0, 46.0],
                    start: -2.2,
                    sweep: 3.6,
                    through_center: false,
                },
                StrokeSpec::new(14.0),
                WHITE,
            )],
        ),
        plate(
            "basic/stroked-arcs-render-correctly-with-square-ends",
            // The third cap, and the one an arc shows differently from a line:
            // a square end projects along the tangent, so on a curve it leaves
            // the circle rather than continuing it, and the two ends project in
            // directions that are not parallel.
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [46.0, 46.0],
                    start: -2.2,
                    sweep: 3.6,
                    through_center: false,
                },
                StrokeSpec {
                    cap: LineCap::Square,
                    ..StrokeSpec::new(14.0)
                },
                WHITE,
            )],
        ),
        // A stroked slice, which is where a *join* shows on an arc rather than
        // a cap: through the center the outline has a vertex, and the two
        // straight edges meet there at whatever angle the sweep left. A miter
        // runs the edges out to their crossing; a round join arcs between them.
        // On an open arc neither is reachable, which is why the pair above says
        // nothing about joins.
        //
        // A narrow sweep, and that is the difference between this pair meaning
        // something and not. At a hundred and thirty-seven degrees the vertex
        // is obtuse enough that a miter and a round join agree to within a
        // fifteenth of a per cent of the frame -- twenty-five pixels, which is
        // a pair of plates that would look identical to anyone comparing them.
        // At forty the miter runs out to a point and the two are three times as
        // far apart -- seventy-eight pixels against twenty-five, which is
        // still a small plate and is now a plate about something.
        plate(
            "basic/stroked-arcs-render-correctly-with-miter-joins-and-center",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [42.0, 42.0],
                    start: -1.2,
                    sweep: 0.7,
                    through_center: true,
                },
                StrokeSpec {
                    join: LineJoin::Miter,
                    ..StrokeSpec::new(10.0)
                },
                WHITE,
            )],
        ),
        plate(
            "basic/stroked-arcs-render-correctly-with-round-joins-and-center",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [42.0, 42.0],
                    start: -1.2,
                    sweep: 0.7,
                    through_center: true,
                },
                StrokeSpec {
                    join: LineJoin::Round,
                    ..StrokeSpec::new(10.0)
                },
                WHITE,
            )],
        ),
        // The third join, and the one the pair above cannot stand in for. A
        // miter runs the two edges out to their crossing and a round join arcs
        // between them; a bevel cuts straight across, which at this sweep sits
        // between the two rather than near either. The same narrow sweep, for
        // the reason stated above it -- at an obtuse vertex all three agree.
        plate(
            "basic/stroked-arcs-render-correctly-with-bevel-joins-and-center",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [42.0, 42.0],
                    start: -1.2,
                    sweep: 0.7,
                    through_center: true,
                },
                StrokeSpec {
                    join: LineJoin::Bevel,
                    ..StrokeSpec::new(10.0)
                },
                WHITE,
            )],
        ),
        // Upstream draws its whole farm once per cap and reads the two against
        // each other; this draws one arc twice, so the difference is where the
        // ends are rather than which plate is being looked at. A square end
        // projects half the stroke width along the tangent and a butt end stops
        // on it, so the red shows past the blue at both ends and nowhere else.
        plate(
            "basic/stroked-arcs-render-correctly-with-square-and-butt-ends",
            vec![
                Item::stroke(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [44.0, 44.0],
                        start: -2.2,
                        sweep: 3.6,
                        through_center: false,
                    },
                    StrokeSpec {
                        cap: LineCap::Square,
                        ..StrokeSpec::new(16.0)
                    },
                    RED,
                ),
                Item::stroke(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [44.0, 44.0],
                        start: -2.2,
                        sweep: 3.6,
                        through_center: false,
                    },
                    StrokeSpec {
                        cap: LineCap::Butt,
                        ..StrokeSpec::new(16.0)
                    },
                    BLUE,
                ),
            ],
        ),
        // And all three, which is upstream's third cap plate. A round end is a
        // half disc and a square end is a half square, so the green shows in
        // the corners the round cap leaves and the red shows past both.
        plate(
            "basic/stroked-arcs-render-correctly-with-square-and-butt-and-round-ends",
            vec![
                Item::stroke(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [44.0, 44.0],
                        start: -2.2,
                        sweep: 3.6,
                        through_center: false,
                    },
                    StrokeSpec {
                        cap: LineCap::Square,
                        ..StrokeSpec::new(16.0)
                    },
                    RED,
                ),
                Item::stroke(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [44.0, 44.0],
                        start: -2.2,
                        sweep: 3.6,
                        through_center: false,
                    },
                    StrokeSpec {
                        cap: LineCap::Round,
                        ..StrokeSpec::new(16.0)
                    },
                    GREEN,
                ),
                Item::stroke(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [44.0, 44.0],
                        start: -2.2,
                        sweep: 3.6,
                        through_center: false,
                    },
                    StrokeSpec {
                        cap: LineCap::Butt,
                        ..StrokeSpec::new(16.0)
                    },
                    BLUE,
                ),
            ],
        ),
        // The pair upstream draws translucently, and translucency is the whole
        // of what they are for. A stroke is tessellated as a run of overlapping
        // quads with a cap on each end, so anywhere the outline covers a pixel
        // twice an opaque stroke looks perfect and a half-transparent one comes
        // out darker. This arc closes to within twenty degrees and its stroke
        // is wider than its diameter, so the two caps sit on top of each other
        // and that patch is visibly darker -- correctly, since it is two shapes
        // over one pixel.
        //
        // What must *not* darken is the run between them, and
        // `a_translucent_stroke_blends_with_itself_only_where_its_caps_overlap`
        // is where that is asserted rather than looked at.
        plate(
            "basic/stroked-arcs-render-correctly-with-translucency-and-round-ends",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [26.0, 26.0],
                    start: 0.0,
                    sweep: 5.93,
                    through_center: false,
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(40.0)
                },
                BLUE_HALF,
            )],
        ),
        plate(
            "basic/stroked-arcs-render-correctly-with-translucency-and-square-ends",
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [26.0, 26.0],
                    start: 0.0,
                    sweep: 5.93,
                    through_center: false,
                },
                StrokeSpec {
                    cap: LineCap::Square,
                    ..StrokeSpec::new(40.0)
                },
                BLUE_HALF,
            )],
        ),
        plate(
            "basic/stroked-arcs-cover-full-arc-with-butt-ends",
            // A whole turn, where the two butt ends meet each other rather than
            // ending in air. The picture that says it went wrong is a seam: a
            // sweep flattened to slightly less than a turn leaves a gap, and one
            // slightly more leaves the ends overlapping where the stroke is
            // translucent. Opaque here, so a gap is the visible failure.
            vec![Item::stroke(
                Shape::Arc {
                    center: [64.0, 64.0],
                    radii: [44.0, 44.0],
                    start: 0.0,
                    sweep: std::f32::consts::TAU,
                    through_center: false,
                },
                StrokeSpec::new(16.0),
                WHITE,
            )],
        ),
        plate(
            "basic/translucent-filled-arcs-render-correctly",
            vec![
                Item::fill(
                    Shape::Arc {
                        center: [56.0, 64.0],
                        radii: [44.0, 44.0],
                        start: -1.0,
                        sweep: 2.6,
                        through_center: true,
                    },
                    [1.0, 0.2, 0.2, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::Arc {
                        center: [76.0, 64.0],
                        radii: [44.0, 44.0],
                        start: 1.4,
                        sweep: 2.6,
                        through_center: true,
                    },
                    [0.2, 0.6, 1.0, 0.5],
                )
                .with_blend(BlendMode::SrcOver),
            ],
        ),
        plate(
            "basic/filled-round-rects-render-correctly",
            filled_round_rects(false),
        ),
        plate(
            "basic/filled-round-rect-paths-render-correctly",
            filled_round_rects(true),
        ),
        plate(
            "basic/can-render-rounded-rect-with-uniform-radii",
            vec![Item::stroke(
                Shape::RoundedRect {
                    min: [20.0, 32.0],
                    max: [108.0, 96.0],
                    radius: 16.0,
                },
                StrokeSpec::new(6.0),
                WHITE,
            )],
        ),
        plate(
            "basic/can-render-different-shapes-with-same-color-source",
            vec![
                Item::filled(
                    Shape::Circle {
                        center: [40.0, 40.0],
                        radius: 28.0,
                    },
                    ramp(),
                ),
                Item::filled(
                    Shape::RoundedRect {
                        min: [70.0, 12.0],
                        max: [120.0, 68.0],
                        radius: 12.0,
                    },
                    ramp(),
                ),
                Item::filled(
                    Shape::Polygon(vec![[20.0, 118.0], [64.0, 76.0], [108.0, 118.0]]),
                    ramp(),
                ),
            ],
        ),
        plate(
            "basic/compare-anti-alias-and-non-anti-alias",
            vec![
                Item::fill(
                    Shape::Circle {
                        center: [40.0, 64.0],
                        radius: 30.0,
                    },
                    WHITE,
                ),
                Item::fill(
                    Shape::Circle {
                        center: [92.0, 64.0],
                        radius: 30.0,
                    },
                    WHITE,
                ),
            ],
        )
        .with_samples(1),
        plate(
            "basic/can-render-lines-with-caps-angles-and-alphas",
            (0..6)
                .map(|i| {
                    let t = i as f32 / 5.0;
                    let angle = t * std::f32::consts::PI;
                    let (dx, dy) = (angle.cos() * 44.0, angle.sin() * 44.0);
                    Item::stroke(
                        Shape::Polyline(vec![[64.0 - dx, 64.0 - dy], [64.0 + dx, 64.0 + dy]]),
                        StrokeSpec {
                            cap: match i % 3 {
                                0 => LineCap::Butt,
                                1 => LineCap::Round,
                                _ => LineCap::Square,
                            },
                            ..StrokeSpec::new(9.0)
                        },
                        [1.0, 1.0, 1.0, 0.3 + 0.7 * t],
                    )
                    .with_blend(BlendMode::SrcOver)
                })
                .collect(),
        ),
    ]
}

/// A gradient several scenes fill different shapes with, to show one color
/// source spanning shapes that have nothing else in common.
fn ramp() -> Fill {
    Fill::LinearGradient {
        start: [8.0, 8.0],
        end: [120.0, 120.0],
        stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
        tile: TileMode::Clamp,
    }
}

/// `aiks_dl_primitive_shape_unittests.cc` -- a hairline under a transform.
///
/// Every scene in that file is the same three draws under a different
/// transform, which is what its own comment says it is for: "the effects of
/// scaling, rotation and skew transforms on the consistency of a stroke
/// (particularly hairlines)". A filled shape, a darker filled shape inside it,
/// and the inner shape's outline stroked at a width of *zero* -- upstream's
/// `RenderParameters::stroke_width` defaults to `0.0f` and every one of the
/// three passes it through untouched.
///
/// So the chapter exists at all only because a stroke width of zero means a
/// hairline here now. It could not have been written before that, and the
/// inventory said so.
///
/// The shapes are centered on the origin and translated into the frame, rather
/// than centered in the frame and transformed about their middle the way
/// upstream does it. The two are the same picture and the second cannot be said
/// here: a `Transform` composes scale, then shear, then rotation, then
/// translation, so a pivot has to be folded into the translation, and folding
/// it is arithmetic in the plate rather than in the renderer. Upstream also
/// skews before it scales where this scales before it skews, which is visible
/// only in the one scene that does both.
fn primitive_shape() -> Vec<Scene> {
    // A hairline is one device pixel whatever the transform does, so the thing
    // each of these shows is that the outline stays the same weight as the
    // shape under it is stretched and turned. Radii a little under half the
    // plate, so the skew has room to carry the shape without clipping it.
    let plate = |name: &'static str, circle: bool, transform: Transform| {
        let fill = 40.0f32;
        let inner = 34.0f32;
        // Upstream elongates the rectangle past the square the circle would
        // fill, by a fifth of the radius at this scale.
        let wide = |r: f32| match circle {
            true => r,
            false => r + 8.0,
        };
        let shape = move |r: f32| match circle {
            true => Shape::Circle {
                center: [0.0, 0.0],
                radius: r,
            },
            false => Shape::Rect {
                min: [-wide(r), -r],
                max: [wide(r), r],
            },
        };
        Scene::tree(
            name,
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([0.0, 0.0, 0.0, 1.0]),
                    blend: BlendMode::Src,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Draw(Box::new(
                    Item::fill(shape(fill), [0.1, 0.3, 0.9, 1.0]).with_transform(transform),
                )),
                Node::Draw(Box::new(
                    Item::fill(shape(inner), [0.0, 0.0, 0.0, 0.5]).with_transform(transform),
                )),
                // The hairline. Stated as a stroke of no width, which is the
                // whole subject: `dart:ui` documents that as the thinnest line
                // the device can draw and upstream widens it to one pixel at
                // full coverage.
                Node::Draw(Box::new(
                    Item::stroke(shape(inner), StrokeSpec::new(0.0), WHITE)
                        .with_transform(transform),
                )),
            ],
        )
    };
    vec![
        plate(
            "primitive/can-render-skewed-circle-hairline",
            true,
            Transform {
                skew: [0.75, 0.75],
                translate: [64.0, 64.0],
                ..Transform::default()
            },
        ),
        plate(
            "primitive/can-render-skewed-rect-hairline",
            false,
            Transform {
                skew: [0.75, 0.5],
                translate: [64.0, 64.0],
                ..Transform::default()
            },
        ),
        // The one upstream added from a review comment, and the one with an edge
        // near vertical: a scale, a shear and a turn together, which is where an
        // estimate of the pixel size taken on the wrong axis shows.
        plate(
            "primitive/can-render-transformed-rect-with-near-vertical-edge-hairline",
            false,
            Transform {
                scale: [1.552, 1.0],
                skew: [0.458, 0.0],
                rotate: 33.75_f32.to_radians(),
                translate: [64.0, 64.0],
                ..Transform::default()
            },
        ),
    ]
}

/// `aiks_dl_path_unittests.cc` -- curves, strokes and contours.
fn path() -> Vec<Scene> {
    vec![
        plate_tree(
            "path/draw-lines-with-draw-line",
            draw_lines_grid(LineForm::Line),
        ),
        plate_tree("path/draw-lines-with-path", draw_lines_grid(LineForm::Path)),
        plate_tree(
            "path/draw-lines-with-filled-rects",
            draw_lines_grid(LineForm::Rect),
        ),
        plate_tree(
            "path/draw-lines-with-filled-round-rects",
            draw_lines_grid(LineForm::RoundRect),
        ),
        plate(
            "path/can-render-strokes",
            // A single thick segment, said as a path. Upstream's whole scene,
            // and the plainest stroke in the file: two ends, no join, and a
            // width wide enough that the caps are a sixth of the picture.
            vec![Item::stroke(
                Shape::Polyline(vec![[16.0, 64.0], [112.0, 64.0]]),
                StrokeSpec::new(16.0),
                RED,
            )],
        ),
        plate(
            "path/draw-rect-strokes-render-correctly",
            // A rectangle stroked through its path rather than through
            // `draw_rect`, which is where the corner is a join the tessellator
            // has to build rather than a field the shader evaluates.
            vec![Item::stroke(
                Shape::Rect {
                    min: [24.0, 24.0],
                    max: [104.0, 104.0],
                },
                StrokeSpec::new(10.0),
                RED,
            )
            .as_path()],
        ),
        plate(
            "path/draw-rect-strokes-with-bevel-join-render-correctly",
            // The same rectangle with the corner cut off instead of carried to
            // a point. Upstream keeps the pair, and the difference is four
            // small triangles -- which is exactly the size of difference a
            // join that fell back to the default would hide.
            vec![Item::stroke(
                Shape::Rect {
                    min: [24.0, 24.0],
                    max: [104.0, 104.0],
                },
                StrokeSpec {
                    join: LineJoin::Bevel,
                    ..StrokeSpec::new(10.0)
                },
                RED,
            )
            .as_path()],
        ),
        plate_tree(
            "path/fat-stroke-arc",
            // An arc stroked wider than the shape it is inscribed in, with the
            // shape drawn under it and a line at the frontier its outer edge
            // must reach and not pass: the rectangle's right side plus half the
            // stroke. Upstream draws that line for a reader to check by eye,
            // and it is checked here instead.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([
                        0x11 as f32 / 255.0,
                        0x11 as f32 / 255.0,
                        0x11 as f32 / 255.0,
                        1.0,
                    ]),
                    blend: BlendMode::Src,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Draw(Box::new(Item::fill(
                    Shape::Rect {
                        min: [20.0, 20.0],
                        max: [60.0, 60.0],
                    },
                    RED,
                ))),
                Node::Draw(Box::new(
                    Item::stroke(
                        Shape::Arc {
                            center: [40.0, 40.0],
                            radii: [20.0, 20.0],
                            start: 0.0,
                            sweep: std::f32::consts::FRAC_PI_2,
                            through_center: false,
                        },
                        StrokeSpec::new(48.0),
                        WHITE,
                    )
                    .with_blend(BlendMode::SrcOver),
                )),
                // The frontier: sixty plus half of forty-eight.
                Node::Draw(Box::new(
                    Item::stroke(
                        Shape::Line {
                            from: [84.0, 0.0],
                            to: [84.0, 100.0],
                        },
                        StrokeSpec::new(1.0),
                        RED,
                    )
                    .with_blend(BlendMode::SrcOver),
                )),
            ],
        ),
        plate(
            "path/blurred-circle-with-stroke-width",
            // A stroked circle under a mask blur. The two halves of it are
            // routed separately here -- a circle is a field the shader
            // evaluates, and a blurred one is a different field again -- so a
            // stroked circle that is also blurred is the case where a renderer
            // has to decide which of the two it is, and the answer is neither:
            // it is a ring, blurred.
            vec![Item::stroke(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 32.0,
                },
                StrokeSpec::new(10.0),
                GREEN,
            )
            .with_mask_blur(3.0)
            .with_blend(BlendMode::SrcOver)],
        ),
        plate(
            "path/rotate-color-filtered-path",
            // An arrow in two contours, stroked, recolored by a filter that
            // keeps only the filter's own color where the stroke covered, and
            // the whole thing turned a quarter turn. Upstream's point is the
            // order: the filter applies in the shape's own space and the
            // rotation applies to the result, so a renderer that filtered after
            // transforming would still draw an arrow and still draw it the
            // right color.
            //
            // What it can catch is a filter dropped on a transformed draw, and
            // the color chosen makes that loud -- the paint underneath is
            // black, so losing the filter loses the arrow into the ground.
            [
                vec![[60.0, 95.0], [60.0, 25.0]],
                vec![[25.0, 60.0], [60.0, 95.0], [95.0, 60.0]],
            ]
            .into_iter()
            .map(|points| {
                Item::stroke(
                    Shape::Polyline(points),
                    StrokeSpec {
                        cap: LineCap::Round,
                        join: LineJoin::Round,
                        ..StrokeSpec::new(8.0)
                    },
                    BLACK,
                )
                .with_color_filter(
                    // Alice blue, which is upstream's, kept only where the
                    // stroke put coverage.
                    ColorFilter::blend([240.0 / 255.0, 248.0 / 255.0, 1.0, 1.0], BlendMode::SrcIn),
                )
                .with_transform(Transform {
                    rotate: std::f32::consts::FRAC_PI_2,
                    translate: [120.0, 16.0],
                    ..Transform::default()
                })
                .with_blend(BlendMode::SrcOver)
            })
            .collect(),
        ),
        plate(
            "path/can-render-clips",
            // The clip cuts exactly through the circle's center, as upstream's
            // does, so what is left is one quarter and two straight edges
            // meeting at the middle of what was a curve. Clipped by a shape
            // rather than by a rectangle field, which is the stencil route:
            // upstream says it with `ClipPath`, and a scissor could express
            // this one and would be exercising something else.
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 44.0,
                },
                [1.0, 0.0, 1.0, 1.0],
            )
            .with_clip_shape(Shape::Rect {
                min: [0.0, 0.0],
                max: [64.0, 64.0],
            })],
        ),
        plate(
            "path/two-contour-path-with-single-point-contour",
            // Two contours in one path: a segment, and a contour holding a
            // single point. The second has no direction, so nothing but the
            // cap gives it a shape -- with a round cap it is a dot of the
            // stroke's width, and with any other it is nothing at all.
            //
            // What it is really asking is whether the two contours stayed
            // apart. A renderer that ran them together would draw a line from
            // the end of the first to the point, which is a diagonal across
            // half the plate and impossible to miss.
            vec![Item::stroke(
                Shape::Contours(vec![
                    vec![[24.0, 24.0], [56.0, 56.0]],
                    vec![[96.0, 96.0], [96.0, 96.0]],
                ]),
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(12.0)
                },
                RED,
            )],
        ),
        plate_tree(
            "path/two-contour-path-with-connecting-lines",
            // Two contours that meet at a point, drawn three times for the
            // three joins. The join is the thing being watched and it should
            // never appear: the contours end at the same place but they are
            // separate, so what belongs there is two caps, and a mitered spike
            // at the apex would say the renderer joined them.
            [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel]
                .into_iter()
                .enumerate()
                .map(|(i, join)| {
                    let y = i as f32 * 40.0 + 12.0;
                    Node::Draw(Box::new(Item::stroke(
                        Shape::Contours(vec![
                            vec![[24.0, y], [48.0, y + 24.0]],
                            vec![[48.0, y + 24.0], [72.0, y]],
                        ]),
                        StrokeSpec {
                            join,
                            ..StrokeSpec::new(8.0)
                        },
                        RED,
                    )))
                })
                .collect(),
        ),
        plate(
            "path/can-render-quadratic-stroke-with-instant-turn",
            // A quadratic whose ends are the same point: it goes out to the
            // control point and comes straight back, so the stroke is a pill
            // laid along the diagonal. Flat at either end means the turn was
            // treated as the curve ending rather than as the curve reversing.
            vec![Item::stroke(
                Shape::Conic {
                    start: [96.0, 96.0],
                    ctrl: [32.0, 32.0],
                    end: [96.0, 96.0],
                    weight: 1.0,
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(24.0)
                },
                RED,
            )],
        ),
        plate(
            "path/can-render-stroke-path-with-cubic-line",
            // A cubic whose control points lie outside the band its ends
            // define, so the curve crosses its own chord twice and the stroke
            // has to widen around three inflections rather than one.
            vec![Item::stroke(
                Shape::Cubic {
                    start: [8.0, 64.0],
                    c0: [24.0, 120.0],
                    c1: [104.0, 8.0],
                    end: [120.0, 64.0],
                },
                StrokeSpec::new(8.0),
                RED,
            )],
        ),
        plate(
            "path/can-draw-an-open-path-that-isnt-a-rect",
            // Four points closed into a quadrilateral that is nothing like a
            // rectangle. Upstream draws it to check that closing a path does
            // not send it down whatever fast route a rectangle gets, and the
            // shape is chosen so that route would be obvious: its bounding box
            // is half again the area of the shape itself.
            vec![Item::stroke(
                Shape::Polygon(vec![
                    [12.0, 12.0],
                    [120.0, 28.0],
                    [70.0, 72.0],
                    [24.0, 12.0],
                ]),
                StrokeSpec::new(6.0),
                RED,
            )],
        ),
        plate(
            "path/can-render-curved-strokes",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [10.0, 100.0],
                    c0: [30.0, 10.0],
                    c1: [98.0, 10.0],
                    end: [118.0, 100.0],
                },
                StrokeSpec::new(6.0),
                WHITE,
            )],
        ),
        plate(
            "path/a-curve-under-perspective",
            // The chapter perspective alone was holding up. A curve rather than
            // a rectangle because the curve is what can go wrong: flattening
            // happens before the transform, so a tolerance taken from one
            // number for the whole shape leaves the magnified end showing the
            // polygon it was flattened into. The near half of this stroke is
            // where that would appear.
            vec![Item::stroke(
                Shape::Cubic {
                    start: [10.0, 100.0],
                    c0: [30.0, 10.0],
                    c1: [98.0, 10.0],
                    end: [118.0, 100.0],
                },
                StrokeSpec::new(6.0),
                WHITE,
            )
            .with_transform(Transform {
                perspective: [0.004, 0.0],
                ..Transform::default()
            })],
        ),
        plate(
            "path/can-render-thick-curved-strokes",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [10.0, 100.0],
                    c0: [30.0, 10.0],
                    c1: [98.0, 10.0],
                    end: [118.0, 100.0],
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(28.0)
                },
                BLUE,
            )],
        ),
        plate(
            "path/can-render-thin-curved-strokes",
            vec![Item::stroke(
                Shape::Cubic {
                    start: [10.0, 100.0],
                    c0: [30.0, 10.0],
                    c1: [98.0, 10.0],
                    end: [118.0, 100.0],
                },
                StrokeSpec::new(0.6),
                WHITE,
            )],
        ),
        plate(
            "path/can-render-stroke-path-that-ends-at-sharp-turn",
            vec![Item::stroke(
                Shape::Polyline(vec![[20.0, 108.0], [64.0, 20.0], [66.0, 108.0]]),
                StrokeSpec {
                    join: LineJoin::Miter,
                    ..StrokeSpec::new(12.0)
                },
                WHITE,
            )],
        ),
        plate(
            "path/can-draw-an-open-path",
            vec![Item::stroke(
                Shape::Polyline(vec![
                    [16.0, 96.0],
                    [48.0, 32.0],
                    [80.0, 96.0],
                    [112.0, 32.0],
                ]),
                StrokeSpec {
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    ..StrokeSpec::new(10.0)
                },
                GREEN,
            )],
        ),
        plate(
            "path/can-render-difference-paths",
            vec![Item::filled(
                Shape::RuledPolygon {
                    // Two concentric rings wound the same way, so the even-odd
                    // rule leaves the middle empty and a nonzero fill would
                    // not.
                    points: ring(48.0).into_iter().chain(ring(26.0)).collect(),
                    rule: FillRule::EvenOdd,
                },
                Fill::Solid(YELLOW),
            )],
        ),
        plate(
            "path/stroke-with-a-zero-length-segment",
            // A point repeated in the middle of a run. The segment between the
            // two copies has no direction, so anything deriving a normal from
            // it divides by its length -- and the failure is not a wrong
            // picture but a hole, a spike, or nothing at all. Impeller's own
            // path suite carries this case for the same reason.
            vec![Item::stroke(
                Shape::Polyline(vec![
                    [16.0, 40.0],
                    [56.0, 40.0],
                    [56.0, 40.0],
                    [112.0, 40.0],
                ]),
                StrokeSpec {
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    ..StrokeSpec::new(14.0)
                },
                RED,
            )],
        ),
        plate(
            "path/stroke-that-doubles-back-on-itself",
            // An instant turn: the run goes out and returns along the same
            // line, so the join between the two segments is a full reversal.
            // A miter there is infinitely long and has to fall back, which is
            // the one case the miter limit exists for and the one a gentle
            // corner cannot reach.
            vec![
                Item::stroke(
                    Shape::Polyline(vec![[24.0, 36.0], [104.0, 36.0], [24.0, 36.0]]),
                    StrokeSpec {
                        cap: LineCap::Butt,
                        join: LineJoin::Miter,
                        ..StrokeSpec::new(16.0)
                    },
                    GREEN,
                ),
                Item::stroke(
                    Shape::Polyline(vec![[24.0, 84.0], [104.0, 84.0], [24.0, 84.0]]),
                    StrokeSpec {
                        cap: LineCap::Round,
                        join: LineJoin::Round,
                        ..StrokeSpec::new(16.0)
                    },
                    BLUE,
                ),
            ],
        ),
        plate(
            "path/arcs-of-degenerate-sweep",
            // Three sweeps that are not a normal arc: none at all, a full turn,
            // and more than a full turn. A zero sweep is a point and must draw
            // either nothing or a cap, never a whole ring; a sweep past two pi
            // must not wind twice and cancel itself under a nonzero fill.
            vec![
                Item::stroke(
                    Shape::Arc {
                        center: [32.0, 64.0],
                        radii: [22.0, 22.0],
                        start: 0.0,
                        sweep: 0.0,
                        through_center: false,
                    },
                    StrokeSpec {
                        cap: LineCap::Round,
                        ..StrokeSpec::new(8.0)
                    },
                    YELLOW,
                ),
                Item::stroke(
                    Shape::Arc {
                        center: [64.0, 64.0],
                        radii: [22.0, 22.0],
                        start: 0.0,
                        sweep: std::f32::consts::TAU,
                        through_center: false,
                    },
                    StrokeSpec::new(6.0),
                    GREEN,
                ),
                Item::filled(
                    Shape::Arc {
                        center: [100.0, 64.0],
                        radii: [22.0, 22.0],
                        start: 0.4,
                        sweep: std::f32::consts::TAU * 1.35,
                        through_center: true,
                    },
                    Fill::Solid(BLUE),
                ),
            ],
        ),
        plate(
            "path/circles-from-subpixel-to-large",
            // A radius sweep across the scale where coverage is decided one
            // sample at a time rather than by area. At four samples a circle of
            // radius a third draws nothing, half a pixel lights one sample, and
            // it is not until about one and a half that any pixel comes out
            // solid. That staircase is not a fault -- it is what sample-based
            // coverage is -- but every step of it is a place two rasterizers
            // could put their samples differently and disagree, which is what
            // this plate is for. The smallest circle here is below the first
            // step, and the backends agreeing that it draws nothing at all is
            // as much a comparison as the ones that do.
            (0..6)
                .map(|i| {
                    let radius = 0.4 * (2.6f32).powi(i);
                    Item::filled(
                        Shape::Circle {
                            center: [14.0 + i as f32 * 20.0, 64.0],
                            radius,
                        },
                        Fill::Solid(WHITE),
                    )
                })
                .collect(),
        ),
        plate(
            "path/rounded-rects-from-square-to-stadium",
            // A corner radius taken past half the shorter side, where it is
            // clamped. Unclamped the corners would cross and the outline would
            // fold through itself, which fills as a bow tie under a nonzero
            // rule and as a hole under an even-odd one -- two wrong pictures
            // rather than one, and neither an error.
            (0..4)
                .map(|i| {
                    let top = 8.0 + i as f32 * 30.0;
                    Item::filled(
                        Shape::RoundedRect {
                            min: [16.0, top],
                            max: [112.0, top + 24.0],
                            radius: [0.0, 4.0, 12.0, 40.0][i],
                        },
                        Fill::Solid(BLUE),
                    )
                })
                .collect(),
        ),
        plate(
            "path/rings-between-two-rounded-rectangles",
            // `drawDRRect`, at four ratios of hole to shape. What is being
            // drawn is one path of two contours under the even-odd rule, so a
            // rule applied per contour rather than per path would fill every
            // one of these solid and the plate would be four rectangles.
            (0..4)
                .map(|i| {
                    let left = 4.0 + i as f32 * 31.0;
                    let inset = [2.0, 6.0, 10.0, 13.0][i];
                    Item::filled(
                        Shape::DiffRoundedRect {
                            outer: [[left, 40.0], [left + 28.0, 88.0]],
                            outer_radius: [0.0, 6.0, 14.0, 14.0][i],
                            inner: [
                                [left + inset, 40.0 + inset],
                                [left + 28.0 - inset, 88.0 - inset],
                            ],
                            inner_radius: [0.0, 2.0, 8.0, 0.0][i],
                        },
                        Fill::Solid(BLUE),
                    )
                })
                .collect(),
        ),
        plate(
            "path/a-ring-under-a-rotation",
            // The hole has to turn with the shape. Two contours transformed
            // apart -- or a hole positioned from untransformed coordinates --
            // stay concentric only while the transform is a translation, so an
            // upright frame would hide it and this one does not.
            vec![
                Item::filled(
                    Shape::DiffRoundedRect {
                        outer: [[-36.0, -22.0], [36.0, 22.0]],
                        outer_radius: 12.0,
                        inner: [[-24.0, -10.0], [24.0, 10.0]],
                        inner_radius: 6.0,
                    },
                    Fill::Solid(BLUE),
                )
                .with_transform(Transform {
                    rotate: 0.5,
                    translate: [64.0, 64.0],
                    ..Transform::default()
                }),
                // A second ring at a different angle, so the plate says
                // something about the angle rather than about one of them.
                Item::filled(
                    Shape::DiffRoundedRect {
                        outer: [[-30.0, -14.0], [30.0, 14.0]],
                        outer_radius: 14.0,
                        inner: [[-18.0, -6.0], [18.0, 6.0]],
                        inner_radius: 6.0,
                    },
                    Fill::Solid(RED),
                )
                .with_transform(Transform {
                    rotate: -1.1,
                    translate: [64.0, 64.0],
                    ..Transform::default()
                }),
            ],
        ),
        plate(
            "path/cubic-with-a-cusp",
            // Control points crossing, so the curve reverses direction at a
            // point where its tangent vanishes. A stroke there has no normal to
            // offset along, which is the curve equivalent of the repeated point
            // above and reaches a different part of the same arithmetic.
            vec![Item::stroke(
                Shape::Cubic {
                    start: [24.0, 92.0],
                    c0: [104.0, 20.0],
                    c1: [24.0, 20.0],
                    end: [104.0, 92.0],
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    ..StrokeSpec::new(10.0)
                },
                YELLOW,
            )],
        ),
        plate(
            "path/stroke-caps-and-joins",
            vec![
                Item::stroke(
                    Shape::Polyline(vec![[16.0, 24.0], [64.0, 24.0], [64.0, 52.0]]),
                    StrokeSpec {
                        cap: LineCap::Butt,
                        join: LineJoin::Miter,
                        ..StrokeSpec::new(12.0)
                    },
                    RED,
                ),
                Item::stroke(
                    Shape::Polyline(vec![[16.0, 68.0], [64.0, 68.0], [64.0, 96.0]]),
                    StrokeSpec {
                        cap: LineCap::Round,
                        join: LineJoin::Round,
                        ..StrokeSpec::new(12.0)
                    },
                    GREEN,
                ),
                Item::stroke(
                    Shape::Polyline(vec![[16.0, 112.0], [64.0, 112.0], [64.0, 124.0]]),
                    StrokeSpec {
                        cap: LineCap::Square,
                        join: LineJoin::Bevel,
                        ..StrokeSpec::new(12.0)
                    },
                    BLUE,
                ),
            ],
        ),
        plate(
            "path/can-draw-multi-contour-convex-path",
            vec![Item::fill(
                Shape::Polygon(vec![[16.0, 16.0], [56.0, 16.0], [56.0, 56.0], [16.0, 56.0]]),
                RED,
            )]
            .into_iter()
            .chain([Item::fill(
                Shape::Polygon(vec![
                    [72.0, 72.0],
                    [112.0, 72.0],
                    [112.0, 112.0],
                    [72.0, 112.0],
                ]),
                BLUE,
            )])
            .collect(),
        ),
        plate(
            "path/can-render-filled-conic-paths",
            vec![Item::fill(
                Shape::Conic {
                    start: [16.0, 104.0],
                    ctrl: [64.0, 8.0],
                    end: [112.0, 104.0],
                    weight: std::f32::consts::FRAC_1_SQRT_2,
                },
                BLUE,
            )],
        ),
        plate(
            "path/can-render-stroked-conic-paths",
            vec![Item::stroke(
                Shape::Conic {
                    start: [16.0, 104.0],
                    ctrl: [64.0, 8.0],
                    end: [112.0, 104.0],
                    weight: std::f32::consts::FRAC_1_SQRT_2,
                },
                StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(8.0)
                },
                WHITE,
            )],
        ),
        plate(
            "path/can-render-tight-conic-path",
            vec![Item::stroke(
                // A weight well above one pulls the curve hard toward the
                // control point, which is where a subdivision that stops too
                // early shows a corner.
                Shape::Conic {
                    start: [24.0, 100.0],
                    ctrl: [64.0, 4.0],
                    end: [104.0, 100.0],
                    weight: 9.0,
                },
                StrokeSpec::new(5.0),
                YELLOW,
            )],
        ),
        plate(
            "path/solid-strokes-render-correctly",
            (0..5)
                .map(|i| {
                    let y = 20.0 + i as f32 * 22.0;
                    Item::stroke(
                        Shape::Polyline(vec![[14.0, y], [114.0, y]]),
                        StrokeSpec::new(1.0 + i as f32 * 3.5),
                        WHITE,
                    )
                })
                .collect(),
        ),
    ]
}

/// A regular polygon standing in for a circle, for the even-odd scene.
fn ring(radius: f32) -> Vec<[f32; 2]> {
    (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            [64.0 + radius * a.cos(), 64.0 + radius * a.sin()]
        })
        .collect()
}

/// `aiks_dl_gradient_unittests.cc` -- every gradient kind and tile mode.
fn gradient() -> Vec<Scene> {
    let band = Shape::Rect {
        min: [4.0, 4.0],
        max: [124.0, 124.0],
    };
    // A quarter of the way across, so every tile mode has something outside it
    // to act on -- which is the whole point of the four scenes below.
    let quarter = |tile| Fill::LinearGradient {
        start: [34.0, 0.0],
        end: [64.0, 0.0],
        stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
        tile,
    };
    let many = |count: usize| -> Vec<Stop> {
        (0..count)
            .map(|i| {
                let t = i as f32 / (count - 1) as f32;
                Stop::new([t, 0.4 + 0.5 * (1.0 - t), 1.0 - t, 1.0], t)
            })
            .collect()
    };

    vec![
        plate(
            "gradient/can-render-linear-gradient-clamp",
            vec![Item::filled(band.clone(), quarter(TileMode::Clamp))],
        ),
        plate(
            "gradient/can-render-linear-gradient-repeat",
            vec![Item::filled(band.clone(), quarter(TileMode::Repeat))],
        ),
        plate(
            "gradient/can-render-linear-gradient-mirror",
            vec![Item::filled(band.clone(), quarter(TileMode::Mirror))],
        ),
        plate(
            "gradient/can-render-linear-gradient-decal",
            vec![Item::filled(band.clone(), quarter(TileMode::Decal))],
        ),
        plate(
            "gradient/can-render-linear-gradient-many-colors-clamp",
            vec![Item::filled(band.clone(), many_colors(TileMode::Clamp))],
        ),
        plate(
            "gradient/can-render-linear-gradient-many-colors-repeat",
            vec![Item::filled(band.clone(), many_colors(TileMode::Repeat))],
        ),
        plate(
            "gradient/can-render-linear-gradient-many-colors-mirror",
            vec![Item::filled(band.clone(), many_colors(TileMode::Mirror))],
        ),
        plate(
            "gradient/can-render-linear-gradient-many-colors-decal",
            vec![Item::filled(band.clone(), many_colors(TileMode::Decal))
                // Composited, because a decal draws nothing outside its ramp
                // and "nothing" written by a mode that replaces is a hole
                // rather than an absence.
                .with_blend(BlendMode::SrcOver)],
        ),
        plate(
            "gradient/can-render-sweep-gradient-many-colors-clamp",
            vec![Item::filled(
                band.clone(),
                sweep_many_colors(TileMode::Clamp),
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-many-colors-repeat",
            vec![Item::filled(
                band.clone(),
                sweep_many_colors(TileMode::Repeat),
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-many-colors-mirror",
            vec![Item::filled(
                band.clone(),
                sweep_many_colors(TileMode::Mirror),
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-many-colors-decal",
            vec![
                Item::filled(band.clone(), sweep_many_colors(TileMode::Decal))
                    .with_blend(BlendMode::SrcOver),
            ],
        ),
        plate(
            "gradient/can-render-linear-gradient-with-overlapping-stops-clamp",
            // Two pairs of stops, each pair the same color and the second pair
            // starting where the first ends. A ramp built by interpolating
            // between neighbors has to cope with two stops at one offset, and
            // what it should produce is a hard edge down the diagonal rather
            // than a division by the zero distance between them.
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [4.0, 4.0],
                    end: [124.0, 124.0],
                    stops: vec![
                        Stop::new(WARM, 0.0),
                        Stop::new(WARM, 0.5),
                        Stop::new(COOL, 0.5),
                        Stop::new(COOL, 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-linear-gradient-decal-with-color-filter",
            // A decal gradient whose far stop is transparent, recolored by a
            // quarter of green composited over it. Upstream's comment is the
            // assertion: the green covers the whole rectangle, including the
            // border outside the ramp where the decal drew nothing -- a filter
            // applies to what the shader produced, and outside a decal what it
            // produced is transparent rather than absent.
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [4.0, 4.0],
                    end: [44.0, 44.0],
                    stops: vec![Stop::new(WARM, 0.0), Stop::new(COOL_CLEAR, 1.0)],
                    tile: TileMode::Decal,
                },
            )
            .with_color_filter(ColorFilter::blend(
                [0.0, 1.0, 0.0, 64.0 / 255.0],
                BlendMode::SrcOver,
            ))
            .with_blend(BlendMode::SrcOver)],
        ),
        plate(
            "gradient/can-render-linear-gradient-with-image-filter",
            // The filter applies to what the draw produced rather than to the
            // color it computed, so the gradient is blurred as an image and its
            // edges leave the shape. The far stop is transparent, which is what
            // makes the difference between the two orders visible at all.
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [4.0, 4.0],
                    end: [44.0, 44.0],
                    stops: vec![Stop::new(WARM, 0.0), Stop::new(COOL_CLEAR, 1.0)],
                    tile: TileMode::Clamp,
                },
            )
            .with_image_filter(ImageFilter::blur(6.0))
            .with_blend(BlendMode::SrcOver)],
        ),
        plate(
            "gradient/fast-gradient-test-horizontal",
            fast_gradient([0.0, 0.0], [56.0, 0.0], TileMode::Clamp),
        ),
        plate(
            "gradient/fast-gradient-test-horizontal-reversed",
            fast_gradient([56.0, 0.0], [0.0, 0.0], TileMode::Clamp),
        ),
        plate(
            "gradient/fast-gradient-test-vertical",
            fast_gradient([0.0, 0.0], [0.0, 120.0], TileMode::Clamp),
        ),
        plate(
            "gradient/fast-gradient-test-vertical-reversed",
            fast_gradient([0.0, 120.0], [0.0, 0.0], TileMode::Clamp),
        ),
        plate(
            "gradient/verify-non-optimized-gradient",
            // The same two shapes, with the endpoints pulled inside the shape
            // and reversed, and repeating. Upstream's comment says what it is
            // for: whatever fast route an axis-aligned gradient spanning its
            // shape may take, this one must not take it, and the picture says
            // whether the condition was tested or assumed.
            fast_gradient([0.0, 90.0], [0.0, 60.0], TileMode::Repeat),
        ),
        plate(
            "gradient/can-render-linear-gradient-way-many-colors-clamp",
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [8.0, 0.0],
                    end: [120.0, 0.0],
                    stops: many(24),
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-linear-gradient-many-colors-uneven-stops",
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [8.0, 0.0],
                    end: [120.0, 0.0],
                    // Bunched at one end, so the ramp is not a uniform sweep
                    // and a table built as though it were would be visibly
                    // wrong.
                    stops: vec![
                        Stop::new(RED, 0.0),
                        Stop::new(YELLOW, 0.05),
                        Stop::new(GREEN, 0.1),
                        Stop::new(BLUE, 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-linear-gradient-with-incomplete-stops",
            vec![Item::filled(
                band.clone(),
                // Neither end of the unit interval is named, so the shader has
                // to hold the first and last colors beyond them.
                Fill::LinearGradient {
                    start: [8.0, 0.0],
                    end: [120.0, 0.0],
                    stops: vec![Stop::new(RED, 0.3), Stop::new(BLUE, 0.7)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-radial-gradient",
            vec![Item::filled(
                band.clone(),
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 56.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-radial-gradient-many-colors",
            vec![Item::filled(
                band.clone(),
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 56.0,
                    stops: many(9),
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-clamp",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-repeat",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Repeat,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-mirror",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Mirror,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-decal",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::PI,
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Decal,
                },
            )],
        ),
        plate(
            "gradient/can-render-conical-gradient",
            vec![Item::filled(
                band.clone(),
                Fill::ConicalGradient {
                    start_center: [44.0, 44.0],
                    start_radius: 0.0,
                    end_center: [64.0, 64.0],
                    end_radius: 58.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/conical-concentric-circles",
            // The two circles share a center, which makes the cone a plain
            // radial gradient. Worth its own plate because it is the case the
            // general solution degenerates to: the quadratic that locates a
            // point along the cone loses its linear term when the centers
            // coincide, so an implementation that always divides by it fails
            // exactly here and nowhere else.
            vec![Item::filled(
                band.clone(),
                Fill::ConicalGradient {
                    start_center: [64.0, 64.0],
                    start_radius: 12.0,
                    end_center: [64.0, 64.0],
                    end_radius: 56.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/conical-with-the-focus-on-the-edge",
            // The starting point sitting exactly on the ending circle, which is
            // where the cone opens into a half-plane: the region the gradient
            // covers stops being bounded and the far side of the shape is
            // outside it altogether. The classic hard case, and the one where a
            // discriminant that should be zero comes out slightly negative.
            vec![Item::filled(
                band.clone(),
                Fill::ConicalGradient {
                    start_center: [20.0, 64.0],
                    start_radius: 0.0,
                    end_center: [64.0, 64.0],
                    end_radius: 44.0,
                    stops: vec![Stop::new(YELLOW, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/conical-with-separate-circles",
            // Neither circle containing the other, so the cone is a genuine
            // cone with a region outside it that no parameter reaches. What
            // fills that region is the tile mode's business, and decal is the
            // mode that makes the boundary visible rather than smearing the
            // last stop across it.
            vec![Item::filled(
                band.clone(),
                Fill::ConicalGradient {
                    start_center: [36.0, 46.0],
                    start_radius: 10.0,
                    end_center: [86.0, 86.0],
                    end_radius: 22.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(RED, 1.0)],
                    tile: TileMode::Decal,
                },
            )
            // Over the ground rather than replacing it, which every other plate
            // here can skip and this one cannot: outside the cone the material
            // is transparent, and the default `Src` would write that
            // transparency into the frame instead of letting the ground show.
            // The boundary is the whole subject, so it has to be visible.
            .with_blend(BlendMode::SrcOver)],
        ),
        plate(
            "gradient/linear-with-a-zero-length-axis",
            // Start and end at the same point. There is no direction to project
            // onto and the projection divides by the axis's own length, so this
            // is the gradient equivalent of a repeated point in a stroked path.
            // Every pixel has to land on one end of the ramp or the other --
            // one flat color, not a division by zero.
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [64.0, 64.0],
                    end: [64.0, 64.0],
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/radial-with-a-zero-radius",
            // The same degeneracy on the other family, and the one that found a
            // fault. Folding the radius into the mapping makes a radius of
            // nothing a singular matrix, and inverting a singular matrix gives
            // the identity -- which invented a radius of one clip unit, so the
            // gradient came out spanning half the plate and would have changed
            // with the plate's size. It now settles on the last stop, which is
            // where the real thing goes as the radius shrinks.
            vec![Item::filled(
                band.clone(),
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 0.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(GREEN, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/stops-sharing-an-offset-make-a-hard-edge",
            // Two stops at the same position, which is how a caller asks for a
            // band rather than a blend. The interpolation between them spans no
            // distance, so anything dividing by the gap between neighboring
            // stops divides by zero -- and the picture that says it went wrong
            // is a smear where there should be a line.
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [12.0, 12.0],
                    end: [116.0, 116.0],
                    stops: vec![
                        Stop::new(WHITE, 0.0),
                        Stop::new(WHITE, 0.45),
                        Stop::new(RED, 0.45),
                        Stop::new(RED, 0.7),
                        Stop::new(BLUE, 0.7),
                        Stop::new(BLUE, 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-gradient-decal-with-background",
            vec![
                Item::fill(band.clone(), [0.25, 0.25, 0.3, 1.0]),
                Item::filled(band.clone(), quarter(TileMode::Decal)).with_blend(BlendMode::SrcOver),
            ],
        ),
        plate(
            "gradient/gradient-strokes-render-correctly",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 42.0,
                },
                Fill::LinearGradient {
                    start: [22.0, 0.0],
                    end: [106.0, 0.0],
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )
            .with_stroke(StrokeSpec::new(12.0))],
        ),
        // The three gradient kinds that were missing the incomplete-stops
        // scene the linear one above already has. Same property in each: the
        // first stop is not at zero and the last is not at one, so the shader
        // has to hold the end colors across the interval nobody named rather
        // than running off the end of the table.
        //
        // Upstream draws all four kinds in one four-quadrant plate with
        // alignment lines under the gradient. The lines are there so a human at
        // a playground can see where the repeats land; nothing here is looked
        // at by a human, and a plate that holds four gradients tells you which
        // of the four broke only by where the difference is. One kind each.
        plate(
            "gradient/can-render-radial-gradient-with-incomplete-stops",
            vec![Item::filled(
                band.clone(),
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 56.0,
                    stops: vec![Stop::new(RED, 0.3), Stop::new(BLUE, 0.7)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-with-incomplete-stops",
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [64.0, 64.0],
                    start_angle: 0.0,
                    end_angle: std::f32::consts::TAU,
                    stops: vec![Stop::new(RED, 0.3), Stop::new(BLUE, 0.7)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-conical-gradient-with-incomplete-stops",
            vec![Item::filled(
                band.clone(),
                Fill::ConicalGradient {
                    start_center: [50.0, 50.0],
                    start_radius: 6.0,
                    end_center: [64.0, 64.0],
                    end_radius: 52.0,
                    stops: vec![Stop::new(RED, 0.3), Stop::new(BLUE, 0.7)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-linear-gradient-mask-blur",
            // A gradient under a mask blur, which is worth a plate of its own
            // because of the order the two happen in: the blur acts on the
            // shape's coverage and the gradient fills what survives, so a
            // renderer that blurred the filled result instead would smear the
            // stripes as well as the outline. Alternating stops make that
            // visible -- a smeared ramp between two colors still looks like a
            // ramp, and a smeared stripe pattern does not look like stripes.
            vec![
                Item::filled(
                    Shape::Circle {
                        center: [46.0, 50.0],
                        radius: 30.0,
                    },
                    Fill::LinearGradient {
                        start: [20.0, 20.0],
                        end: [76.0, 76.0],
                        stops: (0..11)
                            .map(|i| {
                                let c = if i % 2 == 0 { RED } else { WHITE };
                                Stop::new(c, i as f32 / 10.0)
                            })
                            .collect(),
                        tile: TileMode::Clamp,
                    },
                )
                .with_mask_blur(6.0)
                .with_blend(BlendMode::SrcOver),
                Item::filled(
                    Shape::Rect {
                        min: [24.0, 74.0],
                        max: [110.0, 108.0],
                    },
                    Fill::LinearGradient {
                        start: [20.0, 20.0],
                        end: [76.0, 76.0],
                        stops: (0..11)
                            .map(|i| {
                                let c = if i % 2 == 0 { RED } else { WHITE };
                                Stop::new(c, i as f32 / 10.0)
                            })
                            .collect(),
                        tile: TileMode::Clamp,
                    },
                )
                .with_mask_blur(6.0)
                .with_blend(BlendMode::SrcOver),
            ],
        ),
        // The four dithering plates, and what they are for is worth stating
        // exactly, because their names promise something the plate cannot
        // deliver at this size.
        //
        // Banding needs a ramp that spends many pixels on each representable
        // value. Upstream's linear plate runs 0xCC to 0x33 along a diagonal of
        // about nine hundred and forty pixels: a hundred and fifty-three levels
        // over that distance is a band six pixels wide, which is why it is the
        // picture attached to the issue that put dithering in the renderer. The
        // same two colors across this plate's hundred-and-seventy-pixel
        // diagonal cross a level about every pixel. There is no band here to
        // break up, and scaling the plate up to make one would cost more memory
        // than every other plate in the catalog put together.
        //
        // So these do not show dithering working, and nothing in this file
        // could: both backends dither identically, so the comparison this
        // catalog performs is blind to it either way. What measures it is
        // `dithering_tracks_a_gradient_better_than_rounding_does` in the public
        // API's tests, which reconstructs the same ramp undithered and requires
        // the dithered one to track it at least three times as closely.
        //
        // What these are is the rest of the chapter's reason: four scenes that
        // exist upstream and now exist here, running each gradient kind through
        // the dither on both backends. The colors and geometry are upstream's,
        // scaled.
        plate(
            "gradient/can-render-linear-gradient-with-dithering-enabled",
            // 0xCCCCCC to 0x333333, which is upstream's pair and is taken from
            // the issue that put dithering in the renderer at all. Both are
            // gray, so all three channels band together and in step, which is
            // what makes it visible rather than merely present.
            vec![Item::filled(
                band.clone(),
                Fill::LinearGradient {
                    start: [4.0, 4.0],
                    end: [124.0, 124.0],
                    stops: vec![
                        Stop::new([0.8, 0.8, 0.8, 1.0], 0.0),
                        Stop::new([0.2, 0.2, 0.2, 1.0], 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-radial-gradient-with-dithering-enabled",
            // White to black across the full radius. A radial ramp bands in
            // rings rather than stripes, which is a different picture of the
            // same defect and the reason upstream keeps all four.
            vec![Item::filled(
                band.clone(),
                Fill::RadialGradient {
                    center: [64.0, 64.0],
                    radius: 60.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLACK, 1.0)],
                    tile: TileMode::Clamp,
                },
            )],
        ),
        plate(
            "gradient/can-render-sweep-gradient-with-dithering-enabled",
            // Ninety degrees of arc, mirrored, about a center a sixth of the
            // way into the plate -- upstream's arrangement, which puts the
            // center near a corner so the bands fan across the whole plate
            // instead of meeting at the middle.
            vec![Item::filled(
                band.clone(),
                Fill::SweepGradient {
                    center: [24.0, 24.0],
                    start_angle: std::f32::consts::FRAC_PI_4,
                    end_angle: 3.0 * std::f32::consts::FRAC_PI_4,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLACK, 1.0)],
                    tile: TileMode::Mirror,
                },
            )],
        ),
        plate(
            "gradient/can-render-conical-gradient-with-dithering-enabled",
            // A degenerate start circle -- a point -- opening onto one of
            // radius twenty, which is upstream's, scaled. Mirrored, so the
            // parameter past the far circle folds back rather than clamping,
            // and the banding continues out to the plate's edge instead of
            // stopping at a flat surround.
            vec![Item::filled(
                band.clone(),
                Fill::ConicalGradient {
                    start_center: [4.0, 4.2],
                    start_radius: 0.0,
                    end_center: [24.0, 24.0],
                    end_radius: 20.0,
                    stops: vec![Stop::new(WHITE, 0.0), Stop::new(BLACK, 1.0)],
                    tile: TileMode::Mirror,
                },
            )],
        ),
    ]
}

/// `aiks_dl_clip_unittests.cc`.
fn clip() -> Vec<Scene> {
    vec![
        Scene::tree(
            "clip/can-render-clipped-backdrop-filter-with-superellipse",
            // Upstream's sheet: a round superellipse clipping a whole group,
            // translucent rows inside it, a blurred panel clipped to a rounded
            // rectangle of its own, and a bar drawn over the lot afterwards.
            // The shape it came from is a Cupertino sheet with a frosted app
            // bar, and what it is for is the clip stack across the pass the
            // backdrop cuts -- a parent clip popped too early takes the rows
            // with it, and a backdrop reading past its own clip blurs the bar.
            //
            // The corners are equal here where upstream rounds only the top
            // two. The asymmetry is the sheet's, not the clip stack's, and a
            // squircle with one radius is the shape this catalog already has a
            // plate for.
            vec![Node::Clip {
                rect: None,
                rect_out: None,
                shape: Some(Shape::RoundSuperellipse {
                    min: [8.0, 14.0],
                    max: [120.0, 128.0],
                    radii: [[10.0, 10.0]; 4],
                }),
                transform: Transform::default(),
                children: vec![
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [8.0, 14.0],
                            max: [120.0, 128.0],
                        },
                        [0.20, 0.22, 0.28, 1.0],
                    ))),
                    // The rows, each barely darker than the sheet, so a clip
                    // that dropped one is a missing band rather than a
                    // different shade.
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [8.0, 26.0],
                            max: [120.0, 38.0],
                        },
                        [0.0, 0.0, 0.0, 0.18],
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [8.0, 44.0],
                            max: [120.0, 56.0],
                        },
                        [0.0, 0.0, 0.0, 0.18],
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [8.0, 62.0],
                            max: [120.0, 74.0],
                        },
                        [0.0, 0.0, 0.0, 0.18],
                    ))),
                    // The frosted panel, clipped to its own rounded rectangle
                    // inside the sheet's clip, and placed over the third row
                    // rather than below them all: a blur over flat color is a
                    // flat color, so the row's edge running under the panel is
                    // the only thing in the frame that shows the backdrop was
                    // read at all.
                    Node::Clip {
                        rect: None,
                        rect_out: None,
                        shape: Some(Shape::RoundedRect {
                            min: [16.0, 58.0],
                            max: [112.0, 82.0],
                            radius: 6.0,
                        }),
                        transform: Transform::default(),
                        children: vec![Node::Layer {
                            layer: Box::new(LayerSpec {
                                backdrop: ImageFilter::blur(5.0),
                                ..LayerSpec::default()
                            }),
                            bounds: None,
                            transform: Transform::default(),
                            children: vec![Node::Draw(Box::new(Item::fill(
                                Shape::Rect {
                                    min: [16.0, 58.0],
                                    max: [112.0, 82.0],
                                },
                                [1.0, 0.6, 0.0, 0.35],
                            )))],
                        }],
                    },
                    // And the bar over everything, which the backdrop must not
                    // have blurred: it is drawn after.
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [8.0, 14.0],
                            max: [120.0, 24.0],
                        },
                        WHITE,
                    ))),
                ],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        plate(
            "clip/difference-clip-keeps-what-is-outside",
            // `clipRect` with `ClipOp.difference`, which is the one clip that
            // operation applies to. Stated with an ordinary clip as well, since
            // that is the arrangement where the two have to compose rather than
            // the difference simply removing a square from a full frame.
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                YELLOW,
            )
            .with_clip([16.0, 16.0, 112.0, 112.0])
            .with_clip_out([48.0, 48.0, 80.0, 80.0])],
        ),
        plate(
            "clip/a-rectangular-clip-under-perspective",
            // The plate that catches a rectangle handed to the scissor unit
            // when it is no longer one. Under a divisor that varies with x, a
            // clip rectangle's horizontal edges bow toward the vanishing point
            // and its box is not the region asked for -- so this has to go
            // through the stencil, and the shape of the clipped fill is what
            // says whether it did. A scissor would leave a plain rectangle.
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                YELLOW,
            )
            .with_clip([16.0, 16.0, 112.0, 112.0])
            .with_transform(Transform {
                perspective: [0.006, 0.0],
                ..Transform::default()
            })],
        ),
        plate(
            "clip/difference-clip-under-a-rotation",
            // Turned, the rectangle is a quadrilateral and the clip has to
            // remove that rather than a box around it. The corners of the hole
            // are what says which happened.
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                GREEN,
            )
            .with_clip_out([34.0, 34.0, 94.0, 94.0])
            .with_transform(Transform {
                rotate: 0.4,
                translate: [26.0, -22.0],
                ..Transform::default()
            })],
        ),
        plate(
            "clip/can-render-nested-clips",
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                YELLOW,
            )
            // Two clips on one item, one axis-aligned and one not, so both
            // paths through the clip machinery narrow the same draw.
            .with_clip([20.0, 20.0, 108.0, 108.0])
            .with_clip_shape(Shape::Circle {
                center: [64.0, 64.0],
                radius: 40.0,
            })],
        ),
        plate(
            "clip/clips-use-current-transform",
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                GREEN,
            )
            .with_clip_shape(Shape::Rect {
                min: [40.0, 40.0],
                max: [88.0, 88.0],
            })
            // The clip is stated in the item's own space, so it turns with it
            // -- a clip that did not would stay square here.
            .with_transform(Transform {
                rotate: 0.5,
                translate: [10.0, -6.0],
                ..Transform::default()
            })],
        ),
        plate(
            "clip/can-render-with-contiguous-clip-restores",
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    RED,
                )
                .with_clip_shape(Shape::Circle {
                    center: [44.0, 64.0],
                    radius: 34.0,
                }),
                // A second item with its own clip: each is built and stepped
                // back independently, so the first one's clip must not still
                // be in force here.
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    BLUE,
                )
                .with_blend(BlendMode::SrcOver)
                .with_clip_shape(Shape::Circle {
                    center: [84.0, 64.0],
                    radius: 34.0,
                }),
            ],
        ),
        single_sampled_plate(
            "clip/framebuffer-blends-respect-clips",
            // An advanced blend confined to a clip. The mode is the subject
            // rather than the shape: a separable mode is a fixed-function
            // blend the hardware applies as it writes, so the clip is already
            // deciding which pixels get written and nothing more is needed. An
            // advanced mode is not -- it reads the destination, either through
            // a framebuffer fetch or through a copy of the target, and a
            // reader that ignores the clip happily blends into pixels no draw
            // should have touched.
            //
            // So the red square is drawn under `Multiply` across a region much
            // larger than the circle it is clipped to, and the corners of that
            // square are the answer: the ground has to be exactly as it was
            // outside the circle, whatever the blend did inside it.
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    WHITE,
                ),
                Item::fill(
                    Shape::Rect {
                        min: [28.0, 28.0],
                        max: [100.0, 100.0],
                    },
                    RED,
                )
                .with_blend(BlendMode::Multiply)
                .with_clip_shape(Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 30.0,
                }),
                // And something ordinary through the same clip afterwards, so
                // the plate says the clip is still the clip once an advanced
                // mode has been through it.
                Item::fill(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 30.0,
                    },
                    [0.0, 0.7, 0.2, 0.55],
                )
                .with_blend(BlendMode::SrcOver),
            ],
        ),
    ]
}

/// `aiks_dl_opacity_unittests.cc`.
fn opacity() -> Vec<Scene> {
    let overlapping = || {
        vec![
            Node::Draw(Box::new(Item::fill(
                Shape::Circle {
                    center: [50.0, 64.0],
                    radius: 32.0,
                },
                RED,
            ))),
            Node::Draw(Box::new(
                Item::fill(
                    Shape::Circle {
                        center: [82.0, 64.0],
                        radius: 32.0,
                    },
                    BLUE,
                )
                .with_blend(BlendMode::SrcOver),
            )),
        ]
    };
    vec![
        Scene::tree(
            "blur/erode",
            // Two discs joined by a bar thinner than twice the radius, eroded
            // as a group rather than one shape at a time. The bar disappears
            // entirely and the discs shrink, which is the picture that says the
            // erosion ran over the union: filtering each shape on its own would
            // give the same discs but is a different operation, and on shapes
            // that overlapped it would give a different answer.
            vec![Node::Layer {
                layer: Box::new(LayerSpec::eroded(7.0, 7.0)),
                bounds: Some([0.0, 0.0, 128.0, 128.0]),
                transform: Transform::default(),
                children: vec![
                    Node::Draw(Box::new(Item::fill(
                        Shape::Circle {
                            center: [40.0, 64.0],
                            radius: 26.0,
                        },
                        WHITE,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Circle {
                            center: [88.0, 64.0],
                            radius: 26.0,
                        },
                        WHITE,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [40.0, 60.0],
                            max: [88.0, 68.0],
                        },
                        WHITE,
                    ))),
                ],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "opacity/can-render-group-opacity",
            vec![Node::Layer {
                // The picture that distinguishes a group's opacity from each
                // member's: where the two circles overlap, a group at half
                // alpha shows one blend and two half-alpha circles show two.
                layer: Box::new(LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: overlapping(),
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "opacity/draw-opacity-peephole",
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                }),
                bounds: Some([12.0, 24.0, 116.0, 104.0]),
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(Item::fill(
                    Shape::RoundedRect {
                        min: [20.0, 32.0],
                        max: [108.0, 96.0],
                        radius: 14.0,
                    },
                    WHITE,
                )))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "opacity/can-render-group-opacity-to-savelayer",
            // A group at seven tenths inside another group at seven tenths,
            // which upstream words as one save layer forwarding its opacity to
            // the next. What it checks is that the opacity is distributed and
            // not applied twice over on one side and not at all on the other:
            // the two rectangles overlap, so the inner group has to be
            // flattened before either alpha touches it.
            //
            // The row above this one named subpass collapse as the obstacle,
            // which is upstream's optimization for exactly this arrangement --
            // it collapses the inner layer into the outer where the contents
            // allow. That decides how many passes the picture costs, not what
            // the picture is, and nothing here needs it to draw one.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    alpha: 0.7,
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Layer {
                    layer: Box::new(LayerSpec {
                        alpha: 0.7,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![
                        Node::Draw(Box::new(Item::fill(
                            Shape::Rect {
                                min: [16.0, 16.0],
                                max: [96.0, 96.0],
                            },
                            RED,
                        ))),
                        Node::Draw(Box::new(
                            Item::fill(
                                Shape::Rect {
                                    min: [32.0, 32.0],
                                    max: [112.0, 112.0],
                                },
                                RED,
                            )
                            .with_blend(BlendMode::SrcOver),
                        )),
                    ],
                }],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}

/// The fifteen modes upstream lists in its advanced-blend color-filter grid,
/// in its order.
///
/// The twelve separable ones first, then the four non-separable -- which is
/// upstream's order and not this repository's, and is kept so the plate's
/// layout can be read against the original's.
const ADVANCED: &[BlendMode] = &[
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::HardLight,
    BlendMode::SoftLight,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Multiply,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
    BlendMode::Luminosity,
];

/// Every blend mode, by the name the catalog gives its scene.
///
/// Named here rather than derived from a `Display` implementation because a
/// scene's name is a `&'static str` and has to be one: it is an identifier the
/// playground steps through and a test reports, not a label. Writing them out
/// also means adding a mode to the renderer does not silently add a scene
/// nobody looked at.
const MODES: &[(BlendMode, &str)] = &[
    (BlendMode::Clear, "blend/blend-mode-clear"),
    (BlendMode::Src, "blend/blend-mode-src"),
    (BlendMode::Dst, "blend/blend-mode-dst"),
    (BlendMode::SrcOver, "blend/blend-mode-src-over"),
    (BlendMode::DstOver, "blend/blend-mode-dst-over"),
    (BlendMode::SrcIn, "blend/blend-mode-src-in"),
    (BlendMode::DstIn, "blend/blend-mode-dst-in"),
    (BlendMode::SrcOut, "blend/blend-mode-src-out"),
    (BlendMode::DstOut, "blend/blend-mode-dst-out"),
    (BlendMode::SrcATop, "blend/blend-mode-src-atop"),
    (BlendMode::DstATop, "blend/blend-mode-dst-atop"),
    (BlendMode::Xor, "blend/blend-mode-xor"),
    (BlendMode::Plus, "blend/blend-mode-plus"),
    (BlendMode::Modulate, "blend/blend-mode-modulate"),
    (BlendMode::Multiply, "blend/blend-mode-multiply"),
    (BlendMode::Screen, "blend/blend-mode-screen"),
    (BlendMode::Overlay, "blend/blend-mode-overlay"),
    (BlendMode::Darken, "blend/blend-mode-darken"),
    (BlendMode::Lighten, "blend/blend-mode-lighten"),
    (BlendMode::ColorDodge, "blend/blend-mode-color-dodge"),
    (BlendMode::ColorBurn, "blend/blend-mode-color-burn"),
    (BlendMode::HardLight, "blend/blend-mode-hard-light"),
    (BlendMode::SoftLight, "blend/blend-mode-soft-light"),
    (BlendMode::Difference, "blend/blend-mode-difference"),
    (BlendMode::Exclusion, "blend/blend-mode-exclusion"),
    (BlendMode::Hue, "blend/blend-mode-hue"),
    (BlendMode::Saturation, "blend/blend-mode-saturation"),
    (BlendMode::Color, "blend/blend-mode-color"),
    (BlendMode::Luminosity, "blend/blend-mode-luminosity"),
];

/// The same modes again, for the family upstream generates at half source
/// alpha.
///
/// A second table of names rather than a suffix computed from the first, for
/// the reason the first is written out: a scene's name is an identifier the
/// playground steps through and a test reports, and it has to be a
/// `&'static str`.
const MODES_AT_HALF_ALPHA: &[(BlendMode, &str)] = &[
    (BlendMode::Clear, "blend/blend-mode-src-alpha-clear"),
    (BlendMode::Src, "blend/blend-mode-src-alpha-src"),
    (BlendMode::Dst, "blend/blend-mode-src-alpha-dst"),
    (BlendMode::SrcOver, "blend/blend-mode-src-alpha-src-over"),
    (BlendMode::DstOver, "blend/blend-mode-src-alpha-dst-over"),
    (BlendMode::SrcIn, "blend/blend-mode-src-alpha-src-in"),
    (BlendMode::DstIn, "blend/blend-mode-src-alpha-dst-in"),
    (BlendMode::SrcOut, "blend/blend-mode-src-alpha-src-out"),
    (BlendMode::DstOut, "blend/blend-mode-src-alpha-dst-out"),
    (BlendMode::SrcATop, "blend/blend-mode-src-alpha-src-atop"),
    (BlendMode::DstATop, "blend/blend-mode-src-alpha-dst-atop"),
    (BlendMode::Xor, "blend/blend-mode-src-alpha-xor"),
    (BlendMode::Plus, "blend/blend-mode-src-alpha-plus"),
    (BlendMode::Modulate, "blend/blend-mode-src-alpha-modulate"),
    (BlendMode::Multiply, "blend/blend-mode-src-alpha-multiply"),
    (BlendMode::Screen, "blend/blend-mode-src-alpha-screen"),
    (BlendMode::Overlay, "blend/blend-mode-src-alpha-overlay"),
    (BlendMode::Darken, "blend/blend-mode-src-alpha-darken"),
    (BlendMode::Lighten, "blend/blend-mode-src-alpha-lighten"),
    (
        BlendMode::ColorDodge,
        "blend/blend-mode-src-alpha-color-dodge",
    ),
    (
        BlendMode::ColorBurn,
        "blend/blend-mode-src-alpha-color-burn",
    ),
    (
        BlendMode::HardLight,
        "blend/blend-mode-src-alpha-hard-light",
    ),
    (
        BlendMode::SoftLight,
        "blend/blend-mode-src-alpha-soft-light",
    ),
    (
        BlendMode::Difference,
        "blend/blend-mode-src-alpha-difference",
    ),
    (BlendMode::Exclusion, "blend/blend-mode-src-alpha-exclusion"),
    (BlendMode::Hue, "blend/blend-mode-src-alpha-hue"),
    (
        BlendMode::Saturation,
        "blend/blend-mode-src-alpha-saturation",
    ),
    (BlendMode::Color, "blend/blend-mode-src-alpha-color"),
    (
        BlendMode::Luminosity,
        "blend/blend-mode-src-alpha-luminosity",
    ),
];

/// `aiks_dl_blend_unittests.cc`.
///
/// The per-mode scenes mirror what the original generates with a macro over
/// every blend mode. A scene naming an advanced mode declares that need by
/// containing one, so a device without the extension reports it as a gap
/// rather than as a difference.
fn blend() -> Vec<Scene> {
    let mut scenes: Vec<Scene> = MODES
        .iter()
        .map(|(mode, name)| {
            // Single-sampled, and not by preference. Measured on this machine's
            // Vulkan software rasterizer: an advanced blend under multisampling
            // produces nothing at all -- the draw is silently dropped, and
            // seventeen plates of this catalog rendered identically to
            // themselves with the blended draw deleted. The same scenes are
            // correct at one sample, and correct at both sample counts on GLES,
            // which is the same Mesa through a different extension. So it is
            // the driver rather than this renderer, and the answer is not to
            // work around it but to stop asking: these plates are about color
            // arithmetic rather than edges, and multisampling was never buying
            // them anything.
            //
            // The `blend_modes.rs` suite checks the arithmetic of every mode
            // against its equation and is unaffected either way; what these
            // recover is the end-to-end picture, which is what the catalog is
            // for.
            single_sampled_plate(
                name,
                vec![
                    // A destination with structure rather than a flat color:
                    // dodge, burn and the two contrast modes are functions of
                    // what is underneath, and a flat backdrop would exercise
                    // one point of each curve.
                    //
                    // Colored rather than gray, which took a measurement to
                    // find. The backdrop used to run dark gray to light, and
                    // two of the modes cannot say anything against gray: hue
                    // is `set_lum(set_sat(cs, sat(cb)), lum(cb))`, and a gray
                    // backdrop has no saturation, so the whole expression
                    // collapses to the backdrop. Both plates rendered
                    // identically to themselves with the blended circle
                    // deleted. A range of value *and* of hue keeps the
                    // contrast modes exercised and gives the non-separable
                    // ones something to exchange.
                    Item::filled(
                        Shape::Rect {
                            min: [8.0, 8.0],
                            max: [120.0, 120.0],
                        },
                        Fill::LinearGradient {
                            start: [8.0, 8.0],
                            end: [120.0, 120.0],
                            stops: vec![
                                Stop::new([0.08, 0.12, 0.5, 1.0], 0.0),
                                Stop::new([0.95, 0.82, 0.2, 1.0], 1.0),
                            ],
                            tile: TileMode::Clamp,
                        },
                    ),
                    // Translucent, so the modes that read the source's alpha
                    // differ from the ones that do not.
                    Item::fill(
                        Shape::Circle {
                            center: [64.0, 64.0],
                            radius: 40.0,
                        },
                        [0.9, 0.35, 0.2, 0.75],
                    )
                    .with_blend(*mode),
                ],
            )
        })
        .collect();

    // Upstream generates two families over every blend mode, and the second is
    // the one this had not mirrored: the mode on a *group* rather than on a
    // draw, at half alpha. The distinction is real and neither family covers
    // the other. A draw's mode combines one shape's color with the frame; a
    // group's combines a finished image with it, after the group's own alpha
    // has scaled what it holds -- so the two have to compose in that order, and
    // a renderer applying the alpha after the mode would agree with the draw
    // family and disagree with this one for every mode that is not linear.
    scenes.extend(MODES_AT_HALF_ALPHA.iter().map(|(mode, name)| {
        Scene::tree(
            name,
            vec![
                // The same destination the draw family uses, so the two can be
                // read against each other.
                Node::Draw(Box::new(Item::filled(
                    Shape::Rect {
                        min: [8.0, 8.0],
                        max: [120.0, 120.0],
                    },
                    Fill::LinearGradient {
                        start: [8.0, 8.0],
                        end: [120.0, 120.0],
                        stops: vec![
                            Stop::new([0.08, 0.12, 0.5, 1.0], 0.0),
                            Stop::new([0.95, 0.82, 0.2, 1.0], 1.0),
                        ],
                        tile: TileMode::Clamp,
                    },
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        alpha: 0.5,
                        blend: *mode,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(
                        Item::fill(
                            Shape::Circle {
                                center: [64.0, 64.0],
                                radius: 40.0,
                            },
                            // Opaque here, where the draw family's circle is
                            // translucent: the group's alpha is what makes this
                            // one half strength, and a translucent circle inside
                            // it would confuse the two.
                            [0.9, 0.35, 0.2, 1.0],
                        )
                        .with_blend(BlendMode::SrcOver),
                    ))],
                },
            ],
        )
        .with_background(DARK)
        // Single-sampled for the reason the draw family is: an advanced blend
        // under multisampling draws nothing on this machine's Vulkan software
        // rasterizer.
        .with_samples(1)
    }));

    scenes.push(plate(
        "blend/paint-blend-mode-is-respected",
        // Two groups in one list, and the point is that the mode belongs to the
        // paint rather than to the canvas: the first pair composites and the
        // second adds, so where the first pair overlaps it darkens toward the
        // ground and where the second overlaps it runs to white. A renderer
        // holding the mode as state rather than per draw would give the whole
        // plate one of the two behaviors.
        vec![
            Item::fill(
                Shape::Circle {
                    center: [34.0, 40.0],
                    radius: 22.0,
                },
                [1.0, 0.0, 0.0, 0.5],
            )
            .with_blend(BlendMode::SrcOver),
            Item::fill(
                Shape::Circle {
                    center: [58.0, 40.0],
                    radius: 22.0,
                },
                [0.0, 1.0, 0.0, 0.5],
            )
            .with_blend(BlendMode::SrcOver),
            Item::fill(
                Shape::Circle {
                    center: [84.0, 96.0],
                    radius: 18.0,
                },
                RED,
            )
            .with_blend(BlendMode::Plus),
            Item::fill(
                Shape::Circle {
                    center: [108.0, 96.0],
                    radius: 18.0,
                },
                GREEN,
            )
            .with_blend(BlendMode::Plus),
            Item::fill(
                Shape::Circle {
                    center: [96.0, 74.0],
                    radius: 18.0,
                },
                BLUE,
            )
            .with_blend(BlendMode::Plus),
        ],
    ));

    scenes.push(plate(
        "blend/foreground-pipeline-blend-applies-transform-correctly",
        // An image recolored by a filter that is itself a blend, drawn under a
        // rotation. A color filter acts on what the material produced, in the
        // material's own space, and the transform then places the result -- so
        // the tint has to arrive on the turned rectangle rather than on an
        // upright one. Upstream keeps this and an advanced-mode twin; the twin
        // is refused here, which `docs/non-parity.md` records.
        vec![Item::filled(
            Shape::Rect {
                min: [-40.0, -28.0],
                max: [40.0, 28.0],
            },
            sheet(
                [-40.0, -28.0, 40.0, 28.0],
                ALL,
                TileMode::Clamp,
                Sampling::Linear,
            ),
        )
        .with_color_filter(ColorFilter::blend(
            [1.0, 165.0 / 255.0, 0.0, 1.0],
            BlendMode::SrcIn,
        ))
        .with_transform(Transform {
            rotate: 30.0f32.to_radians(),
            translate: [64.0, 64.0],
            ..Transform::default()
        })
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(plate(
        "blend/foreground-advanced-blend-applies-transform-correctly",
        // The twin of the plate above, in a mode a matrix cannot state. The
        // filter is evaluated per fragment against the constant rather than
        // folded into the material's arithmetic, and the claim is the same:
        // the recoloring happens in the material's own space and the transform
        // places the result.
        vec![Item::filled(
            Shape::Rect {
                min: [-40.0, -28.0],
                max: [40.0, 28.0],
            },
            sheet(
                [-40.0, -28.0, 40.0, 28.0],
                ALL,
                TileMode::Clamp,
                Sampling::Linear,
            ),
        )
        .with_color_filter(ColorFilter::blend(
            [1.0, 165.0 / 255.0, 0.0, 1.0],
            BlendMode::ColorDodge,
        ))
        .with_transform(Transform {
            rotate: 30.0f32.to_radians(),
            translate: [64.0, 64.0],
            ..Transform::default()
        })
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(plate_tree(
        "blend/foreground-blend-subpass-collapse-optimization",
        // A group recolored by a blend against a constant, holding one turned
        // square. Upstream names it for an optimization that folds the group
        // into the draw beneath it, which decides how many passes it takes and
        // not what it draws.
        //
        // Blue through `ColorDodge` against red: the dodge divides by one minus
        // the source, so the red channel saturates and the blue survives, and
        // the square comes out magenta.
        //
        // What surrounds it is the more interesting half. `ColorDodge` is one
        // of the modes that produces something from nothing, so the filter
        // floods every pixel the group covers -- and an unbounded group covers
        // the bounding box of what went into it, which for a square turned a
        // third of a turn is a larger upright square. So the plate is a magenta
        // diamond in a red box on the ground, and the red box is what says the
        // group was recolored as a finished image over its own extent rather
        // than shape by shape.
        vec![Node::Layer {
            layer: Box::new(LayerSpec {
                color_filter: ColorFilter::blend(RED, BlendMode::ColorDodge),
                ..LayerSpec::default()
            }),
            bounds: None,
            transform: Transform::default(),
            children: vec![Node::Draw(Box::new(
                Item::fill(
                    Shape::Rect {
                        min: [-26.0, -26.0],
                        max: [26.0, 26.0],
                    },
                    BLUE,
                )
                .with_transform(Transform {
                    rotate: 120.0f32.to_radians(),
                    translate: [64.0, 64.0],
                    ..Transform::default()
                }),
            ))],
        }],
    ));

    scenes.push(plate(
        "blend/color-filter-advanced-blend",
        // Upstream's grid of every advanced mode as a color filter, over a
        // destination with structure so the piecewise ones show their branches.
        // Fifteen modes in a five-by-three grid.
        //
        // A color filter against a constant is not the same thing as the same
        // mode on the paint: this one never reads the frame, so it needs no
        // framebuffer fetch and no extension, and it is available on a device
        // where the paint's own advanced blending is not.
        ADVANCED
            .iter()
            .enumerate()
            .map(|(i, mode)| {
                let (col, row) = ((i % 5) as f32, (i / 5) as f32);
                let (x, y) = (col * 25.0 + 3.0, row * 42.0 + 3.0);
                Item::filled(
                    Shape::Rect {
                        min: [x, y],
                        max: [x + 22.0, y + 38.0],
                    },
                    Fill::LinearGradient {
                        start: [x, y],
                        end: [x + 22.0, y + 38.0],
                        stops: vec![
                            Stop::new([0.15, 0.15, 0.15, 1.0], 0.0),
                            Stop::new([0.9, 0.9, 0.9, 1.0], 1.0),
                        ],
                        tile: TileMode::Clamp,
                    },
                )
                .with_color_filter(ColorFilter::blend([0.95, 0.45, 0.15, 1.0], *mode))
            })
            .collect(),
    ));

    scenes.push(
        Scene::tree(
            "blend/can-render-advanced-blend-color-filter-with-save-layer",
            // The filter on a group rather than on a draw. What it acts on is
            // the finished layer, so the black ground and the white rectangle
            // inside it are one image by the time the mode sees them -- and
            // `Difference` against a half-alpha green gives two different
            // answers on the two, which is what says the filter ran on the
            // composite rather than on each draw.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    color_filter: ColorFilter::blend([0.0, 1.0, 0.0, 0.5], BlendMode::Difference),
                    ..LayerSpec::default()
                }),
                bounds: Some([0.0, 0.0, 128.0, 128.0]),
                transform: Transform::default(),
                children: vec![
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [0.0, 0.0],
                            max: [128.0, 128.0],
                        },
                        BLACK,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [26.0, 26.0],
                            max: [102.0, 102.0],
                        },
                        WHITE,
                    ))),
                ],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    scenes.push(
        Scene::tree(
            "blend/draw-paint-with-advanced-blend-over-filter",
            // A paint covering everything, in an advanced mode, over a
            // destination that a mask blur put there. What a paint covers is
            // the clip rather than any shape, so this is the case where an
            // advanced mode has to read a destination the renderer built in a
            // pass of its own rather than one it drew directly.
            vec![
                Node::Draw(Box::new(Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    WHITE,
                ))),
                Node::Draw(Box::new(
                    Item::fill(
                        Shape::Circle {
                            center: [64.0, 64.0],
                            radius: 42.0,
                        },
                        BLACK,
                    )
                    .with_mask_blur(12.0)
                    .with_blend(BlendMode::SrcOver),
                )),
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid(GREEN),
                    blend: BlendMode::Screen,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
            ],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    scenes.push(single_sampled_plate(
        "blend/emulated-advanced-blend-restore",
        // An advanced blend inside a clip, followed by a draw the clip must
        // still cut. A mode the hardware cannot do directly is emulated with a
        // pass of its own, and the failure this is named for is that pass
        // leaving the clip behind: the blue rectangle sits entirely outside the
        // clip and must not appear at all.
        vec![
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                WHITE,
            ),
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [102.0, 76.0],
                },
                RED,
            )
            .with_blend(BlendMode::Difference)
            .with_clip([26.0, 26.0, 102.0, 76.0]),
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [26.0, 26.0],
                },
                BLUE,
            )
            .with_blend(BlendMode::SrcOver)
            .with_clip([26.0, 26.0, 102.0, 76.0]),
        ],
    ));

    scenes.push(single_sampled_plate(
        "blend/blend-mode-should-cover-whole-screen",
        vec![
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                RED,
            ),
            // A source covering everything, which is what the original checks:
            // a blend that left an unwritten margin would show the ground.
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                BLUE,
            )
            .with_blend(BlendMode::Screen),
        ],
    ));

    scenes.push(plate(
        "blend/framebuffer-advanced-blend-coverage",
        vec![
            // Upstream's gray, which is what the multiply has to act on.
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                [169.0 / 255.0, 169.0 / 255.0, 169.0 / 255.0, 1.0],
            ),
            // The sheet drawn into a rectangle smaller than itself, which is
            // the scale upstream applies before drawing. What the scene is for
            // is that the scale reaches the image at all: upstream's own name
            // is for the path it takes there, where a blend that reads the
            // framebuffer is a separate pass and the transform has to survive
            // being handed to it.
            Item::filled(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [59.2, 59.2],
                },
                sheet(
                    [8.0, 8.0, 59.2, 59.2],
                    ALL,
                    TileMode::Clamp,
                    Sampling::Linear,
                ),
            )
            .with_blend(BlendMode::Multiply),
        ],
    ));

    scenes.push(plate(
        "blend/color-filter-blend",
        vec![Item::filled(
            Shape::Rect {
                min: [8.0, 8.0],
                max: [120.0, 120.0],
            },
            ramp(),
        )
        // A blend against a constant, applied to the paint rather than to the
        // framebuffer -- which is the distinction this scene exists to show,
        // since the picture is the one the blend mode would give against a
        // flat destination of that color.
        .with_color_filter(ColorFilter::blend([0.2, 0.5, 1.0, 1.0], BlendMode::SrcIn))],
    ));

    scenes.push(plate(
        "blend/color-filter-matrix",
        vec![Item::filled(
            Shape::Rect {
                min: [8.0, 8.0],
                max: [120.0, 120.0],
            },
            ramp(),
        )
        .with_color_filter(ColorFilter::matrix([
            0.2126, 0.7152, 0.0722, 0.0, 0.0, //
            0.2126, 0.7152, 0.0722, 0.0, 0.0, //
            0.2126, 0.7152, 0.0722, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, 0.0,
        ]))],
    ));

    scenes.push(plate(
        "blend/gamma-encode",
        // The gamma pair over a ramp, which is where the curve is legible: a
        // linear ramp encoded into sRGB brightens most in the dark end, and
        // that is exactly the shape of the curve. A pair of flat swatches
        // would show two colors and say nothing about the function between
        // them.
        vec![Item::filled(
            Shape::Rect {
                min: [8.0, 8.0],
                max: [120.0, 120.0],
            },
            ramp(),
        )
        .with_color_filter(ColorFilter::linear_to_srgb())],
    ));

    scenes.push(plate(
        "blend/gamma-decode",
        // The other direction, over the same ramp, so the two plates read as
        // opposite bends of one curve when set side by side.
        vec![Item::filled(
            Shape::Rect {
                min: [8.0, 8.0],
                max: [120.0, 120.0],
            },
            ramp(),
        )
        .with_color_filter(ColorFilter::srgb_to_linear())],
    ));

    scenes.push(plate(
        "blend/clear-blend",
        vec![
            Item::fill(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                GREEN,
            ),
            // Clear takes neither operand, so this punches a hole rather than
            // drawing anything -- and the ground showing through it is the
            // whole picture.
            Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 34.0,
                },
                WHITE,
            )
            .with_blend(BlendMode::Clear),
        ],
    ));

    scenes.push(plate(
        "blend/color-wheel",
        (0..12)
            .map(|i| {
                let turn = i as f32 / 12.0 * std::f32::consts::TAU;
                let at = [64.0 + 26.0 * turn.cos(), 64.0 + 26.0 * turn.sin()];
                Item::fill(
                    Shape::Circle {
                        center: at,
                        radius: 26.0,
                    },
                    [
                        0.5 + 0.5 * turn.cos(),
                        0.5 + 0.5 * (turn + 2.094).cos(),
                        0.5 + 0.5 * (turn + 4.189).cos(),
                        1.0,
                    ],
                )
                .with_blend(BlendMode::Plus)
            })
            .collect(),
    ));

    scenes.push(
        Scene::tree(
            "blend/can-draw-paint-with-advanced-blend",
            // `drawPaint` twice, the second under a non-separable mode. Worth its
            // own plate because a paint has no shape: an advanced mode needs its
            // destination, and the destination here is the whole frame rather than
            // whatever a shape happens to cover. A renderer that read the
            // destination from a shape's bounds would be right on every other
            // plate in this chapter and wrong on this one.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([0.282, 0.820, 0.800, 1.0]),
                    blend: BlendMode::Src,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([1.0, 0.271, 0.0, 0.5]),
                    blend: BlendMode::Hue,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
            ],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    scenes.push(
        Scene::tree(
            "blend/destructive-blend-color-filter-floods-clip",
            // An empty group whose color filter replaces whatever it is given.
            // Nothing is drawn inside it, so the group is transparent -- and a
            // filter that ignores its input turns transparent into opaque red,
            // which then covers everything the group's bounds admit.
            //
            // The picture is the flood. A renderer that skipped an empty group as
            // an optimization, or applied the filter only where something had been
            // drawn, leaves the ground showing and is obviously wrong rather than
            // subtly so.
            //
            // Stated as a matrix because that is how a blend against a constant is
            // stated here: `Src` against red keeps none of its input, so every
            // coefficient is zero and the constant is the color. `docs/parity.md`
            // puts it as any blend against a constant that is affine in what it
            // blends, and a constant function is the affine one with no slope.
            vec![
                Node::Draw(Box::new(Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    [0.1, 0.2, 0.85, 1.0],
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        color_filter: ColorFilter::matrix([
                            0.0, 0.0, 0.0, 0.0, 1.0, //
                            0.0, 0.0, 0.0, 0.0, 0.0, //
                            0.0, 0.0, 0.0, 0.0, 0.0, //
                            0.0, 0.0, 0.0, 0.0, 1.0,
                        ]),
                        blend: BlendMode::SrcOver,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: Vec::new(),
                },
            ],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    scenes.push(plate(
        "blend/draw-advanced-blend-partly-offscreen",
        // An advanced mode where its destination runs out. The circle is
        // clipped along the bottom, so part of what it would blend with is not
        // there -- and a mode reading its destination has to find the ground
        // outside the clip rather than what a texture the size of the shape
        // happens to hold beyond its edge.
        //
        // The fill repeats so the boundary is legible: a solid one clipped
        // wrongly still looks like a circle with a straight edge.
        vec![
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                [0.1, 0.2, 0.85, 1.0],
            ),
            Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 52.0,
                },
                Fill::LinearGradient {
                    start: [0.0, 0.0],
                    end: [30.0, 30.0],
                    stops: vec![
                        Stop::new([0.957, 0.263, 0.212, 1.0], 0.0),
                        Stop::new([0.129, 0.588, 0.953, 1.0], 1.0),
                    ],
                    tile: TileMode::Repeat,
                },
            )
            .with_blend(BlendMode::Lighten)
            .with_clip([0.0, 0.0, 128.0, 90.0]),
        ],
    ));

    scenes
}

/// The whole sheet, in the destination it is usually drawn into.
const SHEET: [f32; 4] = [16.0, 16.0, 112.0, 112.0];
/// All of it.
const ALL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn sheet(rect: [f32; 4], source: [f32; 4], tile: TileMode, sampling: Sampling) -> Fill {
    Fill::Image {
        rect,
        source,
        tile,
        sampling,
        alpha: 1.0,
        tint: WHITE,
    }
}

/// The two stroke plates that put a circle's outline somewhere extreme.
///
/// Upstream draws each as four quadrants of the same shape -- filled, stroked,
/// both, and a filled circle of `radius + width / 2` beside them. That last one
/// is the assertion: a stroke's *outer* edge is a circle of exactly that
/// radius, so the two quadrants agree or the stroker is offsetting wrongly.
///
/// Upstream builds its circle as a path of four cubics, deliberately, so the
/// general stroker sees it. Here the same shape takes the fragment-evaluated
/// route instead, and that is worth having rather than working around: the
/// tessellated stroker is already covered by the corpus, and what these two
/// plates then say is that the *field* holds up where the numbers are hard --
/// a stroke half a device pixel wide under a twentyfold zoom, and one five
/// times wider than the shape it outlines.
fn extreme_strokes() -> Vec<Scene> {
    let quadrant =
        |cx: f32, cy: f32, zoom: f32, radius: f32, width: f32, items: &[(bool, [f32; 4])]| {
            items
                .iter()
                .map(|(stroked, color)| {
                    let shape = Shape::Circle {
                        center: [0.0, 0.0],
                        radius,
                    };
                    let item = if *stroked {
                        Item::stroke(shape, StrokeSpec::new(width), *color)
                    } else {
                        Item::fill(shape, *color)
                    };
                    Node::Draw(Box::new(item.with_transform(Transform {
                        scale: [zoom, zoom],
                        translate: [cx, cy],
                        ..Transform::default()
                    })))
                })
                .collect::<Vec<_>>()
        };

    vec![
        // A twentyfold zoom on a shape one unit across, so the stroke lands at
        // half a device pixel: below one, which is where a stroke is widened to
        // a pixel and dimmed to pay for it. That interaction is the reason this
        // plate is worth drawing at this zoom rather than a gentler one.
        Scene::tree(
            "basic/zoomed-stroked-path-renders-correctly",
            [
                quadrant(
                    34.0,
                    34.0,
                    20.0,
                    1.0,
                    0.025,
                    &[(false, BLUE), (true, GREEN)],
                ),
                quadrant(94.0, 34.0, 20.0, 1.0, 0.025, &[(false, BLUE)]),
                quadrant(34.0, 94.0, 20.0, 1.0, 0.025, &[(true, GREEN)]),
                // The comparison: a filled circle of the outer radius, which
                // the stroke's outer edge has to land on.
                quadrant(94.0, 94.0, 20.0, 1.0125, 0.0, &[(false, BLUE)]),
            ]
            .concat(),
        )
        .with_background(WHITE)
        .with_samples(4),
        // The other extreme: a stroke five times wider than the radius, so the
        // band's inner edge would be at a negative radius and the outline is a
        // disc with a hole that closed.
        Scene::tree(
            "basic/stroked-path-with-large-stroke-width-renders-correctly",
            [
                quadrant(34.0, 34.0, 1.0, 8.0, 40.0, &[(false, BLUE), (true, GREEN)]),
                quadrant(94.0, 34.0, 1.0, 8.0, 40.0, &[(false, BLUE)]),
                quadrant(34.0, 94.0, 1.0, 8.0, 40.0, &[(true, GREEN)]),
                quadrant(94.0, 94.0, 1.0, 28.0, 0.0, &[(false, BLUE)]),
            ]
            .concat(),
        )
        .with_background(WHITE)
        .with_samples(4),
    ]
}

/// The save-layer plates from `aiks_dl_basic_unittests.cc`.
///
/// Grouped because they are the same question asked four ways: where does a
/// layer's target sit, and what does it cover. That is arithmetic rather than a
/// picture -- `save_layer_bounds` gives the layer a target the size of the
/// region and offsets everything drawn into it -- and it is the kind of
/// arithmetic that draws something plausible when it is wrong.
fn save_layer_pictures() -> Vec<Scene> {
    // Upstream's coordinates, which already fit a plate this size.
    let everywhere = Shape::Rect {
        min: [0.0, 0.0],
        max: [128.0, 128.0],
    };
    let bounded = |bounds: [f32; 4], children: Vec<Node>| Node::Layer {
        layer: Box::new(LayerSpec::default()),
        bounds: Some(bounds),
        transform: Transform::default(),
        children,
    };
    let fill_all = |color: [f32; 4]| Node::Draw(Box::new(Item::fill(everywhere.clone(), color)));

    vec![
        // Three layers side by side, each bounded to a small square and each
        // filling the whole frame inside it. Nothing but the bounds decides
        // what shows, so a layer that ignored them would paint the frame three
        // times and leave one flat color.
        Scene::tree(
            "basic/sibling-save-layer-bounds-are-respected",
            vec![
                bounded(
                    [25.0, 25.0, 50.0, 50.0],
                    vec![fill_all([0.0, 0.0, 0.0, 1.0])],
                ),
                bounded([35.0, 35.0, 60.0, 60.0], vec![fill_all(GREEN)]),
                bounded([45.0, 45.0, 70.0, 70.0], vec![fill_all(RED)]),
            ],
        )
        .with_background(DARK)
        .with_samples(4),
        // One layer bounded to a quarter of what is drawn into it, with three
        // overlapping squares inside. The bounds cut the corner off all three,
        // and the order inside survives being cut: blue over green over red.
        Scene::tree(
            "basic/can-perform-save-layer-with-bounds",
            vec![bounded(
                [0.0, 0.0, 50.0, 50.0],
                vec![
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [0.0, 0.0],
                            max: [100.0, 100.0],
                        },
                        RED,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [10.0, 10.0],
                            max: [110.0, 110.0],
                        },
                        GREEN,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [20.0, 20.0],
                            max: [120.0, 120.0],
                        },
                        BLUE,
                    ))),
                ],
            )],
        )
        .with_background(DARK)
        .with_samples(4),
        // A layer with nothing in it, composited with a mode that discards its
        // destination. Empty is not the same as absent: the layer covers its
        // bounds whether or not anything was drawn into it, so the mode acts on
        // that whole region and cuts a hole in the image behind it.
        //
        // Upstream states the region with a clip where this states it with the
        // layer's bounds, which the scene format puts on a layer and not on a
        // clip of its own. Same region, same picture.
        Scene::tree(
            "basic/empty-save-layer-renders-with-clear",
            vec![
                Node::Draw(Box::new(Item::filled(
                    everywhere.clone(),
                    sheet(ALL, ALL, TileMode::Clamp, Sampling::Linear),
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        blend: BlendMode::Clear,
                        ..LayerSpec::default()
                    }),
                    bounds: Some([40.0, 40.0, 90.0, 90.0]),
                    transform: Transform::default(),
                    children: Vec::new(),
                },
            ],
        )
        .with_background(DARK)
        .with_samples(1),
        // A layer on its own with nothing under it, at half opacity. The
        // simplest thing a save layer does, and the catalog had no plate for
        // it: group opacity over a ground, with one shape inside so the
        // half-alpha is the layer's and not the shape's.
        Scene::tree(
            "basic/can-save-layer-standalone",
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(Item::fill(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 56.0,
                    },
                    RED,
                )))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        // A clip of a shape a scissor cannot express, then a bounded layer
        // inside it, then an advanced blend inside that. Three things that each
        // narrow or divert what is drawn, stacked: the clip goes to the
        // stencil, the layer to a target of its own, and the blend needs its
        // destination -- and the destination it needs is the layer's, not the
        // frame's.
        Scene::tree(
            "basic/can-render-clipped-layers",
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid(WHITE),
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Layer {
                    layer: Box::new(LayerSpec::default()),
                    bounds: Some([25.0, 25.0, 75.0, 75.0]),
                    transform: Transform::default(),
                    children: vec![
                        Node::Draw(Box::new(
                            Item::fill(everywhere.clone(), WHITE).with_clip_shape(Shape::Circle {
                                center: [50.0, 50.0],
                                radius: 25.0,
                            }),
                        )),
                        Node::Draw(Box::new(
                            Item::fill(everywhere.clone(), GREEN)
                                .with_clip_shape(Shape::Circle {
                                    center: [50.0, 50.0],
                                    radius: 25.0,
                                })
                                .with_blend(BlendMode::HardLight),
                        )),
                    ],
                },
            ],
        )
        .with_background(DARK)
        .with_samples(1),
        // The one that is about the origin rather than the extent. A bounded
        // layer's target starts where the bounds start, so everything drawn
        // into it has to be placed against that origin rather than against the
        // frame's -- and a renderer that forgot would slide the contents by the
        // bounds' offset and still draw three squares. The yellow outline is
        // where the bounds are, so the two can be read against each other.
        Scene::tree(
            "basic/coverage-origin-should-be-accounted-for-in-subpasses",
            vec![
                Node::Draw(Box::new(
                    Item::fill(
                        Shape::Rect {
                            min: [20.0, 20.0],
                            max: [80.0, 80.0],
                        },
                        [1.0, 1.0, 0.0, 1.0],
                    )
                    .with_stroke(StrokeSpec::new(2.5)),
                )),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        alpha: 0.5,
                        ..LayerSpec::default()
                    }),
                    bounds: Some([20.0, 20.0, 80.0, 80.0]),
                    transform: Transform::default(),
                    children: vec![
                        Node::Draw(Box::new(Item::fill(
                            Shape::Rect {
                                min: [12.0, 12.0],
                                max: [62.0, 62.0],
                            },
                            RED,
                        ))),
                        Node::Draw(Box::new(Item::fill(
                            Shape::Rect {
                                min: [25.0, 25.0],
                                max: [75.0, 75.0],
                            },
                            GREEN,
                        ))),
                        Node::Draw(Box::new(Item::fill(
                            Shape::Rect {
                                min: [37.0, 37.0],
                                max: [87.0, 87.0],
                            },
                            BLUE,
                        ))),
                    ],
                },
            ],
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}

/// The rounded-rectangle plates that eight radii unblocked.
///
/// `docs/playground-parity.md` records five basic-chapter scenes that a single
/// circular radius could not describe, and records the limit being built rather
/// than kept. What it did not record is that the scenes stayed unwritten
/// afterwards, which is the ordinary way a built capability goes unexercised:
/// nothing fails when a picture nobody drew is missing.
///
/// Coordinates are a fifth of upstream's, which is what fits shapes drawn at
/// five hundred points onto a plate of a hundred and twenty-eight.
fn rounded_rect_radii() -> Vec<Scene> {
    const FIFTH: f32 = 0.2;
    let f = |v: f32| v * FIFTH;

    vec![
        // Upstream's four corners, no two of which are the same pair, and none
        // circular: fifty by twenty-five and its transpose, arranged so the
        // shape is symmetric across both diagonals. A renderer that read the
        // pair in the wrong order draws the transpose, which is a different
        // shape and is still a plausible rounded rectangle -- which is why the
        // symmetry matters. It is symmetric under swapping *both* members of
        // every pair and not under swapping the corners.
        plate(
            "basic/can-render-rounded-rect-with-non-uniform-radii",
            vec![Item::fill(
                Shape::RoundedRectWithRadii {
                    min: [f(100.0), f(100.0)],
                    max: [f(600.0), f(600.0)],
                    radii: [
                        [f(50.0), f(25.0)],
                        [f(25.0), f(50.0)],
                        [f(25.0), f(50.0)],
                        [f(50.0), f(25.0)],
                    ],
                },
                RED,
            )],
        ),
        // `NoDimplesInRRectPath` at its sliders' defaults, which is where it is
        // interesting: a corner radius of fifty across and a hundred down on a
        // rectangle sixty tall. The y radius overruns half the height by more
        // than three times, so every radius is scaled by `dart:ui`'s rule, and
        // the dimple the scene is named for is what appears when the scaling is
        // applied per corner instead of once for the whole shape.
        Scene::tree(
            "basic/no-dimples-in-r-rect-path",
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([0.1, 0.1, 0.1, 1.0]),
                    blend: BlendMode::SrcOver,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Draw(Box::new(
                    // A half rather than the fifth the others use: this shape
                    // is two hundred by sixty where they are five hundred
                    // square, and at a fifth its corners would be four pixels
                    // across.
                    Item::filled(
                        Shape::RoundedRectWithRadii {
                            min: [14.0, 49.0],
                            max: [114.0, 79.0],
                            radii: [[25.0, 50.0]; 4],
                        },
                        Fill::LinearGradient {
                            start: [14.0, 49.0],
                            end: [114.0, 149.0],
                            stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                            tile: TileMode::Clamp,
                        },
                    )
                    .with_stroke(StrokeSpec::new(10.0)),
                )),
            ],
        )
        .with_background(DARK)
        .with_samples(4),
        // The ring drawn twice: once as a difference of two rounded rectangles,
        // and once as the outer one with the inner cleared out of it. The two
        // are the same picture, which is the whole of the scene -- an even-odd
        // fill and a destination-clearing blend arrive at it by routes that
        // share nothing, so a bug in either shows as the pair disagreeing.
        //
        // The radii ascend around the outer rectangle -- five, ten, twenty,
        // fifty -- and the inner ones are each five less, which is what makes
        // the ring an even thickness at four different corner curvatures.
        {
            let ring = |dx: f32| {
                let dy = f(50.0);
                let o = [[dx + f(0.0), dy], [dx + f(100.0), dy + f(100.0)]];
                let i = [[dx + f(5.0), dy + f(5.0)], [dx + f(95.0), dy + f(95.0)]];
                let outer_radii = [[f(5.0); 2], [f(10.0); 2], [f(20.0); 2], [f(50.0); 2]];
                let inner_radii = [[f(0.0); 2], [f(5.0); 2], [f(15.0); 2], [f(45.0); 2]];
                (o, outer_radii, i, inner_radii)
            };
            let (o, orad, i, irad) = ring(f(60.0));
            let (o2, orad2, i2, irad2) = ring(f(360.0));
            Scene::tree(
                "basic/compare-diff-round-rect-and-round-rect",
                vec![
                    Node::Paint(Box::new(PaintSpec {
                        fill: Fill::Solid([0.0, 0.0, 0.0, 1.0]),
                        blend: BlendMode::Src,
                        clip: None,
                        clip_out: None,
                        transform: Transform::default(),
                    })),
                    Node::Draw(Box::new(Item::fill(
                        Shape::DiffRoundedRectWithRadii {
                            outer: o,
                            outer_radii: orad,
                            inner: i,
                            inner_radii: irad,
                        },
                        SKY_BLUE,
                    ))),
                    Node::Draw(Box::new(Item::fill(
                        Shape::RoundedRectWithRadii {
                            min: o2[0],
                            max: o2[1],
                            radii: orad2,
                        },
                        SKY_BLUE,
                    ))),
                    Node::Draw(Box::new(
                        Item::fill(
                            Shape::RoundedRectWithRadii {
                                min: i2[0],
                                max: i2[1],
                                radii: irad2,
                            },
                            SKY_BLUE,
                        )
                        .with_blend(BlendMode::Clear),
                    )),
                ],
            )
            .with_background(DARK)
            .with_samples(4)
        },
    ]
}

/// Five more of `aiks_dl_basic_unittests.cc`, none of which needed anything
/// built.
///
/// The row for that file has been short by a large number of ordinary pictures
/// for a while, and the reason is that they are ordinary: nothing here is
/// blocked, so nothing was forced. What they cover is not nothing, though --
/// two of them are the only plates that draw a paint with no shape at all, and
/// one is the only one that shears.
fn basic_pictures() -> Vec<Scene> {
    // Upstream's two, kept as they are written there.
    const TURQUOISE: [f32; 4] = [72.0 / 255.0, 209.0 / 255.0, 204.0 / 255.0, 1.0];
    const ORANGE_RED: [f32; 4] = [1.0, 69.0 / 255.0, 0.0, 0.5];

    let paint = |color: [f32; 4]| {
        Node::Paint(Box::new(PaintSpec {
            fill: Fill::Solid(color),
            blend: BlendMode::SrcOver,
            clip: None,
            clip_out: None,
            transform: Transform::default(),
        }))
    };

    // `CanRenderSimpleClips` at a fifth of upstream's coordinates, which is
    // what fits its three groups on a plate this size. Upstream scales by two
    // when it draws them, so the shapes are the same shapes.
    const FIFTH: f32 = 0.2;
    let at = |l: f32, t: f32, r: f32, b: f32, dx: f32, dy: f32| {
        [
            l * FIFTH + dx,
            t * FIFTH + dy,
            r * FIFTH + dx,
            b * FIFTH + dy,
        ]
    };
    let clipped_group = |fill: Fill, dx: f32, dy: f32| -> Vec<Node> {
        // Each region is `drawPaint` under a clip. A paint has no shape, so
        // what it covers is the clip -- which is why an item filling the whole
        // plate under a clip shape is the same picture and not an
        // approximation of it.
        let whole = || Shape::Rect {
            min: [0.0, 0.0],
            max: [128.0, 128.0],
        };
        let rect = at(50.0, 50.0, 150.0, 150.0, dx, dy);
        let oval = at(200.0, 50.0, 300.0, 150.0, dx, dy);
        let tall = at(50.0, 200.0, 150.0, 300.0, dx, dy);
        let wide = at(200.0, 230.0, 300.0, 270.0, dx, dy);
        let narrow = at(230.0, 200.0, 270.0, 300.0, dx, dy);
        let radius = 20.0 * FIFTH;
        let rounded = |r: [f32; 4]| Shape::RoundedRect {
            min: [r[0], r[1]],
            max: [r[2], r[3]],
            radius,
        };
        vec![
            Node::Draw(Box::new(
                Item::filled(whole(), fill.clone()).with_clip(rect),
            )),
            Node::Draw(Box::new(
                Item::filled(whole(), fill.clone()).with_clip_shape(Shape::Oval {
                    min: [oval[0], oval[1]],
                    max: [oval[2], oval[3]],
                }),
            )),
            // The last three are rounded rectangles whose radius is large
            // against one side and small against the other, which is where a
            // corner that normalized the wrong way round would show.
            Node::Draw(Box::new(
                Item::filled(whole(), fill.clone()).with_clip_shape(rounded(tall)),
            )),
            Node::Draw(Box::new(
                Item::filled(whole(), fill.clone()).with_clip_shape(rounded(wide)),
            )),
            Node::Draw(Box::new(
                Item::filled(whole(), fill).with_clip_shape(rounded(narrow)),
            )),
        ]
    };

    // Upstream's seven-stop mirror-tiled radial, which is the second of its
    // three fills.
    let sunset = Fill::RadialGradient {
        center: [100.0, 120.0],
        radius: 15.0,
        stops: vec![
            Stop::new([0x1f as f32 / 255.0, 0.0, 0x5c as f32 / 255.0, 1.0], 0.0),
            Stop::new(
                [0x5b as f32 / 255.0, 0.0, 0x60 as f32 / 255.0, 1.0],
                1.0 / 6.0,
            ),
            Stop::new(
                [
                    0x87 as f32 / 255.0,
                    0x01 as f32 / 255.0,
                    0x60 as f32 / 255.0,
                    1.0,
                ],
                2.0 / 6.0,
            ),
            Stop::new(
                [
                    0xac as f32 / 255.0,
                    0x25 as f32 / 255.0,
                    0x53 as f32 / 255.0,
                    1.0,
                ],
                3.0 / 6.0,
            ),
            Stop::new(
                [
                    0xe1 as f32 / 255.0,
                    0x6b as f32 / 255.0,
                    0x5c as f32 / 255.0,
                    1.0,
                ],
                4.0 / 6.0,
            ),
            Stop::new(
                [
                    0xf3 as f32 / 255.0,
                    0x90 as f32 / 255.0,
                    0x60 as f32 / 255.0,
                    1.0,
                ],
                5.0 / 6.0,
            ),
            Stop::new([1.0, 0xb5 as f32 / 255.0, 0x6b as f32 / 255.0, 1.0], 1.0),
        ],
        tile: TileMode::Mirror,
    };

    let square = Shape::Rect {
        min: [25.0, 25.0],
        max: [50.0, 50.0],
    };
    let moved = |shape: &Shape, by: f32| match shape {
        Shape::Rect { min, max } => Shape::Rect {
            min: [min[0] + by, min[1] + by],
            max: [max[0] + by, max[1] + by],
        },
        other => other.clone(),
    };

    vec![
        // A paint with no shape, which is the whole of what this one is. The
        // scale upstream applies before it makes no difference and is left
        // out: what a paint covers is the clip, and neither has one.
        Scene::tree("basic/can-draw-paint", vec![paint(TURQUOISE)])
            .with_background(DARK)
            .with_samples(4),
        // The same twice, the second translucent. Distinct from the plate in
        // the blend chapter that draws these two colors under `Hue`: this is
        // the ordinary composite, and it is what says the second paint reaches
        // the whole frame rather than whatever the first one's bounds were.
        Scene::tree(
            "basic/can-draw-paint-multiple-times",
            vec![paint(TURQUOISE), paint(ORANGE_RED)],
        )
        .with_background(DARK)
        .with_samples(4),
        // The only plate that shears. Upstream's coefficients on a fifth of
        // upstream's square, which is what keeps the far corner on the plate:
        // two and five are steep, and the corner travels five times its own
        // height down the frame.
        plate(
            "basic/can-perform-skew",
            vec![Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [20.0, 20.0],
                },
                RED,
            )
            .with_transform(Transform {
                skew: [2.0, 5.0],
                ..Transform::default()
            })],
        ),
        Scene::tree(
            "basic/can-render-simple-clips",
            [
                // Upstream's white ground, which the clipped regions are read
                // against.
                vec![paint(WHITE)],
                clipped_group(Fill::Solid(BLUE), 0.0, 0.0),
                clipped_group(sunset, 0.0, 60.0),
                clipped_group(
                    sheet(
                        [0.0, 0.0, 32.0, 32.0],
                        ALL,
                        TileMode::Repeat,
                        Sampling::Nearest,
                    ),
                    60.0,
                    0.0,
                ),
            ]
            .concat(),
        )
        .with_background(DARK)
        .with_samples(4),
        // Three squares stepped down the diagonal, the middle one inside a
        // layer of its own. What it is for is the order: a layer composites
        // when it is restored, so the square inside it goes *behind* the one
        // drawn after it and in front of the one drawn before -- and a
        // renderer that composited layers last would put it in front of both.
        Scene::tree(
            "basic/save-layer-draws-behind-subsequent-entities",
            vec![
                Node::Draw(Box::new(Item::fill(square.clone(), [0.0, 0.0, 0.0, 1.0]))),
                Node::Layer {
                    layer: Box::new(LayerSpec::default()),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(Item::fill(
                        moved(&square, 10.0),
                        GREEN,
                    )))],
                },
                Node::Draw(Box::new(Item::fill(moved(&square, 20.0), RED))),
            ],
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}

/// The texture scenes from `aiks_dl_basic_unittests.cc`.
///
/// Kept apart from the rest of that file's plates only because they are the
/// ones that need the fixture, which is worth being able to see in one place.
fn image() -> Vec<Scene> {
    let whole = Shape::Rect {
        min: [0.0, 0.0],
        max: [128.0, 128.0],
    };
    // A destination smaller than the shape it fills, so every tile mode has
    // something outside it to act on.
    let quarter: [f32; 4] = [40.0, 40.0, 88.0, 88.0];

    vec![
        plate(
            "basic/can-render-image",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
            )],
        ),
        plate(
            "basic/image-cubic-sampling",
            // A small piece of the sheet magnified hard, which is where the
            // three qualities actually differ: at or near an image's own size a
            // cubic read and a linear one agree to within the target's
            // precision, and only a strong magnification separates them.
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                sheet(
                    [48.0, 48.0, 80.0, 80.0],
                    ALL,
                    TileMode::Clamp,
                    Sampling::Cubic,
                ),
            )],
        ),
        plate(
            "basic/image-mipmap-sampling",
            // The sheet drawn at half its size and repeated across the plate,
            // which is the only arrangement that makes minification legible at
            // this scale: eight texels land on four pixels, so the level built
            // for that size is the one read. Every other quality here reads the
            // sheet itself and shows the moire a point sample of a repeating
            // pattern gives.
            vec![Item::filled(
                whole.clone(),
                sheet(
                    [0.0, 0.0, 4.0, 4.0],
                    ALL,
                    TileMode::Repeat,
                    Sampling::Mipmap,
                ),
            )],
        ),
        plate(
            "basic/image-mipmap-sampling-magnified",
            // The same quality where there is nothing above the sheet to read,
            // so it has to come out as the linear plate does. A level chosen
            // even slightly above zero would show here as a softened sheet, and
            // this is the plate a reader would compare against to see it.
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                sheet(SHEET, ALL, TileMode::Clamp, Sampling::Mipmap),
            )],
        ),
        plate(
            "basic/image-nearest-sampling",
            // The same piece at the same magnification with no reconstruction
            // at all, so the pair reads as the two ends of what the setting
            // does.
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                sheet(
                    [48.0, 48.0, 80.0, 80.0],
                    ALL,
                    TileMode::Clamp,
                    Sampling::Nearest,
                ),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-clamp",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Clamp, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-repeat",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Repeat, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-mirror",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Mirror, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-decal",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Decal, Sampling::Linear),
            )],
        ),
        plate(
            "basic/can-render-tiled-texture-clamp-with-translate",
            vec![Item::filled(
                whole.clone(),
                sheet(quarter, ALL, TileMode::Clamp, Sampling::Linear),
            )
            // The mapping travels with the item, so the clamped edges move
            // with it rather than staying where the rectangle was written.
            .with_transform(Transform {
                translate: [18.0, -12.0],
                ..Transform::default()
            })],
        ),
        plate(
            "basic/can-render-image-rect",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                // One quadrant of the sheet across the whole destination,
                // which is what a sprite out of a sheet is.
                sheet(
                    SHEET,
                    [0.0, 0.0, 0.5, 0.5],
                    TileMode::Clamp,
                    Sampling::Linear,
                ),
            )],
        ),
        plate(
            "basic/draw-image-rect-src-outside-bounds",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                // A source running past the sheet's own edge, which clamping
                // has to answer without reading anything that is not there.
                sheet(
                    SHEET,
                    [0.5, 0.5, 1.6, 1.6],
                    TileMode::Clamp,
                    Sampling::Linear,
                ),
            )],
        ),
        plate(
            "basic/can-render-inverted-image-with-color-filter",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
            )
            .with_color_filter(ColorFilter::matrix([
                -1.0, 0.0, 0.0, 0.0, 1.0, //
                0.0, -1.0, 0.0, 0.0, 1.0, //
                0.0, 0.0, -1.0, 0.0, 1.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]))],
        ),
        plate(
            "basic/image-color-source-effect-transform",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 52.0,
                },
                sheet(SHEET, ALL, TileMode::Repeat, Sampling::Linear),
            )
            .with_transform(Transform {
                rotate: 0.6,
                scale: [1.3, 0.8],
                ..Transform::default()
            })],
        ),
    ]
}

/// A triangle, as three corners of the plate.
fn triangle() -> Vec<[f32; 2]> {
    vec![[64.0, 16.0], [116.0, 108.0], [12.0, 108.0]]
}

fn mesh_group(name: &'static str, specs: Vec<MeshSpec>) -> Scene {
    Scene::tree(
        name,
        specs.into_iter().map(|s| Node::Mesh(Box::new(s))).collect(),
    )
    .with_background(DARK)
    .with_samples(4)
}

fn mesh(name: &'static str, spec: MeshSpec) -> Scene {
    Scene::tree(name, vec![Node::Mesh(Box::new(spec))])
        .with_background(DARK)
        .with_samples(4)
}

fn mesh_of(positions: Vec<[f32; 2]>, fill: Fill) -> MeshSpec {
    MeshSpec {
        tint_blend: BlendMode::Modulate,
        mode: VertexMode::Triangles,
        positions,
        colors: Vec::new(),
        texture_coords: Vec::new(),
        indices: Vec::new(),
        fill,
        blend: BlendMode::SrcOver,
        transform: Transform::default(),
        image_filter: ImageFilter::None,
        mask_blur: 0.0,
    }
}

/// `aiks_dl_vertices_unittests.cc`.
fn vertices() -> Vec<Scene> {
    let quad = vec![[16.0, 16.0], [112.0, 16.0], [112.0, 112.0], [16.0, 112.0]];
    let corners = vec![
        ALL_CORNERS[0],
        ALL_CORNERS[1],
        ALL_CORNERS[2],
        ALL_CORNERS[3],
    ];

    vec![
        mesh(
            "vertices/mesh-through-an-image-filter",
            // A mesh does not pass through the call where a paint's image
            // filter is noticed, so it had its own route added and this is the
            // plate that holds the two backends to the same answer on it. A
            // dilation, because its reach is exact: the triangle grows by ten
            // and nothing about the picture is a matter of taste.
            MeshSpec {
                image_filter: ImageFilter::Dilate {
                    radius_x: 10.0,
                    radius_y: 10.0,
                },
                ..mesh_of(
                    vec![[34.0, 88.0], [64.0, 30.0], [94.0, 88.0]],
                    Fill::Solid(YELLOW),
                )
            },
        ),
        mesh(
            "vertices/mesh-through-a-blur",
            // The other kind of filter over a mesh, where what is checked is
            // not an extent but a gradient of coverage -- two rasterizers can
            // agree about where a dilation ends and still disagree about how a
            // blur falls off.
            MeshSpec {
                image_filter: ImageFilter::blur(6.0),
                ..mesh_of(
                    vec![[34.0, 88.0], [64.0, 30.0], [94.0, 88.0]],
                    Fill::Solid(GREEN),
                )
            },
        ),
        mesh(
            "vertices/draw-vertices-solid-color-triangles-without-indices",
            mesh_of(triangle(), Fill::Solid(BLUE)),
        ),
        mesh(
            "vertices/draw-vertices-solid-color-triangles-with-indices",
            MeshSpec {
                positions: quad.clone(),
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(Vec::new(), Fill::Solid(GREEN))
            },
        ),
        mesh(
            "vertices/can-convert-triangle-fan-to-triangles",
            MeshSpec {
                mode: VertexMode::TriangleFan,
                positions: fan(),
                ..mesh_of(Vec::new(), Fill::Solid(YELLOW))
            },
        ),
        mesh(
            "vertices/draw-vertices-linear-gradient-without-indices",
            mesh_of(triangle(), ramp()),
        ),
        mesh(
            "vertices/draw-vertices-linear-gradient-with-texture-coordinates",
            MeshSpec {
                positions: vec![[24.0, 104.0], [64.0, 24.0], [104.0, 104.0]],
                // Coordinates that are not the positions, and not a
                // rearrangement of them either. A gradient on a mesh is read at
                // the coordinates rather than at the geometry, so the ramp runs
                // across the triangle in a direction the triangle's own shape
                // does not suggest -- and a renderer sampling by position
                // instead would draw a plausible gradient pointing the wrong
                // way.
                texture_coords: vec![[104.0, 24.0], [24.0, 64.0], [104.0, 104.0]],
                ..mesh_of(
                    Vec::new(),
                    Fill::LinearGradient {
                        start: [24.0, 24.0],
                        end: [104.0, 104.0],
                        stops: vec![Stop::new(BLUE, 0.0), Stop::new(RED, 1.0)],
                        tile: TileMode::Repeat,
                    },
                )
            },
        ),
        mesh(
            "vertices/vertices-geometry-color-uv-position-data-advanced-blend",
            MeshSpec {
                positions: quad.clone(),
                // The same six vertices and the same half-alpha colors as the
                // separable plate above, combined into the fill by a mode that
                // reads its destination. An advanced mode over a per-vertex
                // color is where the two things this renderer keeps apart --
                // the tint blend and the draw's own blend -- are easiest to
                // confuse, because both are blends and only one of them is
                // this one.
                colors: corners.iter().map(|c| [c[0], c[1], c[2], 0.5]).collect(),
                indices: vec![0, 1, 2, 0, 2, 3],
                tint_blend: BlendMode::ColorBurn,
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh(
            "vertices/vertices-geometry-color-uv-position-data",
            MeshSpec {
                positions: quad.clone(),
                // A color at each corner, which no gradient describes: the
                // four are independent and the interior is all of them at once.
                colors: corners.clone(),
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(Vec::new(), Fill::Solid(WHITE))
            },
        ),
        mesh(
            "vertices/draw-vertices-premultiplies-colors",
            MeshSpec {
                positions: quad.clone(),
                // Alpha varying between the corners, which is the case that
                // tells a premultiplied interpolation from a straight one: the
                // transparent end fades toward nothing rather than staying
                // bright and only thinning.
                colors: vec![
                    [1.0, 0.3, 0.1, 1.0],
                    [1.0, 0.3, 0.1, 0.0],
                    [1.0, 0.3, 0.1, 0.0],
                    [1.0, 0.3, 0.1, 1.0],
                ],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(Vec::new(), Fill::Solid(WHITE))
            },
        ),
        mesh(
            "vertices/vertices-geometry-uv-position-data",
            MeshSpec {
                positions: quad.clone(),
                texture_coords: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh(
            "vertices/draw-vertices-image-source-with-texture-coordinates",
            MeshSpec {
                positions: quad.clone(),
                // Mirrored coordinates, so the sheet lands the other way round
                // from where the geometry sits -- which is what says the
                // coordinates were read rather than the position.
                texture_coords: vec![[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh(
            "vertices/draw-vertices-image-source-with-texture-coordinates-and-color-blending",
            MeshSpec {
                positions: quad,
                texture_coords: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                colors: corners,
                indices: vec![0, 1, 2, 0, 2, 3],
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh_group(
            "vertices/vertex-colors-combined-by-each-separable-mode",
            // Twelve modes over one fill, each on its own quad whose four vertices carry
            // four different colors -- so every mode is exercised across a gradient of
            // sources rather than at one value, where several of them agree.
            vec![
                MeshSpec {
                    tint_blend: BlendMode::Multiply,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[4.0, 4.0], [31.0, 4.0], [31.0, 31.0], [4.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Screen,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[35.0, 4.0], [62.0, 4.0], [62.0, 31.0], [35.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Overlay,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[66.0, 4.0], [93.0, 4.0], [93.0, 31.0], [66.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Darken,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[97.0, 4.0], [124.0, 4.0], [124.0, 31.0], [97.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Lighten,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[4.0, 35.0], [31.0, 35.0], [31.0, 62.0], [4.0, 62.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::ColorDodge,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[35.0, 35.0], [62.0, 35.0], [62.0, 62.0], [35.0, 62.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::ColorBurn,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[66.0, 35.0], [93.0, 35.0], [93.0, 62.0], [66.0, 62.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::HardLight,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[97.0, 35.0], [124.0, 35.0], [124.0, 62.0], [97.0, 62.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::SoftLight,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[4.0, 66.0], [31.0, 66.0], [31.0, 93.0], [4.0, 93.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Difference,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[35.0, 66.0], [62.0, 66.0], [62.0, 93.0], [35.0, 93.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Exclusion,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[66.0, 66.0], [93.0, 66.0], [93.0, 93.0], [66.0, 93.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Plus,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[97.0, 66.0], [124.0, 66.0], [124.0, 93.0], [97.0, 93.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
            ],
        ),
        mesh_group(
            "vertices/vertex-colors-combined-by-each-nonseparable-mode",
            // The four modes that mix channels rather than acting on each alone. They
            // are the half most likely to translate differently, carrying a sort and a
            // luminosity clip that the separable ones do not.
            vec![
                MeshSpec {
                    tint_blend: BlendMode::Hue,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[4.0, 4.0], [31.0, 4.0], [31.0, 31.0], [4.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Saturation,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[35.0, 4.0], [62.0, 4.0], [62.0, 31.0], [35.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Color,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[66.0, 4.0], [93.0, 4.0], [93.0, 31.0], [66.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
                MeshSpec {
                    tint_blend: BlendMode::Luminosity,
                    colors: vec![RED, GREEN, BLUE, WHITE],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        vec![[97.0, 4.0], [124.0, 4.0], [124.0, 31.0], [97.0, 31.0]],
                        Fill::Solid([0.25, 0.6, 0.9, 1.0]),
                    )
                },
            ],
        ),
        mesh(
            "vertices/a-strip-and-a-fan-cover-what-a-list-would",
            // `TriangleStrip` was in no plate, so the index expansion that
            // makes a strip out of a run of points had never been put in front
            // of both backends. A zigzag, because a straight strip is
            // degenerate: its triangles enclose nothing, which is a picture
            // that agrees with every wrong answer.
            MeshSpec {
                mode: VertexMode::TriangleStrip,
                colors: vec![RED, GREEN, BLUE, WHITE, GREEN, RED],
                ..mesh_of(
                    (0..6)
                        .map(|i| [10.0 + i as f32 * 21.0, if i % 2 == 0 { 16.0 } else { 58.0 }])
                        .collect(),
                    Fill::Solid(WHITE),
                )
            },
        ),
        mesh(
            "vertices/a-fan-around-one-point",
            // Every triangle shares the first position, so the picture is a
            // wedge rather than a band and a fan drawn as a strip is visibly
            // not this.
            MeshSpec {
                mode: VertexMode::TriangleFan,
                colors: vec![WHITE, RED, GREEN, BLUE, GREEN, RED],
                ..mesh_of(
                    {
                        let mut points = vec![[64.0, 118.0]];
                        points.extend((0..5).map(|i| {
                            let t = 0.6 + i as f32 * 0.45;
                            [64.0 + 48.0 * t.cos(), 118.0 - 48.0 * t.sin()]
                        }));
                        points
                    },
                    Fill::Solid(WHITE),
                )
            },
        ),
        mesh(
            "vertices/a-mesh-filled-by-a-runtime-effect",
            // The inventory listed runtime effects as blocking this file. They
            // do not: a mesh without texture coordinates takes its material
            // from the paint's shader like any other geometry, and a caller's
            // program is one of those. What is refused is a *textured* mesh
            // under one, since the coordinates would have nothing to read
            // from -- and that refusal is a stated error rather than a gap.
            mesh_of(
                vec![
                    [12.0, 20.0],
                    [116.0, 44.0],
                    [12.0, 68.0],
                    [116.0, 60.0],
                    [12.0, 84.0],
                    [116.0, 108.0],
                ],
                Fill::RuntimeEffect {
                    program: 0,
                    uniforms: crate::fixture::effect_uniforms(RED, BLUE, 0.0),
                    images: Vec::new(),
                },
            ),
        ),
        Scene::tree(
            "vertices/draw-vertices-texture-coordinates-with-fragment-shader",
            // Four quads tiling one square, each stating texture coordinates
            // that are its own share of a single range, all filled by one
            // caller's program that ramps along them.
            //
            // The picture is the assertion. Read together the four make one
            // continuous ramp; a renderer where the coordinate did not reach
            // the program would draw the whole ramp four times over, with three
            // seams down the middle, because each quad's own clip position runs
            // the same way as every other's.
            //
            // Upstream's is four `DrawVertices` calls with
            // `texture_coordinates` set to the positions. Here the coordinates
            // are those positions over the square's extent, since the program
            // ramps across nought to one rather than across a hundred pixels --
            // the same statement in the units the fixture reads.
            //
            // This scene is why `docs/non-parity.md` section 14 is gone. It had
            // recorded that a program cannot be read at a mesh's coordinates,
            // on the reasoning that a program replaces the fragment shader and
            // leaves the attribute nowhere to arrive. The vertex stage is this
            // renderer's own whatever the fragment does, and it hands on `uv`.
            [
                ([0.0, 0.0], [60.0, 60.0]),
                ([60.0, 0.0], [120.0, 60.0]),
                ([0.0, 60.0], [60.0, 120.0]),
                ([60.0, 60.0], [120.0, 120.0]),
            ]
            .into_iter()
            .map(|(min, max)| {
                let corners = [
                    [min[0], min[1]],
                    [max[0], min[1]],
                    [max[0], max[1]],
                    [min[0], max[1]],
                ];
                Node::Mesh(Box::new(MeshSpec {
                    positions: corners.to_vec(),
                    // The same points, over the square the four of them cover.
                    texture_coords: corners
                        .iter()
                        .map(|p| [p[0] / 120.0, p[1] / 120.0])
                        .collect(),
                    indices: vec![0, 1, 2, 0, 2, 3],
                    ..mesh_of(
                        Vec::new(),
                        Fill::RuntimeEffect {
                            program: 3,
                            uniforms: crate::fixture::effect_uniforms(RED, BLUE, 0.0),
                            images: Vec::new(),
                        },
                    )
                }))
            })
            .collect(),
        )
        .with_background(DARK)
        .with_samples(4),
        mesh(
            "vertices/a-mesh-under-a-transform-filled-by-a-runtime-effect",
            // The same triangles turned. The program splits on the clip-space
            // coordinate, so the split stays vertical through the middle of
            // the target while the geometry it covers does not -- which is the
            // picture that says the transform reached the vertices and not the
            // fragment program, and they are separate stages for that reason.
            MeshSpec {
                transform: Transform {
                    rotate: 0.6,
                    translate: [64.0, 64.0],
                    ..Transform::default()
                },
                ..mesh_of(
                    vec![
                        [-52.0, -34.0],
                        [52.0, -34.0],
                        [-52.0, 6.0],
                        [52.0, 10.0],
                        [-52.0, 34.0],
                        [52.0, 34.0],
                    ],
                    Fill::RuntimeEffect {
                        program: 0,
                        uniforms: crate::fixture::effect_uniforms(GREEN, BLUE, 0.0),
                        images: Vec::new(),
                    },
                )
            },
        ),
        mesh(
            "vertices/vertices-geometry-uv-position-data-with-translate",
            MeshSpec {
                positions: triangle(),
                texture_coords: vec![[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                transform: Transform {
                    translate: [-18.0, 10.0],
                    ..Transform::default()
                },
                ..mesh_of(
                    Vec::new(),
                    sheet(SHEET, ALL, TileMode::Clamp, Sampling::Linear),
                )
            },
        ),
        mesh(
            "vertices/vertices-geometry-with-mask-filter",
            // Upstream's, and a regression test there rather than a feature
            // one: a mesh under a mask filter used to draw nothing at all.
            //
            // The mesh carries no colors of its own, which is what makes the
            // blur well defined and is easy to read past. A mask blur fills
            // through blurred coverage, so the fill has to have a value out in
            // the halo where the triangles are not -- a paint does, being a
            // function of position, and per-vertex colors do not. A mesh
            // carrying them is refused rather than extrapolated.
            MeshSpec {
                positions: vec![[18.0, 18.0], [110.0, 18.0], [18.0, 110.0]],
                mask_blur: 7.0,
                ..mesh_of(Vec::new(), Fill::Solid([0.95, 0.85, 0.4, 1.0]))
            },
        ),
    ]
}

/// Four colors, one per corner.
const ALL_CORNERS: [[f32; 4]; 4] = [
    [1.0, 0.2, 0.2, 1.0],
    [0.2, 1.0, 0.3, 1.0],
    [0.3, 0.4, 1.0, 1.0],
    [1.0, 0.9, 0.2, 1.0],
];

/// A fan around the plate's center.
fn fan() -> Vec<[f32; 2]> {
    let mut points = vec![[64.0, 64.0]];
    for i in 0..=8 {
        let a = i as f32 / 8.0 * std::f32::consts::TAU;
        points.push([64.0 + 48.0 * a.cos(), 64.0 + 48.0 * a.sin()]);
    }
    points
}

fn atlas(name: &'static str, spec: AtlasSpec) -> Scene {
    Scene::tree(name, vec![Node::Atlas(Box::new(spec))])
        .with_background(DARK)
        .with_samples(4)
}

/// One sprite per quadrant of the sheet, laid out in a row.
fn quadrants(color: [f32; 4]) -> Vec<SpriteSpec> {
    (0..4)
        .map(|i| {
            let (sx, sy) = ((i % 2) as f32 * 4.0, (i / 2) as f32 * 4.0);
            SpriteSpec {
                source: [sx, sy, sx + 4.0, sy + 4.0],
                rotate: 0.0,
                scale: 6.0,
                translate: [10.0 + i as f32 * 28.0, 52.0],
                color,
            }
        })
        .collect()
}

/// `aiks_dl_atlas_unittests.cc`.
fn atlas_scenes() -> Vec<Scene> {
    vec![
        atlas(
            "atlas/draw-atlas-no-color",
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: quadrants(WHITE),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/draw-atlas-with-color-simple",
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: (0..4)
                    .map(|i| SpriteSpec {
                        color: ALL_CORNERS[i],
                        ..quadrants(WHITE)[i]
                    })
                    .collect(),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/draw-atlas-with-opacity",
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: quadrants(WHITE),
                blend: BlendMode::SrcOver,
                alpha: 0.4,
            },
        ),
        atlas(
            "atlas/draw-atlas-no-color-full-size",
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: vec![SpriteSpec {
                    source: [0.0, 0.0, 8.0, 8.0],
                    rotate: 0.0,
                    scale: 12.0,
                    translate: [16.0, 16.0],
                    color: WHITE,
                }],
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        // Every plate above leaves the batch blend at its default, so the
        // field had never been anything else and a batch that ignored it would
        // have drawn all five correctly. These two overlap deliberately: under
        // `Plus` the overlaps are where the mode shows, and a sprite drawn on
        // its own says nothing about what combining means.
        atlas(
            "atlas/sprites-added-where-they-overlap",
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: (0..5)
                    .map(|i| SpriteSpec {
                        source: [0.0, 0.0, 8.0, 8.0],
                        rotate: 0.0,
                        scale: 7.0,
                        // Sixteen apart against a width of fifty-six, so each
                        // overlaps its neighbor by well over half and the run
                        // has single, double and triple coverage in it.
                        translate: [18.0 + i as f32 * 16.0, 36.0],
                        color: [0.35, 0.2, 0.5, 1.0],
                    })
                    .collect(),
                blend: BlendMode::Plus,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/sprites-multiplied-into-what-is-behind",
            // `Multiply` reads the destination, which a batch drawn in one
            // call has to get right within itself as well as against the
            // background: the sprites overlap, so some fragments multiply
            // against another sprite rather than against the ground.
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: (0..4)
                    .map(|i| {
                        let (sx, sy) = ((i % 2) as f32 * 4.0, (i / 2) as f32 * 4.0);
                        SpriteSpec {
                            source: [sx, sy, sx + 4.0, sy + 4.0],
                            rotate: 0.0,
                            scale: 9.0,
                            translate: [22.0 + i as f32 * 22.0, 46.0],
                            color: WHITE,
                        }
                    })
                    .collect(),
                blend: BlendMode::Multiply,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/sprite-colors-combined-by-a-mode-that-is-not-a-multiply",
            // The batch's own blend decides how the result reaches the target;
            // this is the other one, deciding how each sprite's color meets the
            // texels it covers. `Difference` because it is nothing like a
            // multiply: a sprite that vanished under `Modulate` shows here, so
            // the plate says which of the two settings it is reading.
            AtlasSpec {
                tint_blend: BlendMode::Difference,
                sprites: (0..4)
                    .map(|i| {
                        let (sx, sy) = ((i % 2) as f32 * 4.0, (i / 2) as f32 * 4.0);
                        SpriteSpec {
                            source: [sx, sy, sx + 4.0, sy + 4.0],
                            rotate: 0.0,
                            scale: 7.0,
                            translate: [12.0 + i as f32 * 26.0, 46.0],
                            color: [0.9, 0.55, 0.2, 1.0],
                        }
                    })
                    .collect(),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/sprites-of-different-sizes-from-one-sheet",
            // One sheet, four source rectangles of different extents, each
            // scaled differently again. What this asks is that a sprite's
            // coordinates come from its own source rather than from a size
            // shared by the batch -- which four equal quadrants could not ask,
            // being indistinguishable from a batch that assumed them.
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: vec![
                    SpriteSpec {
                        source: [0.0, 0.0, 8.0, 8.0],
                        rotate: 0.0,
                        scale: 6.0,
                        translate: [8.0, 8.0],
                        color: WHITE,
                    },
                    SpriteSpec {
                        source: [4.0, 0.0, 8.0, 4.0],
                        rotate: 0.0,
                        scale: 9.0,
                        translate: [64.0, 12.0],
                        color: WHITE,
                    },
                    SpriteSpec {
                        source: [0.0, 4.0, 2.0, 8.0],
                        rotate: 0.0,
                        scale: 14.0,
                        translate: [10.0, 66.0],
                        color: WHITE,
                    },
                    SpriteSpec {
                        source: [2.0, 2.0, 6.0, 6.0],
                        rotate: 0.4,
                        scale: 8.0,
                        translate: [78.0, 62.0],
                        color: WHITE,
                    },
                ],
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/draw-atlas-advanced-and-transform",
            AtlasSpec {
                tint_blend: BlendMode::Modulate,
                sprites: (0..6)
                    .map(|i| {
                        let t = i as f32 / 6.0;
                        let turn = t * std::f32::consts::TAU;
                        SpriteSpec {
                            source: [0.0, 0.0, 4.0, 4.0],
                            rotate: turn,
                            scale: 5.0,
                            translate: [
                                64.0 + 34.0 * turn.cos() - 10.0,
                                64.0 + 34.0 * turn.sin() - 10.0,
                            ],
                            color: WHITE,
                        }
                    })
                    .collect(),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        atlas(
            "atlas/draw-atlas-with-color-burn",
            // Four grays running black to white against a mode that divides by
            // what it is given: `ColorBurn` leaves the destination alone at
            // white and takes it to black at black, so the four sprites make a
            // sweep from untouched to erased over the same texels. The plate
            // beside this one uses `Difference` with one color, which says the
            // setting is read; this says the arithmetic is right, because a
            // burn done as a multiply would still darken and would darken
            // wrong.
            AtlasSpec {
                tint_blend: BlendMode::ColorBurn,
                sprites: (0..4)
                    .map(|i| {
                        let (sx, sy) = ((i % 2) as f32 * 4.0, (i / 2) as f32 * 4.0);
                        let level = i as f32 / 3.0;
                        SpriteSpec {
                            source: [sx, sy, sx + 4.0, sy + 4.0],
                            rotate: 0.0,
                            scale: 7.0,
                            translate: [12.0 + i as f32 * 26.0, 46.0],
                            color: [level, level, level, 1.0],
                        }
                    })
                    .collect(),
                blend: BlendMode::SrcOver,
                alpha: 1.0,
            },
        ),
        plate_tree(
            "atlas/draw-image-rect-with-blend-color-filter",
            image_rect_filtered(ColorFilter::blend([1.0, 0.0, 0.0, 0.4], BlendMode::SrcOver)),
        ),
        plate_tree(
            "atlas/draw-image-rect-with-matrix-color-filter",
            image_rect_filtered(ColorFilter::matrix([
                -1.0, 0.0, 0.0, 1.0, 0.0, //
                0.0, -1.0, 0.0, 1.0, 0.0, //
                0.0, 0.0, -1.0, 1.0, 0.0, //
                1.0, 1.0, 1.0, 1.0, 0.0,
            ])),
        ),
    ]
}

/// Upstream's pair of image draws, the same but for a filter that should not
/// change anything.
///
/// Both carry the same color filter. The left one also carries an identity
/// matrix image filter, which upstream puts there to take the draw off its
/// atlas fast path -- and which, being the identity, sends the draw through an
/// offscreen and resamples it on the way back. So the two are a fast route and
/// a slow one, and upstream keeps them side by side for a reader to see that
/// they agree.
///
/// The claim survives the translation even though the fast path does not: an
/// identity matrix filter must be invisible, and a filter that resamples
/// through a target has every opportunity not to be.
fn image_rect_filtered(filter: ColorFilter) -> Vec<Node> {
    let panel = |x: f32| {
        Item::filled(
            Shape::Rect {
                min: [x, 36.0],
                max: [x + 56.0, 92.0],
            },
            sheet(
                [x, 36.0, x + 56.0, 92.0],
                ALL,
                TileMode::Clamp,
                Sampling::Linear,
            ),
        )
        .with_color_filter(filter)
        .with_blend(BlendMode::SrcOver)
    };
    vec![
        Node::Paint(Box::new(PaintSpec {
            fill: Fill::Solid(WHITE),
            blend: BlendMode::Src,
            clip: None,
            clip_out: None,
            transform: Transform::default(),
        })),
        Node::Draw(Box::new(panel(4.0).with_image_filter(
            ImageFilter::Matrix {
                transform: emblema_core::Transform2D::IDENTITY,
            },
        ))),
        Node::Draw(Box::new(panel(68.0))),
    ]
}

/// `aiks_dl_blur_unittests.cc`, as far as the styles reach.
///
/// The file is the largest of them and most of it turns on mask blur styles,
/// which is what these four are. What is still missing from it needs blurs
/// this renderer does not have -- a blur that survives a rotation and a clip
/// together, and the tiny-mipmap cases. The backdrop-key cases were on that
/// list until the key was built; they are `backdrop_ids` below.
fn blur() -> Vec<Scene> {
    let styles = [
        ("blur/gaussian-blur-style-normal", MaskBlurStyle::Normal),
        ("blur/gaussian-blur-style-solid", MaskBlurStyle::Solid),
        ("blur/gaussian-blur-style-outer", MaskBlurStyle::Outer),
        ("blur/gaussian-blur-style-inner", MaskBlurStyle::Inner),
    ];
    let mut scenes: Vec<Scene> = styles
        .iter()
        .map(|(name, style)| {
            plate(
                name,
                vec![Item::fill(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 34.0,
                    },
                    WHITE,
                )
                .with_mask_blur(7.0)
                .with_mask_blur_style(*style)
                .with_blend(BlendMode::SrcOver)],
            )
        })
        .collect();

    scenes.push(plate(
        "blur/solid-color-circle-mask-blur-tiny-sigma",
        vec![Item::fill(
            Shape::Circle {
                center: [64.0, 64.0],
                radius: 40.0,
            },
            WHITE,
        )
        // Small enough that the halo is a pixel or two, which is the case a
        // blur implemented by scaling a target down and back up gets wrong.
        .with_mask_blur(0.6)
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(plate(
        "blur/can-render-foreground-advanced-blend-with-mask-blur",
        // A mask blur and an advanced-mode color filter on one draw, which is
        // the pair upstream keeps this scene for: its own comment says the
        // filter should apply to the color only and respect the alpha mask.
        //
        // Those are two claims and the plate shows both. `Color` takes the hue
        // and saturation of its source and the luminosity of its destination,
        // so a gray circle filtered green against nothing comes out green --
        // and the mask blur's halo, which is the same gray at a falling alpha,
        // has to come out the same green at a falling alpha rather than fading
        // toward the filter's own color. A filter applied after the mask,
        // rather than to the color the mask then scales, is what would show as
        // the second.
        //
        // Upstream's clip is here too, and cuts the circle on two sides: what
        // it is for is the case where the blurred halo runs into the clip, so
        // the filter and the mask meet a hard edge as well as a soft one.
        vec![Item::fill(
            Shape::Circle {
                center: [64.0, 64.0],
                radius: 32.0,
            },
            [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0],
        )
        .with_mask_blur(2.0)
        .with_color_filter(ColorFilter::blend(GREEN, BlendMode::Color))
        .with_clip_shape(Shape::Rect {
            min: [16.0, 24.0],
            max: [80.0, 88.0],
        })
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(plate(
        "blur/can-render-mask-blur-huge-sigma",
        vec![Item::fill(
            Shape::Circle {
                center: [64.0, 64.0],
                radius: 20.0,
            },
            WHITE,
        )
        // Reaching well past the plate, so the halo is cut by the frame rather
        // than by the layer -- which is the distinction a bounded layer sized
        // to its content alone gets wrong.
        .with_mask_blur(30.0)
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(plate(
        "blur/mask-blur-with-zero-sigma-is-skipped",
        vec![Item::fill(
            Shape::RoundedRect {
                min: [24.0, 32.0],
                max: [104.0, 96.0],
                radius: 12.0,
            },
            WHITE,
        )
        .with_mask_blur(0.0)
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes
}

fn shadow_plate(name: &'static str, spec: ShadowSpec) -> Scene {
    // A pale ground, because a shadow is a darkening and the plates elsewhere
    // are drawn on a dark one where it would be invisible.
    Scene::tree(name, vec![Node::Shadow(Box::new(spec))])
        .with_background([230.0 / 255.0, 230.0 / 255.0, 235.0 / 255.0, 1.0])
        .with_samples(4)
}

/// Upstream's two spiral casters, which differ only in which way the radius
/// runs.
///
/// Twenty steps around and twice around, with the radius walking away from
/// `base` by `step` a turn and back -- so the contour crosses itself on every
/// revolution and the shape it encloses is a question about the fill rule
/// rather than about an outline.
fn spiral(base: f32, step: f32) -> Vec<[f32; 2]> {
    const STEPS: i32 = 20;
    // From zero rather than from one, which is upstream's shape written without
    // its move-to: that first point is the same formula at `i == 0`.
    (0..STEPS * 2)
        .map(|i| {
            let angle = std::f32::consts::TAU * i as f32 / STEPS as f32;
            let radius = base + step * (i - STEPS).abs() as f32;
            [64.0 + angle.cos() * radius, 64.0 + angle.sin() * radius]
        })
        .collect()
}

fn caster(shape: Shape) -> ShadowSpec {
    ShadowSpec {
        shape,
        color: [0.0, 0.0, 0.0, 1.0],
        elevation: 8.0,
        transparent_occluder: false,
        transform: Transform::default(),
        with_caster: true,
    }
}

/// A run of the fixture glyphs, placed along a line.
fn run(indices: &[u32], origin: [f32; 2]) -> Vec<(u32, [f32; 2])> {
    indices
        .iter()
        .enumerate()
        .map(|(i, index)| (*index, [origin[0] + i as f32 * 14.0, origin[1]]))
        .collect()
}

fn text(name: &'static str, spec: GlyphRunSpec) -> Scene {
    Scene::tree(name, vec![Node::Glyphs(Box::new(spec))])
        .with_background(DARK)
        .with_samples(4)
}

fn run_of(glyphs: Vec<(u32, [f32; 2])>, color: [f32; 4]) -> GlyphRunSpec {
    GlyphRunSpec {
        glyphs,
        color,
        blend: BlendMode::SrcOver,
        transform: Transform::default(),
        image_filter: ImageFilter::None,
        mask_blur: 0.0,
        mask_blur_style: MaskBlurStyle::Normal,
    }
}

/// Glyph runs, which Impeller's own text file cannot be mirrored for -- that
/// one shapes real fonts, and a font file decides the picture. These use
/// synthetic coverage instead, so what is compared is the path a run takes
/// through this renderer rather than anyone's hinting.
fn glyphs() -> Vec<Scene> {
    vec![
        text(
            "text/a-run-reads-its-atlas-as-coverage",
            // The four fixture glyphs: solid, half, a ring and a wedge. Half
            // coverage has to come out as the paint's color at half alpha
            // rather than as a lighter color, which is the whole difference
            // between an atlas read as coverage and one read as color.
            run_of(run(&[0, 1, 2, 3], [22.0, 58.0]), YELLOW),
        ),
        text(
            "text/a-run-through-an-image-filter",
            // A dilation over a run, which goes through a layer -- the route a
            // run did not take at all until recently.
            GlyphRunSpec {
                image_filter: ImageFilter::Dilate {
                    radius_x: 4.0,
                    radius_y: 4.0,
                },
                ..run_of(run(&[0, 2, 3], [30.0, 58.0]), GREEN)
            },
        ),
        text(
            "text/a-run-with-a-normal-mask-blur",
            // A text shadow, which is what a mask blur over a run is for.
            GlyphRunSpec {
                mask_blur: 4.0,
                ..run_of(run(&[0, 2, 3], [30.0, 58.0]), WHITE)
            },
        ),
        text(
            "text/a-run-with-a-solid-mask-blur",
            // The shape kept at full strength with its blur around it, which
            // is the shadow and the text in one draw.
            GlyphRunSpec {
                mask_blur: 4.0,
                mask_blur_style: MaskBlurStyle::Solid,
                ..run_of(run(&[0, 2, 3], [30.0, 58.0]), BLUE)
            },
        ),
        text(
            "text/a-run-under-a-rotation",
            // A run turns with the canvas like anything else, and its coverage
            // is sampled through the rotation rather than snapped to it.
            GlyphRunSpec {
                transform: Transform {
                    rotate: 0.35,
                    translate: [18.0, -14.0],
                    ..Transform::default()
                },
                ..run_of(run(&[0, 1, 2, 3], [22.0, 58.0]), RED)
            },
        ),
        Scene::tree(
            "text/can-render-text-in-save-layer",
            // A run inside a group, which is upstream's scene and is a
            // different path from a run drawn straight onto the frame: the
            // atlas is sampled into the group's target and the group is then
            // composited, so the coverage is resampled once more than it
            // otherwise would be. At half alpha, because a group that is
            // opaque and unfiltered is one a renderer may legitimately skip.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Glyphs(Box::new(run_of(
                    run(&[0, 1, 2, 3], [22.0, 70.0]),
                    WHITE,
                )))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        text(
            "text/can-render-text-outside-boundaries",
            // Placed so the run leaves the frame on both sides. A glyph is a
            // quad sampling an atlas, and a quad partly outside the target is
            // where a coordinate computed from the quad's own corners rather
            // than from the visible part of it goes wrong -- the surviving half
            // reads the wrong texels and the run smears instead of being cut.
            run_of(run(&[0, 1, 2, 3, 0, 1, 2, 3], [-24.0, 64.0]), WHITE),
        ),
    ]
}

fn picture(name: &'static str, spec: PictureSpec) -> Scene {
    Scene::tree(name, vec![Node::Picture(Box::new(spec))])
        .with_background(DARK)
        .with_samples(4)
}

/// The picture scenes from `aiks_dl_unittests.cc`, which is where that file's
/// round-trip cases live.
fn pictures() -> Vec<Scene> {
    let contents = || {
        vec![
            Node::Draw(Box::new(Item::fill(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [56.0, 56.0],
                },
                RED,
            ))),
            Node::Draw(Box::new(Item::fill(
                Shape::Circle {
                    center: [32.0, 32.0],
                    radius: 14.0,
                },
                BLUE,
            ))),
        ]
    };
    vec![
        {
            // Upstream's point set drawn twice, once with a round cap and once
            // with a square one -- which is the whole of what a cap means to a
            // point, there being no direction for one to extend along. Two of
            // the seven points are the same point and two more are close
            // together, and the color is translucent, so where they land on
            // each other the ink doubles.
            let points = vec![
                [0.0, 0.0],
                [26.0, 26.0],
                [26.0, 0.0],
                [0.0, 26.0],
                [0.0, 0.0],
                [12.0, 12.0],
                [14.0, 14.0],
            ];
            let field = |cap: LineCap, x: f32| PointsSpec {
                mode: PointMode::Points,
                points: points.clone(),
                stroke: StrokeSpec {
                    cap,
                    ..StrokeSpec::new(7.0)
                },
                color: [1.0, 1.0, 0.2, 0.5],
                blend: BlendMode::SrcOver,
                transform: Transform::translate(x, 50.0),
                clip_shape: None,
            };
            Scene::tree(
                "dl/can-draw-points",
                vec![
                    Node::Points(Box::new(field(LineCap::Round, 16.0))),
                    Node::Points(Box::new(field(LineCap::Square, 76.0))),
                ],
            )
            .with_background(DARK)
            .with_samples(4)
        },
        points_plate(
            "dl/can-draw-scaled-points-large-scale-small-radius",
            // A point a ten-thousandth of a unit wide under a millionfold
            // scale, which is upstream's and is a hundred device pixels. A
            // renderer that sized the point in device pixels from the width
            // alone would draw nothing at all.
            PointsSpec {
                mode: PointMode::Points,
                points: vec![[0.0, 0.0]],
                stroke: StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(100.0 * 0.000_001)
                },
                color: RED,
                blend: BlendMode::SrcOver,
                transform: Transform {
                    scale: [1_000_000.0, 1_000_000.0],
                    translate: [64.0, 64.0],
                    ..Transform::default()
                },
                clip_shape: None,
            },
        ),
        points_plate(
            "dl/can-draw-scaled-points-small-scale-large-radius",
            // The same product the other way round, which catches the opposite
            // mistake: a hundred million units wide under a millionth of a
            // scale is the same hundred device pixels, and a renderer that
            // clamped the width before applying the transform would fill the
            // frame.
            PointsSpec {
                mode: PointMode::Points,
                points: vec![[0.0, 0.0]],
                stroke: StrokeSpec {
                    cap: LineCap::Round,
                    ..StrokeSpec::new(100.0 * 1_000_000.0)
                },
                color: RED,
                blend: BlendMode::SrcOver,
                transform: Transform {
                    scale: [0.000_001, 0.000_001],
                    translate: [64.0, 64.0],
                    ..Transform::default()
                },
                clip_shape: None,
            },
        ),
        Scene::tree(
            "dl/translucent-save-layer-with-advanced-blend-mode-draws-correctly",
            // A group carrying both an alpha and an advanced mode, which have
            // to compose in that order: the alpha scales what the group drew
            // and the mode then combines the scaled result with the frame.
            // Applied the other way round, `Lighten` would compare a full
            // strength green against the red and take the green outright.
            vec![
                Node::Draw(Box::new(Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [102.0, 102.0],
                    },
                    RED,
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        alpha: 0.5,
                        blend: BlendMode::Lighten,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(
                        Item::fill(
                            Shape::Circle {
                                center: [51.0, 51.0],
                                radius: 26.0,
                            },
                            GREEN,
                        )
                        .with_blend(BlendMode::SrcOver),
                    ))],
                },
            ],
        )
        .with_background(DARK)
        // Single-sampled: an advanced blend under multisampling draws nothing
        // on this machine's Vulkan software rasterizer. See the note at the
        // blend family.
        .with_samples(1),
        Scene::tree(
            "dl/image-filtered-save-layer-with-unbounded-contents",
            // A bounded group whose contents are a paint, which covers the clip
            // rather than any shape -- so what the filter is handed is decided
            // by the group's bounds and nothing else. The white marks are
            // upstream's and are outside the group: they say where its edge
            // should be, and the blurred yellow square has to arrive between
            // them.
            [
                registration_marks(),
                vec![Node::Layer {
                    layer: Box::new(LayerSpec {
                        filter: ImageFilter::blur(4.0),
                        ..LayerSpec::default()
                    }),
                    bounds: Some([44.0, 44.0, 84.0, 84.0]),
                    transform: Transform::default(),
                    children: vec![Node::Paint(Box::new(PaintSpec {
                        fill: Fill::Solid([1.0, 0.95, 0.2, 1.0]),
                        blend: BlendMode::SrcOver,
                        clip: None,
                        clip_out: None,
                        transform: Transform::default(),
                    }))],
                }],
            ]
            .concat(),
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "dl/image-filtered-unbounded-save-layer-with-unbounded-contents",
            // The same question with the bounds taken away. An unbounded group
            // whose contents are also unbounded has nothing but the target to
            // be sized by, so the blur runs over the whole frame -- and the
            // rectangle inside it is there to show that the interior is blurred
            // rather than only the edges, which a group sized to its contents
            // would still manage.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    filter: ImageFilter::blur(6.0),
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![
                    Node::Paint(Box::new(PaintSpec {
                        fill: Fill::Solid([1.0, 0.95, 0.2, 1.0]),
                        blend: BlendMode::SrcOver,
                        clip: None,
                        clip_out: None,
                        transform: Transform::default(),
                    })),
                    Node::Draw(Box::new(
                        Item::fill(
                            Shape::Rect {
                                min: [50.0, 50.0],
                                max: [78.0, 78.0],
                            },
                            BLUE,
                        )
                        .with_blend(BlendMode::SrcOver),
                    )),
                ],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "dl/depth-values-for-line-mode",
            // A stroked circle, then a run of lines clipped to that same
            // circle. `Lines` mode makes several draws out of one call, and
            // upstream's comment says what the scene is for: they all have to
            // carry the same depth. A clip is tested against depth, so draws
            // given different ones would be cut differently -- some segments
            // surviving the circle and some not, out of a single call.
            vec![
                Node::Draw(Box::new(Item::stroke(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 52.0,
                    },
                    StrokeSpec::new(3.0),
                    RED,
                ))),
                Node::Points(Box::new(PointsSpec {
                    mode: PointMode::Lines,
                    // Five segments running off both sides of the circle, so
                    // every one of them meets the clip.
                    points: (0..5)
                        .flat_map(|i| {
                            let y = i as f32 * 26.0 - 26.0;
                            [[-8.0, y], [136.0, y + 96.0]]
                        })
                        .collect(),
                    stroke: StrokeSpec::new(6.0),
                    color: BLUE,
                    blend: BlendMode::SrcOver,
                    transform: Transform::default(),
                    clip_shape: Some(Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 52.0,
                    }),
                })),
            ],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "dl/depth-values-for-polygon-mode",
            // The plate above with one thing changed. `Polygon` joins the whole
            // run into a single open polyline where `Lines` reads it as
            // separate segments, so the same ten points draw a zig-zag here and
            // five parallel strokes there -- and the invariant is the same one:
            // several draws out of one call, all needing the depth the clip is
            // tested against, or the run comes back cut in pieces.
            //
            // The points are deliberately identical. A pair that differs in the
            // mode alone says which of the two modes is wrong when one of them
            // is; a pair that also moved its geometry says only that something
            // did.
            vec![
                Node::Draw(Box::new(Item::stroke(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 52.0,
                    },
                    StrokeSpec::new(3.0),
                    RED,
                ))),
                Node::Points(Box::new(PointsSpec {
                    mode: PointMode::Polygon,
                    points: (0..5)
                        .flat_map(|i| {
                            let y = i as f32 * 26.0 - 26.0;
                            [[-8.0, y], [136.0, y + 96.0]]
                        })
                        .collect(),
                    stroke: StrokeSpec::new(6.0),
                    color: BLUE,
                    blend: BlendMode::SrcOver,
                    transform: Transform::default(),
                    clip_shape: Some(Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 52.0,
                    }),
                })),
            ],
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "dl/matrix-backdrop-filter",
            // A backdrop filter that is a matrix rather than a blur: the inner
            // group captures what the outer one has drawn, halves it and puts
            // it down offset, so a half-size copy of the rectangle and the
            // circle lands with its circle on the far edge of the original.
            //
            // The first matrix backdrop in the catalog, and until this went in
            // the only one this renderer refused. The other backdrop plates
            // filter with a blur, a caller's program, or the two composed, and
            // all three leave a pixel where they found it -- so nothing here
            // was asking whether a backdrop can be read anywhere but where it
            // was written.
            //
            // What the copy does not cover is the half that says the seed
            // decals rather than clamps: the capture is the outer group's own
            // content on transparent, so the copy is transparent nearly
            // everywhere and the original shows through it. Where the two
            // circles overlap, one at half alpha over the other reads brighter
            // than either.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([0.0, 0.0, 0.0, 1.0]),
                    blend: BlendMode::Src,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Layer {
                    layer: Box::new(LayerSpec::default()),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![
                        Node::Draw(Box::new(Item::stroke(
                            Shape::Rect {
                                min: [0.0, 0.0],
                                max: [120.0, 120.0],
                            },
                            StrokeSpec::new(2.0),
                            RED,
                        ))),
                        Node::Draw(Box::new(
                            Item::fill(
                                Shape::Circle {
                                    center: [80.0, 80.0],
                                    radius: 40.0,
                                },
                                [0.0, 1.0, 0.0, 0.5],
                            )
                            .with_blend(BlendMode::Plus),
                        )),
                        Node::Layer {
                            layer: Box::new(LayerSpec {
                                backdrop: ImageFilter::Matrix {
                                    // Halve about the circle's center and set
                                    // the result down a radius away along the
                                    // diagonal, which is upstream's arithmetic
                                    // in this plate's units.
                                    transform: magnify(
                                        Vec2::splat(40.0),
                                        0.5,
                                        Vec2::splat(40.0 + 40.0 * std::f32::consts::FRAC_1_SQRT_2),
                                    )
                                    .into(),
                                },
                                ..LayerSpec::default()
                            }),
                            bounds: None,
                            transform: Transform::default(),
                            children: Vec::new(),
                        },
                    ],
                },
            ],
        )
        .with_background(DARK)
        // A backdrop filter cuts the pass to read what it was writing, which a
        // multisampled pass cannot be resumed from; the same reason the blur
        // chapter's backdrop plates give.
        .with_samples(1),
        Scene::tree(
            "dl/matrix-backdrop-filter-with-translation",
            // The same matrix backdrop confined to a panel, which is upstream's
            // second scene for it and the one that puts a clip between the
            // capture and the seed.
            //
            // The clip is what the plate is for. The group is unbounded, so
            // without it the halved copy would cover the frame; with it the
            // copy is a panel in the corner and the original is untouched
            // beside it. Written at a fifth of upstream's coordinates, and with
            // the matrix chosen so the whole of the content lands inside the
            // panel rather than a corner of it.
            vec![
                Node::Draw(Box::new(Item::stroke(
                    Shape::Rect {
                        min: [4.0, 4.0],
                        max: [100.0, 100.0],
                    },
                    StrokeSpec::new(2.0),
                    RED,
                ))),
                Node::Draw(Box::new(Item::fill(
                    Shape::Circle {
                        center: [52.0, 52.0],
                        radius: 28.0,
                    },
                    GREEN,
                ))),
                Node::Clip {
                    rect: Some([64.0, 64.0, 124.0, 124.0]),
                    rect_out: None,
                    shape: None,
                    transform: Transform::default(),
                    children: vec![Node::Layer {
                        layer: Box::new(LayerSpec {
                            backdrop: ImageFilter::Matrix {
                                transform: magnify(Vec2::splat(52.0), 0.5, Vec2::splat(94.0))
                                    .into(),
                            },
                            ..LayerSpec::default()
                        }),
                        bounds: None,
                        transform: Transform::default(),
                        children: Vec::new(),
                    }],
                },
            ],
        )
        .with_background(DARK)
        // A backdrop filter cuts the pass to read what it was writing, which a
        // multisampled pass cannot be resumed from.
        .with_samples(1),
        Scene::tree(
            "dl/collapsed-draw-paint-in-subpass",
            // A paint inside a group whose mode combines it with the frame.
            // Upstream names this for an optimization that collapses the group
            // away, which is a question about how many passes it takes rather
            // than about what it draws -- so the picture is the picture either
            // way, and this renderer draws it without the optimization.
            vec![
                Node::Paint(Box::new(PaintSpec {
                    fill: Fill::Solid([1.0, 1.0, 0.2, 1.0]),
                    blend: BlendMode::Src,
                    clip: None,
                    clip_out: None,
                    transform: Transform::default(),
                })),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        blend: BlendMode::Multiply,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![Node::Paint(Box::new(PaintSpec {
                        fill: Fill::Solid([100.0 / 255.0, 149.0 / 255.0, 237.0 / 255.0, 0.75]),
                        blend: BlendMode::SrcOver,
                        clip: None,
                        clip_out: None,
                        transform: Transform::default(),
                    }))],
                },
            ],
        )
        .with_background(DARK)
        // An advanced mode on a group draws nothing on this machine's Vulkan
        // software rasterizer, whatever the sample count; see the blend family.
        .with_samples(1),
        Scene::tree(
            "dl/color-matrix-filter-subpass-collapse-optimization",
            filter_collapse(ColorFilter::matrix([
                -1.0, 0.0, 0.0, 1.0, 0.0, //
                0.0, -1.0, 0.0, 1.0, 0.0, //
                0.0, 0.0, -1.0, 1.0, 0.0, //
                1.0, 1.0, 1.0, 1.0, 0.0,
            ])),
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "dl/linear-to-srgb-filter-subpass-collapse-optimization",
            filter_collapse(ColorFilter::linear_to_srgb()),
        )
        .with_background(DARK)
        .with_samples(4),
        Scene::tree(
            "dl/srgb-to-linear-filter-subpass-collapse-optimization",
            filter_collapse(ColorFilter::srgb_to_linear()),
        )
        .with_background(DARK)
        .with_samples(4),
        picture(
            "dl/draw-picture-at-its-own-scale",
            // A picture placed by the transform and nothing else. It has no
            // bounds of its own, so its extent is the rectangle and the corner
            // goes where the transform says.
            PictureSpec {
                size: Extent2D::new(64, 64),
                children: contents(),
                transform: Transform::translate(32.0, 32.0),
                blend: BlendMode::SrcOver,
            },
        ),
        picture(
            "dl/draw-picture-magnified",
            // The limitation, drawn rather than described: a recording is
            // already tessellated, so magnifying one resamples the picture it
            // became instead of re-flattening its curves at the new scale. The
            // circle's edge is what shows it.
            PictureSpec {
                size: Extent2D::new(64, 64),
                children: contents(),
                transform: Transform {
                    scale: [1.9, 1.9],
                    rotate: 0.0,
                    skew: [0.0, 0.0],
                    translate: [4.0, 4.0],
                    perspective: [0.0, 0.0],
                },
                blend: BlendMode::SrcOver,
            },
        ),
        picture(
            "dl/draw-picture-holding-a-layer",
            // A picture carrying a pass of its own, which is the case where the
            // indices inside it have to move: its layer names a pass by
            // position in a list this recording is appending to.
            PictureSpec {
                size: Extent2D::new(64, 64),
                children: vec![Node::Layer {
                    layer: Box::new(LayerSpec::opacity(0.5)),
                    bounds: Some([0.0, 0.0, 64.0, 64.0]),
                    transform: Transform::default(),
                    children: contents(),
                }],
                transform: Transform::translate(32.0, 32.0),
                blend: BlendMode::SrcOver,
            },
        ),
    ]
}

/// `aiks_dl_shadow_unittests.cc`.
///
/// Most of that file is a convex-shadow optimization: one test per winding and
/// shape kind, each asserting that the mesh the optimization produced matches
/// the mesh the general path would have. That assertion cannot come across --
/// this renderer has no such optimization and nothing here counts vertices in a
/// shadow.
///
/// The pairing does come across, and it is the half worth having. A shadow must
/// not depend on which way its caster is wound, whatever route drew it, and the
/// scenes below are the same casters upstream uses with the same two windings
/// each. `a_shadow_does_not_care_which_way_its_caster_is_wound` in
/// `tests/catalog.rs` is what reads them as pairs; the naming is what lets it,
/// so a scene named `-counter-clockwise-` must have a `-clockwise-` sibling
/// differing in nothing else.
///
/// Upstream's casters are paths built point by point, and two of them cannot be
/// written here: a circle and an oval drawn out of conics tweaked off the exact
/// weight, so that the path is convex but unrecognizable, which the scene model
/// has no closed multi-segment curve for. Both are here as the analytic shapes
/// upstream compares those paths *against*, which is the nearer half of what
/// they are for.
///
/// A third was on that list until [`Shape::Polygons`] existed. It holds two
/// closed triangles, which [`Shape::Contours`] cannot say -- its contours are
/// open, on purpose -- and the note beside that variant said the closed case
/// should be written when a use for it appeared. This is the use.
fn shadow() -> Vec<Scene> {
    vec![
        // A rectangle with one corner moved by a twentieth of a pixel, which is
        // upstream's and is there so nothing recognizes the path as a rectangle
        // while the shadow stays rectangular. The analytic route is not lost by
        // writing it this way -- the round rect below and the elevation sweep
        // both take it.
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-rect",
            caster(Shape::Polygon(vec![
                [16.0, 16.0],
                [111.952, 16.0],
                [112.0, 112.0],
                [16.0, 112.0],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-counter-clockwise-rect",
            caster(Shape::Polygon(vec![
                [16.0, 16.0],
                [16.0, 112.0],
                [112.0, 112.0],
                [111.952, 16.0],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-triangle",
            caster(Shape::Polygon(vec![
                [64.0, 16.0],
                [112.0, 112.0],
                [16.0, 112.0],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-counter-clockwise-triangle",
            caster(Shape::Polygon(vec![
                [64.0, 16.0],
                [16.0, 112.0],
                [112.0, 112.0],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-octagon",
            caster(Shape::Polygon(vec![
                [16.0, 28.0],
                [28.0, 16.0],
                [100.0, 16.0],
                [112.0, 28.0],
                [112.0, 100.0],
                [100.0, 112.0],
                [28.0, 112.0],
                [16.0, 100.0],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-counter-clockwise-octagon",
            caster(Shape::Polygon(vec![
                [16.0, 28.0],
                [16.0, 100.0],
                [28.0, 112.0],
                [100.0, 112.0],
                [112.0, 100.0],
                [112.0, 28.0],
                [100.0, 16.0],
                [28.0, 16.0],
            ])),
        ),
        // A triangle with a point added halfway along two of its sides. They
        // are on the edge they sit on, so they change nothing about the shape
        // and everything about the vertex list, which is upstream's subject:
        // a hull walk that treats a colinear point as a turn gets a different
        // mesh out of the same picture.
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-with-extra-colinear-vertices",
            caster(Shape::Polygon(vec![
                [64.0, 16.0],
                [88.0, 64.0],
                [112.0, 112.0],
                [64.0, 112.0],
                [16.0, 112.0],
                [40.0, 64.0],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-counter-clockwise-with-extra-colinear-vertices",
            caster(Shape::Polygon(vec![
                [64.0, 16.0],
                [40.0, 64.0],
                [16.0, 112.0],
                [64.0, 112.0],
                [112.0, 112.0],
                [88.0, 64.0],
            ])),
        ),
        // The three upstream keeps on the other side of the question: casters
        // the optimization must decline. An hourglass and two spirals, each a
        // single closed contour that crosses itself, so no winding is the
        // caster's winding and the shadow is whatever the fill rule says the
        // caster covers.
        shadow_plate(
            "shadow/draw-shadow-does-not-optimize-hourglass",
            caster(Shape::Polygon(vec![
                [16.0, 16.0],
                [112.0, 112.0],
                [16.0, 112.0],
                [112.0, 16.0],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-does-not-optimize-multiple-contours",
            // Two closed triangles meeting at a point, which is the caster the
            // optimization declines for being more than one contour rather
            // than for being concave. They share a vertex and nothing else, so
            // what a fill rule would decide about an overlap never arises --
            // upstream's point is the count of contours and not their winding.
            caster(Shape::Polygons(vec![
                vec![[40.0, 16.0], [64.0, 112.0], [16.0, 112.0]],
                vec![[88.0, 16.0], [112.0, 112.0], [64.0, 112.0]],
            ])),
        ),
        shadow_plate(
            "shadow/draw-shadow-does-not-optimize-inner-outer-spiral",
            caster(Shape::Polygon(spiral(38.4, 0.48))),
        ),
        shadow_plate(
            "shadow/draw-shadow-does-not-optimize-outer-inner-spiral",
            caster(Shape::Polygon(spiral(48.0, -0.48))),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-circle",
            caster(Shape::Circle {
                center: [64.0, 60.0],
                radius: 30.0,
            }),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-uniform-round-rect",
            caster(Shape::RoundedRect {
                min: [32.0, 36.0],
                max: [96.0, 84.0],
                radius: 14.0,
            }),
        ),
        shadow_plate(
            "shadow/draw-shadow-can-optimize-clockwise-oval",
            caster(Shape::Oval {
                min: [26.0, 42.0],
                max: [102.0, 80.0],
            }),
        ),
        shadow_plate(
            "shadow/elevation-sets-how-far-and-how-soft",
            // Elevation is the one number a caller sets, and it does two things
            // at once: it moves the shadow and it spreads it. A plate at a
            // single elevation cannot show either, so this is the middle of a
            // sweep and the two either side of it are below.
            ShadowSpec {
                elevation: 2.0,
                ..caster(Shape::RoundedRect {
                    min: [34.0, 40.0],
                    max: [94.0, 88.0],
                    radius: 10.0,
                })
            },
        ),
        shadow_plate(
            "shadow/elevation-high",
            ShadowSpec {
                elevation: 24.0,
                ..caster(Shape::RoundedRect {
                    min: [34.0, 40.0],
                    max: [94.0, 88.0],
                    radius: 10.0,
                })
            },
        ),
        shadow_plate(
            "shadow/shadow-of-a-concave-caster",
            // Every other caster here is convex, which is what the optimization
            // Impeller's file is mostly about requires. This renderer has no
            // such optimization, so a concave caster is not a different path --
            // but it is a different picture, and one where a shadow built from
            // a hull rather than from the shape would be obviously wrong: the
            // notch has to be dark inside.
            ShadowSpec {
                elevation: 10.0,
                ..caster(Shape::Polygon(vec![
                    [30.0, 34.0],
                    [98.0, 34.0],
                    [98.0, 94.0],
                    [76.0, 94.0],
                    [76.0, 58.0],
                    [52.0, 58.0],
                    [52.0, 94.0],
                    [30.0, 94.0],
                ]))
            },
        ),
        shadow_plate(
            "shadow/shadow-without-its-caster",
            // The caster left off, so what is drawn is the shadow alone. Every
            // other plate covers the middle of the shadow with the shape that
            // cast it, which means the part of the picture under the caster is
            // never seen -- and that is exactly the part an occluder is
            // supposed to hide.
            ShadowSpec {
                with_caster: false,
                elevation: 10.0,
                ..caster(Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 30.0,
                })
            },
        ),
        shadow_plate(
            "shadow/transparent-occluder-shows-what-is-beneath",
            // The pair to the plate above: with a transparent occluder the
            // shadow under the caster is kept, because the caster will not hide
            // it. Drawn without the caster on top, the two plates differ in the
            // middle and nowhere else, which is the whole of what the flag
            // means.
            ShadowSpec {
                with_caster: false,
                transparent_occluder: true,
                elevation: 10.0,
                ..caster(Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 30.0,
                })
            },
        ),
        shadow_plate(
            "shadow/can-draw-rotated-convex-shadow",
            ShadowSpec {
                transform: Transform {
                    rotate: 0.45,
                    translate: [14.0, -18.0],
                    ..Transform::default()
                },
                ..caster(Shape::Rect {
                    min: [36.0, 36.0],
                    max: [92.0, 84.0],
                })
            },
        ),
        shadow_plate(
            "shadow/can-draw-perspective-convex-shadow",
            // The third of upstream's three, and the one that is not about the
            // convex-shadow optimization at all -- it draws a mask blur under a
            // three-dimensional rotation and a perspective matrix, where the
            // other two rotate and scale. Translated here the way its two
            // siblings above are, as a shadow under the transform, since what
            // this catalog can say about it is whether the shadow follows the
            // caster through a projection.
            //
            // Well short of the vanishing line: at the plate's far edge the
            // divisor is a third above one, where the transform's own
            // documentation warns that a scene meant to be compared across
            // devices should stay away from where it reaches zero.
            ShadowSpec {
                transform: Transform {
                    perspective: [0.0025, 0.0],
                    ..Transform::default()
                },
                ..caster(Shape::Rect {
                    min: [34.0, 34.0],
                    max: [94.0, 90.0],
                })
            },
        ),
        shadow_plate(
            "shadow/can-draw-nonuniform-scale-convex-shadow",
            ShadowSpec {
                transform: Transform {
                    scale: [1.4, 0.7],
                    translate: [-26.0, 26.0],
                    ..Transform::default()
                },
                ..caster(Shape::RoundedRect {
                    min: [32.0, 36.0],
                    max: [96.0, 84.0],
                    radius: 12.0,
                })
            },
        ),
        shadow_plate(
            "shadow/transparent-shadow-produces-correct-color",
            ShadowSpec {
                // Nothing drawn on top, and the part beneath the caster kept:
                // the two together are what the flag is for, and the picture
                // is the whole blurred shape rather than a ring.
                transparent_occluder: true,
                with_caster: false,
                color: [0.1, 0.2, 0.6, 1.0],
                elevation: 10.0,
                ..caster(Shape::Circle {
                    center: [64.0, 60.0],
                    radius: 30.0,
                })
            },
        ),
    ]
}

/// Overlapping shapes, for the scenes that group things.
fn pair() -> Vec<Node> {
    vec![
        Node::Draw(Box::new(Item::fill(
            Shape::Circle {
                center: [50.0, 64.0],
                radius: 30.0,
            },
            RED,
        ))),
        Node::Draw(Box::new(
            Item::fill(
                Shape::Circle {
                    center: [82.0, 64.0],
                    radius: 30.0,
                },
                BLUE,
            )
            .with_blend(BlendMode::SrcOver),
        )),
    ]
}

/// Stripes to sit behind a layer, so a backdrop filter has something to act on.
///
/// A backdrop blur reads what is already on the target. Over a flat background
/// it reads one color and blurs it into the same color, which is why the two
/// backdrop plates below used to render identically to themselves with the
/// blur removed -- they asked for the filter and could not show it.
fn behind() -> Vec<Node> {
    (0..8)
        .map(|i| {
            Node::Draw(Box::new(Item::fill(
                Shape::Rect {
                    min: [i as f32 * 16.0, 0.0],
                    max: [i as f32 * 16.0 + 8.0, 128.0],
                },
                if i % 2 == 0 { GREEN } else { WHITE },
            )))
        })
        .collect()
}

/// A layer over a striped ground, for the filters that read what is behind.
fn grouped_over(name: &'static str, layer: LayerSpec, bounds: Option<[f32; 4]>) -> Scene {
    let mut items = behind();
    items.push(Node::Layer {
        layer: Box::new(layer),
        bounds,
        transform: Transform::default(),
        children: pair(),
    });
    Scene::tree(name, items)
        .with_background(DARK)
        .with_samples(4)
}

fn grouped(name: &'static str, layer: LayerSpec, bounds: Option<[f32; 4]>) -> Scene {
    Scene::tree(
        name,
        vec![Node::Layer {
            layer: Box::new(layer),
            bounds,
            transform: Transform::default(),
            children: pair(),
        }],
    )
    .with_background(DARK)
    .with_samples(4)
}

/// The rest of `aiks_dl_blur_unittests.cc` that needs no capability this
/// renderer lacks.
///
/// The file's mask-blur variants are the same shape drawn at each style
/// against translucent and opaque colors, which is precisely what the styles
/// were built for. What is still missing from it is unwritten rather than
/// blocked: the two obstacles this used to name -- a mask blur over a gradient,
/// and backdrop filters identified by a key -- have both since been built, and
/// the file's row in the inventory now says nothing stops the rest.
fn blur_variants() -> Vec<Scene> {
    let disc = |color: [f32; 4]| {
        Item::fill(
            Shape::Circle {
                center: [64.0, 64.0],
                radius: 32.0,
            },
            color,
        )
        .with_blend(BlendMode::SrcOver)
    };
    let translucent = [1.0, 0.4, 0.2, 0.5];
    let opaque = [1.0, 0.4, 0.2, 1.0];

    let mut scenes = vec![
        plate(
            "blur/mask-blur-variant-test-normal-translucent",
            vec![disc(translucent).with_mask_blur(6.0)],
        ),
        plate(
            "blur/mask-blur-variant-test-normal-translucent-zero-sigma",
            vec![disc(translucent).with_mask_blur(0.0)],
        ),
        plate(
            "blur/mask-blur-variant-test-solid-translucent",
            vec![disc(translucent)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Solid)],
        ),
        plate(
            "blur/mask-blur-variant-test-solid-opaque",
            vec![disc(opaque)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Solid)],
        ),
        plate(
            "blur/mask-blur-variant-test-inner-translucent",
            vec![disc(translucent)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Inner)],
        ),
        plate(
            "blur/mask-blur-variant-test-outer-translucent",
            vec![disc(translucent)
                .with_mask_blur(6.0)
                .with_mask_blur_style(MaskBlurStyle::Outer)],
        ),
        plate(
            "blur/a-blurred-image",
            // Nothing in this file blurred an image. Every blur here softens
            // coverage a tessellator produced -- a shape, a run of glyphs, a
            // layer of those -- and a blurred photograph is the commonest blur
            // there is. What differs is the source: the passes read a sampled
            // texture rather than a computed coverage, so the sheet's own
            // sampling and the blur's taps compose, and where the image ends is
            // where the tile mode decides what the taps outside it read.
            //
            // Checked once that the filter does something, which agreement
            // between the backends cannot say -- both would agree on a filter
            // that was dropped. Four texels left of the blurred rectangle the
            // ground has been disturbed; four texels right of the unblurred one
            // it is still exactly the background. The blurred sample is darker
            // than the ground rather than brighter, the sheet's edge there
            // being dark, so a check by luminosity would have called the blur
            // absent.
            vec![
                Item::filled(
                    Shape::Rect {
                        min: [8.0, 8.0],
                        max: [60.0, 60.0],
                    },
                    sheet(
                        [8.0, 8.0, 60.0, 60.0],
                        ALL,
                        TileMode::Clamp,
                        Sampling::Linear,
                    ),
                )
                .with_image_filter(ImageFilter::blur(4.0)),
                // Beside it unblurred at the same size, so the plate shows the
                // blur rather than the image.
                Item::filled(
                    Shape::Rect {
                        min: [68.0, 8.0],
                        max: [120.0, 60.0],
                    },
                    sheet(
                        [68.0, 8.0, 120.0, 60.0],
                        ALL,
                        TileMode::Clamp,
                        Sampling::Linear,
                    ),
                ),
                // And a stronger one over a smaller piece of the sheet, where
                // the reach is a larger share of the image and the edges carry
                // most of the picture.
                Item::filled(
                    Shape::Rect {
                        min: [30.0, 72.0],
                        max: [98.0, 120.0],
                    },
                    sheet(
                        [30.0, 72.0, 98.0, 120.0],
                        [0.25, 0.25, 0.75, 0.75],
                        TileMode::Clamp,
                        Sampling::Linear,
                    ),
                )
                .with_image_filter(ImageFilter::blur_xy(9.0, 0.0)),
            ],
        ),
        plate(
            "blur/blur-has-no-edge",
            vec![
                // Larger than the plate, so the halo is cut by the frame. A
                // bounded layer sized to its content alone puts a straight
                // edge where the blur should simply continue past the view.
                Item::fill(
                    Shape::Rect {
                        min: [-40.0, 40.0],
                        max: [168.0, 88.0],
                    },
                    WHITE,
                )
                .with_mask_blur(10.0)
                .with_blend(BlendMode::SrcOver),
            ],
        ),
        plate(
            "blur/dilate",
            // A cross, so that the plate shows what a dilation does to a
            // concave corner as well as to a convex one: the inner corners
            // fill in by the radius, which a single rectangle would not show.
            vec![
                Item::fill(
                    Shape::Rect {
                        min: [40.0, 20.0],
                        max: [88.0, 108.0],
                    },
                    WHITE,
                )
                .with_image_filter(ImageFilter::Dilate {
                    radius_x: 6.0,
                    radius_y: 6.0,
                }),
                Item::fill(
                    Shape::Rect {
                        min: [20.0, 40.0],
                        max: [108.0, 88.0],
                    },
                    WHITE,
                )
                .with_image_filter(ImageFilter::Dilate {
                    radius_x: 6.0,
                    radius_y: 6.0,
                }),
            ],
        ),
        plate(
            "blur/composed-filters",
            // Two filters composed, which is a chain of layers rather than one
            // -- the outermost is peeled off, its layer opened, and the rest
            // handed back to be drawn inside it. An erosion inside a dilation
            // is a closing, so the notch between the two arms fills while the
            // outline returns to about where it started: a picture neither
            // filter gives on its own, which is what makes it worth comparing
            // across backends.
            //
            // The radius has to beat half the notch or the closing is the
            // identity on this shape and the plate draws what it would have
            // drawn unfiltered. The notch is twenty-four across, so nine --
            // which is what this asked for until the default blend stopped
            // erasing the ground and made the plate's own picture visible --
            // filled nothing.
            vec![Item::fill(
                Shape::Polygon(vec![
                    [30.0, 34.0],
                    [98.0, 34.0],
                    [98.0, 94.0],
                    [76.0, 94.0],
                    [76.0, 62.0],
                    [52.0, 62.0],
                    [52.0, 94.0],
                    [30.0, 94.0],
                ]),
                WHITE,
            )
            .with_image_filter(ImageFilter::compose(
                ImageFilter::Erode {
                    radius_x: 14.0,
                    radius_y: 14.0,
                },
                ImageFilter::Dilate {
                    radius_x: 14.0,
                    radius_y: 14.0,
                },
            ))],
        ),
        plate(
            "blur/dilate-one-axis",
            // Radii that differ, which is what makes the structuring element a
            // rectangle rather than a square and the filter two passes rather
            // than one.
            vec![Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 24.0,
                },
                WHITE,
            )
            .with_image_filter(ImageFilter::Dilate {
                radius_x: 24.0,
                radius_y: 2.0,
            })],
        ),
        plate(
            "blur/gaussian-blur-one-dimension",
            vec![
                // A shape thin in one axis and long in the other, where a blur
                // that ran the same distance both ways would swallow it.
                Item::fill(
                    Shape::Rect {
                        min: [12.0, 60.0],
                        max: [116.0, 68.0],
                    },
                    WHITE,
                )
                .with_mask_blur(5.0)
                .with_blend(BlendMode::SrcOver),
            ],
        ),
        plate(
            "blur/solid-color-ovals-mask-blur-tiny-sigma",
            // Upstream's three, and it had been one. The scene is a sweep, not
            // a shape: the same oval at a deviation of nothing, of a hundredth,
            // and of one, so what it shows is where a mask blur starts having
            // an effect. A single oval at a made-up deviation in between shows
            // none of that, and was also below what
            // `every_plate_that_asks_for_a_mask_blur_can_show_one` will accept
            // -- it passed that check on an artifact rather than on its blur;
            // see the commit that gave these draws a blend.
            [
                (8.0f32, 0.0f32, GREEN),
                (52.0, 0.01, [1.0, 1.0, 0.0, 1.0]),
                (96.0, 1.0, RED),
            ]
            .into_iter()
            .map(|(x, sigma, color)| {
                Item::fill(
                    Shape::Oval {
                        min: [x, 32.0],
                        max: [x + 24.0, 96.0],
                    },
                    color,
                )
                .with_mask_blur(sigma)
                .with_blend(BlendMode::SrcOver)
            })
            .collect(),
        ),
    ];

    // Single-sampled, and not by preference. A backdrop filter has to read
    // what is already in the target, and a multisampled pass here must clear
    // rather than preserve -- preserving would need a resolved buffer copied
    // back into a multisampled one. So the two do not compose, and a scene
    // asking for both is refused by the backend rather than drawn wrongly.
    scenes.push(
        grouped_over(
            "blur/can-render-backdrop-blur",
            LayerSpec::default().with_backdrop_blur(6.0),
            Some([24.0, 44.0, 104.0, 84.0]),
        )
        .with_samples(1),
    );
    scenes.push(
        grouped_over(
            "blur/can-render-backdrop-blur-huge-sigma",
            LayerSpec::default().with_backdrop_blur(40.0),
            Some([24.0, 44.0, 104.0, 84.0]),
        )
        .with_samples(1),
    );
    scenes.push(grouped(
        "blur/can-render-bounded-blur",
        LayerSpec::default().with_blur(6.0),
        Some([16.0, 32.0, 112.0, 96.0]),
    ));
    scenes.push(
        Scene::tree(
            "blur/can-render-bounded-blur-with-translation",
            // A backdrop blur bounded to a rectangle, twice: once only moved,
            // the way a scrolled list item is, and once moved, scaled and
            // turned. Upstream states each rectangle twice over -- as a clip
            // and as the filter's own bounds. Here each subcase is one layer
            // whose bounds travel with its transform, and what that checks is
            // that they do: bounds left in the frame's axes would put the first
            // band in the wrong place and leave the second unmoved by its turn.
            //
            // The second band is upright here and tilted upstream, and that is
            // the scene's limit rather than the renderer's. A layer's target is
            // an axis-aligned box in device space -- upstream's is too -- so a
            // turned rectangle's bounds become the box around it, and upstream
            // gets its tilted edge from the clip it states under the same
            // transform. A clip that turns is something a scene can put on one
            // draw and not around a layer, so that edge has nothing to say it
            // with. Looked at rather than assumed: at thirty degrees the band is
            // the upright box around the turned rectangle.
            // `a_bounded_backdrop_blur_moves_with_the_transform_it_was_opened_under`
            // checks that each band's bounds follow its own transform.
            vec![
                Node::Draw(Box::new(Item::filled(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    sheet(
                        [0.0, 0.0, 128.0, 128.0],
                        ALL,
                        TileMode::Clamp,
                        Sampling::Nearest,
                    ),
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec::default().with_backdrop_blur(4.0)),
                    bounds: Some([0.0, 0.0, 44.0, 22.0]),
                    transform: Transform {
                        translate: [20.0, 28.0],
                        ..Transform::default()
                    },
                    children: Vec::new(),
                },
                // The turned one, and the turn is stated as a clip rather than
                // only as the layer's transform. A layer's target is an
                // axis-aligned box in device space, here and upstream alike, so
                // the bounds of a turned rectangle are the box around it; what
                // gives upstream's band its tilted edge is the clip it states
                // under the same transform, and this is that clip.
                Node::Clip {
                    rect: Some([2.0, 2.0, 40.0, 18.0]),
                    rect_out: None,
                    shape: None,
                    transform: Transform {
                        scale: [1.2, 1.2],
                        // Five degrees, as upstream turns it.
                        rotate: 5.0_f32.to_radians(),
                        translate: [12.0, 70.0],
                        ..Transform::default()
                    },
                    children: vec![Node::Layer {
                        layer: Box::new(LayerSpec::default().with_backdrop_blur(3.0)),
                        bounds: None,
                        transform: Transform::default(),
                        children: Vec::new(),
                    }],
                },
            ],
        )
        .with_background(DARK)
        .with_samples(4),
    );
    scenes.push({
        // Upstream's shader is a five-by-five checkerboard rendered into a
        // texture first, which is `Picture.toImage` and machinery this catalog
        // does not state. The fixture sheet stands in for it: what the scene is
        // about is an image-shaded paint drawn plain and through a blur, on a
        // rectangle and on strokes, and any image that varies says that. The
        // sheet lands in a twenty-pixel tile at the origin and repeats, as the
        // checkerboard did, so a shape away from the origin reads a tile other
        // than the first.
        let shader = || {
            sheet(
                [4.0, 4.0, 24.0, 24.0],
                ALL,
                TileMode::Repeat,
                Sampling::Linear,
            )
        };
        // Scaled from upstream's six hundred pixels by a fifth. The blur is
        // doubled from what that scale gives, since a deviation of one does not
        // show at this size, and the strokes are two pixels rather than one.
        let blur = ImageFilter::blur(2.0);
        let crossed = |dx: f32, dy: f32, filter: ImageFilter| -> Vec<Node> {
            let at = |x: f32, y: f32| [4.0 + dx + x, 4.0 + dy + y];
            [
                (at(20.0, 20.0), at(40.0, 40.0)),
                (at(20.0, 40.0), at(40.0, 20.0)),
                (at(30.0, 20.0), at(40.0, 30.0)),
                (at(20.0, 30.0), at(30.0, 40.0)),
            ]
            .into_iter()
            .map(|(from, to)| {
                Node::Draw(Box::new(
                    Item {
                        stroke: Some(StrokeSpec::new(2.0)),
                        ..Item::filled(Shape::Line { from, to }, shader())
                    }
                    .with_image_filter(filter.clone()),
                ))
            })
            .collect()
        };
        let mut nodes = vec![
            Node::Draw(Box::new(Item::fill(
                Shape::Rect {
                    min: [4.0, 4.0],
                    max: [64.0, 124.0],
                },
                [0.0, 100.0 / 255.0, 0.0, 1.0],
            ))),
            Node::Draw(Box::new(Item::filled(
                Shape::Rect {
                    min: [24.0, 24.0],
                    max: [44.0, 44.0],
                },
                shader(),
            ))),
            Node::Draw(Box::new(Item::fill(
                Shape::Rect {
                    min: [64.0, 4.0],
                    max: [124.0, 124.0],
                },
                RED,
            ))),
            Node::Draw(Box::new(
                Item::filled(
                    Shape::Rect {
                        min: [84.0, 24.0],
                        max: [104.0, 44.0],
                    },
                    shader(),
                )
                .with_image_filter(blur.clone()),
            )),
        ];
        nodes.extend(crossed(0.0, 60.0, ImageFilter::None));
        nodes.extend(crossed(60.0, 60.0, blur));
        Scene::tree("blur/blurred-rectangle-with-shader", nodes)
            .with_background(DARK)
            .with_samples(4)
    });
    scenes.push(
        Scene::tree(
            "blur/blur-under-a-rotated-scale",
            // A blur is a device-space filter over a finished layer: it runs on
            // the target, in the target's own axes, after the transform has
            // already placed everything. A rotation alone cannot show that --
            // an isotropic blur in the shape's own space is still isotropic
            // after being turned -- so the transform here scales unevenly as
            // well. Blurred in the shape's space and then scaled, the halo
            // would come out four times wider than tall and turned with the
            // shape; blurred on the target it is the same distance every way.
            vec![Node::Layer {
                layer: Box::new(LayerSpec::default().with_blur(5.0)),
                bounds: Some([0.0, 0.0, 128.0, 128.0]),
                transform: Transform {
                    scale: [2.0, 0.5],
                    rotate: 0.6,
                    skew: [0.0, 0.0],
                    translate: [64.0, 64.0],
                    perspective: [0.0, 0.0],
                },
                children: vec![Node::Draw(Box::new(Item::fill(
                    Shape::Rect {
                        min: [-3.0, -3.0],
                        max: [3.0, 3.0],
                    },
                    WHITE,
                )))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    );
    scenes.push(
        Scene::tree(
            "blur/blur-under-rotation-and-clip",
            // Both at once, which is the combination worth its own plate. The
            // clip is stated in the rotated space the shape is drawn in and
            // cuts the disc in half there; the blur then runs over the layer
            // the clip has already cut, so the straight edge is softened along
            // with the curved one. That is what blurring a group means, and it
            // is the opposite picture from clipping a blurred result -- where
            // the cut would stay hard and the halo would stop dead at it.
            vec![Node::Layer {
                layer: Box::new(LayerSpec::default().with_blur(5.0)),
                bounds: Some([0.0, 0.0, 128.0, 128.0]),
                transform: Transform {
                    rotate: 0.6,
                    translate: [64.0, 64.0],
                    ..Transform::default()
                },
                children: vec![Node::Draw(Box::new(
                    Item::fill(
                        Shape::Circle {
                            center: [0.0, 0.0],
                            radius: 40.0,
                        },
                        WHITE,
                    )
                    .with_clip([-40.0, -40.0, 0.0, 40.0]),
                ))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    // A stroked stadium filled with a gradient and mask blurred, one plate per
    // style. Every other mask-blur plate above fills its shape with a flat
    // color, and the two white guide lines under this one are upstream's: a mask
    // blur that stretched what it blurred rather than blurring the coverage
    // would move the stroke off them.
    //
    // The crossing is the point. A mask blur acts on coverage and the fill
    // colors whatever survives, so a stroke gives it coverage that is a band
    // rather than a disc, and a gradient makes it obvious if the fill were
    // sampled at the blurred position instead of the shape's own.
    let stadium = || Shape::RoundedRect {
        min: [14.0, 44.0],
        max: [114.0, 74.0],
        // Larger than half the shorter side, so it clamps and the shape is a
        // stadium -- which is what upstream's 50-by-100 on a 200-by-60 rect
        // gives, and the reason this reads as an oval rather than a rectangle.
        radius: 25.0,
    };
    let guides = || {
        vec![
            Item::stroke(
                Shape::Polyline(vec![[64.0, 40.0], [64.0, 78.0]]),
                StrokeSpec::new(1.0),
                WHITE,
            ),
            Item::stroke(
                Shape::Polyline(vec![[10.0, 59.0], [118.0, 59.0]]),
                StrokeSpec::new(1.0),
                WHITE,
            ),
        ]
    };
    let banded = |sigma: f32, style: MaskBlurStyle| {
        let mut items = guides();
        items.push(
            Item::filled(
                stadium(),
                Fill::LinearGradient {
                    start: [0.0, 0.0],
                    end: [128.0, 128.0],
                    stops: vec![Stop::new(RED, 0.0), Stop::new(BLUE, 1.0)],
                    tile: TileMode::Clamp,
                },
            )
            .with_stroke(StrokeSpec::new(10.0))
            .with_mask_blur(sigma)
            .with_mask_blur_style(style)
            .with_blend(BlendMode::SrcOver),
        );
        items
    };

    // The same five turned, which is upstream's own second set of them. A blur
    // runs on the target in the target's own axes, and the input it is handed
    // is snapshotted in a space with the rotation taken out -- so a snapshot
    // that kept a property it should not, or a bound computed in the wrong one
    // of those two spaces, cuts the stroke off along a line that is not in the
    // picture. Upstream's fix was exactly that, and these are the plates it
    // came with.
    //
    // Turned about the stadium's own center rather than about the origin after
    // a translation, as upstream does it: the frame here is 128 pixels and a
    // rotation about the origin would carry the shape out of it.
    let turned = |degrees: f32| {
        let (cx, cy) = (64.0f32, 59.0f32);
        let radians = degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        Transform {
            rotate: radians,
            translate: [cx - (cx * cos - cy * sin), cy - (cx * sin + cy * cos)],
            ..Transform::default()
        }
    };

    for (name, sigma, style) in [
        (
            "blur/gradient-oval-stroke-mask-blur-translated-and-rotated",
            5.0,
            MaskBlurStyle::Normal,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-sigma-zero-translated-and-rotated",
            0.0,
            MaskBlurStyle::Normal,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-outer-translated-and-rotated",
            5.0,
            MaskBlurStyle::Outer,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-inner-translated-and-rotated",
            5.0,
            MaskBlurStyle::Inner,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-solid-translated-and-rotated",
            5.0,
            MaskBlurStyle::Solid,
        ),
    ] {
        let items = banded(sigma, style)
            .into_iter()
            .map(|item| item.with_transform(turned(45.0)))
            .collect();
        scenes.push(plate(name, items));
    }

    for (name, sigma, style) in [
        (
            "blur/gradient-oval-stroke-mask-blur",
            5.0,
            MaskBlurStyle::Normal,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-sigma-zero",
            0.0,
            MaskBlurStyle::Normal,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-outer",
            5.0,
            MaskBlurStyle::Outer,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-inner",
            5.0,
            MaskBlurStyle::Inner,
        ),
        (
            "blur/gradient-oval-stroke-mask-blur-solid",
            5.0,
            MaskBlurStyle::Solid,
        ),
    ] {
        scenes.push(plate(name, banded(sigma, style)));
    }

    // The blur styles again, over a gradient-filled triangle, and each followed
    // by a flat rectangle that overlaps nothing of it.
    //
    // That rectangle is the subject and is upstream's. Inner and solid are
    // implemented by clipping to the shape before compositing the blurred
    // coverage, and a clip that is not popped afterwards is invisible in a
    // plate holding one draw -- there is nothing after it to be wrongly
    // clipped. So the plate holds something after it, in a corner the triangle
    // does not reach: if the clip leaked, the rectangle is missing rather than
    // merely different.
    let triangle = || Shape::Polygon(vec![[64.0, 34.0], [94.0, 94.0], [34.0, 94.0]]);
    for (name, style) in [
        (
            "blur/gaussian-blur-style-inner-gradient",
            MaskBlurStyle::Inner,
        ),
        (
            "blur/gaussian-blur-style-outer-gradient",
            MaskBlurStyle::Outer,
        ),
        (
            "blur/gaussian-blur-style-solid-gradient",
            MaskBlurStyle::Solid,
        ),
    ] {
        scenes.push(plate(
            name,
            vec![
                Item::filled(
                    triangle(),
                    Fill::LinearGradient {
                        start: [0.0, 0.0],
                        end: [64.0, 64.0],
                        stops: vec![
                            Stop::new([0.957, 0.263, 0.212, 1.0], 0.0),
                            Stop::new([0.757, 0.263, 0.212, 1.0], 1.0),
                        ],
                        tile: TileMode::Mirror,
                    },
                )
                .with_mask_blur(6.0)
                .with_mask_blur_style(style)
                .with_blend(BlendMode::SrcOver),
                Item::fill(
                    Shape::Rect {
                        min: [4.0, 4.0],
                        max: [30.0, 30.0],
                    },
                    RED,
                )
                .with_blend(BlendMode::SrcOver),
            ],
        ));
    }

    // The channel swap upstream composes with a blur, in both orders.
    let swap = || {
        ImageFilter::Color(ColorFilter::matrix([
            0.0, 1.0, 0.0, 0.0, 0.0, //
            1.0, 0.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, 0.0,
        ]))
    };
    for (name, filter) in [
        // Upstream calls the recolor-then-blur order "inner" and the other
        // "outer", after which of the two is nearer the drawing.
        (
            "blur/compose-paint-blur-inner",
            ImageFilter::compose(swap(), ImageFilter::blur(6.0)),
        ),
        (
            "blur/compose-paint-blur-outer",
            ImageFilter::compose(ImageFilter::blur(6.0), swap()),
        ),
    ] {
        scenes.push(
            Scene::new(
                name,
                // Green, so the swap is loud: it takes green to red, and both
                // orders end green-to-red -- what differs is where the halo
                // gets its color, since one blurs a red disc and the other
                // recolors a blurred green one. On a solid color those agree,
                // and the ground they are drawn over is what makes them not.
                vec![Item::fill(
                    Shape::Circle {
                        center: [64.0, 64.0],
                        radius: 36.0,
                    },
                    GREEN,
                )
                .with_image_filter(filter)
                .with_blend(BlendMode::SrcOver)],
            )
            .with_background(DARK)
            .with_samples(4),
        );
    }

    scenes.push(plate(
        "blur/can-render-clipped-blur",
        // A blurred circle whose clip cuts it after the blur rather than
        // before. The halo stops dead at the clip's edge and the circle's
        // own edge stays soft, which is the opposite of clipping the shape
        // and blurring what is left -- that would soften the cut too.
        vec![Item::fill(
            Shape::Circle {
                center: [76.0, 76.0],
                radius: 36.0,
            },
            GREEN,
        )
        .with_image_filter(ImageFilter::blur(6.0))
        .with_clip([16.0, 24.0, 112.0, 112.0])
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(plate(
        "blur/can-render-foreground-blend-with-mask-blur",
        // A color filter over a mask blur, which upstream keeps because
        // the two are easy to apply in the wrong order. The filter acts on
        // the color and the mask decides where that color lands, so the
        // result is a green disc with a soft edge -- not a green disc with
        // a hard edge, which is what applying the filter to the finished
        // coverage would give.
        vec![Item::fill(
            Shape::Circle {
                center: [76.0, 76.0],
                radius: 36.0,
            },
            WHITE,
        )
        .with_color_filter(ColorFilter::blend(GREEN, BlendMode::Src))
        .with_mask_blur(6.0)
        .with_clip([16.0, 24.0, 112.0, 112.0])
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(
        Scene::tree(
            "blur/gaussian-blur-at-periphery-vertical",
            // The horizontal plate's twin, turned. A strip down the middle of
            // the frame runs to the top and bottom edges, so the kernel reaches
            // past the target along the axis this one is read for. The ground's
            // stripes turn with it: a blur reading past the top row and finding
            // the clear rather than the clamped edge texel shows against bars
            // it crosses and not against bars it runs along.
            [
                banded_ground(),
                vec![Node::Layer {
                    layer: Box::new(LayerSpec {
                        backdrop_blur: 8.0,
                        blend: BlendMode::Src,
                        ..LayerSpec::default()
                    }),
                    bounds: Some([47.0, 0.0, 81.0, 128.0]),
                    transform: Transform::default(),
                    children: Vec::new(),
                }],
            ]
            .concat(),
        )
        .with_background(DARK)
        .with_samples(1),
    );

    scenes.push(
        Scene::tree(
            "blur/gaussian-blur-flipped",
            // A blurred layer under a mirrored transform. The determinant is
            // negative, so a renderer deriving the layer's extent by
            // transforming its corners and subtracting gets a negative width
            // and a target of no size -- which draws nothing at all rather
            // than drawing something wrong, and is the failure upstream keeps
            // this scene for.
            vec![
                Node::Draw(Box::new(Item::fill(
                    Shape::Rect {
                        min: [0.0, 0.0],
                        max: [128.0, 128.0],
                    },
                    WHITE,
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        filter: ImageFilter::blur(6.0),
                        ..LayerSpec::default()
                    }),
                    bounds: Some([16.0, 40.0, 112.0, 88.0]),
                    transform: Transform {
                        scale: [-1.0, 1.0],
                        translate: [128.0, 0.0],
                        ..Transform::default()
                    },
                    // Left of center in the layer's own space, which is right
                    // of center on the frame. Symmetric content would have made
                    // the mirror invisible and the plate worthless.
                    children: vec![Node::Draw(Box::new(
                        Item::fill(
                            Shape::Rect {
                                min: [24.0, 8.0],
                                max: [60.0, 120.0],
                            },
                            RED,
                        )
                        .with_blend(BlendMode::SrcOver),
                    ))],
                },
            ],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    scenes.push(
        Scene::tree(
            "blur/blur-gradient-with-opacity",
            // A gradient under a mask blur, inside a group at half opacity. The
            // mask blur over a varying fill is the route that draws the fill
            // across everything the blur reaches and masks it with a blurred
            // coverage, so it is three layers deep before the group's own
            // opacity is applied -- and the opacity has to reach the composite
            // of all of it rather than any one of them.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    alpha: 0.5,
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(
                    Item::filled(
                        Shape::Rect {
                            min: [28.0, 28.0],
                            max: [100.0, 100.0],
                        },
                        Fill::LinearGradient {
                            start: [28.0, 28.0],
                            end: [100.0, 100.0],
                            stops: vec![Stop::new(RED, 0.0), Stop::new(GREEN, 1.0)],
                            tile: TileMode::Clamp,
                        },
                    )
                    .with_mask_blur(4.0)
                    .with_blend(BlendMode::SrcOver),
                ))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    scenes.push(plate(
        "blur/mask-blur-texture",
        // An image under a mask blur, with an opaque rectangle beside it so
        // the halo has something to be read against. A mask blur acts on
        // coverage and the fill colors whatever survives, so an image's
        // coverage is the rectangle it is drawn into -- the picture is the
        // image with soft edges, not a blurred image.
        vec![
            Item::filled(
                Shape::Rect {
                    min: [40.0, 40.0],
                    max: [112.0, 112.0],
                },
                sheet(
                    [40.0, 40.0, 112.0, 112.0],
                    ALL,
                    TileMode::Clamp,
                    Sampling::Nearest,
                ),
            )
            .with_mask_blur(6.0)
            .with_blend(BlendMode::SrcOver),
            Item::fill(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [48.0, 48.0],
                },
                RED,
            )
            .with_blend(BlendMode::SrcOver),
        ],
    ));

    // The same blurred image clipped to a window, once scaled and once scaled
    // and turned. Upstream keeps both because the clip is stated outside the
    // transform: the window stays put on the frame while what is drawn into it
    // moves, so the blur's target is decided by one space and its contents by
    // another.
    for (name, rotate) in [
        ("blur/gaussian-blur-scaled-and-clipped", 0.0f32),
        (
            "blur/gaussian-blur-rotated-and-clipped",
            25.0f32.to_radians(),
        ),
    ] {
        scenes.push(
            Scene::tree(
                name,
                vec![Node::Layer {
                    layer: Box::new(LayerSpec::default()),
                    bounds: Some([34.0, 45.0, 94.0, 83.0]),
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(
                        Item::filled(
                            Shape::Rect {
                                min: [-56.0, -42.0],
                                max: [56.0, 42.0],
                            },
                            sheet(
                                [-56.0, -42.0, 56.0, 42.0],
                                ALL,
                                TileMode::Clamp,
                                Sampling::Nearest,
                            ),
                        )
                        .with_image_filter(ImageFilter::blur(4.0))
                        .with_transform(Transform {
                            scale: [0.6, 0.6],
                            rotate,
                            translate: [64.0, 64.0],
                            ..Transform::default()
                        })
                        .with_blend(BlendMode::SrcOver),
                    ))],
                }],
            )
            .with_background(DARK)
            .with_samples(4),
        );
    }

    scenes.push(plate(
        "blur/clipped-blur-filter-renders-correctly",
        // A shape larger than the frame, moved so most of it is off the
        // top, under a mask blur wide enough that the halo alone would fill
        // the picture. The second contour is upstream's and is the reason
        // the scene exists: a path holding more than one contour cannot be
        // recognized as a rounded rectangle, so the blur cannot be
        // evaluated in the fragment stage and has to take the general
        // route -- on a target sized for a shape most of which is not
        // there.
        vec![Item::fill(
            Shape::Contours(vec![
                vec![[0.0, 0.0], [128.0, 0.0], [128.0, 128.0], [0.0, 128.0]],
                vec![[0.0, 0.0], [0.5, 0.0], [0.5, 0.5]],
            ]),
            RED,
        )
        .with_mask_blur(16.0)
        .with_transform(Transform::translate(0.0, -64.0))
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes.push(plate(
        "blur/clear-blend-with-blur",
        // A blurred shape drawn with a mode that discards its destination,
        // so what the blur produces is not color but how much is taken
        // away. The hole's edge is the blur's falloff read as erasure.
        //
        // This plate could not exist until `Clear` was let past the
        // evaluated blur's coverage guard: on the route it used to take,
        // the layer's composite covered the layer's bounds and cleared a
        // rectangle.
        vec![
            Item::fill(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                BLUE,
            ),
            Item::fill(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 36.0,
                },
                WHITE,
            )
            .with_mask_blur(7.0)
            .with_blend(BlendMode::Clear),
        ],
    ));

    scenes.push(
        Scene::tree(
            "blur/gaussian-blur-rotated-non-uniform",
            // A blur along one axis only, on content turned off that axis.
            // Upstream draws this at forty-five degrees under a scale, and it
            // is the scene that says a deviation is stated in the caller's
            // space rather than the target's: the smear runs along the shape's
            // own axis, not along the screen's.
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    filter: ImageFilter::blur_xy(11.0, 0.0),
                    ..LayerSpec::default()
                }),
                bounds: None,
                transform: Transform {
                    scale: [0.6, 0.6],
                    rotate: 45.0f32.to_radians(),
                    translate: [64.0, 64.0],
                    ..Transform::default()
                },
                children: vec![Node::Draw(Box::new(
                    Item::fill(
                        Shape::Rect {
                            min: [-50.0, -50.0],
                            max: [50.0, 50.0],
                        },
                        GREEN,
                    )
                    .with_blend(BlendMode::SrcOver),
                ))],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    );

    // A deviation per axis, which `dart:ui` states and this renderer now
    // carries. Two squares, one blurred along x alone and one along y, so
    // either plate on its own says the sigma arrived and the pair says which
    // of the two it was. Upstream's own scene for this turns the whole thing
    // and is not mirrored -- see `docs/non-parity.md` section 14.
    for (name, filter) in [
        ("blur/a-blur-along-one-axis", ImageFilter::blur_xy(9.0, 0.0)),
        (
            "blur/a-blur-along-the-other-axis",
            ImageFilter::blur_xy(0.0, 9.0),
        ),
    ] {
        scenes.push(
            Scene::tree(
                name,
                vec![Node::Layer {
                    layer: Box::new(LayerSpec {
                        filter,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(Item::fill(
                        Shape::Rect {
                            min: [48.0, 48.0],
                            max: [80.0, 80.0],
                        },
                        WHITE,
                    )))],
                }],
            )
            .with_background(DARK)
            .with_samples(4),
        );
    }

    scenes.push(
        Scene::tree(
            "blur/gaussian-blur-at-periphery-horizontal",
            // A backdrop blur whose region runs to the frame's own edge, so the
            // kernel reaches past the target on one side and has to answer with
            // the clamped edge texel rather than with whatever is there. The
            // band is deliberately at the top: a blur that read past the top row
            // and found the clear would darken the band's upper half, and even
            // bars under it make that visible where a flat color would not.
            [
                barred_ground(),
                vec![Node::Layer {
                    layer: Box::new(LayerSpec {
                        backdrop_blur: 8.0,
                        blend: BlendMode::Src,
                        ..LayerSpec::default()
                    }),
                    bounds: Some([0.0, 0.0, 128.0, 34.0]),
                    transform: Transform::default(),
                    children: Vec::new(),
                }],
            ]
            .concat(),
        )
        .with_background(DARK)
        .with_samples(1),
    );

    scenes.push(
        Scene::tree(
            "blur/can-render-nested-backdrop-blur",
            // A backdrop inside a backdrop. The inner group filters what the
            // outer one had already filtered, so the middle of the plate is
            // blurred twice and the ring around it once -- and a renderer that
            // seeded the inner group from the *unfiltered* target would leave
            // the middle looking like the outside.
            [
                barred_ground(),
                vec![Node::Layer {
                    layer: Box::new(LayerSpec {
                        backdrop_blur: 5.0,
                        blend: BlendMode::Src,
                        ..LayerSpec::default()
                    }),
                    bounds: Some([12.0, 12.0, 116.0, 116.0]),
                    transform: Transform::default(),
                    children: vec![Node::Layer {
                        layer: Box::new(LayerSpec {
                            backdrop_blur: 5.0,
                            blend: BlendMode::Src,
                            ..LayerSpec::default()
                        }),
                        bounds: Some([40.0, 40.0, 88.0, 88.0]),
                        transform: Transform::default(),
                        children: Vec::new(),
                    }],
                }],
            ]
            .concat(),
        )
        .with_background(DARK)
        .with_samples(1),
    );

    scenes.push(plate(
        "blur/mask-blur-doesnt-stretch-contents",
        // A mask blur over an image, under a scale. The blur acts on coverage
        // and the image fills what survives, so the image keeps the size the
        // transform gave it -- a renderer that blurred the filled result would
        // spread the picture as well as the outline, and the sheet's own edges
        // are what show that. A flat fill could not.
        vec![Item::filled(
            Shape::Rect {
                min: [24.0, 24.0],
                max: [104.0, 104.0],
            },
            sheet(
                [24.0, 24.0, 104.0, 104.0],
                ALL,
                TileMode::Clamp,
                Sampling::Linear,
            ),
        )
        .with_mask_blur(9.0)
        .with_blend(BlendMode::SrcOver)],
    ));

    scenes
}

/// A ground of even bars, for the plates that measure what a blur did to it.
///
/// A photograph will not do, and that is measured rather than assumed: the
/// fixture sheet's own detail varies across it, so comparing how much a region
/// varies before and after a blur compares the sheet's content as much as the
/// blur's work -- the first version of the two backdrop plates below came out
/// saying an unblurred corner was smoother than a twice-blurred middle, which
/// is the sheet being flat there. Even bars vary the same everywhere, so what
/// changes between regions is what was done to them.
fn barred_ground() -> Vec<Node> {
    ruled_ground(false)
}

/// The same ground with its stripes the other way round.
///
/// Which way they run is not decoration. A blur reading past the edge of its
/// target has to answer with the clamped edge texel, and the way to see that it
/// did is to blur *across* the stripes -- so a band at the top of the frame is
/// read against vertical bars, and a strip down the side against horizontal
/// ones. Stripes parallel to the blur would look identical either way.
fn banded_ground() -> Vec<Node> {
    ruled_ground(true)
}

fn ruled_ground(horizontal: bool) -> Vec<Node> {
    let mut items = vec![Node::Draw(Box::new(Item::fill(
        Shape::Rect {
            min: [0.0, 0.0],
            max: [128.0, 128.0],
        },
        WHITE,
    )))];
    for i in 0..16 {
        let at = i as f32 * 8.0;
        let (min, max) = if horizontal {
            ([0.0, at], [128.0, at + 4.0])
        } else {
            ([at, 0.0], [at + 4.0, 128.0])
        };
        items.push(Node::Draw(Box::new(Item::fill(
            Shape::Rect { min, max },
            [0.05, 0.05, 0.1, 1.0],
        ))));
    }
    items
}

/// `aiks_dl_unittests.cc` and `aiks_dl_opacity_unittests.cc`, for the scenes
/// about grouping rather than about any one shape.
fn layers() -> Vec<Scene> {
    vec![
        // The same question asked of an image rather than of two shapes, which
        // is upstream's own variant of it. What a translucent group is worth
        // depends on the group being finished before the alpha is applied, and
        // a photograph is the operand that shows it: every texel differs, so a
        // group faded per-draw instead of per-group would not merely be the
        // wrong strength, it would be the wrong picture where the two copies
        // overlap.
        //
        // Both copies of the sheet are in the frame at once, one straight and
        // one through the group, so the plate carries its own reference.
        Scene::tree(
            "dl/translucent-save-layer-image-draws-correctly",
            vec![
                Node::Draw(Box::new(Item::filled(
                    Shape::Rect {
                        min: [8.0, 8.0],
                        max: [64.0, 64.0],
                    },
                    sheet(
                        [8.0, 8.0, 64.0, 64.0],
                        ALL,
                        TileMode::Clamp,
                        Sampling::Linear,
                    ),
                ))),
                Node::Layer {
                    layer: Box::new(LayerSpec {
                        alpha: 0.5,
                        ..LayerSpec::default()
                    }),
                    bounds: None,
                    transform: Transform::default(),
                    children: vec![Node::Draw(Box::new(Item::filled(
                        Shape::Rect {
                            min: [64.0, 64.0],
                            max: [120.0, 120.0],
                        },
                        sheet(
                            [64.0, 64.0, 120.0, 120.0],
                            ALL,
                            TileMode::Clamp,
                            Sampling::Linear,
                        ),
                    )))],
                },
            ],
        )
        .with_background(DARK)
        .with_samples(4),
        grouped(
            "dl/translucent-save-layer-draws-correctly",
            LayerSpec {
                alpha: 0.45,
                ..LayerSpec::default()
            },
            None,
        ),
        // The family's other half: a filter on the *group* rather than a color
        // filter on its way out. Upstream keeps four of these and the
        // distinction they turn on is the same one -- what a translucent group
        // is worth depends on when the recoloring happens relative to the
        // alpha, and a filter that is not a plain scale is what tells the
        // orders apart.
        grouped(
            "dl/translucent-save-layer-with-blend-image-filter-draws-correctly",
            LayerSpec {
                alpha: 0.45,
                // Destination-over against a constant, as an *image* filter --
                // so it runs over the finished group where the entry below runs
                // over each color on its way out. The pair is here to show the
                // two are not the same picture.
                filter: ImageFilter::Color(ColorFilter::blend(RED, BlendMode::DstOver)),
                ..LayerSpec::default()
            },
            None,
        ),
        grouped(
            "dl/translucent-save-layer-with-color-matrix-image-filter-draws-correctly",
            // Upstream's matrix, which doubles alpha and leaves color alone --
            // and this plate moves barely one per cent of its frame against the
            // unfiltered group, which is right rather than broken. The group's
            // content is opaque, so doubling its alpha clamps to what it
            // already was; only the edges, where coverage is partial, have
            // anywhere to go. It is kept because it is upstream's, and it is
            // still a different picture from every other plate in this family
            // by between a quarter and the whole frame.
            LayerSpec {
                alpha: 0.45,
                filter: ImageFilter::Color(ColorFilter::matrix([
                    1.0, 0.0, 0.0, 0.0, 0.0, //
                    0.0, 1.0, 0.0, 0.0, 0.0, //
                    0.0, 0.0, 1.0, 0.0, 0.0, //
                    0.0, 0.0, 0.0, 2.0, 0.0,
                ])),
                ..LayerSpec::default()
            },
            None,
        ),
        grouped(
            "dl/translucent-save-layer-with-color-filter-and-image-filter-draws-correctly",
            LayerSpec {
                alpha: 0.5,
                // Upstream's other pairing of the two, and the one that says
                // the image filter stage is not the color filter stage wearing
                // a different name: here the *image* filter is a color matrix,
                // so both stages recolor and only their order tells them apart.
                // The matrix leaks green into blue and halves alpha, and the
                // color filter modulates by green, so a renderer running them
                // the other way round leaves a different amount of blue.
                filter: ImageFilter::Color(ColorFilter::matrix([
                    1.0, 0.0, 0.0, 0.0, 0.0, //
                    0.0, 1.0, 0.0, 0.0, 0.0, //
                    0.0, 0.2, 1.0, 0.0, 0.0, //
                    0.0, 0.0, 0.0, 0.5, 0.0,
                ])),
                color_filter: ColorFilter::blend(GREEN, BlendMode::Modulate),
                ..LayerSpec::default()
            },
            None,
        ),
        grouped(
            "dl/translucent-save-layer-with-color-and-image-filter-draws-correctly",
            LayerSpec {
                alpha: 0.45,
                // Both at once, which is the scene that says they are separate
                // stages rather than two spellings: the color filter recolors
                // what the group produced, and the image filter then blurs
                // that. Either alone is a different plate.
                color_filter: ColorFilter::matrix([
                    0.0, 0.0, 1.0, 0.0, 0.0, //
                    0.0, 1.0, 0.0, 0.0, 0.0, //
                    1.0, 0.0, 0.0, 0.0, 0.0, //
                    0.0, 0.0, 0.0, 1.0, 0.0,
                ]),
                filter: ImageFilter::blur(4.0),
                ..LayerSpec::default()
            },
            None,
        ),
        // The same group with a filter on it, twice. Upstream keeps eight of
        // these and the family is the point: a translucent group has two things
        // to get in the right order, the alpha it composites with and whatever
        // recolors it on the way out. Applying the filter after the alpha, or
        // folding the alpha into the filter, gives a different picture from
        // applying the filter to the finished group and then compositing it --
        // and only a filter that is not a plain scale can tell them apart,
        // which is why neither of these is one.
        grouped(
            "dl/translucent-save-layer-with-blend-color-filter-draws-correctly",
            LayerSpec {
                alpha: 0.45,
                // Destination-over against a constant: the group is composited
                // over red rather than red over the group, so what the filter
                // contributes is strongest where the group is thinnest. That
                // reads the group's own alpha, which is what makes it a test of
                // the order rather than of the color.
                color_filter: ColorFilter::blend(RED, BlendMode::DstOver),
                ..LayerSpec::default()
            },
            None,
        ),
        grouped(
            "dl/translucent-save-layer-with-color-matrix-color-filter-draws-correctly",
            LayerSpec {
                alpha: 0.45,
                // Upstream's matrix: identity on color and twice on alpha. It
                // is chosen to fight the layer's own alpha rather than to look
                // like anything, and the two have to compose in one order only
                // -- doubling what is already at forty-five hundredths is not
                // the same as halving what has been doubled.
                color_filter: ColorFilter::matrix([
                    1.0, 0.0, 0.0, 0.0, 0.0, //
                    0.0, 1.0, 0.0, 0.0, 0.0, //
                    0.0, 0.0, 1.0, 0.0, 0.0, //
                    0.0, 0.0, 0.0, 2.0, 0.0,
                ]),
                ..LayerSpec::default()
            },
            None,
        ),
        grouped(
            "dl/can-render-destructive-save-layer",
            LayerSpec {
                // Replaces rather than composites, so the group erases what is
                // under it including the ground.
                blend: BlendMode::Src,
                ..LayerSpec::default()
            },
            Some([24.0, 34.0, 108.0, 94.0]),
        ),
        Scene::tree(
            "dl/can-render-tiny-overlapping-subpasses",
            (0..6)
                .map(|i| {
                    let x = 20.0 + i as f32 * 15.0;
                    Node::Layer {
                        layer: Box::new(LayerSpec {
                            alpha: 0.7,
                            ..LayerSpec::default()
                        }),
                        bounds: Some([x, 50.0, x + 22.0, 78.0]),
                        transform: Transform::default(),
                        children: vec![Node::Draw(Box::new(
                            Item::fill(
                                Shape::Circle {
                                    center: [x + 11.0, 64.0],
                                    radius: 12.0,
                                },
                                [0.3 + 0.12 * i as f32, 0.9 - 0.1 * i as f32, 1.0, 1.0],
                            )
                            .with_blend(BlendMode::SrcOver),
                        ))],
                    }
                })
                .collect(),
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}

/// `aiks_dl_runtime_effect_unittests.cc`.
///
/// That file's scenes are a caller's shaders doing things no material does --
/// which is the whole category, so what can be shown here is bounded by the
/// one fixture program rather than by the renderer. What these plates
/// establish is that a caller's program reaches the picture at all, through a
/// shape, through a transform, and beside the built-in shader.
fn runtime_effect() -> Vec<Scene> {
    let effect = |threshold: f32| Fill::RuntimeEffect {
        program: 0,
        uniforms: crate::fixture::effect_uniforms(RED, BLUE, threshold),
        images: Vec::new(),
    };
    vec![
        plate(
            "effect/can-render-runtime-effect",
            vec![Item::filled(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                effect(0.0),
            )],
        ),
        Scene::tree(
            "effect/draw-paint-transforms-bounds",
            // A program with no shape at all: `drawPaint` takes a paint, and a
            // paint carries a shader, so what a program fills here is whatever
            // the clip admits. Upstream's name is about the bounds -- what a
            // paint covers is the clip carried back through the transform, and
            // its scene draws one under a scale to say the transform reached
            // that and not only the drawing.
            //
            // The program splits on the clip-space coordinate, so no transform
            // moves the split: what this plate has to show is coverage, and a
            // rotation is the case where getting the bounds wrong shows. A
            // paint covers the clip carried back through the transform, and a
            // renderer that took the clip forward instead would fill a turned
            // rectangle -- leaving the frame's corners empty, which is the
            // whole of what the failure looks like.
            vec![Node::Paint(Box::new(PaintSpec {
                fill: effect(0.0),
                blend: BlendMode::SrcOver,
                clip: None,
                clip_out: None,
                transform: Transform {
                    rotate: 30.0f32.to_radians(),
                    translate: [64.0, 64.0],
                    ..Transform::default()
                },
            }))],
        )
        .with_background(DARK)
        .with_samples(4),
        plate(
            "effect/can-render-clipped-runtime-effects",
            // A caller's program filling a rectangle, cut to a rounded one. A
            // program replaces this renderer's fragment shader outright, so
            // the clip cannot be arithmetic inside it and has to come from the
            // stencil -- which makes this the plate that says a program's draw
            // is clipped like any other rather than being a special case that
            // escaped the machinery around it.
            vec![Item::filled(
                Shape::Rect {
                    min: [0.0, 0.0],
                    max: [128.0, 128.0],
                },
                effect(0.0),
            )
            .with_clip_shape(Shape::RoundedRect {
                min: [8.0, 8.0],
                max: [120.0, 120.0],
                radius: 24.0,
            })],
        ),
        plate(
            "effect/runtime-effect-image-filter-rotated",
            // The program used as an image filter, on content that is turned.
            // A filter acts on what the draw produced, and what the draw
            // produced is already in the frame's space -- so the program reads
            // an upright image of a turned shape, and the tint it applies does
            // not turn with the shape. Upstream draws this one at forty-five
            // degrees for that reason.
            vec![Item::filled(
                Shape::Rect {
                    min: [-40.0, -28.0],
                    max: [40.0, 28.0],
                },
                sheet(
                    [-40.0, -28.0, 40.0, 28.0],
                    ALL,
                    TileMode::Clamp,
                    Sampling::Linear,
                ),
            )
            .with_image_filter(ImageFilter::Runtime {
                program: 2,
                uniforms: crate::fixture::tint_uniforms([0.2, 0.9, 0.5, 1.0]),
            })
            .with_transform(Transform {
                rotate: std::f32::consts::FRAC_PI_4,
                translate: [64.0, 64.0],
                ..Transform::default()
            })
            .with_blend(BlendMode::SrcOver)],
        ),
        plate(
            "effect/runtime-effect-can-precompile",
            vec![Item::filled(
                // A shape rather than the frame, so what is outside it shows
                // that the program filled geometry rather than everything.
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 46.0,
                },
                effect(-0.25),
            )],
        ),
        plate(
            "effect/runtime-effect-through-a-color-filter",
            // A color filter is arithmetic at the end of this renderer's own
            // fragment shader, and a caller's program replaces that shader --
            // so this one has to act on the image the program drew, through a
            // layer. Worth a plate because the route is entirely different from
            // every other filtered draw, and because it was silently doing
            // nothing until recently.
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                effect(0.0),
            )
            .with_color_filter(ColorFilter::matrix([
                0.2126, 0.7152, 0.0722, 0.0, 0.0, //
                0.2126, 0.7152, 0.0722, 0.0, 0.0, //
                0.2126, 0.7152, 0.0722, 0.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, 0.0,
            ]))],
        ),
        plate(
            "effect/runtime-effect-sampling-two-textures",
            // The case that needed the descriptor set layout to grow. The
            // program differences its two textures, which is the operation that
            // cannot be mistaken for either alone: where they agree it is black
            // whatever they hold, so a backend binding one texture twice -- or
            // leaving the second binding at its placeholder -- does not make
            // this picture.
            //
            // The sheet is slot zero and the glyph atlas slot one, which are
            // the two textures a scene has. They differ everywhere, the sheet
            // being color and the atlas coverage.
            vec![Item::filled(
                Shape::Rect {
                    min: [8.0, 8.0],
                    max: [120.0, 120.0],
                },
                Fill::RuntimeEffect {
                    program: 1,
                    uniforms: {
                        let mut out = vec![0.0; emblema_hal::RUNTIME_FLOATS];
                        out[0..4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
                        out
                    },
                    images: vec![0, 1],
                },
            )],
        ),
        plate(
            "effect/runtime-effect-with-transform",
            vec![Item::filled(
                Shape::Rect {
                    min: [24.0, 40.0],
                    max: [104.0, 88.0],
                },
                effect(0.0),
            )
            // The shape turns; the split does not, because it is computed from
            // the fragment's place in clip space and a caller's program owes
            // nothing to a transform it was never given.
            .with_transform(Transform {
                rotate: 0.4,
                ..Transform::default()
            })],
        ),
        // A program as an image filter rather than as a fill. Its input is the
        // layer the draw landed in, so what these show is the draw multiplied
        // by the program's tint -- and the fill under it is a gradient on
        // purpose. A flat fill would be tinted to the same picture by a program
        // that read the placeholder texture instead of the layer, which is what
        // a binding gets when it names nothing; a gradient survives the tint
        // and would not survive the substitution.
        plate(
            "effect/can-render-runtime-effect-filter",
            vec![Item::filled(
                Shape::Rect {
                    min: [16.0, 16.0],
                    max: [112.0, 112.0],
                },
                Fill::LinearGradient {
                    start: [16.0, 0.0],
                    end: [112.0, 0.0],
                    stops: vec![
                        Stop::new(WHITE, 0.0),
                        Stop::new([0.15, 0.15, 0.15, 1.0], 1.0),
                    ],
                    tile: TileMode::Clamp,
                },
            )
            .with_image_filter(ImageFilter::Runtime {
                program: 2,
                uniforms: crate::fixture::tint_uniforms([1.0, 0.45, 0.1, 1.0]),
            })],
        ),
        // The same program composed with a blur, both ways round. Upstream
        // keeps a pair for this and the pair is the point: composing is the one
        // thing a program used as a filter can do that a program used as a
        // paint cannot, and the two orders are different pictures -- tinting a
        // blurred edge is not blurring a tinted one.
        plate(
            "effect/compose-paint-runtime-outer",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 34.0,
                },
                Fill::Solid(WHITE),
            )
            .with_blend(BlendMode::SrcOver)
            .with_image_filter(ImageFilter::compose(
                ImageFilter::Runtime {
                    program: 2,
                    uniforms: crate::fixture::tint_uniforms([0.2, 0.9, 0.5, 1.0]),
                },
                ImageFilter::blur(7.0),
            ))],
        ),
        plate(
            "effect/compose-paint-runtime-inner",
            vec![Item::filled(
                Shape::Circle {
                    center: [64.0, 64.0],
                    radius: 34.0,
                },
                Fill::Solid(WHITE),
            )
            .with_blend(BlendMode::SrcOver)
            .with_image_filter(ImageFilter::compose(
                ImageFilter::blur(7.0),
                ImageFilter::Runtime {
                    program: 2,
                    uniforms: crate::fixture::tint_uniforms([0.2, 0.9, 0.5, 1.0]),
                },
            ))],
        ),
    ]
}

/// `aiks_dl_blur_unittests.cc`'s four scenes that name a backdrop id.
///
/// A `backdropId` says that the layers naming it filter one captured image
/// rather than each capturing afresh, so it is a different picture wherever
/// they reach the same pixels -- without it the second panel filters the
/// first's result. Upstream's panels are a hundred pixels apart at a sigma of
/// thirty, which reach each other; a plate is a hundred and twenty-eight
/// pixels across, so these overlap outright and the difference is at the
/// panels' own centers rather than only in their halos.
///
/// The group replaces rather than composites, as upstream's save paint does
/// and for the reason the other backdrop plates give: the group is empty, and
/// a composited empty group is nothing at all.
fn backdrop_ids() -> Vec<Scene> {
    // Three panels rather than upstream's six. The scenes differ once there
    // are two, and each further one costs a full-frame blur on every backend
    // the catalog is compared across.
    const PANELS: [[f32; 4]; 3] = [
        [8.0, 40.0, 64.0, 88.0],
        [36.0, 40.0, 92.0, 88.0],
        [64.0, 40.0, 120.0, 88.0],
    ];

    let panel = |sigma: f32| LayerSpec {
        backdrop: ImageFilter::blur(sigma),
        blend: BlendMode::Src,
        backdrop_id: Some(1),
        ..LayerSpec::default()
    };
    let plate = |name: &'static str, nodes: Vec<Node>| {
        Scene::tree(name, [behind(), nodes].concat())
            .with_background(DARK)
            // Single-sampled, for the reason every backdrop plate is: a
            // backdrop cuts the pass to read what it was writing, and a
            // multisampled pass cannot be resumed.
            .with_samples(1)
    };
    let over = |bounds: [f32; 4], layer: LayerSpec, children: Vec<Node>| Node::Layer {
        layer: Box::new(layer),
        bounds: Some(bounds),
        transform: Transform::default(),
        children,
    };

    vec![
        // One panel, where the id has nothing to share with. Upstream tests it
        // separately and it is worth keeping: naming an id must not change the
        // picture when only one layer names it, and a capture recorded but
        // never reused is the path most likely to go wrong quietly.
        plate(
            "blur/backdrop-blur-with-single-backdrop-id",
            vec![over(PANELS[1], panel(6.0), Vec::new())],
        ),
        // Three overlapping panels at one sigma. This is the scene the shared
        // filtered pass is for: one capture, one blur, three placements.
        plate(
            "blur/multiple-backdrop-blur-with-single-backdrop-id",
            PANELS
                .iter()
                .map(|bounds| over(*bounds, panel(6.0), Vec::new()))
                .collect(),
        ),
        // The same, at three sigmas. Upstream gives up its shared filtered
        // snapshot entirely here, since it keeps one snapshot per id and only
        // when every filter on the id is equal; the capture is still shared
        // both there and here, and here each distinct sigma is computed once.
        plate(
            "blur/multiple-backdrop-blur-with-single-backdrop-id-and-distinct-filters",
            PANELS
                .iter()
                .enumerate()
                .map(|(i, bounds)| over(*bounds, panel(4.0 + 4.0 * i as f32), Vec::new()))
                .collect(),
        ),
        // And each panel but the first inside a group of its own, which is the
        // case that says an id keys a captured image rather than a surface: the
        // inner layer's own target holds nothing, and what it filters is what
        // the id captured before any of them drew.
        plate(
            "blur/multiple-backdrop-blur-with-single-backdrop-id-different-layers",
            PANELS
                .iter()
                .enumerate()
                .map(|(i, bounds)| {
                    let inner = over(*bounds, panel(6.0), Vec::new());
                    if i == 0 {
                        inner
                    } else {
                        Node::Layer {
                            layer: Box::new(LayerSpec::default()),
                            bounds: None,
                            transform: Transform::default(),
                            children: vec![inner],
                        }
                    }
                })
                .collect(),
        ),
    ]
}

/// The backdrop scenes, which a blur-only backdrop could not describe.
///
/// Each draws a gradient, then an empty group whose *backdrop* is filtered and
/// which replaces rather than composites -- with `SrcOver` an empty group is
/// transparent and the plate would show the gradient untouched while appearing
/// to pass. Then a mark on top, so the plate says the group closed and left the
/// canvas where it found it.
fn backdrops() -> Vec<Scene> {
    // A gradient with hard-edged bars over it, and the bars are the point. A
    // blur does almost nothing to a smooth ramp, so a ground of gradient alone
    // made the two sigmas four levels apart and the inner half of the
    // composition nearly untested. An edge is what a blur has something to do
    // with.
    let ground = || {
        let mut items = vec![Node::Draw(Box::new(Item::filled(
            Shape::Rect {
                min: [0.0, 0.0],
                max: [128.0, 128.0],
            },
            Fill::LinearGradient {
                start: [0.0, 0.0],
                end: [128.0, 128.0],
                stops: vec![Stop::new(WHITE, 0.0), Stop::new([0.1, 0.2, 0.8, 1.0], 1.0)],
                tile: TileMode::Clamp,
            },
        )))];
        for i in 0..4 {
            let x = 12.0 + i as f32 * 30.0;
            items.push(Node::Draw(Box::new(Item::fill(
                Shape::Rect {
                    min: [x, 8.0],
                    max: [x + 12.0, 120.0],
                },
                [0.05, 0.05, 0.08, 1.0],
            ))));
        }
        items
    };
    let mark = || {
        Node::Draw(Box::new(
            Item::stroke(
                Shape::Polyline(vec![[16.0, 16.0], [112.0, 16.0]]),
                StrokeSpec::new(4.0),
                GREEN,
            )
            .with_blend(BlendMode::SrcOver),
        ))
    };
    let tint = || ImageFilter::Runtime {
        program: 2,
        uniforms: crate::fixture::tint_uniforms([1.0, 0.5, 0.2, 1.0]),
    };
    let plate = |name: &'static str, backdrop: ImageFilter, bounds: Option<[f32; 4]>| {
        Scene::tree(
            name,
            [
                ground(),
                vec![Node::Layer {
                    layer: Box::new(LayerSpec {
                        backdrop,
                        // Replaces, as upstream's save paint does: the group is
                        // empty, and a composited empty group is nothing.
                        blend: BlendMode::Src,
                        ..LayerSpec::default()
                    }),
                    bounds,
                    transform: Transform::default(),
                    children: Vec::new(),
                }],
                vec![mark()],
            ]
            .concat(),
        )
        .with_background(DARK)
        // Single-sampled, and not by preference: the same reason the backdrop
        // plates in the blur chapter are. A backdrop filter cuts the pass to
        // read what it was writing, and a multisampled pass cannot be resumed
        // -- restoring one would want a resolved-to-multisample copy, which
        // this technique has no reverse of.
        .with_samples(1)
    };
    vec![
        plate(
            "effect/compose-backdrop-runtime-outer-blur-inner",
            ImageFilter::compose(tint(), ImageFilter::blur(8.0)),
            None,
        ),
        plate(
            "effect/compose-backdrop-runtime-outer-blur-inner-small-sigma",
            // The same pair at a sigma small enough that the blur is barely
            // there, which is upstream's second scene and is not redundant: a
            // composition that dropped its inner half would draw this one
            // correctly and the one above wrongly.
            ImageFilter::compose(tint(), ImageFilter::blur(2.0)),
            None,
        ),
        plate(
            "effect/clipped-compose-backdrop-runtime-outer-blur-inner-small-sigma",
            ImageFilter::compose(tint(), ImageFilter::blur(2.0)),
            // Bounded, which for a backdrop is the filtered region rather than
            // an optimization -- so this is a panel and the two above are the
            // whole frame.
            Some([24.0, 40.0, 104.0, 96.0]),
        ),
        plate(
            "effect/clipped-backdrop-filter-with-shader",
            tint(),
            Some([24.0, 40.0, 104.0, 96.0]),
        ),
    ]
}

/// A matrix that magnifies about a point and puts the result somewhere.
///
/// A filter's matrix acts on the finished image in device pixels, so a bare
/// scale magnifies about the frame's own corner and throws the result off the
/// plate. Composing the move in is what makes the picture, and it is the part
/// a caller reading `from_scale` would not think to do.
fn magnify(about: Vec2, factor: f32, to: Vec2) -> Affine2 {
    Affine2::from_translation(to)
        * Affine2::from_scale(Vec2::splat(factor))
        * Affine2::from_translation(-about)
}

/// The matrix image filter scenes, from the basic and base files.
///
/// What they show is the distinction between filtering a finished image and
/// drawing under a transform, which is the only reason both exist. A plate
/// cannot state that on its own -- both put the shape in the same place -- so
/// each of these is drawn beside the other, and the difference is in how the
/// edges and the interior arrived rather than in where they are.
fn image_filters() -> Vec<Scene> {
    let card = Shape::RoundedRect {
        min: [8.0, 8.0],
        max: [40.0, 40.0],
        radius: 6.0,
    };
    vec![
        plate(
            "dl/matrix-image-filter-magnify",
            vec![
                // The same card twice: once resampled to twice its size, once
                // drawn at twice its size, side by side so a reader comparing
                // the two edges compares them in one picture.
                //
                // The filter magnifies about the device origin, so putting the
                // result somewhere means composing the move into the matrix --
                // not into the item's transform, which would move the source
                // and then magnify the move as well.
                Item::filled(card.clone(), ramp()).with_image_filter(ImageFilter::Matrix {
                    transform: magnify(Vec2::new(24.0, 24.0), 2.0, Vec2::new(36.0, 64.0)).into(),
                }),
                Item::filled(card.clone(), ramp())
                    .with_blend(BlendMode::SrcOver)
                    .with_transform(Transform {
                        scale: [2.0, 2.0],
                        translate: [44.0, 16.0],
                        ..Transform::default()
                    }),
            ],
        ),
        plate(
            "basic/massive-scaling-matrix-image-filter",
            vec![Item::fill(
                Shape::Rect {
                    min: [60.0, 60.0],
                    max: [68.0, 68.0],
                },
                YELLOW,
            )
            // Twelve times, which magnifies eight pixels to almost the plate.
            // A resample this large is where the layer's own resolution stops
            // being an implementation detail and becomes the picture.
            .with_image_filter(ImageFilter::Matrix {
                transform: magnify(Vec2::splat(64.0), 12.0, Vec2::splat(64.0)).into(),
            })],
        ),
        Scene::tree(
            "dl/matrix-save-layer-filter",
            vec![Node::Layer {
                layer: Box::new(LayerSpec {
                    // The group is resampled on the way back rather than its
                    // contents drawn larger, which is what a matrix filter on
                    // a save layer means.
                    matrix: Some(Transform {
                        scale: [2.0, 2.0],
                        translate: [-32.0, -32.0],
                        ..Transform::default()
                    }),
                    ..LayerSpec::default()
                }),
                bounds: Some([32.0, 32.0, 96.0, 96.0]),
                transform: Transform::default(),
                children: vec![
                    Node::Draw(Box::new(Item::fill(
                        Shape::Circle {
                            center: [56.0, 56.0],
                            radius: 18.0,
                        },
                        RED,
                    ))),
                    Node::Draw(Box::new(
                        Item::fill(
                            Shape::Circle {
                                center: [76.0, 76.0],
                                radius: 18.0,
                            },
                            BLUE,
                        )
                        .with_blend(BlendMode::SrcOver),
                    )),
                ],
            }],
        )
        .with_background(DARK)
        .with_samples(4),
    ]
}

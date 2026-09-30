//! Shapes as data.
//!
//! Scenes describe geometry declaratively rather than by calling a builder, so
//! one corpus can drive golden comparison, cross-backend conformance,
//! performance runs, and on-device runs without being rewritten for each. Every
//! type here is plain data and can be serialized when the corpus moves out of
//! Rust source.

use emblema_geometry::{FillRule, Path, PathBuilder};
use glam::Vec2;

/// The magic constant for approximating a quarter circle with a cubic.
const KAPPA: f32 = 0.552_284_8;

/// A shape, in user coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect {
        min: [f32; 2],
        max: [f32; 2],
    },
    /// A closed polygon through the given points.
    Polygon(Vec<[f32; 2]>),
    /// A single segment, drawn through `Canvas::draw_line`.
    ///
    /// Distinct from a two-point [`Self::Polyline`] in the same way a
    /// rectangle is distinct from its path: the API offers a call for it, and
    /// a scene naming this one gets that call. Upstream keeps a whole family
    /// of scenes drawing the same thin lines four ways -- as a line, as a
    /// stroked path, as a filled rectangle and as a filled rounded one --
    /// because a renderer may specialize any of them, and a specialization
    /// that disagrees with the general route is a bug nobody sees until the
    /// two are put side by side.
    Line {
        from: [f32; 2],
        to: [f32; 2],
    },
    /// An open run of line segments.
    ///
    /// Distinct from [`Self::Polygon`] in the one way that matters to a
    /// stroke: an open path has two ends, so it has caps. A closed one has a
    /// join everywhere instead, which is why a corpus of closed shapes could
    /// not exercise a cap however many strokes it contained.
    Polyline(Vec<[f32; 2]>),
    /// Several open runs of line segments in one path.
    ///
    /// A path may hold more than one contour, and a stroke treats each
    /// separately: the ends of one are caps rather than a join onto the next,
    /// however close together they are. Upstream keeps two scenes on this,
    /// and both are cases where the difference is easy to lose -- two contours
    /// meeting at a point, where a renderer that joined them would draw a
    /// mitered corner instead of two round caps, and a contour holding a
    /// single point, which has no direction and is drawn only because a round
    /// cap has a shape without one.
    ///
    /// Open rather than closed, and separate from [`Self::Polyline`] rather
    /// than a field on it, because every use of it so far is upstream's and
    /// upstream's are all open.
    Contours(Vec<Vec<[f32; 2]>>),
    /// Several closed polygons in one path.
    ///
    /// [`Self::Polygon`] is to this what [`Self::Polyline`] is to
    /// [`Self::Contours`]: the same geometry, one contour or several. What it
    /// adds over drawing each polygon separately is the fill rule, which acts
    /// on the path as a whole -- two overlapping contours wound the same way
    /// are one region under `NonZero` and a ring under `EvenOdd`, and neither
    /// is what two draws produce.
    ///
    /// Written when a use appeared, which is what the note on `Contours` said
    /// should happen: upstream's shadow file has a caster made of two closed
    /// triangles, and a shadow is cast by what a path covers rather than by
    /// each contour in turn.
    Polygons(Vec<Vec<[f32; 2]>>),
    /// A closed polygon filled by a stated rule.
    ///
    /// Separate from [`Self::Polygon`] rather than a field on it, because the
    /// rule only ever differs from the default for a path that crosses itself
    /// -- every other polygon fills the same either way -- and putting it on
    /// the common variant would make each of the corpus's many polygons state
    /// something that does not matter to it.
    RuledPolygon {
        points: Vec<[f32; 2]>,
        rule: FillRule,
    },
    Circle {
        center: [f32; 2],
        radius: f32,
    },
    /// An ellipse inscribed in a rectangle.
    ///
    /// Not expressible as any of the others: a rounded rectangle asked for a
    /// large radius becomes a stadium rather than an ellipse, and a circle
    /// scaled by a transform is one only if the whole item is scaled with it.
    Oval {
        min: [f32; 2],
        max: [f32; 2],
    },
    /// A rectangle with rounded corners, which is most of an interface.
    RoundedRect {
        min: [f32; 2],
        max: [f32; 2],
        /// Clamped to half the shorter side, so a large one gives a stadium
        /// rather than an outline that crosses itself.
        radius: f32,
    },
    /// The eight numbers `dart:ui`'s `RRect` carries: an x and a y radius for
    /// each corner, in upstream's order -- top left, top right, bottom left,
    /// bottom right.
    ///
    /// A variant of its own rather than a widening of the one above, because
    /// nearly every plate wants one circular radius and a reader of a scene
    /// should not have to write `[[r, r]; 4]` to say so.
    RoundedRectWithRadii {
        min: [f32; 2],
        max: [f32; 2],
        /// Fitted by `dart:ui`'s rule where two radii overrun the side they
        /// share, which is the renderer's own arithmetic rather than a copy.
        radii: [[f32; 2]; 4],
    },
    /// Flutter's rounded superellipse -- `dart:ui`'s `drawRSuperellipse`.
    ///
    /// Here as its own variant rather than as a flag on `RoundedRect` because
    /// the two are different curves that happen to take the same numbers, and
    /// a scene comparing them has to be able to ask for each by name.
    RoundSuperellipse {
        min: [f32; 2],
        max: [f32; 2],
        /// Four corners, each an x and a y radius, in upstream's order:
        /// top-left, top-right, bottom-left, bottom-right.
        ///
        /// Four rather than one because this shape's corners are routinely
        /// unequal in the pictures worth drawing -- a squircle with one radius
        /// is the easy case, and the code that splits a side between two
        /// different corners is only reached when they differ.
        radii: [[f32; 2]; 4],
    },
    /// The ring between two rounded rectangles -- `dart:ui`'s `drawDRRect`.
    ///
    /// Two contours in one path under the even-odd rule, which is the whole of
    /// what makes the inner one a hole rather than a second ring drawn over
    /// the first. Not expressible as two items: drawing the inner one in the
    /// background color would match only over a background of that color, and
    /// would still be wrong under any blend or through any layer.
    DiffRoundedRect {
        /// Minimum and maximum corners of the outer rectangle.
        outer: [[f32; 2]; 2],
        outer_radius: f32,
        /// Minimum and maximum corners of the inner rectangle.
        inner: [[f32; 2]; 2],
        inner_radius: f32,
    },
    /// The same ring, with each rectangle's four corners stated on their own.
    DiffRoundedRectWithRadii {
        outer: [[f32; 2]; 2],
        outer_radii: [[f32; 2]; 4],
        inner: [[f32; 2]; 2],
        inner_radii: [[f32; 2]; 4],
    },
    /// An arc, open for a ring or closed through the center for a slice.
    ///
    /// The two shapes an arc is actually used for, and they differ in the one
    /// way that matters here: the ring is stroked along a curve with two caps,
    /// the slice is a filled region with two straight edges meeting at a point.
    Arc {
        center: [f32; 2],
        radii: [f32; 2],
        start: f32,
        sweep: f32,
        /// Join the ends through the center, making a slice rather than a ring.
        through_center: bool,
    },
    /// An open conic -- a rational quadratic -- which is the curve a circular
    /// arc actually is.
    ///
    /// Distinct from [`Self::Cubic`] in what it can be exactly rather than
    /// approximately: at a weight of `sqrt(2)/2`, with the control point at
    /// the corner, this is a quarter circle and no polynomial curve is.
    Conic {
        start: [f32; 2],
        ctrl: [f32; 2],
        end: [f32; 2],
        weight: f32,
    },
    /// An open cubic, for curve and stroke coverage.
    Cubic {
        start: [f32; 2],
        c0: [f32; 2],
        c1: [f32; 2],
        end: [f32; 2],
    },
}

impl Shape {
    pub fn to_path(&self) -> Path {
        let mut b = PathBuilder::new();
        match self {
            Self::Rect { min, max } => {
                b.move_to(Vec2::from(*min))
                    .line_to(Vec2::new(max[0], min[1]))
                    .line_to(Vec2::from(*max))
                    .line_to(Vec2::new(min[0], max[1]))
                    .close();
            }
            Self::Arc {
                center,
                radii,
                start,
                sweep,
                through_center,
            } => {
                if *through_center {
                    // The center first, so the arc joins to it with a line and
                    // the close brings the far end back: a slice.
                    b.move_to(Vec2::from(*center));
                }
                b.arc(Vec2::from(*center), Vec2::from(*radii), *start, *sweep);
                if *through_center {
                    b.close();
                }
            }
            Self::Polygon(points) => {
                trace(&mut b, points);
                b.close();
            }
            Self::Line { from, to } => {
                b.move_to(Vec2::from(*from)).line_to(Vec2::from(*to));
            }
            Self::Contours(contours) => {
                for points in contours {
                    let mut points = points.iter();
                    if let Some(first) = points.next() {
                        b.move_to(Vec2::from(*first));
                        for p in points {
                            b.line_to(Vec2::from(*p));
                        }
                    }
                }
            }
            Self::Polygons(contours) => {
                for points in contours {
                    trace(&mut b, points);
                    b.close();
                }
            }
            Self::Polyline(points) => {
                trace(&mut b, points);
            }
            Self::RuledPolygon { points, rule } => {
                b = b.with_fill_rule(*rule);
                trace(&mut b, points);
                b.close();
            }
            Self::Oval { min, max } => {
                let (cx, cy) = ((min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0);
                let (rx, ry) = ((max[0] - min[0]) / 2.0, (max[1] - min[1]) / 2.0);
                let (kx, ky) = (KAPPA * rx, KAPPA * ry);
                b.move_to(Vec2::new(cx + rx, cy))
                    .cubic_to(
                        Vec2::new(cx + rx, cy + ky),
                        Vec2::new(cx + kx, cy + ry),
                        Vec2::new(cx, cy + ry),
                    )
                    .cubic_to(
                        Vec2::new(cx - kx, cy + ry),
                        Vec2::new(cx - rx, cy + ky),
                        Vec2::new(cx - rx, cy),
                    )
                    .cubic_to(
                        Vec2::new(cx - rx, cy - ky),
                        Vec2::new(cx - kx, cy - ry),
                        Vec2::new(cx, cy - ry),
                    )
                    .cubic_to(
                        Vec2::new(cx + kx, cy - ry),
                        Vec2::new(cx + rx, cy - ky),
                        Vec2::new(cx + rx, cy),
                    )
                    .close();
            }
            Self::RoundedRect { min, max, radius } => {
                rounded_contour(&mut b, *min, *max, *radius);
            }
            Self::RoundedRectWithRadii { min, max, radii } => {
                radii_contour(&mut b, *min, *max, *radii);
            }
            Self::RoundSuperellipse { min, max, radii } => {
                use emblema_geometry::superellipse::{CornerRadii, RoundSuperellipse};
                RoundSuperellipse::new(
                    emblema_geometry::path::Rect::new(
                        Vec2::new(min[0], min[1]),
                        Vec2::new(max[0], max[1]),
                    ),
                    CornerRadii {
                        top_left: Vec2::new(radii[0][0], radii[0][1]),
                        top_right: Vec2::new(radii[1][0], radii[1][1]),
                        bottom_left: Vec2::new(radii[2][0], radii[2][1]),
                        bottom_right: Vec2::new(radii[3][0], radii[3][1]),
                    },
                )
                .add_to(&mut b);
            }
            Self::DiffRoundedRect {
                outer,
                outer_radius,
                inner,
                inner_radius,
            } => {
                b = b.with_fill_rule(FillRule::EvenOdd);
                rounded_contour(&mut b, outer[0], outer[1], *outer_radius);
                rounded_contour(&mut b, inner[0], inner[1], *inner_radius);
            }
            Self::DiffRoundedRectWithRadii {
                outer,
                outer_radii,
                inner,
                inner_radii,
            } => {
                b = b.with_fill_rule(FillRule::EvenOdd);
                radii_contour(&mut b, outer[0], outer[1], *outer_radii);
                radii_contour(&mut b, inner[0], inner[1], *inner_radii);
            }
            Self::Circle { center, radius } => {
                let (cx, cy) = (center[0], center[1]);
                let r = *radius;
                let k = KAPPA * r;
                b.move_to(Vec2::new(cx + r, cy))
                    .cubic_to(
                        Vec2::new(cx + r, cy + k),
                        Vec2::new(cx + k, cy + r),
                        Vec2::new(cx, cy + r),
                    )
                    .cubic_to(
                        Vec2::new(cx - k, cy + r),
                        Vec2::new(cx - r, cy + k),
                        Vec2::new(cx - r, cy),
                    )
                    .cubic_to(
                        Vec2::new(cx - r, cy - k),
                        Vec2::new(cx - k, cy - r),
                        Vec2::new(cx, cy - r),
                    )
                    .cubic_to(
                        Vec2::new(cx + k, cy - r),
                        Vec2::new(cx + r, cy - k),
                        Vec2::new(cx + r, cy),
                    )
                    .close();
            }
            Self::Conic {
                start,
                ctrl,
                end,
                weight,
            } => {
                b.move_to(Vec2::from(*start)).conic_to(
                    Vec2::from(*ctrl),
                    Vec2::from(*end),
                    *weight,
                );
            }
            Self::Cubic { start, c0, c1, end } => {
                b.move_to(Vec2::from(*start)).cubic_to(
                    Vec2::from(*c0),
                    Vec2::from(*c1),
                    Vec2::from(*end),
                );
            }
        }
        b.build()
    }
}

/// Run a line through the given points, leaving the path open.
fn trace(b: &mut PathBuilder, points: &[[f32; 2]]) {
    if let Some((first, rest)) = points.split_first() {
        b.move_to(Vec2::from(*first));
        for p in rest {
            b.line_to(Vec2::from(*p));
        }
    }
}

/// Trace a rounded rectangle whose corners differ, as one closed contour.
///
/// Delegated to the renderer's own construction rather than traced here, for
/// the reason the executor sends a rounded rectangle through `draw_rrect`
/// rather than through its path: the scaling `dart:ui` applies when two radii
/// overrun the side between them is part of what the shape *is*, and a second
/// copy of that rule here could disagree with the first while still looking
/// like a rounded rectangle.
fn radii_contour(b: &mut PathBuilder, min: [f32; 2], max: [f32; 2], radii: [[f32; 2]; 4]) {
    emblema_core::Rect::new(min[0], min[1], max[0], max[1]).add_rounded_outline(b, radii);
}

/// Trace a rounded rectangle as one closed contour.
///
/// Shared by [`Shape::RoundedRect`] and [`Shape::DiffRoundedRect`] rather than
/// written twice, since a ring whose two contours were built by different code
/// could be a ring of uneven thickness and still look plausible.
fn rounded_contour(b: &mut PathBuilder, min: [f32; 2], max: [f32; 2], radius: f32) {
    let (l, t) = (min[0], min[1]);
    let (r, bo) = (max[0], max[1]);
    if r <= l || bo <= t {
        return;
    }
    let radius = radius.min((r - l) / 2.0).min((bo - t) / 2.0).max(0.0);
    let k = KAPPA * radius;
    b.move_to(Vec2::new(l + radius, t))
        .line_to(Vec2::new(r - radius, t))
        .cubic_to(
            Vec2::new(r - radius + k, t),
            Vec2::new(r, t + radius - k),
            Vec2::new(r, t + radius),
        )
        .line_to(Vec2::new(r, bo - radius))
        .cubic_to(
            Vec2::new(r, bo - radius + k),
            Vec2::new(r - radius + k, bo),
            Vec2::new(r - radius, bo),
        )
        .line_to(Vec2::new(l + radius, bo))
        .cubic_to(
            Vec2::new(l + radius - k, bo),
            Vec2::new(l, bo - radius + k),
            Vec2::new(l, bo - radius),
        )
        .line_to(Vec2::new(l, t + radius))
        .cubic_to(
            Vec2::new(l, t + radius - k),
            Vec2::new(l + radius - k, t),
            Vec2::new(l + radius, t),
        )
        .close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_encloses_its_corners() {
        let shape = Shape::Rect {
            min: [10.0, 20.0],
            max: [30.0, 50.0],
        };
        let bounds = shape.to_path().bounds();
        assert_eq!(bounds.min, Vec2::new(10.0, 20.0));
        assert_eq!(bounds.max, Vec2::new(30.0, 50.0));
    }

    #[test]
    fn a_circle_is_bounded_by_its_radius() {
        let shape = Shape::Circle {
            center: [50.0, 50.0],
            radius: 20.0,
        };
        let bounds = shape.to_path().bounds();
        // Control points of a cubic circle approximation lie within the
        // bounding box of the circle itself on the axes.
        assert!((bounds.min.x - 30.0).abs() < 0.01);
        assert!((bounds.max.x - 70.0).abs() < 0.01);
    }

    #[test]
    fn an_empty_polygon_produces_an_empty_path() {
        // Scenes come from data and may be degenerate; converting must not
        // panic on the way in.
        assert!(Shape::Polygon(Vec::new()).to_path().is_empty());
    }
}

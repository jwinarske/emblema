//! Turning paths into triangles.
//!
//! Two strategies, selected by geometry rather than by caller preference:
//!
//! - **Fan fill** for convex paths. A convex polygon triangulates by fanning
//!   from its first vertex, which needs no sweep, no intermediate structures,
//!   and no stencil pass. Most UI geometry — buttons, cards, indicators — is
//!   convex, so this is the common case rather than an optimization for a rare
//!   one.
//! - **General tessellation** through lyon for everything else.
//!
//! Choosing wrongly is not symmetric. Fanning a concave polygon emits
//! triangles that cover area outside the path, which renders visibly wrong;
//! sending a convex path through the general tessellator merely costs time.
//! Convexity detection is conservative for exactly this reason.

use crate::flatten::{flatten, DEFAULT_TOLERANCE};
use crate::path::{polygon_convexity, Convexity, FillRule, Path, Verb};
use crate::stroke::{LineCap, LineJoin, StrokeStyle};
use glam::Vec2;

/// Triangles, as a vertex buffer plus an index buffer.
///
/// Indexed rather than expanded so shared vertices are transformed once, which
/// matters for the fan case where every triangle shares vertex zero.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VertexBuffers {
    pub vertices: Vec<Vec2>,
    pub indices: Vec<u32>,
}

impl VertexBuffers {
    /// Drop the contents but keep the allocations.
    ///
    /// The frame loop tessellates repeatedly; reusing capacity is what keeps
    /// per-frame allocation out of the steady state.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Whether every index addresses a real vertex and the buffer is whole
    /// triangles. Used by tests and debug assertions.
    pub fn is_well_formed(&self) -> bool {
        self.indices.len() % 3 == 0
            && self
                .indices
                .iter()
                .all(|i| (*i as usize) < self.vertices.len())
    }
}

/// A tolerance lyon will accept, from whatever the caller had.
///
/// Two things are wrong with passing one straight through. A tolerance that is
/// not a positive length is not a tolerance -- `0.0` trips an assertion inside
/// lyon's flattener and takes the process down with it, which is the failure
/// `Path::is_finite` exists to prevent by a different road. And an
/// arbitrarily small one is a subdivision count with nothing at the top of it.
///
/// So a value that is not finite and positive falls back to the default, on
/// the same reading this crate gives everywhere else: a number that is not a
/// length describes no length. The floor is eight orders above the assertion
/// lyon makes and four below the tightest tolerance this renderer produces --
/// device tolerance divided by the transform's scale, so a quarter pixel under
/// a two-hundred-and-fifty-times zoom is still two hundred and fifty times
/// above it.
fn usable_tolerance(tolerance: f32) -> f32 {
    const FLOOR: f32 = 1e-6;
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return DEFAULT_TOLERANCE;
    }
    tolerance.max(FLOOR)
}

/// Reusable tessellation scratch space.
///
/// Holds the output buffers and lyon's internal state across calls so a steady
/// frame loop stops allocating once buffers reach their working size.
#[derive(Default)]
pub struct Tessellator {
    buffers: VertexBuffers,
    fill: lyon_tessellation::FillTessellator,
    stroke: lyon_tessellation::StrokeTessellator,
}

impl Tessellator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tessellate a filled path, returning triangles in path space.
    ///
    /// `tolerance` is the flattening tolerance in device pixels, already
    /// scaled for the transform the result will be drawn under.
    pub fn fill(&mut self, path: &Path, tolerance: f32) -> &VertexBuffers {
        // Checked here rather than at each call site, because every route into
        // the tessellator -- a path a caller built, a rounded rectangle, an
        // oval, the outline of a line -- ends up on this line, and lyon asserts
        // on a non-finite coordinate rather than declining it. One guard at the
        // boundary covers all of them; a guard per shape covers the ones
        // somebody remembered.
        if !path.is_finite() || !path.is_within_tessellation_range() {
            self.buffers.clear();
            return &self.buffers;
        }
        let tolerance = usable_tolerance(tolerance);
        self.buffers.clear();

        let polylines = flatten(path, tolerance);
        if polylines.is_empty() {
            return &self.buffers;
        }

        // A single convex subpath is the common UI case and needs no sweep.
        if polylines.len() == 1
            && polygon_convexity(strip_closing_duplicate(&polylines[0])) == Convexity::Convex
        {
            fan_fill(strip_closing_duplicate(&polylines[0]), &mut self.buffers);
            return &self.buffers;
        }

        general_fill(
            &polylines,
            path.fill_rule(),
            &mut self.fill,
            &mut self.buffers,
        );
        &self.buffers
    }

    /// Tessellate a stroked path, returning triangles in path space.
    ///
    /// Curves are handed to the tessellator intact rather than pre-flattened.
    /// Offsetting a polyline and offsetting the curve it approximates are not
    /// the same operation: the polyline's corners become joins that the curve
    /// does not have, so pre-flattening would stipple a smooth curve with
    /// spurious miter or round joins along its length.
    pub fn stroke(&mut self, path: &Path, style: &StrokeStyle, tolerance: f32) -> &VertexBuffers {
        // As in `fill`: lyon asserts on a coordinate that is not a number, and
        // a stroke reaches it by a different road.
        //
        // The range check matters more here than it does there. A fill is
        // flattened by this crate, which caps its segment count; a stroke hands
        // its curves to lyon, which does not, so the vertex count grows with
        // the coordinate and nothing stops it -- measured, three verbs at a
        // coordinate of `1e15` stroke to thirty-one million vertices. See
        // `Path::is_within_tessellation_range`.
        if !path.is_finite() || !path.is_within_tessellation_range() {
            self.buffers.clear();
            return &self.buffers;
        }
        // And the width, which is a length like any other and is the one that
        // took the process down. A stroke's outline is offset by half its
        // width, so a width outside the range a coordinate may occupy
        // describes a band wider than the whole space the tessellator will
        // accept a point in -- there is no picture in it, and the same
        // argument that bounds a coordinate bounds this.
        //
        // Measured before the guard: `Paint::stroke(color, 1e30)` with a round
        // join, through `Canvas::draw_path`, aborted the process. See
        let tolerance = usable_tolerance(tolerance);
        use lyon_tessellation::{
            BuffersBuilder, LineCap as LyonCap, LineJoin as LyonJoin, StrokeOptions,
        };

        self.buffers.clear();
        if path.is_empty() || !style.is_visible() {
            return &self.buffers;
        }

        let lyon_path = to_lyon_path(path);
        let options = StrokeOptions::default()
            .with_line_width(style.width)
            .with_tolerance(tolerance)
            // lyon expresses the limit as half the SVG ratio: it bevels when
            // 1/sin(angle/2) exceeds twice the configured value, so passing an
            // SVG limit through unchanged would degrade at roughly half the
            // intended angle. Halving it restores the documented semantics.
            //
            // The floor is not defensive style — lyon asserts on anything below
            // MINIMUM_MITER_LIMIT, so an unclamped caller value panics inside
            // the tessellator. It costs exactness for SVG limits under 2, which
            // all behave as 2.
            .with_miter_limit((style.miter_limit * 0.5).max(StrokeOptions::MINIMUM_MITER_LIMIT))
            .with_line_cap(match style.cap {
                LineCap::Butt => LyonCap::Butt,
                LineCap::Round => LyonCap::Round,
                LineCap::Square => LyonCap::Square,
            })
            .with_line_join(match style.join {
                // Miter, not MiterClip. The two differ once the miter limit is
                // exceeded: MiterClip truncates the spike at the limit, while
                // Miter drops back to a bevel. The latter is what SVG and
                // PostScript specify, and what StrokeStyle documents.
                LineJoin::Miter => LyonJoin::Miter,
                LineJoin::Round => LyonJoin::Round,
                LineJoin::Bevel => LyonJoin::Bevel,
            });

        let mut geometry: lyon_tessellation::VertexBuffers<Vec2, u32> =
            lyon_tessellation::VertexBuffers::new();
        {
            let mut builder = BuffersBuilder::new(&mut geometry, ToVec2);
            if self
                .stroke
                .tessellate_path(&lyon_path, &options, &mut builder)
                .is_err()
            {
                return &self.buffers;
            }
        }

        self.buffers.vertices.extend_from_slice(&geometry.vertices);
        self.buffers.indices.extend_from_slice(&geometry.indices);
        &self.buffers
    }

    pub fn buffers(&self) -> &VertexBuffers {
        &self.buffers
    }
}

/// Convert to a lyon path, preserving curves rather than flattening them.
fn to_lyon_path(path: &Path) -> lyon_tessellation::path::Path {
    use lyon_tessellation::math::Point as LyonPoint;
    use lyon_tessellation::path::Path as LyonPath;

    let pt = |p: Vec2| LyonPoint::new(p.x, p.y);
    let mut builder = LyonPath::builder();
    let mut open = false;

    for (verb, points) in path.segments() {
        match verb {
            Verb::MoveTo => {
                if open {
                    builder.end(false);
                }
                builder.begin(pt(points[0]));
                open = true;
            }
            Verb::LineTo if open => {
                builder.line_to(pt(points[0]));
            }
            Verb::QuadTo if open => {
                builder.quadratic_bezier_to(pt(points[0]), pt(points[1]));
            }
            Verb::CubicTo if open => {
                builder.cubic_bezier_to(pt(points[0]), pt(points[1]), pt(points[2]));
            }
            Verb::Close if open => {
                builder.end(true);
                open = false;
            }
            // A segment verb with no open subpath cannot occur through
            // PathBuilder, which inserts an implicit move. Ignoring it keeps
            // hand-constructed paths from panicking inside lyon.
            _ => {}
        }
    }
    if open {
        builder.end(false);
    }
    builder.build()
}

/// Drop a trailing point that merely repeats the first.
///
/// Flattening closes subpaths by appending the start point, which is what the
/// tessellator wants but would make convexity analysis see a zero-length edge.
fn strip_closing_duplicate(points: &[Vec2]) -> &[Vec2] {
    if points.len() > 2 && points.first() == points.last() {
        &points[..points.len() - 1]
    } else {
        points
    }
}

/// Triangulate a convex polygon by fanning from its first vertex.
fn fan_fill(points: &[Vec2], out: &mut VertexBuffers) {
    if points.len() < 3 {
        return;
    }
    out.vertices.extend_from_slice(points);
    for i in 1..points.len() as u32 - 1 {
        out.indices.extend_from_slice(&[0, i, i + 1]);
    }
}

/// Tessellate arbitrary geometry through lyon.
fn general_fill(
    polylines: &[Vec<Vec2>],
    fill_rule: FillRule,
    tessellator: &mut lyon_tessellation::FillTessellator,
    out: &mut VertexBuffers,
) {
    use lyon_tessellation::math::Point as LyonPoint;
    use lyon_tessellation::path::Path as LyonPath;
    use lyon_tessellation::{BuffersBuilder, FillOptions, FillRule as LyonFillRule};

    let mut builder = LyonPath::builder();
    for line in polylines {
        let stripped = strip_closing_duplicate(line);
        if stripped.len() < 3 {
            continue;
        }
        builder.begin(LyonPoint::new(stripped[0].x, stripped[0].y));
        for p in &stripped[1..] {
            builder.line_to(LyonPoint::new(p.x, p.y));
        }
        builder.close();
    }
    let lyon_path = builder.build();

    let options = FillOptions::default().with_fill_rule(match fill_rule {
        FillRule::NonZero => LyonFillRule::NonZero,
        FillRule::EvenOdd => LyonFillRule::EvenOdd,
    });

    // u32 indices, not lyon's u16 default: 65536 vertices is well within reach
    // for a detailed path, and silently truncating there would corrupt
    // geometry rather than merely degrade it.
    let mut geometry: lyon_tessellation::VertexBuffers<Vec2, u32> =
        lyon_tessellation::VertexBuffers::new();
    {
        let mut builder = BuffersBuilder::new(&mut geometry, ToVec2);
        // Tessellation failure means self-intersecting or otherwise
        // pathological input. Emitting nothing is the graceful outcome: a
        // missing shape is recoverable, a panic in the frame loop is not.
        if tessellator
            .tessellate_path(&lyon_path, &options, &mut builder)
            .is_err()
        {
            return;
        }
    }

    out.vertices.extend_from_slice(&geometry.vertices);
    out.indices.extend_from_slice(&geometry.indices);
}

/// Emits lyon's vertices directly as [`Vec2`], avoiding a conversion pass over
/// the buffer after tessellation.
struct ToVec2;

impl lyon_tessellation::FillVertexConstructor<Vec2> for ToVec2 {
    fn new_vertex(&mut self, vertex: lyon_tessellation::FillVertex) -> Vec2 {
        let p = vertex.position();
        Vec2::new(p.x, p.y)
    }
}

impl lyon_tessellation::StrokeVertexConstructor<Vec2> for ToVec2 {
    fn new_vertex(&mut self, vertex: lyon_tessellation::StrokeVertex) -> Vec2 {
        let p = vertex.position();
        Vec2::new(p.x, p.y)
    }
}

/// Twice the signed area of a triangle. Positive is counter-clockwise.
fn signed_area2(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b - a).perp_dot(c - a)
}

/// Total unsigned area covered by a triangle buffer.
pub fn covered_area(buffers: &VertexBuffers) -> f32 {
    buffers
        .indices
        .chunks_exact(3)
        .map(|t| {
            let (a, b, c) = (
                buffers.vertices[t[0] as usize],
                buffers.vertices[t[1] as usize],
                buffers.vertices[t[2] as usize],
            );
            signed_area2(a, b, c).abs() * 0.5
        })
        .sum()
}

/// Unsigned area of a simple polygon, by the shoelace formula.
pub fn polygon_area(points: &[Vec2]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        acc += a.perp_dot(b);
    }
    (acc * 0.5).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flatten::DEFAULT_TOLERANCE;
    use crate::path::PathBuilder;

    fn square() -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(10.0, 0.0))
            .line_to(Vec2::new(10.0, 10.0))
            .line_to(Vec2::new(0.0, 10.0))
            .close();
        b.build()
    }

    /// An L shape: concave, so it must not take the fan path.
    fn el_shape() -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(10.0, 0.0))
            .line_to(Vec2::new(10.0, 4.0))
            .line_to(Vec2::new(4.0, 4.0))
            .line_to(Vec2::new(4.0, 10.0))
            .line_to(Vec2::new(0.0, 10.0))
            .close();
        b.build()
    }

    #[test]
    fn convex_fill_fans_from_the_first_vertex() {
        let mut t = Tessellator::new();
        let buffers = t.fill(&square(), DEFAULT_TOLERANCE);

        // A fan over n vertices is exactly n-2 triangles, all sharing vertex 0.
        assert_eq!(buffers.vertices.len(), 4);
        assert_eq!(buffers.triangle_count(), 2);
        assert!(buffers.indices.chunks_exact(3).all(|t| t[0] == 0));
        assert!(buffers.is_well_formed());
    }

    #[test]
    fn convex_fill_covers_exactly_the_polygon_area() {
        let mut t = Tessellator::new();
        let buffers = t.fill(&square(), DEFAULT_TOLERANCE);
        assert!((covered_area(buffers) - 100.0).abs() < 0.01);
    }

    #[test]
    fn concave_fill_covers_the_polygon_and_not_its_hull() {
        let path = el_shape();
        let mut t = Tessellator::new();
        let buffers = t.fill(&path, DEFAULT_TOLERANCE);

        assert!(buffers.is_well_formed());
        // The L covers 64 units; its convex hull covers 100. Fanning a concave
        // polygon would paint the notch, so this is the assertion that catches
        // a wrong strategy choice.
        let area = covered_area(buffers);
        assert!(
            (area - 64.0).abs() < 0.5,
            "expected the L's own area, got {area}"
        );
    }

    #[test]
    fn indices_always_address_real_vertices() {
        let mut t = Tessellator::new();
        for path in [square(), el_shape()] {
            let buffers = t.fill(&path, DEFAULT_TOLERANCE);
            assert!(buffers.is_well_formed());
        }
    }

    #[test]
    fn empty_and_degenerate_paths_produce_no_triangles() {
        let mut t = Tessellator::new();
        assert!(t.fill(&Path::default(), DEFAULT_TOLERANCE).is_empty());

        // A subpath with fewer than three distinct points encloses nothing.
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO).line_to(Vec2::new(1.0, 1.0)).close();
        assert!(t.fill(&b.build(), DEFAULT_TOLERANCE).is_empty());
    }

    #[test]
    fn buffers_are_reused_across_calls_without_leaking_previous_geometry() {
        let mut t = Tessellator::new();
        let first = t.fill(&el_shape(), DEFAULT_TOLERANCE).triangle_count();
        assert!(first > 0);

        let second = t.fill(&square(), DEFAULT_TOLERANCE);
        // Stale triangles from the previous path would inflate this.
        assert_eq!(second.triangle_count(), 2);
        assert_eq!(second.vertices.len(), 4);
    }

    #[test]
    fn fill_rule_changes_the_result_for_overlapping_subpaths() {
        // A square with a smaller square inside, wound the same way. Non-zero
        // fills the whole outer square; even-odd leaves the inner one hollow.
        let build = |rule: FillRule| {
            let mut b = PathBuilder::new().with_fill_rule(rule);
            b.move_to(Vec2::new(0.0, 0.0))
                .line_to(Vec2::new(10.0, 0.0))
                .line_to(Vec2::new(10.0, 10.0))
                .line_to(Vec2::new(0.0, 10.0))
                .close()
                .move_to(Vec2::new(3.0, 3.0))
                .line_to(Vec2::new(7.0, 3.0))
                .line_to(Vec2::new(7.0, 7.0))
                .line_to(Vec2::new(3.0, 7.0))
                .close();
            b.build()
        };

        let mut t = Tessellator::new();
        let nonzero = covered_area(t.fill(&build(FillRule::NonZero), DEFAULT_TOLERANCE));
        let evenodd = covered_area(t.fill(&build(FillRule::EvenOdd), DEFAULT_TOLERANCE));

        assert!((nonzero - 100.0).abs() < 0.5, "non-zero got {nonzero}");
        assert!((evenodd - 84.0).abs() < 0.5, "even-odd got {evenodd}");
    }

    #[test]
    fn polygon_area_matches_the_shoelace_result() {
        let unit = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 3.0),
            Vec2::new(0.0, 3.0),
        ];
        assert!((polygon_area(&unit) - 6.0).abs() < 1e-5);
        // Winding direction must not change the magnitude.
        let reversed: Vec<_> = unit.iter().rev().copied().collect();
        assert!((polygon_area(&reversed) - 6.0).abs() < 1e-5);
    }

    #[test]
    fn a_curved_path_tessellates_to_roughly_its_true_area() {
        // A circle approximated by four cubics, radius 10, area ~314.16.
        let r = 10.0f32;
        let k = 0.552_284_8 * r;
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(r, 0.0))
            .cubic_to(Vec2::new(r, k), Vec2::new(k, r), Vec2::new(0.0, r))
            .cubic_to(Vec2::new(-k, r), Vec2::new(-r, k), Vec2::new(-r, 0.0))
            .cubic_to(Vec2::new(-r, -k), Vec2::new(-k, -r), Vec2::new(0.0, -r))
            .cubic_to(Vec2::new(k, -r), Vec2::new(r, -k), Vec2::new(r, 0.0))
            .close();

        let mut t = Tessellator::new();
        let area = covered_area(t.fill(&b.build(), 0.05));
        let expected = std::f32::consts::PI * r * r;
        // Flattening inscribes the curve, so the result is slightly under.
        assert!(
            (area - expected).abs() / expected < 0.01,
            "got {area}, expected about {expected}"
        );
    }
}

#[cfg(test)]
mod stroke_tests {
    use super::*;
    use crate::flatten::DEFAULT_TOLERANCE;
    use crate::path::PathBuilder;

    /// A horizontal segment of the given length, open.
    fn segment(len: f32) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO).line_to(Vec2::new(len, 0.0));
        b.build()
    }

    #[test]
    fn butt_cap_covers_exactly_length_times_width() {
        let mut t = Tessellator::new();
        let buffers = t.stroke(&segment(10.0), &StrokeStyle::new(2.0), DEFAULT_TOLERANCE);

        assert!(buffers.is_well_formed());
        // Butt caps add nothing beyond the endpoints, so the stroke is exactly
        // the rectangle 10 x 2.
        let area = covered_area(buffers);
        assert!((area - 20.0).abs() < 0.01, "expected 20, got {area}");
    }

    #[test]
    fn round_cap_adds_a_disc_worth_of_area() {
        let mut t = Tessellator::new();
        let style = StrokeStyle::new(2.0).with_cap(LineCap::Round);
        let area = covered_area(t.stroke(&segment(10.0), &style, 0.01));

        // Two half-discs of radius 1 make one full disc. Tessellation
        // inscribes the arc, so the result lands just under.
        let expected = 20.0 + std::f32::consts::PI;
        assert!(
            (area - expected).abs() < 0.1,
            "expected about {expected}, got {area}"
        );
    }

    #[test]
    fn square_cap_extends_by_a_half_width_at_each_end() {
        let mut t = Tessellator::new();
        let style = StrokeStyle::new(2.0).with_cap(LineCap::Square);
        let area = covered_area(t.stroke(&segment(10.0), &style, DEFAULT_TOLERANCE));

        // Each cap adds a 1 x 2 block, so the stroke becomes 12 x 2.
        assert!((area - 24.0).abs() < 0.01, "expected 24, got {area}");
    }

    #[test]
    fn caps_are_ordered_by_the_area_they_add() {
        let mut t = Tessellator::new();
        let path = segment(10.0);
        let area = |cap| {
            let mut t2 = Tessellator::new();
            covered_area(t2.stroke(&path, &StrokeStyle::new(2.0).with_cap(cap), 0.01))
        };
        let (butt, round, square) = (
            area(LineCap::Butt),
            area(LineCap::Round),
            area(LineCap::Square),
        );
        assert!(butt < round && round < square, "{butt} {round} {square}");
        let _ = t.stroke(&path, &StrokeStyle::default(), DEFAULT_TOLERANCE);
    }

    #[test]
    fn width_scales_area_linearly() {
        let path = segment(10.0);
        let mut t = Tessellator::new();
        let narrow = covered_area(t.stroke(&path, &StrokeStyle::new(1.0), DEFAULT_TOLERANCE));
        let wide = covered_area(t.stroke(&path, &StrokeStyle::new(4.0), DEFAULT_TOLERANCE));
        assert!((wide - narrow * 4.0).abs() < 0.01, "{narrow} {wide}");
    }

    #[test]
    fn an_invisible_stroke_produces_nothing() {
        let mut t = Tessellator::new();
        for width in [0.0, -2.0, f32::NAN] {
            let buffers = t.stroke(&segment(10.0), &StrokeStyle::new(width), DEFAULT_TOLERANCE);
            assert!(buffers.is_empty(), "width {width} produced geometry");
        }
    }

    #[test]
    fn an_empty_path_produces_nothing() {
        let mut t = Tessellator::new();
        assert!(t
            .stroke(&Path::default(), &StrokeStyle::new(2.0), DEFAULT_TOLERANCE)
            .is_empty());
    }

    /// A corner of `degrees`, opening upward, apex at the origin.
    fn corner(degrees: f32) -> Path {
        let half = degrees.to_radians() / 2.0;
        let r = 50.0;
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(half.sin(), half.cos()) * r)
            .line_to(Vec2::ZERO)
            .line_to(Vec2::new(-half.sin(), half.cos()) * r);
        b.build()
    }

    fn join_area(path: &Path, join: LineJoin, limit: f32) -> f32 {
        let mut t = Tessellator::new();
        let style = StrokeStyle::new(3.0)
            .with_join(join)
            .with_miter_limit(limit);
        covered_area(t.stroke(path, &style, 0.02))
    }

    #[test]
    fn miter_degrades_to_bevel_at_the_svg_threshold() {
        // SVG degrades when 1/sin(angle/2) exceeds the limit, which at the
        // default limit of 4 falls at about 28.96 degrees. lyon expresses the
        // limit as half that ratio, so this is the test that catches the
        // conversion being dropped: without it the threshold lands near 14
        // degrees and both cases below would miter.
        let sharper = corner(28.5);
        assert_eq!(
            join_area(&sharper, LineJoin::Miter, 4.0),
            join_area(&sharper, LineJoin::Bevel, 4.0),
            "below the threshold a miter join must produce bevel geometry"
        );

        let shallower = corner(29.5);
        assert!(
            join_area(&shallower, LineJoin::Miter, 4.0)
                > join_area(&shallower, LineJoin::Bevel, 4.0),
            "above the threshold the miter must survive"
        );
    }

    #[test]
    fn raising_the_limit_re_enables_a_miter_that_would_otherwise_bevel() {
        let path = corner(20.0);
        let bevelled = join_area(&path, LineJoin::Miter, 4.0);
        let mitered = join_area(&path, LineJoin::Miter, 8.0);
        assert!(
            mitered > bevelled,
            "a higher limit should keep the spike: {mitered} vs {bevelled}"
        );
    }

    #[test]
    fn bevel_covers_less_than_miter_where_the_miter_survives() {
        let path = corner(90.0);
        assert!(join_area(&path, LineJoin::Bevel, 4.0) < join_area(&path, LineJoin::Miter, 4.0));
    }

    #[test]
    fn a_miter_limit_below_the_backend_minimum_does_not_panic() {
        // lyon asserts on a miter limit under 1.0, and the SVG-to-lyon
        // conversion halves the value, so any caller limit below 2 would reach
        // that assert unclamped. A stroke style is caller data; it must not be
        // able to abort the process.
        let path = corner(90.0);
        for limit in [0.0, 0.5, 1.0, 1.9] {
            let area = join_area(&path, LineJoin::Miter, limit);
            assert!(area > 0.0, "limit {limit} produced no geometry");
        }
    }

    #[test]
    fn stroking_a_curve_does_not_pre_flatten_into_spurious_joins() {
        // A smooth curve stroked with miter joins must not sprout spikes at
        // every flattening vertex. If it were pre-flattened, a tight miter
        // limit would change the area; on a genuinely smooth curve the joins
        // are all shallow and the limit is irrelevant.
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0)).cubic_to(
            Vec2::new(0.0, 40.0),
            Vec2::new(60.0, 40.0),
            Vec2::new(60.0, 0.0),
        );
        let path = b.build();

        let area = |limit: f32| {
            let mut t = Tessellator::new();
            let style = StrokeStyle::new(4.0)
                .with_join(LineJoin::Miter)
                .with_miter_limit(limit);
            covered_area(t.stroke(&path, &style, 0.1))
        };

        let (tight, generous) = (area(1.0), area(10.0));
        assert!(
            (tight - generous).abs() / generous < 0.01,
            "miter limit changed a smooth curve's area: {tight} vs {generous}"
        );
    }

    #[test]
    fn stroke_buffers_do_not_leak_between_calls() {
        let mut t = Tessellator::new();
        let big = t.stroke(&segment(100.0), &StrokeStyle::new(10.0), DEFAULT_TOLERANCE);
        let big_tris = big.triangle_count();
        assert!(big_tris > 0);

        let small = t.stroke(&segment(1.0), &StrokeStyle::new(1.0), DEFAULT_TOLERANCE);
        assert!(small.is_well_formed());
        let area = covered_area(small);
        assert!(
            (area - 1.0).abs() < 0.01,
            "stale geometry inflated area to {area}"
        );
    }
}

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
//!
//! # What the general path costs, against the floor
//!
//! lyon is not the cheapest way to triangulate a *simple* polygon, and the gap is
//! worth knowing because `non-parity.md` 18 rests on this being the expensive half.
//! Measured 2026-10-01 against `earcutr`, ear clipping, on x86-64 and on a Pi 5's
//! A76 -- identical triangle counts in every case, so the two agree on the output:
//!
//! | geometry | lyon : earcut, x86-64 | on A76 |
//! |---|---|---|
//! | 1,185 real tile rings, median 7 points | 1.8x | 1.7x |
//! | 160 stars, 8 points | 3.0x | 2.5x |
//! | 160 stars, 64 points | 2.9x | 2.2x |
//! | 160 stars, 256 points | 4.1x | 2.9x |
//!
//! So roughly a factor of two on real map geometry and up to four on dense concave
//! shapes. On the VisionFive 2, where one Berlin tile costs nineteen milliseconds to
//! triangulate, a factor of 1.7 is about eight milliseconds a tile.
//!
//! **It is not a swap.** Ear clipping needs a simple polygon: lyon resolves
//! self-intersection and both fill rules, and `non-parity.md` 18 depends on that -- a
//! self-intersecting path fills the same here as it does under a stencil precisely
//! because lyon applies the rule. Every one of the 1,185 tile rings happened to be
//! simple, which is what let the counts match, and MVT encoders do not guarantee it.
//!
//! **Claimed for a single contour, under a cap.** `fill` tries ear clipping on one
//! closed contour and verifies before using it, falling back to lyon otherwise -- see
//! `ear_fill`. The verification is what bounds it: proving a contour simple costs a
//! quadratic edge-pair test, which overtakes lyon's sweep at about thirty points, so
//! `MAX_EAR_POINTS` sits at twenty-four. That covers the median tile ring of seven and
//! leaves the dense end where it was. On the bench's twelve-point concave row the
//! recording goes from 0.175 ms to 0.119 ms, a third off; the seventy-two-point row
//! is past the cap, and a build with the cap lowered to refuse both rows reads the
//! twelve-point row at 0.175 ms again, which is what says the win is this route.
//!
//! Checking the output was not enough on its own, which took generated input to find
//! rather than reading -- `is_simple_polygon` has that story.

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

        // A single concave contour is what ear clipping serves and lyon
        // over-serves, and the output is checked rather than trusted. See
        // `ear_fill`.
        if polylines.len() == 1
            && ear_fill(strip_closing_duplicate(&polylines[0]), &mut self.buffers)
        {
            return &self.buffers;
        }
        self.buffers.clear();

        general_fill(
            &polylines,
            path.fill_rule(),
            &mut self.fill,
            &mut self.buffers,
        );
        &self.buffers
    }

    /// Fill through the general route, skipping both fast paths.
    ///
    /// `ear_fill`'s verification is the only thing between a wrong fill and the
    /// framebuffer, and the only way to state that as a property is against the route
    /// it replaces -- so `property.rs` generates contours, takes whichever route
    /// `fill` chose, and where it chose the fast one compares the area against this.
    ///
    /// Hidden because it is not a choice a caller should be making: the fast paths are
    /// selected by geometry, and asking for the slow one asks for the same picture at
    /// more cost.
    #[doc(hidden)]
    pub fn fill_general(&mut self, path: &Path, tolerance: f32) -> &VertexBuffers {
        self.buffers.clear();
        if !path.is_finite() || !path.is_within_tessellation_range() {
            return &self.buffers;
        }
        let polylines = flatten(path, usable_tolerance(tolerance));
        if polylines.is_empty() {
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

/// Triangulate one closed contour by ear clipping, and answer whether the result
/// was *proved* to be a triangulation of it.
///
/// Ear clipping is between 1.7 and 4 times cheaper than the general sweep -- the
/// module documentation has the measurements -- but it is only correct for a simple
/// polygon, and this renderer cannot know in advance that a contour is one. lyon can
/// be asked for any contour and resolves both fill rules; earcut cannot and does not.
///
/// So the precondition is tested rather than assumed, by `is_simple_polygon`, whose
/// documentation says why checking only the output is not sufficient. What is left
/// here is shape: `n - 2` triangles, which is how many a simple polygon of `n`
/// vertices has, every index inside the contour, and no triangle naming one of its
/// corners twice.
///
/// And for a contour that passes, the fill rule stops mattering: non-zero and
/// even-odd agree on a simple polygon, so there is nothing for the rule to decide.
/// That is what makes this safe to run before consulting it.
///
/// The verification is a quadratic edge-pair test against lyon's `O(n log n)` sweep,
/// so it only pays on a small contour. `MAX_EAR_POINTS` is where the two were
/// measured to cross.
fn ear_fill(points: &[Vec2], out: &mut VertexBuffers) -> bool {
    /// The largest contour this route will attempt.
    ///
    /// Two measurements set it. `earcutr` does not return on every finite polygon: a
    /// contour of 6,235 points spanning zero to 16,777,215, generated by `hostile.rs`,
    /// hung the suite inside `earcut` before any check here could run -- so a cap is
    /// the only guard that works, a verification being no use on an answer it never
    /// gets. And the simplicity test below overtakes what ear clipping saves:
    /// `examples/ear-crossover.rs` sweeps this route against `fill_general` over the
    /// bench's star contour, and on a desktop it runs 1.36x faster at 20 points, 1.15x
    /// at 24, 1.01x at 30 and 0.94x at 32. Past thirty the fast path is the slow one.
    ///
    /// Twenty-four, not thirty, because that crossover is a property of one machine's
    /// cache and the margin should survive another. It still covers the work: the
    /// vector tile rings measured for `non-parity.md` 18 have a median of seven points
    /// and a ninety-ninth percentile of fifty-four. The tail goes to lyon, which
    /// returns on anything.
    const MAX_EAR_POINTS: usize = 24;

    let n = points.len();
    if !(3..=MAX_EAR_POINTS).contains(&n) {
        return false;
    }
    if !is_simple_polygon(points) {
        return false;
    }

    let flat: Vec<f64> = points
        .iter()
        .flat_map(|p| [f64::from(p.x), f64::from(p.y)])
        .collect();
    let Ok(indices) = earcutr::earcut(&flat, &[], 2) else {
        return false;
    };
    if indices.len() != (n - 2) * 3 {
        return false;
    }

    // Indices must address the contour and name three distinct corners. Earcut's
    // edge bookkeeping is not re-derived beyond that: `is_simple_polygon` above
    // establishes the precondition directly, and the edge-parity map that stood
    // here allocated on every call -- more than ear clipping saves.
    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
        if a >= n || b >= n || c >= n || a == b || b == c || a == c {
            return false;
        }
    }

    out.vertices.extend_from_slice(points);
    out.indices
        .extend(indices.into_iter().map(|index| index as u32));
    true
}

/// Whether a closed contour is a simple polygon: no two edges meet except at a vertex
/// they share.
///
/// **This exists because the cheap checks were not enough, and a generated test said
/// so.** `ear_fill` first verified `earcutr`'s output by triangle count and edge
/// parity, reasoning that a triangulation bounded by the input edges can only be a
/// triangulation of the input. `property.rs` refuted that on its first run with five
/// points -- `[(40,20), (50,20), (0,30), (50,0), (30,60)]`, which self-intersects, for
/// which earcut returns exactly `n - 2` triangles whose edges pass parity, and whose
/// area is 500 against the 494.2 the non-zero rule fills.
///
/// So simplicity is tested rather than inferred. Quadratic in the edge count, which is
/// why `MAX_EAR_POINTS` is small: at twenty-four points it is 252 segment pairs of a
/// few operations each, under what lyon's sweep costs on that contour, and at five
/// hundred it would be many times over.
///
/// Conservative at every boundary. A zero-length edge, a touch at an endpoint, a
/// collinear overlap -- each returns false and sends the contour to lyon, which
/// resolves all of them under the rule the caller asked for.
fn is_simple_polygon(points: &[Vec2]) -> bool {
    let n = points.len();
    let edge = |i: usize| (points[i], points[(i + 1) % n]);

    for i in 0..n {
        let (a, b) = edge(i);
        if a == b {
            return false;
        }
        // Pairs sharing a vertex are adjacent and may meet there; every other pair
        // must not meet at all. `j` starts past `i + 1`, and the wrap pair -- edge
        // `n - 1` against edge `0` -- is excluded by the upper bound.
        let last = if i == 0 { n - 1 } else { n };
        for j in (i + 2)..last {
            let (c, d) = edge(j);
            if segments_meet(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

/// Whether two segments touch or cross anywhere, endpoints and collinear overlaps
/// included.
fn segments_meet(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).perp_dot(r - p);
    let (d1, d2) = (side(c, d, a), side(c, d, b));
    let (d3, d4) = (side(a, b, c), side(a, b, d));

    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    // A zero is collinear or touching, and a point inside the other segment's extent
    // is a meeting -- counted whether or not it crosses, because a contour that
    // touches itself is not one this route will reason about.
    let within = |p: Vec2, q: Vec2, r: Vec2| {
        r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
    };
    (d1 == 0.0 && within(c, d, a))
        || (d2 == 0.0 && within(c, d, b))
        || (d3 == 0.0 && within(a, b, c))
        || (d4 == 0.0 && within(a, b, d))
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

#[cfg(test)]
mod ear_clipping {
    use super::*;

    /// An L, which is concave and simple: five vertices, three triangles.
    fn ell() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(40.0, 0.0),
            Vec2::new(40.0, 10.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(10.0, 40.0),
            Vec2::new(0.0, 40.0),
        ]
    }

    /// A bowtie: the only self-intersecting contour in this module, and the case the
    /// verification exists for.
    fn bowtie() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(40.0, 40.0),
            Vec2::new(40.0, 0.0),
            Vec2::new(0.0, 40.0),
        ]
    }

    fn triangle_area_sum(out: &VertexBuffers) -> f32 {
        out.indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (
                    out.vertices[t[0] as usize],
                    out.vertices[t[1] as usize],
                    out.vertices[t[2] as usize],
                );
                ((b - a).perp_dot(c - a) * 0.5).abs()
            })
            .sum()
    }

    /// A concave simple contour takes the fast route and is proved.
    #[test]
    fn a_concave_simple_contour_is_ear_clipped() {
        let points = ell();
        let mut out = VertexBuffers::default();
        assert!(ear_fill(&points, &mut out), "an L is simple and concave");

        // Ear clipping of an n-gon is exactly n - 2 triangles over the n input
        // vertices, and adds none of its own.
        assert_eq!(out.vertices.len(), points.len());
        assert_eq!(out.indices.len(), (points.len() - 2) * 3);
        // The L covers 40x10 plus 10x30.
        assert!((triangle_area_sum(&out) - 700.0).abs() < 0.01);
    }

    /// `fill` routes a concave contour here, not just `ear_fill` when called directly.
    ///
    /// Without this the rest of the module tests a function nothing reaches, and the
    /// generated cross-check in `tests/property.rs` would pass on a dispatch that never
    /// chose this route. A fan cannot produce this output on this contour: the L is
    /// concave, so `polygon_convexity` refuses the fan, and lyon adds vertices of its
    /// own and would not return the contour back unchanged.
    #[test]
    fn fill_sends_a_concave_contour_to_ear_clipping() {
        let points = ell();
        let mut builder = Path::builder().with_fill_rule(FillRule::NonZero);
        builder.move_to(points[0]);
        for p in &points[1..] {
            builder.line_to(*p);
        }
        builder.close();
        let path = builder.build();
        assert_eq!(path.convexity(), Convexity::Concave, "the L is concave");

        let mut tess = Tessellator::new();
        let filled = tess.fill(&path, 0.25);
        assert_eq!(filled.vertices, points, "the contour's own vertices");
        assert_eq!(filled.indices.len(), (points.len() - 2) * 3);
    }

    /// A self-intersecting contour is refused, and lyon is what fills it.
    ///
    /// This is the property the whole verification exists for: ear clipping returns
    /// *something* for a bowtie, and that something is not the non-zero fill.
    #[test]
    fn a_self_intersecting_contour_is_refused() {
        let mut out = VertexBuffers::default();
        assert!(
            !ear_fill(&bowtie(), &mut out),
            "a bowtie must not pass verification"
        );
        assert!(out.indices.is_empty(), "a refusal writes nothing");

        // And the whole path still fills, through the general route, with the two
        // lobes the non-zero rule asks for: 20x20 each, halved, twice.
        let mut builder = Path::builder().with_fill_rule(FillRule::NonZero);
        builder.move_to(bowtie()[0]);
        for p in &bowtie()[1..] {
            builder.line_to(*p);
        }
        builder.close();
        let mut tess = Tessellator::new();
        let filled = tess.fill(&builder.build(), 0.25);
        assert!(
            (triangle_area_sum(filled) - 800.0).abs() < 1.0,
            "two lobes of 400: {}",
            triangle_area_sum(filled)
        );
    }

    /// Past the cap it declines without calling earcut.
    ///
    /// The cap is what stands between this route and the 6,235-point contour that hung
    /// `hostile.rs`, so it is asserted rather than left to the constant. Five hundred
    /// and thirteen points is far past any cap this route would be given, so the
    /// assertion does not move when the cap does.
    #[test]
    fn a_contour_past_the_cap_is_declined() {
        let big: Vec<Vec2> = (0..513)
            .map(|i| {
                let t = i as f32 * 0.01;
                Vec2::new(t.cos() * 100.0, t.sin() * 100.0)
            })
            .collect();
        let mut out = VertexBuffers::default();
        assert!(!ear_fill(&big, &mut out), "past the cap");
        assert!(out.indices.is_empty());
    }
}

#[cfg(test)]
mod routes_agree_on_real_shapes {
    use super::*;
    use crate::superellipse::RoundSuperellipse;

    fn area(out: &VertexBuffers) -> f64 {
        out.indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (
                    out.vertices[t[0] as usize],
                    out.vertices[t[1] as usize],
                    out.vertices[t[2] as usize],
                );
                f64::from(((b - a).perp_dot(c - a) * 0.5).abs())
            })
            .sum()
    }

    /// Whichever route a real shape takes, it fills the same area.
    ///
    /// `property.rs` states this over generated lattice contours, where
    /// self-intersection is common. This states it over the shapes the corpus actually
    /// draws, where the risk is the opposite one: a smooth curve flattens to hundreds
    /// of nearly-collinear points, which is where a convexity test or an ear clipper
    /// is most likely to disagree with a sweep by a little rather than a lot.
    ///
    /// Added because the superellipse's recorded cost row moved when the fast paths
    /// changed -- 301 triangles to 299 -- and a different triangulation of the same
    /// region is fine while a different region is not. Counting triangles cannot tell
    /// those apart; area can.
    #[test]
    fn a_rounded_superellipse_fills_the_same_area_either_route() {
        for (size, radius) in [(100.0, 25.0), (100.0, 49.0), (240.0, 60.0), (64.0, 8.0)] {
            let path = RoundSuperellipse::with_radius(
                crate::Rect::new(Vec2::ZERO, Vec2::new(size, size)),
                radius,
            )
            .to_path();
            let mut tess = Tessellator::new();
            let chosen = area(tess.fill(&path, 0.25));
            let general = area(tess.fill_general(&path, 0.25));
            assert!(
                (chosen - general).abs() <= 0.001 * general.max(1.0),
                "size {size} radius {radius}: chosen route fills {chosen}, general {general}"
            );
        }
    }
}

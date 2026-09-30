//! Path representation.
//!
//! Paths are stored as a verb stream with a parallel point buffer rather than
//! as an enum-per-segment vector. Tessellation walks both linearly, and
//! keeping points contiguous means the walk touches one cache line per few
//! segments instead of chasing a pointer per segment.

use glam::Vec2;

/// How overlapping subpaths combine when filling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    /// A point is inside when the signed crossing count is nonzero.
    #[default]
    NonZero,
    /// A point is inside when the crossing count is odd.
    EvenOdd,
}

/// A path segment operator.
///
/// Each verb consumes a fixed number of points from the point buffer, given by
/// [`Verb::point_count`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    MoveTo,
    LineTo,
    QuadTo,
    CubicTo,
    Close,
}

impl Verb {
    /// Points this verb consumes from the point buffer.
    pub const fn point_count(self) -> usize {
        match self {
            Self::MoveTo | Self::LineTo => 1,
            Self::QuadTo => 2,
            Self::CubicTo => 3,
            Self::Close => 0,
        }
    }
}

/// An axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// The empty rect, which absorbs any point when unioned.
    pub fn empty() -> Self {
        Self {
            min: Vec2::splat(f32::INFINITY),
            max: Vec2::splat(f32::NEG_INFINITY),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    pub fn union_point(&mut self, p: Vec2) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn width(&self) -> f32 {
        (self.max.x - self.min.x).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.max.y - self.min.y).max(0.0)
    }
}

/// Whether a subpath is convex, which selects the fill strategy.
///
/// Convex paths take a fan-fill fast path that skips both tessellation and the
/// stencil pass, so detection is worth doing — but a wrong `Convex` answer
/// renders incorrectly, while a wrong `Concave` answer only costs speed.
/// Detection is therefore conservative: anything it cannot prove convex is
/// reported [`Convexity::Concave`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convexity {
    Convex,
    Concave,
}

/// A path: a verb stream plus the points those verbs consume.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    verbs: Vec<Verb>,
    points: Vec<Vec2>,
    fill_rule: FillRule,
    /// The rounded rectangle this path was built from, if it was one.
    ///
    /// A path is a verb stream, and once a shape has been written into one
    /// there is no way back: a rounded rectangle and a hand-drawn outline that
    /// happens to match it are the same path. Some routes want the shape rather
    /// than the outline -- the blurred rounded rectangle is evaluated in the
    /// fragment stage and needs a rect and a radius, not eight cubics -- so
    /// what built it is recorded here rather than reconstructed by inspection,
    /// which is the version of this that misfires on a shape it merely
    /// resembles.
    ///
    /// Upstream keeps the same thing on `DlPath`, which can still answer
    /// whether it is a round rect after being consumed as a path.
    ///
    /// Set only by a builder that actually laid down that shape. It is part of
    /// equality because two paths differing in it are two different claims
    /// about the same outline, and nothing here compares whole paths anyway.
    rounded_rect: Option<(Rect, f32)>,
}

/// The largest coordinate magnitude the tessellator will accept.
///
/// Two to the twenty-fourth, which is the largest integer an `f32` represents
/// exactly. See [`Path::is_within_tessellation_range`] for why it is here and
/// why this is the value.
pub const MAX_COORDINATE: f32 = 16_777_216.0;

impl Path {
    pub fn builder() -> PathBuilder {
        PathBuilder::new()
    }

    /// The rounded rectangle this was built from, if it was built from one.
    ///
    /// A radius of zero is a plain rectangle and is reported as such; `None`
    /// means only that nothing recorded a shape, never that the outline is not
    /// one.
    pub fn as_rounded_rect(&self) -> Option<(Rect, f32)> {
        self.rounded_rect
    }

    pub fn verbs(&self) -> &[Verb] {
        &self.verbs
    }

    pub fn points(&self) -> &[Vec2] {
        &self.points
    }

    /// Whether every point in this path is a real location.
    ///
    /// A path carrying a NaN or an infinity is not a shape. It arrives when a
    /// caller's own arithmetic has already gone wrong -- a division by a zero
    /// extent, an inverted degenerate transform -- and there is no picture it
    /// asks for, so the tessellator refuses one rather than inventing it.
    ///
    /// Worth having as a question rather than a debug assertion because lyon
    /// asserts on a non-finite coordinate, and an assertion in a dependency
    /// takes the process down. A library given a bad number should decline to
    /// draw, not abort the application holding it.
    pub fn is_finite(&self) -> bool {
        self.points.iter().all(|p| p.is_finite())
    }

    /// Whether every coordinate is small enough to tessellate.
    ///
    /// Finite is not the same as usable, and the gap between them is where the
    /// tessellator's cost stops being bounded. `f32::MAX` is finite; so is
    /// `1e15`, and a path with three verbs at that magnitude strokes to
    /// thirty-one million vertices -- six hundred megabytes of position and
    /// index from a cubic and a close. The fill route is safe from it because
    /// this crate's own flattener caps at [`MAX_SEGMENTS`], but a stroke hands
    /// its curves to lyon intact, deliberately and for the reason
    /// `Tessellator::stroke` gives, and lyon subdivides by its own arithmetic
    /// with no such cap. The output grows linearly in the coordinate, which
    /// makes it a caller-controlled allocation with nothing at the top of it.
    ///
    /// [`MAX_COORDINATE`] is where that stops, and the limit is the same one
    /// two different arguments arrive at. A float past two to the twenty-fourth
    /// has an interval above one between it and its neighbor, so a coordinate
    /// there cannot name a pixel and no picture depends on it. And measured, a
    /// curve at that magnitude strokes to about twenty thousand vertices,
    /// which is a shape rather than an allocation.
    ///
    /// [`MAX_SEGMENTS`]: crate::flatten::MAX_SEGMENTS
    pub fn is_within_tessellation_range(&self) -> bool {
        self.points
            .iter()
            .all(|p| p.x.abs() <= MAX_COORDINATE && p.y.abs() <= MAX_COORDINATE)
    }

    pub fn fill_rule(&self) -> FillRule {
        self.fill_rule
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    /// Bounds of the control points.
    ///
    /// This is a bound, not a tight fit: a curve lies within the convex hull of
    /// its control points, so a curve that bends away from its handles reports
    /// a larger box than it occupies. That is the right trade for culling,
    /// where a conservative overestimate is safe and cheap while a tight fit
    /// costs a solve per curve.
    pub fn bounds(&self) -> Rect {
        let mut r = Rect::empty();
        for p in &self.points {
            r.union_point(*p);
        }
        r
    }

    /// Walk the path as `(verb, points)` pairs.
    pub fn segments(&self) -> impl Iterator<Item = (Verb, &[Vec2])> {
        let mut cursor = 0usize;
        self.verbs.iter().map(move |verb| {
            let n = verb.point_count();
            let slice = &self.points[cursor..cursor + n];
            cursor += n;
            (*verb, slice)
        })
    }

    /// Whether the path is provably convex.
    ///
    /// Answers [`Convexity::Concave`] for anything with more than one subpath,
    /// or containing curves, rather than analyzing further. Curved paths are
    /// classified after flattening, where the question is a polygon question.
    pub fn convexity(&self) -> Convexity {
        if self
            .verbs
            .iter()
            .any(|v| matches!(v, Verb::QuadTo | Verb::CubicTo))
        {
            return Convexity::Concave;
        }
        if self.verbs.iter().filter(|v| **v == Verb::MoveTo).count() > 1 {
            return Convexity::Concave;
        }
        polygon_convexity(&self.points)
    }
}

/// Which quadrant a direction points into, as a number that increases
/// counter-clockwise.
///
/// Zero-length directions land in the first quadrant, which is harmless: a
/// repeated point contributes no rotation and the count below is of net
/// advance, not of steps.
fn quadrant(v: Vec2) -> i32 {
    match (v.x >= 0.0, v.y >= 0.0) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    }
}

/// Classify a polygon by the consistency of its turn directions.
///
/// A simple polygon is convex when every turn goes the same way. Collinear
/// points contribute a zero cross product and are skipped rather than treated
/// as a reversal, since they are common in generated geometry and do not
/// affect convexity.
///
/// Turn agreement alone is not enough, because a self-crossing polygon can
/// have every turn agree: a pentagram turns the same way at all five points
/// and is not remotely convex. What separates them is how far the polygon
/// turns in total. Walking a simple closed polygon comes back to the start
/// having turned through exactly one full circle; walking a pentagram turns
/// through two.
///
/// That total is counted rather than measured. Once the turns are known to
/// agree the direction rotates monotonically, so each step advances through
/// zero, one or two quadrants in the one direction and never doubles back --
/// and summing those advances gives four per revolution exactly. The
/// alternative, accumulating the turn angles with `atan2`, computes the same
/// number and measured six times slower than the whole rest of this function,
/// on a path that runs once per filled path per frame.
///
/// This matters well beyond classification. The caller uses `Convex` to take a
/// fan fill, which triangulates from one vertex and applies no fill rule at
/// all -- so a self-crossing polygon called convex is filled by a routine that
/// cannot express what filling it means, and its fill rule is silently
/// discarded.
pub fn polygon_convexity(points: &[Vec2]) -> Convexity {
    if points.len() < 3 {
        return Convexity::Convex;
    }
    let n = points.len();
    let mut sign = 0i32;
    // Both readings of the advance, since which one counts depends on the
    // direction of travel and that is not settled until the walk finishes.
    // Two running sums rather than a list of steps, so this allocates nothing.
    let (mut counter_clockwise, mut clockwise) = (0i32, 0i32);
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        let c = points[(i + 2) % n];
        let incoming = b - a;
        let outgoing = c - b;
        let cross = incoming.perp_dot(outgoing);
        let advance = (quadrant(outgoing) - quadrant(incoming)).rem_euclid(4);
        counter_clockwise += advance;
        clockwise += (-advance).rem_euclid(4);
        if cross.abs() <= f32::EPSILON {
            continue;
        }
        let s = if cross > 0.0 { 1 } else { -1 };
        if sign == 0 {
            sign = s;
        } else if sign != s {
            return Convexity::Concave;
        }
    }
    if sign == 0 {
        // Every turn was collinear, so the polygon is a segment traversed out
        // and back. Degenerate rather than self-crossing, and it covers no
        // area either way.
        return Convexity::Convex;
    }
    let turned = if sign > 0 {
        counter_clockwise
    } else {
        clockwise
    };
    if turned > 4 {
        return Convexity::Concave;
    }
    Convexity::Convex
}

/// Accumulates verbs and points into a [`Path`].
#[derive(Debug, Clone, Default)]
pub struct PathBuilder {
    verbs: Vec<Verb>,
    points: Vec<Vec2>,
    fill_rule: FillRule,
    /// Where the current subpath started, for `close`.
    subpath_start: Option<Vec2>,
    current: Option<Vec2>,
    rounded_rect: Option<(Rect, f32)>,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fill_rule(mut self, rule: FillRule) -> Self {
        self.fill_rule = rule;
        self
    }

    pub fn move_to(&mut self, p: Vec2) -> &mut Self {
        self.verbs.push(Verb::MoveTo);
        self.points.push(p);
        self.subpath_start = Some(p);
        self.current = Some(p);
        self
    }

    /// Add a line segment.
    ///
    /// A line before any `move_to` implicitly starts the subpath at the
    /// origin, matching how path data from SVG and font outlines behaves when
    /// it omits the opening move.
    pub fn line_to(&mut self, p: Vec2) -> &mut Self {
        self.ensure_started();
        self.verbs.push(Verb::LineTo);
        self.points.push(p);
        self.current = Some(p);
        self
    }

    pub fn quad_to(&mut self, ctrl: Vec2, to: Vec2) -> &mut Self {
        self.ensure_started();
        self.verbs.push(Verb::QuadTo);
        self.points.push(ctrl);
        self.points.push(to);
        self.current = Some(to);
        self
    }

    /// Append a rational quadratic -- a conic -- through one control point.
    ///
    /// The curve a circular arc actually is, and the one a font or an SVG
    /// hands over. `weight` is the control point's pull: one is an ordinary
    /// quadratic, less than one is elliptical, more is hyperbolic, and
    /// `sqrt(2)/2` with the control at the corner of a square is exactly a
    /// quarter circle.
    ///
    /// # Why this becomes quadratics here rather than surviving as a verb
    ///
    /// A conic of weight one *is* a quadratic, and splitting a conic in half
    /// moves its weight toward one -- quadratically, so a handful of splits
    /// puts it within a thousandth. So this subdivides until the weight is
    /// near enough and emits ordinary quadratics, which every part of this
    /// crate and the tessellator already understand.
    ///
    /// The alternative is a verb of its own, converted during flattening where
    /// the transform's scale is known. That is what a renderer does when it
    /// wants an absolute error in device pixels. What is bought here instead
    /// is a *relative* error: the approximation is a fixed fraction of the
    /// curve's own size, so it stays correct at every scale the path is later
    /// drawn at, and nothing downstream has to learn a fifth verb.
    ///
    /// # Subdividing by the error rather than by the weight
    ///
    /// The criterion below stops when the *error* is small enough, and that is
    /// worth spelling out because the obvious criterion -- stop when the
    /// weight is near one -- is what it replaced, and it was expensive by a
    /// factor of four.
    ///
    /// A weight criterion counts halvings of the weight's distance from one,
    /// and each halving doubles the output. So a curve gets subdivided by how
    /// hyperbolic it is rather than by how much the approximation misses. A
    /// rounded superellipse octant emits conics at weights around 0.7 and 8.3;
    /// under the weight criterion those became thirty-two and sixty-four
    /// quadratics, and the corpus scene cost 1340 vertices where a circle
    /// costs 40. It is 305 now, and the stroked one 274 against 922.
    ///
    /// The error itself is four lines: a rational quadratic and its weight-one
    /// counterpart differ most at their midpoints, both midpoints have closed
    /// forms, and the distance between them measured against the chord is a
    /// relative bound -- the same quantity the weight criterion was a proxy
    /// for, at the same tolerance of a thousandth.
    ///
    /// It is a little less accurate where the proxy over-subdivided, which is
    /// most places. Measured against the old criterion: four pixels of the
    /// filled corpus scene and eight of the stroked one, out of sixteen
    /// thousand, and all of them at the shape's tangent extremes where the
    /// outline lies along a pixel boundary and a sub-pixel shift moves every
    /// sample in the pixel together. Every comparison in the workspace passes
    /// unchanged, including the pin on upstream's boundary points, which is
    /// what says the outline is still the shape upstream draws.
    ///
    /// A weight that is zero or negative or not finite describes no curve, and
    /// gives the straight line between the ends.
    pub fn conic_to(&mut self, ctrl: Vec2, to: Vec2, weight: f32) -> &mut Self {
        self.ensure_started();
        let from = self.current.unwrap_or(ctrl);
        if !weight.is_finite() || weight <= 0.0 || !ctrl.is_finite() || !to.is_finite() {
            return self.line_to(to);
        }
        self.push_conic(from, ctrl, to, weight, 0);
        self
    }

    /// Split until the weight is near one, then emit a quadratic.
    fn push_conic(&mut self, from: Vec2, ctrl: Vec2, to: Vec2, weight: f32, depth: u32) {
        // A thousandth of the way from one is close enough that the quadratic
        // and the conic differ by well under a thousandth of the curve's own
        // extent, which no rasterizer this feeds can show.
        //
        // The depth cap is not expected to be reached: the weight halves its
        // distance from one roughly every split, so even a wildly hyperbolic
        // conic converges in a handful. It is here because a bound that cannot
        // be reached costs nothing and a recursion without one is a stack
        // overflow waiting for an input nobody thought of.
        const NEAR_ONE: f32 = 1e-3;
        const MAX_DEPTH: u32 = 6;
        const RELATIVE_ERROR: f32 = 1e-3;
        if (weight - 1.0).abs() <= NEAR_ONE || depth >= MAX_DEPTH {
            self.quad_to(ctrl, to);
            return;
        }
        // The weight is a proxy and this is the thing itself: how far the
        // quadratic actually lies from the conic. The two are only loosely
        // related, and the proxy is the expensive way round -- each halving of
        // the weight's distance from one doubles the output, so a curve gets
        // subdivided by how hyperbolic it is rather than by how much the
        // approximation misses.
        //
        // Compared at the midpoints, which is where a rational quadratic and
        // its weight-one counterpart differ most, and against the chord rather
        // than an absolute length. That is what keeps the bound *relative* --
        // the property the note above is about, and the reason this can be
        // decided here rather than during flattening where the scale is known.
        let conic_mid = (from + ctrl * (2.0 * weight) + to) / (2.0 * (1.0 + weight));
        let quad_mid = (from + ctrl * 2.0 + to) * 0.25;
        let chord = (to - from).length();
        if chord > 0.0 && (conic_mid - quad_mid).length() <= RELATIVE_ERROR * chord {
            self.quad_to(ctrl, to);
            return;
        }

        // De Casteljau for a rational quadratic, at the halfway parameter. The
        // denominators are the weights the same construction gives the control
        // points, which is why the midpoint is not the average of three
        // points.
        let scale = 1.0 / (1.0 + weight);
        let left_ctrl = (from + ctrl * weight) * scale;
        let right_ctrl = (ctrl * weight + to) * scale;
        let mid = (from + ctrl * (2.0 * weight) + to) * (0.5 * scale);
        let split_weight = ((1.0 + weight) * 0.5).sqrt();

        self.push_conic(from, left_ctrl, mid, split_weight, depth + 1);
        self.push_conic(mid, right_ctrl, to, split_weight, depth + 1);
    }

    pub fn cubic_to(&mut self, c0: Vec2, c1: Vec2, to: Vec2) -> &mut Self {
        self.ensure_started();
        self.verbs.push(Verb::CubicTo);
        self.points.push(c0);
        self.points.push(c1);
        self.points.push(to);
        self.current = Some(to);
        self
    }

    /// Append an elliptical arc, centered at `center` with the given radii.
    ///
    /// Angles are in radians, measured from the positive X axis toward positive
    /// Y, and `sweep` may be negative to travel the other way. A sweep of a
    /// full turn or more is clamped to one: going round twice draws the same
    /// pixels as going round once, and the extra segments are cost without a
    /// picture.
    ///
    /// If the path has a current point, a line joins it to the arc's start, the
    /// way SVG and Skia both behave; otherwise the arc begins with a move. That
    /// is what makes a pie slice `move_to(center)` then `arc(..)` then `close`,
    /// and a progress ring just `arc(..)` on an empty builder.
    ///
    /// # Accuracy
    ///
    /// The arc is emitted as cubics of at most a quarter turn each, with
    /// control points at `(4/3)·tan(θ/4)` of the radius for a segment spanning
    /// `θ`. That expression is where the familiar 0.5523 comes from — it is
    /// this evaluated at a right angle — and using the constant for any other
    /// angle is the usual way to draw an arc that is visibly wrong near its
    /// ends. At a quarter turn the worst radial error is under three parts in
    /// ten thousand, so a circle a thousand pixels across is off by less than a
    /// third of a pixel.
    pub fn arc(&mut self, center: Vec2, radii: Vec2, start: f32, sweep: f32) -> &mut Self {
        if !center.is_finite() || !radii.is_finite() || !start.is_finite() || !sweep.is_finite() {
            return self;
        }
        let point_at = |angle: f32| {
            Vec2::new(
                center.x + radii.x * angle.cos(),
                center.y + radii.y * angle.sin(),
            )
        };
        let first = point_at(start);
        match self.current {
            Some(_) => {
                self.line_to(first);
            }
            None => {
                self.move_to(first);
            }
        }

        let turn = std::f32::consts::TAU;
        let sweep = sweep.clamp(-turn, turn);
        if sweep == 0.0 {
            return self;
        }
        // At most a quarter turn per cubic, which is where the error bound
        // below holds. More segments would be more accurate and are not needed;
        // fewer are visibly wrong.
        let segments = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0);
        let step = sweep / segments;
        // The tangent handles are proportional to the *derivative* at the
        // endpoints, which for an ellipse carries the radii, so each is scaled
        // by its own axis rather than by a single radius.
        let k = (4.0 / 3.0) * (step / 4.0).tan();
        let mut angle = start;
        for _ in 0..segments as u32 {
            let next = angle + step;
            let (from, to) = (point_at(angle), point_at(next));
            // The derivative of the parameterization, which is the tangent
            // direction scaled by the radii.
            let d_from = Vec2::new(-radii.x * angle.sin(), radii.y * angle.cos());
            let d_to = Vec2::new(-radii.x * next.sin(), radii.y * next.cos());
            self.cubic_to(from + d_from * k, to - d_to * k, to);
            angle = next;
        }
        self
    }

    /// Close the current subpath.
    ///
    /// Closing an empty builder is a no-op rather than an error, so callers
    /// replaying arbitrary path data do not have to special-case it.
    pub fn close(&mut self) -> &mut Self {
        if self.verbs.is_empty() {
            return self;
        }
        self.verbs.push(Verb::Close);
        self.current = self.subpath_start;
        self
    }

    /// Current pen position, if the path has started.
    pub fn current_point(&self) -> Option<Vec2> {
        self.current
    }

    fn ensure_started(&mut self) {
        if self.verbs.is_empty() {
            self.verbs.push(Verb::MoveTo);
            self.points.push(Vec2::ZERO);
            self.subpath_start = Some(Vec2::ZERO);
            self.current = Some(Vec2::ZERO);
        }
    }

    /// Record that what was laid down is this rounded rectangle.
    ///
    /// An assertion by the caller, not a check: nothing here reads the verbs
    /// back to confirm it, because the only caller is the one that just wrote
    /// them. A builder that marks a shape it did not draw gets that shape drawn
    /// wherever a route prefers the shape to the outline -- so this is called
    /// beside the drawing, never afterwards from somewhere that inferred it.
    ///
    /// A radius at or below zero is a plain rectangle and is recorded as such.
    /// Anything not finite records nothing, since it describes no shape.
    pub fn as_rounded_rect(&mut self, rect: Rect, radius: f32) -> &mut Self {
        let finite = rect.min.x.is_finite()
            && rect.min.y.is_finite()
            && rect.max.x.is_finite()
            && rect.max.y.is_finite()
            && radius.is_finite();
        self.rounded_rect = finite.then(|| (rect, radius.max(0.0)));
        self
    }

    pub fn build(self) -> Path {
        Path {
            verbs: self.verbs,
            points: self.points,
            fill_rule: self.fill_rule,
            rounded_rect: self.rounded_rect,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verb_point_counts_match_what_the_builder_pushes() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(1.0, 0.0))
            .quad_to(Vec2::new(2.0, 1.0), Vec2::new(3.0, 0.0))
            .cubic_to(
                Vec2::new(4.0, 1.0),
                Vec2::new(5.0, -1.0),
                Vec2::new(6.0, 0.0),
            )
            .close();
        let path = b.build();

        let expected: usize = path.verbs().iter().map(|v| v.point_count()).sum();
        // A mismatch here would desynchronize the parallel buffers and make
        // every later segment read the wrong points.
        assert_eq!(expected, path.points().len());
    }

    #[test]
    fn segments_walk_verbs_and_points_in_step() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .quad_to(Vec2::new(1.0, 1.0), Vec2::new(2.0, 0.0));
        let path = b.build();

        let collected: Vec<_> = path.segments().collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].0, Verb::MoveTo);
        assert_eq!(collected[0].1, &[Vec2::ZERO]);
        assert_eq!(collected[1].0, Verb::QuadTo);
        assert_eq!(collected[1].1, &[Vec2::new(1.0, 1.0), Vec2::new(2.0, 0.0)]);
    }

    #[test]
    fn line_without_move_starts_at_origin() {
        let mut b = PathBuilder::new();
        b.line_to(Vec2::new(1.0, 1.0));
        let path = b.build();
        assert_eq!(path.verbs()[0], Verb::MoveTo);
        assert_eq!(path.points()[0], Vec2::ZERO);
    }

    #[test]
    fn closing_an_empty_builder_is_a_no_op() {
        let path = {
            let mut b = PathBuilder::new();
            b.close();
            b.build()
        };
        assert!(path.is_empty());
    }

    #[test]
    fn close_returns_the_pen_to_the_subpath_start() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(5.0, 5.0))
            .line_to(Vec2::new(9.0, 5.0))
            .close();
        assert_eq!(b.current_point(), Some(Vec2::new(5.0, 5.0)));
    }

    #[test]
    fn bounds_cover_every_control_point() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0)).cubic_to(
            Vec2::new(0.0, 10.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(10.0, 0.0),
        );
        let path = b.build();
        let bounds = path.bounds();
        for p in path.points() {
            assert!(bounds.contains(*p), "bounds must contain {p:?}");
        }
        // Control-point hull, not a tight fit: the curve never reaches y=10.
        assert_eq!(bounds.max.y, 10.0);
    }

    #[test]
    fn empty_path_has_empty_bounds() {
        let path = Path::default();
        assert!(path.bounds().is_empty());
        assert_eq!(path.bounds().width(), 0.0);
    }

    #[test]
    fn square_is_convex_and_chevron_is_not() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        assert_eq!(polygon_convexity(&square), Convexity::Convex);

        // A chevron reverses turn direction at the notch.
        let chevron = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(2.0, 4.0),
        ];
        assert_eq!(polygon_convexity(&chevron), Convexity::Concave);
    }

    #[test]
    fn collinear_points_do_not_break_convexity() {
        // Generated geometry frequently contains collinear runs; treating a
        // zero cross product as a reversal would reject valid fan-fill cases.
        let with_collinear = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        assert_eq!(polygon_convexity(&with_collinear), Convexity::Convex);
    }

    #[test]
    fn convexity_is_conservative_about_curves_and_subpaths() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .quad_to(Vec2::new(1.0, 1.0), Vec2::new(2.0, 0.0));
        // Curves are classified after flattening, so the unflattened answer
        // errs toward the slow path rather than risking a wrong fast path.
        assert_eq!(b.build().convexity(), Convexity::Concave);

        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(1.0, 0.0))
            .close()
            .move_to(Vec2::new(5.0, 5.0))
            .line_to(Vec2::new(6.0, 5.0));
        assert_eq!(b.build().convexity(), Convexity::Concave);
    }

    #[test]
    fn degenerate_polygons_are_trivially_convex() {
        assert_eq!(polygon_convexity(&[]), Convexity::Convex);
        assert_eq!(polygon_convexity(&[Vec2::ZERO]), Convexity::Convex);
        assert_eq!(
            polygon_convexity(&[Vec2::ZERO, Vec2::new(1.0, 1.0)]),
            Convexity::Convex
        );
    }

    /// Every point on a flattened path, for checking a shape rather than a
    /// vertex list.
    fn points_of(path: &Path) -> Vec<Vec2> {
        crate::flatten::flatten(path, 0.01)
            .into_iter()
            .flatten()
            .collect()
    }

    #[test]
    fn an_arc_stays_on_its_circle() {
        // The property that matters, and the one the familiar constant gets
        // wrong at any angle but a right one: every point the arc passes
        // through is at the radius, not merely its ends.
        for sweep in [
            std::f32::consts::FRAC_PI_6,
            std::f32::consts::FRAC_PI_2,
            2.0,
            std::f32::consts::PI,
            -std::f32::consts::PI,
            std::f32::consts::TAU,
        ] {
            let mut builder = PathBuilder::new();
            builder.arc(Vec2::new(50.0, 50.0), Vec2::splat(40.0), 0.3, sweep);
            let path = builder.build();
            let worst = points_of(&path)
                .iter()
                .map(|p| ((*p - Vec2::new(50.0, 50.0)).length() - 40.0).abs())
                .fold(0.0f32, f32::max);
            assert!(
                worst < 0.05,
                "a sweep of {sweep} strays {worst} from a radius of forty"
            );
        }
    }

    #[test]
    fn an_arc_begins_and_ends_where_it_was_asked_to() {
        let center = Vec2::new(10.0, 20.0);
        let radii = Vec2::new(30.0, 30.0);
        let (start, sweep) = (0.5f32, 1.7f32);
        let mut builder = PathBuilder::new();
        builder.arc(center, radii, start, sweep);
        let points = points_of(&builder.build());
        let want_first = center + Vec2::new(radii.x * start.cos(), radii.y * start.sin());
        let end = start + sweep;
        let want_last = center + Vec2::new(radii.x * end.cos(), radii.y * end.sin());
        assert!((points[0] - want_first).length() < 0.01, "{:?}", points[0]);
        assert!(
            (*points.last().unwrap() - want_last).length() < 0.01,
            "{:?}",
            points.last()
        );
    }

    #[test]
    fn a_negative_sweep_travels_the_other_way() {
        let center = Vec2::ZERO;
        let arc_of = |sweep: f32| {
            let mut builder = PathBuilder::new();
            builder.arc(center, Vec2::splat(10.0), 0.0, sweep);
            points_of(&builder.build())
        };
        // A quarter turn forward passes through positive Y, backward through
        // negative Y. Same endpoints in X, opposite in Y.
        let forward = arc_of(std::f32::consts::FRAC_PI_2);
        let backward = arc_of(-std::f32::consts::FRAC_PI_2);
        assert!(forward.iter().all(|p| p.y >= -0.01), "forward dipped");
        assert!(backward.iter().all(|p| p.y <= 0.01), "backward rose");
    }

    #[test]
    fn an_elliptical_arc_follows_the_ellipse() {
        // Different radii, so a circle-shaped approximation would fail this
        // while passing every test above.
        let (center, radii) = (Vec2::new(0.0, 0.0), Vec2::new(60.0, 20.0));
        let mut builder = PathBuilder::new();
        builder.arc(center, radii, 0.0, std::f32::consts::TAU);
        let worst = points_of(&builder.build())
            .iter()
            .map(|p| {
                let (x, y) = (p.x / radii.x, p.y / radii.y);
                (x * x + y * y - 1.0).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 0.002, "off the ellipse by {worst}");
    }

    #[test]
    fn an_arc_after_a_move_is_joined_by_a_line() {
        // What makes a pie slice: the center, a line out to the arc, the arc,
        // and a close. Without the joining line the slice would be a chorded
        // segment with the center left dangling.
        let mut builder = PathBuilder::new();
        builder.move_to(Vec2::ZERO);
        builder.arc(Vec2::ZERO, Vec2::splat(10.0), 0.0, 1.0);
        builder.close();
        let path = builder.build();
        assert_eq!(path.verbs()[0], Verb::MoveTo);
        assert_eq!(
            path.verbs()[1],
            Verb::LineTo,
            "the arc did not join the current point"
        );
        assert!(points_of(&path).iter().any(|p| p.length() < 0.01));
    }

    #[test]
    fn an_arc_on_an_empty_builder_starts_with_a_move() {
        let mut builder = PathBuilder::new();
        builder.arc(Vec2::ZERO, Vec2::splat(10.0), 0.0, 1.0);
        assert_eq!(builder.build().verbs()[0], Verb::MoveTo);
    }

    #[test]
    fn a_degenerate_arc_adds_no_curve() {
        // A zero sweep still places the pen, which is what lets a caller emit
        // one unconditionally in a loop. A non-finite one does nothing at all,
        // since there is no position it describes.
        let mut builder = PathBuilder::new();
        builder.arc(Vec2::ZERO, Vec2::splat(10.0), 0.0, 0.0);
        let path = builder.build();
        assert_eq!(path.verbs(), &[Verb::MoveTo]);

        let mut builder = PathBuilder::new();
        builder.arc(Vec2::ZERO, Vec2::splat(f32::NAN), 0.0, 1.0);
        assert!(builder.build().is_empty());
    }

    #[test]
    fn a_sweep_past_a_full_turn_is_one_turn() {
        // Round and round draws the same pixels, and the extra segments are
        // cost without a picture -- but a self-overlapping path also changes
        // what a nonzero fill rule does, so this is about correctness as well.
        let count = |sweep| {
            let mut builder = PathBuilder::new();
            builder.arc(Vec2::ZERO, Vec2::splat(10.0), 0.0, sweep);
            builder.build().verbs().len()
        };
        assert_eq!(
            count(std::f32::consts::TAU),
            count(std::f32::consts::TAU * 3.0)
        );
    }
}

#[cfg(test)]
mod conic_tests {
    use super::*;

    fn points_of(path: &Path) -> Vec<Vec2> {
        crate::flatten::flatten(path, 0.01)
            .into_iter()
            .flatten()
            .collect()
    }

    #[test]
    fn a_conic_of_weight_one_is_the_quadratic_it_already_was() {
        // The identity the whole conversion rests on. If these differ, the
        // subdivision is not preserving the curve it was given.
        let mut conic = PathBuilder::new();
        conic.move_to(Vec2::new(0.0, 0.0)).conic_to(
            Vec2::new(50.0, 100.0),
            Vec2::new(100.0, 0.0),
            1.0,
        );
        let mut quad = PathBuilder::new();
        quad.move_to(Vec2::new(0.0, 0.0))
            .quad_to(Vec2::new(50.0, 100.0), Vec2::new(100.0, 0.0));

        assert_eq!(points_of(&conic.build()), points_of(&quad.build()));
    }

    #[test]
    fn a_conic_of_the_circular_weight_traces_a_circle() {
        // The reason conics exist. A quadratic cannot be a circular arc and a
        // cubic can only approximate one; a conic of weight `sqrt(2)/2`, with
        // its control point at the corner of the square the arc spans, is one
        // exactly. So every flattened point has to sit on the circle, and how
        // far any of them strays is the whole error of this conversion.
        const R: f32 = 100.0;
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(R, 0.0)).conic_to(
            Vec2::new(R, R),
            Vec2::new(0.0, R),
            std::f32::consts::FRAC_1_SQRT_2,
        );
        let points = points_of(&b.build());
        assert!(points.len() > 8, "a quarter turn should not be two lines");

        let worst = points
            .iter()
            .map(|p| (p.length() - R).abs())
            .fold(0.0f32, f32::max);
        // A hundredth of a pixel on a hundred-pixel radius, which is a part in
        // ten thousand and below what the flattening tolerance itself permits.
        assert!(
            worst < 0.05,
            "the arc strays {worst} from the circle it is supposed to be"
        );
    }

    #[test]
    fn a_weight_that_describes_no_curve_gives_the_line_between_the_ends() {
        for weight in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let mut b = PathBuilder::new();
            b.move_to(Vec2::new(0.0, 0.0)).conic_to(
                Vec2::new(50.0, 100.0),
                Vec2::new(100.0, 0.0),
                weight,
            );
            let points = points_of(&b.build());
            assert_eq!(
                points,
                vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)],
                "a weight of {weight} should give a straight line"
            );
        }
    }

    #[test]
    fn a_hyperbolic_conic_still_converges_and_stays_inside_its_hull() {
        // Weights above one pull the curve toward the control point rather
        // than away, and the subdivision has to converge from that side too. A
        // rational quadratic never leaves the triangle its three points make,
        // whatever the weight, which is what says the conversion did not
        // overshoot.
        let (a, c, e) = (
            Vec2::new(0.0, 0.0),
            Vec2::new(50.0, 100.0),
            Vec2::new(100.0, 0.0),
        );
        let mut b = PathBuilder::new();
        b.move_to(a).conic_to(c, e, 8.0);
        for p in points_of(&b.build()) {
            assert!(
                p.y >= -0.01 && p.y <= c.y + 0.01 && p.x >= -0.01 && p.x <= e.x + 0.01,
                "{p:?} is outside the triangle its own control points make"
            );
        }
    }
}

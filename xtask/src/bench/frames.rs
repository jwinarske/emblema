//! The frames the bench measures: what is drawn, how much of it, and how many
//! times.
//!
//! Split from `bench.rs` so the drift counter can watch what is *measured* without
//! flagging every change to how it is reported or tested. That counter exists
//! because a commit can move a recorded number with nothing in the tree saying so,
//! and it proved the point against itself on 2026-09-18: with `bench.rs` watched
//! whole, the next two commits were a test and the counter's own fix, and both were
//! reported as having touched what the bench times. Two false positives out of two
//! is how a printed warning becomes one nobody reads.
//!
//! So the rule for this file is narrow and worth stating. Anything here changes what
//! a recorded number means, and a change here wants a board run behind it. Anything
//! in `bench.rs` -- the clock, the report, the baseline parsing, the tests -- does
//! not.
//!
//! One thing that changes these numbers and is in neither file: **which filesystem
//! the executable is run from.** A byte-identical binary builds the stroked path in
//! 0.238 ms from ext4 and 0.283 from tmpfs, eighteen per cent, while every shorter
//! row here does not move at all. Measured on one machine by copying one file back
//! and forth and hashing it both times, after the difference had first been mistaken
//! for the effect of moving this code into its own module. Consistent with how a
//! text segment gets mapped, and the mechanism is not established from here.
//!
//! What follows from it is a rule for reading the board's numbers rather than a
//! change to them: `docs/on-a-board.md` copies the cross-built binary to `/tmp` and
//! the baseline was recorded that way, so a run from anywhere else is not comparable
//! with that file even at the same commit. The longest-running row is the one that
//! moves, which is the shape to expect if it is ever seen again.

use emblema_core::{
    Canvas, Color, GradientStop, Layer, Paint, Recording, Rect, Shader, TileMode, Vec2,
};
use emblema_hal::Extent2D;

/// The frame the document's number was taken from.
pub(super) const EXTENT: Extent2D = Extent2D {
    width: 1920,
    height: 1080,
};

/// A hundred and sixty rounded rectangles, as the document says.
pub(super) const SHAPES: usize = 160;

/// Rendered before timing starts, so pipeline creation and the first
/// allocation of every buffer are not counted as frame cost.
pub(super) const WARMUP: usize = 5;

/// Timed frames.
///
/// Two hundred rather than the thirty this began with, and the reason is the
/// percentile below. A ninety-ninth percentile of thirty samples is the
/// largest of them by another name -- `ceil(0.99 * 30)` is thirty -- so
/// reporting one would have dressed the maximum up as a distribution. Two
/// hundred puts the ninety-ninth at the third-largest, which is a tail rather
/// than an outlier, and costs about fifty milliseconds a configuration at the
/// times this actually measures.
pub(super) const FRAMES: usize = 200;

/// The percentile below is only a percentile if there are samples enough for it.
///
/// At the compiler rather than in a test, because it is a statement about a
/// constant: `ceil(0.99 * n)` is `n` for every `n` under a hundred, so a p99
/// taken from fewer would be the maximum under another name and no run would
/// say so.
const _: () = assert!(FRAMES >= 100);

/// How a frame's shapes are drawn, which is the whole question.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// A rounded rectangle stated as one, which reaches the analytic distance
    /// field: coverage from an implicit function in the fragment stage, over a
    /// quad, with the pass left at one sample.
    Analytic,
    /// The same shape stated as a path, which tessellates it. Antialiased, so
    /// the pass multisamples.
    TessellatedMultisampled,
    /// The same again with antialiasing off, which leaves the pass at one
    /// sample and is the third of the document's three figures.
    TessellatedSingleSampled,
    /// The same shape *stroked*, through the analytic field: a distance to the
    /// outline rather than to the interior, still one sample.
    ///
    /// Strokes were timed by nothing at all until this, which is how a shader
    /// change that cost two and a half per cent reached a release. They are
    /// their own pair of routes and deserve their own comparison: what an
    /// outline costs is not what a fill costs, and the tessellated one has to
    /// build two contours where a fill builds one.
    StrokedAnalytic,
    /// The stroked shape as a path, which sends it through the stroker.
    /// Antialiasing off, so this and the field above are one sample each and
    /// the difference between them is the route.
    StrokedTessellated,
}

impl Path {
    pub fn name(self) -> &'static str {
        match self {
            Self::Analytic => "distance field, 1 sample",
            Self::TessellatedMultisampled => "tessellated, 4 samples",
            Self::TessellatedSingleSampled => "tessellated, 1 sample",
            Self::StrokedAnalytic => "stroked field, 1 sample",
            Self::StrokedTessellated => "stroked path, 1 sample",
        }
    }

    /// Whether this path strokes rather than fills.
    fn strokes(self) -> bool {
        matches!(self, Self::StrokedAnalytic | Self::StrokedTessellated)
    }

    /// Whether the shape goes to the tessellator rather than the field.
    fn tessellates(self) -> bool {
        matches!(
            self,
            Self::TessellatedMultisampled
                | Self::TessellatedSingleSampled
                | Self::StrokedTessellated
        )
    }
}

/// Wide enough that the stroke is geometry rather than the thin-stroke rule.
///
/// A width under a device pixel is widened to one and dimmed to pay for it,
/// which is a different measurement and a much smaller one -- what these two
/// rows are for is what an outline costs when there is an outline. Four device
/// pixels at this frame size, on shapes a hundred and twenty across.
pub(super) const STROKE_WIDTH: f32 = 4.0;

/// Under a device pixel the width is widened to one and dimmed to pay for it,
/// which is a different measurement and a much smaller one. Held here so that
/// lowering the constant fails the build rather than quietly changing what the
/// two rows mean.
const _: () = assert!(STROKE_WIDTH >= 1.0);

/// Lay the shapes out in a grid that fills the frame.
///
/// Placed off the whole pixel deliberately: an axis-aligned rectangle at
/// integer bounds has no edge to antialias, which is the one case where the
/// field's advantage does not exist and the comparison would flatter it.
pub(super) fn shapes() -> impl Iterator<Item = Rect> {
    let columns = 16usize;
    let rows = SHAPES / columns;
    let width = EXTENT.width as f32 / columns as f32;
    let height = EXTENT.height as f32 / rows as f32;
    (0..SHAPES).map(move |i| {
        let (column, row) = (i % columns, i / columns);
        let left = column as f32 * width + 4.3;
        let top = row as f32 * height + 4.7;
        Rect::new(left, top, left + width - 8.0, top + height - 8.0)
    })
}

/// One frame's worth of drawing, recorded the way the path under test asks.
pub fn recording(path: Path) -> Recording {
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::BLACK);
    // The field route needs antialiasing -- its coverage *is* the distance, and
    // the analytic path declines a paint that asked for none -- while the
    // tessellated rows turn it off to stay at one sample. So this is which
    // route the path names rather than a per-path flag.
    let anti_alias = !matches!(
        path,
        Path::TessellatedSingleSampled | Path::StrokedTessellated
    );
    let paint = match path.strokes() {
        true => Paint::stroke(Color::WHITE, STROKE_WIDTH),
        false => Paint::fill(Color::WHITE),
    }
    .with_anti_alias(anti_alias);
    for rect in shapes() {
        let drawn = match path.tessellates() {
            // The same shape and the same paint, stated as a path so that the
            // tessellator sees it rather than the fragment stage. Comparing
            // the two forms of one shape is what makes this a measurement of
            // the path rather than of the content.
            true => canvas.draw_path(&rect.to_rounded_path(12.0), &paint),
            false => canvas.draw_rrect(rect, 12.0, &paint),
        };
        drawn.expect("a rounded rectangle");
    }
    canvas.finish()
}

/// How the full-frame row names itself.
pub(super) const FRAME: &str = "full frame, mixed content";

/// The mixed frame, built up one element at a time.
///
/// The whole frame is a budget and not a comparison, which was the point of it
/// -- but a budget nobody can attribute is a number you cannot act on. On the
/// VisionFive 2 that frame costs sixty-four milliseconds, four times a sixty
/// hertz period, over twelve draws, and nothing said which of them to look at.
///
/// So the frame is measured four times, each stage adding one element to the
/// one before, and the *difference* between two rows is what that element cost.
/// Additive rather than leave-one-out because every stage is then a frame that
/// could be drawn, and because removing something from the middle changes what
/// the things over it composite against.
///
/// The last stage is the whole frame and keeps its old name, so the row that
/// two baselines already carry stays the row it was.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// A clear and the gradient wash. Five stops, so the recorder bakes a ramp
    /// and the shader samples it -- the one thing here that tabulates one.
    Ground,
    /// The three rounded rectangles over it, and nothing else: the fill route a
    /// card takes when it casts no shadow.
    Cards,
    /// Each card's shadow under it. The delta is what a blurred occluder costs,
    /// which is the analytic rounded-rectangle blur rather than a layer.
    Shadows,
    /// The blurred highlight over everything, which is the only stage that opens
    /// a layer: its own target, two blur passes and a composite back.
    All,
}

impl Frame {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ground => "frame, gradient ground",
            Self::Cards => "frame, plus cards",
            Self::Shadows => "frame, plus shadows",
            Self::All => FRAME,
        }
    }

    /// Whether this stage draws the cards.
    fn cards(self) -> bool {
        !matches!(self, Self::Ground)
    }

    /// Whether it draws their shadows under them.
    fn shadows(self) -> bool {
        matches!(self, Self::Shadows | Self::All)
    }

    /// Whether it draws the blurred layer over them.
    fn highlight(self) -> bool {
        matches!(self, Self::All)
    }
}

/// Every stage, in the order they build on each other.
pub(super) const FRAME_STAGES: [Frame; 4] =
    [Frame::Ground, Frame::Cards, Frame::Shadows, Frame::All];

/// How many concave polygons the concave rows draw, and how many points each has.
///
/// Laid out on the same sixteen-by-ten grid `shapes()` uses, so the two concave
/// rows cover the same area as each other and as the comparison rows above. The
/// point counts differ by six times and the area does not, which is the whole
/// design: `non-parity.md` 18 claims a concave fill's cost here scales with its
/// vertex count on the CPU and with its area on the GPU, and two rows that hold
/// one of those fixed while multiplying the other is what tests it.
///
/// Twelve and seventy-two rather than rounder numbers because a star needs an even
/// count -- a point and a notch per pair -- and because seventy-two across a
/// hundred and sixty shapes is 11,520 points, within sight of the 13,795 in the
/// Berlin tile that entry measured, so the figures are comparable to the ones
/// already recorded there.
pub(super) const CONCAVE_POINTS: [usize; 2] = [12, 72];

/// A star polygon inscribed in `bounds`, with `points` vertices.
///
/// Concave by construction, which is the property that matters: a convex path goes
/// to a fan and never reaches the general triangulator, so a convex scene could not
/// show what triangulation costs. Alternating radii put every other vertex at forty
/// per cent of the way out, which is a notch deep enough that no two adjacent edges
/// are collinear at any count this uses.
///
/// Straight edges only, no curves. That matches the vector-tile geometry entry 18
/// measured against, and the entry notes why it matters: flattening a curve is CPU
/// work too, so a curve-heavy path would shift the share away from triangulation and
/// make the comparison say something else.
fn star(bounds: Rect, points: usize) -> emblema_core::Path {
    let center = Vec2::new(
        (bounds.left + bounds.right) * 0.5,
        (bounds.top + bounds.bottom) * 0.5,
    );
    let outer = Vec2::new(
        (bounds.right - bounds.left) * 0.5,
        (bounds.bottom - bounds.top) * 0.5,
    );
    let mut builder = emblema_core::Path::builder();
    for i in 0..points {
        let t = i as f32 / points as f32 * std::f32::consts::TAU;
        let reach = if i % 2 == 0 { 1.0 } else { 0.4 };
        let at = Vec2::new(
            center.x + outer.x * reach * t.cos(),
            center.y + outer.y * reach * t.sin(),
        );
        if i == 0 {
            builder.move_to(at);
        } else {
            builder.line_to(at);
        }
    }
    builder.close();
    builder.build()
}

/// How a concave row names itself.
pub(super) fn concave_name(points: usize) -> &'static str {
    match points {
        12 => "concave, 12 points",
        72 => "concave, 72 points",
        _ => "concave",
    }
}

/// A hundred and sixty concave polygons, each with `points` vertices.
///
/// The scene `non-parity.md` 18 was missing. That entry is the deepest divergence in
/// the file -- upstream resolves a filled path's winding in the stencil buffer and
/// this renderer triangulates it on the CPU -- and its cost was measured on a Berlin
/// vector tile rather than on anything this bench draws, because nothing this bench
/// draws reaches the general triangulator at all. The comparison rows are rounded
/// rectangles, which `fill` sends to a fan; the two frames are rectangles, gradients
/// and blurs. A concave fill had no row.
///
/// Aliased, and the reason is the same one the tessellated comparison rows give: the
/// analytic route declines a paint that asked for no antialiasing, so turning it off
/// is what keeps this scene on the triangulator rather than quietly measuring a
/// distance field. It also keeps the pass at one sample, so the GPU side is fill and
/// not a resolve.
pub(super) fn concave(points: usize) -> Recording {
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::BLACK);
    let paint = Paint::fill(Color::srgb(0.28, 0.52, 0.86, 1.0)).with_anti_alias(false);
    for bounds in shapes() {
        canvas
            .draw_path(&star(bounds, points), &paint)
            .expect("a concave fill");
    }
    canvas.finish()
}

/// How the stacked-interface row names itself.
pub(super) const STACKED: &str = "stacked interface";

/// An interface's shape rather than a sampler of operations, and the one frame
/// here with overdraw in it.
///
/// `FRAME` is a mixed frame: it reaches a ramp, a blur, a layer and an analytic
/// rounded rectangle, which is what makes it a budget worth quoting. What it is
/// not is an interface. Its ground is a full-screen gradient that almost nothing
/// covers, so 94 per cent of what it paints it also shows, and two recorded
/// differences from upstream turned out to be unmeasurable against it for that
/// reason -- `non-parity.md` 19, which wants an axis-aligned gradient, and 21,
/// which wants opaque content stacked over opaque content.
///
/// This frame is those two cases. A top bar, a sidebar and a content panel cover
/// 94 per cent of the wash beneath them, eight list rows cover most of the panel
/// again, and the whole frame paints 2.53 times its own area -- so a little over
/// one and a half frames of fill is spent on pixels that are never seen. That is
/// what an interface does, and none of it was measurable here before.
///
/// The wash is vertical and its endpoints are the covered rectangle's top and
/// bottom edges, which is what upstream's `CanApplyFastGradient` requires. The
/// existing frame's ground is diagonal and would not qualify.
///
/// **Added beside `FRAME` rather than replacing it.** Every timing in
/// `on-a-board.md`, both ratio tables in `architecture.md` and both baselines'
/// histories were measured against that frame, so changing it would silently
/// change what a year of recorded numbers meant. A new frame costs a re-recording
/// on each board and costs nothing else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stacked {
    /// The wash alone: one axis-aligned gradient over the whole target, and the
    /// only stage here that draws no opaque content.
    Wash,
    /// Plus the bar, the sidebar and the content panel -- 94 per cent of the wash
    /// covered by three opaque rectangles.
    Panels,
    /// Plus the list rows inside the panel, which is a second layer of opaque
    /// over opaque and what takes the painted area past twice the frame.
    All,
}

impl Stacked {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wash => "stacked, wash",
            Self::Panels => "stacked, plus panels",
            Self::All => STACKED,
        }
    }

    fn panels(self) -> bool {
        !matches!(self, Self::Wash)
    }

    fn rows(self) -> bool {
        matches!(self, Self::All)
    }
}

/// The stacked frame's geometry, at module scope so a test can recompute the
/// coverage the documentation above claims rather than take it on trust. The
/// numbers in that prose are the whole reason the scene exists, and prose is where
/// this tree's mistakes live.
pub(super) const BAR_H: f32 = 80.0;
pub(super) const SIDE_W: f32 = 320.0;
pub(super) const INSET: f32 = 24.0;
pub(super) const GAP: f32 = 16.0;
pub(super) const ROWS: usize = 8;

/// Every stage of the stacked frame, in the order they build on each other.
pub(super) const STACKED_STAGES: [Stacked; 3] = [Stacked::Wash, Stacked::Panels, Stacked::All];

/// One stage of the stacked interface. `Stacked::All` is the whole of it.
///
/// Solid fills throughout, deliberately. The question this frame asks is what
/// covering a pixel repeatedly costs, so every occluder is opaque and as plain as
/// a draw can be -- an occluder that carried a gradient or a blur would answer a
/// different question and answer it less clearly.
pub(super) fn stacked(stage: Stacked) -> Recording {
    let (w, h) = (EXTENT.width as f32, EXTENT.height as f32);
    let (bar_h, side_w, inset) = (BAR_H, SIDE_W, INSET);
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::srgb(0.04, 0.05, 0.07, 1.0));

    // A vertical wash over the whole target. Axis-aligned with its endpoints on
    // the covered rectangle's edges, which is the case `non-parity.md` 19 is
    // about and the case nothing here had.
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, w, h),
            &Paint::default().with_shader(Shader::LinearGradient {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(0.0, h),
                stops: (0..5)
                    .map(|i| {
                        let t = i as f32 / 4.0;
                        GradientStop {
                            offset: t,
                            color: Color::srgb(0.10 + 0.06 * t, 0.11, 0.20 - 0.04 * t, 1.0),
                        }
                    })
                    .collect(),
                tile: TileMode::Clamp,
            }),
        )
        .expect("the wash");

    if !stage.panels() {
        return canvas.finish();
    }

    // The bar, the sidebar and the content panel, in the order an interface
    // composites them: each is opaque and each hides what it covers.
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, w, bar_h),
            &Paint::fill(Color::srgb(0.14, 0.15, 0.22, 1.0)),
        )
        .expect("the bar");
    canvas
        .draw_rect(
            Rect::new(0.0, bar_h, side_w, h),
            &Paint::fill(Color::srgb(0.11, 0.12, 0.18, 1.0)),
        )
        .expect("the sidebar");
    let content = Rect::new(side_w + inset, bar_h + inset, w - inset, h - inset);
    canvas
        .draw_rect(content, &Paint::fill(Color::srgb(0.16, 0.17, 0.24, 1.0)))
        .expect("the content panel");

    if !stage.rows() {
        return canvas.finish();
    }

    // Eight rows inside the panel, each opaque over the panel that is itself
    // opaque over the wash. This is the stage that takes the painted area past
    // twice the frame.
    let gap = GAP;
    let row_h = (content.height() - gap * (ROWS as f32 + 1.0)) / ROWS as f32;
    for i in 0..ROWS {
        let top = content.top + gap * (i as f32 + 1.0) + row_h * i as f32;
        let shade = 0.19 + 0.01 * (i % 2) as f32;
        canvas
            .draw_rect(
                Rect::new(content.left + gap, top, content.right - gap, top + row_h),
                &Paint::fill(Color::srgb(shade, shade + 0.01, shade + 0.07, 1.0)),
            )
            .expect("a list row");
    }

    canvas.finish()
}

/// One stage of the mixed frame; `Frame::All` is the whole of it, and what a
/// whole frame of mixed content costs is a budget rather than a comparison.
///
/// The three routes above answer one narrow question and answer it well: the
/// same shapes, twice, so the difference is the route. That is not a frame. It
/// has one material, no layer, no blur and no gradient, so it says nothing
/// about what an interface costs — and a renderer can be quick at a hundred
/// and sixty identical rectangles and slow at everything a real frame is made
/// of.
///
/// So this is the other kind: a ground that is a gradient, cards that carry
/// shadows, and a blurred layer over the top, at the size a display actually
/// is. Every one of those reaches machinery the comparison never touches — the
/// ramp, the blur's passes, the layer's own target and its composite back.
///
/// Deliberately *not* the panel example's frame, which it otherwise resembles.
/// That one turns antialiasing off on its ground because `execute_deferred`
/// cannot submit a multisampled pass, which is a constraint of presenting to a
/// display and not of drawing. A frame written to be timed should look like a
/// frame, so this leaves it on.
///
/// Static, at one instant of that scene rather than a moving one: a benchmark
/// that changed its own content between runs would report the content.
pub(super) fn frame(stage: Frame) -> Recording {
    let (w, h) = (EXTENT.width as f32, EXTENT.height as f32);
    let mut canvas = Canvas::new(EXTENT);
    canvas.clear(Color::srgb(0.05, 0.06, 0.09, 1.0));

    // A wash behind everything. A gradient rather than a flat fill because it
    // is the one thing here that tabulates a ramp.
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, w, h),
            &Paint::default().with_shader(Shader::LinearGradient {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(w, h),
                // Five, and the count is the point rather than the picture:
                // at or below `MAX_STOPS` a gradient travels inside the
                // material and tabulates nothing. Past it the recorder bakes a
                // ramp and the shader samples it, which is the path a frame
                // should be timed on and the one two stops would miss.
                stops: (0..5)
                    .map(|i| {
                        let t = i as f32 / 4.0;
                        GradientStop {
                            offset: t,
                            color: Color::srgb(0.08 + 0.14 * t, 0.10, 0.18 + 0.02 * t, 1.0),
                        }
                    })
                    .collect(),
                tile: TileMode::Clamp,
            }),
        )
        .expect("the ground");

    let center = Vec2::new(w * 0.5, h * 0.5);
    let orbit = w.min(h) * 0.26;
    let side = w.min(h) * 0.20;
    let hues: &[Color] = if stage.cards() {
        &[
            Color::srgb(0.98, 0.42, 0.28, 1.0),
            Color::srgb(0.36, 0.82, 0.62, 1.0),
            Color::srgb(0.42, 0.58, 0.98, 1.0),
        ]
    } else {
        &[]
    };
    for (i, hue) in hues.iter().copied().enumerate() {
        let phase = i as f32 * std::f32::consts::TAU / 3.0;
        let at = Vec2::new(
            center.x + orbit * phase.cos(),
            center.y + orbit * phase.sin() * 0.55,
        );
        let card = Rect::new(
            at.x - side * 0.5,
            at.y - side * 0.5,
            at.x + side * 0.5,
            at.y + side * 0.5,
        );
        // Shadow first and card over it, which is the order every real caller
        // uses and the one the occluder flag describes.
        if stage.shadows() {
            canvas
                .draw_shadow(&card.to_rounded_path(side * 0.18), Color::BLACK, 8.0, false)
                .expect("a shadow");
        }
        canvas
            .draw_rrect(card, side * 0.18, &Paint::fill(hue))
            .expect("a card");
    }

    // A blurred highlight, so the layer's own target and its composite back are
    // in the number too.
    if stage.highlight() {
        canvas.save_layer(Layer::opacity(0.5).with_blur(24.0));
        canvas
            .draw_circle(
                center,
                w.min(h) * 0.10,
                &Paint::fill(Color::srgb(1.0, 0.95, 0.80, 1.0)),
            )
            .expect("a highlight");
        canvas.restore();
    }

    canvas.finish()
}

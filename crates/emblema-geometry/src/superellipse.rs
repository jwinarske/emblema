//! Flutter's rounded superellipse -- the iOS-style squircle.
//!
//! A corner here is not one curve. It is a superellipse arc that leaves the
//! flat side, a circular arc through the diagonal, and a second superellipse
//! arc back onto the next side, joined so the tangents agree. The
//! superellipse's degree is not a constant either: it is read from a fitted
//! table on the ratio of side to corner radius, so a shallow corner and a deep
//! one are different curves rather than the same curve scaled.
//!
//! **None of that is derivable.** The table was fitted upstream by search, and
//! the only thing that makes it safe to transcribe is that upstream also
//! publishes points on the resulting boundary. Those are transcribed beside it
//! in the tests, which is what turns "these twenty-two numbers were copied
//! correctly" from an assertion into a check: a wrong degree or a misplaced
//! join moves the joint points off the boundary by more than the two
//! hundredths of a unit the assertions allow.
//!
//! Read [`docs/parity.md`] on why this went unbuilt for so long. The short
//! version is that the reason recorded there -- that nothing here could check
//! a transcription -- was wrong, and checking it was the whole job.
//!
//! [`docs/parity.md`]: https://github.com/jwinarske/emblema/blob/main/docs/parity.md

use crate::path::{Path, PathBuilder, Rect};
use glam::Vec2;

/// Upstream's epsilon, and used here everywhere it is used there.
const CLOSE_ENOUGH: f32 = 1e-3;

/// `1 - cos(pi/4)`, the fraction of the radius by which the circular arc's
/// far point is drawn in from the corner of the bounding square.
const GAP_FACTOR: f32 = 0.292_893_22;

/// Fitted `(n, k_xJ)` against the ratio of side to radius.
///
/// `n` is the superellipse's degree and `k_xJ` is `1 / (1 - xJ/a)`, where `xJ`
/// is where the superellipse arc hands over to the circular one. Both were
/// found by search upstream; neither has a closed form here or there.
///
/// The rows are not evenly spaced -- ratios 2.00 to 2.50 in steps of 0.10,
/// then 2.50 to 5.00 in steps of 0.50 -- so the two zones interpolate on
/// different strides and past 5.00 the fit becomes a straight line.
const CORNER_TABLE: [[f32; 2]; 11] = [
    /* ratio 2.00 */ [2.000_000_0, 1.132_766_8],
    /* ratio 2.10 */ [2.183_498, 1.203_119_2],
    /* ratio 2.20 */ [2.338_886_6, 1.286_988],
    /* ratio 2.30 */ [2.486_606, 1.363_519_4],
    /* ratio 2.40 */ [2.622_266, 1.447_179_8],
    /* ratio 2.50 */ [2.751_489_9, 1.533_858_2],
    /* ratio 3.00 */ [3.362_982_7, 1.982_882_8],
    /* ratio 3.50 */ [4.086_499, 2.238_118_5],
    /* ratio 4.00 */ [4.854_811, 2.475_634_6],
    /* ratio 4.50 */ [5.629_455_5, 2.729_486],
    /* ratio 5.00 */ [6.430_238, 2.980_204],
];

const MIN_RATIO: f32 = 2.00;
const FIRST_STEP_INVERSE: f32 = 10.0;
const FIRST_MAX_RATIO: f32 = 2.50;
const FIRST_NUM_RECORDS: f32 = 6.0;
const SECOND_STEP_INVERSE: f32 = 2.0;
const SECOND_MAX_RATIO: f32 = 5.00;
const THIRD_N_SLOPE: f32 = 1.559_599_4;
const THIRD_KXJ_SLOPE: f32 = 0.522_807_2;

/// Normalized conic weights against the degree `n`, for approximating the
/// superellipse arc with two conics.
///
/// These affect the drawn outline and not [`RoundSuperellipse::contains`],
/// which stays analytic -- so the boundary points transcribed from upstream
/// check the table above and say nothing about this one. What checks this one
/// is that the outline it produces has to agree with `contains`.
const CONIC_TABLE: [[f32; 2]; 13] = [
    /* n =  2.0 */ [0.7078, 8.3194],
    /* n =  3.0 */ [0.7895, 2.4523],
    /* n =  4.0 */ [0.8379, 1.8528],
    /* n =  5.0 */ [0.8701, 1.6891],
    /* n =  6.0 */ [0.8932, 1.5806],
    /* n =  7.0 */ [0.9107, 1.5043],
    /* n =  8.0 */ [0.9244, 1.4470],
    /* n =  9.0 */ [0.9355, 1.4037],
    /* n = 10.0 */ [0.9448, 1.3701],
    /* n = 11.0 */ [0.9526, 1.3431],
    /* n = 12.0 */ [0.9594, 1.3212],
    /* n = 13.0 */ [0.9653, 1.3032],
    /* n = 14.0 */ [0.9705, 1.2880],
];

/// The corner radii of a rounded superellipse, one `Vec2` per corner.
///
/// Named apart from `emblema_core::RoundingRadii`, which is the same four
/// numbers as a bare array. This crate does not depend on that one, and a
/// struct is what keeps `top_left` from being written where `top_right` was
/// meant -- which in a shape whose corners may all differ is a mistake that
/// draws something plausible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CornerRadii {
    pub top_left: Vec2,
    pub top_right: Vec2,
    pub bottom_left: Vec2,
    pub bottom_right: Vec2,
}

impl CornerRadii {
    /// The same radius at every corner.
    pub fn uniform(radius: Vec2) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            bottom_left: radius,
            bottom_right: radius,
        }
    }

    fn all_same(&self) -> bool {
        self.top_left == self.top_right
            && self.top_left == self.bottom_left
            && self.top_left == self.bottom_right
    }
}

/// One eighth of the outline, in the space of a corner with equal radii.
#[derive(Debug, Clone, Copy)]
struct Octant {
    offset: Vec2,
    /// The superellipse's half-extent and degree. A degree below two marks a
    /// corner sharp enough to be treated as square, which the ratio would
    /// otherwise drive to infinity.
    se_a: f32,
    se_n: f32,
    circle_start: Vec2,
    circle_center: Vec2,
    circle_max_angle: f32,
}

/// One quarter of the outline: two octants that meet on the diagonal.
#[derive(Debug, Clone, Copy)]
struct Quadrant {
    offset: Vec2,
    signed_scale: Vec2,
    top: Octant,
    right: Octant,
}

/// A rounded superellipse: a rectangle whose corners are Flutter's squircle.
#[derive(Debug, Clone, Copy)]
pub struct RoundSuperellipse {
    top_right: Quadrant,
    bottom_right: Quadrant,
    bottom_left: Quadrant,
    top_left: Quadrant,
    /// Set when one quadrant describes all four, which is both a shortcut and
    /// a different containment rule -- a shape with four equal corners is
    /// tested by mirroring the point rather than by asking each quadrant.
    all_corners_same: bool,
}

/// Interpolate the degree `n` and `xJ/a` for a ratio of side to radius.
fn n_and_xj(ratio: f32) -> (f32, f32) {
    if ratio > SECOND_MAX_RATIO {
        // Past the table the fit is linear, which is why the last row and a
        // slope are enough.
        let last = CORNER_TABLE[CORNER_TABLE.len() - 1];
        let n = THIRD_N_SLOPE * (ratio - SECOND_MAX_RATIO) + last[0];
        let k_xj = THIRD_KXJ_SLOPE * (ratio - SECOND_MAX_RATIO) + last[1];
        return (n, 1.0 - 1.0 / k_xj);
    }
    let ratio = ratio.clamp(MIN_RATIO, SECOND_MAX_RATIO);
    let steps = if ratio < FIRST_MAX_RATIO {
        (ratio - MIN_RATIO) * FIRST_STEP_INVERSE
    } else {
        (ratio - FIRST_MAX_RATIO) * SECOND_STEP_INVERSE + FIRST_NUM_RECORDS - 1.0
    };

    let left = (steps.floor() as usize).min(CORNER_TABLE.len() - 2);
    let frac = steps - left as f32;

    let n = (1.0 - frac) * CORNER_TABLE[left][0] + frac * CORNER_TABLE[left + 1][0];
    let k_xj = (1.0 - frac) * CORNER_TABLE[left][1] + frac * CORNER_TABLE[left + 1][1];
    (n, 1.0 - 1.0 / k_xj)
}

/// The center of the circle of radius `r` through both `a` and `b`.
fn circle_center_through(a: Vec2, b: Vec2, r: f32) -> Vec2 {
    let a_to_b = b - a;
    let m = (a + b) / 2.0;
    let c_to_m = Vec2::new(-a_to_b.y, a_to_b.x);
    let distance_am = a_to_b.length() / 2.0;
    let distance_cm = (r * r - distance_am * distance_am).sqrt();
    m - distance_cm * c_to_m.normalize()
}

/// The signed angle from `from` to `to`, as `AngleTo` computes it upstream.
fn angle_between(from: Vec2, to: Vec2) -> f32 {
    to.y.atan2(to.x) - from.y.atan2(from.x)
}

fn compute_octant(center: Vec2, a: f32, radius: f32) -> Octant {
    if radius <= CLOSE_ENOUGH {
        // A corner this sharp is a square one. Left alone it would drive the
        // ratio arbitrarily high and the degree with it, which is how NaNs get
        // into the outline.
        return Octant {
            offset: center,
            se_a: a,
            se_n: 0.0,
            circle_start: Vec2::new(a, a),
            circle_center: Vec2::ZERO,
            circle_max_angle: 0.0,
        };
    }

    let ratio = a * 2.0 / radius;
    let g = GAP_FACTOR * radius;

    let (n, xj_over_a) = n_and_xj(ratio);
    let xj = xj_over_a * a;
    let yj = (1.0 - xj_over_a.powf(n)).powf(1.0 / n) * a;

    // Where the superellipse hands over to the circle, the two must share a
    // tangent, and that is what fixes the circle's radius rather than a
    // choice.
    let tan_phij = (xj / yj).powf(n - 1.0);
    let d = (xj - tan_phij * yj) / (1.0 - tan_phij);
    let r = (a - d - g) * std::f32::consts::SQRT_2;

    let point_m = Vec2::new(a - g, a - g);
    let point_j = Vec2::new(xj, yj);
    let circle_center = circle_center_through(point_j, point_m, r);
    let circle_max_angle = angle_between(point_m - circle_center, point_j - circle_center);

    Octant {
        offset: center,
        se_a: a,
        se_n: n,
        circle_start: point_j,
        circle_center,
        circle_max_angle,
    }
}

fn compute_quadrant(center: Vec2, corner: Vec2, in_radii: Vec2, sign: Vec2) -> Quadrant {
    let corner_vector = corner - center;
    let radii = Vec2::new(
        in_radii.x.abs().min(corner_vector.x.abs()),
        in_radii.y.abs().min(corner_vector.y.abs()),
    );

    // An elliptical corner is a circular one that has been stretched, so the
    // whole quadrant is computed round and then scaled back. Every guard here
    // is against a zero in the radii or the corner vector, either of which
    // turns the scale into a NaN.
    let norm_radius = radii.x.min(radii.y);
    let forward_scale = if norm_radius == 0.0 {
        Vec2::ONE
    } else {
        radii / norm_radius
    };
    let norm_half_size = corner_vector.abs() / forward_scale;
    let raw = corner_vector / norm_half_size;
    let signed_scale = Vec2::new(
        if raw.x.is_nan() { sign.x } else { raw.x },
        if raw.y.is_nan() { sign.y } else { raw.y },
    );

    // The two octants belong to two different square-like superellipses, and
    // they only meet cleanly on the diagonal if those two are offset from the
    // quadrant's center by the same distance in opposite directions.
    let c = norm_half_size.x - norm_half_size.y;

    Quadrant {
        offset: center,
        signed_scale,
        top: compute_octant(Vec2::new(0.0, -c), norm_half_size.x, norm_radius),
        right: compute_octant(Vec2::new(c, 0.0), norm_half_size.y, norm_radius),
    }
}

/// Where two adjacent corners divide the side they share, in proportion to
/// their radii.
fn split(low: f32, high: f32, ratio_low: f32, ratio_high: f32) -> f32 {
    if ratio_low == 0.0 && ratio_high == 0.0 {
        return (low + high) / 2.0;
    }
    (low * ratio_high + high * ratio_low) / (ratio_low + ratio_high)
}

/// Whether `p` is inside the first octant's arc, answering `true` for any
/// point outside that octant so the callers can take an `&&` of all of them.
fn octant_contains(param: &Octant, p: Vec2) -> bool {
    if p.x < 0.0 || p.y < 0.0 || p.y < p.x {
        return true;
    }
    if p.x <= param.circle_start.x {
        let p_se = p / param.se_a;
        return p_se.x.powf(param.se_n) + p_se.y.powf(param.se_n) <= 1.0;
    }
    let circle_radius = param.circle_start.distance_squared(param.circle_center);
    (p - param.circle_center).length_squared() < circle_radius
}

fn corner_contains(param: &Quadrant, p: Vec2, check_quadrant: bool) -> bool {
    let mut norm_point = (p - param.offset) / param.signed_scale;
    if check_quadrant {
        if norm_point.x < 0.0 || norm_point.y < 0.0 {
            return true;
        }
    } else {
        norm_point = norm_point.abs();
    }
    if param.top.se_n < 2.0 || param.right.se_n < 2.0 {
        // A square corner, which owns its top and left borders but not its
        // bottom and right ones -- the same half-open rule a plain rectangle
        // follows, so that two shapes sharing an edge do not both claim it.
        let x_delta = param.right.offset.x + param.right.se_a - norm_point.x;
        let y_delta = param.top.offset.y + param.top.se_a - norm_point.y;
        let x_within = x_delta > 0.0 || (x_delta == 0.0 && param.signed_scale.x < 0.0);
        let y_within = y_delta > 0.0 || (y_delta == 0.0 && param.signed_scale.y < 0.0);
        return x_within && y_within;
    }
    let from_top = norm_point - param.top.offset;
    let from_right = norm_point - param.right.offset;
    octant_contains(&param.top, from_top)
        && octant_contains(&param.right, Vec2::new(from_right.y, from_right.x))
}

/// Where the two conics meet and what weights they carry, for one octant.
struct SuperellipseConics {
    /// `A`, on the flat side, then the control point and the split point `H`.
    c1: Vec2,
    h: Vec2,
    weight1: f32,
    /// The control point for `H` to `J`, where the circular arc takes over.
    c2: Vec2,
    j: Vec2,
    weight2: f32,
    a: Vec2,
}

/// The conic weights and the split point for a superellipse arc of degree `n`.
///
/// Transcribed exactly, including a formula that looks wrong, is wrong, and
/// still produces the better outline.
///
/// `sqrt(n)` and `xJOverA` multiply only the right-hand term of each
/// interpolation, so the weight climbs across each interval and then drops
/// back to the raw table value at the next whole degree. It is a sawtooth:
/// `weight1` is 1.359 just below `n = 3` and 0.790 at it, a forty percent step
/// repeated at all twelve of the table's boundaries. Upstream's own comment
/// says the table holds normalized weights and that `weight1 = factor1 *
/// sqrt(n)`, which the code does only when `frac` reaches 1.
///
/// I read that as a bug and implemented the consistent reading. Sweeping the
/// side-to-radius ratio from 2 to 12 and measuring both against
/// [`RoundSuperellipse::contains`] says the consistent reading is worse
/// everywhere: its worst is 0.056 against 0.046, and past a ratio of 5 it sits
/// at 0.017 to 0.036 where this sits at 0.003 to 0.023. The fitted factors
/// evidently absorb the formula they were searched against, so "fixing" the
/// formula without refitting the table makes the shape worse.
///
/// The sawtooth is still visible in the output, and is recorded in
/// `docs/non-parity.md` as an upstream artifact carried deliberately. Across
/// the crossing at `n = 3`, a ratio of 2.700 lands 0.005 from the true curve
/// and 2.705 lands 0.042 -- a ninefold jump for two tenths of a percent of
/// corner radius, where the shape itself is continuous. Matching it is
/// parity; smoothing it here would put this renderer's squircle somewhere
/// Flutter's is not.
fn superellipse_conic_factors(n: f32, xj_over_a: f32) -> (f32, f32, f32) {
    const STEP: f32 = 1.0;
    const MIN_N: f32 = 2.0;
    let max_n = MIN_N + (CONIC_TABLE.len() - 1) as f32 * STEP;
    let n = if n >= max_n { max_n } else { n };

    let steps = ((n - MIN_N) / STEP).clamp(0.0, (CONIC_TABLE.len() - 1) as f32);
    let left = (steps.floor() as usize).min(CONIC_TABLE.len() - 2);
    let frac = steps - left as f32;

    let weight1 = (1.0 - frac) * CONIC_TABLE[left][0] + frac * CONIC_TABLE[left + 1][0] * n.sqrt();
    let weight2 = (1.0 - frac) * CONIC_TABLE[left][1] + frac * CONIC_TABLE[left + 1][1] * xj_over_a;

    // `H` sits between `A` and `J` in proportion to `sqrt(n)`, so a higher
    // degree moves it toward `A`. The flat stretch of a high-degree
    // superellipse is the part a conic approximates worst, so it gets the
    // shorter of the two spans. The proportion is upstream's and empirical.
    let yh_proportion = n.sqrt();
    (weight1, weight2, yh_proportion)
}

/// The intersection of two lines given a point and a slope on each.
fn intersection(p1: Vec2, k1: f32, p2: Vec2, k2: f32) -> Vec2 {
    if (k1 - k2).abs() < CLOSE_ENOUGH {
        return (p1 + p2) / 2.0;
    }
    let x = (k1 * p1.x - k2 * p2.x + p2.y - p1.y) / (k1 - k2);
    Vec2::new(x, k1 * (x - p1.x) + p1.y)
}

fn superellipse_conics(param: &Octant, yj_over_a: f32) -> SuperellipseConics {
    let a_point = Vec2::new(0.0, param.se_a);
    let j = param.circle_start;
    let (weight1, weight2, yh_proportion) =
        superellipse_conic_factors(param.se_n, j.x / param.se_a);
    // `H` is picked by its height, proportionally between A's and J's, so it
    // needs `yj_over_a` and cannot come from the degree alone.
    let yh_over_a = (1.0 * yh_proportion + yj_over_a) / (yh_proportion + 1.0);

    let h = Vec2::new(
        (1.0 - yh_over_a.powf(param.se_n)).powf(1.0 / param.se_n) * param.se_a,
        yh_over_a * param.se_a,
    );

    // The control points are where the tangents meet, which is what makes the
    // joins smooth rather than merely continuous.
    let k_a = 0.0;
    let k_j = -(j.x / j.y).powf(param.se_n - 1.0);
    let k_h = -(h.x / h.y).powf(param.se_n - 1.0);

    SuperellipseConics {
        a: a_point,
        c1: intersection(a_point, k_a, h, k_h),
        h,
        weight1,
        c2: intersection(h, k_h, j, k_j),
        j,
        weight2,
    }
}

/// The circular arc as a cubic: start, two controls, end.
fn circular_arc_points(param: &Octant) -> [Vec2; 4] {
    let start_vector = param.circle_start - param.circle_center;
    let (sin, cos) = (-param.circle_max_angle).sin_cos();
    let end_vector = Vec2::new(
        start_vector.x * cos - start_vector.y * sin,
        start_vector.x * sin + start_vector.y * cos,
    );
    let circle_end = param.circle_center + end_vector;
    let start_tangent = Vec2::new(start_vector.y, -start_vector.x).normalize();
    let end_tangent = Vec2::new(-end_vector.y, end_vector.x).normalize();
    let bezier_factor = (param.circle_max_angle / 4.0).tan() * 4.0 / 3.0;
    let radius = start_vector.length();

    [
        param.circle_start,
        param.circle_start + start_tangent * bezier_factor * radius,
        circle_end + end_tangent * bezier_factor * radius,
        circle_end,
    ]
}

/// The map from an octant's own space out to the shape's.
#[derive(Clone, Copy)]
struct Placement {
    offset: Vec2,
    scale: Vec2,
    octant_offset: Vec2,
    flip: bool,
}

impl Placement {
    fn apply(&self, p: Vec2) -> Vec2 {
        let p = if self.flip { Vec2::new(p.y, p.x) } else { p };
        self.offset + self.scale * (self.octant_offset + p)
    }
}

impl RoundSuperellipse {
    /// A rounded superellipse in `bounds` with the given corner radii.
    pub fn new(bounds: Rect, radii: CornerRadii) -> Self {
        let center = (bounds.min + bounds.max) / 2.0;
        let right_top = Vec2::new(bounds.max.x, bounds.min.y);

        if radii.all_same() && radii.top_left.x != 0.0 && radii.top_left.y != 0.0 {
            // Four equal corners describe the whole shape from one quadrant.
            // Four *empty* ones do not, because a plain rectangle needs the
            // half-open border rule that the square-corner branch applies.
            let q = compute_quadrant(center, right_top, radii.top_right, Vec2::new(-1.0, 1.0));
            return Self {
                top_right: q,
                bottom_right: q,
                bottom_left: q,
                top_left: q,
                all_corners_same: true,
            };
        }

        let top_split = split(
            bounds.min.x,
            bounds.max.x,
            radii.top_left.x,
            radii.top_right.x,
        );
        let right_split = split(
            bounds.min.y,
            bounds.max.y,
            radii.top_right.y,
            radii.bottom_right.y,
        );
        let bottom_split = split(
            bounds.min.x,
            bounds.max.x,
            radii.bottom_left.x,
            radii.bottom_right.x,
        );
        let left_split = split(
            bounds.min.y,
            bounds.max.y,
            radii.top_left.y,
            radii.bottom_left.y,
        );

        Self {
            top_right: compute_quadrant(
                Vec2::new(top_split, right_split),
                right_top,
                radii.top_right,
                Vec2::new(1.0, -1.0),
            ),
            bottom_right: compute_quadrant(
                Vec2::new(bottom_split, right_split),
                bounds.max,
                radii.bottom_right,
                Vec2::new(1.0, 1.0),
            ),
            bottom_left: compute_quadrant(
                Vec2::new(bottom_split, left_split),
                Vec2::new(bounds.min.x, bounds.max.y),
                radii.bottom_left,
                Vec2::new(-1.0, 1.0),
            ),
            top_left: compute_quadrant(
                Vec2::new(top_split, left_split),
                bounds.min,
                radii.top_left,
                Vec2::new(-1.0, -1.0),
            ),
            all_corners_same: false,
        }
    }

    /// A rounded superellipse with one radius at every corner.
    pub fn with_radius(bounds: Rect, radius: f32) -> Self {
        Self::new(bounds, CornerRadii::uniform(Vec2::splat(radius)))
    }

    /// The outline, as a closed path of conics and cubics.
    ///
    /// Each quadrant is two octants and each octant is two conics for the
    /// superellipse arc and one cubic for the circular one. A shape whose four
    /// corners match is drawn from a single quadrant reflected four ways,
    /// which is why `reverse` exists: two of those reflections traverse the
    /// corner the other way round, and a path has to stay in one direction.
    pub fn to_path(&self) -> Path {
        let mut builder = PathBuilder::new();
        self.add_to(&mut builder);
        builder.build()
    }

    /// Append the outline to a builder that may already hold contours.
    ///
    /// Separate from [`RoundSuperellipse::to_path`] so a caller assembling
    /// several shapes into one path -- a ring between two of these, a scene
    /// that draws one under an even-odd rule -- does not have to build a path
    /// and take it apart again.
    pub fn add_to(&self, builder: &mut PathBuilder) {
        let tr = &self.top_right;
        let start = tr.offset + tr.signed_scale * (tr.top.offset + Vec2::new(0.0, tr.top.se_a));
        builder.move_to(start);

        if self.all_corners_same {
            self.add_quadrant(builder, tr, false, Vec2::new(1.0, 1.0));
            self.add_quadrant(builder, tr, true, Vec2::new(1.0, -1.0));
            self.add_quadrant(builder, tr, false, Vec2::new(-1.0, -1.0));
            self.add_quadrant(builder, tr, true, Vec2::new(-1.0, 1.0));
        } else {
            self.add_quadrant(builder, &self.top_right, false, Vec2::ONE);
            self.add_quadrant(builder, &self.bottom_right, true, Vec2::ONE);
            self.add_quadrant(builder, &self.bottom_left, false, Vec2::ONE);
            self.add_quadrant(builder, &self.top_left, true, Vec2::ONE);
        }

        builder.line_to(start);
        builder.close();
    }

    fn add_quadrant(
        &self,
        builder: &mut PathBuilder,
        param: &Quadrant,
        reverse: bool,
        scale_sign: Vec2,
    ) {
        let scale = param.signed_scale * scale_sign;
        if param.top.se_n < 2.0 || param.right.se_n < 2.0 {
            // A square corner is two straight lines, and there is no arc to
            // place, so it never reaches the octant code below.
            let corner = param.top.offset + Vec2::new(param.top.se_a, param.top.se_a);
            builder.line_to(param.offset + scale * corner);
            let next = if reverse {
                param.top.offset + Vec2::new(0.0, param.top.se_a)
            } else {
                param.right.offset + Vec2::new(param.right.se_a, 0.0)
            };
            builder.line_to(param.offset + scale * next);
            return;
        }
        if reverse {
            self.add_octant(builder, &param.right, param.offset, scale, false, true);
            self.add_octant(builder, &param.top, param.offset, scale, true, false);
        } else {
            self.add_octant(builder, &param.top, param.offset, scale, false, false);
            self.add_octant(builder, &param.right, param.offset, scale, true, true);
        }
    }

    fn add_octant(
        &self,
        builder: &mut PathBuilder,
        param: &Octant,
        offset: Vec2,
        scale: Vec2,
        reverse: bool,
        flip: bool,
    ) {
        let place = Placement {
            offset,
            scale,
            octant_offset: param.offset,
            flip,
        };
        let circle = circular_arc_points(param);
        let se = superellipse_conics(param, param.circle_start.y / param.se_a);

        if reverse {
            builder.cubic_to(
                place.apply(circle[2]),
                place.apply(circle[1]),
                place.apply(circle[0]),
            );
            builder.conic_to(place.apply(se.c2), place.apply(se.h), se.weight2);
            builder.conic_to(place.apply(se.c1), place.apply(se.a), se.weight1);
        } else {
            builder.conic_to(place.apply(se.c1), place.apply(se.h), se.weight1);
            builder.conic_to(place.apply(se.c2), place.apply(se.j), se.weight2);
            builder.cubic_to(
                place.apply(circle[1]),
                place.apply(circle[2]),
                place.apply(circle[3]),
            );
        }
    }

    /// Whether the shape contains `p`, computed against the curve itself.
    ///
    /// Analytic rather than a test against the outline this produces, and
    /// deliberately: it is the definition the outline is checked against, so
    /// deriving one from the other would leave nothing checking either.
    pub fn contains(&self, p: Vec2) -> bool {
        if self.all_corners_same {
            return corner_contains(&self.top_right, p, false);
        }
        corner_contains(&self.top_right, p, true)
            && corner_contains(&self.bottom_right, p, true)
            && corner_contains(&self.bottom_left, p, true)
            && corner_contains(&self.top_left, p, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect::new(Vec2::new(l, t), Vec2::new(r, b))
    }

    /// Assert that `p` is inside and `p + offset` is outside.
    ///
    /// This is upstream's `CHECK_POINT_WITH_OFFSET`, and the pair is the whole
    /// point: either half alone is satisfied by a shape of the wrong size, and
    /// together they pin the boundary between them to two hundredths.
    #[track_caller]
    fn boundary(shape: &RoundSuperellipse, p: Vec2, offset: Vec2) {
        assert!(shape.contains(p), "{p:?} should be inside");
        assert!(
            !shape.contains(p + offset),
            "{:?} should be outside",
            p + offset
        );
    }

    /// The point and its three mirrors, as upstream checks them.
    #[track_caller]
    fn boundary_and_mirrors(shape: &RoundSuperellipse, p: Vec2, center: Vec2) {
        for sign in [
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(-1.0, 1.0),
            Vec2::new(-1.0, -1.0),
        ] {
            boundary(
                shape,
                (p - center) * sign + center,
                Vec2::new(0.02, 0.02) * sign,
            );
        }
    }

    /// Transcribed from upstream's `NoCornerRoundSuperellipseContains`.
    ///
    /// Zero radii are the one case that is not a superellipse at all, and the
    /// asymmetry in the expectations is not a mistake: a rectangle owns its
    /// top and left borders and not its bottom and right ones, so that two
    /// shapes sharing an edge do not both claim the pixels on it.
    #[test]
    fn a_superellipse_with_no_corners_is_a_rectangle_including_its_borders() {
        let shape = RoundSuperellipse::with_radius(rect(-50.0, -50.0, 50.0, 50.0), 0.0);

        assert!(shape.contains(Vec2::new(-50.0, -50.0)));
        assert!(shape.contains(Vec2::new(-50.0, 49.99)));
        assert!(shape.contains(Vec2::new(49.99, -50.0)));
        assert!(shape.contains(Vec2::new(49.99, 49.99)));

        for outside in [
            Vec2::new(-50.01, -50.0),
            Vec2::new(-50.0, -50.01),
            Vec2::new(-50.01, 50.0),
            Vec2::new(-50.0, 50.01),
            Vec2::new(50.01, -50.0),
            Vec2::new(50.0, -50.01),
            Vec2::new(50.01, 50.0),
            Vec2::new(50.0, 50.01),
        ] {
            assert!(!shape.contains(outside), "{outside:?} should be outside");
        }
    }

    /// Transcribed from upstream's `TinyCornerContains`.
    ///
    /// The discontinuity is deliberate upstream and worth keeping: a radius of
    /// zero keeps the corner point, and the smallest nonzero radius drops it.
    #[test]
    fn the_smallest_corner_still_cuts_the_corner_off() {
        let shape = RoundSuperellipse::new(
            rect(-50.0, -50.0, 50.0, 50.0),
            CornerRadii::uniform(Vec2::splat(0.01)),
        );
        for corner in [
            Vec2::new(-50.0, -50.0),
            Vec2::new(-50.0, 50.0),
            Vec2::new(50.0, -50.0),
            Vec2::new(50.0, 50.0),
        ] {
            assert!(!shape.contains(corner), "{corner:?} should be outside");
        }
    }

    /// Transcribed from upstream's `UniformSquareContains`.
    ///
    /// The labels are upstream's and they are what make this more than seven
    /// numbers: the joints are where the superellipse hands over to the
    /// circular arc, so a wrong degree or a misplaced handover moves those two
    /// points first and by the most.
    #[test]
    fn a_square_superellipse_has_upstreams_boundary() {
        let shape = RoundSuperellipse::with_radius(rect(-50.0, -50.0, 50.0, 50.0), 5.0);
        let o = Vec2::ZERO;
        boundary_and_mirrors(&shape, Vec2::new(0.0, 49.995), o); // Top
        boundary_and_mirrors(&shape, Vec2::new(44.245, 49.95), o); // Top curve start
        boundary_and_mirrors(&shape, Vec2::new(45.72, 49.87), o); // Top joint
        boundary_and_mirrors(&shape, Vec2::new(48.53, 48.53), o); // Circular arc mid
        boundary_and_mirrors(&shape, Vec2::new(49.87, 45.72), o); // Right joint
        boundary_and_mirrors(&shape, Vec2::new(49.95, 44.245), o); // Right curve start
        boundary_and_mirrors(&shape, Vec2::new(49.995, 0.0), o); // Right
    }

    /// Transcribed from upstream's `UniformEllipticalContains`.
    ///
    /// Unequal radii, which is the case that exercises the normalize-scale-
    /// unnormalize path rather than the round one.
    #[test]
    fn an_elliptical_superellipse_has_upstreams_boundary() {
        let shape = RoundSuperellipse::new(
            rect(-50.0, -50.0, 50.0, 50.0),
            CornerRadii::uniform(Vec2::new(5.0, 10.0)),
        );
        let o = Vec2::ZERO;
        boundary_and_mirrors(&shape, Vec2::new(0.0, 49.995), o);
        boundary_and_mirrors(&shape, Vec2::new(44.245, 49.911), o);
        boundary_and_mirrors(&shape, Vec2::new(45.72, 49.75), o);
        boundary_and_mirrors(&shape, Vec2::new(48.51, 47.07), o);
        boundary_and_mirrors(&shape, Vec2::new(49.87, 41.44), o);
        boundary_and_mirrors(&shape, Vec2::new(49.95, 38.49), o);
        boundary_and_mirrors(&shape, Vec2::new(49.995, 0.0), o);
    }

    /// Transcribed from upstream's `UniformRectangularContains`.
    ///
    /// Off-origin bounds and unequal sides, so a shape built around an assumed
    /// center fails this and passes the two above it.
    #[test]
    fn a_rectangular_superellipse_off_the_origin_has_upstreams_boundary() {
        let bounds = rect(0.0, 0.0, 50.0, 100.0);
        let shape = RoundSuperellipse::new(bounds, CornerRadii::uniform(Vec2::new(23.0, 30.0)));
        let center = (bounds.min + bounds.max) / 2.0;
        for p in [
            Vec2::new(24.99, 99.99), // Bottom mid edge
            Vec2::new(29.99, 99.64),
            Vec2::new(34.99, 98.06),
            Vec2::new(39.99, 94.73),
            Vec2::new(44.13, 89.99),
            Vec2::new(48.46, 79.99),
            Vec2::new(49.70, 69.99),
            Vec2::new(49.97, 59.99),
            Vec2::new(49.99, 49.99), // Right mid edge
        ] {
            boundary_and_mirrors(&shape, p, center);
        }
    }

    /// Transcribed from upstream's `SlimDiagonalContains`.
    ///
    /// Large radii on one diagonal and tiny ones on the other, which makes an
    /// almond lying north-west to south-east. This is the only transcribed
    /// shape whose corners differ, so it is the only one that reaches the
    /// four-quadrant containment rule and the side splitting -- everything
    /// above it is served by the single-quadrant shortcut.
    #[test]
    fn a_slim_diagonal_superellipse_has_upstreams_boundary() {
        let shape = RoundSuperellipse::new(
            rect(-50.0, -50.0, 50.0, 50.0),
            CornerRadii {
                top_left: Vec2::splat(1.0),
                top_right: Vec2::splat(99.0),
                bottom_left: Vec2::splat(99.0),
                bottom_right: Vec2::splat(1.0),
            },
        );

        assert!(shape.contains(Vec2::ZERO));
        for outside in [
            Vec2::new(-49.999, -49.999),
            Vec2::new(-49.999, 49.999),
            Vec2::new(49.999, 49.999),
            Vec2::new(49.999, -49.999),
        ] {
            assert!(!shape.contains(outside), "{outside:?} should be outside");
        }

        // The pointy ends, north-west and south-east.
        boundary(&shape, Vec2::new(-49.70, -49.70), Vec2::new(-0.02, -0.02));
        boundary(&shape, Vec2::new(49.70, 49.70), Vec2::new(0.02, 0.02));

        for p in [
            Vec2::new(-40.0, -49.59),
            Vec2::new(-20.0, -45.64),
            Vec2::new(0.0, -37.01),
            Vec2::new(20.0, -21.96),
            Vec2::new(21.05, -20.92),
            Vec2::new(40.0, 5.68),
        ] {
            boundary(&shape, p, Vec2::new(0.02, -0.02));
            boundary(&shape, p * -1.0, Vec2::new(-0.02, 0.02));
        }
    }

    /// Even-odd point-in-polygon over the flattened outline.
    ///
    /// Written here rather than borrowed because the crate has no such test on
    /// `Path` -- and a check of the outline that used the same tessellator the
    /// renderer uses would be checking two things at once.
    fn polygon_contains(contours: &[Vec<Vec2>], p: Vec2) -> bool {
        let mut inside = false;
        for contour in contours {
            for i in 0..contour.len() {
                let (a, b) = (contour[i], contour[(i + 1) % contour.len()]);
                if (a.y > p.y) != (b.y > p.y) {
                    let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
                    if x > p.x {
                        inside = !inside;
                    }
                }
            }
        }
        inside
    }

    /// Find the radius at which `inside` stops being true along one ray.
    fn edge_along(center: Vec2, dir: Vec2, inside: &dyn Fn(Vec2) -> bool) -> f32 {
        let (mut lo, mut hi) = (0.0f32, 400.0f32);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if inside(center + dir * mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) / 2.0
    }

    /// The drawn outline has to agree with the containment rule.
    ///
    /// This is what checks the conic weights and the whole path assembly,
    /// because the boundary points transcribed above are analytic and never
    /// touch either. The two definitions are independent -- one is a
    /// superellipse equation and a circle, the other is four conics and two
    /// cubics per quadrant -- so agreement between them is evidence and not a
    /// tautology.
    ///
    /// Measured rather than merely asserted: the worst disagreement over three
    /// shapes and 360 rays is reported when it fails, so a regression says how
    /// far off it went rather than only that it did.
    #[test]
    fn the_outline_agrees_with_the_containment_rule() {
        let shapes: [(&str, RoundSuperellipse, Vec2); 3] = [
            (
                "square, radius 5",
                RoundSuperellipse::with_radius(rect(-50.0, -50.0, 50.0, 50.0), 5.0),
                Vec2::ZERO,
            ),
            (
                "elliptical, radii 5x10",
                RoundSuperellipse::new(
                    rect(-50.0, -50.0, 50.0, 50.0),
                    CornerRadii::uniform(Vec2::new(5.0, 10.0)),
                ),
                Vec2::ZERO,
            ),
            (
                "rectangular off-origin, radii 23x30",
                RoundSuperellipse::new(
                    rect(0.0, 0.0, 50.0, 100.0),
                    CornerRadii::uniform(Vec2::new(23.0, 30.0)),
                ),
                Vec2::new(25.0, 50.0),
            ),
        ];

        for (name, shape, center) in shapes {
            let outline = crate::flatten::flatten(&shape.to_path(), 0.01);
            let mut worst = 0.0f32;
            let mut worst_at = 0.0f32;
            for step in 0..360 {
                let theta = step as f32 * std::f32::consts::TAU / 360.0;
                let dir = Vec2::new(theta.cos(), theta.sin());
                let analytic = edge_along(center, dir, &|p| shape.contains(p));
                let drawn = edge_along(center, dir, &|p| polygon_contains(&outline, p));
                if (analytic - drawn).abs() > worst {
                    worst = (analytic - drawn).abs();
                    worst_at = theta.to_degrees();
                }
            }
            // Measured at 0.006, 0.006 and 0.025 on the three shapes. The
            // bound sits just above the worst of them, and it has to: the
            // other reading of the conic weights comes in at 0.012, 0.024 and
            // 0.045, so a looser bound would admit both and check neither.
            // That was not hypothetical -- 0.05 was tried first and passed
            // with the weights wrong.
            assert!(
                worst < 0.03,
                "{name}: outline and containment differ by {worst:.4} at {worst_at:.0} degrees"
            );
        }
    }

    /// Transcribed from upstream's `PointsOutsideOfSharpCorner`.
    ///
    /// A regression test there, and worth carrying because the point is far
    /// outside the bounds entirely -- it passes trivially for any shape that
    /// checks its bounding box first, and this one does not, so it reaches the
    /// containment logic the way upstream's does.
    #[test]
    fn a_point_well_outside_a_sharp_cornered_superellipse_is_outside() {
        let shape = RoundSuperellipse::new(
            rect(196.0, 0.0, 294.0, 28.0),
            CornerRadii {
                top_left: Vec2::ZERO,
                top_right: Vec2::splat(3.0),
                bottom_left: Vec2::ZERO,
                bottom_right: Vec2::splat(3.0),
            },
        );
        assert!(!shape.contains(Vec2::new(147.0, 14.0)));
    }
}

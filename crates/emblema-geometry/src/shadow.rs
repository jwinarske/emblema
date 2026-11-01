//! The shadow of a blurred convex path, as a mesh rather than a blur pass.
//!
//! Upstream draws a mask-blurred fill it cannot express analytically -- an
//! oval that is not a circle, a rounded rectangle with unequal corners, any
//! other convex path -- as a mesh of two rings: the path's polygon inset
//! toward its centroid by the blur's reach, where the shadow is darkest, and
//! the polygon outset by the same reach, where it has gone, each vertex
//! carrying a Gaussian coefficient from zero to one that the fragment shader
//! maps through the Gaussian's integral. `ShadowPathGeometry` in
//! `impeller/entity/geometry/shadow_path_geometry.cc`, itself adapted from
//! Skia's `SkShadowTessellator`; this is that algorithm, step for step.
//!
//! Only a single convex contour has a mesh: [`ambient_shadow_mesh`] answers
//! `None` for anything else, and the caller blurs it the general way, as
//! upstream does.
//!
//! The mesh is built in the path's own space scaled to device pixels along
//! each axis, so that its sub-pixel grid and its rounding are in pixels, and
//! scaled back before it is returned: the vertices are in the path's space.

use crate::path::{Path, Verb};
use glam::{DVec2, Vec2};

/// A shadow mesh: triangles over `positions`, each vertex with the Gaussian
/// coefficient the shadow has there, from zero where it has faded to one
/// where it is darkest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShadowMesh {
    pub positions: Vec<Vec2>,
    pub gaussians: Vec<f32>,
    pub indices: Vec<u32>,
}

impl ShadowMesh {
    /// Whether there is nothing to draw: a path that enclosed no area.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// The blur's reach the mesh is built for, in device pixels, from the mask
/// blur's deviation in the path's space and the transform's largest scale:
/// upstream's `kSigmaScale` of 2.8, chosen by eye so that the mesh matches
/// its analytic shapes.
pub fn shadow_reach(sigma: f32, max_scale: f32) -> f32 {
    const SIGMA_SCALE: f32 = 2.8;
    sigma * SIGMA_SCALE * max_scale
}

/// The ambient shadow mesh of `path` for a blur reaching `reach` device
/// pixels, under a transform whose axes scale by `scale`.
///
/// `None` when the path is not one convex contour, or when the inset
/// polygon cannot be resolved: the caller blurs it some other way. An empty
/// mesh when it encloses no area.
pub fn ambient_shadow_mesh(path: &Path, scale: Vec2, reach: f32) -> Option<ShadowMesh> {
    if !(scale.x > 0.0 && scale.y > 0.0 && scale.is_finite()) {
        return Some(ShadowMesh::default());
    }
    if !(reach.is_finite() && reach >= 0.0) {
        return None;
    }
    let mut pins = Pins::new(scale);
    flatten_for_fill(path, scale.x.max(scale.y), &mut pins);
    let shape = match pins.finish() {
        Status::Empty => return Some(ShadowMesh::default()),
        Status::Unsupported => return None,
        Status::Convex(shape) => shape,
    };
    let trigs = trigs_for_radius(reach);
    let mut polygon = Polygon::new(reach);
    let mut mesh = polygon.convex_mesh(shape, &trigs)?;
    for p in &mut mesh.positions {
        *p /= scale;
    }
    Some(mesh)
}

// --- Flattening, as upstream's `PathTessellator::PathToFilledVertices`. ---

/// Receives a flattened path, a contour at a time.
trait VertexWriter {
    fn write(&mut self, point: Vec2);
    fn end_contour(&mut self);
}

/// Upstream's Wang's formula precision.
const PRECISION: f32 = 4.0;

fn quad_subdivisions(scale: f32, p0: Vec2, p1: Vec2, p2: Vec2) -> f32 {
    let k = scale * 0.25 * PRECISION;
    (k * (p0 - p1 * 2.0 + p2).length()).sqrt()
}

fn cubic_subdivisions(scale: f32, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2) -> f32 {
    let k = scale * 0.75 * PRECISION;
    let a = p0 - p1 * 2.0 + p2;
    let b = p1 - p2 * 2.0 + p3;
    let max_len_sq = a.length_squared().max(b.length_squared());
    (k * max_len_sq.sqrt()).sqrt()
}

fn quad_at(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    p0 * (mt * mt) + p1 * (2.0 * mt * t) + p2 * (t * t)
}

fn cubic_at(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    p0 * (mt * mt * mt) + p1 * (3.0 * mt * mt * t) + p2 * (3.0 * mt * t * t) + p3 * (t * t * t)
}

/// A fill's polygons: each contour from its first point, closed back to it
/// with the first point written again, as upstream's `PathPruner` and
/// `PathFillWriter` deliver them. Curves are divided as Wang's formula says
/// at `scale`.
fn flatten_for_fill(path: &Path, scale: f32, out: &mut impl VertexWriter) {
    let points = path.points();
    let mut i = 0usize;
    let mut origin = Vec2::ZERO;
    let mut current = Vec2::ZERO;
    // Whether the contour has begun: its first point is written with its
    // first segment, as upstream begins one.
    let mut started = false;
    for verb in path.verbs() {
        let n = verb.point_count();
        let Some(pts) = points.get(i..i + n) else {
            break;
        };
        i += n;
        if *verb == Verb::MoveTo {
            end_contour(out, &mut started, current, origin);
            origin = pts[0];
            current = pts[0];
            continue;
        }
        if !started {
            out.write(origin);
            started = true;
        }
        match verb {
            Verb::LineTo => {
                if pts[0] != current {
                    out.write(pts[0]);
                    current = pts[0];
                }
            }
            Verb::QuadTo => {
                let (cp, p2) = (pts[0], pts[1]);
                if cp == current || p2 == cp {
                    if p2 != current {
                        out.write(p2);
                        current = p2;
                    }
                    continue;
                }
                let count = quad_subdivisions(scale, current, cp, p2).ceil();
                let mut k = 1.0;
                while k < count {
                    out.write(quad_at(current, cp, p2, k / count));
                    k += 1.0;
                }
                out.write(p2);
                current = p2;
            }
            Verb::CubicTo => {
                let (c1, c2, p3) = (pts[0], pts[1], pts[2]);
                if c1 == current && c2 == current && p3 == current {
                    continue;
                }
                let count = cubic_subdivisions(scale, current, c1, c2, p3).ceil();
                let mut k = 1.0;
                while k < count {
                    out.write(cubic_at(current, c1, c2, p3, k / count));
                    k += 1.0;
                }
                out.write(p3);
                current = p3;
            }
            Verb::MoveTo | Verb::Close => {}
        }
    }
    end_contour(out, &mut started, current, origin);
}

/// Closes a begun contour back to its first point, as a fill closes it.
fn end_contour(out: &mut impl VertexWriter, started: &mut bool, current: Vec2, origin: Vec2) {
    if *started {
        if current != origin {
            out.write(origin);
        }
        out.end_contour();
    }
    *started = false;
}

// --- The pins: one per polygon vertex, as upstream's `UmbraPin`. ---

/// Where a pin's umbra fraction says it has not been visited.
const FRACTION_UNINITIALIZED: f32 = -1.0;

/// A point on the polygon and the pin pushed in from it toward the
/// centroid: how far, where its tip and its umbra vertex lie, and its
/// neighbors in the ring of pins that still shape the umbra.
#[derive(Debug, Clone, Copy)]
struct Pin {
    path_vertex: Vec2,
    path_delta: Vec2,
    penumbra_delta: Vec2,
    pin_tip: Vec2,
    umbra_vertex: Vec2,
    umbra_index: u32,
    umbra_fraction: f32,
    next: usize,
    prev: usize,
}

impl Pin {
    fn at(path_vertex: Vec2) -> Pin {
        Pin {
            path_vertex,
            path_delta: Vec2::ZERO,
            penumbra_delta: Vec2::ZERO,
            pin_tip: Vec2::ZERO,
            umbra_vertex: Vec2::ZERO,
            umbra_index: 0,
            umbra_fraction: FRACTION_UNINITIALIZED,
            next: 0,
            prev: 0,
        }
    }

    fn is_fraction_initialized(&self) -> bool {
        self.umbra_fraction > FRACTION_UNINITIALIZED
    }
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

/// Upstream's `Normalize`: `(1, 0)` for the zero vector.
fn normalize(v: Vec2) -> Vec2 {
    let len = (f64::from(v.x) * f64::from(v.x) + f64::from(v.y) * f64::from(v.y)).sqrt() as f32;
    if len == 0.0 {
        Vec2::X
    } else {
        v / len
    }
}

/// Upstream's `PerpendicularRight`: `(-y, x)`.
fn perpendicular_right(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

/// The squared distance from `p` to the segment `p0` to `p1`.
fn distance_to_segment_squared(p: Vec2, p0: Vec2, p1: Vec2) -> f32 {
    let u = p1 - p0;
    let v = p - p0;
    let dot = u.dot(v);
    if dot <= 0.0 {
        return v.length_squared();
    }
    let u_len_sq = u.length_squared();
    if dot >= u_len_sq {
        return (p - p1).length_squared();
    }
    let c = cross(u, v);
    c * c / u_len_sq
}

/// Counts the changes of sign of one axis along the polygon: more than
/// three and it winds more than once, so it is not convex.
#[derive(Default)]
struct DirectionDetector {
    last: f32,
    changes: usize,
}

impl DirectionDetector {
    fn accumulate(&mut self, d: f32) {
        if self.last == 0.0 || self.last * d < 0.0 {
            self.last = 1.0f32.copysign(d);
            self.changes += 1;
        }
    }

    fn is_concave(&self) -> bool {
        self.changes > 3
    }
}

/// A convex polygon's pins, its centroid and which way it turns.
struct Shape {
    pins: Vec<Pin>,
    centroid: Vec2,
    direction: f32,
}

enum Status {
    Empty,
    Unsupported,
    Convex(Shape),
}

/// Upstream's `UmbraPinAccumulator`: the polygon on a sixteenth-pixel grid,
/// duplicates and collinear points dropped, one contour only.
struct Pins {
    grid: Vec2,
    pins: Vec<Pin>,
    first_contour_ended: bool,
    multiple_contours: bool,
}

/// The sub-pixel grid the polygon is simplified on, and its reciprocal.
const SUB_PIXEL_COUNT: f32 = 16.0;
const SUB_PIXEL_SCALE: f32 = 1.0 / SUB_PIXEL_COUNT;

impl Pins {
    fn new(scale: Vec2) -> Pins {
        Pins {
            grid: scale * SUB_PIXEL_COUNT,
            pins: Vec::new(),
            first_contour_ended: false,
            multiple_contours: false,
        }
    }

    fn to_grid(&self, p: Vec2) -> Vec2 {
        (p * self.grid).round() * SUB_PIXEL_SCALE
    }

    /// Upstream's `FinalizePath`: convexity, the turning direction and the
    /// area-weighted centroid.
    fn finish(mut self) -> Status {
        if self.multiple_contours {
            return Status::Unsupported;
        }
        if self.pins.len() < 3 {
            return Status::Empty;
        }
        let mut xd = DirectionDetector::default();
        let mut yd = DirectionDetector::default();
        let mut relative_centroid = Vec2::ZERO;
        let mut direction = 0.0f32;
        let mut area = 0.0f32;
        let n = self.pins.len();
        let mut prev = self.pins[n - 1].path_vertex;
        let mut prev_prev = self.pins[n - 2].path_vertex;
        let first = self.pins[0].path_vertex;
        for pin in &self.pins {
            let p = pin.path_vertex;
            let delta = p - prev;
            xd.accumulate(delta.x);
            yd.accumulate(delta.y);
            if xd.is_concave() || yd.is_concave() {
                return Status::Unsupported;
            }
            if direction != 0.0 {
                let c = cross(prev - prev_prev, p - prev_prev);
                if c * direction < 0.0 {
                    return Status::Unsupported;
                }
            }
            let v0 = prev - first;
            let v1 = p - first;
            let quad_area = cross(v0, v1);
            if quad_area != 0.0 {
                if direction == 0.0 {
                    direction = 1.0f32.copysign(quad_area);
                } else if quad_area * direction < 0.0 {
                    return Status::Unsupported;
                }
                relative_centroid += (v0 + v1) * quad_area;
                area += quad_area;
            }
            prev_prev = prev;
            prev = p;
        }
        if direction == 0.0 {
            return Status::Empty;
        }
        relative_centroid /= 3.0 * area;
        let centroid = first + relative_centroid;
        if !centroid.is_finite() {
            return Status::Unsupported;
        }
        Status::Convex(Shape {
            pins: std::mem::take(&mut self.pins),
            centroid,
            direction,
        })
    }
}

impl VertexWriter for Pins {
    fn write(&mut self, point: Vec2) {
        if self.first_contour_ended {
            self.multiple_contours = true;
            return;
        }
        let point = self.to_grid(point);
        if let Some(prev) = self.pins.last().map(|p| p.path_vertex) {
            if point == prev {
                return;
            }
            if self.pins.len() >= 2 {
                let prev_prev = self.pins[self.pins.len() - 2].path_vertex;
                if cross(prev - prev_prev, point - prev_prev) == 0.0 {
                    self.pins.pop();
                    if point == prev_prev {
                        return;
                    }
                }
            }
        }
        self.pins.push(Pin::at(point));
    }

    fn end_contour(&mut self) {
        if self.first_contour_ended {
            self.multiple_contours = true;
            return;
        }
        // The contour was closed by writing its first point again; that
        // point is dropped, as upstream drops it.
        if self.pins.len() > 1
            && self.pins[0].path_vertex == self.pins[self.pins.len() - 1].path_vertex
        {
            self.pins.pop();
        }
        self.first_contour_ended = true;
    }
}

// --- The corner fans' angles, as upstream's `Tessellator::Trigs`. ---

/// Upstream's circle tolerance, in pixels.
const CIRCLE_TOLERANCE: f64 = 0.1;

/// Upstream's `ComputeQuadrantDivisions`: how many steps a quarter turn
/// takes at `radius` pixels. Its table for radii under 1,024 is this
/// formula at the radius rounded up.
fn quadrant_divisions(radius: f32) -> usize {
    if radius <= 0.0 {
        return 1;
    }
    let r = if radius < 1024.0 {
        f64::from(radius.ceil())
    } else {
        f64::from(radius)
    };
    let k = CIRCLE_TOLERANCE / r;
    (std::f64::consts::FRAC_PI_4 / (1.0 - k).acos())
        .ceil()
        .max(1.0) as usize
}

/// The cosines and sines of a quarter turn in `divisions` steps, both ends
/// included.
fn trigs_for_radius(radius: f32) -> Vec<DVec2> {
    let divisions = quadrant_divisions(radius);
    let step = std::f64::consts::FRAC_PI_2 / divisions as f64;
    let mut t = Vec::with_capacity(divisions + 1);
    t.push(DVec2::new(1.0, 0.0));
    for i in 1..divisions {
        let a = i as f64 * step;
        t.push(DVec2::new(a.cos(), a.sin()));
    }
    t.push(DVec2::new(0.0, 1.0));
    t
}

/// `v` turned by the angle whose cosine and sine are `trig`, in double
/// precision as upstream's `Trig` turns it.
fn rotate(trig: DVec2, v: Vec2) -> Vec2 {
    let (x, y) = (f64::from(v.x), f64::from(v.y));
    Vec2::new(
        (x * trig.x - y * trig.y) as f32,
        (x * trig.y + y * trig.x) as f32,
    )
}

// --- The mesh, as upstream's `PolygonInfo`. ---

/// Tolerances adopted from Skia's, as upstream adopts them.
const CROSS_TOLERANCE: f32 = 1.0 / 2048.0;
const INTERSECTION_TOLERANCE: f32 = 1.0e-6;
const MIN_SUB_PIXEL_DISTANCE_SQUARED: f32 = SUB_PIXEL_SCALE * SUB_PIXEL_SCALE;

fn nearly_zero(x: f32, tolerance: f32) -> bool {
    x.abs() <= tolerance
}

/// Upstream's `OutsideInterval`.
fn outside_interval(numer: f32, denom: f32, denom_positive: bool) -> bool {
    (denom_positive && (numer < 0.0 || numer > denom))
        || (!denom_positive && (numer > 0.0 || numer < denom))
}

fn finite_length_squared(v: Vec2) -> f32 {
    if v.is_finite() {
        v.dot(v)
    } else {
        -1.0
    }
}

/// Where two pins cross, and how far along each.
struct Crossing {
    point: Vec2,
    fraction0: f32,
    fraction1: f32,
}

struct Polygon {
    reach: f32,
    umbra_gaussian: f32,
    mesh: ShadowMesh,
}

impl Polygon {
    fn new(reach: f32) -> Polygon {
        Polygon {
            reach,
            umbra_gaussian: 1.0,
            mesh: ShadowMesh::default(),
        }
    }

    fn convex_mesh(&mut self, shape: Shape, trigs: &[DVec2]) -> Option<ShadowMesh> {
        let Shape {
            mut pins,
            centroid,
            direction,
        } = shape;
        self.pin_directions(&mut pins, centroid, direction);
        let (head, count) = Self::resolve_umbra(&mut pins, direction)?;
        self.compute_mesh(&mut pins, centroid, head, count, trigs, direction)?;
        Some(std::mem::take(&mut self.mesh))
    }

    /// Upstream's `ComputePinDirectionsAndMinDistanceToCentroid`.
    fn pin_directions(&mut self, pins: &mut [Pin], centroid: Vec2, direction: f32) {
        let desired = self.reach;
        let mut min_sq = desired * desired;
        let n = pins.len();
        for i in 0..n {
            let prev = pins[(i + n - 1) % n].path_vertex;
            min_sq = min_sq.min(distance_to_segment_squared(
                centroid,
                prev,
                pins[i].path_vertex,
            ));
        }
        const TOLERANCE: f32 = 1.0e-2;
        let mut umbra = min_sq.sqrt();
        if umbra < desired + TOLERANCE {
            let inset = umbra - TOLERANCE;
            let ratio = 0.5 * (inset / desired + 1.0);
            self.umbra_gaussian = if ratio.is_finite() {
                ratio.clamp(0.0, 1.0)
            } else {
                0.0
            };
            umbra = inset;
        }
        let penumbra_scale = -self.reach;
        for i in 0..n {
            let p = (i + n - 1) % n;
            pins[i].prev = p;
            pins[p].next = i;
            pins[p].path_delta = pins[i].path_vertex - pins[p].path_vertex;
            let dir = perpendicular_right(normalize(pins[p].path_delta)) * direction;
            pins[p].penumbra_delta = dir * penumbra_scale;
            let tip = pins[p].path_vertex + dir * umbra;
            pins[p].pin_tip = tip;
            pins[p].umbra_vertex = tip;
        }
    }

    /// Upstream's `ComputeIntersection`, from Skia's `SkPolyUtils`.
    fn intersection(pin0: &Pin, pin1: &Pin) -> Option<Crossing> {
        let v0 = pin0.path_delta;
        let v1 = pin1.path_delta;
        let w = pin1.pin_tip - pin0.pin_tip;
        let mut denom = cross(v0, v1);
        let denom_positive = denom > 0.0;
        let (numerator0, numerator1);
        if nearly_zero(denom, CROSS_TOLERANCE) {
            if !nearly_zero(cross(w, v0), CROSS_TOLERANCE)
                || !nearly_zero(cross(w, v1), CROSS_TOLERANCE)
            {
                return None;
            }
            let v0_sq = finite_length_squared(v0);
            if v0_sq <= 0.0 {
                let v1_sq = finite_length_squared(v1);
                if v1_sq <= 0.0 {
                    if w.is_finite() && w != Vec2::ZERO {
                        return Some(Crossing {
                            point: pin0.pin_tip,
                            fraction0: 0.0,
                            fraction1: 0.0,
                        });
                    }
                    return None;
                }
                let n1 = v1.dot(-w);
                denom = v1_sq;
                if outside_interval(n1, denom, true) {
                    return None;
                }
                numerator0 = 0.0;
                numerator1 = n1;
            } else {
                let mut n0 = v0.dot(w);
                denom = v0_sq;
                let mut n1 = 0.0;
                if outside_interval(n0, denom, true) {
                    let v1_sq = finite_length_squared(v1);
                    if v1_sq <= 0.0 {
                        return None;
                    }
                    let old_n0 = n0;
                    n0 = v0.dot(w + v1);
                    n1 = denom;
                    if outside_interval(n0, denom, true) {
                        if n0 * old_n0 > 0.0 {
                            return None;
                        }
                        n0 = 0.0;
                        n1 = v1.dot(-w);
                        denom = v1_sq;
                    }
                }
                numerator0 = n0;
                numerator1 = n1;
            }
        } else {
            let n0 = cross(w, v1);
            if outside_interval(n0, denom, denom_positive) {
                return None;
            }
            let n1 = cross(w, v0);
            if outside_interval(n1, denom, denom_positive) {
                return None;
            }
            numerator0 = n0;
            numerator1 = n1;
        }
        let fraction0 = numerator0 / denom;
        let fraction1 = numerator1 / denom;
        Some(Crossing {
            point: pin0.pin_tip + v0 * fraction0,
            fraction0,
            fraction1,
        })
    }

    /// Takes pin `i` out of the ring; the head moves on if it was the head,
    /// and is `None` once the ring is empty.
    fn remove(pins: &mut [Pin], i: usize, head: &mut Option<usize>) {
        let (n, p) = (pins[i].next, pins[i].prev);
        pins[p].next = n;
        pins[n].prev = p;
        if *head == Some(i) {
            *head = if n == i { None } else { Some(n) };
        }
    }

    /// Upstream's `ComputeSide`: which side of the line through `p0` along
    /// `v` the point `p` is on.
    fn side(p0: Vec2, v: Vec2, p: Vec2) -> f32 {
        let c = cross(v, p - p0);
        if nearly_zero(c, CROSS_TOLERANCE) {
            0.0
        } else if c > 0.0 {
            1.0
        } else {
            -1.0
        }
    }

    /// Upstream's `ResolveUmbraIntersections`: the ring of pins whose umbra
    /// vertices make the inset polygon, its head and length.
    fn resolve_umbra(pins: &mut [Pin], direction: f32) -> Option<(usize, usize)> {
        let mut head = Some(0usize);
        let mut curr = 0usize;
        let mut prev = pins[curr].prev;
        let mut allowed = pins.len() * pins.len() + 1;
        while head.is_some() && prev != curr {
            allowed -= 1;
            if allowed == 0 {
                return None;
            }
            match Self::intersection(&pins[prev], &pins[curr]) {
                Some(x) => {
                    if x.fraction0 < pins[prev].umbra_fraction {
                        Self::remove(pins, prev, &mut head);
                        prev = pins[prev].prev;
                    } else if pins[curr].is_fraction_initialized()
                        && pins[curr].umbra_vertex.distance_squared(x.point)
                            < INTERSECTION_TOLERANCE
                    {
                        break;
                    } else {
                        pins[curr].umbra_vertex = x.point;
                        pins[curr].umbra_fraction = x.fraction1;
                        prev = curr;
                        curr = pins[curr].next;
                    }
                }
                None => {
                    let (ct, cd, pt, pd) = (
                        pins[curr].pin_tip,
                        pins[curr].path_delta,
                        pins[prev].pin_tip,
                        pins[prev].path_delta,
                    );
                    let side = direction * Self::side(ct, cd, pt);
                    if side < 0.0 && side == direction * Self::side(ct, cd, pt + pd) {
                        Self::remove(pins, prev, &mut head);
                        prev = pins[prev].prev;
                    } else {
                        Self::remove(pins, curr, &mut head);
                        curr = pins[curr].next;
                    }
                }
            }
        }
        let head = head?;
        let mut prev = head;
        let mut curr = pins[head].next;
        let mut count = 1usize;
        let mut head = Some(head);
        while Some(curr) != head {
            if pins[prev]
                .umbra_vertex
                .distance_squared(pins[curr].umbra_vertex)
                < MIN_SUB_PIXEL_DISTANCE_SQUARED
            {
                Self::remove(pins, curr, &mut head);
                curr = pins[curr].next;
            } else {
                count += 1;
                prev = curr;
                curr = pins[curr].next;
            }
            head?;
        }
        if count < 3 {
            return None;
        }
        Some((head?, count))
    }

    /// Upstream's `ComputeMesh`: the inner ring from the centroid to the
    /// umbra polygon, the outer ring from it to the penumbra, rounded at
    /// each corner by a fan.
    fn compute_mesh(
        &mut self,
        pins: &mut [Pin],
        centroid: Vec2,
        head: usize,
        count: usize,
        trigs: &[DVec2],
        direction: f32,
    ) -> Option<()> {
        let penumbra = pins.len() * 2 + trigs.len() * 4;
        self.mesh.positions.reserve(count + 1 + penumbra);
        self.mesh.gaussians.reserve(count + 1 + penumbra);
        self.mesh.indices.reserve((count + penumbra) * 3);

        self.populate_umbra(pins, head, centroid)?;

        // Each pin bridges from where the previous pin's penumbra ended, the
        // point perpendicular to the previous side, to the same offset from
        // its own vertex, then fans round its corner to the point
        // perpendicular to the next side, where the next pin begins.
        let n = pins.len();
        let mut prev = n - 1;
        let mut last_index =
            self.vertex(pins[prev].path_vertex + pins[prev].penumbra_delta, 0.0)?;
        for curr in 0..n {
            let (pp, cp) = (pins[prev], pins[curr]);
            if pp.umbra_index != cp.umbra_index {
                self.triangle(last_index, pp.umbra_index, cp.umbra_index);
            }
            let bridged = cp.path_vertex + pp.penumbra_delta;
            let bridged_index = self.vertex(bridged, 0.0)?;
            if last_index != bridged_index {
                self.triangle(cp.umbra_index, last_index, bridged_index);
            }
            let end = cp.path_vertex + cp.penumbra_delta;
            last_index = self.fan(&cp, bridged, end, bridged_index, trigs, direction)?;
            prev = curr;
        }
        Some(())
    }

    /// Upstream's `PopulateUmbraVertices`: each pin's nearest surviving
    /// umbra vertex, and the fan from the centroid across them.
    fn populate_umbra(&mut self, pins: &mut [Pin], head: usize, centroid: Vec2) -> Option<()> {
        let mut last = self.vertex(centroid, self.umbra_gaussian)?;
        let mut next_umbra = head;
        let mut curr_umbra = pins[next_umbra].prev;
        for i in 0..pins.len() {
            let pv = pins[i].path_vertex;
            if next_umbra == i
                || pv.distance_squared(pins[curr_umbra].umbra_vertex)
                    > pv.distance_squared(pins[next_umbra].umbra_vertex)
            {
                curr_umbra = next_umbra;
                next_umbra = pins[next_umbra].next;
                let index = self.vertex(pins[curr_umbra].umbra_vertex, self.umbra_gaussian)?;
                pins[curr_umbra].umbra_index = index;
                if last != 0 {
                    self.triangle(0, last, index);
                }
                last = index;
            }
            if curr_umbra != i {
                pins[i].umbra_vertex = pins[curr_umbra].umbra_vertex;
                pins[i].umbra_index = last;
            }
        }
        if last != pins[0].umbra_index {
            self.triangle(0, last, pins[0].umbra_index);
        }
        Some(())
    }

    /// Upstream's `AppendFan`: the penumbra around `pin`'s corner from
    /// `start` to `end`, a vertex at each step of `trigs`.
    fn fan(
        &mut self,
        pin: &Pin,
        start: Vec2,
        end: Vec2,
        start_index: u32,
        trigs: &[DVec2],
        direction: f32,
    ) -> Option<u32> {
        let center = pin.path_vertex;
        let center_index = pin.umbra_index;
        let mut prev_index = start_index;
        let mut start_delta = start - center;
        let end_delta = end - center;
        let count = trigs.len();
        // A bound the loop cannot reach for a convex polygon, which turns a
        // full circle once in all; it is here so that no input can spin it.
        let mut budget = 4 * count + 8;
        let mut i = 1usize;
        while i < count {
            budget = budget.checked_sub(1)?;
            let t = trigs[i];
            let trig = if direction >= 0.0 {
                t
            } else {
                DVec2::new(t.x, -t.y)
            };
            let fan_delta = rotate(trig, start_delta);
            if cross(fan_delta, end_delta) * direction <= 0.0 {
                break;
            }
            let cur = self.vertex(center + fan_delta, 0.0)?;
            if prev_index != cur {
                self.triangle(center_index, prev_index, cur);
                prev_index = cur;
            }
            if i == count - 1 {
                i = 0;
                start_delta = fan_delta;
            }
            i += 1;
        }
        let cur = self.vertex(center + end_delta, 0.0)?;
        if prev_index != cur {
            self.triangle(center_index, prev_index, cur);
        }
        Some(cur)
    }

    /// Upstream's `AppendVertex`: a vertex the same as the last is the last.
    /// `None` past what a 32-bit index addresses.
    fn vertex(&mut self, p: Vec2, gaussian: f32) -> Option<u32> {
        let m = &mut self.mesh;
        let index = u32::try_from(m.positions.len()).ok()?;
        if index > 0 && m.gaussians.last() == Some(&gaussian) && m.positions.last() == Some(&p) {
            return Some(index - 1);
        }
        m.positions.push(p);
        m.gaussians.push(gaussian);
        Some(index)
    }

    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.mesh.indices.extend([a, b, c]);
    }
}

/// Upstream's `IPHalfFractionToFastGaussianCDF`: the shadow's opacity at a
/// Gaussian coefficient, the Abramowitz and Stegun approximation of erf
/// over -2 to 2. The shader computes the same; this is for tests and
/// reference.
pub fn shadow_opacity(gaussian: f32) -> f32 {
    let x = gaussian * 4.0 - 2.0;
    let a = x.abs();
    let b = (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a + 1.0;
    let erf = x.signum() * (1.0 - 1.0 / (b * b * b * b));
    (1.0 + erf) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::PathBuilder;

    fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
        // Four cubics, as a canvas's oval is built.
        const K: f32 = 0.552_284_8;
        let (kx, ky) = (K * rx, K * ry);
        let mut b = PathBuilder::new();
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
        b.build()
    }

    fn rect(l: f32, t: f32, r: f32, bt: f32) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(l, t))
            .line_to(Vec2::new(r, t))
            .line_to(Vec2::new(r, bt))
            .line_to(Vec2::new(l, bt))
            .close();
        b.build()
    }

    #[test]
    fn upstreams_quadrant_divisions_are_its_formula() {
        // A sample of upstream's table for radii under 1,024, which is the
        // formula at the radius rounded up.
        for (r, want) in [
            (0.0, 1),
            (1.0, 2),
            (2.0, 3),
            (3.0, 4),
            (4.0, 4),
            (6.0, 5),
            (9.0, 6),
            (10.5, 6),
        ] {
            assert_eq!(quadrant_divisions(r), want, "radius {r}");
        }
        let t = trigs_for_radius(9.0);
        assert_eq!(t.len(), 7);
        assert_eq!(t[0], DVec2::new(1.0, 0.0));
        assert_eq!(t[6], DVec2::new(0.0, 1.0));
    }

    #[test]
    fn an_oval_has_a_mesh_dark_inside_and_clear_outside() {
        let path = ellipse(100.0, 80.0, 60.0, 30.0);
        let mesh = ambient_shadow_mesh(&path, Vec2::ONE, 10.0).expect("convex");
        assert!(!mesh.is_empty());
        assert_eq!(mesh.positions.len(), mesh.gaussians.len());
        assert!(mesh.indices.len() % 3 == 0);
        assert!(mesh
            .indices
            .iter()
            .all(|&i| (i as usize) < mesh.positions.len()));
        // The centroid first, at the center, and dark: the oval is wider than
        // the blur reaches. Not quite one: upstream starts the shortest
        // distance at the reach itself, so its tolerance always backs the
        // umbra off by a hundredth, 0.5 * (9.99 / 10 + 1).
        assert!((mesh.positions[0] - Vec2::new(100.0, 80.0)).length() < 0.1);
        assert!(
            (mesh.gaussians[0] - 0.9995).abs() < 1e-6,
            "{}",
            mesh.gaussians[0]
        );
        // The outer ring at zero, one reach outside the oval: nothing past it.
        let outer: Vec<Vec2> = mesh
            .positions
            .iter()
            .zip(&mesh.gaussians)
            .filter(|(_, g)| **g == 0.0)
            .map(|(p, _)| *p)
            .collect();
        assert!(!outer.is_empty());
        for p in outer {
            let d = (p - Vec2::new(100.0, 80.0)) / Vec2::new(60.0, 30.0);
            assert!(d.length() > 1.0, "a clear vertex inside the oval at {p}");
        }
        let bounds = mesh
            .positions
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                (lo.min(p.x), hi.max(p.x))
            });
        assert!(
            (bounds.0 - 30.0).abs() < 0.5 && (bounds.1 - 170.0).abs() < 0.5,
            "{bounds:?}"
        );
    }

    #[test]
    fn a_shape_thinner_than_the_reach_is_lighter_or_has_no_mesh() {
        // Eight from the center at its narrowest against a reach of ten: the
        // umbra is pulled back to 7.99 and dimmed to 0.5 * (7.99 / 10 + 1).
        let mesh =
            ambient_shadow_mesh(&ellipse(100.0, 80.0, 60.0, 8.0), Vec2::ONE, 10.0).expect("convex");
        assert!(
            (mesh.gaussians[0] - 0.8995).abs() < 1e-4,
            "{}",
            mesh.gaussians[0]
        );
        // A rectangle narrower than twice the reach: its long sides' umbra
        // vertices meet within a sixteenth of a pixel, fewer than three are
        // left, and upstream has no mesh for it either.
        assert_eq!(
            ambient_shadow_mesh(&rect(0.0, 0.0, 6.0, 40.0), Vec2::ONE, 10.0),
            None
        );
    }

    #[test]
    fn a_scaled_mesh_is_in_the_paths_space() {
        let path = ellipse(50.0, 50.0, 30.0, 20.0);
        let one = ambient_shadow_mesh(&path, Vec2::ONE, 8.0).expect("convex");
        let two = ambient_shadow_mesh(&path, Vec2::splat(2.0), 16.0).expect("convex");
        // The same reach in the path's space, built at twice the pixels: the
        // outer ring lands in the same place within a grid step.
        let extent = |m: &ShadowMesh| m.positions.iter().fold(f32::MIN, |hi, p| hi.max(p.x));
        assert!((extent(&one) - extent(&two)).abs() < 0.2);
    }

    #[test]
    fn a_concave_path_or_two_contours_has_no_mesh() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(40.0, 0.0))
            .line_to(Vec2::new(20.0, 10.0))
            .line_to(Vec2::new(40.0, 40.0))
            .line_to(Vec2::new(0.0, 40.0))
            .close();
        assert_eq!(ambient_shadow_mesh(&b.build(), Vec2::ONE, 4.0), None);

        let mut b = PathBuilder::new();
        for x in [0.0, 100.0] {
            b.move_to(Vec2::new(x, 0.0))
                .line_to(Vec2::new(x + 40.0, 0.0))
                .line_to(Vec2::new(x + 40.0, 40.0))
                .close();
        }
        assert_eq!(ambient_shadow_mesh(&b.build(), Vec2::ONE, 4.0), None);
    }

    #[test]
    fn a_path_with_no_area_has_an_empty_mesh() {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(40.0, 0.0))
            .close();
        assert_eq!(
            ambient_shadow_mesh(&b.build(), Vec2::ONE, 4.0),
            Some(ShadowMesh::default())
        );
        assert_eq!(
            ambient_shadow_mesh(&rect(0.0, 0.0, 10.0, 10.0), Vec2::ZERO, 4.0),
            Some(ShadowMesh::default())
        );
    }

    #[test]
    fn the_opacity_curve_runs_from_clear_to_dark() {
        assert!(shadow_opacity(0.0) < 0.003);
        assert!((shadow_opacity(0.5) - 0.5).abs() < 1e-6);
        assert!(shadow_opacity(1.0) > 0.997);
    }
}

//! Convexity, checked against the definition it is an optimization of.
//!
//! Classifying a polygon convex lets the tessellator fan-fill it, which skips
//! the sweep entirely -- and applies no fill rule, since a fan has no notion of
//! one. So a wrong `Convex` is not a missed optimization but a silently wrong
//! picture: a self-crossing path filled as though it did not cross, with its
//! fill rule discarded. That is what this guards.
//!
//! The shipped classifier counts quadrant advances, which is exact and cheap.
//! The definition it implements is the total turning angle, which is neither.
//! Both are here and the two are required to agree, so the cheap one can be
//! changed with something to check it against.

use emblema_geometry::path::polygon_convexity;
use emblema_geometry::Convexity;
use glam::Vec2;

/// Turning accumulated as an angle, in double precision: the definition.
///
/// A simple closed polygon turns through one full circle and a self-crossing
/// one through two or more, so this is what convexity means once the turns are
/// known to agree. It is not what ships, because an `atan2` per vertex costs
/// several times the whole rest of the classification.
fn reference(points: &[Vec2]) -> Convexity {
    if points.len() < 3 {
        return Convexity::Convex;
    }
    let n = points.len();
    let (mut sign, mut turning) = (0i32, 0.0f64);
    for i in 0..n {
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        let (u, v) = (b - a, c - b);
        let cross = u.perp_dot(v);
        turning += (cross as f64).atan2(u.dot(v) as f64);
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
        return Convexity::Convex;
    }
    if (turning.abs() - std::f64::consts::TAU).abs() > 0.5 {
        return Convexity::Concave;
    }
    Convexity::Convex
}

/// A reproducible stream of coordinates.
///
/// Its own generator rather than a dependency: the corpus of shapes wanted here
/// is "arbitrary triangles through nonagons", which needs no distribution worth
/// naming, and the seed is fixed so a disagreement is reproducible.
fn lcg(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 33) as f32 / (1u64 << 31) as f32) - 1.0
}

#[test]
fn agrees_with_the_angular_definition() {
    let mut state = 0x5eed_u64;
    let mut disagreements = 0;
    let mut concave_seen = 0;
    for case in 0..20000 {
        let n = 3 + (case % 9);
        let pts: Vec<Vec2> = (0..n)
            .map(|_| Vec2::new(lcg(&mut state) * 100.0, lcg(&mut state) * 100.0))
            .collect();
        let a = polygon_convexity(&pts);
        let b = reference(&pts);
        if a != b {
            disagreements += 1;
            if disagreements < 5 {
                println!("case {case}: {a:?} vs {b:?} {pts:?}");
            }
        }
        if b == Convexity::Concave {
            concave_seen += 1;
        }
    }
    // Regular star polygons, where the turns agree and the shape crosses.
    for (points, skip) in [(5usize, 2usize), (7, 2), (7, 3), (9, 4), (11, 3)] {
        let pts: Vec<Vec2> = (0..points)
            .map(|k| {
                let a = (k * skip) as f32 / points as f32 * std::f32::consts::TAU;
                Vec2::new(64.0 + 48.0 * a.cos(), 64.0 + 48.0 * a.sin())
            })
            .collect();
        let got = polygon_convexity(&pts);
        println!(
            "{{{}}}/{{{}}} star: {:?} (reference {:?})",
            points,
            skip,
            got,
            reference(&pts)
        );
        assert_eq!(
            got,
            Convexity::Concave,
            "a {points}/{skip} star is not convex"
        );
    }
    println!(
        "{} random cases, {} concave, {} disagreements",
        20000, concave_seen, disagreements
    );
    assert_eq!(disagreements, 0);
}

/// Not a threshold, which would be a flaky test on a shared machine, but a
/// number printed where a change to this code would be reviewed. The exact
/// version costs several times the counted one, which is why the counted one
/// is what ships.
#[test]
fn the_exact_definition_is_the_expensive_one() {
    for n in [64usize, 512] {
        let pts: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(200.0 * a.cos(), 200.0 * a.sin())
            })
            .collect();
        let bench = |f: fn(&[Vec2]) -> Convexity| {
            for _ in 0..200 {
                std::hint::black_box(f(&pts));
            }
            let t = std::time::Instant::now();
            for _ in 0..20000 {
                std::hint::black_box(f(&pts));
            }
            t.elapsed().as_secs_f64() * 1e9 / 20000.0
        };
        println!(
            "n={}: shipped {:.0} ns, angular reference {:.0} ns",
            n,
            bench(polygon_convexity),
            bench(reference)
        );
    }
}

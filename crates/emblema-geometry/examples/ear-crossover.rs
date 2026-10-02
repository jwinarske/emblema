//! Where ear clipping stops beating lyon, on this machine.
//!
//! `ear_fill`'s cap is a measurement, not a judgment, and this is the measurement.
//! The verification that makes ear clipping safe to use is quadratic in the point
//! count while lyon's sweep is `O(n log n)`, so the fast path wins on a small contour
//! and loses on a large one. The cap belongs just below where the two cross.
//!
//! The contour is the bench's own: a star whose vertices alternate between full and
//! four-tenths reach, which is `xtask/src/bench/frames.rs`'s `concave` scene. Both
//! columns are best-of-seven over two hundred fills, so the figures are floors rather
//! than averages -- the cap wants the ratio where neither route is being charged for
//! something the machine did.
//!
//! Rows past the cap read about one, both columns having taken lyon. Seeing the
//! crossover itself means raising `MAX_EAR_POINTS` first -- which is how the figures in
//! its own doc comment were taken.
//!
//! The crossover moves with cache and memory, so a board is worth re-running before
//! trusting a cap set here. See `docs/on-a-board.md`.

use emblema_geometry::tessellate::Tessellator;
use emblema_geometry::Path;
use glam::Vec2;

/// The bench's concave scene, as one path.
fn star(points: usize) -> Path {
    let mut builder = Path::builder();
    for i in 0..points {
        let t = i as f32 / points as f32 * std::f32::consts::TAU;
        let reach = if i % 2 == 0 { 1.0 } else { 0.4 };
        let at = Vec2::new(
            500.0 + 400.0 * reach * t.cos(),
            500.0 + 400.0 * reach * t.sin(),
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

/// Microseconds per fill, taking the fastest of seven passes of two hundred.
fn floor_of(mut fill: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..7 {
        let started = std::time::Instant::now();
        for _ in 0..200 {
            fill();
        }
        best = best.min(started.elapsed().as_secs_f64() / 200.0 * 1e6);
    }
    best
}

fn main() {
    let mut tessellator = Tessellator::new();
    println!(
        "{:>5}  {:>9}  {:>9}  {:>6}",
        "n", "fill us", "lyon us", "ratio"
    );
    for points in [8usize, 12, 16, 20, 24, 28, 32, 40, 48, 64, 96, 128] {
        let path = star(points);
        let fast = floor_of(|| {
            tessellator.fill(&path, 0.25);
        });
        let lyon = floor_of(|| {
            tessellator.fill_general(&path, 0.25);
        });
        // Above the cap both columns take lyon, so the ratio reads about one and the
        // sweep has run past the point it was measuring.
        println!(
            "{points:>5}  {fast:>9.3}  {lyon:>9.3}  {:>5.2}x",
            lyon / fast
        );
    }
}

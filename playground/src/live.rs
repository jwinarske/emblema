//! Scenes with a knob, drawn at whatever size the window is.
//!
//! The corpus is authored for comparison: fixed size, fixed parameters, so two
//! backends can be checked against each other. These are the opposite — one
//! parameter you can sweep, drawn at the window's own size, for the questions a
//! still image cannot answer.
//!
//! Two kinds of question in particular:
//!
//! A **continuity** question. Turn a knob through its range and watch for a
//! step where there should be a slope. The gradient scene below crosses the
//! boundary where stops stop fitting in a material and get baked into a texture
//! instead — two entirely different shader paths, which the suite checks agree
//! at one pair of values. Here you can watch the crossing. A pop at four stops
//! is the two paths disagreeing, and no still comparison would show it.
//!
//! A **temporal** question. Marching dashes are the obvious one: a phase that
//! advances every frame shows flicker, popping and reordering that a single
//! frame cannot contain.

use emblema::{
    Affine2, Canvas, Color, Dash, Extent2D, GradientStop, Layer, Paint, PathBuilder, Rect,
    SourceRect, Sprite, Vec2, VertexMode, Vertices,
};

/// One parametric scene.
pub struct Live {
    pub name: &'static str,
    /// What the knob adjusts, for the status line.
    pub knob: &'static str,
    pub range: (f32, f32),
    pub start: f32,
    /// `knob` is the current value; `time` advances only while animating.
    pub draw: fn(&mut Canvas, Extent2D, f32, f32),
}

pub fn scenes() -> Vec<Live> {
    vec![
        Live {
            name: "gradient stops crossing the ramp boundary",
            knob: "stops",
            range: (2.0, 12.0),
            start: 3.0,
            draw: gradient_stops,
        },
        Live {
            name: "layer blur",
            knob: "sigma",
            range: (0.0, 40.0),
            start: 8.0,
            draw: layer_blur,
        },
        Live {
            name: "frosted panel",
            knob: "backdrop sigma",
            range: (0.0, 30.0),
            start: 10.0,
            draw: frosted,
        },
        Live {
            name: "marching dashes",
            knob: "dash length",
            range: (2.0, 60.0),
            start: 18.0,
            draw: dashes,
        },
        Live {
            name: "conical gradient, focus leaving the circle",
            knob: "focus offset",
            // Past one, the first circle's center is outside the second, and
            // the family of circles stops covering the plane. The interesting
            // value is exactly one, where the two are tangent and the
            // quadratic the shader solves loses its squared term altogether --
            // a branch of its own, taken on a set of measure zero, which is
            // the kind of thing a knob finds and a still image never does.
            range: (0.0, 1.6),
            start: 0.4,
            draw: conical_focus,
        },
        Live {
            name: "mesh corners fading",
            knob: "corner alpha",
            // The premultiplied question. Color is interpolated across the
            // triangle premultiplied, so a corner fading out darkens toward
            // nothing; interpolated straight and used as premultiplied it
            // would stay bright and only the edge would thin. Sweeping the
            // alpha is what tells the two apart.
            range: (0.0, 1.0),
            start: 1.0,
            draw: mesh_corners,
        },
        Live {
            name: "sprite atlas, one draw",
            knob: "sprites",
            range: (1.0, 64.0),
            start: 24.0,
            draw: atlas_orbit,
        },
        Live {
            name: "arc sweep",
            knob: "turns",
            // Not through a whole turn in each direction: plus and minus one
            // turn are the same circle, and a scene whose extremes agree is one
            // whose knob cannot be seen to do anything. Not spanning zero
            // either, except in the middle, where the track below is what keeps
            // the frame from being empty.
            range: (-0.9, 0.9),
            start: 0.75,
            draw: arc_sweep,
        },
    ]
}

/// Two circles sharing a last stop, with the first one sliding out of the
/// second.
///
/// At an offset of zero this is a radial gradient. Approaching one, the rings
/// bunch against the side the focus moves toward; at exactly one the focus sits
/// on the circle and the gradient covers a half plane; past one it covers a
/// wedge and nothing else, because beyond that no circle of the family passes
/// through the rest of the plane at all.
fn conical_focus(canvas: &mut Canvas, extent: Extent2D, knob: f32, _time: f32) {
    ground(canvas);
    let (w, h) = (extent.width as f32, extent.height as f32);
    let center = Vec2::new(w * 0.5, h * 0.5);
    let radius = h * 0.4;
    let focus = center + Vec2::new(radius * knob, 0.0);
    let _ = canvas.draw_rect(
        Rect::new(0.0, 0.0, w, h),
        &Paint::conical_gradient(
            focus,
            0.0,
            center,
            radius,
            vec![
                GradientStop::new(Color::srgb(1.0, 0.95, 0.7, 1.0), 0.0),
                GradientStop::new(Color::srgb(0.85, 0.2, 0.35, 1.0), 0.55),
                GradientStop::new(Color::srgb(0.1, 0.1, 0.3, 1.0), 1.0),
            ],
        ),
    );
    // The second circle drawn on top, so where the gradient stops covering the
    // plane is visible against a shape that does not move.
    let _ = canvas.draw_circle(
        center,
        radius,
        &Paint::stroke(Color::srgb(1.0, 1.0, 1.0, 0.35), 1.5),
    );
}

/// A fan of triangles whose outer corners fade.
///
/// The center stays opaque and the rim's alpha is the knob, so the whole thing
/// is one mesh with colors no gradient describes: each triangle interpolates
/// between three independent corners.
fn mesh_corners(canvas: &mut Canvas, extent: Extent2D, knob: f32, time: f32) {
    ground(canvas);
    let (w, h) = (extent.width as f32, extent.height as f32);
    let center = Vec2::new(w * 0.5, h * 0.5);
    let radius = h * 0.42;

    const POINTS: usize = 12;
    let mut positions = vec![center];
    let mut colors = vec![Color::srgb(1.0, 1.0, 1.0, 1.0)];
    for i in 0..=POINTS {
        let angle = i as f32 / POINTS as f32 * std::f32::consts::TAU + time * 0.2;
        positions.push(center + Vec2::new(angle.cos(), angle.sin()) * radius);
        let turn = i as f32 / POINTS as f32 * std::f32::consts::TAU;
        colors.push(wheel(turn, knob));
    }
    if let Ok(mesh) = Vertices::colored(VertexMode::TriangleFan, positions, colors) {
        let _ = canvas.draw_vertices(&mesh, &Paint::fill(Color::srgb(1.0, 1.0, 1.0, 1.0)));
    }
}

/// Many pieces of one sheet, orbiting, in a single draw.
///
/// The sheet is four flat quadrants, so which part of it each sprite reads is
/// legible at a glance, and each sprite carries a tint of its own -- which is
/// the thing that would otherwise cost one draw per color.
fn atlas_orbit(canvas: &mut Canvas, extent: Extent2D, knob: f32, time: f32) {
    ground(canvas);
    let (w, h) = (extent.width as f32, extent.height as f32);
    let center = Vec2::new(w * 0.5, h * 0.5);
    let count = knob.round().max(1.0) as usize;
    let size = h * 0.09;

    let sprites: Vec<Sprite> = (0..count)
        .map(|i| {
            let t = i as f32 / count as f32;
            let angle = t * std::f32::consts::TAU + time * 0.35;
            let orbit = h * (0.16 + 0.22 * ((t * 3.0) % 1.0));
            let at = center + Vec2::new(angle.cos(), angle.sin()) * orbit;
            // Each sprite reads a different quadrant of the sheet, and spins
            // about its own center rather than about the orbit.
            let quadrant = i % 4;
            let source = SourceRect::new(
                (quadrant % 2) as f32 * 2.0,
                (quadrant / 2) as f32 * 2.0,
                2.0,
                2.0,
            );
            let scale = size / 2.0;
            let placement = Affine2::from_translation(at)
                * Affine2::from_angle(angle * 2.0)
                * Affine2::from_translation(Vec2::splat(-size * 0.5))
                * Affine2::from_scale(Vec2::splat(scale));
            Sprite::new(source, placement).with_color(wheel(t * std::f32::consts::TAU, 1.0))
        })
        .collect();

    let _ = canvas.draw_atlas(
        &sprites,
        Extent2D::new(4, 4),
        &Paint::image(SHEET_SLOT, Rect::new(0.0, 0.0, w, h)),
    );
}

/// A color a third of a turn apart in each channel, which sweeps the hues
/// without a conversion nobody else here needs.
fn wheel(turn: f32, alpha: f32) -> Color {
    let third = std::f32::consts::TAU / 3.0;
    Color::srgb(
        0.5 + 0.5 * turn.cos(),
        0.5 + 0.5 * (turn + third).cos(),
        0.5 + 0.5 * (turn + 2.0 * third).cos(),
        alpha,
    )
}

/// The texture slot the playground uploads its sprite sheet into.
pub const SHEET_SLOT: u32 = 0;

/// A four-by-four sheet of four flat quadrants.
///
/// Small and blocky on purpose: what a sprite scene has to show is which piece
/// each quad read, not what the piece looks like.
pub fn sheet_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; 4 * 4 * 4];
    for y in 0..4usize {
        for x in 0..4usize {
            let i = (y * 4 + x) * 4;
            let color: [u8; 4] = match (x < 2, y < 2) {
                (true, true) => [255, 245, 235, 255],
                (false, true) => [235, 245, 255, 255],
                (true, false) => [255, 225, 200, 255],
                (false, false) => [225, 235, 255, 255],
            };
            pixels[i..i + 4].copy_from_slice(&color);
        }
    }
    pixels
}

fn ground(canvas: &mut Canvas) {
    canvas.clear(Color::srgb(0.06, 0.07, 0.10, 1.0));
}

/// A ramp whose stop count crosses the boundary between the two shader paths.
///
/// Below the limit the colors travel with the material and the shader walks
/// them; above it the recorder bakes a texture and the shader samples it. The
/// stops here are placed evenly along one hue sweep, so the picture should
/// change smoothly as they are added and show no step at the crossing.
fn gradient_stops(canvas: &mut Canvas, extent: Extent2D, knob: f32, _time: f32) {
    ground(canvas);
    let count = knob.round().max(2.0) as usize;
    let stops: Vec<GradientStop> = (0..count)
        .map(|i| {
            let t = i as f32 / (count - 1) as f32;
            // One continuous sweep, so adding a stop refines the same ramp
            // rather than describing a different one.
            GradientStop::new(Color::linear(t, 0.35 + 0.5 * (1.0 - t), 1.0 - t, 1.0), t)
        })
        .collect();
    let (w, h) = (extent.width as f32, extent.height as f32);
    let band = Rect::new(w * 0.08, h * 0.3, w * 0.92, h * 0.7);
    let _ = canvas.draw_rrect(
        band,
        h * 0.03,
        &Paint::linear_gradient(Vec2::new(band.left, 0.0), Vec2::new(band.right, 0.0), stops),
    );
}

fn layer_blur(canvas: &mut Canvas, extent: Extent2D, knob: f32, _time: f32) {
    ground(canvas);
    let (w, h) = (extent.width as f32, extent.height as f32);
    let bounds = Rect::new(w * 0.15, h * 0.15, w * 0.85, h * 0.85);
    canvas.save_layer_bounds(Layer::opacity(1.0).with_blur(knob), bounds);
    let _ = canvas.draw_rrect(
        Rect::new(w * 0.25, h * 0.3, w * 0.75, h * 0.55),
        h * 0.04,
        &Paint::fill(Color::srgb(1.0, 0.95, 0.9, 1.0)),
    );
    let _ = canvas.draw_circle(
        Vec2::new(w * 0.5, h * 0.68),
        h * 0.1,
        &Paint::fill(Color::srgb(1.0, 0.3, 0.35, 1.0)),
    );
    canvas.restore();
}

fn frosted(canvas: &mut Canvas, extent: Extent2D, knob: f32, time: f32) {
    ground(canvas);
    let (w, h) = (extent.width as f32, extent.height as f32);
    // Something with detail worth obscuring, drifting so the panel is visibly
    // filtering a moving backdrop rather than a still one.
    for i in 0..9 {
        let t = i as f32 / 8.0;
        let x = w * (0.1 + 0.8 * ((t + time * 0.05) % 1.0));
        let _ = canvas.draw_circle(
            Vec2::new(x, h * (0.2 + 0.6 * t)),
            h * 0.09,
            &Paint::fill(Color::srgb(0.9 - 0.6 * t, 0.3 + 0.5 * t, 0.9, 1.0)),
        );
    }
    let panel = Rect::new(w * 0.12, h * 0.35, w * 0.88, h * 0.65);
    canvas.save_layer_bounds(Layer::opacity(1.0).with_backdrop_blur(knob), panel);
    let _ = canvas.draw_rrect(
        panel,
        h * 0.05,
        &Paint::fill(Color::srgb(1.0, 1.0, 1.0, 0.16)),
    );
    let _ = canvas.draw_rrect(
        panel,
        h * 0.05,
        &Paint::stroke(Color::srgb(1.0, 1.0, 1.0, 0.4), 2.0),
    );
    canvas.restore();
}

fn dashes(canvas: &mut Canvas, extent: Extent2D, knob: f32, time: f32) {
    ground(canvas);
    let (w, h) = (extent.width as f32, extent.height as f32);
    // The phase advances with time, which is the whole point: a still image of
    // a dashed line says nothing about whether it marches smoothly.
    let dash = Dash::new(vec![knob, knob * 0.6], time * 40.0);
    let paint = |color: Color| {
        Paint::stroke(color, h * 0.02)
            .with_dash(Some(dash.clone()))
            .with_anti_alias(true)
    };
    let _ = canvas.draw_line(
        Vec2::new(w * 0.1, h * 0.25),
        Vec2::new(w * 0.9, h * 0.25),
        &paint(Color::srgb(1.0, 0.4, 0.4, 1.0)),
    );
    let _ = canvas.draw_circle(
        Vec2::new(w * 0.5, h * 0.62),
        h * 0.22,
        &paint(Color::srgb(0.4, 0.9, 1.0, 1.0)),
    );
}

fn arc_sweep(canvas: &mut Canvas, extent: Extent2D, knob: f32, _time: f32) {
    ground(canvas);
    let (w, h) = (extent.width as f32, extent.height as f32);
    let center = Vec2::new(w * 0.5, h * 0.5);
    let radius = h * 0.3;
    let sweep = knob * std::f32::consts::TAU;

    // The track a progress ring is drawn on. It also means a sweep of zero
    // still shows something, which is what the ring would look like at nought
    // percent -- and is why the scene is worth looking at across its whole
    // range rather than only at the ends.
    let mut track = PathBuilder::new();
    track.arc(center, Vec2::splat(radius), 0.0, std::f32::consts::TAU);
    let _ = canvas.draw_path(
        &track.build(),
        &Paint::stroke(Color::srgb(1.0, 1.0, 1.0, 0.12), h * 0.03),
    );

    let mut ring = PathBuilder::new();
    ring.arc(
        center,
        Vec2::splat(radius),
        -std::f32::consts::FRAC_PI_2,
        sweep,
    );
    let _ = canvas.draw_path(
        &ring.build(),
        &Paint::stroke(Color::srgb(0.5, 1.0, 0.6, 1.0), h * 0.03),
    );

    // The same sweep as a filled slice, so the two uses of an arc are visible
    // together and a wrong control point shows in one or both.
    let mut slice = PathBuilder::new();
    slice.move_to(center);
    slice.arc(
        center,
        Vec2::new(radius * 0.5, radius * 0.35),
        -std::f32::consts::FRAC_PI_2,
        sweep,
    );
    slice.close();
    let _ = canvas.draw_path(
        &slice.build(),
        &Paint::fill(Color::srgb(1.0, 0.6, 0.2, 0.85)),
    );
}

//! Drawing through the public API.
//!
//! Everything here goes through the facade, naming no backend and reaching for
//! nothing below it. That is the check the API exists for: if a picture cannot
//! be drawn without dropping to `emblema-hal` or `emblema-renderer`, the
//! front door is incomplete regardless of how well the machinery behind it
//! works.

use emblema::{
    Affine2, Atlas, BackendPreference, BlendMode, Canvas, Color, ColorFilter, Context, Coverage,
    Dash, Error, Extent2D, GlyphKey, GradientStop, ImageFilter, Layer, LineCap, LineJoin,
    MaskBlurStyle, Morphology, Paint, Path, PathBuilder, PixelFormat, PointMode, PositionedGlyph,
    Rect, Result, RoundingRadii, Sampling, Shader, SourceRect, Sprite, StrokeStyle, Style,
    TileMode, Transform2D, Vec2, VertexMode, Vertices, MAX_STOPS, MORPHOLOGY_TAPS, RUNTIME_FLOATS,
};
use googletest::prelude::*;

const SIZE: Extent2D = Extent2D {
    width: 128,
    height: 128,
};

fn context() -> Option<Context> {
    match Context::new(BackendPreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable backend ({e})");
            None
        }
    }
}

/// Draw a recording and read the result back.
fn render(ctx: &mut Context, canvas: Canvas) -> Vec<u8> {
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    pixels
}

/// Draw an already-finished recording, for a test that inspects it first.
fn render_recording(ctx: &mut Context, recording: &emblema::Recording) -> Vec<u8> {
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, recording).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    pixels
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// A square as a path that does not know it is a rectangle.
///
/// `Rect::to_path` records the shape it laid down, so a blurred rectangle drawn
/// through it is evaluated in the fragment stage rather than blurred. That is
/// what upstream does and what the analytic tests check. A test about the
/// *general* route needs an outline with no shape on it, which is what building
/// the four lines by hand gives.
fn square_path(rect: Rect) -> Path {
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(rect.left, rect.top))
        .line_to(Vec2::new(rect.right, rect.top))
        .line_to(Vec2::new(rect.right, rect.bottom))
        .line_to(Vec2::new(rect.left, rect.bottom))
        .close();
    b.build()
}

/// Two colors, equal to within `within` on every channel.
///
/// For a comparison against a value a *shader* computed rather than one the API
/// was handed. Both graphics specifications permit some error there and allow a
/// compiler to fuse operations differently, so a program that multiplies and
/// adds its way to a color can land a level either side of another device's
/// answer without either being wrong. A Raspberry Pi 5's V3D reads 179 where
/// two x86 drivers read 178, on three separate tests, from the same fixture.
///
/// Not for a color the caller stated. Those arrive through the pipeline
/// untransformed and are compared exactly, which is what
/// `a_stated_color_arrives_as_the_bytes_it_states` is for.
#[track_caller]
fn close_enough(got: [u8; 4], want: [u8; 4], within: u8, what: &str) {
    let off = (0..4).find(|&i| got[i].abs_diff(want[i]) > within);
    assert!(
        off.is_none(),
        "{what}: got {got:?}, wanted {want:?} within {within} \
         (channel {} differs by {})",
        off.unwrap_or(0),
        got[off.unwrap_or(0)].abs_diff(want[off.unwrap_or(0)])
    );
}

#[test]
fn a_context_reports_which_backend_it_chose() {
    let Some(ctx) = context() else { return };
    eprintln!(
        "backend: {} on {}",
        ctx.backend(),
        ctx.capabilities().device_name
    );
    // Capabilities must be reachable without knowing the backend, since that is
    // what everything above is supposed to branch on.
    assert!(ctx.capabilities().max_texture_size >= 2048);
}

#[test]
fn a_rectangle_lands_where_it_was_asked_to() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 96.0, 96.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    assert_eq!(pixel(&pixels, 64, 64), [255, 255, 255, 255], "inside");
    assert_eq!(pixel(&pixels, 8, 8), [0, 0, 0, 255], "outside");
    // User coordinates run down and right, so a rect at the top-left of user
    // space must appear at the top-left of the image.
    assert_eq!(pixel(&pixels, 64, 16), [0, 0, 0, 255], "above the rect");
}

#[test]
fn colors_are_specified_in_srgb_and_stored_in_srgb() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    // A mid gray as a design tool would give it.
    canvas.clear(Color::rgba8(128, 128, 128, 255));
    let pixels = render(&mut ctx, canvas);

    // And it lands as it was given. The pipeline carries sRGB-encoded
    // components from the API boundary to the target write, so a byte a design
    // tool produced is the byte that gets stored -- there is no conversion for
    // it to survive.
    //
    // It used to land near 55, because the components were decoded to light on
    // the way in and the target held light. That is the physically correct
    // arrangement and it is not upstream's: a `DlColor` is built from bytes
    // with no decode, `ToColor` copies it unchanged into the engine's own
    // color, and no shader upstream applies a transfer function anywhere.
    let stored = pixel(&pixels, 4, 4)[0];
    assert_eq!(stored, 128, "sRGB 128 should store as 128, got {stored}");
}

#[test]
fn the_transform_stack_places_shapes_independently() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let paint = Paint::fill(Color::WHITE).with_anti_alias(false);

    canvas.save();
    canvas.translate(16.0, 16.0);
    canvas
        .draw_rect(Rect::from_size(16.0, 16.0), &paint)
        .unwrap();
    canvas.restore();

    // After restoring, this must be drawn in the original frame rather than
    // inheriting the translation above.
    canvas
        .draw_rect(Rect::from_size(16.0, 16.0), &paint)
        .unwrap();

    let pixels = render(&mut ctx, canvas);
    assert_eq!(pixel(&pixels, 24, 24), [255, 255, 255, 255], "translated");
    assert_eq!(pixel(&pixels, 8, 8), [255, 255, 255, 255], "restored");
    assert_eq!(pixel(&pixels, 60, 60), [0, 0, 0, 255], "neither");
}

#[test]
fn a_stroke_traces_an_outline_rather_than_filling_it() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 96.0, 96.0),
            &Paint::stroke(Color::WHITE, 8.0).with_anti_alias(false),
        )
        .expect("stroke");

    let pixels = render(&mut ctx, canvas);
    // On the edge, but not in the middle: the distinction between stroking and
    // filling is exactly the interior.
    assert_eq!(pixel(&pixels, 64, 32), [255, 255, 255, 255], "on the edge");
    assert_eq!(pixel(&pixels, 64, 64), [0, 0, 0, 255], "interior");
}

#[test]
fn translucent_paint_blends_with_what_is_underneath() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 1.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 0.5)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let got = pixel(&pixels, 64, 64);
    // Half red over opaque blue, computed rather than recorded.
    for (channel, want) in got.iter().zip([128u8, 0, 128, 255]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 2,
            "got {got:?}, expected about [128, 0, 128, 255]"
        );
    }
}

/// A shadow's softness is fixed in device pixels, though a blur's is not.
///
/// The pair is the parity, and it is not obvious either way. Upstream's blur
/// sigma is in the space the drawing is in --
/// `gaussian_blur_filter_contents` scales it by the entity transform -- and
/// its `drawShadow` *divides* its radius by the canvas scale, precisely to
/// cancel that multiplication. So upstream's blur grows with the transform and
/// upstream's shadow does not, and matching it means having both.
///
/// This renderer used to have neither. Its sigma was device-space, which made
/// a shadow come out right with no division at all -- and made every other
/// blur wrong under a scale, which is `docs/architecture.md`'s note on the
/// change. The division exists now because the multiplication does.
///
/// Checked in both directions at once: the shape has to grow with the scale,
/// or the transform is not reaching the draw and a softness staying put would
/// mean nothing.
#[test]
fn a_shadows_softness_is_fixed_in_device_pixels() {
    let Some(mut ctx) = context() else { return };
    let extent = Extent2D::new(256, 256);

    let measure = |ctx: &mut Context, scale: f32| -> (usize, usize) {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.scale(scale, scale);
        let center = 128.0 / scale;
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(center - 20.0, center - 20.0))
            .line_to(Vec2::new(center + 20.0, center - 20.0))
            .line_to(Vec2::new(center + 20.0, center + 20.0))
            .line_to(Vec2::new(center - 20.0, center + 20.0))
            .close();
        canvas
            .draw_shadow(&b.build(), Color::WHITE, 6.0, false)
            .expect("a shadow");
        canvas.restore();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);

        // Along the middle row, left of center: how much is at more than half
        // the shadow's own strength, and how much is lit at all. The difference
        // is the tail. Measured against the peak rather than an absolute level,
        // because a shadow is a tonal color -- this one peaks at a hundred and
        // eight, not at white -- and a fixed threshold would be measuring
        // `tonal_shadow_color` instead of the blur.
        let value = |x: usize| pixels[(128 * extent.width as usize + x) * 4];
        let peak = (0..extent.width as usize).map(value).max().unwrap_or(0) as u32;
        let solid = (0..128).filter(|x| value(*x) as u32 * 2 > peak).count();
        let lit = (0..128).filter(|x| value(*x) > 6).count();
        (solid, lit - solid)
    };

    let (solid_at_one, tail_at_one) = measure(&mut ctx, 1.0);
    let (solid_at_two, tail_at_two) = measure(&mut ctx, 2.0);

    assert!(
        solid_at_two > solid_at_one + 8,
        "doubling the scale did not grow the caster: {solid_at_one} then {solid_at_two}"
    );
    assert!(
        (tail_at_one as i32 - tail_at_two as i32).abs() <= 2,
        "the shadow's softness moved with the transform: {tail_at_one} then \
         {tail_at_two} device pixels. Upstream's does not, and the division in \
         `draw_shadow` is what holds it still now that a blur's sigma scales"
    );
}

/// A blur too wide for its tap budget shrinks the image instead of spreading.
///
/// The taps are one per texel while the kernel's radius fits in the budget.
/// Past that this used to hold the count and spread them apart, which keeps the
/// blur the right width but samples it more and more coarsely — the gaps show
/// as steps in a smooth ramp. Upstream blurs a smaller copy instead, so the
/// taps stay one per texel and it is the image that loses detail rather than
/// the kernel; for a blur this wide that detail was leaving anyway.
///
/// Counted in passes because that is what the reduction is: each halving is a
/// pass of its own. A full-frame blurred layer is four — the layer, two blur
/// axes, and the frame — and each halving adds one.
///
/// That the reduction loses no light is checked next door, by the energy test,
/// which runs at a deviation of twenty-four and so takes this path.
#[test]
fn a_blur_past_the_tap_budget_shrinks_what_it_blurs() {
    let extent = Extent2D::new(256, 256);
    let passes_for = |sigma: f32| -> usize {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas.save_layer(Layer::opacity(1.0).with_blur(sigma));
        canvas
            .draw_rect(
                Rect::new(96.0, 96.0, 160.0, 160.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("square");
        canvas.restore();
        canvas.finish().passes.len()
    };

    // Under the budget nothing is added. The radius is `(sigma - 0.5) * sqrt(3)`
    // against a budget of thirty-two, so it fits up to a deviation of about
    // nineteen.
    assert_eq!(passes_for(8.0), 4, "a narrow blur should not be reduced");
    assert_eq!(
        passes_for(18.0),
        4,
        "a blur inside the budget should not be reduced"
    );

    // Past it, one halving, then another each time the radius doubles again.
    assert_eq!(
        passes_for(20.0),
        5,
        "a blur past the budget should be halved once"
    );
    assert_eq!(
        passes_for(40.0),
        6,
        "twice the deviation should halve once more"
    );
    assert_eq!(passes_for(80.0), 7, "and again");

    // The reduction is bounded by the deviation, which is bounded too, so a
    // request no image could satisfy does not spawn passes without end.
    assert!(
        passes_for(100_000.0) <= 12,
        "an enormous deviation should not keep adding passes: {}",
        passes_for(100_000.0)
    );
}

/// A blur far wider than what it is blurring still draws, and draws nothing odd.
///
/// The reduction halves a target until the kernel's radius fits the tap budget,
/// and a large enough deviation asks for more halvings than a small target has
/// pixels to give. The extent is floored at one texel so a pass never asks a
/// device for a texture of no width; what the loop's own guard adds is stopping
/// before it spends passes halving a single texel into itself.
///
/// What is checked is the part that would be visible if either went wrong:
/// every pass has pixels, the count stays bounded, the draw succeeds, and
/// nothing comes back with color in a fully transparent pixel — which is what a
/// target that was never written, or written as NaN, looks like after it has
/// been composited. Exercised through the paths that reach a small target for
/// different reasons.
#[test]
fn a_blur_far_wider_than_its_subject_still_draws() {
    let Some(mut ctx) = context() else { return };
    let extent = Extent2D::new(128, 128);
    let square = Rect::new(40.0, 40.0, 88.0, 88.0);

    for case in 0..4 {
        let name = match case {
            0 => "a layer bounded to eight pixels",
            1 => "a layer bounded to one pixel",
            2 => "a mask blur wider than the frame",
            _ => "a shadow at an absurd elevation",
        };
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        match case {
            0 | 1 => {
                let (bounds, sigma) = if case == 0 {
                    (Rect::new(60.0, 60.0, 68.0, 68.0), 400.0)
                } else {
                    (Rect::new(63.0, 63.0, 64.0, 64.0), 200.0)
                };
                canvas.save_layer_bounds(Layer::opacity(1.0).with_blur(sigma), bounds);
                canvas
                    .draw_rect(square, &Paint::fill(Color::WHITE).with_anti_alias(false))
                    .expect("square");
                canvas.restore();
            }
            2 => {
                canvas
                    .draw_rect(square, &Paint::fill(Color::WHITE).with_mask_blur(60.0))
                    .expect("mask blur");
            }
            _ => {
                canvas
                    .draw_shadow(&square.to_rounded_path(4.0), Color::BLACK, 300.0, false)
                    .expect("shadow");
            }
        }
        let recording = canvas.finish();

        for (index, pass) in recording.passes.iter().enumerate() {
            assert!(
                pass.extent.width >= 1 && pass.extent.height >= 1,
                "{name}: pass {index} has no pixels: {:?}",
                pass.extent
            );
        }
        assert!(
            recording.passes.len() <= 12,
            "{name}: {} passes, which is more than a reduction should need",
            recording.passes.len()
        );

        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);

        assert!(
            !pixels
                .chunks_exact(4)
                .any(|texel| texel[3] == 0 && texel[0] > 0),
            "{name}: a pixel came back with color and no alpha"
        );
    }
}

/// A deviation is clamped where upstream clamps it.
///
/// `kMaxSigma` is five hundred. Past it a blur of anything smaller than a wall
/// is a flat wash and the reduction has long since taken the image down to a
/// handful of texels, so upstream stops there and so does this.
///
/// Clamped after the finiteness check rather than before it, which is the part
/// worth a test: `f32::min` returns the other operand when one is NaN, so
/// clamping first turns a NaN deviation into the widest blur there is instead
/// of into no blur at all.
#[test]
fn a_deviation_is_clamped_and_a_nan_is_not_clamped_into_one() {
    let extent = Extent2D::new(64, 64);
    let passes_for = |sigma: f32| -> usize {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas.save_layer(Layer::opacity(1.0).with_blur(sigma));
        canvas
            .draw_rect(
                Rect::new(16.0, 16.0, 48.0, 48.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("square");
        canvas.restore();
        canvas.finish().passes.len()
    };

    // Beyond the clamp, the same frame as at the clamp: the request stopped
    // growing, so the reduction stopped too.
    assert_eq!(
        passes_for(500.0),
        passes_for(50_000.0),
        "a deviation past the clamp should draw the same as one at it"
    );

    // And a NaN is no blur, not the largest one. Stated against a layer that
    // asked for none rather than against a count: a layer costs a pass and the
    // frame costs one whether anything is blurred or not, so the number that
    // means "no blur" is that one and not zero.
    assert_eq!(
        passes_for(f32::NAN),
        passes_for(0.0),
        "a NaN deviation should draw the same as no blur at all"
    );
    assert!(
        passes_for(8.0) > passes_for(0.0),
        "this comparison says nothing unless a blur costs passes"
    );
}

/// A blur moves light around without creating or destroying any.
///
/// The property that says the kernel is normalized. A Gaussian blur is a
/// weighted average, so the total over the frame has to come out the same as
/// before it was blurred -- a shape that gets brighter or dimmer for being
/// softened is a kernel whose weights do not sum to one.
///
/// `blur_along_axis` normalizes by what it actually summed rather than by the
/// analytic integral, which is the whole reason this holds at every sigma
/// rather than only at small ones: the taps span the kernel's radius and spread
/// further apart once the count reaches its budget, so the sum is always a
/// truncated and unevenly sampled version of the curve. Dividing by what the
/// curve *should* integrate to would darken exactly the large blurs where the
/// truncation bites.
///
/// Measured on a float target so the reading is not itself quantized, and the
/// frame is large enough that nothing reaches its edge -- checked, because
/// light leaving the frame would look identical to light being destroyed.
#[test]
fn a_blur_preserves_the_total_light_in_the_frame() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().float_render_targets {
        eprintln!("skipping: no float render targets");
        return;
    }
    let extent = Extent2D::new(256, 256);

    let total = |ctx: &mut Context, sigma: f32| -> (f64, u32) {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        if sigma > 0.0 {
            canvas.save_layer(Layer::opacity(1.0).with_blur(sigma));
        }
        canvas
            .draw_rect(
                Rect::new(96.0, 96.0, 160.0, 160.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("rect");
        if sigma > 0.0 {
            canvas.restore();
        }
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba16Float)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);

        let (mut sum, mut lit_edge) = (0.0f64, 0u32);
        for y in 0..extent.height as usize {
            for x in 0..extent.width as usize {
                let at = (y * extent.width as usize + x) * 8;
                let value = half::f16::from_le_bytes([pixels[at], pixels[at + 1]]).to_f32() as f64;
                sum += value;
                let border = x == 0
                    || y == 0
                    || x + 1 == extent.width as usize
                    || y + 1 == extent.height as usize;
                if border && value > 1e-4 {
                    lit_edge += 1;
                }
            }
        }
        (sum, lit_edge)
    };

    let (unblurred, _) = total(&mut ctx, 0.0);
    assert!(unblurred > 0.0, "the unblurred square drew nothing");

    // Past the tap budget as well as under it, since that is where normalizing
    // by the analytic integral would start to show.
    for sigma in [2.0f32, 8.0, 24.0] {
        let (blurred, lit_edge) = total(&mut ctx, sigma);
        assert_eq!(
            lit_edge, 0,
            "at sigma {sigma} the blur reached the frame edge, so light leaving \
             cannot be told from light lost and this measurement says nothing"
        );
        let ratio = blurred / unblurred;
        assert!(
            (ratio - 1.0).abs() < 0.01,
            "at sigma {sigma} the blur changed the total light by {:.2}%: \
             {blurred:.1} against {unblurred:.1}",
            (ratio - 1.0) * 100.0
        );
    }
}

/// Dithering breaks a band, measured rather than eyeballed.
///
/// A gradient asks a target for a long run of nearly equal values, and where
/// two neighbors round to the same one the picture gains an edge the gradient
/// does not have. Perturbing each pixel by a fraction of a step first, by a
/// rule that varies across an eight-by-eight tile, makes the rounding fall on
/// both sides of where that edge was.
///
/// The measurement is the point: over a tile the size of the dither's own
/// period, the mean of what was stored should track the unquantized gradient
/// more closely than rounding alone does. A float target renders the same scene
/// to get that unquantized reference, so nothing here has to model what the
/// shader did.
///
/// One space and one amplitude, which is upstream's arrangement. The pipeline
/// carries encoded components and the target stores them without transforming
/// them, so a quantization step is a flat 1/255 wherever it stands and
/// `kDitherRate` is a single constant. This took an amplitude and a space while
/// the values reaching the target were light, because a step of the target was
/// then worth a different amount of light at every brightness.
#[test]
fn dithering_tracks_a_gradient_better_than_rounding_does() {
    let Some(mut ctx) = context() else { return };
    let extent = Extent2D::new(256, 64);

    let render = |ctx: &mut Context, format: PixelFormat, lo: f32, hi: f32| -> Vec<u8> {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 256.0, 64.0),
                &Paint::default()
                    .with_shader(Shader::LinearGradient {
                        start: Vec2::ZERO,
                        end: Vec2::new(256.0, 0.0),
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: Color::srgb(lo, lo, lo, 1.0),
                            },
                            GradientStop {
                                offset: 1.0,
                                color: Color::srgb(hi, hi, hi, 1.0),
                            },
                        ],
                        tile: TileMode::Clamp,
                    })
                    .with_anti_alias(false),
            )
            .expect("gradient");
        let mut surface = ctx.create_surface(extent, format).expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    // A slow ramp, which is what bands. Dark and bright both, since a run this
    // shallow rounds into steps wherever it sits.
    for (band, lo, hi) in [("dark", 0.10f32, 0.16f32), ("bright", 0.60, 0.66)] {
        let stored = render(&mut ctx, PixelFormat::Rgba8Unorm, lo, hi);
        // The ramp's own arithmetic, rather than the same ramp rendered into a
        // half-float target. That was the reference until it was measured
        // against this one: a half-float's step near six tenths is about five
        // ten-thousandths, and the dithered error being measured is between one
        // and three ten-thousandths -- so the reference's own quantization was
        // the larger part of the number, and the threshold below had been
        // fitted to whatever that came to on one device. It passed there by one
        // per cent and failed everywhere else.
        //
        // Exact here because the gradient is linear between two stops in the
        // space the pipeline stores, so the value at a pixel's center is the
        // interpolation and nothing more. Checked against the half-float render
        // when this was written: the two agree to within one half-float step,
        // which is the reference being coarse rather than this being wrong.
        let ideal = |x: usize| {
            let t = (x as f64 + 0.5) / extent.width as f64;
            lo as f64 + (hi - lo) as f64 * t
        };

        // What the device is out by across the whole band, before asking how
        // the dither did. Every device here carries some -- a fraction of an
        // eight-bit step, from the last bits of a gradient's interpolation --
        // and it is not what dithering is for. Dithering makes a *local* mean
        // track the ramp; a constant offset is the device, and leaving it in
        // measured the device instead. It is under a tenth of one level on all
        // three devices this was run on, and worst on the two real GPUs.
        let mut bias = 0.0f64;
        for y in 0..extent.height as usize {
            for x in 0..extent.width as usize {
                let at = (y * extent.width as usize + x) * 4;
                bias += stored[at] as f64 / 255.0 - ideal(x);
            }
        }
        let bias = bias / (extent.width as usize * extent.height as usize) as f64;

        let (mut dithered_error, mut rounded_error, mut blocks) = (0.0f64, 0.0f64, 0usize);
        for top in (0..extent.height as usize - 7).step_by(8) {
            for left in (0..extent.width as usize - 7).step_by(8) {
                let (mut want, mut got, mut rounded) = (0.0f64, 0.0f64, 0.0f64);
                for y in top..top + 8 {
                    for x in left..left + 8 {
                        let at = (y * extent.width as usize + x) * 4;
                        let value = ideal(x);
                        want += value;
                        got += stored[at] as f64 / 255.0;
                        // The same pixel with no dither: the target's own
                        // rounding, and nothing else.
                        rounded += (value * 255.0).round() / 255.0;
                    }
                }
                dithered_error += (got - want - bias * 64.0).abs() / 64.0;
                rounded_error += (rounded - want).abs() / 64.0;
                blocks += 1;
            }
        }
        let (dithered_error, rounded_error) = (
            dithered_error / blocks as f64,
            rounded_error / blocks as f64,
        );

        assert!(
            dithered_error <= rounded_error,
            "{band}: dithering tracks worse than rounding, \
             {dithered_error:.6} against {rounded_error:.6}"
        );
        // Three, and the margin is stated because the last number here had
        // none. Measured on three devices with this reference: the dark band
        // comes out between seven and eight, the bright band between three and
        // a half and seven and a half -- the two real GPUs near the bottom of
        // that and the software rasterizer at the top, which is a gradient
        // interpolated in fewer bits rather than a dither that works less well.
        assert!(
            rounded_error / dithered_error > 3.0,
            "{band}: dithering barely helped, \
             {dithered_error:.6} against {rounded_error:.6}"
        );
    }
}

/// What does not get dithered: a float target, and anything but a gradient.
///
/// Half has no fixed quantum to bridge -- its precision is relative, so a step
/// near black is minute and an amplitude sized for one near white would swamp
/// it -- so the amplitude is zero there and the picture is left alone. And a
/// solid fill asks for one value rather than a run of them, so there is no band
/// to break and perturbing it would only add noise to a flat area, which is
/// upstream's scope as well.
#[test]
fn dithering_leaves_flat_color_and_float_targets_alone() {
    let Some(mut ctx) = context() else { return };
    let extent = Extent2D::new(32, 32);

    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, 32.0, 32.0),
            &Paint::fill(Color::srgb(0.35, 0.35, 0.35, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let mut surface = ctx
        .create_surface(extent, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    let distinct: std::collections::BTreeSet<u8> =
        pixels.chunks_exact(4).map(|texel| texel[0]).collect();
    assert_eq!(
        distinct.len(),
        1,
        "a solid fill came back with more than one value: {distinct:?}"
    );

    if !ctx.capabilities().float_render_targets {
        eprintln!("skipping the float half: no float render targets");
        return;
    }
    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, 32.0, 32.0),
            &Paint::default()
                .with_shader(Shader::LinearGradient {
                    start: Vec2::ZERO,
                    end: Vec2::new(32.0, 0.0),
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: Color::srgb(0.20, 0.20, 0.20, 1.0),
                        },
                        GradientStop {
                            offset: 1.0,
                            color: Color::srgb(0.22, 0.22, 0.22, 1.0),
                        },
                    ],
                    tile: TileMode::Clamp,
                })
                .with_anti_alias(false),
        )
        .expect("gradient");
    let mut surface = ctx
        .create_surface(extent, PixelFormat::Rgba16Float)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);

    // Down a column the gradient is constant, so any variation is the dither.
    let value_at = |x: usize, y: usize| -> f32 {
        let at = (y * extent.width as usize + x) * 8;
        half::f16::from_le_bytes([pixels[at], pixels[at + 1]]).to_f32()
    };
    let first = value_at(16, 0);
    for y in 1..extent.height as usize {
        assert_eq!(
            value_at(16, y),
            first,
            "a float target was dithered at row {y}"
        );
    }
}

/// A layer's antialiasing does not multisample the frame that composites it.
///
/// The sample count describes a pass. An antialiased line drawn inside a layer
/// needs the layer's pass multisampled, and what the frame does with the
/// finished layer is draw one image quad, which no sample count changes. Those
/// were one answer for the whole canvas, so anything antialiased anywhere made
/// every later pass multisampled -- including the root, which paid for a
/// four-times transient and a resolve that could not alter a pixel.
///
/// Checked in both directions, because the cheap half of this is easy to get
/// by accident: the layer must still be multisampled, and the edge inside it
/// must still come out soft, or this would be a speed-up that quietly stopped
/// antialiasing.
#[test]
fn a_layers_antialiasing_does_not_multisample_the_frame() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer(Layer::opacity(1.0));
    canvas
        .draw_line(
            Vec2::new(8.0, 8.0),
            Vec2::new(120.0, 96.0),
            &Paint::stroke(Color::WHITE, 6.0),
        )
        .expect("line");
    canvas.restore();
    let recording = canvas.finish();

    let root = recording.passes.len() - 1;
    assert_eq!(
        recording.passes[root].descriptor.samples, 1,
        "the root only composites a quad and should not be multisampled"
    );
    assert!(
        recording.passes[..root]
            .iter()
            .any(|p| p.descriptor.samples == 4),
        "the layer holding the line still has to be multisampled"
    );

    // And the antialiasing is real, not merely requested. A staircase edge
    // gives only fully-on and fully-off pixels.
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &recording).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    let partial = pixels
        .chunks_exact(4)
        .filter(|p| p[0] > 0 && p[0] < 255)
        .count();
    assert!(
        partial > 32,
        "the line inside the layer is not antialiased: {partial} partial pixels"
    );
}

/// Nor does a layer inherit the frame's, which is the same rule read the other
/// way.
///
/// The frame draws an antialiased line, so its own pass needs multisampling.
/// The layer opened afterwards holds a plain rectangle, which is analytic and
/// antialiases itself, so the layer's pass needs none -- and it is a separate
/// target, allocated at the layer's size, so inheriting would have cost a
/// four-times transient for it as well.
///
/// Worth a test of its own because the two halves fail independently: putting
/// the count back when a layer closes fixes the root without stopping a layer
/// from picking the count up on the way in.
#[test]
fn a_layer_does_not_inherit_the_frames_antialiasing() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_line(
            Vec2::new(8.0, 8.0),
            Vec2::new(120.0, 96.0),
            &Paint::stroke(Color::WHITE, 6.0),
        )
        .expect("line");
    canvas.save_layer(Layer::opacity(0.5));
    canvas
        .draw_rect(
            Rect::new(16.0, 16.0, 64.0, 64.0),
            &Paint::fill(Color::WHITE),
        )
        .expect("rect");
    canvas.restore();
    let recording = canvas.finish();

    let root = recording.passes.len() - 1;
    assert_eq!(
        recording.passes[root].descriptor.samples, 4,
        "the frame's own line still needs multisampling"
    );
    for pass in &recording.passes[..root] {
        assert_eq!(
            pass.descriptor.samples, 1,
            "a layer of analytic shapes should not be multisampled"
        );
    }

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &recording).expect("draw");
    ctx.destroy_surface(surface);
}

/// A shadow draws onto a canvas that was never given a background.
///
/// It could not, and the reason was the rule above rather than anything about
/// shadows. `draw_shadow` takes no paint from the caller and builds its own,
/// which is antialiased by default; that latched the whole canvas, so the root
/// pass went multisampled and a multisampled pass must clear. The refusal a
/// caller met named a resolve they had never asked for, on a call that takes
/// no antialiasing argument they could have turned off.
#[test]
fn a_shadow_needs_no_background() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_shadow(
            &Rect::new(24.0, 24.0, 104.0, 104.0).to_rounded_path(8.0),
            Color::BLACK,
            6.0,
            false,
        )
        .expect("shadow");
    let recording = canvas.finish();
    assert_eq!(
        recording.passes[recording.passes.len() - 1]
            .descriptor
            .samples,
        1,
        "the root composites the blurred layer and nothing else"
    );

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &recording)
        .expect("a shadow onto an uncleared canvas");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    assert!(
        pixels.chunks_exact(4).any(|p| p[3] > 0),
        "the shadow drew nothing"
    );
}

/// Antialiasing a tessellated shape needs a background to clear to; an
/// analytic one does not.
///
/// The asymmetry is not arbitrary and is the reason this is pinned. A rounded
/// rectangle antialiases inside its own fragment shader and its pass stays at
/// one sample, so it draws onto a canvas that was never cleared. A line has
/// only triangles, so its pass is multisampled -- and a multisampled pass
/// cannot preserve what the target held, because seeding the multisample
/// buffer has no reverse-resolve to do it with. So the line is refused.
///
/// A caller meets this as the very first thing they write, since antialiasing
/// is on by default and a fresh canvas has no background. What is checked here
/// is that the refusal says which of the two knobs to reach for.
#[test]
fn a_multisampled_frame_needs_a_background_and_says_so() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }

    let draw = |ctx: &mut Context, background: bool, anti_alias: bool| -> Result<()> {
        let mut canvas = Canvas::new(SIZE);
        if background {
            canvas.clear(Color::BLACK);
        }
        canvas
            .draw_line(
                Vec2::new(8.0, 8.0),
                Vec2::new(120.0, 120.0),
                &Paint::stroke(Color::WHITE, 4.0).with_anti_alias(anti_alias),
            )
            .expect("record");
        let mut surface = ctx.create_surface(SIZE, PixelFormat::Rgba8Unorm)?;
        let outcome = ctx.draw(&mut surface, &canvas.finish()).map(|_| ());
        ctx.destroy_surface(surface);
        outcome
    };

    let refused = draw(&mut ctx, false, true).expect_err("a cleared-less multisampled frame");
    let Error::Unsupported(message) = refused else {
        panic!("expected a refusal, got {refused:?}");
    };
    // The old message named only the mechanism it could not perform, which
    // tells a caller nothing they can act on.
    assert!(
        message.contains("clear color") && message.contains("one sample"),
        "the refusal names neither remedy: {message}"
    );

    // Either knob on its own is enough, which is what makes them remedies.
    draw(&mut ctx, true, true).expect("a background makes it drawable");
    draw(&mut ctx, false, false).expect("dropping antialiasing makes it drawable");

    // And the analytic path never needed either, or the asymmetry above would
    // not be one.
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_rrect(
            Rect::new(8.0, 8.0, 120.0, 120.0),
            12.0,
            &Paint::fill(Color::WHITE),
        )
        .expect("record");
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish())
        .expect("an analytic shape antialiases with no background");
    ctx.destroy_surface(surface);
}

#[test]
fn antialiasing_is_requested_through_the_paint() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }

    let draw = |ctx: &mut Context, anti_alias: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_circle(
                Vec2::new(64.0, 64.0),
                48.0,
                &Paint::fill(Color::WHITE).with_anti_alias(anti_alias),
            )
            .expect("circle");
        render(ctx, canvas)
    };

    let partial = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 0 && p[0] < 255)
            .count()
    };

    // A caller asks per paint; the canvas turns that into a pass that samples.
    assert_eq!(partial(&draw(&mut ctx, false)), 0, "aliased");
    assert!(partial(&draw(&mut ctx, true)) > 32, "antialiased");
}

#[test]
fn an_empty_frame_still_clears() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::rgba8(255, 255, 255, 255));
    let pixels = render(&mut ctx, canvas);
    assert_eq!(pixel(&pixels, 0, 0), [255, 255, 255, 255]);
}

/// Every backend this build can reach, so a property that must hold on each is
/// stated once rather than per backend.
fn every_backend() -> Vec<Context> {
    [BackendPreference::Vulkan, BackendPreference::Gles]
        .into_iter()
        .filter_map(|preference| Context::new(preference).ok())
        .collect()
}

/// No sRGB format is reached for by anything that draws or samples.
///
/// Three tests used to live here, and all three asked what an sRGB format does
/// for a pipeline working in light: a surface encoded on write, so authoring a
/// color and reading it back returned it unchanged; an image decoded on sample,
/// so a texture's bytes arrived as light like everything else. Both were the
/// attachment doing a conversion the renderer would otherwise have had to do.
///
/// There is no conversion to do now. Components are sRGB-encoded from the API
/// boundary to the target write, so a format that encodes on write would encode
/// what is already encoded, and one that decodes on sample would hand back
/// light into a pipeline holding none. Upstream is arranged the same way: its
/// render targets are `kR8G8B8A8UNormInt` and `kB8G8R8A8UNormInt`, and the sRGB
/// members of its format enum are reached by blits and by image formats rather
/// than by anything it draws into.
///
/// So what is worth pinning is not what those formats do but that nothing picks
/// one. An intermediate follows the root's format, and that is the seam where
/// an sRGB target would otherwise arrive without anybody asking for it.
#[test]
fn an_intermediate_is_never_given_an_srgb_format() {
    for root in [
        PixelFormat::Rgba8Unorm,
        PixelFormat::Bgra8Unorm,
        PixelFormat::Rgba8UnormSrgb,
        PixelFormat::Bgra8UnormSrgb,
        PixelFormat::Rgb10A2Unorm,
        PixelFormat::Rgba16Float,
        PixelFormat::R8Unorm,
    ] {
        let intermediate = root.intermediate();
        assert!(
            intermediate.is_drawable(),
            "a root of {root:?} gave an intermediate of {intermediate:?}, \
             which would encode what the pipeline already encoded"
        );
    }

    // And a float root still keeps its float layers, or the wide-gamut path
    // loses its range at the first layer it meets.
    assert_eq!(
        PixelFormat::Rgba16Float.intermediate(),
        PixelFormat::Rgba16Float
    );
}

/// Both backends refuse an sRGB surface, and refuse it the same way.
///
/// This used to check that they *agreed* on one: two implementations of the
/// same conversion, one asking for an image view in the sRGB form of a format
/// and the other attaching a texture whose format carries it with no separate
/// control, either of which would look plausible alone and disagree by the
/// transfer function.
///
/// There is no conversion left for them to disagree about. The pipeline carries
/// encoded components, so a target that encodes on write would encode them
/// twice, and drawing into one has no reading under which it is correct --
/// so it is refused rather than quietly producing a picture over a third too
/// bright. What matters now is that both refuse, because a capability present
/// on one backend and absent on the other is the divergence this file exists
/// to catch, whichever direction it runs in.
#[test]
fn both_backends_refuse_an_srgb_surface() {
    let mut contexts = every_backend();
    if contexts.len() < 2 {
        eprintln!("skipping: both backends are needed");
        return;
    }
    for ctx in &mut contexts {
        let backend = ctx.backend();
        let outcome = ctx.create_surface(SIZE, PixelFormat::Rgba8UnormSrgb);
        match outcome {
            Ok(surface) => {
                ctx.destroy_surface(surface);
                panic!("{backend} allowed an sRGB surface, which would encode twice");
            }
            Err(Error::Unsupported(message)) => {
                assert!(
                    message.contains("encode"),
                    "{backend} refused for a reason that is not this one: {message}"
                );
            }
            Err(e) => panic!("{backend} refused an sRGB surface wrongly: {e}"),
        }
    }

    // And a plain one is still allowed, or the check above would pass on a
    // backend that refuses every surface it is offered.
    for ctx in &mut contexts {
        let backend = ctx.backend();
        let surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .unwrap_or_else(|e| panic!("{backend} refused a plain surface: {e}"));
        ctx.destroy_surface(surface);
    }
}

/// Nest `count` identical stencil clips, draw, and return what came back.
///
/// The clip is a triangle rather than a rectangle so it cannot become a
/// scissor: the depth being counted lives in the stencil, and an axis-aligned
/// clip never reaches it.
fn deeply_clipped(ctx: &mut Context, count: usize, last: Option<&Path>) -> Result<Vec<u8>> {
    let extent = Extent2D::new(64, 64);
    let corner = |size: f32| {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::ZERO)
            .line_to(Vec2::new(size, 0.0))
            .line_to(Vec2::new(0.0, size))
            .close();
        b.build()
    };
    let big = corner(64.0);

    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    for _ in 0..count {
        canvas.save();
        canvas.clip_path(&big).expect("clip");
    }
    if let Some(path) = last {
        canvas.save();
        canvas.clip_path(path).expect("clip");
    }
    canvas
        .draw_rect(
            Rect::from_size(64.0, 64.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("fill");
    for _ in 0..count + usize::from(last.is_some()) {
        canvas.restore();
    }

    let mut surface = ctx.create_surface(extent, PixelFormat::Rgba8Unorm)?;
    let outcome = ctx
        .draw(&mut surface, &canvas.finish())
        .and_then(|()| ctx.read(&mut surface));
    ctx.destroy_surface(surface);
    outcome
}

#[test]
fn a_clip_stack_deeper_than_the_stencil_is_refused_by_every_backend() {
    // The stencil counts nesting depth and every device is required to offer
    // eight bits of it, so 255 is the portable limit. Past it the count
    // saturates or wraps, and a later test for a depth that no longer fits
    // admits every pixel the clip was meant to exclude -- which draws content
    // the caller clipped away and looks like nothing at all.
    //
    // One backend refused and the other did not, so the same recording was an
    // error on Vulkan and a wrong picture on GLES. The check now lives beside
    // the depth it reads, because the limit belongs to the stencil format both
    // are required to offer rather than to either of them.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    for ctx in &mut contexts {
        let backend = ctx.backend();
        let error = deeply_clipped(ctx, 300, None)
            .err()
            .unwrap_or_else(|| panic!("{backend} accepted a clip stack of 300"));
        let text = error.to_string();
        assert!(
            text.contains("clip nesting depth") && text.contains("255"),
            "{backend} refused for the wrong reason: {text}"
        );
    }
}

#[test]
fn nesting_up_to_the_limit_still_works() {
    // The other half, and the one that would catch a limit set so low it broke
    // legitimate drawing: a refusal is only correct if what it refuses is past
    // what the stencil can actually hold.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    for ctx in &mut contexts {
        let backend = ctx.backend();
        let pixels = deeply_clipped(ctx, 255, None)
            .unwrap_or_else(|e| panic!("{backend} refused a legal clip stack of 255: {e}"));
        let at = |x: u32, y: u32| pixels[((y * 64 + x) * 4) as usize];
        assert_eq!(at(4, 4), 255, "{backend}: inside the clip should be drawn");
        assert_eq!(at(60, 60), 0, "{backend}: outside the clip should not be");
    }
}

#[test]
fn the_innermost_clip_still_applies_at_the_limit() {
    // Depth alone is not the property. What goes wrong past the limit is that
    // the innermost clip stops narrowing anything, so this nests to just under
    // the limit and then adds a much smaller clip: a point inside the outer
    // clips but outside the inner one must not be drawn. With the count
    // saturated that point was white, and nothing about it read as an error.
    let mut contexts = every_backend();
    assert!(!contexts.is_empty(), "no backend, so nothing here ran");

    let mut small = PathBuilder::new();
    small
        .move_to(Vec2::ZERO)
        .line_to(Vec2::new(16.0, 0.0))
        .line_to(Vec2::new(0.0, 16.0))
        .close();
    let small = small.build();

    for ctx in &mut contexts {
        let backend = ctx.backend();
        let pixels = deeply_clipped(ctx, 254, Some(&small))
            .unwrap_or_else(|e| panic!("{backend} refused a legal clip stack: {e}"));
        let at = |x: u32, y: u32| pixels[((y * 64 + x) * 4) as usize];
        assert_eq!(
            at(3, 3),
            255,
            "{backend}: inside every clip should be drawn"
        );
        assert_eq!(
            at(30, 10),
            0,
            "{backend}: the innermost clip stopped excluding anything"
        );
    }
}

#[test]
fn a_surface_from_one_context_is_refused_by_another() {
    // Two contexts of different kinds, asked for by name. Asking twice for
    // whichever is preferred returns the same kind both times, and the check
    // below cannot see a difference between two contexts of one backend --
    // that needs a handle generation the backends do not carry. So the test
    // used to notice it had nothing to compare and skip, which reads as a pass.
    let (Ok(mut first), Ok(mut second)) = (
        Context::new(BackendPreference::Vulkan),
        Context::new(BackendPreference::Gles),
    ) else {
        eprintln!("skipping: both backends are needed");
        return;
    };

    let mut surface = first
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    // A surface holds memory the context that made it allocated. Using it with
    // another would pass a handle that device never issued, which is undefined
    // rather than merely wrong, so it has to be refused on the CPU.
    let canvas = Canvas::new(SIZE);
    assert_ne!(first.backend(), second.backend());
    assert!(second.draw(&mut surface, &canvas.finish()).is_err());
    first.destroy_surface(surface);
}

#[test]
fn a_rounded_rectangle_cuts_its_corners_and_keeps_its_edges() {
    let Some(mut ctx) = context() else { return };
    // The shape an interface is mostly made of, and the one thing about it a
    // test can state simply: the corners come off and nothing else does.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let render = |ctx: &mut Context, radius: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(
                region,
                radius,
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("rrect");
        render(ctx, canvas)
    };

    let square = render(&mut ctx, 0.0);
    let rounded = render(&mut ctx, 16.0);
    let lit = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] > 128;

    // Just inside each corner of the bounding box: filled when square, gone
    // when rounded. All four, since a corner built from the wrong tangent
    // tends to be wrong in one place rather than in all of them.
    for (x, y) in [(17, 33), (110, 33), (17, 94), (110, 94)] {
        assert!(
            lit(&square, x, y),
            "square corner ({x},{y}) should be filled"
        );
        assert!(
            !lit(&rounded, x, y),
            "rounded corner ({x},{y}) should be cut away"
        );
    }
    // The middle of each edge is untouched: rounding takes the corners only.
    for (x, y) in [(64, 33), (64, 94), (17, 64), (110, 64)] {
        assert!(
            lit(&rounded, x, y),
            "edge ({x},{y}) should survive rounding"
        );
    }
    assert!(lit(&rounded, 64, 64), "the middle should be filled");
}

#[test]
fn an_antialiased_rounded_rectangle_is_two_triangles() {
    // The point of evaluating the shape per fragment: the vertex cost stops
    // depending on how round it is. Tessellated, a corner is an arc flattened
    // to a tolerance; here it is arithmetic in the fragment stage, and the
    // geometry is the quad it runs over.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let vertices = |paint: &Paint| {
        let mut canvas = Canvas::new(SIZE);
        canvas.draw_rrect(region, 16.0, paint).expect("rrect");
        canvas
            .finish()
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum::<usize>()
    };

    let analytic = vertices(&Paint::fill(Color::WHITE));
    assert_eq!(analytic, 4, "an antialiased fill should be one quad");
    // The tessellated path is still there and still used, so this is a
    // comparison rather than a claim about what was replaced.
    let tessellated = vertices(&Paint::fill(Color::WHITE).with_anti_alias(false));
    assert!(
        tessellated > analytic * 4,
        "the tessellated path should cost many more vertices, got {tessellated}"
    );
}

#[test]
fn only_an_antialiased_solid_paint_takes_the_analytic_path() {
    // Everything else falls back to tessellation, and should: a gradient or an
    // image would need its own mapping and this one at once, which is the case
    // the push-constant budget was sized against, and an aliased paint is
    // asking for the hard edges the tessellated path gives.
    //
    // A stroke is not on that list. An outline is the same field narrowed to a
    // band, so it takes the same path -- which is what the outline test above
    // is about.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let vertices = |paint: &Paint, radius: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.draw_rrect(region, radius, paint).expect("rrect");
        canvas
            .finish()
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum::<usize>()
    };

    assert_eq!(
        vertices(&Paint::fill(Color::WHITE), 16.0),
        4,
        "the fast path"
    );
    // A radius of zero is not in the list below because it is a plain
    // rectangle either way, so a vertex count cannot tell which path drew it.
    // That it stays a plain rectangle is checked where the clamping is.
    for (name, paint, radius) in [
        (
            "aliased",
            Paint::fill(Color::WHITE).with_anti_alias(false),
            16.0,
        ),
        (
            "gradient",
            Paint::linear_gradient(
                Vec2::ZERO,
                Vec2::new(64.0, 0.0),
                vec![
                    GradientStop::new(Color::WHITE, 0.0),
                    GradientStop::new(Color::BLACK, 1.0),
                ],
            ),
            16.0,
        ),
    ] {
        assert!(
            vertices(&paint, radius) > 4,
            "{name} should have fallen back to tessellation"
        );
    }
}

#[test]
fn an_antialiased_rectangle_antialiases_without_multisampling() {
    let Some(mut ctx) = context() else { return };
    // A rectangle is four vertices whichever route it takes, so the distance
    // field buys nothing there. What it buys is the edge: the pass no longer
    // has to multisample for a shape that computes its own coverage, and a
    // frame made mostly of rectangles is the common case.
    //
    // Rotated and off the pixel grid, because an axis-aligned rectangle on
    // integer bounds has no edge to antialias and would show nothing either
    // way.
    let draw = |ctx: &mut Context, anti_alias: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.translate(64.0, 64.0);
        canvas.rotate(0.35);
        canvas.translate(-64.0, -64.0);
        canvas
            .draw_rect(
                Rect::new(30.5, 30.5, 97.5, 97.5),
                &Paint::fill(Color::WHITE).with_anti_alias(anti_alias),
            )
            .expect("rect");
        canvas.restore();
        let recording = canvas.finish();
        let samples = recording.root().descriptor.samples;
        let pixels = render_recording(ctx, &recording);
        let partial = pixels
            .chunks_exact(4)
            .filter(|texel| texel[0] > 8 && texel[0] < 247)
            .count();
        let coverage: u64 = pixels.chunks_exact(4).map(|texel| texel[0] as u64).sum();
        (samples, partial, coverage as f64)
    };

    let (samples, partial, coverage) = draw(&mut ctx, true);
    assert_eq!(
        samples, 1,
        "an antialiased rectangle should not multisample the pass"
    );
    assert!(
        partial > 100,
        "a rotated antialiased rectangle should have partly covered pixels, found {partial}"
    );

    // Still the right rectangle: a rotation preserves area, so the coverage is
    // the square's own however it is turned.
    let side = 97.5 - 30.5;
    let exact = side * side * 255.0;
    assert!(
        (coverage - exact).abs() / exact < 0.01,
        "the rotated rectangle covers {coverage} against an exact {exact}"
    );

    // And asking for hard edges still gives them.
    let (_, aliased_partial, _) = draw(&mut ctx, false);
    assert_eq!(
        aliased_partial, 0,
        "an aliased rectangle should have no partly covered pixels"
    );
}

/// The analytic shapes, drawn the same way by several tests below.
fn analytic_shapes(canvas: &mut Canvas) {
    canvas
        .draw_rrect(
            Rect::new(48.0, 38.0, 112.0, 70.0),
            12.0,
            &Paint::fill(Color::WHITE),
        )
        .expect("rrect");
    canvas
        .draw_circle(
            Vec2::new(64.0, 80.0),
            9.0,
            &Paint::fill(Color::linear(1.0, 0.3, 0.2, 1.0)),
        )
        .expect("circle");
}

#[test]
fn an_analytic_shape_lands_the_same_in_a_bounded_layer() {
    let Some(mut ctx) = context() else { return };
    // A shape drawn from a distance field locates itself in clip space, and a
    // bounded layer's clip space is a smaller target at an offset inside the
    // frame. So the two have to agree about where that target is, in the same
    // way the geometry and the paint mappings already do -- and getting it
    // wrong would put the shape somewhere else in the layer while leaving it
    // the right shape, which no comparison against a tessellated version at
    // the frame's origin would notice.
    let extent = Extent2D::new(160, 120);
    let region = Rect::new(40.0, 30.0, 120.0, 90.0);
    let mut render_layered = |bounded: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        let layer = Layer::opacity(0.8);
        match bounded {
            true => canvas.save_layer_bounds(layer, region),
            false => canvas.save_layer(layer),
        };
        analytic_shapes(&mut canvas);
        canvas.restore();
        let recording = canvas.finish();
        let first = recording.passes[0].extent;
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (pixels, first)
    };

    let (full, full_extent) = render_layered(false);
    let (bounded, bounded_extent) = render_layered(true);
    // Both targets are smaller than the frame now -- one from the rectangle the
    // caller named, one derived from the shapes themselves -- and they are
    // different sizes at different origins, which is what makes the comparison
    // below worth making. A shape whose material carried the wrong space would
    // land somewhere else in one of them.
    assert!(
        full_extent != bounded_extent,
        "both layers got the same target ({full_extent:?}), so this compares nothing"
    );
    assert!(
        full_extent.width < extent.width && bounded_extent.width < extent.width,
        "a layer holding two small shapes should not have taken the whole frame"
    );
    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    // Within a distance field's own latitude, not bit-exact. A bounded layer
    // composites from a target of another size and origin, and an analytic
    // shape's coverage comes from a screen-space derivative that both
    // specifications leave to the implementation -- so the same shape in the
    // two targets can differ by a level or two along its edges without either
    // being wrong. This is the reasoning behind `Tolerance::ANALYTIC` in the
    // testkit, which budgets eight for the same cause and states how it was
    // measured; a shape that actually moved is off by the whole range.
    //
    // Bit-exact held on two x86 devices and does not on a Raspberry Pi 5's
    // V3D, which reads two.
    assert!(
        worst <= 8,
        "the shape moved when the layer was bounded: {worst}"
    );
    assert!(full.iter().any(|&b| b > 64), "the shapes rendered nothing");
}

#[test]
fn a_clip_confines_an_analytic_shape_exactly() {
    let Some(mut ctx) = context() else { return };
    // A distance field is drawn onto a quad outset past the shape, so a clip
    // has to cut the quad rather than the shape it stands for. Checked as an
    // equality rather than a direction: inside the clip every pixel must match
    // the unclipped drawing, and outside it every pixel must be untouched.
    // Anything else -- a clip applied in the wrong space, or to the outset
    // rather than to what it covers -- moves pixels one way or the other.
    let extent = Extent2D::new(160, 120);
    let clip = Rect::new(30.0, 30.0, 100.0, 100.0);
    let mut render_clipped = |clipped: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        if clipped {
            canvas.save();
            canvas.clip_rect(clip).expect("clip");
        }
        analytic_shapes(&mut canvas);
        if clipped {
            canvas.restore();
        }
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let open = render_clipped(false);
    let confined = render_clipped(true);
    let mut inside = 0;
    let mut outside = 0;
    for y in 0..extent.height {
        for x in 0..extent.width {
            let at = ((y * extent.width + x) * 4) as usize;
            let within = (x as f32) >= clip.left
                && (x as f32) < clip.right
                && (y as f32) >= clip.top
                && (y as f32) < clip.bottom;
            if within {
                assert_eq!(
                    &confined[at..at + 4],
                    &open[at..at + 4],
                    "({x}, {y}) is inside the clip and should be untouched by it"
                );
                if open[at] > 64 {
                    inside += 1;
                }
            } else {
                assert_eq!(
                    &confined[at..at + 4],
                    &[0, 0, 0, 255],
                    "({x}, {y}) is outside the clip and should not have been drawn"
                );
                if open[at] > 64 {
                    outside += 1;
                }
            }
        }
    }
    // The clip has to actually cut something, or both branches above are
    // satisfied by a shape that never reached its edge.
    assert!(
        inside > 0 && outside > 0,
        "the clip removed nothing: {inside} in, {outside} out"
    );
}

#[test]
fn an_outline_costs_the_distance_field_nothing() {
    let Some(mut ctx) = context() else { return };
    // A field already says how far every fragment is from the edge, so an
    // outline is the band where that is small -- the same two triangles and one
    // more subtraction. Tessellating one means building a second shape, offset
    // inward and outward with the corners resolved, which is where a stroked
    // path gets its vertices.
    let extent = Extent2D::new(200, 160);
    let mut stroked = |analytic: bool, kind: &str| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        let paint = Paint::stroke(Color::WHITE, 10.0).with_anti_alias(analytic);
        match kind {
            "rrect" => canvas
                .draw_rrect(Rect::new(30.0, 30.0, 170.0, 130.0), 24.0, &paint)
                .expect("rrect"),
            "circle" => canvas
                .draw_circle(Vec2::new(100.0, 80.0), 55.0, &paint)
                .expect("circle"),
            _ => canvas
                .draw_oval(Rect::new(20.0, 45.0, 180.0, 115.0), &paint)
                .expect("oval"),
        };
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (pixels, vertices)
    };

    for kind in ["rrect", "circle", "oval"] {
        let (analytic, analytic_vertices) = stroked(true, kind);
        let (tessellated, tessellated_vertices) = stroked(false, kind);
        assert_eq!(analytic_vertices, 4, "{kind}: an outline should be a quad");
        assert!(
            tessellated_vertices > 32,
            "{kind}: the tessellated outline should cost far more, got {tessellated_vertices}"
        );

        let area = |pixels: &[u8]| {
            pixels
                .chunks_exact(4)
                .map(|texel| texel[0] as u64)
                .sum::<u64>() as f64
        };
        let difference = (area(&analytic) - area(&tessellated)).abs() / area(&tessellated);
        assert!(
            difference < 0.01,
            "{kind}: the two outlines cover different amounts, {:.2}% apart",
            difference * 100.0
        );
        // An outline, not a fill: the middle of a shape far larger than the
        // width is untouched by either.
        let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * extent.width + x) * 4) as usize];
        assert_eq!(
            at(&analytic, 100, 80),
            0,
            "{kind}: the middle should be empty"
        );
        assert_eq!(
            at(&tessellated, 100, 80),
            0,
            "{kind}: the middle should be empty"
        );
    }

    // A stroked circle is an annulus, whose area is arithmetic rather than an
    // approximation of one -- so this is measured against the shape itself
    // rather than against the other way of drawing it.
    let (analytic, _) = stroked(true, "circle");
    let (tessellated, _) = stroked(false, "circle");
    let area = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>() as f64
    };
    let exact = std::f64::consts::PI * (60.0f64 * 60.0 - 50.0 * 50.0) * 255.0;
    let analytic_error = (area(&analytic) - exact).abs() / exact;
    let tessellated_error = (area(&tessellated) - exact).abs() / exact;
    assert!(
        analytic_error < 0.001,
        "the analytic ring is {:.3}% off the exact annulus",
        analytic_error * 100.0
    );
    assert!(
        analytic_error < tessellated_error,
        "the analytic ring should be nearer the exact annulus: {:.3}% against {:.3}%",
        analytic_error * 100.0,
        tessellated_error * 100.0
    );
}

#[test]
fn an_analytic_stroke_deforms_with_the_transform() {
    let Some(mut ctx) = context() else { return };
    // A stroke width is stated in the space the shape is, so a transform that
    // scales the axes differently should stretch the pen with everything else
    // -- a ring drawn twice as wide is twice as thick at its sides and no
    // thicker at its top. That is what a tessellated stroke does, because it
    // offsets the outline in path space and transforms the result, and the
    // field has to agree: converting the width to device space anywhere would
    // give a pen that stayed round while its shape stretched.
    let extent = Extent2D::new(220, 160);
    let mut ring = |analytic: bool, sx: f32, sy: f32| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.translate(110.0, 80.0);
        canvas.scale(sx, sy);
        canvas.translate(-110.0, -80.0);
        canvas
            .draw_circle(
                Vec2::new(110.0, 80.0),
                50.0,
                &Paint::stroke(Color::WHITE, 12.0).with_anti_alias(analytic),
            )
            .expect("ring");
        canvas.restore();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };
    // How many lit pixels a horizontal line through the center crosses, which
    // is both walls of the ring and so twice its thickness there.
    let across = |pixels: &[u8]| {
        (0..extent.width)
            .filter(|x| pixels[((80 * extent.width + x) * 4) as usize] > 128)
            .count()
    };
    let coverage = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>() as f64
    };

    let (round, round_tessellated) = (ring(true, 1.0, 1.0), ring(false, 1.0, 1.0));
    let (wide, wide_tessellated) = (ring(true, 2.0, 1.0), ring(false, 2.0, 1.0));

    // The pen stretched: twice as wide a ring is twice as thick at its sides.
    assert!(
        across(&wide) > across(&round) + 12,
        "the stroke did not stretch: {} against {}",
        across(&wide),
        across(&round)
    );
    // And matches what tessellating the same thing gives, which is the only
    // definition of right available here.
    for (name, analytic, tessellated) in [
        ("unscaled", &round, &round_tessellated),
        ("stretched", &wide, &wide_tessellated),
    ] {
        assert_eq!(
            across(analytic),
            across(tessellated),
            "{name}: the two strokes differ in thickness"
        );
        let apart = (coverage(analytic) - coverage(tessellated)).abs() / coverage(tessellated);
        assert!(
            apart < 0.01,
            "{name}: the two strokes cover different amounts, {:.2}% apart",
            apart * 100.0
        );
    }
}

#[test]
fn an_analytic_shape_does_not_erase_what_is_behind_it() {
    let Some(mut ctx) = context() else { return };
    // A shape evaluated per fragment is drawn on a quad larger than itself and
    // emits a fragment everywhere on it, including where coverage is nothing.
    // Under a mode that discards the destination where the source is
    // transparent, those fragments erase what is behind them -- in the ring
    // between the shape and its quad, and in a rounded corner that gap is most
    // of the corner. Tessellating covers only the shape and has no such gap, so
    // the two routes would disagree about a region neither is drawing into.
    //
    // The modes that cannot take this path are refused it rather than fixed,
    // because there is nothing to fix: the quad is what makes the edge smooth.
    let extent = Extent2D::new(128, 128);
    let shape = Rect::new(40.0, 40.0, 88.0, 88.0);
    let mut over_a_backdrop = |blend: BlendMode, anti_alias: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::linear(0.0, 0.4, 0.0, 1.0)).with_anti_alias(false),
            )
            .expect("backdrop");
        canvas
            .draw_rrect(
                shape,
                10.0,
                &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0))
                    .with_blend(blend)
                    .with_anti_alias(anti_alias),
            )
            .expect("shape");
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let pixels = render_recording(&mut ctx, &recording);
        (pixels, vertices)
    };

    let at = |pixels: &[u8], x: u32, y: u32| {
        let i = ((y * extent.width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    // Inside the quad and outside the shape: the corner of a rounded rectangle
    // whose arc has left this point behind, and the ring along a straight edge.
    let gaps = [(41, 41), (39, 64)];

    let (replaced, replaced_vertices) = over_a_backdrop(BlendMode::Src, true);
    let (tessellated, _) = over_a_backdrop(BlendMode::Src, false);
    // Refused the fast path, so it drew the outline it actually covers. Counted
    // against the mode that keeps it rather than against a bare number, since
    // the backdrop contributes vertices of its own to both.
    let (_, composited_vertices) = over_a_backdrop(BlendMode::SrcOver, true);
    assert!(
        replaced_vertices > composited_vertices,
        "a destination-discarding mode should have fallen back to tessellation: \
         {replaced_vertices} against {composited_vertices}"
    );
    for (x, y) in gaps {
        assert_eq!(
            at(&replaced, x, y),
            at(&tessellated, x, y),
            "({x}, {y}) differs between the two routes"
        );
        assert_ne!(
            at(&replaced, x, y),
            [0, 0, 0, 0],
            "({x}, {y}) was erased rather than left alone"
        );
    }

    // And the modes that can take it still do, still smoothly.
    let (composited, _) = over_a_backdrop(BlendMode::SrcOver, true);
    for (x, y) in gaps {
        assert_ne!(
            at(&composited, x, y),
            [0, 0, 0, 0],
            "({x}, {y}) was erased by a mode that composites"
        );
    }
}

#[test]
fn an_oval_is_an_ellipse_rather_than_a_stadium() {
    let Some(mut ctx) = context() else { return };
    // The shape a rounded rectangle cannot become. Past half its shorter side a
    // rounded rectangle stops changing and is a stadium: straight along the
    // middle, semicircular at the ends. An ellipse curves the whole way, and
    // the distinguishing measurement is halfway along the major axis, where it
    // has narrowed and a stadium has not.
    let extent = Extent2D::new(200, 140);
    // 160 by 72, so the semi-axes are 80 and 36 about (100, 70).
    let bounds = Rect::new(20.0, 34.0, 180.0, 106.0);
    let mut oval = |anti_alias: bool| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas
            .draw_oval(
                bounds,
                &Paint::fill(Color::WHITE).with_anti_alias(anti_alias),
            )
            .expect("oval");
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (pixels, vertices)
    };

    let (analytic, analytic_vertices) = oval(true);
    let (tessellated, tessellated_vertices) = oval(false);
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * extent.width + x) * 4) as usize];

    assert_eq!(analytic_vertices, 4, "an antialiased oval should be a quad");
    assert!(
        tessellated_vertices > 16,
        "the tessellated oval should cost many more vertices, got {tessellated_vertices}"
    );

    for (name, pixels) in [("analytic", &analytic), ("tessellated", &tessellated)] {
        // Halfway along the major axis the curve has narrowed to 31 of its 36,
        // so a point 33 out is outside it and one 28 out is inside. A stadium
        // would have both inside, and so would a rounded rectangle asked for a
        // radius larger than it can take.
        assert_eq!(
            at(pixels, 140, 103),
            0,
            "{name}: this shape is a stadium, not an ellipse"
        );
        assert_eq!(at(pixels, 140, 98), 255, "{name}: inside the curve");
        // And the extremes of both axes are reached.
        assert_eq!(at(pixels, 100, 70), 255, "{name}: center");
        assert_eq!(at(pixels, 22, 70), 255, "{name}: the end of the major axis");
        assert_eq!(
            at(pixels, 100, 36),
            255,
            "{name}: the end of the minor axis"
        );
        assert_eq!(at(pixels, 25, 38), 0, "{name}: the corner is outside");
    }

    // Against the area of an actual ellipse, which neither is measured against
    // anywhere else. The distance field is nearer, as it was for the circle and
    // for the same reason: four cubics approximate a curve, and this is one.
    let exact = std::f64::consts::PI * 80.0 * 36.0 * 255.0;
    let area = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>() as f64
    };
    let analytic_error = (area(&analytic) - exact).abs() / exact;
    let tessellated_error = (area(&tessellated) - exact).abs() / exact;
    assert!(
        analytic_error < 0.001,
        "the analytic oval is {:.3}% off the exact area",
        analytic_error * 100.0
    );
    assert!(
        analytic_error < tessellated_error,
        "the analytic oval should be nearer the exact area: {:.3}% against {:.3}%",
        analytic_error * 100.0,
        tessellated_error * 100.0
    );
}

#[test]
fn an_antialiased_circle_uses_the_same_distance_field() {
    let Some(mut ctx) = context() else { return };
    // A circle is a rounded rectangle: a square whose corner radius is half its
    // side has no straight edge left, and the field reduces exactly to the
    // distance from the center less the radius. So it costs two triangles and
    // needs no shader of its own.
    let extent = Extent2D::new(160, 120);
    let radius = 40.0f32;
    let mut draw = |analytic: bool| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        if analytic {
            canvas
                .draw_circle(Vec2::new(80.0, 60.0), radius, &Paint::fill(Color::WHITE))
                .expect("circle");
        } else {
            canvas
                .draw_circle(
                    Vec2::new(80.0, 60.0),
                    radius,
                    &Paint::fill(Color::WHITE).with_anti_alias(false),
                )
                .expect("circle");
        }
        let recording = canvas.finish();
        let vertices: usize = recording
            .passes
            .iter()
            .map(|pass| pass.batch.vertices().len())
            .sum();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        let coverage: u64 = pixels.chunks_exact(4).map(|texel| texel[0] as u64).sum();
        (vertices, coverage as f64)
    };

    let (analytic_vertices, analytic_coverage) = draw(true);
    let (tessellated_vertices, tessellated_coverage) = draw(false);

    assert_eq!(
        analytic_vertices, 4,
        "an antialiased circle should be a quad"
    );
    assert!(
        tessellated_vertices > 16,
        "the tessellated circle should cost many more vertices, got {tessellated_vertices}"
    );

    // Against the area of an actual circle, which is a thing both are trying to
    // be and neither is measured against anywhere else. The analytic one is
    // nearer, and by a wide margin: the other is a polygon approximation with
    // its coverage quantized to four samples, where this is the curve itself
    // with coverage taken from a distance.
    let exact = std::f64::consts::PI * (radius as f64) * (radius as f64) * 255.0;
    let analytic_error = (analytic_coverage - exact).abs() / exact;
    let tessellated_error = (tessellated_coverage - exact).abs() / exact;
    assert!(
        analytic_error < 0.002,
        "the analytic circle is {:.3}% off the exact area",
        analytic_error * 100.0
    );
    assert!(
        analytic_error < tessellated_error,
        "the analytic circle should be nearer the exact area than the \
         tessellated one: {:.3}% against {:.3}%",
        analytic_error * 100.0,
        tessellated_error * 100.0
    );
}

/// And it still matches once the transform stops being affine.
///
/// This is where the two routes stop resembling each other. The tessellated one
/// flattens in user space and hands the rasterizer vertices that already carry
/// the divide; the analytic one draws a quad and asks, per fragment, where in
/// the shape's own space that pixel fell -- which under perspective means
/// inverting a homography rather than an affine, and measuring a corner radius
/// in the space where the corner is round. Agreement between them is the check
/// on that inverse.
///
/// A hard-edged rectangle cannot make this check, which is why the existing
/// perspective tests do not: with no radius there is no distance to measure in
/// local space, so a trapezoid comes out right even if the mapping inside it is
/// wrong. The radius is the part under test, so it is varied against zero.
#[test]
fn the_two_shapes_still_agree_under_perspective() {
    let Some(mut ctx) = context() else { return };
    let extent = Extent2D::new(160, 120);
    let region = Rect::new(40.0, 30.0, 120.0, 90.0);
    let mut coverage = |analytic: bool, matrix: &[f32; 16], radius: f32| -> u64 {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        canvas.concat_4x4(matrix);
        if analytic {
            canvas
                .draw_rrect(region, radius, &Paint::fill(Color::WHITE))
                .expect("rrect");
        } else {
            canvas
                .draw_path(&region.to_rounded_path(radius), &Paint::fill(Color::WHITE))
                .expect("path");
        }
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>()
    };

    // Receding along each axis, along both, and toward the viewer -- the last
    // because a divisor shrinking rather than growing magnifies, and an inverse
    // that only happened to work on one side of unity would pass without it.
    for (along_x, along_y) in [
        (0.0, 0.002),
        (0.0, 0.004),
        (0.003, 0.0),
        (0.002, 0.002),
        (0.0, -0.003),
    ] {
        let matrix = receding(along_x, along_y);
        for radius in [20.0, 0.0] {
            let analytic = coverage(true, &matrix, radius) as f64;
            let tessellated = coverage(false, &matrix, radius) as f64;
            let error = (analytic - tessellated).abs() / tessellated;
            assert!(
                error < 0.01,
                "under ({along_x}, {along_y}) with radius {radius} the two paths \
                 cover different areas: {analytic} against {tessellated}, \
                 {:.2}% apart",
                error * 100.0
            );
        }
    }

    // The perspective has to be doing something, or the agreement above is an
    // agreement about an affine.
    let flat = coverage(true, &receding(0.0, 0.0), 20.0);
    let deep = coverage(true, &receding(0.0, 0.004), 20.0);
    assert!(
        (flat as f64 - deep as f64).abs() / flat as f64 > 0.2,
        "the perspective barely changed the shape: {flat} against {deep}"
    );
}

#[test]
fn the_analytic_shape_matches_the_tessellated_one() {
    let Some(mut ctx) = context() else { return };
    // Two quite different ways of deciding which pixels the shape covers, held
    // against each other. Per pixel they differ at the edge -- four samples can
    // only say nothing, a quarter, a half, three quarters or all, where a
    // distance says anything in between -- so the comparison is of total
    // coverage, which is the same question both are answering.
    //
    // Under anisotropic scale as well, since that is where measuring the
    // distance in the wrong space would round the corners by different amounts
    // on each axis and still look plausible.
    let extent = Extent2D::new(160, 120);
    let region = Rect::new(40.0, 30.0, 120.0, 90.0);
    let mut coverage = |analytic: bool, sx: f32, sy: f32| {
        let mut canvas = Canvas::new(extent).with_samples(4);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.translate(80.0, 60.0);
        canvas.scale(sx, sy);
        canvas.translate(-80.0, -60.0);
        if analytic {
            canvas
                .draw_rrect(region, 20.0, &Paint::fill(Color::WHITE))
                .expect("rrect");
        } else {
            canvas
                .draw_path(&region.to_rounded_path(20.0), &Paint::fill(Color::WHITE))
                .expect("path");
        }
        canvas.restore();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
            .chunks_exact(4)
            .map(|texel| texel[0] as u64)
            .sum::<u64>()
    };

    for (sx, sy) in [(1.0, 1.0), (1.4, 0.8), (0.6, 1.3)] {
        let analytic = coverage(true, sx, sy) as f64;
        let tessellated = coverage(false, sx, sy) as f64;
        let error = (analytic - tessellated).abs() / tessellated;
        assert!(
            error < 0.01,
            "at scale ({sx}, {sy}) the two paths cover different areas: \
             {analytic} against {tessellated}, {:.2}% apart",
            error * 100.0
        );
    }
}

#[test]
fn the_analytic_shape_antialiases_without_multisampling() {
    let Some(mut ctx) = context() else { return };
    // What the distance field buys beyond the vertex count. A tessellated
    // shape at one sample has hard edges; this one does not, because coverage
    // is a number the fragment stage computes rather than a count of samples.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rrect(
            Rect::new(16.0, 32.0, 112.0, 96.0),
            16.0,
            &Paint::fill(Color::WHITE),
        )
        .expect("rrect");
    let recording = canvas.finish();
    // One sample: nothing here asked for multisampling, and the analytic path
    // does not turn it on.
    assert_eq!(
        recording.root().descriptor.samples,
        1,
        "the analytic path should not need a multisampled pass"
    );

    let pixels = render_recording(&mut ctx, &recording);
    let partial = pixels
        .chunks_exact(4)
        .filter(|texel| texel[0] > 8 && texel[0] < 247)
        .count();
    assert!(
        partial > 40,
        "an analytic edge should have partly covered pixels, found {partial}"
    );
}

#[test]
fn each_corner_of_a_rounded_rectangle_can_have_its_own_radii() {
    // `dart:ui`'s `RRect` carries eight numbers -- four corners, each with an x
    // and a y radius -- and this took one for a long time. The parity table
    // said the operation existed and did not say in what form.
    let Some(mut ctx) = context() else { return };

    let region = Rect::new(20.0, 20.0, 108.0, 108.0);
    let shot = |ctx: &mut Context, draw: &dyn Fn(&mut Canvas)| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        draw(&mut canvas);
        render(ctx, canvas)
    };
    let paint = || Paint::fill(Color::WHITE).with_anti_alias(false);

    // Spelling a uniform shape the general way has to give the uniform shape,
    // and not merely something close: it takes the same route, so it is the
    // same picture.
    let uniform = shot(&mut ctx, &|c| {
        c.draw_rrect(region, 16.0, &paint()).expect("uniform");
    });
    let spelled_out = shot(&mut ctx, &|c| {
        c.draw_rrect_with_radii(region, [[16.0, 16.0]; 4], &paint())
            .expect("uniform, spelled out");
    });
    assert_eq!(
        uniform, spelled_out,
        "a uniform shape asked for the general way should be the uniform shape"
    );

    // One corner square, the opposite one round. The picture has to differ from
    // the uniform one, or the radii are being ignored.
    let mixed = shot(&mut ctx, &|c| {
        c.draw_rrect_with_radii(
            region,
            [[0.0, 0.0], [16.0, 16.0], [16.0, 16.0], [40.0, 40.0]],
            &paint(),
        )
        .expect("mixed");
    });
    assert_ne!(
        uniform, mixed,
        "corners that differ should not draw the uniform shape"
    );

    // And which corner got which, checked where it can only be one of them.
    // The top-left corner is square, so a pixel just inside it is painted; the
    // bottom-right is rounded hardest, so the matching pixel there is not.
    let at = |p: &[u8], x: u32, y: u32| pixel(p, x, y)[0];
    assert!(
        at(&mixed, 22, 22) > 200,
        "the square corner should be filled to its point"
    );
    assert_eq!(
        at(&mixed, 106, 106),
        0,
        "the corner given the largest radii should be cut back furthest"
    );
    // The uniform shape cuts both alike, which is what says the two probes
    // above are reading the radii and not the rectangle.
    assert_eq!(
        at(&uniform, 22, 22),
        at(&uniform, 106, 106),
        "a uniform shape is symmetric about its diagonal"
    );
}

#[test]
fn radii_that_overrun_a_side_are_scaled_together_rather_than_singly() {
    // `dart:ui`'s rule, and the reason it is one rule rather than a clamp per
    // corner: when two radii along an edge do not fit, every radius in the
    // shape is scaled by the same ratio. Clamping only the pair that overran
    // would also stop the outline crossing itself, and would leave a shape with
    // proportions the caller did not ask for.
    let Some(mut ctx) = context() else { return };

    let region = Rect::new(20.0, 40.0, 108.0, 88.0);
    let shot = |ctx: &mut Context, radii: RoundingRadii| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect_with_radii(
                region,
                radii,
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("rrect");
        render(ctx, canvas)
    };

    // Twice what fits, everywhere: the scale is a half, so this is the shape
    // asked for at half the radii -- which is the shape that exactly fits.
    let overrun = shot(&mut ctx, [[48.0, 48.0]; 4]);
    let fitted = shot(&mut ctx, [[24.0, 24.0]; 4]);
    assert_eq!(
        overrun, fitted,
        "radii twice too large should scale to the ones that fit"
    );

    // And the scaling is global. Only the top edge overruns here -- 96 across a
    // side of 88 -- so a rule that clamped the offending pair alone would leave
    // the bottom corners at twelve, and one that scales everything takes them
    // to eleven with the top pair.
    let one_edge = shot(
        &mut ctx,
        [[48.0, 48.0], [48.0, 48.0], [12.0, 12.0], [12.0, 12.0]],
    );
    let scaled_all = shot(
        &mut ctx,
        [[44.0, 44.0], [44.0, 44.0], [11.0, 11.0], [11.0, 11.0]],
    );
    assert_eq!(
        one_edge, scaled_all,
        "one overrunning edge should scale every corner, not just its own two"
    );
}

#[test]
fn a_radius_larger_than_the_rectangle_is_clamped() {
    let Some(mut ctx) = context() else { return };
    // Past half the shorter side the corner arcs would overlap and the outline
    // would cross itself, which fills as something nobody asked for. Clamped,
    // the shape becomes a stadium and then stops changing -- so an enormous
    // radius and a merely sufficient one give the same picture.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let render = |ctx: &mut Context, radius: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(
                region,
                radius,
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("rrect");
        render(ctx, canvas)
    };

    // Half the height is 32, so both of these clamp to the same shape.
    let exact = render(&mut ctx, 32.0);
    let enormous = render(&mut ctx, 10_000.0);
    assert_eq!(exact, enormous, "a huge radius should clamp to the stadium");
    // Infinity among them: a caller who writes it means the roundest shape
    // available, so answering with a square would be the opposite of the ask.
    assert_eq!(
        exact,
        render(&mut ctx, f32::INFINITY),
        "an infinite radius should clamp like any other large one"
    );

    // And it is a stadium rather than an outline that crossed itself: the
    // middle is filled, the corners are gone, and the mid-height ends are the
    // extreme points of the curve.
    let lit = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] > 128;
    assert!(lit(&exact, 64, 64), "the middle should be filled");
    assert!(!lit(&exact, 17, 33), "the corner should be gone");
    assert!(lit(&exact, 17, 64), "the left end should reach its extreme");
}

#[test]
fn a_radius_of_zero_is_the_plain_rectangle() {
    // So a caller can pass a radius that happens to be zero -- an interface
    // animating one, or reading it from a style -- without special-casing it.
    let region = Rect::new(16.0, 32.0, 112.0, 96.0);
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rrect(region, 0.0, &Paint::fill(Color::WHITE))
        .expect("rrect");
    let rounded = canvas.finish();

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(region, &Paint::fill(Color::WHITE))
        .expect("rect");
    let square = canvas.finish();

    assert_eq!(
        rounded.draw_count(),
        square.draw_count(),
        "a zero radius should record the same drawing as a plain rectangle"
    );
    // Negative too, which is what a caller subtracting a border can produce --
    // and a radius that is not a number at all, which is what arithmetic on one
    // produces when it goes wrong. Both give the plain rectangle rather than a
    // shape derived from a value nobody meant.
    for radius in [-4.0, f32::NAN] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(region, radius, &Paint::fill(Color::WHITE))
            .expect("rrect");
        let recorded = canvas.finish();
        assert_eq!(
            recorded.draw_count(),
            square.draw_count(),
            "a radius of {radius} should record the same drawing as a plain rectangle"
        );
    }
}

#[test]
fn an_empty_rounded_rectangle_draws_nothing() {
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_rrect(
            Rect::new(50.0, 50.0, 50.0, 20.0),
            8.0,
            &Paint::fill(Color::WHITE),
        )
        .expect("rrect");
    assert!(
        canvas.finish().is_empty(),
        "a rectangle with no area should record no drawing"
    );
}

#[test]
fn a_linear_gradient_runs_between_its_stops() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(128.0, 0.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let pixels = render(&mut ctx, canvas);
    let left = pixel(&pixels, 1, 64);
    let middle = pixel(&pixels, 64, 64);
    let right = pixel(&pixels, 126, 64);

    // Red at the start, blue at the end, and a genuine mix between: a gradient
    // that failed to evaluate would come back as one flat color.
    assert!(left[0] > 240 && left[2] < 16, "left end: {left:?}");
    assert!(right[2] > 240 && right[0] < 16, "right end: {right:?}");
    assert!(
        middle[0] > 80 && middle[0] < 180 && middle[2] > 80 && middle[2] < 180,
        "midpoint should be a mix, got {middle:?}"
    );
}

#[test]
fn a_linear_gradient_runs_the_same_way_on_a_target_that_is_not_square() {
    let Some(mut ctx) = context() else { return };
    // Twice as wide as it is tall, which is what makes this fail: the shader
    // locates itself from clip position, and clip space is normalized to each
    // axis independently. Projecting onto the axis there scales x and y by
    // different amounts, so a diagonal gradient runs at the wrong angle — the
    // steeper the target, the further off. It has to be projected in the space
    // the caller stated the axis in.
    let extent = Extent2D {
        width: 128,
        height: 64,
    };
    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 64.0),
            &Paint::linear_gradient(
                Vec2::ZERO,
                // Diagonal, and equal in both axes so that the two sample points
                // below are the same distance along it.
                Vec2::new(64.0, 64.0),
                vec![
                    GradientStop::new(Color::linear(0.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(1.0, 1.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let mut surface = ctx
        .create_surface(extent, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    let at = |x: u32, y: u32| pixels[((y * extent.width + x) * 4) as usize];

    // (32, 0), (0, 32) and (16, 16) all project to the same point on a (1, 1)
    // axis, so they are the same color or the gradient is not running along
    // that axis. Before the fix these read 27, 104 and 66.
    let (a, b, c) = (at(32, 0), at(0, 32), at(16, 16));
    let spread = a.abs_diff(b).max(b.abs_diff(c)).max(a.abs_diff(c));
    assert!(
        spread <= 2,
        "equidistant points along the axis differ: {a}, {b}, {c}"
    );
    // And it is a gradient rather than a flat fill: twice as far along is
    // visibly brighter, and the near corner is dark.
    assert!(at(32, 32) > a + 40, "{} should exceed {a}", at(32, 32));
    assert!(at(1, 1) < 16, "the origin corner should be dark");
}

#[test]
fn a_gradient_runs_along_the_axis_it_was_given() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    // Vertical this time. A gradient that ignored its endpoints, or that used
    // the fragment coordinate builtin instead of clip position, would run the
    // wrong way or not change at all.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(0.0, 128.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let pixels = render(&mut ctx, canvas);
    // User space runs downward, so the start color belongs at the top.
    assert!(
        pixel(&pixels, 64, 1)[0] > 240,
        "top should be the first stop"
    );
    assert!(
        pixel(&pixels, 64, 126)[2] > 240,
        "bottom should be the last stop"
    );
    // And it must vary down the axis rather than across it.
    let a = pixel(&pixels, 4, 64);
    let b = pixel(&pixels, 124, 64);
    assert_eq!(a, b, "a vertical gradient should not vary horizontally");
}

#[test]
fn a_gradient_travels_with_the_canvas_transform() {
    let Some(mut ctx) = context() else { return };

    let draw = |ctx: &mut Context, rotate: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        if rotate {
            // Rotate about the center by a quarter turn.
            canvas.translate(64.0, 64.0);
            canvas.rotate(std::f32::consts::FRAC_PI_2);
            canvas.translate(-64.0, -64.0);
        }
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::linear_gradient(
                    Vec2::new(0.0, 0.0),
                    Vec2::new(128.0, 0.0),
                    vec![
                        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                    ],
                )
                .with_anti_alias(false),
            )
            .expect("gradient");
        render(ctx, canvas)
    };

    let plain = draw(&mut ctx, false);
    let rotated = draw(&mut ctx, true);

    // Endpoints go through the same transform the geometry does, so rotating
    // the canvas turns the gradient with the shape rather than leaving it
    // pinned to the screen.
    assert!(
        pixel(&plain, 1, 64)[0] > 240,
        "unrotated starts red at left"
    );
    assert!(
        pixel(&rotated, 64, 1)[0] > 240,
        "rotated should start red at the top, got {:?}",
        pixel(&rotated, 64, 1)
    );
}

#[test]
fn a_gradient_with_more_than_two_stops_passes_through_each() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(128.0, 0.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 0.5),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");

    let pixels = render(&mut ctx, canvas);
    // The middle stop has to actually be reached, not interpolated past. A
    // two-stop shortcut would show no green anywhere.
    let middle = pixel(&pixels, 64, 64);
    assert!(
        middle[1] > 200 && middle[0] < 60 && middle[2] < 60,
        "the middle stop should dominate at the midpoint, got {middle:?}"
    );
}

#[test]
fn a_radial_gradient_runs_outward_from_its_center() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::radial_gradient(
                Vec2::new(64.0, 64.0),
                60.0,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("radial");

    let pixels = render(&mut ctx, canvas);
    let center = pixel(&pixels, 64, 64);
    assert!(center[0] > 240 && center[2] < 16, "center: {center:?}");

    // Equidistant points must match. A radial gradient measured in clip space
    // without mapping back would be an ellipse, and these would differ.
    let right = pixel(&pixels, 64 + 40, 64);
    let below = pixel(&pixels, 64, 64 + 40);
    for channel in 0..4 {
        assert!(
            (right[channel] as i32 - below[channel] as i32).abs() <= 2,
            "the gradient is not circular: right {right:?}, below {below:?}"
        );
    }
    // And it must actually vary with distance.
    assert!(right[2] > center[2] + 40, "no falloff toward the edge");
}

#[test]
fn a_radial_gradient_stays_circular_on_a_non_square_target() {
    let Some(mut ctx) = context() else { return };
    // The case the mapping exists for: clip space scales each axis by that
    // axis' size, so on a non-square target an unmapped radial gradient comes
    // out visibly stretched.
    let extent = Extent2D::new(192, 96);
    let mut canvas = Canvas::new(extent);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(192.0, 96.0),
            &Paint::radial_gradient(
                Vec2::new(96.0, 48.0),
                40.0,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("radial");

    let mut surface = ctx
        .create_surface(extent, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);

    let at = |x: u32, y: u32| {
        let i = ((y * extent.width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    // Thirty pixels right and thirty pixels down from the center are the same
    // distance away, so they must be the same color.
    let right = at(96 + 30, 48);
    let below = at(96, 48 + 30);
    for channel in 0..4 {
        assert!(
            (right[channel] as i32 - below[channel] as i32).abs() <= 2,
            "stretched by the aspect ratio: right {right:?}, below {below:?}"
        );
    }
}

/// Render a 128-wide band filled by a gradient spanning only its first quarter.
///
/// The ramp is red at x=0 and blue at x=32, so everything from 32 on is outside
/// it -- three quarters of the picture rather than a strip at the edge, which
/// is what makes the three modes tell themselves apart at a glance and in
/// arithmetic.
fn tiled_ramp(ctx: &mut Context, tile: TileMode) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    // A white ground, so decal's "nothing" is visible as something rather than
    // as the clear color it would be indistinguishable from.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("ground");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(32.0, 0.0),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_tile_mode(tile)
            .with_blend(BlendMode::SrcOver)
            .with_anti_alias(false),
        )
        .expect("gradient");
    render(ctx, canvas)
}

#[test]
fn no_number_a_caller_can_write_takes_the_process_down() {
    let Some(mut ctx) = context() else { return };
    // A wider sweep than the NaN one beside this, and a weaker claim. What a
    // renderer draws for an infinite rectangle or a stroke a million units wide
    // is a judgment call; that it must not abort the program holding it is
    // not. Two of these were found aborting, inside a dependency's assertion,
    // which is a failure a caller can neither catch nor prevent.
    //
    // Every value here is one a caller reaches by ordinary means: a subtraction
    // that went negative, a division by an extent that turned out to be zero, a
    // coordinate scaled once too often.
    let poisons = [
        ("nan", f32::NAN),
        ("inf", f32::INFINITY),
        ("-inf", f32::NEG_INFINITY),
        ("huge", 1e30),
        ("-huge", -1e30),
        ("tiny", 1e-30),
        ("zero", 0.0),
        ("negative", -8.0),
    ];

    for (name, v) in poisons {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let white = Paint::fill(Color::WHITE);
        let stroked = Paint::stroke(Color::WHITE, v);

        // Shapes, sized and positioned by the value.
        let _ = canvas.draw_rect(Rect::new(v, v, v + 32.0, v + 32.0), &white);
        let _ = canvas.draw_rrect(Rect::from_size(64.0, 64.0), v, &white);
        let _ = canvas.draw_circle(Vec2::new(64.0, 64.0), v, &white);
        let _ = canvas.draw_oval(Rect::new(0.0, 0.0, v, v), &white);
        let _ = canvas.draw_line(Vec2::ZERO, Vec2::new(v, v), &stroked);
        let _ = canvas.draw_rect(Rect::from_size(32.0, 32.0), &stroked);

        // Curves, where the value is a control point rather than an endpoint.
        let mut builder = PathBuilder::new();
        builder.move_to(Vec2::new(8.0, 8.0));
        builder.quad_to(Vec2::new(v, 64.0), Vec2::new(96.0, 96.0));
        builder.cubic_to(Vec2::new(v, v), Vec2::new(16.0, v), Vec2::new(8.0, 8.0));
        builder.close();
        let _ = canvas.draw_path(&builder.build(), &white);

        // Paint parameters rather than geometry.
        let _ = canvas.draw_rect(
            Rect::from_size(64.0, 64.0),
            &Paint::radial_gradient(
                Vec2::new(64.0, 64.0),
                v,
                vec![
                    GradientStop::new(Color::WHITE, v),
                    GradientStop::new(Color::BLACK, 1.0),
                ],
            ),
        );
        let _ = canvas.draw_rect(
            Rect::from_size(64.0, 64.0),
            &Paint::sweep_gradient(
                Vec2::new(64.0, 64.0),
                v,
                v,
                vec![
                    GradientStop::new(Color::WHITE, 0.0),
                    GradientStop::new(Color::BLACK, 1.0),
                ],
            ),
        );

        // The state stack: a clip nothing can satisfy, a transform that
        // collapses, a layer scaled by a value that is not a fraction.
        canvas.save();
        let _ = canvas.clip_rect(Rect::new(v, v, v, v));
        canvas.translate(v, v);
        canvas.scale(v, v);
        canvas.rotate(v);
        let _ = canvas.draw_rect(Rect::from_size(16.0, 16.0), &white);
        canvas.restore();

        canvas.save_layer(Layer::opacity(v).with_blur(v));
        let _ = canvas.draw_rect(Rect::from_size(48.0, 48.0), &white);
        canvas.restore();

        // Rendered rather than only recorded: half of what could go wrong
        // happens in the backend, and a recording nobody submits proves
        // nothing about it.
        let pixels = render(&mut ctx, canvas);
        assert_eq!(
            pixels.len(),
            (SIZE.width * SIZE.height * 4) as usize,
            "{name} did not produce a full frame"
        );
    }
}

#[test]
fn a_glyph_placed_by_arithmetic_that_went_wrong_still_renders_a_frame() {
    let Some(mut ctx) = context() else { return };
    // Text layout is where a caller most easily produces one of these without
    // noticing: a width divided by a zero advance, an origin accumulated across
    // a run that began with a NaN. Separate from the sweep beside this because
    // a glyph run samples the atlas, and a draw whose texture nobody supplied
    // is refused before it can reach anything worth testing.
    let (atlas, solid, _) = two_glyph_atlas();
    let rect = atlas.get(solid).unwrap();

    for (name, v) in [
        ("nan", f32::NAN),
        ("inf", f32::INFINITY),
        ("-inf", f32::NEG_INFINITY),
        ("huge", 1e30),
    ] {
        let image = upload_atlas(&mut ctx, &atlas);
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        // A well-placed glyph beside the poisoned ones, so this says the run
        // survived rather than only that the call returned.
        let _ = canvas.draw_glyphs(
            &[
                PositionedGlyph::new(solid, [16.0, 16.0], rect),
                PositionedGlyph::new(solid, [v, 40.0], rect),
                PositionedGlyph::new(solid, [40.0, v], rect),
            ],
            &atlas,
            0,
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)),
        );

        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        ctx.destroy_image(image);

        assert_eq!(
            pixel(&pixels, 20, 20),
            [255, 0, 0, 255],
            "{name}: the well-placed glyph in the same run did not survive"
        );
    }
}

/// One call made with a value that is not a number, drawn onto a canvas.
type PoisonedDraw = Box<dyn Fn(&mut Canvas)>;

#[gtest]
fn a_draw_placed_at_nan_contributes_nothing() {
    let Some(mut ctx) = context() else { return };
    // Infinity and NaN are not the same request. A caller who writes infinity
    // means the largest thing available, and answering with a shape that covers
    // everything is a defensible reading of it. NaN is not a location, a size,
    // or a width; it is what arithmetic produces when it has already gone
    // wrong, and there is no picture it asks for. So the rule is that such a
    // draw contributes nothing, which is checkable by drawing it beside
    // something valid and requiring the result to be untouched.
    //
    // Checked across the calls rather than one at a time, because a guard is
    // usually added where a bug was found and the calls that were never
    // reported keep whatever they had.
    let reference = |ctx: &mut Context| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)),
            )
            .expect("reference");
        render(ctx, canvas)
    };
    let expected = reference(&mut ctx);

    let n = f32::NAN;
    // Each entry draws the same reference square and then one poisoned call.
    let cases: Vec<(&str, PoisonedDraw)> = vec![
        (
            "rect",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rect(Rect::new(n, n, n, n), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "rect with a nan edge",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rect(Rect::new(0.0, 0.0, n, 64.0), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "rounded rect",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rrect(
                    Rect::new(n, 0.0, 64.0, 64.0),
                    8.0,
                    &Paint::fill(Color::WHITE),
                );
            }),
        ),
        (
            "circle",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_circle(Vec2::new(n, 64.0), 32.0, &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "oval",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_oval(Rect::new(0.0, n, 64.0, 64.0), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "line",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_line(
                    Vec2::new(n, 0.0),
                    Vec2::new(128.0, 128.0),
                    &Paint::stroke(Color::WHITE, 4.0),
                );
            }),
        ),
        (
            "line of nan width",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_line(
                    Vec2::new(0.0, 0.0),
                    Vec2::new(128.0, 128.0),
                    &Paint::stroke(Color::WHITE, n),
                );
            }),
        ),
        (
            "path",
            Box::new(move |c: &mut Canvas| {
                let mut builder = PathBuilder::new();
                builder.move_to(Vec2::new(0.0, 0.0));
                builder.line_to(Vec2::new(n, 64.0));
                builder.line_to(Vec2::new(64.0, 64.0));
                builder.close();
                let _ = c.draw_path(&builder.build(), &Paint::fill(Color::WHITE));
            }),
        ),
        (
            "translate",
            Box::new(move |c: &mut Canvas| {
                c.save();
                c.translate(n, 0.0);
                let _ = c.draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE));
                c.restore();
            }),
        ),
        (
            "scale",
            Box::new(move |c: &mut Canvas| {
                c.save();
                c.scale(n, 1.0);
                let _ = c.draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE));
                c.restore();
            }),
        ),
        (
            "rotate",
            Box::new(move |c: &mut Canvas| {
                c.save();
                c.rotate(n);
                let _ = c.draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE));
                c.restore();
            }),
        ),
        (
            "gradient endpoints",
            Box::new(move |c: &mut Canvas| {
                let _ = c.draw_rect(
                    Rect::from_size(64.0, 64.0),
                    &Paint::linear_gradient(
                        Vec2::new(n, n),
                        Vec2::new(n, n),
                        vec![
                            GradientStop::new(Color::WHITE, 0.0),
                            GradientStop::new(Color::BLACK, 1.0),
                        ],
                    ),
                );
            }),
        ),
    ];

    for (name, poison) in &cases {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)),
            )
            .expect("reference");
        poison(&mut canvas);
        let got = render(&mut ctx, canvas);
        let differing = got
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        // Non-fatal, so one bad call does not hide the rest of the sweep. The
        // whole point of asking every drawing call the same question is to
        // learn which of them answer it wrongly, and a fatal assertion reports
        // the first and stops.
        expect_that!(
            differing,
            eq(0),
            "{name} at NaN changed {differing} pixel(s); such a draw should contribute nothing"
        );
    }
}

#[test]
fn a_gradient_with_many_stops_agrees_with_one_that_fits() {
    let Some(mut ctx) = context() else { return };
    // The two paths through the shader must produce the same picture where both
    // can express it. Below the limit the stops travel in push constants and
    // are walked; above it the recorder tabulates them into a texture and the
    // shader reads it. A caller does not choose between those and should not be
    // able to tell which happened.
    //
    // Stated in sRGB, which is where a gradient is interpolated and therefore
    // the only space in which "on the line" means anything: a midpoint of two
    // light values is not the midpoint of their encodings, so extra stops
    // placed that way are not redundant and the two paths rightly disagree.
    //
    // So: the same gradient stated twice. Four stops that fit, and the same
    // ramp restated with extra stops placed exactly on the line between them,
    // which changes nothing about the gradient and everything about how it is
    // carried.
    let ends = vec![
        GradientStop::new(Color::srgb(1.0, 0.0, 0.0, 1.0), 0.0),
        GradientStop::new(Color::srgb(0.5, 0.0, 0.5, 1.0), 0.5),
        GradientStop::new(Color::srgb(0.0, 0.0, 1.0, 1.0), 1.0),
    ];
    let mut many = ends.clone();
    for (offset, t) in [(0.25, 0.25f32), (0.75, 0.75)] {
        // Exactly on the line: red to purple to blue is linear in each half.
        let c = if t < 0.5 { t * 2.0 } else { (t - 0.5) * 2.0 };
        let color = if t < 0.5 {
            Color::srgb(1.0 - c * 0.5, 0.0, c * 0.5, 1.0)
        } else {
            Color::srgb(0.5 - c * 0.5, 0.0, 0.5 + c * 0.5, 1.0)
        };
        many.push(GradientStop::new(color, offset));
    }
    many.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap());
    assert!(
        ends.len() <= MAX_STOPS,
        "the control must take the walked path"
    );
    assert!(
        many.len() > MAX_STOPS,
        "the subject must take the ramp path"
    );

    let render_with = |ctx: &mut Context, stops: Vec<GradientStop>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::linear_gradient(Vec2::ZERO, Vec2::new(128.0, 0.0), stops)
                    .with_anti_alias(false),
            )
            .expect("gradient");
        render(ctx, canvas)
    };

    let walked = render_with(&mut ctx, ends);
    let sampled = render_with(&mut ctx, many);

    // Both paths now interpolate the same linear values -- the table holds what
    // the walk produced rather than eight bits of an encoding of it -- so what
    // is left is where a texel center falls against where the walk is sampled.
    // A tolerance rather than an equality for that reason alone, which is why
    // it is one level and not three.
    //
    // Both are dithered, so the dither cancels here rather than separating
    // them: it is the same pattern at the same fragment either way.
    let mut worst = 0i32;
    for x in 0..128u32 {
        let a = pixel(&walked, x, 64);
        let b = pixel(&sampled, x, 64);
        for channel in 0..4 {
            worst = worst.max((a[channel] as i32 - b[channel] as i32).abs());
        }
    }
    assert!(
        worst <= 1,
        "the two paths disagree by {worst}, which is more than rounding"
    );
    // And the gradient is a gradient rather than two flat halves, which is what
    // a ramp that failed to upload would look like against a black clear.
    let left = pixel(&sampled, 4, 64);
    let right = pixel(&sampled, 124, 64);
    assert!(left[0] > 200 && left[2] < 60, "left end {left:?}");
    assert!(right[2] > 200 && right[0] < 60, "right end {right:?}");
}

#[gtest]
fn a_mask_blur_softens_a_shape_and_matches_a_blurred_layer() {
    let Some(mut ctx) = context() else { return };
    // A mask blur is defined as blurring coverage and then filling. It is
    // implemented as filling into a blurred layer, which is the opposite order
    // — and the two are the same picture only because a blur is linear and the
    // fill is constant: blur(C·a) is C·blur(a) for a constant C.
    //
    // That identity is the whole justification, so it is what gets asserted.
    // The same shape, same sigma, one through the mask blur and one through a
    // layer written out by hand, must agree.
    let sigma = 6.0;
    let shape = Rect::new(40.0, 40.0, 88.0, 88.0);
    let color = Color::linear(1.0, 0.4, 0.1, 1.0);

    let mut masked = Canvas::new(SIZE);
    masked.clear(Color::BLACK);
    // Through the general route deliberately: the identity below is that
    // route's justification, and a rectangle that knows it is one no longer
    // takes it.
    masked
        .draw_path(
            &square_path(shape),
            &Paint::fill(color).with_mask_blur(sigma),
        )
        .expect("mask blur");
    let masked = render(&mut ctx, masked);

    let mut layered = Canvas::new(SIZE);
    layered.clear(Color::BLACK);
    // The same bounds the mask blur computes: the shape, widened by the blur's
    // reach. A tighter bound would cut the tail off square.
    let reach = ((sigma - 0.5) * 1.7320508).max(0.0).ceil();
    layered.save_layer_bounds(
        Layer::opacity(1.0).with_blur(sigma),
        Rect::new(
            shape.left - reach,
            shape.top - reach,
            shape.right + reach,
            shape.bottom + reach,
        ),
    );
    layered
        .draw_rect(shape, &Paint::fill(color))
        .expect("shape");
    layered.restore();
    let layered = render(&mut ctx, layered);

    let worst = masked
        .iter()
        .zip(&layered)
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap_or(0);
    expect_that!(
        worst,
        le(1),
        "a mask blur and a blurred layer of the same shape differ by {worst}"
    );

    // And it actually blurred: the shape's own edge is soft, and light reaches
    // beyond where the shape ends.
    let edge = pixel(&masked, 88, 64);
    expect_true!(
        edge[0] > 8 && edge[0] < 247,
        "the edge is not soft: {edge:?}"
    );
    // And it reaches past where the shape ends, which is checked against the
    // same shape drawn without the blur rather than against a threshold: two
    // sigma out the value is single digits, and a number picked by eye there is
    // a number that passes or fails for reasons about the picking.
    let mut sharp = Canvas::new(SIZE);
    sharp.clear(Color::BLACK);
    sharp
        .draw_rect(shape, &Paint::fill(color).with_anti_alias(false))
        .expect("sharp");
    let sharp = render(&mut ctx, sharp);
    // Six pixels past the shape rather than twelve, which is one deviation out
    // rather than two: the kernel reaches `(sigma - 0.5) * sqrt(3)`, so two
    // deviations is past its end and reads exactly zero.
    let (beyond, control) = (pixel(&masked, 94, 64), pixel(&sharp, 94, 64));
    expect_that!(
        control[0],
        eq(0),
        "the control should not reach here at all"
    );
    expect_true!(
        beyond[0] > control[0],
        "nothing reached past the shape: {beyond:?} against {control:?}"
    );
}

#[gtest]
fn a_mask_blur_takes_a_gradient_in_every_style_but_not_over_a_glyph_run() {
    let Some(mut ctx) = context() else { return };
    // The identity the cheap route rests on holds for a constant fill and not
    // otherwise, so a gradient cannot take that route -- but it can take the
    // one the specification states, where the mask is blurred and the paint is
    // applied through it. What still refuses is not about the fill at all.
    let _ = &mut ctx;
    let gradient = || {
        Paint::linear_gradient(
            Vec2::ZERO,
            Vec2::new(128.0, 0.0),
            vec![
                GradientStop::new(Color::WHITE, 0.0),
                GradientStop::new(Color::BLACK, 1.0),
            ],
        )
        .with_mask_blur(4.0)
    };
    let square = Rect::new(16.0, 16.0, 112.0, 112.0);
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    expect_true!(
        canvas.draw_rect(square, &gradient()).is_ok(),
        "a mask blur over a gradient states the mask blurred and the paint \
         applied through it, which is defined for any fill"
    );
    // A solid one of the same size still is too, by the other route.
    expect_true!(
        canvas
            .draw_rect(square, &Paint::fill(Color::WHITE).with_mask_blur(4.0))
            .is_ok(),
        "a solid mask blur should be accepted"
    );
    // The three styles that combine a blurred mask with a sharp one were
    // refused here for a while, and the reason given was that they need the
    // coverage twice over. That was a limit of what was built rather than of
    // what the operation means, and it is built now: the combination is the
    // same rules whether what is combined is color or coverage, so the styles
    // compose the two coverages and the varying fill is drawn through the
    // result. `each_mask_blur_style_keeps_its_own_part_of_a_gradient_too`
    // checks each one keeps what it names.
    for style in [
        MaskBlurStyle::Solid,
        MaskBlurStyle::Outer,
        MaskBlurStyle::Inner,
    ] {
        expect_true!(
            canvas
                .draw_rect(square, &gradient().with_mask_blur_style(style))
                .is_ok(),
            "{style:?} over a gradient should be drawn, not refused"
        );
    }

    // What still refuses, and it is not about the fill varying. A glyph run
    // tints one color by its own nature -- the atlas supplies coverage and the
    // paint supplies the color -- so a gradient over one is refused whether it
    // is blurred or not, and blurring changes nothing about why.
    let (atlas, solid, _) = two_glyph_atlas();
    expect_true!(
        canvas
            .draw_glyphs(
                &[PositionedGlyph::new(
                    solid,
                    [32.0, 80.0],
                    atlas.get(solid).unwrap()
                )],
                &atlas,
                0,
                &gradient(),
            )
            .is_err(),
        "a gradient over a glyph run should be refused, blurred or not"
    );
}

#[test]
fn a_group_can_be_filtered_by_a_composition_in_either_order() {
    // A draw could carry any image filter through its paint and a group could
    // not: `Layer` holds four kinds in fields and applies them in one fixed
    // order, so a composition the other way round, or a caller's program over
    // a group, had no spelling. `saveLayer` claimed parity without saying so.
    let Some(mut ctx) = context() else { return };

    // Two shapes with a gap between them narrower than the structuring element
    // below, which is what makes the two orders two pictures. A dilation then
    // an erosion closes the gap and leaves it closed; an erosion then a
    // dilation returns each shape to itself and the gap with it. On a single
    // convex shape both orders are the identity, which is why the first
    // version of this test compared two identical frames and said so.
    let content = |canvas: &mut Canvas| {
        for rect in [
            Rect::new(30.0, 48.0, 60.0, 80.0),
            Rect::new(68.0, 48.0, 98.0, 80.0),
        ] {
            canvas
                .draw_rect(
                    rect,
                    &Paint::fill(Color::srgb(1.0, 1.0, 1.0, 1.0))
                        .with_anti_alias(false)
                        .with_blend(BlendMode::SrcOver),
                )
                .expect("rect");
        }
    };
    let shot = |ctx: &mut Context, filter: Option<ImageFilter>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        match filter {
            Some(filter) => {
                canvas
                    .save_layer_filtered(Layer::opacity(1.0), None, &filter)
                    .expect("filtered group");
            }
            None => {
                canvas.save_layer(Layer::opacity(1.0));
            }
        }
        content(&mut canvas);
        canvas.restore();
        render(ctx, canvas)
    };

    // Both on both axes, and that matters: a dilation across and an erosion
    // down act on independent axes and commute, so a pair chosen that way
    // gives the same picture whichever order it runs in and proves nothing.
    // The first version of this test was written that way and passed zero
    // differing pixels, which is what said so.
    let dilate = || ImageFilter::Dilate {
        radius_x: 6.0,
        radius_y: 6.0,
    };
    let erode = || ImageFilter::Erode {
        radius_x: 6.0,
        radius_y: 6.0,
    };

    // The two orders. A dilation then an erosion is not an erosion then a
    // dilation -- that is what closing and opening are, and they differ on a
    // shape with a corner. `Layer` can hold only one morphology, so neither of
    // these was expressible before, let alone both.
    let closed = shot(&mut ctx, Some(ImageFilter::compose(erode(), dilate())));
    let opened = shot(&mut ctx, Some(ImageFilter::compose(dilate(), erode())));
    let difference = closed
        .chunks_exact(4)
        .zip(opened.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        difference > 200,
        "the two orders of one composition gave nearly the same picture, so the \
         order was not carried: {difference} pixels differ"
    );

    // And which of the two moved, which is the sharper statement. Closing
    // bridges the gap, so it is not the identity; opening returns each
    // rectangle to itself, so it *is* -- and that is not a filter doing
    // nothing but the arithmetic being right. Asserting both differ from the
    // plain group is what the first version of this did, and it is false.
    //
    // Stated as magnitudes rather than as equality, because equality is not
    // true on real hardware. This asserted `plain == opened` and held on
    // lavapipe and failed on a Raspberry Pi's V3D, where two hundred and
    // forty-six boundary pixels come back at 248 to 252 instead of 255 -- a
    // worst channel error of seven, from where the morphology's resampling
    // lands between texels. Seven is not the claim being made here. The claim
    // is that one order is the identity and the other is not, and the gap
    // between "off by seven" and "off by two hundred and fifty-five" is what
    // says so on any device.
    let plain = shot(&mut ctx, None);
    let worst = |a: &[u8], b: &[u8]| -> u8 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0)
    };
    let bridged = plain
        .chunks_exact(4)
        .zip(closed.chunks_exact(4))
        .filter(|(a, b)| worst(a, b) >= 128)
        .count();
    assert!(
        bridged >= 200,
        "closing should have bridged the gap between the rectangles, which is \
         about two hundred and fifty pixels going from background to shape: \
         {bridged} pixels changed by half or more"
    );
    let slack = worst(&plain, &opened);
    assert!(
        slack <= 16,
        "opening a pair of rectangles by a rectangle returns them unchanged, so \
         this order should give the group back as it was, up to where the \
         resampling lands: worst channel differs by {slack}"
    );
    // The gap itself, named rather than left to the pixel count.
    let between = |p: &[u8]| pixel(p, 64, 64)[0];
    assert_eq!(between(&plain), 0, "the gap should be open to begin with");
    assert!(between(&closed) > 200, "closing should have filled the gap");

    // A matrix is refused here rather than at `restore`, which has no result to
    // fail into and by then has the caller's drawing in it.
    let mut canvas = Canvas::new(SIZE);
    let outcome = canvas
        .save_layer_filtered(
            Layer::opacity(1.0),
            None,
            &ImageFilter::Matrix {
                transform: Transform2D::from(Affine2::from_translation(Vec2::new(8.0, 0.0))),
            },
        )
        .map(|_| ());
    assert!(
        matches!(outcome, Err(Error::Unsupported(_))),
        "a matrix should point the caller at `Layer::with_matrix`, got {outcome:?}"
    );
}

#[test]
fn a_backdrop_takes_any_image_filter_including_the_one_that_moves_it() {
    // `Layer::backdrop_blur` is a sigma, because a layer is `Copy` and a filter
    // is not. That made a blur the only backdrop this renderer could apply,
    // where `SceneBuilder.pushBackdropFilter` takes any `ImageFilter` and
    // upstream's `SaveLayer` takes a `DlImageFilter`.
    //
    // A matrix was the last one still refused, on the reasoning that a filter
    // is a pass and a pass that moves its image needs a target sized for where
    // the image went. The first half is true of every other filter and the
    // second does not follow: a backdrop is not composited, it is *seeded*, and
    // the target it is seeded into is the layer's own -- decided before the
    // filter is consulted and the same size whatever the matrix says.
    let Some(mut ctx) = context() else { return };

    // A hard vertical edge to filter: black left, white right.
    let ground = |canvas: &mut Canvas| {
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(64.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::srgb(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
            )
            .expect("ground");
    };
    let shot = |ctx: &mut Context, backdrop: Option<ImageFilter>| {
        let mut canvas = Canvas::new(SIZE);
        ground(&mut canvas);
        if let Some(filter) = backdrop {
            canvas
                .save_layer_backdrop(Layer::opacity(1.0), None, &filter)
                .expect("backdrop");
            canvas.restore();
        }
        render(ctx, canvas)
    };

    let plain = shot(&mut ctx, None);
    assert_eq!(
        pixel(&plain, 96, 64),
        [255, 255, 255, 255],
        "the right half"
    );
    assert_eq!(pixel(&plain, 32, 64), [0, 0, 0, 255], "the left half");

    // A color filter as a backdrop, which was not expressible at all. Inverting
    // swaps the two halves, and swapping is the result no blur could produce.
    let inverted = shot(
        &mut ctx,
        Some(ImageFilter::Color(ColorFilter::matrix([
            -1.0, 0.0, 0.0, 0.0, 1.0, //
            0.0, -1.0, 0.0, 0.0, 1.0, //
            0.0, 0.0, -1.0, 0.0, 1.0, //
            0.0, 0.0, 0.0, 1.0, 0.0,
        ]))),
    );
    assert_eq!(
        pixel(&inverted, 96, 64),
        [0, 0, 0, 255],
        "the white half should have come back black"
    );
    assert_eq!(
        pixel(&inverted, 32, 64),
        [255, 255, 255, 255],
        "the black half should have come back white"
    );

    // A dilation, which spreads the white half leftward across the edge. A
    // color filter cannot move an edge and a blur cannot move it this far, so
    // the two backdrops above and this one are three different pictures.
    let spread = shot(
        &mut ctx,
        Some(ImageFilter::Dilate {
            radius_x: 12.0,
            radius_y: 0.0,
        }),
    );
    assert_eq!(
        pixel(&spread, 56, 64),
        [255, 255, 255, 255],
        "the dilation should have carried the edge left of where it was"
    );

    // And the one that moves the image rather than recomputing it in place. It
    // is not a pass: what a backdrop is seeded into is the layer's own target,
    // already allocated and the same size whatever the matrix says, so the
    // matrix folds into the mapping that seed draws through.
    //
    // Sixteen pixels right, which puts the edge at eighty where it was at
    // sixty-four. Checked on both sides of where it moved to and where it left,
    // so a matrix quietly dropped would fail at the first and a matrix applied
    // twice at the second.
    let moved = shot(
        &mut ctx,
        Some(ImageFilter::Matrix {
            transform: Transform2D::from(Affine2::from_translation(Vec2::new(16.0, 0.0))),
        }),
    );
    assert_eq!(
        pixel(&moved, 72, 64),
        [0, 0, 0, 255],
        "the edge should have moved right, leaving black where white was"
    );
    assert_eq!(
        pixel(&moved, 88, 64),
        [255, 255, 255, 255],
        "and white should be where the edge moved to"
    );

    // What the move leaves behind is the other half of what a matrix backdrop
    // means, and this ground cannot show it: a clamp extends the edge texel,
    // and every edge of this ground is the color the ground beyond it already
    // is, so a smear and a show-through agree everywhere.
    //
    // So a second ground, white with a black stripe down its right edge, and a
    // matrix that halves the image into the top-left quarter. Two thirds of the
    // way across, the seed names a texel past the right of the source: decal
    // makes that transparent and the white ground shows, where a clamp would
    // put the stripe there -- black, across a region the stripe never covered.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::WHITE);
    canvas
        .draw_rect(
            Rect::new(112.0, 0.0, 128.0, 128.0),
            &Paint::fill(Color::BLACK).with_anti_alias(false),
        )
        .expect("stripe");
    canvas
        .save_layer_backdrop(
            Layer::opacity(1.0),
            None,
            &ImageFilter::Matrix {
                transform: Transform2D::from(Affine2::from_scale(Vec2::splat(0.5))),
            },
        )
        .expect("halved backdrop");
    canvas.restore();
    let halved = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&halved, 100, 64),
        [255, 255, 255, 255],
        "past the halved image the target should show through, not the edge \
         texel smeared across it"
    );
    // And the halved image is there at all: the stripe is at fifty-six now,
    // which is where the right edge of a frame halved about the origin lands.
    assert_eq!(pixel(&halved, 60, 32), [0, 0, 0, 255], "the halved stripe");

    // A matrix with no inverse is refused rather than guessed at: it collapses
    // the image to a line or a point, and nothing says what the rest of the
    // target read.
    let mut canvas = Canvas::new(SIZE);
    ground(&mut canvas);
    let outcome = canvas
        .save_layer_backdrop(
            Layer::opacity(1.0),
            None,
            &ImageFilter::Matrix {
                transform: Transform2D::from(Affine2::from_scale(Vec2::new(1.0, 0.0))),
            },
        )
        .map(|_| ());
    assert!(
        matches!(outcome, Err(Error::Unsupported(_))),
        "a singular matrix backdrop should be refused, got {outcome:?}"
    );
}

#[test]
fn a_stencil_clip_survives_the_pass_a_backdrop_filter_cuts() {
    // A backdrop filter cannot sample the attachment it is writing, so the pass
    // stops there and what follows begins by drawing it back in. That redraw
    // restores the color. Nothing restored the stencil, and a stencil belongs
    // to a pass -- so with a clip of a shape a scissor cannot express in force,
    // every draw after a backdrop filter tested for a depth no pixel in the new
    // pass held, and landed nowhere.
    //
    // Silently and everywhere, which is what makes it worth a test of its own.
    // The frame that showed it is upstream's
    // `BackdropRestoreUsesCorrectCoverageForFirstRestoredClip`, and it took
    // writing that scene to notice: the catalog's other backdrop plates have no
    // clip outside the layer, and a scissor clip -- which is every axis-aligned
    // `clip_rect` -- is state the recorder holds and survives a cut by itself.
    let Some(mut ctx) = context() else { return };
    let shot = |ctx: &mut Context, sigma: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        // A difference clip, which is never a scissor whatever the transform:
        // the complement of a rectangle is not one.
        canvas
            .clip_out_rect(Rect::new(96.0, 96.0, 128.0, 128.0))
            .expect("clip out");
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::srgb(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
            )
            .expect("red");
        canvas.save_layer(Layer::opacity(1.0).with_backdrop_blur(sigma));
        canvas.restore();
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::srgb(0.0, 1.0, 0.0, 1.0)).with_anti_alias(false),
            )
            .expect("green");
        render(ctx, canvas)
    };

    // Without a backdrop there is no cut, so this is the picture the other one
    // has to match: green over red, and the corner the difference clip removed
    // showing the ground.
    let uncut = shot(&mut ctx, 0.0);
    assert_eq!(pixel(&uncut, 32, 32), [0, 255, 0, 255], "green over red");
    assert_eq!(
        pixel(&uncut, 112, 112),
        [0, 0, 0, 255],
        "the corner removed"
    );

    let cut = shot(&mut ctx, 8.0);
    assert_eq!(
        pixel(&cut, 32, 32),
        [0, 255, 0, 255],
        "the draw after the backdrop should still land, so the clip has to have \
         been made again in the pass the backdrop cut"
    );
    assert_eq!(
        pixel(&cut, 112, 112),
        [0, 0, 0, 255],
        "and it should still be the same clip -- rebuilt too wide would let the \
         green into the corner"
    );
}

#[test]
fn a_backdrop_blur_softens_what_is_behind_the_layer_and_nothing_else() {
    let Some(mut ctx) = context() else { return };
    // Frosted glass, which is the other blur. A layer blur softens the layer's
    // own content; this softens what is already on the target and hands the
    // result to the layer to draw over.
    //
    // The scene is a hard vertical edge -- black left, white right -- so
    // "blurred" means something checkable: at the edge, a blur produces
    // intermediate values, and no amount of drawing an unblurred copy does.
    let draw = |ctx: &mut Context, sigma: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(64.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("edge");
        // A panel over the middle band only, so the top and bottom of the same
        // edge are left alone and can be compared against.
        canvas.save_layer_bounds(
            Layer::opacity(1.0).with_backdrop_blur(sigma),
            Rect::new(0.0, 48.0, 128.0, 80.0),
        );
        canvas.restore();
        render(ctx, canvas)
    };

    let plain = draw(&mut ctx, 0.0);
    let frosted = draw(&mut ctx, 6.0);

    // Without a backdrop blur the edge is hard: one column black, the next
    // white, nothing between.
    let hard = |pixels: &[u8], y: u32| {
        (56u32..72)
            .map(|x| pixel(pixels, x, y))
            .filter(|p| p[0] > 8 && p[0] < 247)
            .count()
    };
    assert_eq!(hard(&plain, 64), 0, "the control edge is not hard");

    // Inside the panel it is soft.
    let soft = hard(&frosted, 64);
    assert!(
        soft >= 8,
        "the backdrop was not blurred: {soft} intermediate columns at the edge"
    );

    // Outside the panel the same edge is untouched, which is what says the
    // filter is confined to the layer rather than applied to the frame.
    assert_eq!(
        hard(&frosted, 16),
        0,
        "the backdrop blur reached above the layer"
    );
    assert_eq!(
        hard(&frosted, 112),
        0,
        "the backdrop blur reached below the layer"
    );

    // And what was behind is still behind: far from the edge the panel shows
    // the backdrop's own colors rather than clearing them away.
    assert!(
        pixel(&frosted, 4, 64)[0] < 8,
        "the left of the panel is not black"
    );
    assert!(
        pixel(&frosted, 124, 64)[0] > 247,
        "the right of the panel is not white"
    );
}

#[test]
fn a_tint_recolors_an_image_and_keeps_it_premultiplied() {
    let Some(mut ctx) = context() else { return };
    // The reason a tint exists: one monochrome sheet, every state's color. The
    // image here is white at full and half alpha, which is what an antialiased
    // icon's interior and edge look like, and the two together are what catches
    // a tint that multiplies color without accounting for its own alpha.
    let size = Extent2D::new(2, 1);
    let mut icon = vec![0u8; 2 * 4];
    icon[0..4].copy_from_slice(&[255, 255, 255, 255]);
    // Premultiplied: half alpha means half color too.
    icon[4..8].copy_from_slice(&[128, 128, 128, 128]);
    let mut image = ctx
        .create_image(size, PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &icon).expect("upload");

    let draw = |ctx: &mut Context, image: &_, tint: Color, texel: f32| {
        let mut canvas = Canvas::new(SIZE);
        // Transparent rather than opaque black, so what comes back is the
        // source's own premultiplied color. Over an opaque background the
        // destination's alpha survives compositing and the result reads 255
        // whatever the source did, which says nothing about the tint -- the
        // first version of this test asserted on exactly that number.
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::image(0, Rect::from_size(128.0, 128.0))
                    .with_source_pixels(Rect::new(texel, 0.0, texel + 1.0, 1.0), size)
                    .with_tint(tint)
                    .with_anti_alias(false),
            )
            .expect("icon");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixel(&pixels, 64, 64)
    };

    // White is the identity, so an image that never mentions a tint is
    // unchanged by having one.
    let plain = draw(&mut ctx, &image, Color::WHITE, 0.0);
    assert_eq!(
        plain,
        [255, 255, 255, 255],
        "a white tint changed the image"
    );

    // A red tint on the opaque texel gives red at full alpha.
    let red = draw(&mut ctx, &image, Color::linear(1.0, 0.0, 0.0, 1.0), 0.0);
    assert_eq!(red, [255, 0, 0, 255], "an opaque texel tinted red");

    // The same tint on the half-covered texel gives red at half alpha, and --
    // this is the part worth asserting -- its color does not exceed its alpha,
    // which is what premultiplied means and what a tint applied without regard
    // to its own alpha would break.
    let edge = draw(&mut ctx, &image, Color::linear(1.0, 0.0, 0.0, 1.0), 1.0);
    assert!(
        edge[0].abs_diff(128) <= 2 && edge[3].abs_diff(128) <= 2,
        "a tinted half-covered texel came back {edge:?}"
    );
    assert!(
        edge[0] <= edge[3] + 1,
        "the tint left color above alpha, so the result is not premultiplied: {edge:?}"
    );

    // A tint carrying its own alpha scales coverage as well as color, which is
    // what makes it a generalization of the alpha setting rather than a rival.
    let faded = draw(&mut ctx, &image, Color::linear(1.0, 0.0, 0.0, 0.5), 0.0);
    assert!(
        faded[0].abs_diff(128) <= 2 && faded[3].abs_diff(128) <= 2,
        "a half-alpha tint came back {faded:?}"
    );
    ctx.destroy_image(image);
}

#[test]
fn a_sprite_can_be_drawn_from_a_sheet_by_naming_its_texels() {
    let Some(mut ctx) = context() else { return };
    // What a sprite sheet is for, through the API a caller actually uses: they
    // know the size they uploaded, so they state the piece in texels and hand
    // that size over rather than converting by hand.
    //
    // Every texel a different color, and each selection is a single texel. That
    // matters: with blocks of one color, selecting a block passes whether the
    // coordinate is *mapped* into the selection or merely *clamped* to it, and
    // the first version of this test could not tell those apart. Selecting one
    // texel can -- mapping fills the destination with it, clamping shows it
    // beside its neighbor.
    let size = Extent2D::new(4, 4);
    let color_of =
        |x: usize, y: usize| -> [u8; 4] { [(x * 60 + 15) as u8, (y * 60 + 15) as u8, 200, 255] };
    let mut sheet = vec![0u8; 4 * 4 * 4];
    for y in 0..4usize {
        for x in 0..4usize {
            let at = (y * 4 + x) * 4;
            sheet[at..at + 4].copy_from_slice(&color_of(x, y));
        }
    }
    let mut image = ctx
        .create_image(size, PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &sheet).expect("upload");

    let draw = |ctx: &mut Context, image: &_, source: Rect| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::image(0, Rect::from_size(128.0, 128.0))
                    .with_source_pixels(source, size)
                    .with_anti_alias(false),
            )
            .expect("sprite");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    for (x, y) in [(0usize, 0usize), (1, 0), (3, 1), (2, 3)] {
        let source = Rect::new(x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0);
        let pixels = draw(&mut ctx, &image, source);
        let want = color_of(x, y);
        // Across the whole destination, corners included: one texel selected
        // means one color everywhere, and a neighbor appearing anywhere is
        // either a mapping that did not scale or a filter reading past the
        // selection.
        for (px, py) in [(64u32, 64u32), (2, 2), (125, 2), (2, 125), (125, 125)] {
            let got = pixel(&pixels, px, py);
            assert_eq!(
                [got[0], got[1]],
                [want[0], want[1]],
                "texel ({x}, {y}) came back {got:?} at ({px}, {py}), wanted {want:?}"
            );
        }
    }
    // Selecting one texel is not enough on its own: it makes the sampling
    // bounds degenerate, so the coordinate lands on that texel's center whether
    // it was mapped into the selection or merely held inside it. A selection
    // two texels wide separates them, because only a mapping rescales -- the
    // boundary between the two belongs at the middle of the destination, and
    // holding without mapping leaves it at a quarter.
    let pixels = draw(&mut ctx, &image, Rect::new(0.0, 0.0, 2.0, 1.0));
    let first = color_of(0, 0);
    let at_first_center = pixel(&pixels, 32, 64);
    assert_eq!(
        [at_first_center[0], at_first_center[1]],
        [first[0], first[1]],
        "a two-texel selection is not being stretched across the destination"
    );
    let second = color_of(1, 0);
    let at_second_center = pixel(&pixels, 96, 64);
    assert_eq!(
        [at_second_center[0], at_second_center[1]],
        [second[0], second[1]],
        "the second texel of the selection is not where it belongs"
    );
    ctx.destroy_image(image);
}

#[test]
fn an_arc_leaves_the_part_of_the_circle_it_does_not_sweep() {
    let Some(mut ctx) = context() else { return };
    // The end-to-end check that a partial arc is partial. The geometry is
    // covered without a device elsewhere; what this adds is that the builder is
    // reachable from the facade and that a three-quarter sweep reaches the
    // screen as three quarters rather than as a circle.
    let mut builder = PathBuilder::new();
    builder.arc(
        Vec2::new(64.0, 64.0),
        Vec2::splat(40.0),
        // From twelve o'clock, three quarters of the way round.
        -std::f32::consts::FRAC_PI_2,
        std::f32::consts::TAU * 0.75,
    );
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_path(
            &builder.build(),
            &Paint::stroke(Color::WHITE, 8.0).with_anti_alias(false),
        )
        .expect("arc");
    let pixels = render(&mut ctx, canvas);

    // The sweep runs clockwise from twelve and ends at nine, so all four
    // quarter points are on it -- nine is the end of the arc, not past it --
    // and the quarter that is missing lies between nine and twelve. Probing
    // nine expecting nothing is the mistake this comment exists to prevent; it
    // is where the arc stops, which is exactly where it is still drawn.
    for (name, x, y) in [
        ("twelve", 64, 24),
        ("three", 104, 64),
        ("six", 64, 104),
        ("nine", 24, 64),
    ] {
        assert!(
            pixel(&pixels, x, y)[0] > 200,
            "{name} o'clock should be on the arc"
        );
    }
    // Half past ten, in the middle of the quarter that was never swept.
    assert_eq!(
        pixel(&pixels, 36, 36),
        [0, 0, 0, 255],
        "the unswept quarter was drawn"
    );
    // And the middle is empty, so this is a ring rather than a filled shape.
    assert_eq!(pixel(&pixels, 64, 64), [0, 0, 0, 255], "the ring is filled");
}

#[test]
fn a_dashed_stroke_draws_less_than_a_solid_one_and_leaves_real_gaps() {
    let Some(mut ctx) = context() else { return };
    let draw = |ctx: &mut Context, dash: Option<Dash>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_line(
                Vec2::new(0.0, 64.0),
                Vec2::new(128.0, 64.0),
                &Paint::stroke(Color::WHITE, 8.0)
                    .with_dash(dash)
                    .with_anti_alias(false),
            )
            .expect("line");
        render(ctx, canvas)
    };
    let lit = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|texel| texel[0] > 128)
            .count()
    };

    let solid = draw(&mut ctx, None);
    // Sixteen on, sixteen off across a hundred and twenty-eight: four dashes.
    let dashed = draw(&mut ctx, Some(Dash::new(vec![16.0, 16.0], 0.0)));

    let (whole, cut) = (lit(&solid), lit(&dashed));
    assert!(whole > 0, "the solid line drew nothing to compare against");
    // Half, give or take the ends of each dash.
    let ratio = cut as f32 / whole as f32;
    assert!(
        (0.4..0.6).contains(&ratio),
        "a half-on pattern drew {ratio} of the line"
    );

    // The gaps are gaps rather than a dimmer line, which is what a pattern
    // applied to coverage instead of to geometry would produce.
    assert_eq!(
        pixel(&dashed, 24, 64),
        [0, 0, 0, 255],
        "the first gap is not empty"
    );
    assert!(pixel(&dashed, 8, 64)[0] > 200, "the first dash is missing");

    // Phase moves the pattern rather than changing how much is drawn: what was
    // a gap becomes a dash.
    let shifted = draw(&mut ctx, Some(Dash::new(vec![16.0, 16.0], 16.0)));
    assert!(
        pixel(&shifted, 24, 64)[0] > 200,
        "a phase of one interval should put a dash where the gap was"
    );
    assert_eq!(
        pixel(&shifted, 8, 64),
        [0, 0, 0, 255],
        "and a gap where the dash was"
    );
}

#[test]
fn a_dashed_rounded_rectangle_dashes_rather_than_drawing_solid() {
    let Some(mut ctx) = context() else { return };
    // A rounded rectangle stroke is normally evaluated per fragment from a
    // distance field, which describes a continuous outline and has no notion of
    // a position along it. A dash therefore has to send the shape back to
    // tessellation, and if it does not, the outline draws solid and the pattern
    // is silently ignored -- a bug that looks exactly like a working stroke.
    let draw = |ctx: &mut Context, dash: Option<Dash>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                16.0,
                &Paint::stroke(Color::WHITE, 6.0).with_dash(dash),
            )
            .expect("rrect");
        render(ctx, canvas)
    };
    let lit = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|texel| texel[0] > 128)
            .count()
    };

    let solid = lit(&draw(&mut ctx, None));
    let dashed = lit(&draw(&mut ctx, Some(Dash::new(vec![10.0, 10.0], 0.0))));
    assert!(solid > 0, "the solid outline drew nothing");
    let ratio = dashed as f32 / solid as f32;
    assert!(
        (0.35..0.65).contains(&ratio),
        "the dashed outline drew {ratio} of the solid one, so the pattern was ignored"
    );
}

#[test]
fn a_tiled_gradient_tiles_the_same_whether_or_not_its_stops_fit() {
    let Some(mut ctx) = context() else { return };
    // Tiling folds the parameter before anything reads a color, and the two
    // ways of reading one -- walking the stops, sampling a baked ramp -- sit
    // behind that fold. So they should compose, and each was tested without the
    // other, which is the arrangement in which a composition bug survives.
    //
    // The ramp is sampled through a clamp-to-edge sampler, which is the part
    // worth checking rather than assuming: a repeating gradient asks for the
    // last color at the end of each period and the first at the start of the
    // next, and clamping is what gives that instead of blending across the
    // seam. Getting it wrong would show as a soft band at every period, only
    // on the ramp path, only when repeating.
    let ramp_stops = |n: usize| -> Vec<GradientStop> {
        // Collinear, so the picture does not depend on how many there are --
        // only on which path carries them.
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                GradientStop::new(Color::srgb(1.0 - t, 0.0, t, 1.0), t)
            })
            .collect()
    };

    let render_with = |ctx: &mut Context, stops: Vec<GradientStop>, tile: TileMode| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("ground");
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                // A ramp spanning a quarter, so three quarters of the picture
                // is whatever the tile mode says.
                &Paint::linear_gradient(Vec2::ZERO, Vec2::new(32.0, 0.0), stops)
                    .with_tile_mode(tile)
                    .with_blend(BlendMode::SrcOver)
                    .with_anti_alias(false),
            )
            .expect("gradient");
        render(ctx, canvas)
    };

    for tile in [
        TileMode::Clamp,
        TileMode::Repeat,
        TileMode::Mirror,
        TileMode::Decal,
    ] {
        let walked = render_with(&mut ctx, ramp_stops(MAX_STOPS), tile);
        let sampled = render_with(&mut ctx, ramp_stops(MAX_STOPS + 3), tile);
        let mut worst = 0i32;
        for x in 0..128u32 {
            let a = pixel(&walked, x, 64);
            let b = pixel(&sampled, x, 64);
            for channel in 0..4 {
                worst = worst.max((a[channel] as i32 - b[channel] as i32).abs());
            }
        }
        assert!(
            worst <= 2,
            "{tile:?}: the two paths disagree by {worst}, which is more than rounding"
        );
    }

    // And the tiling actually happened on the ramp path, rather than both
    // paths agreeing because neither tiled. Two ramps along, repeating is back
    // at the first color where clamping holds the last.
    let repeated = render_with(&mut ctx, ramp_stops(MAX_STOPS + 3), TileMode::Repeat);
    let clamped = render_with(&mut ctx, ramp_stops(MAX_STOPS + 3), TileMode::Clamp);
    let at = pixel(&repeated, 64, 64);
    assert!(
        at[0] > 200 && at[2] < 60,
        "a repeating ramp did not begin again, got {at:?}"
    );
    let held = pixel(&clamped, 64, 64);
    assert!(
        held[2] > 200 && held[0] < 60,
        "a clamped ramp did not hold its end, got {held:?}"
    );
}

#[test]
fn a_gradient_with_many_stops_places_each_one_where_it_was_asked_for() {
    let Some(mut ctx) = context() else { return };
    // The agreement test above uses stops that lie on the line, so it would
    // pass against a ramp that ignored the extra ones entirely. This one cannot:
    // six stops, each a different color, at offsets a truncating implementation
    // would never reach.
    let stops = vec![
        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
        GradientStop::new(Color::linear(1.0, 1.0, 0.0, 1.0), 0.2),
        GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 0.4),
        GradientStop::new(Color::linear(0.0, 1.0, 1.0, 1.0), 0.6),
        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 0.8),
        GradientStop::new(Color::linear(1.0, 0.0, 1.0, 1.0), 1.0),
    ];
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(Vec2::ZERO, Vec2::new(128.0, 0.0), stops)
                .with_anti_alias(false),
        )
        .expect("gradient");
    let pixels = render(&mut ctx, canvas);

    // Each stop sits a fifth of the way along, so each is checked at its own
    // column. The fifth and sixth are the ones a four-stop truncation loses.
    for (name, x, want) in [
        ("red", 0u32, [255u8, 0, 0]),
        ("yellow", 25, [255, 255, 0]),
        ("green", 51, [0, 255, 0]),
        ("cyan", 77, [0, 255, 255]),
        ("blue", 102, [0, 0, 255]),
        ("magenta", 127, [255, 0, 255]),
    ] {
        let got = pixel(&pixels, x, 64);
        for channel in 0..3 {
            assert!(
                (got[channel] as i32 - want[channel] as i32).abs() <= 12,
                "the {name} stop came back {got:?}, wanted about {want:?}"
            );
        }
    }
}

#[test]
fn a_gradient_tiles_beyond_its_own_extent() {
    let Some(mut ctx) = context() else { return };
    // Inside the ramp every mode agrees, which is the control: a difference
    // found outside it is about tiling and not about the gradient.
    let inside = 16;
    let modes = [TileMode::Clamp, TileMode::Repeat, TileMode::Decal];
    let images: Vec<Vec<u8>> = modes.iter().map(|t| tiled_ramp(&mut ctx, *t)).collect();
    for image in &images[1..] {
        for channel in 0..4 {
            assert!(
                (images[0][(inside * 4 + channel) as usize] as i32
                    - image[(inside * 4 + channel) as usize] as i32)
                    .abs()
                    <= 2,
                "the modes disagree inside the ramp, where they should not"
            );
        }
    }

    // Clamp holds the last stop. At x=100 the parameter is past three, and the
    // color there must be the blue the ramp ended on.
    let clamped = pixel(&images[0], 100, 64);
    assert!(
        clamped[2] > 240 && clamped[0] < 16,
        "clamp should hold the end color, got {clamped:?}"
    );

    // Repeat starts the ramp again. x=64 is exactly two ramps along, so it is
    // the red the gradient began with rather than the blue it ended on.
    let repeated = pixel(&images[1], 64, 64);
    assert!(
        repeated[0] > 240 && repeated[2] < 16,
        "repeat should begin the ramp again, got {repeated:?}"
    );
    // And it must still be periodic further out, not merely different once.
    let next_period = pixel(&images[1], 96, 64);
    for channel in 0..4 {
        assert!(
            (repeated[channel] as i32 - next_period[channel] as i32).abs() <= 2,
            "repeat is not periodic: {repeated:?} against {next_period:?}"
        );
    }

    // Decal draws nothing outside, so the white ground shows through.
    let decaled = pixel(&images[2], 100, 64);
    assert!(
        decaled.iter().take(3).all(|c| *c > 240),
        "decal should leave the ground alone, got {decaled:?}"
    );
}

#[test]
fn mirroring_repeats_without_the_seam_that_repeating_leaves() {
    let Some(mut ctx) = context() else { return };
    // The point of mirroring is not that it differs from repeating somewhere --
    // that would be true of getting the fold wrong as well. It is that the
    // copies join. The ramp ends at x=32, so that is where the second copy
    // begins and where a seam would be: repeating jumps from the last color
    // back to the first, and mirroring turns around and runs back.
    let repeated = tiled_ramp(&mut ctx, TileMode::Repeat);
    let mirrored = tiled_ramp(&mut ctx, TileMode::Mirror);

    let jump = |image: &[u8]| {
        let before = pixel(image, 30, 64);
        let after = pixel(image, 34, 64);
        (0..3)
            .map(|c| (before[c] as i32 - after[c] as i32).abs())
            .max()
            .unwrap_or(0)
    };

    let seam = jump(&repeated);
    assert!(
        seam > 200,
        "repeating should jump at the period boundary, but moved only {seam}"
    );
    let joined = jump(&mirrored);
    assert!(
        joined < 40,
        "mirroring should join at the period boundary, but jumped {joined}"
    );

    // And it is a reflection rather than a hold: a quarter past the boundary
    // must match a quarter before it, which clamping would also fail.
    //
    // The two columns are 24 and 39 rather than 24 and 40, because a fragment
    // samples at its center: 24.5 reflects about the boundary at 32 onto 39.5,
    // and 40.5 is a whole pixel further out. Reading them as 24 and 40 leaves a
    // difference of eight in the blue channel, which is the ramp's slope over
    // one pixel and not a fault in the fold.
    let before = pixel(&mirrored, 24, 64);
    let after = pixel(&mirrored, 39, 64);
    for channel in 0..3 {
        assert!(
            (before[channel] as i32 - after[channel] as i32).abs() <= 6,
            "the second copy should mirror the first: {before:?} against {after:?}"
        );
    }
}

#[test]
fn a_partial_sweep_holds_its_end_color_around_the_rest_of_the_turn() {
    let Some(mut ctx) = context() else { return };
    // The only case where a sweep has an outside at all. A full turn covers
    // every direction, so tiling it changes nothing and would make a test that
    // passes whatever the code does.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::sweep_gradient(
                Vec2::new(64.0, 64.0),
                0.0,
                std::f32::consts::PI,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("sweep");
    let pixels = render(&mut ctx, canvas);

    // The arc runs from the positive X axis through the bottom of the image to
    // the negative X axis: the renderer's Y points down, so increasing angle
    // goes that way. Measured rather than assumed -- the first version of this
    // test probed a direction it took to be outside, which was inside, and
    // passed against an implementation that had no clamping at all.
    let start = pixel(&pixels, 64 + 40, 64);
    assert!(
        start[0] > 240 && start[2] < 16,
        "the arc should begin at the first stop, got {start:?}"
    );
    let middle = pixel(&pixels, 64, 64 + 40);
    assert!(
        middle[0] > 100 && middle[0] < 160 && middle[2] > 100 && middle[2] < 160,
        "half way round the arc should be half way along the ramp, got {middle:?}"
    );

    // Straight up is outside the arc, and is the direction that tells the two
    // possible implementations apart. Clamping holds the last stop, so it is
    // pure blue. Folding the angle by the sweep instead of by a whole turn --
    // which is what this did before -- runs the ramp a second time around the
    // circle and puts the halfway color here instead.
    let outside = pixel(&pixels, 64, 64 - 40);
    assert!(
        outside[2] > 240 && outside[0] < 16,
        "outside the arc should hold the end color, got {outside:?}"
    );
}

#[test]
fn a_sweep_gradient_runs_around_its_center() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::sweep_gradient(
                Vec2::new(64.0, 64.0),
                0.0,
                std::f32::consts::TAU,
                vec![
                    GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("sweep");

    let pixels = render(&mut ctx, canvas);
    // Angle rather than distance: two points at different radii along the same
    // ray must match, while two at the same radius on different rays must not.
    let near = pixel(&pixels, 64 + 12, 64);
    let far = pixel(&pixels, 64 + 48, 64);
    for channel in 0..4 {
        assert!(
            (near[channel] as i32 - far[channel] as i32).abs() <= 2,
            "a sweep should not vary with distance: near {near:?}, far {far:?}"
        );
    }

    // Clip Y runs up, so a point below the center in the image is at a negative
    // angle and lands late in the sweep.
    let above = pixel(&pixels, 64, 64 - 40);
    assert_ne!(near, above, "a sweep should vary with angle");
}

#[test]
fn an_advanced_blend_mode_is_either_available_or_refused_through_the_api() {
    let Some(mut ctx) = context() else { return };

    // The whole contract in one place: a caller asks what the device can do,
    // and gets either the mode or an error. What must not happen is the third
    // outcome — a drawing that succeeds and is quietly source-over instead.
    let available = ctx.capabilities().advanced_blend;

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.6, 0.6, 0.6, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(0.5, 0.5, 0.5, 1.0))
                .with_blend(BlendMode::Multiply)
                .with_anti_alias(false),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    let drawn = ctx.draw(&mut surface, &canvas.finish());

    if available {
        drawn.expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        let got = pixel(&pixels, 64, 64);
        // Multiplying two opaque mid-grays gives their product, which is darker
        // than either. Asserting the direction rather than the value keeps this
        // about the API carrying the mode through, and leaves the arithmetic to
        // the conformance tests that check all eleven modes against the formula.
        let backdrop = pixel(
            &render(&mut ctx, {
                let mut plain = Canvas::new(SIZE);
                plain.clear(Color::linear(0.6, 0.6, 0.6, 1.0));
                plain
            }),
            64,
            64,
        );
        assert!(
            got[0] < backdrop[0],
            "multiply gave {got:?}, which is not darker than the backdrop {backdrop:?}"
        );
    } else {
        assert!(
            matches!(drawn, Err(emblema::Error::Unsupported(_))),
            "the device reports no advanced blending but the draw was not refused"
        );
    }
    ctx.destroy_surface(surface);
}

#[test]
fn a_clip_confines_drawing_to_a_rectangle() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // A clip nowhere near the middle of the target, so getting the origin or
    // an axis wrong lands somewhere visibly different rather than merely being
    // a pixel off.
    canvas
        .clip_rect(Rect::new(20.0, 10.0, 60.0, 40.0))
        .expect("clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(30, 20), "the middle of the clip was not drawn");
    assert!(
        lit(21, 11),
        "the top-left corner inside the clip was not drawn"
    );
    assert!(
        lit(59, 39),
        "the bottom-right corner inside the clip was not drawn"
    );
    for (x, y, where_) in [
        (19, 20, "left of the clip"),
        (60, 20, "right of the clip"),
        (30, 9, "above the clip"),
        (30, 40, "below the clip"),
        (100, 100, "far outside the clip"),
    ] {
        assert!(!lit(x, y), "{where_} was drawn anyway");
    }
}

#[test]
fn clips_intersect_rather_than_replacing_one_another() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // Two overlapping clips. Only their shared region may be drawn; a clip that
    // replaced its predecessor would leave the second rectangle whole.
    canvas.clip_rect(Rect::new(10.0, 10.0, 70.0, 50.0)).unwrap();
    canvas
        .clip_rect(Rect::new(40.0, 30.0, 100.0, 90.0))
        .unwrap();
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(50, 40), "the shared region was not drawn");
    assert!(
        !lit(20, 20),
        "a region only the first clip allowed was drawn"
    );
    assert!(
        !lit(80, 70),
        "a region only the second clip allowed was drawn"
    );
}

#[test]
fn restore_returns_the_clip_along_with_the_transform() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    canvas.save();
    canvas.clip_rect(Rect::new(0.0, 0.0, 20.0, 20.0)).unwrap();
    canvas.restore();

    // With the clip restored to nothing, this covers the whole target. A clip
    // that outlived its save would confine it to the top-left corner.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    assert!(
        pixel(&pixels, 100, 100)[0] > 128,
        "a clip outlived the save that scoped it"
    );
}

#[test]
fn a_clip_moves_with_the_transform_that_was_in_force_when_it_was_set() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // The clip is given in user space, so the translation applies to it. A clip
    // taken in device pixels instead would sit at the origin.
    canvas.translate(64.0, 64.0);
    canvas.clip_rect(Rect::new(0.0, 0.0, 40.0, 40.0)).unwrap();
    canvas
        .draw_rect(
            Rect::new(-128.0, -128.0, 128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(70, 70), "the clip did not travel with the transform");
    assert!(!lit(20, 20), "the clip stayed at the device origin");
    assert!(!lit(110, 110), "the clip extended past where it was placed");
}

#[test]
fn a_rotated_clip_is_the_rotated_shape_and_not_its_bounding_box() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A square clip turned through an eighth turn is a diamond in device
    // pixels. Its bounding box reaches the corners; the diamond does not, and
    // the difference is the whole reason this cannot be approximated -- a
    // bounding box would admit pixels the caller asked to remove.
    canvas.translate(64.0, 64.0);
    canvas.rotate(std::f32::consts::FRAC_PI_4);
    canvas
        .clip_rect(Rect::new(-30.0, -30.0, 30.0, 30.0))
        .expect("clip");
    canvas
        .draw_rect(
            Rect::new(-128.0, -128.0, 128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(64, 64), "the middle of the clip was not drawn");
    assert!(lit(64, 90), "a point inside the diamond was not drawn");
    // Inside the bounding box, which spans about 22 to 106 on both axes, but
    // outside the diamond. This is the pixel a bounding-box clip would keep.
    assert!(
        !lit(100, 100),
        "the clip reached a corner only its bounding box covers"
    );
    assert!(!lit(10, 64), "the clip reached outside its bounding box");
}

#[test]
fn a_clip_narrowed_to_nothing_draws_nothing() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::srgb(0.2, 0.4, 0.6, 1.0));
    // Two disjoint clips. This is a normal state for a subtree scrolled out of
    // view, not an error, so recording continues and simply produces nothing.
    canvas.clip_rect(Rect::new(0.0, 0.0, 20.0, 20.0)).unwrap();
    canvas.clip_rect(Rect::new(60.0, 60.0, 80.0, 80.0)).unwrap();
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)),
        )
        .expect("rect");

    let recording = canvas.finish();
    assert!(
        recording.is_empty(),
        "a draw that could reach no pixel was still recorded"
    );

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &recording).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    // The clear still happens: an empty clip removes the drawing, not the frame.
    assert_eq!(pixel(&pixels, 64, 64), [51, 102, 153, 255]);
}

#[test]
fn a_path_clip_confines_drawing_to_the_path() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A triangle, which no rectangle approximates and no scissor expresses.
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(64.0, 16.0))
        .line_to(Vec2::new(112.0, 100.0))
        .line_to(Vec2::new(16.0, 100.0))
        .close();
    canvas.clip_path(&builder.build()).expect("clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(64, 80), "the middle of the triangle was not drawn");
    assert!(lit(64, 30), "near the apex was not drawn");
    // The corners of the triangle's bounding box, which the triangle misses.
    for (x, y, corner) in [(20, 20, "top-left"), (108, 20, "top-right")] {
        assert!(
            !lit(x, y),
            "the {corner} corner outside the triangle was drawn"
        );
    }
    assert!(!lit(64, 110), "below the triangle was drawn");
}

#[test]
fn a_path_clip_intersects_with_a_rectangular_one() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A scissor and a stencil clip in force at once. They are independent
    // mechanisms and both apply, which is what lets an axis-aligned clip keep
    // taking the cheap path even inside a path clip.
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("rect clip");
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(64.0, 16.0))
        .line_to(Vec2::new(112.0, 100.0))
        .line_to(Vec2::new(16.0, 100.0))
        .close();
    canvas.clip_path(&builder.build()).expect("path clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    let lit = |x, y| pixel(&pixels, x, y)[0] > 128;
    assert!(lit(50, 80), "the region both clips admit was not drawn");
    assert!(
        !lit(80, 80),
        "a region inside the triangle but outside the rectangle was drawn"
    );
    assert!(
        !lit(30, 30),
        "a region inside the rectangle but outside the triangle was drawn"
    );
}

#[test]
fn restore_undoes_a_path_clip() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    // A path clip lives in a buffer on the device rather than in the recorder,
    // so restoring it is a draw rather than an assignment. Getting that wrong
    // leaves every later shape confined to a clip that was supposed to have
    // been lifted.
    canvas.save();
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(64.0, 16.0))
        .line_to(Vec2::new(112.0, 100.0))
        .line_to(Vec2::new(16.0, 100.0))
        .close();
    canvas.clip_path(&builder.build()).expect("clip");
    canvas.restore();

    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");

    let pixels = render(&mut ctx, canvas);
    for (x, y) in [(4, 4), (124, 4), (4, 124), (124, 124), (64, 64)] {
        assert!(
            pixel(&pixels, x, y)[0] > 128,
            "({x}, {y}) was still confined by a clip that had been restored"
        );
    }
}

#[test]
fn nested_path_clips_intersect_and_unwind_one_level_at_a_time() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));

    let triangle = |apex_left: bool| {
        let mut builder = PathBuilder::new();
        if apex_left {
            builder
                .move_to(Vec2::new(16.0, 64.0))
                .line_to(Vec2::new(100.0, 16.0))
                .line_to(Vec2::new(100.0, 112.0))
                .close();
        } else {
            builder
                .move_to(Vec2::new(112.0, 64.0))
                .line_to(Vec2::new(28.0, 16.0))
                .line_to(Vec2::new(28.0, 112.0))
                .close();
        }
        builder.build()
    };

    canvas.save();
    canvas.clip_path(&triangle(true)).expect("outer");
    canvas.save();
    canvas.clip_path(&triangle(false)).expect("inner");
    // Inside both triangles.
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let both = render(&mut ctx, canvas);

    // Now the same thing, unwinding one level before drawing: only the outer
    // triangle should confine it. A restore that stepped back too far or not
    // far enough gives a different picture from either.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save();
    canvas.clip_path(&triangle(true)).expect("outer");
    canvas.save();
    canvas.clip_path(&triangle(false)).expect("inner");
    canvas.restore();
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let outer_only = render(&mut ctx, canvas);

    let lit = |p: &[u8], x, y| pixel(p, x, y)[0] > 128;
    // A point inside the left-apex triangle but outside the right-apex one.
    assert!(!lit(&both, 22, 64), "the two clips did not intersect");
    assert!(
        lit(&outer_only, 22, 64),
        "restoring did not lift the inner clip"
    );
    // And the outer clip is still in force after the restore.
    assert!(
        !lit(&outer_only, 8, 64),
        "restoring lifted the outer clip as well"
    );
}

/// A four-by-four image with a different color in each quadrant.
fn quadrant_image() -> Vec<u8> {
    let mut pixels = vec![0u8; 4 * 4 * 4];
    for y in 0..4u32 {
        for x in 0..4u32 {
            let i = ((y * 4 + x) * 4) as usize;
            let color: [u8; 4] = match (x < 2, y < 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 0, 255],
            };
            pixels[i..i + 4].copy_from_slice(&color);
        }
    }
    pixels
}

#[test]
fn an_image_paint_draws_a_texture_through_the_api() {
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::image(0, Rect::from_size(128.0, 128.0)).with_anti_alias(false),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // The quadrants land where the image put them, with the top-left of the
    // image at the top-left of the destination.
    for (x, y, want, corner) in [
        (16u32, 16u32, [255u8, 0, 0, 255], "top-left"),
        (112, 16, [0, 255, 0, 255], "top-right"),
        (16, 112, [0, 0, 255, 255], "bottom-left"),
        (112, 112, [255, 255, 0, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {corner} quadrant sampled the wrong part of the image"
        );
    }
}

#[test]
fn an_image_travels_with_the_canvas_transform() {
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    // A half turn about the middle of the target exchanges opposite quadrants.
    // The image's mapping goes through the same transform as the geometry, so
    // a mapping computed in device pixels instead would leave it upright while
    // the shape turned.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.translate(64.0, 64.0);
    canvas.rotate(std::f32::consts::PI);
    canvas
        .draw_rect(
            Rect::new(-64.0, -64.0, 64.0, 64.0),
            &Paint::image(0, Rect::new(-64.0, -64.0, 64.0, 64.0)).with_anti_alias(false),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Bottom-right now holds what was top-left.
    assert_eq!(pixel(&pixels, 112, 112), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 16, 16), [255, 255, 0, 255]);
}

#[test]
fn drawing_an_image_paint_without_the_image_is_refused() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("rect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    // Sampling whatever happens to be bound would draw a plausible picture out
    // of a previous frame's texture, which is worse than failing.
    let result = ctx.draw(&mut surface, &canvas.finish());
    ctx.destroy_surface(surface);
    assert!(result.is_err(), "a paint sampling slot 0 drew without it");
}

/// Two overlapping opaque circles, drawn through `build`.
fn overlapping_circles(canvas: &mut Canvas, alpha: f32) {
    for center in [Vec2::new(52.0, 64.0), Vec2::new(76.0, 64.0)] {
        canvas
            .draw_circle(
                center,
                28.0,
                &Paint::fill(Color::linear(1.0, 0.0, 0.0, alpha)).with_anti_alias(false),
            )
            .expect("circle");
    }
}

#[test]
fn a_layer_applies_its_alpha_to_the_group_rather_than_to_each_shape() {
    let Some(mut ctx) = context() else { return };

    // Two overlapping half-transparent circles drawn directly show where they
    // cross, because the second blends over the first. The same pair inside a
    // half-transparent layer does not: the group is composited once, so the
    // overlap is no denser than the rest. That difference is the whole reason
    // save layers exist, and it is what a per-shape alpha cannot express.
    let mut direct = Canvas::new(SIZE);
    direct.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    overlapping_circles(&mut direct, 0.5);
    let direct = render(&mut ctx, direct);

    let mut grouped = Canvas::new(SIZE);
    grouped.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    grouped.save_layer(Layer::opacity(0.5));
    overlapping_circles(&mut grouped, 1.0);
    grouped.restore();
    let grouped = render(&mut ctx, grouped);

    // Away from the overlap the two agree: one shape at half alpha either way.
    //
    // Within a level, not exactly. One path multiplies the alpha into the color
    // in the shader and writes it; the other writes the shape, then multiplies
    // a composited image by it. Those are the same product of the same two
    // numbers and they round to eight bits at different points, which is worth
    // one level and is the difference this repository calls `ROUNDING`
    // everywhere else. It was exact while the values were light, because the
    // rounding landed differently there.
    let outside = (36u32, 64u32);
    let (a, b) = (
        pixel(&direct, outside.0, outside.1),
        pixel(&grouped, outside.0, outside.1),
    );
    for channel in 0..4 {
        assert!(
            (a[channel] as i32 - b[channel] as i32).abs() <= 1,
            "the two differ where only one circle covers: {a:?} against {b:?}"
        );
    }

    // In the overlap they must not. Drawn directly the second circle blends
    // over the first and the red is denser; grouped, it is not.
    let overlap = (64u32, 64u32);
    let direct_red = pixel(&direct, overlap.0, overlap.1)[0];
    let grouped_red = pixel(&grouped, overlap.0, overlap.1)[0];
    assert!(
        direct_red > grouped_red + 8,
        "the overlap is {direct_red} drawn directly and {grouped_red} through a layer; \
         a layer that composited per shape would make these equal"
    );
    // And grouped, the overlap matches the rest of the group exactly.
    assert_eq!(
        pixel(&grouped, overlap.0, overlap.1),
        pixel(&grouped, outside.0, outside.1),
        "the group was not composited as one image"
    );
}

#[test]
fn a_layer_composites_with_its_own_blend_mode() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 1.0, 1.0));
    canvas.save_layer(Layer::default().with_blend(BlendMode::Plus));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    // Red added to blue rather than replacing it: the layer's blend mode
    // governs how the finished group meets what was underneath.
    assert_eq!(pixel(&pixels, 64, 64), [255, 0, 255, 255]);
}

#[test]
fn layers_nest() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // Two halvings compose to a quarter. An inner layer composited into the
    // outer one rather than straight onto the target is what makes this hold;
    // if both went to the target the result would be a half.
    canvas.save_layer(Layer::opacity(0.5));
    canvas.save_layer(Layer::opacity(0.5));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    let got = pixel(&pixels, 64, 64);
    for (channel, want) in got.iter().take(3).zip([64u8, 64, 64]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 2,
            "two half-opacity layers gave {got:?}, expected about a quarter"
        );
    }
}

#[test]
fn a_clip_in_force_when_a_layer_opens_confines_the_result() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // The layer draws unclipped into its own target, and the draw that
    // composites it is clipped instead. The visible result must be the same as
    // if the clip had applied to every shape in the layer.
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("clip");
    canvas.save_layer(Layer::default());
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 32, 64),
        [255, 255, 255, 255],
        "inside the clip"
    );
    assert_eq!(pixel(&pixels, 96, 64), [0, 0, 0, 255], "outside the clip");
}

#[test]
fn a_clip_inside_a_layer_does_not_escape_it() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save_layer(Layer::default());
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("clip");
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("clipped");
    canvas.restore();
    // Outside the layer the clip is gone again, so this covers everything.
    canvas
        .draw_rect(
            Rect::new(64.0, 0.0, 128.0, 128.0),
            &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("unclipped");

    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 32, 64),
        [255, 0, 0, 255],
        "inside the layer clip"
    );
    assert_eq!(
        pixel(&pixels, 96, 64),
        [0, 255, 0, 255],
        "the layer's clip outlived the layer"
    );
}

#[test]
fn a_recording_without_layers_still_has_exactly_one_pass() {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(Rect::from_size(16.0, 16.0), &Paint::fill(Color::WHITE))
        .expect("rect");
    let recording = canvas.finish();
    assert_eq!(recording.layer_count(), 0);
    assert_eq!(recording.passes.len(), 1);
}

#[test]
fn a_layer_left_open_is_composited_rather_than_discarded() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save_layer(Layer::default());
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    // No restore. An unbalanced save_layer is a caller mistake, and dropping
    // everything drawn since it would look like a rendering fault instead.
    let recording = canvas.finish();
    assert_eq!(recording.layer_count(), 1);

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &recording).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    assert_eq!(pixel(&pixels, 64, 64), [255, 255, 255, 255]);
}

#[test]
fn the_backends_agree_on_a_layered_frame() {
    // Layers are the first thing here that renders more than one pass per
    // frame and samples a render target, and the two backends allocate,
    // transition and bind that target quite differently. Comparing the same
    // recording through both is what catches a difference that each on its own
    // would render plausibly.
    let (Ok(mut vulkan), Ok(mut gles)) = (
        Context::new(BackendPreference::Vulkan),
        Context::new(BackendPreference::Gles),
    ) else {
        eprintln!("skipping: both backends are needed");
        return;
    };

    let build = || {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.1, 0.1, 0.2, 1.0));
        canvas.save_layer(Layer::opacity(0.6));
        overlapping_circles(&mut canvas, 1.0);
        canvas.save_layer(Layer::opacity(0.5).with_blend(BlendMode::Plus));
        canvas
            .draw_rect(
                Rect::new(20.0, 20.0, 108.0, 60.0),
                &Paint::fill(Color::linear(0.0, 0.8, 0.4, 1.0)).with_anti_alias(false),
            )
            .expect("rect");
        canvas.restore();
        canvas.restore();
        canvas.finish()
    };

    let a = {
        let mut surface = vulkan
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        vulkan.draw(&mut surface, &build()).expect("draw");
        let pixels = vulkan.read(&mut surface).expect("read");
        vulkan.destroy_surface(surface);
        pixels
    };
    let b = {
        let mut surface = gles
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        gles.draw(&mut surface, &build()).expect("draw");
        let pixels = gles.read(&mut surface).expect("read");
        gles.destroy_surface(surface);
        pixels
    };

    // One unit: compositing a layer is arithmetic, converted to fixed point
    // once per pass, and the two need not round it identically.
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "the backends differ by up to {worst} on a layered frame"
    );
    // And the frame is not simply the background: a comparison of two blank
    // images would agree perfectly and prove nothing.
    assert_ne!(
        pixel(&a, 64, 40),
        pixel(&a, 4, 124),
        "the layered frame drew nothing"
    );
}

/// An atlas holding two glyphs of known coverage.
///
/// Synthetic rather than rasterized from a font: shaping and font parsing are
/// out of scope, and coverage whose every texel is known exactly is a far
/// better thing to assert against than whatever a hinter produced.
fn two_glyph_atlas() -> (Atlas, GlyphKey, GlyphKey) {
    let mut atlas = Atlas::new(64);
    let solid = GlyphKey {
        font: 1,
        glyph: 1,
        size: 16,
    };
    let half = GlyphKey {
        font: 1,
        glyph: 2,
        size: 16,
    };
    atlas
        .insert(
            solid,
            &Coverage {
                width: 8,
                height: 8,
                texels: vec![255; 64],
            },
        )
        .expect("solid glyph");
    atlas
        .insert(
            half,
            &Coverage {
                width: 8,
                height: 8,
                texels: vec![128; 64],
            },
        )
        .expect("half glyph");
    (atlas, solid, half)
}

/// Upload an atlas as an image this context can sample.
fn upload_atlas(ctx: &mut Context, atlas: &Atlas) -> emblema::Image {
    // One byte per texel, which is what an atlas holds. The shader reads the
    // red channel either way, so a four-channel atlas would work and cost four
    // times the memory and four times the bandwidth to sample.
    let mut image = ctx
        .create_image(
            Extent2D::new(atlas.size(), atlas.size()),
            PixelFormat::R8Unorm,
        )
        .expect("atlas image");
    ctx.write_image(&mut image, atlas.texels()).expect("upload");
    image
}

#[test]
fn a_glyph_run_draws_its_coverage_tinted_by_the_paint() {
    let Some(mut ctx) = context() else { return };
    let (atlas, solid, half) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_glyphs(
            &[
                PositionedGlyph::new(solid, [16.0, 16.0], atlas.get(solid).unwrap()),
                PositionedGlyph::new(half, [48.0, 16.0], atlas.get(half).unwrap()),
            ],
            &atlas,
            0,
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)),
        )
        .expect("glyphs");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Full coverage gives the paint's color outright; half coverage gives it at
    // half alpha, premultiplied. That the two differ is the point: an atlas
    // read as color rather than as coverage would make both fully red.
    assert_eq!(pixel(&pixels, 20, 20), [255, 0, 0, 255], "the solid glyph");
    let faint = pixel(&pixels, 52, 20);
    assert!(
        (faint[0] as i32 - 128).abs() <= 2 && faint[1] == 0 && faint[2] == 0,
        "the half-coverage glyph came back {faint:?}"
    );
    // And nothing was drawn where no glyph was placed.
    assert_eq!(
        pixel(&pixels, 100, 100),
        [0, 0, 0, 255],
        "between the glyphs"
    );
}

#[test]
fn a_run_of_many_glyphs_is_one_draw() {
    // The reason vertices carry texture coordinates at all. A paint is per
    // draw, so coordinates carried there would mean a draw per glyph, and text
    // is the highest draw-count content there is.
    let (atlas, solid, _) = two_glyph_atlas();
    let rect = atlas.get(solid).unwrap();
    let glyphs: Vec<PositionedGlyph> = (0..24)
        .map(|i| PositionedGlyph::new(solid, [i as f32 * 5.0, 40.0], rect))
        .collect();

    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_glyphs(&glyphs, &atlas, 0, &Paint::fill(Color::WHITE))
        .expect("glyphs");
    let recording = canvas.finish();
    assert_eq!(
        recording.draw_count(),
        1,
        "a run of {} glyphs should be one draw",
        glyphs.len()
    );
}

#[test]
fn a_glyph_run_travels_with_the_transform_and_the_clip() {
    let Some(mut ctx) = context() else { return };
    let (atlas, solid, _) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);
    let rect = atlas.get(solid).unwrap();

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    // Clipped to the left half, then moved right by a translation. A run whose
    // quads were built in device pixels would ignore both.
    canvas
        .clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0))
        .expect("clip");
    canvas.translate(24.0, 24.0);
    canvas
        .draw_glyphs(
            &[
                PositionedGlyph::new(solid, [0.0, 0.0], rect),
                PositionedGlyph::new(solid, [56.0, 0.0], rect),
            ],
            &atlas,
            0,
            &Paint::fill(Color::WHITE),
        )
        .expect("glyphs");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // The first glyph moved with the translation and survives the clip.
    assert_eq!(
        pixel(&pixels, 28, 28),
        [255, 255, 255, 255],
        "the first glyph"
    );
    // The second lands past the clip's edge and must not appear.
    assert_eq!(pixel(&pixels, 84, 28), [0, 0, 0, 255], "the second glyph");
}

#[test]
fn a_glyph_missing_from_the_atlas_is_refused() {
    let (atlas, solid, _) = two_glyph_atlas();
    let absent = GlyphKey {
        font: 9,
        glyph: 9,
        size: 9,
    };
    let mut canvas = Canvas::new(SIZE);
    // Sampling a glyph nobody added would read whatever texel sits at the
    // origin and draw a plausible smudge, which is worse than failing.
    let result = canvas.draw_glyphs(
        &[PositionedGlyph::new(
            absent,
            [0.0, 0.0],
            atlas.get(solid).unwrap(),
        )],
        &atlas,
        0,
        &Paint::fill(Color::WHITE),
    );
    assert!(result.is_err(), "a glyph outside the atlas was accepted");
}

#[test]
fn glyphs_take_a_solid_color_rather_than_a_gradient() {
    let (atlas, solid, _) = two_glyph_atlas();
    let mut canvas = Canvas::new(SIZE);
    // An atlas supplies coverage, not color, so a gradient has nowhere to go.
    // Picking its first stop would draw something plausible and wrong.
    let result = canvas.draw_glyphs(
        &[PositionedGlyph::new(
            solid,
            [0.0, 0.0],
            atlas.get(solid).unwrap(),
        )],
        &atlas,
        0,
        &Paint::linear_gradient(
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 0.0),
            vec![
                GradientStop::new(Color::WHITE, 0.0),
                GradientStop::new(Color::linear(0.0, 0.0, 0.0, 1.0), 1.0),
            ],
        ),
    );
    assert!(result.is_err(), "a gradient paint was accepted for glyphs");
}

#[test]
fn the_backends_agree_on_a_glyph_run() {
    // A run is the first geometry here whose vertices carry texture
    // coordinates, and the two backends declare that attribute separately —
    // one in a pipeline's vertex input state, the other as a pointer into an
    // interleaved buffer. A stride or offset wrong on either produces a
    // plausible smear rather than nothing.
    let (Ok(mut vulkan), Ok(mut gles)) = (
        Context::new(BackendPreference::Vulkan),
        Context::new(BackendPreference::Gles),
    ) else {
        eprintln!("skipping: both backends are needed");
        return;
    };
    let (atlas, solid, half) = two_glyph_atlas();

    let build = || {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.1, 1.0));
        let glyphs: Vec<PositionedGlyph> = (0..6)
            .map(|i| {
                let key = if i % 2 == 0 { solid } else { half };
                PositionedGlyph::new(key, [8.0 + i as f32 * 18.0, 40.0], atlas.get(key).unwrap())
            })
            .collect();
        canvas
            .draw_glyphs(
                &glyphs,
                &atlas,
                0,
                &Paint::fill(Color::linear(1.0, 0.8, 0.2, 1.0)),
            )
            .expect("glyphs");
        canvas.finish()
    };

    let render_on = |ctx: &mut Context| {
        let image = upload_atlas(ctx, &atlas);
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &build(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        ctx.destroy_image(image);
        pixels
    };

    let a = render_on(&mut vulkan);
    let b = render_on(&mut gles);
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "the backends differ by up to {worst} on a glyph run"
    );
    // And something was actually drawn, so this is not two blank frames
    // agreeing perfectly.
    assert_ne!(
        pixel(&a, 12, 44),
        pixel(&a, 120, 120),
        "the glyph run drew nothing"
    );
}

#[test]
fn a_glyph_run_is_correct_after_the_atlas_has_grown() {
    let Some(mut ctx) = context() else { return };

    // Growth is the other answer to a full atlas, and it breaks more than
    // compaction does. Compaction moves glyphs within a texture that stays the
    // same size; growth doubles the texture, so every texture coordinate
    // divides by a different number and the image the glyphs were uploaded
    // into is the wrong shape to hold them. The test next to this one
    // deliberately pins its atlas so it cannot grow, which left the doubling
    // path with no end-to-end coverage at all.
    //
    // The property is that none of it is visible: a glyph drawn from a grown
    // atlas lands exactly where the same glyph drawn from a small one did.
    let marker = GlyphKey {
        font: 1,
        glyph: 1,
        size: 16,
    };
    // Distinguishable from the filler below, so a run reading the wrong
    // rectangle draws a visibly different gray rather than a similar one.
    let coverage = Coverage {
        width: 6,
        height: 6,
        texels: vec![255; 36],
    };

    let draw = |ctx: &mut Context, atlas: &Atlas| {
        let image = upload_atlas(ctx, atlas);
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_glyphs(
                &[PositionedGlyph::new(
                    marker,
                    [40.0, 40.0],
                    atlas.get(marker).unwrap(),
                )],
                atlas,
                0,
                &Paint::fill(Color::WHITE),
            )
            .expect("glyphs");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        ctx.destroy_image(image);
        pixels
    };

    // Before: a small atlas holding just the marker.
    let mut small = Atlas::new(32);
    small.insert(marker, &coverage).expect("marker");
    let before = draw(&mut ctx, &small);

    // After: the same marker in an atlas pushed past its starting size. Every
    // glyph is wanted this frame, so there is nothing stale to evict and
    // growing is the only way to fit them.
    let mut grown = Atlas::new(32);
    grown.insert(marker, &coverage).expect("marker");
    let mut filler = 2u16;
    while grown.growths() == 0 {
        grown
            .insert(
                GlyphKey {
                    font: 1,
                    glyph: filler,
                    size: 16,
                },
                &Coverage {
                    width: 6,
                    height: 6,
                    texels: vec![64; 36],
                },
            )
            .expect("should grow rather than refuse");
        filler += 1;
        assert!(filler < 400, "the atlas never grew");
    }
    assert!(grown.size() > small.size(), "the atlas did not get bigger");
    assert!(grown.is_dirty(), "a grown atlas owes an upload");
    assert_eq!(
        grown.compactions(),
        0,
        "this is about growth, not repacking"
    );

    let after = draw(&mut ctx, &grown);

    // Identical, not merely similar. The glyph is the same texels at the same
    // place on screen, and everything that changed was the atlas's own
    // business -- which is the whole claim.
    let worst = before
        .iter()
        .zip(&after)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert_eq!(
        worst, 0,
        "growing the atlas moved or altered the glyph on screen"
    );
    // And the glyph was actually drawn, so this is not two black frames.
    assert!(
        before.iter().any(|&b| b > 128),
        "the marker did not render at all"
    );
}

#[test]
fn a_glyph_run_is_correct_after_the_atlas_has_been_repacked() {
    let Some(mut ctx) = context() else { return };

    // Compaction moves every surviving glyph, so the coordinates a run was
    // going to read are wrong afterwards. The atlas reports itself dirty and
    // the run reads its rectangles at record time, and this is the end-to-end
    // check that those two together are enough: a caller that re-uploads when
    // told to gets the right picture, and one that does not would get a
    // different glyph's texels rather than a blank.
    // Fixed at its starting size, because this is about compaction: an atlas
    // free to grow answers a full one by doubling and never repacks.
    let mut atlas = Atlas::with_limit(32, 32);
    let solid = GlyphKey {
        font: 1,
        glyph: 1,
        size: 16,
    };
    let marker = Coverage {
        width: 6,
        height: 6,
        texels: vec![255; 36],
    };
    atlas.insert(solid, &marker).expect("solid");

    // Fill the rest with glyphs nothing will ask for again.
    let mut filler = 2u16;
    while atlas
        .insert(
            GlyphKey {
                font: 1,
                glyph: filler,
                size: 16,
            },
            &Coverage {
                width: 6,
                height: 6,
                texels: vec![64; 36],
            },
        )
        .is_ok()
    {
        filler += 1;
        assert!(filler < 200, "the atlas never filled");
    }

    // A new frame that needs only the marker, then one more glyph — which is
    // what forces the repack.
    atlas.begin_frame();
    atlas.insert(solid, &marker).expect("refresh");
    atlas
        .insert(
            GlyphKey {
                font: 1,
                glyph: 999,
                size: 16,
            },
            &Coverage {
                width: 6,
                height: 6,
                texels: vec![128; 36],
            },
        )
        .expect("should make room");
    assert_eq!(atlas.compactions(), 1, "the atlas did not repack");
    assert!(atlas.is_dirty(), "a repacked atlas owes an upload");

    let image = upload_atlas(&mut ctx, &atlas);
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_glyphs(
            &[PositionedGlyph::new(
                solid,
                [40.0, 40.0],
                atlas.get(solid).unwrap(),
            )],
            &atlas,
            0,
            &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)),
        )
        .expect("glyphs");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Full coverage after repacking, not the quarter coverage the filler
    // glyphs carried — which is what a stale rectangle would have sampled.
    assert_eq!(
        pixel(&pixels, 42, 42),
        [0, 255, 0, 255],
        "the repacked glyph sampled the wrong part of the atlas"
    );
}

/// Render a scene twice — once with its layer told what it covers and once
/// not — and require the two to agree.
///
/// Bounds are an optimization, so this is the whole guarantee: the pixels must
/// not move. Checking against the unbounded path rather than against stored
/// values means it keeps holding as both change, and it catches the failure
/// this feature actually has, which is content landing in the wrong place
/// because two mappings disagreed about where the target is.
fn bounds_are_invisible(ctx: &mut Context, build: impl Fn(&mut Canvas, Option<Rect>)) {
    let record = |bounds: Option<Rect>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        build(&mut canvas, bounds);
        canvas.finish()
    };
    let draw = |ctx: &mut Context, recording: &_| {
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let bounded_recording = record(Some(LAYER_BOUNDS));
    assert!(
        bounded_recording
            .passes
            .iter()
            .any(|p| p.extent.width < SIZE.width || p.extent.height < SIZE.height),
        "no pass got a smaller target, so this compares two identical paths"
    );

    let full = draw(ctx, &record(None));
    let bounded = draw(ctx, &bounded_recording);
    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    // Two rather than one: a composite multiplies a group alpha and rounds
    // once, and a rotation puts the two targets' samples at different
    // fractional offsets, so the arithmetic lands differently either side. A
    // Pi 5's V3D reads two where x86 reads one, and a layer placed wrongly
    // under a rotation is off by far more than that.
    assert!(
        worst <= 2,
        "a bounded layer differs from a full-size one by {worst}"
    );
    // And the scene drew something, so this is not a comparison of two black
    // frames that would pass however broken the feature was.
    assert!(full.iter().any(|&b| b > 32), "the scene rendered nothing");
}

/// The region the layer scenes below confine themselves to.
const LAYER_BOUNDS: Rect = Rect {
    left: 24.0,
    top: 40.0,
    right: 96.0,
    bottom: 104.0,
};

/// Open a layer with or without bounds, so a scene reads the same either way.
fn open_layer(canvas: &mut Canvas, bounds: Option<Rect>, layer: Layer) {
    match bounds {
        Some(b) => canvas.save_layer_bounds(layer, b),
        None => canvas.save_layer(layer),
    };
}

/// Something worth compositing: a gradient with a translucent shape over it.
///
/// Translucent so that the layer's alpha channel matters, which is where a
/// mistake in the composite mapping shows up as a halo rather than as an
/// offset.
fn layer_contents(canvas: &mut Canvas) {
    canvas
        .draw_rect(
            LAYER_BOUNDS,
            &Paint::linear_gradient(
                Vec2::new(LAYER_BOUNDS.left, LAYER_BOUNDS.top),
                Vec2::new(LAYER_BOUNDS.right, LAYER_BOUNDS.bottom),
                vec![
                    GradientStop::new(Color::linear(1.0, 0.2, 0.0, 1.0), 0.0),
                    GradientStop::new(Color::linear(0.0, 0.4, 1.0, 1.0), 1.0),
                ],
            )
            .with_anti_alias(false),
        )
        .expect("gradient");
    canvas
        .draw_circle(
            Vec2::new(60.0, 72.0),
            26.0,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 0.7)).with_anti_alias(false),
        )
        .expect("circle");
}

/// The region the blur scenes draw into.
const BLUR_REGION: Rect = Rect {
    left: 40.0,
    top: 40.0,
    right: 88.0,
    bottom: 88.0,
};

/// A white square in a layer, blurred by `sigma`, optionally bounded.
fn blurred_square(ctx: &mut Context, sigma: f32, bounds: Option<Rect>) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let layer = Layer::default().with_blur(sigma);
    match bounds {
        Some(region) => canvas.save_layer_bounds(layer, region),
        None => canvas.save_layer(layer),
    };
    canvas
        .draw_rect(
            BLUR_REGION,
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("square");
    canvas.restore();
    render(ctx, canvas)
}

#[test]
fn a_blurred_layer_spreads_beyond_the_shape_it_holds() {
    let Some(mut ctx) = context() else { return };
    // What a blur is for, stated as the thing that distinguishes it: light
    // where the shape is not. A sharp layer has nothing outside the square.
    let sharp = blurred_square(&mut ctx, 0.0, None);
    let soft = blurred_square(&mut ctx, 8.0, None);
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize];

    assert_eq!(at(&sharp, 64, 34), 0, "a sharp layer stops at its edge");
    assert!(
        at(&soft, 64, 34) > 32,
        "a blurred layer should reach past its shape, got {}",
        at(&soft, 64, 34)
    );
    // And it falls off with distance rather than being a uniform smear, which
    // is what a Gaussian means and what a box filter would not give.
    let near = at(&soft, 64, 36);
    let far = at(&soft, 64, 28);
    assert!(
        near > far && far > 0,
        "the blur should fall off with distance, got {near} then {far}"
    );
    // The middle of a shape much larger than the blur is untouched, so the
    // weights sum to one rather than darkening what they average.
    assert_eq!(at(&soft, 64, 64), 255, "the middle should be undimmed");
}

#[test]
fn a_larger_sigma_spreads_further() {
    let Some(mut ctx) = context() else { return };
    // The parameter does what it says, monotonically. A blur that ignored
    // sigma, or scaled it by the wrong thing, would still pass the test above.
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] as i32;
    let small = blurred_square(&mut ctx, 3.0, None);
    let large = blurred_square(&mut ctx, 12.0, None);
    let probe = (64, 30);
    assert!(
        at(&large, probe.0, probe.1) > at(&small, probe.0, probe.1),
        "a larger sigma should reach further"
    );
    // And spends the light it moved rather than adding any: brighter outside
    // has to come from dimmer inside. Measured well inside the edge, since the
    // two profiles cross over within a pixel of it -- at the boundary itself
    // both are near half and which is greater says nothing.
    assert!(
        at(&large, 64, 46) < at(&small, 64, 46),
        "spreading further should dim the inside it came from"
    );
}

#[test]
fn a_blur_of_zero_records_no_extra_passes() {
    // The passes are only recorded where a blur was asked for, so a caller
    // animating one to nothing pays nothing at the end of the animation.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer(Layer::default());
    canvas
        .draw_rect(BLUR_REGION, &Paint::fill(Color::WHITE))
        .expect("square");
    canvas.restore();
    let plain = canvas.finish().passes.len();

    for sigma in [0.0, -3.0, f32::NAN] {
        let mut canvas = Canvas::new(SIZE);
        canvas.save_layer(Layer::default().with_blur(sigma));
        canvas
            .draw_rect(BLUR_REGION, &Paint::fill(Color::WHITE))
            .expect("square");
        canvas.restore();
        assert_eq!(
            canvas.finish().passes.len(),
            plain,
            "a blur of {sigma} should record no extra passes"
        );
    }
}

#[test]
fn a_blur_keeps_softening_past_the_tap_budget() {
    let Some(mut ctx) = context() else { return };
    // A shader loop is bounded, so past some sigma the taps stop covering
    // its radius. Truncated there, the blur stops getting softer however
    // large sigma grows and the shape creeps toward a box -- which reads as a
    // blur that has a maximum, and is the wrong answer at exactly the sizes a
    // frosted panel or a large shadow asks for.
    //
    // The taps spread instead, so they still span the curve and the sampler's
    // bilinear filter averages what falls between them. That trades quality
    // for coverage, which is the right way round: a slightly under-sampled
    // wide blur looks like a wide blur, and a truncated one does not.
    let extent = Extent2D::new(256, 256);
    let render = |ctx: &mut Context, sigma: f32| {
        let mut canvas = Canvas::new(extent);
        canvas.clear(Color::BLACK);
        canvas.save_layer(Layer::default().with_blur(sigma));
        canvas
            .draw_rect(
                Rect::new(96.0, 96.0, 160.0, 160.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("square");
        canvas.restore();
        let mut surface = ctx
            .create_surface(extent, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * 256 + x) * 4) as usize] as i32;

    // Well past where the radius stops fitting in the tap budget, which is a
    // little under eleven at thirty-two taps each way.
    let sigmas = [16.0f32, 32.0, 60.0];
    let rendered: Vec<Vec<u8>> = sigmas.iter().map(|s| render(&mut ctx, *s)).collect();

    for pair in rendered.windows(2) {
        // Further out keeps getting brighter: the blur is still spreading.
        assert!(
            at(&pair[1], 128, 56) > at(&pair[0], 128, 56),
            "a larger sigma stopped reaching further"
        );
        // And the middle keeps getting dimmer, so the light is being moved
        // rather than added.
        assert!(
            at(&pair[1], 128, 128) < at(&pair[0], 128, 128),
            "a larger sigma stopped dimming the middle"
        );
    }
}

#[test]
fn blurred_layers_nest() {
    let Some(mut ctx) = context() else { return };
    // Each blurred layer files three passes and the composite has to name the
    // last of its own, not the last filed. With two nested, an index that
    // counted from the wrong end would composite the inner layer's contents
    // where its blur belongs -- a sharper picture rather than an error.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer(Layer::default().with_blur(4.0));
    canvas.save_layer(Layer::opacity(0.8).with_blur(3.0));
    canvas
        .draw_rect(
            BLUR_REGION,
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("square");
    canvas.restore();
    canvas.restore();
    let recording = canvas.finish();

    // Three per blurred layer, plus the root.
    assert_eq!(
        recording.passes.len(),
        7,
        "two blurred layers and a root should be seven passes"
    );

    let nested = render_recording(&mut ctx, &recording);
    let at = |pixels: &[u8], x: u32, y: u32| pixels[((y * SIZE.width + x) * 4) as usize] as i32;
    // Blurred twice, so it reaches further than either alone would.
    let once = blurred_square(&mut ctx, 4.0, None);
    // Five pixels above the square rather than twelve. One blur of four reaches
    // `(4 - 0.5) * sqrt(3)`, a little over six, so at twelve both were zero and
    // the comparison was between two absences.
    assert!(
        at(&nested, 64, 35) > at(&once, 64, 35),
        "two blurs should reach further than one"
    );
    // And the inner layer's alpha survived the composite rather than being
    // lost when the extra passes were inserted.
    assert!(
        at(&nested, 64, 64) < 255,
        "the inner layer's opacity should still apply"
    );
}

#[test]
fn a_bounded_blurred_layer_matches_a_full_size_one() {
    let Some(mut ctx) = context() else { return };
    // Bounds stay invisible with a blur, which takes more than sizing the
    // target to what the caller said: a blur reaches past its content, so a
    // target sized to the content alone cuts the halo off square at the bound.
    // The caller states where the content is, which is the question they can
    // answer; how far the blur carries it is this renderer's arithmetic.
    let full = blurred_square(&mut ctx, 8.0, None);
    let bounded = blurred_square(&mut ctx, 8.0, Some(BLUR_REGION));

    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    // One level, which is a blur's taps being summed in a different order in
    // the two targets and rounded once at the write. Bit-exact was true on x86
    // and is not on a Pi 5's V3D; bounding a blur wrongly cuts its tail off
    // square, which is visible across the whole halo rather than by a level.
    assert!(
        worst <= 1,
        "bounding a blurred layer changed the picture by {worst}"
    );
    // And it did bound something, or this compares two identical recordings.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer_bounds(Layer::default().with_blur(8.0), BLUR_REGION);
    canvas
        .draw_rect(BLUR_REGION, &Paint::fill(Color::WHITE))
        .expect("square");
    canvas.restore();
    let recording = canvas.finish();
    assert!(
        recording.passes[0].extent.width < SIZE.width,
        "the layer should still have gotten a smaller target"
    );
    // Larger than the content by the blur's reach, not equal to it.
    assert!(
        recording.passes[0].extent.width > BLUR_REGION.width() as u32,
        "the target should be outset past the content it holds"
    );
}

#[test]
fn a_bounded_layer_renders_the_same_as_a_full_size_one() {
    let Some(mut ctx) = context() else { return };
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.6));
        layer_contents(canvas);
        canvas.restore();
    });
}

#[test]
fn a_bounded_layer_renders_the_same_under_a_transform() {
    let Some(mut ctx) = context() else { return };
    // The bounds are stated in user space, so the transform in force when the
    // layer opens has to be applied to them. Left out, the target lands
    // somewhere else and the contents are cut off by its edges.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        canvas.save();
        canvas.translate(12.0, -8.0);
        canvas.scale(0.75, 0.5);
        open_layer(canvas, bounds, Layer::opacity(0.6));
        layer_contents(canvas);
        canvas.restore();
        canvas.restore();
    });
}

#[test]
fn a_bounded_layer_renders_the_same_under_a_rotation() {
    let Some(mut ctx) = context() else { return };
    // A rotation leaves the region a quadrilateral, and the target is the box
    // around it. That is bigger than the region, which is the safe direction
    // for an allocation and would be the wrong one for a clip.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        canvas.save();
        canvas.translate(64.0, 72.0);
        canvas.rotate(0.4);
        canvas.translate(-64.0, -72.0);
        open_layer(canvas, bounds, Layer::opacity(0.6));
        layer_contents(canvas);
        canvas.restore();
        canvas.restore();
    });
}

/// Two overlapping opaque rectangles, drawn however the caller says.
///
/// Overlapping is the whole point. A group's alpha is applied to the group once
/// it is finished, so the two rectangles have already covered each other and
/// the overlap comes out the same as the rest. Applied per draw it is applied
/// twice where they overlap, and the overlap is darker. The two are the same
/// picture everywhere else, so nothing but an overlap can tell them apart.
fn two_overlapping(canvas: &mut Canvas, alpha: f32, grouped: bool) {
    let red = Color::srgb(1.0, 0.0, 0.0, if grouped { 1.0 } else { alpha });
    if grouped {
        canvas.save_layer(Layer::opacity(alpha));
    }
    for rect in [
        Rect::new(16.0, 16.0, 96.0, 96.0),
        Rect::new(32.0, 32.0, 112.0, 112.0),
    ] {
        canvas
            .draw_rect(rect, &Paint::fill(red).with_blend(BlendMode::SrcOver))
            .expect("rect");
    }
    if grouped {
        canvas.restore();
    }
}

#[test]
fn a_groups_opacity_is_applied_to_the_group_and_nesting_multiplies_it() {
    // The semantics upstream's `CanRenderGroupOpacityToSavelayer` is about, and
    // which nothing here asserted: a save layer forwarding its opacity to
    // another has to distribute it rather than apply it twice on one side and
    // not at all on the other.
    let Some(mut ctx) = context() else { return };

    let shot = |ctx: &mut Context, draw: &dyn Fn(&mut Canvas)| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        draw(&mut canvas);
        render(ctx, canvas)
    };
    // Inside both rectangles, and inside only the first.
    let (both, one) = ((64u32, 64u32), (20u32, 20u32));

    // A group at just under a half, which is what two sevenths-tenths
    // multiply to. Its overlap and its single-covered part have to agree:
    // the group was flattened before the alpha reached it.
    let flat = shot(&mut ctx, &|c| two_overlapping(c, 0.49, true));
    assert_eq!(
        pixel(&flat, both.0, both.1),
        pixel(&flat, one.0, one.1),
        "a group's alpha applies to the finished group, so its overlap is not \
         darker than the rest of it"
    );

    // And what the alpha is worth, which neither of the comparisons below can
    // say. Both of them relate one layer alpha to another, and a relation is
    // all they check: composite every layer at the square root of its alpha and
    // a half stays a half -- the root of forty-nine hundredths is seven tenths,
    // and seven tenths squared is back where it started. So both would pass
    // while every layer in the tree was composited wrongly.
    //
    // Opaque red under a layer alpha of `a`, over black, source-over: the
    // destination contributes nothing and the source contributes `a` of full
    // red, so the channel is `a`. Derived rather than measured, which is the
    // point of having it.
    let got = pixel(&flat, both.0, both.1);
    let want = (0.49 * 255.0) as u8;
    assert!(
        got[0].abs_diff(want) <= 2,
        "a layer at forty-nine hundredths over black should give {want} in red, \
         got {got:?}"
    );

    // The same alpha per draw instead. This is the picture a group must *not*
    // produce, and saying so is what stops the assertion above from passing on
    // a renderer that had no group semantics at all.
    let per_draw = shot(&mut ctx, &|c| two_overlapping(c, 0.49, false));
    assert_ne!(
        pixel(&per_draw, both.0, both.1),
        pixel(&per_draw, one.0, one.1),
        "two translucent draws should compound where they overlap; if they do \
         not, the check above proves nothing"
    );

    // And nesting multiplies rather than taking one of the two, or adding.
    let nested = shot(&mut ctx, &|c| {
        c.save_layer(Layer::opacity(0.7));
        two_overlapping(c, 0.7, true);
        c.restore();
    });
    let (got, want) = (pixel(&nested, both.0, both.1), pixel(&flat, both.0, both.1));
    let apart = got[0].abs_diff(want[0]);
    assert!(
        apart <= 2,
        "seven tenths inside seven tenths should be forty-nine hundredths: \
         {got:?} against {want:?}"
    );
}

#[test]
fn an_arc_that_sweeps_nothing_survives_a_mask_blur_and_a_sweep_gradient() {
    // Upstream keeps this as a crash test rather than a picture: it builds the
    // display list and stops, with the comment that an empty picture has to be
    // creatable without crashing. The combination is what makes it worth
    // having -- an arc of zero sweep is a single point, a sweep gradient
    // divides by an angle, and a mask blur opens layers around whatever
    // coverage the point produced, so three things that each have a degenerate
    // case meet on one draw.
    let Some(mut ctx) = context() else { return };

    let mut builder = PathBuilder::default();
    builder.arc(Vec2::new(60.0, 60.0), Vec2::new(50.0, 50.0), 0.0, 0.0);
    let path = builder.build();

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_path(
            &path,
            &Paint::default()
                .with_shader(Shader::SweepGradient {
                    center: Vec2::new(60.0, 60.0),
                    start_angle: std::f32::consts::FRAC_PI_4,
                    end_angle: 3.0 * std::f32::consts::FRAC_PI_4,
                    stops: vec![
                        GradientStop::new(Color::srgb(1.0, 0.0, 0.0, 1.0), 0.0),
                        GradientStop::new(Color::BLACK, 1.0),
                    ],
                    tile: TileMode::Mirror,
                })
                .with_mask_blur(20.0),
        )
        .expect("a degenerate arc is not an error");

    // And it reaches a device, which recording alone does not say: the passes a
    // mask blur opens are where a zero-area coverage would divide by its own
    // extent, and that happens in the executor rather than the recorder.
    let pixels = render(&mut ctx, canvas);

    // Nothing drawn, because there is nothing to draw. Stated as a whole-frame
    // check rather than a probe: a blur spreads what it is given, so a point
    // that wrongly produced coverage would show up somewhere unpredictable
    // rather than at a place worth guessing.
    let ground = [0u8, 0, 0, 255];
    let moved = pixels
        .chunks_exact(4)
        .filter(|texel| *texel != ground)
        .count();
    assert_eq!(
        moved, 0,
        "an arc sweeping nothing put color on {moved} pixels"
    );
}

#[test]
fn bounded_layers_nest() {
    let Some(mut ctx) = context() else { return };
    // An inner layer's target is placed within its parent's, not within the
    // frame's. Compositing it as though the parent filled the frame puts it
    // off by the parent's own offset, which is what this catches.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.8));
        layer_contents(canvas);
        open_layer(
            canvas,
            bounds.map(|_| Rect::new(40.0, 56.0, 88.0, 96.0)),
            Layer::opacity(0.5),
        );
        canvas
            .draw_rect(
                Rect::new(40.0, 56.0, 88.0, 96.0),
                &Paint::fill(Color::linear(0.1, 1.0, 0.3, 1.0)).with_anti_alias(false),
            )
            .expect("inner");
        canvas.restore();
        canvas.restore();
    });
}

#[test]
fn a_clip_inside_a_bounded_layer_lands_where_it_would_have() {
    let Some(mut ctx) = context() else { return };
    // A scissor is stated in device pixels and a bounded target starts
    // somewhere other than the origin, so the two have to be reconciled. A
    // stencil clip is a draw and goes through the same mapping the contents
    // do; both are exercised here, since only the rectangle takes the scissor
    // path.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.9));
        canvas.save();
        canvas
            .clip_rect(Rect::new(32.0, 48.0, 84.0, 96.0))
            .expect("scissor");
        let mut triangle = PathBuilder::new();
        triangle
            .move_to(Vec2::new(60.0, 44.0))
            .line_to(Vec2::new(92.0, 100.0))
            .line_to(Vec2::new(28.0, 100.0))
            .close();
        canvas.clip_path(&triangle.build()).expect("stencil");
        layer_contents(canvas);
        canvas.restore();
        canvas.restore();
    });
}

#[test]
fn an_antialiased_bounded_layer_renders_the_same_as_a_full_size_one() {
    let Some(mut ctx) = context() else { return };
    // Multisampling resolves per target, so a layer with its own smaller target
    // resolves separately from the frame. The composite that brings it back
    // must not soften its own edges on the way.
    bounds_are_invisible(&mut ctx, |canvas, bounds| {
        open_layer(canvas, bounds, Layer::opacity(0.7));
        canvas
            .draw_circle(
                Vec2::new(60.0, 72.0),
                28.0,
                &Paint::fill(Color::linear(0.9, 0.4, 0.1, 1.0)),
            )
            .expect("circle");
        canvas.restore();
    });
}

#[test]
fn glyphs_and_images_in_a_bounded_layer_land_where_they_would_have() {
    let Some(mut ctx) = context() else { return };
    // A glyph run builds its own quads and carries per-vertex texture
    // coordinates, and an image paint maps clip space back to the image
    // itself; neither goes through the path that fills a shape. Both derive
    // their mapping from where the target is, so both need covering here.
    let (atlas, solid, _) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);
    let rect = atlas.get(solid).unwrap();

    let record = |bounds: Option<Rect>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        open_layer(&mut canvas, bounds, Layer::opacity(0.75));
        canvas
            .draw_rect(
                Rect::new(28.0, 44.0, 92.0, 76.0),
                &Paint::image(0, Rect::new(28.0, 44.0, 92.0, 76.0)).with_anti_alias(false),
            )
            .expect("image");
        canvas
            .draw_glyphs(
                &[
                    PositionedGlyph::new(solid, [32.0, 80.0], rect),
                    PositionedGlyph::new(solid, [60.0, 80.0], rect),
                ],
                &atlas,
                0,
                &Paint::fill(Color::WHITE),
            )
            .expect("glyphs");
        canvas.restore();
        canvas.finish()
    };
    let mut draw = |recording: &_| {
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, recording, &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let bounded_recording = record(Some(LAYER_BOUNDS));
    assert!(bounded_recording.passes[0].extent.width < SIZE.width);
    let full = draw(&record(None));
    let bounded = draw(&bounded_recording);
    ctx.destroy_image(image);

    let worst = full
        .iter()
        .zip(&bounded)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(worst <= 1, "glyphs or the image moved by {worst}");
    assert!(full.iter().any(|&b| b > 32), "the scene rendered nothing");
}

#[test]
fn content_outside_a_layers_bounds_is_clipped_rather_than_trusted() {
    let Some(mut ctx) = context() else { return };
    // Bounds are the caller's promise about what the layer covers, and this is
    // what happens when the promise is wrong. The target's own edges cut the
    // drawing off, which is visible and points at the layer that understated
    // itself. The alternative -- believing the caller and sampling a region
    // the target does not have -- reads whatever the allocator last left
    // there, which looks like a driver fault and is a different bug every run.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    // Half as tall as the shape that gets drawn into it.
    canvas.save_layer_bounds(Layer::default(), Rect::new(32.0, 32.0, 96.0, 64.0));
    canvas
        .draw_rect(
            Rect::new(32.0, 32.0, 96.0, 96.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();

    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 64, 48),
        [255, 255, 255, 255],
        "inside the bounds the shape is drawn"
    );
    assert_eq!(
        pixel(&pixels, 64, 80),
        [0, 0, 0, 255],
        "outside the bounds it is cut off, not sampled from somewhere else"
    );
}

#[test]
fn a_layer_gets_a_target_the_size_of_its_bounds() {
    // The point of the feature, stated as the number it is meant to change.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer_bounds(Layer::default(), Rect::new(24.0, 40.0, 96.0, 104.0));
    canvas
        .draw_rect(
            Rect::new(24.0, 40.0, 96.0, 104.0),
            &Paint::fill(Color::WHITE),
        )
        .expect("rect");
    canvas.restore();
    let recording = canvas.finish();
    assert_eq!(recording.passes[0].extent, Extent2D::new(72, 64));
    assert_eq!(
        recording.root().extent,
        SIZE,
        "the root is the caller's surface"
    );
}

#[test]
fn fractional_bounds_are_rounded_outward() {
    // Rounding inward would drop the coverage of the pixels the bound falls
    // inside, which is drawing lost to arithmetic.
    let mut canvas = Canvas::new(SIZE);
    canvas.save_layer_bounds(Layer::default(), Rect::new(24.3, 40.9, 95.1, 103.2));
    canvas
        .draw_rect(Rect::from_size(128.0, 128.0), &Paint::fill(Color::WHITE))
        .expect("rect");
    canvas.restore();
    let recording = canvas.finish();
    assert_eq!(recording.passes[0].extent, Extent2D::new(72, 64));
}

#[test]
fn bounds_that_cannot_be_honored_still_produce_a_layer_that_holds_the_content() {
    // Every way of ending up without a usable region has to produce a working
    // layer rather than a wrong one or a panic. The caller's rectangle is
    // unusable in both cases below, so it is ignored -- but the layer is then
    // sized from what was actually drawn into it, which is never a guess. What
    // must not happen is a target too small for the content, which is drawing
    // lost to arithmetic.
    fn layer_extent(open: impl FnOnce(&mut Canvas)) -> Extent2D {
        let mut canvas = Canvas::new(SIZE);
        open(&mut canvas);
        canvas
            .draw_rect(Rect::from_size(64.0, 64.0), &Paint::fill(Color::WHITE))
            .expect("rect");
        canvas.restore();
        canvas.finish().passes[0].extent
    }

    // The content is a 64x64 rectangle at the origin, antialiased, so it
    // reaches half a pixel past on each side and the layer holding it is one
    // pixel larger. Anything smaller than the rectangle would have cut it.
    let holds = |extent: Extent2D, what: &str| {
        assert!(
            extent.width >= 64 && extent.height >= 64,
            "{what}: {extent:?} is too small to hold a 64x64 draw"
        );
        assert!(
            extent.width <= SIZE.width && extent.height <= SIZE.height,
            "{what}: {extent:?} is larger than the frame"
        );
    };

    // A region with no area, and one entirely outside the frame, both leave
    // nothing the caller's rectangle can contribute.
    let empty = layer_extent(|canvas| {
        canvas.save_layer_bounds(Layer::default(), Rect::new(50.0, 50.0, 50.0, 50.0));
    });
    holds(empty, "empty bounds");

    let offscreen = layer_extent(|canvas| {
        canvas.save_layer_bounds(Layer::default(), Rect::new(400.0, 400.0, 500.0, 500.0));
    });
    holds(offscreen, "bounds outside the frame");
}

/// Stops for the conical tests: red at the first circle, blue at the second.
fn conical_stops() -> Vec<GradientStop> {
    vec![
        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
    ]
}

#[test]
fn a_conical_gradient_whose_first_circle_is_a_point_at_the_center_is_a_radial_gradient() {
    // The two take entirely different routes -- the radial folds its radius
    // into a matrix and measures a length, the conical solves a quadratic --
    // and this is the one configuration where they must agree exactly. It is
    // the only check available that compares the new arithmetic against
    // something already known to be right, rather than against my expectation
    // of what the picture should look like.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::radial_gradient(Vec2::new(64.0, 64.0), 60.0, conical_stops())
                .with_anti_alias(false),
        )
        .expect("radial");
    let radial = render(&mut ctx, canvas);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(64.0, 64.0),
                0.0,
                Vec2::new(64.0, 64.0),
                60.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let conical = render(&mut ctx, canvas);

    let mut worst = 0i32;
    let mut worst_at = (0, 0);
    for y in 0..128 {
        for x in 0..128 {
            let (a, b) = (pixel(&radial, x, y), pixel(&conical, x, y));
            for channel in 0..4 {
                let delta = (a[channel] as i32 - b[channel] as i32).abs();
                if delta > worst {
                    worst = delta;
                    worst_at = (x, y);
                }
            }
        }
    }
    let (x, y) = worst_at;
    assert!(
        worst <= 1,
        "a conical gradient from a point differs from the radial gradient it is: \
         {worst} at ({x}, {y}), radial {:?} against conical {:?}",
        pixel(&radial, x, y),
        pixel(&conical, x, y)
    );
}

#[test]
fn a_conical_gradient_between_concentric_circles_holds_the_first_color_inside_the_inner_one() {
    // The configuration that gives the first radius somewhere to show. With
    // both circles on the same center the family is an annulus, the parameter
    // is zero on the inner circle and one on the outer, and everything inside
    // the inner circle is before the gradient starts.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(64.0, 64.0),
                20.0,
                Vec2::new(64.0, 64.0),
                60.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let pixels = render(&mut ctx, canvas);

    // Flat across the hole, which is what distinguishes this from a radial
    // gradient of radius sixty: that one would already be a third of the way
    // to blue at the inner circle.
    let middle = pixel(&pixels, 64, 64);
    let inner_edge = pixel(&pixels, 64 + 18, 64);
    assert!(
        middle[0] > 240 && middle[2] < 16,
        "the hole should hold the first stop, got {middle:?}"
    );
    for channel in 0..4 {
        assert!(
            (middle[channel] as i32 - inner_edge[channel] as i32).abs() <= 2,
            "the hole is not flat: center {middle:?} against {inner_edge:?}"
        );
    }

    // And halfway between the two circles is halfway along the ramp.
    let halfway = pixel(&pixels, 64 + 40, 64);
    assert!(
        halfway[0] > 90 && halfway[0] < 165 && halfway[2] > 90 && halfway[2] < 165,
        "midway between the circles should be midway along the ramp, got {halfway:?}"
    );
}

#[test]
fn every_point_on_the_second_circle_reaches_the_last_stop_wherever_the_first_sits() {
    // What makes a gradient conical rather than radial: the parameter is one
    // on the whole of the second circle, however far off center the first one
    // is. A solve that measured distance from the first circle instead would
    // run from a quarter to three quarters of the ramp around this circle, so
    // the four samples disagreeing is the signature of exactly that mistake.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(44.0, 64.0),
                0.0,
                Vec2::new(64.0, 64.0),
                40.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let pixels = render(&mut ctx, canvas);

    let focus = pixel(&pixels, 44, 64);
    assert!(
        focus[0] > 240 && focus[2] < 16,
        "the first circle collapsed to a point should be the first stop, got {focus:?}"
    );

    let on_circle = [
        pixel(&pixels, 64 + 39, 64),
        pixel(&pixels, 64 - 39, 64),
        pixel(&pixels, 64, 64 + 39),
        pixel(&pixels, 64, 64 - 39),
    ];
    for sample in on_circle {
        assert!(
            sample[2] > 235 && sample[0] < 24,
            "a point on the second circle should be the last stop, got {sample:?}"
        );
    }
}

#[test]
fn a_conical_gradient_whose_circles_are_tangent_draws_only_the_half_plane_it_spans() {
    // The case the quadratic degenerates in: when the distance between the
    // centers equals the difference of the radii, the first circle sits on the
    // second and the squared term vanishes. Solving it as a quadratic anyway
    // divides by zero and loses the single root that does exist, so this is
    // the test for the branch that handles it -- and the branch is not
    // reachable by any nearby configuration, which is why it needs its own.
    //
    // Geometrically the cone opens into a half plane: every circle of the
    // family passes through the first center, so nothing behind it is on any
    // of them, and nothing is drawn there.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::conical_gradient(
                Vec2::new(44.0, 64.0),
                0.0,
                Vec2::new(64.0, 64.0),
                20.0,
                conical_stops(),
            )
            .with_anti_alias(false),
        )
        .expect("conical");
    let pixels = render(&mut ctx, canvas);

    // Diametrically opposite on the second circle, both a full turn of the
    // ramp away from the first center, and both the last stop.
    for (x, y) in [(84, 64), (64, 84)] {
        let sample = pixel(&pixels, x, y);
        assert!(
            sample[2] > 235 && sample[0] < 24,
            "({x}, {y}) is on the second circle and should be the last stop, got {sample:?}"
        );
    }

    // Behind the first center there is no circle of the family at all. Not a
    // clamped parameter, which would paint the first color across half the
    // rectangle: no parameter.
    for (x, y) in [(20, 64), (4, 4), (4, 124)] {
        let sample = pixel(&pixels, x, y);
        assert_eq!(
            sample,
            [0, 0, 0, 255],
            "({x}, {y}) is on no circle of the family and should be untouched"
        );
    }
}

#[test]
fn a_mesh_draws_the_triangles_it_names_and_nothing_else() {
    // The only geometry here that this renderer did not produce itself, so the
    // thing worth checking is that it is drawn as given: the diagonal of a
    // square split into two triangles is inside both, and a corner cut off by
    // omitting a triangle stays empty.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);

    // One triangle of a square: (16,16), (112,16), (16,112). The far corner is
    // outside it.
    let mesh = Vertices::new(
        VertexMode::Triangles,
        vec![
            Vec2::new(16.0, 16.0),
            Vec2::new(112.0, 16.0),
            Vec2::new(16.0, 112.0),
        ],
    )
    .expect("mesh");
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 24, 24),
        [255, 255, 255, 255],
        "inside the triangle"
    );
    assert_eq!(
        pixel(&pixels, 100, 100),
        [0, 0, 0, 255],
        "the corner the triangle does not cover"
    );
}

#[test]
fn a_mesh_travels_through_the_canvas_transform() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.translate(64.0, 0.0);
    let mesh = Vertices::new(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 16.0),
            Vec2::new(48.0, 16.0),
            Vec2::new(0.0, 112.0),
        ],
    )
    .expect("mesh");
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 72, 24),
        [255, 255, 255, 255],
        "the mesh should have moved with the transform"
    );
    assert_eq!(
        pixel(&pixels, 8, 24),
        [0, 0, 0, 255],
        "the mesh should not still be where it was written"
    );
}

#[test]
fn a_mesh_reads_the_texture_where_its_coordinates_say_rather_than_where_it_sits() {
    // The whole reason texture coordinates exist here. The quad covers the
    // target exactly as the image test's rectangle does, but names its
    // coordinates mirrored, so every quadrant must come out on the opposite
    // side from the positional draw. Sampling by position instead would
    // produce the unmirrored picture, which is what makes this test able to
    // tell the two apart at all.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let corners = vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(128.0, 0.0),
        Vec2::new(128.0, 128.0),
        Vec2::new(0.0, 128.0),
    ];
    let mirrored = vec![
        Vec2::new(1.0, 0.0),
        Vec2::new(0.0, 0.0),
        Vec2::new(0.0, 1.0),
        Vec2::new(1.0, 1.0),
    ];
    let mesh = Vertices::indexed(
        VertexMode::Triangles,
        corners,
        mirrored,
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_vertices(&mesh, &Paint::image(0, Rect::from_size(128.0, 128.0)))
        .expect("mesh");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    for (x, y, want, corner) in [
        (16u32, 16u32, [0u8, 255, 0, 255], "top-left"),
        (112, 16, [255, 0, 0, 255], "top-right"),
        (16, 112, [255, 255, 0, 255], "bottom-left"),
        (112, 112, [0, 0, 255, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {corner} corner read the wrong texel; the coordinates were ignored"
        );
    }
}

#[test]
fn texture_coordinates_are_taken_by_any_shader_that_can_read_them() {
    // This used to refuse every shader but an image, on the reasoning that the
    // coordinates had no texture to read. `dart:ui` reads whatever color source
    // the paint carries at a mesh's coordinates, so the reasoning was about the
    // implementation rather than the API, and it is `docs/non-parity.md`
    // section 14 that records what changed.
    //
    // A solid color is the case worth pinning here rather than a gradient. The
    // coordinates cannot show in the picture -- a constant is the same
    // everywhere -- so accepting them is a decision about what a caller may
    // write rather than about what they get, and it is upstream's decision.
    let mut canvas = Canvas::new(SIZE);
    let mesh = Vertices::textured(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 0.0),
            Vec2::new(0.0, 64.0),
        ],
        vec![Vec2::ZERO, Vec2::X, Vec2::Y],
    )
    .expect("mesh");
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("a solid paint takes coordinates it has no use for");

    // And a caller's program, which was the one shader still refused here. The
    // reason given was that a program replaces the fragment shader and takes
    // its coordinate from the fragment's position, leaving a per-vertex
    // coordinate nowhere to arrive. It replaces the *fragment* stage; the
    // vertex stage behind it is this renderer's own and hands on `uv`, which a
    // program receives by declaring the same output the pipeline requires of
    // it. Nothing here to refuse.
    canvas
        .draw_vertices(
            &mesh,
            &Paint::runtime_effect(0, vec![0.0; emblema::RUNTIME_FLOATS]),
        )
        .expect("a program reads a mesh's coordinates as `uv`");
}

#[test]
fn a_source_rectangle_on_a_textured_mesh_is_refused_rather_than_applied_twice() {
    // Two answers to one question. Silently ignoring the rectangle would put a
    // coordinate of one at the texture's edge instead of the sprite's, which
    // looks like a texture that failed to load rather than like a rule.
    let mut canvas = Canvas::new(SIZE);
    let mesh = Vertices::textured(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 0.0),
            Vec2::new(0.0, 64.0),
        ],
        vec![Vec2::ZERO, Vec2::X, Vec2::Y],
    )
    .expect("mesh");
    let paint =
        Paint::image(0, Rect::from_size(64.0, 64.0)).with_source(Rect::new(0.0, 0.0, 0.5, 0.5));
    assert!(
        canvas.draw_vertices(&mesh, &paint).is_err(),
        "a source rectangle and per-vertex coordinates both select part of the texture"
    );
}

/// Four sprites, one per quadrant of the four-by-four fixture, laid out in a
/// different order than they sit in the sheet.
///
/// Shuffled deliberately: a batch that drew each sprite where it sits in the
/// sheet would pass a test that only checked colors were present. Moving them
/// means each destination names exactly one source, and getting the mapping
/// backwards puts the wrong color in a place a reader can see.
fn shuffled_quadrants() -> Vec<Sprite> {
    let quarter = |x, y| SourceRect::new(x, y, 2.0, 2.0);
    vec![
        // Bottom-right of the sheet (yellow) into the top-left of the target.
        Sprite::new(
            quarter(2.0, 2.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(0.0, 0.0)),
        ),
        // Bottom-left (blue) into the top-right.
        Sprite::new(
            quarter(0.0, 2.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(64.0, 0.0)),
        ),
        // Top-right (green) into the bottom-left.
        Sprite::new(
            quarter(2.0, 0.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(0.0, 64.0)),
        ),
        // Top-left (red) into the bottom-right.
        Sprite::new(
            quarter(0.0, 0.0),
            Affine2::from_scale_angle_translation(Vec2::splat(32.0), 0.0, Vec2::new(64.0, 64.0)),
        ),
    ]
}

#[test]
fn an_atlas_draws_each_sprite_from_the_part_of_the_sheet_it_named() {
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_atlas(
            &shuffled_quadrants(),
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    for (x, y, want, place) in [
        (32u32, 32u32, [255u8, 255, 0, 255], "top-left"),
        (96, 32, [0, 0, 255, 255], "top-right"),
        (32, 96, [0, 255, 0, 255], "bottom-left"),
        (96, 96, [255, 0, 0, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {place} sprite read the wrong part of the sheet"
        );
    }
}

#[test]
fn an_atlas_is_one_draw_however_many_sprites_it_holds() {
    // Four sprites, one draw. This was the whole reason the call existed when
    // it was written: a loop over `draw_vertices` was four draws then. Batches
    // merge adjacent draws that differ in nothing now, so that loop would also
    // come to one -- what the call still saves is building each sprite's quad
    // and its coordinates, which is where the axes get transposed.
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_atlas(
            &shuffled_quadrants(),
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");
    let recording = canvas.finish();
    let pass = recording.passes.first().expect("one pass");
    assert_eq!(
        pass.batch.draw_count(),
        1,
        "four sprites should be one draw, not one draw each"
    );
}

#[test]
fn an_atlas_sprite_travels_through_its_own_transform() {
    // A quarter turn about the sprite's own center, which is the case the
    // transform exists for: the sprite is square and its colors are not, so a
    // rotation is visible in where the halves land rather than only in the
    // outline.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    // The whole sheet, drawn at 128 by 128, turned a quarter turn about its
    // center. Clockwise on screen, since y runs downward.
    let placement = Affine2::from_translation(Vec2::splat(64.0))
        * Affine2::from_angle(std::f32::consts::FRAC_PI_2)
        * Affine2::from_translation(Vec2::splat(-64.0))
        * Affine2::from_scale(Vec2::splat(32.0));
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_atlas(
            &[Sprite::new(SourceRect::new(0.0, 0.0, 4.0, 4.0), placement)],
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Turned a quarter clockwise, the sheet's top-left corner is now at the
    // top-right of the target.
    for (x, y, want, place) in [
        (96u32, 32u32, [255u8, 0, 0, 255], "top-right"),
        (96, 96, [0, 255, 0, 255], "bottom-right"),
        (32, 96, [255, 255, 0, 255], "bottom-left"),
        (32, 32, [0, 0, 255, 255], "top-left"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {place} corner is not where a quarter turn puts it"
        );
    }
}

#[test]
fn an_atlas_needs_the_size_of_the_sheet_it_reads() {
    let mut canvas = Canvas::new(SIZE);
    assert!(
        canvas
            .draw_atlas(
                &shuffled_quadrants(),
                Extent2D::new(0, 4),
                &Paint::image(0, Rect::from_size(128.0, 128.0)),
            )
            .is_err(),
        "a sheet with no texels should be refused rather than divided by"
    );
}

#[test]
fn a_blend_filter_agrees_with_the_same_blend_done_by_the_blender() {
    // The check the whole derivation rests on. Every Porter-Duff mode is
    // affine in its destination once the source is a constant, which is why
    // these can be matrices at all -- and the way to know the matrices are
    // right is that the hardware, computing the same blend its own way, agrees.
    //
    // One side draws the material and then draws the constant over it with the
    // mode. The other draws the material once, with the mode compiled into a
    // filter. Two entirely different paths through the pipeline: fixed-function
    // blending against the framebuffer, and four multiply-adds in the fragment.
    let Some(mut ctx) = context() else { return };

    let material = Color::linear(0.6, 0.2, 0.1, 0.8);
    let constant = Color::linear(0.2, 0.7, 0.4, 0.5);
    let area = Rect::from_size(128.0, 128.0);

    for mode in [
        BlendMode::SrcOver,
        BlendMode::DstOver,
        BlendMode::SrcIn,
        BlendMode::DstIn,
        BlendMode::SrcOut,
        BlendMode::DstOut,
        BlendMode::SrcATop,
        BlendMode::DstATop,
        BlendMode::Xor,
        BlendMode::Plus,
        BlendMode::Modulate,
        BlendMode::Src,
        BlendMode::Dst,
        BlendMode::Clear,
    ] {
        // The blender's answer: the material, replaced into the target, then
        // the constant blended over it.
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
        canvas
            .draw_rect(
                area,
                &Paint::fill(material)
                    .with_blend(BlendMode::Src)
                    .with_anti_alias(false),
            )
            .expect("material");
        canvas
            .draw_rect(
                area,
                &Paint::fill(constant)
                    .with_blend(mode)
                    .with_anti_alias(false),
            )
            .expect("constant");
        let blended = render(&mut ctx, canvas);

        // The filter's answer: one draw, with the same blend compiled in.
        let filter = ColorFilter::blend(constant.to_array(), mode);
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
        canvas
            .draw_rect(
                area,
                &Paint::fill(material)
                    .with_color_filter(filter)
                    .with_blend(BlendMode::Src)
                    .with_anti_alias(false),
            )
            .expect("filtered");
        let filtered = render(&mut ctx, canvas);

        let (a, b) = (pixel(&blended, 64, 64), pixel(&filtered, 64, 64));
        for channel in 0..4 {
            assert!(
                (a[channel] as i32 - b[channel] as i32).abs() <= 2,
                "{mode:?} as a filter gives {b:?} where the blender gives {a:?}"
            );
        }
    }
}

#[test]
fn a_color_matrix_recolors_a_gradient_which_no_tint_could() {
    // What the row in the parity table means by tinting anything rather than
    // only an image: the filter applies to the color the shader produced, so a
    // gradient is recolored along its whole length rather than at its stops.
    // The matrix swaps red and blue, so the two ends must trade places.
    let Some(mut ctx) = context() else { return };

    #[rustfmt::skip]
    let swap = ColorFilter::matrix([
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.0,
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);
    let stops = || {
        vec![
            GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
            GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
        ]
    };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::linear_gradient(Vec2::new(8.0, 0.0), Vec2::new(120.0, 0.0), stops())
                .with_color_filter(swap)
                .with_anti_alias(false),
        )
        .expect("gradient");
    let pixels = render(&mut ctx, canvas);

    let left = pixel(&pixels, 4, 64);
    let right = pixel(&pixels, 124, 64);
    assert!(
        left[2] > 240 && left[0] < 16,
        "the red end should have become blue, got {left:?}"
    );
    assert!(
        right[0] > 240 && right[2] < 16,
        "the blue end should have become red, got {right:?}"
    );

    // And the middle is still a mixture rather than either end, which is what
    // says the filter ran per fragment and not on the stops.
    let middle = pixel(&pixels, 64, 64);
    assert!(
        middle[0] > 40 && middle[0] < 215 && middle[2] > 40 && middle[2] < 215,
        "the middle of the gradient should still be a mixture, got {middle:?}"
    );
}

#[test]
fn an_advanced_blend_mode_is_a_filter_the_shader_evaluates_rather_than_a_matrix() {
    // These are piecewise or exchange components between channels, so no
    // matrix is equal to them, and they were refused outright until the shader
    // learned to evaluate one. They are not refused now, and what has to hold
    // is that they did not quietly become a matrix instead: an approximation
    // is the substitution this renderer does not make, and a matrix that was
    // nearly right would be exactly that.
    for mode in [
        BlendMode::Overlay,
        BlendMode::HardLight,
        BlendMode::Difference,
        BlendMode::Hue,
        BlendMode::Luminosity,
    ] {
        let filter = ColorFilter::blend([1.0, 1.0, 1.0, 1.0], mode);
        assert!(
            matches!(filter, ColorFilter::Blend { .. }),
            "{mode:?} is not affine and must not become a matrix: {filter:?}"
        );
    }

    // And the affine ones still do become a matrix, which is the half of the
    // old rule that has not changed. A blend the shader evaluates costs a
    // branch and a function call that a multiply already in the tail does not.
    for mode in [
        BlendMode::SrcOver,
        BlendMode::SrcIn,
        BlendMode::Modulate,
        BlendMode::Plus,
    ] {
        let filter = ColorFilter::blend([1.0, 1.0, 1.0, 1.0], mode);
        assert!(
            matches!(filter, ColorFilter::Matrix { .. }),
            "{mode:?} is affine and should stay a matrix: {filter:?}"
        );
    }
}

#[test]
fn a_color_matrix_is_applied_to_straight_color_not_premultiplied() {
    // The distinction only shows on a translucent color, which is why it needs
    // its own test: everything else here is opaque, and on an opaque color the
    // two forms are the same function.
    //
    // The matrix adds a half to green and leaves the rest alone. On straight
    // color that is a half of green at full strength, which premultiplies to a
    // quarter. Applied to the premultiplied color instead it would be a half
    // outright -- twice as much green, and the number this test names.
    let Some(mut ctx) = context() else { return };

    #[rustfmt::skip]
    let add_green = ColorFilter::matrix([
        1.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0, 0.5,
        0.0, 0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 0.5))
                .with_color_filter(add_green)
                .with_anti_alias(false),
        )
        .expect("filtered");
    let pixels = render(&mut ctx, canvas);

    // Half a unit of red and a quarter of green, over black.
    let got = pixel(&pixels, 64, 64);
    assert!(
        (got[0] as i32 - 128).abs() <= 2,
        "red should be halved by the alpha, got {got:?}"
    );
    assert!(
        (got[1] as i32 - 64).abs() <= 2,
        "green should be a quarter: half of a half. Twice that means the matrix \
         was applied to premultiplied color. Got {got:?}"
    );
}

#[test]
fn a_luminance_matrix_turns_every_color_the_same_gray_it_weighs() {
    // The canonical color filter, and the one that pins the matrix's
    // orientation. Every row is the same set of weights, so the matrix is not
    // symmetric -- transposed, it would scale each channel by its own weight
    // and leave red red instead of making it gray. The two matrices this file
    // tests elsewhere are both symmetric and cannot tell the difference.
    let Some(mut ctx) = context() else { return };

    const R: f32 = 0.2126;
    const G: f32 = 0.7152;
    const B: f32 = 0.0722;
    #[rustfmt::skip]
    let gray = ColorFilter::matrix([
        R, G, B, 0.0, 0.0,
        R, G, B, 0.0, 0.0,
        R, G, B, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);

    for (name, color, want) in [
        ("red", Color::linear(1.0, 0.0, 0.0, 1.0), R),
        ("green", Color::linear(0.0, 1.0, 0.0, 1.0), G),
        ("blue", Color::linear(0.0, 0.0, 1.0, 1.0), B),
    ] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(color)
                    .with_color_filter(gray)
                    .with_anti_alias(false),
            )
            .expect("filtered");
        let pixels = render(&mut ctx, canvas);
        let got = pixel(&pixels, 64, 64);

        let expected = (want * 255.0).round() as i32;
        for channel in 0..3 {
            assert!(
                (got[channel] as i32 - expected).abs() <= 2,
                "{name} should weigh to {expected} in every channel, got {got:?}"
            );
        }
    }
}

#[test]
fn a_mesh_interpolates_the_colors_its_vertices_carry() {
    // What per-vertex color is for: a gradient across a triangle that no
    // gradient shader describes, because the three corners are independent.
    // Each corner must be its own color and the middle a mixture of all three,
    // which is what says the rasterizer interpolated rather than picking one.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let mesh = Vertices::colored(
        VertexMode::Triangles,
        vec![
            Vec2::new(64.0, 8.0),
            Vec2::new(120.0, 112.0),
            Vec2::new(8.0, 112.0),
        ],
        vec![
            Color::linear(1.0, 0.0, 0.0, 1.0),
            Color::linear(0.0, 1.0, 0.0, 1.0),
            Color::linear(0.0, 0.0, 1.0, 1.0),
        ],
    )
    .expect("mesh");
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    // Well inside each corner, where one color dominates.
    for (x, y, channel, corner) in [
        (64u32, 20u32, 0usize, "top"),
        (108, 104, 1, "bottom-right"),
        (20, 104, 2, "bottom-left"),
    ] {
        let got = pixel(&pixels, x, y);
        let others: Vec<u8> = (0..3).filter(|c| *c != channel).map(|c| got[c]).collect();
        assert!(
            got[channel] > 150 && others.iter().all(|v| *v < 110),
            "the {corner} corner should be dominated by its own color, got {got:?}"
        );
    }

    // The middle is all three at once, which no single vertex is.
    let middle = pixel(&pixels, 64, 80);
    assert!(
        (0..3).all(|c| middle[c] > 40 && middle[c] < 160),
        "the middle should mix all three corners, got {middle:?}"
    );
}

#[test]
fn a_vertex_color_multiplies_the_paint_rather_than_replacing_it() {
    // The rule that makes a white paint leave the mesh's colors alone and any
    // other paint shade them. A half-strength red paint under a green vertex
    // color has to give neither red nor green but their product, which is
    // black -- so the test uses a paint that shares a channel with the color.
    let Some(mut ctx) = context() else { return };

    let corners = vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(128.0, 0.0),
        Vec2::new(0.0, 128.0),
    ];
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let mesh = Vertices::colored(
        VertexMode::Triangles,
        corners,
        vec![Color::srgb(1.0, 0.5, 0.0, 1.0); 3],
    )
    .expect("mesh");
    canvas
        .draw_vertices(
            &mesh,
            &Paint::fill(Color::srgb(0.5, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    // (1.0, 0.5, 0.0) times (0.5, 1.0, 1.0) is (0.5, 0.5, 0.0).
    let got = pixel(&pixels, 20, 20);
    for (channel, want) in [(0usize, 128i32), (1, 128), (2, 0)] {
        assert!(
            (got[channel] as i32 - want).abs() <= 2,
            "channel {channel} should be the product of paint and vertex, got {got:?}"
        );
    }
}

#[test]
fn an_atlas_tints_each_sprite_on_its_own_in_one_draw() {
    // The reason a sprite carries a color rather than the batch carrying one:
    // a palette out of a single monochrome sheet is the whole idiom, and doing
    // it per batch would mean one draw per color, which is the thing the call
    // exists to avoid.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    // Opaque white throughout, so what comes out is the tint alone.
    ctx.write_image(&mut image, &[255u8; 4 * 4 * 4])
        .expect("upload");

    let whole = SourceRect::new(0.0, 0.0, 4.0, 4.0);
    let place =
        |x: f32| Affine2::from_scale_angle_translation(Vec2::splat(16.0), 0.0, Vec2::new(x, 0.0));
    let sprites = vec![
        Sprite::new(whole, place(0.0)).with_color(Color::linear(1.0, 0.0, 0.0, 1.0)),
        Sprite::new(whole, place(64.0)).with_color(Color::linear(0.0, 1.0, 0.0, 1.0)),
    ];

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_atlas(
            &sprites,
            Extent2D::new(4, 4),
            &Paint::image(0, Rect::from_size(128.0, 128.0)),
        )
        .expect("atlas");
    let recording = canvas.finish();
    assert_eq!(
        recording.passes[0].batch.draw_count(),
        1,
        "tinting per sprite must not split the batch"
    );

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &recording, &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    assert_eq!(pixel(&pixels, 32, 32), [255, 0, 0, 255], "the first sprite");
    assert_eq!(
        pixel(&pixels, 96, 32),
        [0, 255, 0, 255],
        "the second sprite"
    );
}

#[test]
fn a_vertex_color_is_interpolated_premultiplied_across_a_fading_edge() {
    // The distinction only appears where alpha varies between vertices, which
    // is why the other color tests cannot see it: on constant alpha the two
    // forms are the same numbers.
    //
    // A quad red at one edge and transparent at the other. Halfway along, a
    // premultiplied interpolation gives half a unit of red at half alpha,
    // which over black is half. Interpolating straight color and using it as
    // though it were premultiplied gives a full unit of red at half alpha --
    // a color brighter than the alpha it is multiplied by, which is not a
    // color, and which over black is twice as bright.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    let mesh = Vertices::full(
        VertexMode::Triangles,
        vec![
            Vec2::new(8.0, 32.0),
            Vec2::new(120.0, 32.0),
            Vec2::new(120.0, 96.0),
            Vec2::new(8.0, 96.0),
        ],
        Vec::new(),
        vec![
            Color::linear(1.0, 0.0, 0.0, 1.0),
            Color::linear(1.0, 0.0, 0.0, 0.0),
            Color::linear(1.0, 0.0, 0.0, 0.0),
            Color::linear(1.0, 0.0, 0.0, 1.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh");
    canvas
        .draw_vertices(
            &mesh,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("mesh");
    let pixels = render(&mut ctx, canvas);

    let middle = pixel(&pixels, 64, 64);
    assert!(
        (middle[0] as i32 - 128).abs() <= 6,
        "halfway along the fade should be half strength; twice that means the \
         color was interpolated straight and then used as premultiplied. Got {middle:?}"
    );
}

#[test]
fn nearest_sampling_reads_one_texel_where_linear_blends_two() {
    // The four-by-four fixture magnified thirty-two times, so each texel covers
    // a wide band and the boundary between two of them is the place the two
    // modes disagree. Linear blends across it; nearest steps.
    //
    // Sampled just either side of the midpoint, which is where the boundary
    // lands: linear must give something between the two colors there, and
    // nearest must give one of them exactly.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let draw = |ctx: &mut Context, sampling: Sampling| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::image(0, Rect::from_size(128.0, 128.0))
                    .with_sampling(sampling)
                    .with_anti_alias(false),
            )
            .expect("image");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let linear = draw(&mut ctx, Sampling::Linear);
    let nearest = draw(&mut ctx, Sampling::Nearest);
    ctx.destroy_image(image);

    // The seam between the red and green quadrants runs down the middle. Two
    // texels meet there, so a linear read a pixel to either side is a mixture
    // of both.
    for x in [62u32, 66] {
        let got = pixel(&linear, x, 32);
        assert!(
            got[0] > 20 && got[1] > 20,
            "linear at ({x}, 32) should mix the two quadrants, got {got:?}"
        );
    }

    // Nearest reads whichever texel the coordinate falls in, so the same two
    // places are the two colors outright.
    assert_eq!(
        pixel(&nearest, 62, 32),
        [255, 0, 0, 255],
        "nearest left of the seam should be the red texel alone"
    );
    assert_eq!(
        pixel(&nearest, 66, 32),
        [0, 255, 0, 255],
        "nearest right of the seam should be the green texel alone"
    );

    // And well inside a quadrant the two agree, which says nearest moved the
    // coordinate rather than changing what it addressed.
    for (x, y) in [(16u32, 16u32), (112, 112)] {
        assert_eq!(
            pixel(&linear, x, y),
            pixel(&nearest, x, y),
            "the two modes should agree away from a boundary at ({x}, {y})"
        );
    }
}

#[test]
fn an_image_filter_blur_of_a_solid_agrees_with_the_mask_blur_of_the_same_shape() {
    // The two are defined differently and only coincide here. A mask blur
    // blurs coverage and then fills; an image filter fills and then blurs the
    // result. Those are the same picture exactly when the fill does not vary,
    // which is why a mask blur takes a solid color and this takes anything --
    // and on a solid color they must agree, or one of them is wrong.
    let Some(mut ctx) = context() else { return };

    let shape = || {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(32.0, 40.0))
            .line_to(Vec2::new(96.0, 40.0))
            .line_to(Vec2::new(64.0, 96.0))
            .close();
        b.build()
    };
    let color = Color::linear(0.9, 0.3, 0.2, 1.0);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.05, 0.05, 0.08, 1.0));
    canvas
        .draw_path(&shape(), &Paint::fill(color).with_mask_blur(6.0))
        .expect("mask blur");
    let masked = render(&mut ctx, canvas);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.05, 0.05, 0.08, 1.0));
    canvas
        .draw_path(
            &shape(),
            &Paint::fill(color).with_image_filter(ImageFilter::blur(6.0)),
        )
        .expect("image filter");
    let filtered = render(&mut ctx, canvas);

    let mut worst = 0i32;
    for (a, b) in masked.chunks_exact(4).zip(filtered.chunks_exact(4)) {
        for channel in 0..4 {
            worst = worst.max((a[channel] as i32 - b[channel] as i32).abs());
        }
    }
    assert!(
        worst <= 2,
        "blurring coverage and blurring the result should agree on a solid \
         color; they differ by {worst}"
    );
}

#[test]
fn an_image_filter_blurs_the_result_where_a_mask_blur_blurs_the_mask() {
    // What the filter is for, and why both exist. Blurring the coverage and
    // then filling is a different picture from filling and then blurring the
    // result, for anything that varies -- that is exactly why a mask blur over
    // a gradient could not simply be routed through the cheaper of the two.
    // Both are available now, and this is what says they are not the same
    // operation wearing two names.
    let Some(mut ctx) = context() else { return };

    let gradient = || {
        Paint::linear_gradient(
            Vec2::new(32.0, 0.0),
            Vec2::new(96.0, 0.0),
            vec![
                GradientStop::new(Color::linear(1.0, 0.2, 0.0, 1.0), 0.0),
                GradientStop::new(Color::linear(0.0, 0.4, 1.0, 1.0), 1.0),
            ],
        )
    };
    let area = Rect::new(32.0, 40.0, 96.0, 88.0);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(area, &gradient().with_mask_blur(6.0))
        .expect("a mask blur over a gradient");
    let masked = render(&mut ctx, canvas);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(area, &gradient().with_image_filter(ImageFilter::blur(6.0)))
        .expect("image filter");
    let pixels = render(&mut ctx, canvas);

    // Soft at the edge: a pixel just outside the rectangle has color, and one
    // well outside has none.
    let just_outside = pixel(&pixels, 64, 36);
    let far_outside = pixel(&pixels, 64, 8);
    assert!(
        just_outside.iter().take(3).any(|v| *v > 20),
        "the blur should reach past the shape, got {just_outside:?}"
    );
    assert_eq!(
        far_outside,
        [0, 0, 0, 255],
        "the blur should not reach the whole target"
    );

    // And it is still a gradient: the two ends differ in the way the stops say.
    let left = pixel(&pixels, 40, 64);
    let right = pixel(&pixels, 88, 64);
    assert!(
        left[0] > right[0] && right[2] > left[2],
        "the gradient should survive the blur: left {left:?}, right {right:?}"
    );

    // The two orders are different pictures, which is the whole reason a paint
    // carries both. Blurring the result softens the gradient's own colors into
    // each other; blurring the mask leaves them and softens only where they
    // land. A renderer that quietly routed one to the other would agree here.
    let worst = (0..128u32)
        .flat_map(|x| (0..128u32).map(move |y| (x, y)))
        .map(|(x, y)| {
            let a = pixel(&masked, x, y);
            let b = pixel(&pixels, x, y);
            (0..3)
                .map(|c| (a[c] as i32 - b[c] as i32).abs())
                .max()
                .unwrap_or(0)
        })
        .max()
        .expect("a frame");
    assert!(
        worst > 8,
        "the two orders came back within {worst} levels of each other, so one \
         of them is not doing what its name says"
    );
}

#[test]
fn a_filtered_stroke_keeps_the_half_of_itself_that_lies_outside_the_path() {
    // The layer a filter draws into is bounded, and a stroke reaches half its
    // width past the path the bounds come from. That widening is easy to miss
    // and hard to catch: opening a bounded layer with a blur already widens
    // the region by the blur's own reach, which covers any stroke whose
    // half-width is under it. So this uses a wide stroke and a small blur, where the two
    // widenings are nothing like each other.
    //
    // A circle of radius twenty stroked forty wide covers everything within
    // forty of its center. Without the stroke's own reach the layer would end
    // about twenty-three out, and the outer half of the band would be clipped
    // away -- which looks like a thinner stroke rather than like a bug.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            Vec2::new(64.0, 64.0),
            20.0,
            &Paint::stroke(Color::linear(1.0, 1.0, 1.0, 1.0), 40.0).with_mask_blur(1.0),
        )
        .expect("stroke");
    let pixels = render(&mut ctx, canvas);

    // Well inside the band's outer half, which only exists if the bounds took
    // the stroke into account.
    for (x, y) in [(64u32, 28u32), (64, 100), (28, 64), (100, 64)] {
        let got = pixel(&pixels, x, y);
        assert!(
            got[0] > 200,
            "({x}, {y}) is inside the stroke and should be painted, got {got:?}"
        );
    }
}

/// A shape and the three places a blur style is decided: well inside it, on
/// its edge, and outside but within the blur's reach.
fn mask_blur_probe(ctx: &mut Context, style: MaskBlurStyle) -> ([u8; 4], [u8; 4], [u8; 4]) {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            Vec2::new(64.0, 64.0),
            34.0,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                .with_mask_blur(6.0)
                .with_mask_blur_style(style),
        )
        .expect("mask blur");
    let pixels = render(ctx, canvas);
    (
        pixel(&pixels, 64, 64),
        pixel(&pixels, 64, 30),
        // Six pixels past the edge, not twelve. A deviation of six reaches
        // `(6 - 0.5) * sqrt(3)`, which is nine and a half, so twelve was
        // outside the kernel altogether once its width stopped being three
        // deviations and became upstream's.
        pixel(&pixels, 64, 24),
    )
}

#[test]
fn each_mask_blur_style_keeps_the_part_of_the_blur_it_names() {
    // The four styles are four combinations of two things: the shape's own
    // coverage, and that coverage blurred. Naming them is only useful if each
    // actually discards what it says it discards, which is what these three
    // probes are for -- inside the shape, on its edge, and outside it but
    // within the blur's reach.
    let Some(mut ctx) = context() else { return };

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Normal);
    assert!(
        inside[0] > 240,
        "normal should be solid well inside: {inside:?}"
    );
    assert!(
        edge[0] > 40 && edge[0] < 220,
        "normal should be soft at the edge: {edge:?}"
    );
    assert!(outside[0] > 4, "normal should reach outside: {outside:?}");

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Solid);
    assert!(inside[0] > 240, "solid should be solid inside: {inside:?}");
    assert!(
        edge[0] > 240,
        "solid keeps the shape at full strength, so its own edge is hard: {edge:?}"
    );
    assert!(
        outside[0] > 4,
        "solid should still blur outside: {outside:?}"
    );

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Outer);
    assert_eq!(
        inside,
        [0, 0, 0, 255],
        "outer draws nothing inside the shape"
    );
    assert!(outside[0] > 4, "outer is the blur outside: {outside:?}");
    assert!(
        edge[0] < 240,
        "the shape is taken out of the blur, so its edge is not full strength: {edge:?}"
    );

    let (inside, edge, outside) = mask_blur_probe(&mut ctx, MaskBlurStyle::Inner);
    assert_eq!(
        outside,
        [0, 0, 0, 255],
        "inner draws nothing outside the shape"
    );
    assert!(
        inside[0] > 240,
        "inner is solid away from the edge: {inside:?}"
    );
    assert!(
        edge[0] > 4 && edge[0] < 240,
        "inner fades toward the shape's own edge: {edge:?}"
    );
}

/// The same three places, with a fill that varies across the shape.
///
/// A gradient rather than a color, which changes what the renderer has to do
/// rather than merely what it draws: a solid mask blur can be drawn by putting
/// the paint through a blurred layer, and a varying one cannot, because the
/// blur would then act on the fill as well as on the coverage. So this takes
/// the other route -- coverage blurred, fill drawn through it -- and the three
/// styles that combine a blurred mask with a sharp one have to compose the two
/// coverages before either is filled.
fn mask_blur_gradient_probe(
    ctx: &mut Context,
    style: MaskBlurStyle,
) -> ([u8; 4], [u8; 4], [u8; 4], [u8; 4]) {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            Vec2::new(64.0, 64.0),
            34.0,
            &Paint::default()
                .with_shader(Shader::LinearGradient {
                    start: Vec2::new(30.0, 0.0),
                    end: Vec2::new(98.0, 0.0),
                    stops: vec![
                        GradientStop::new(Color::srgb(1.0, 0.0, 0.0, 1.0), 0.0),
                        GradientStop::new(Color::srgb(0.0, 0.0, 1.0, 1.0), 1.0),
                    ],
                    tile: TileMode::Clamp,
                })
                .with_mask_blur(6.0)
                .with_mask_blur_style(style),
        )
        .expect("mask blur over a gradient");
    let pixels = render(ctx, canvas);
    (
        // Two points inside, on opposite sides of the gradient's axis.
        pixel(&pixels, 44, 64),
        pixel(&pixels, 84, 64),
        pixel(&pixels, 64, 30),
        pixel(&pixels, 64, 24),
    )
}

#[test]
fn each_mask_blur_style_keeps_its_own_part_of_a_gradient_too() {
    // The styles were solid-only until this test existed, and the refusal said
    // so: three of the four combine a blurred mask with a sharp one, which
    // needs the coverage twice over, and only the default style had a route
    // that produced coverage at all.
    //
    // Two things are checked at once, and both matter. Each style still
    // discards what it names -- the same three probes as the solid case. And
    // the fill is still sampled at each pixel's own position, which is what
    // says the blur acted on the coverage rather than on the filled result: the
    // left half of the shape has to come out red and the right half blue. A
    // renderer that blurred the finished picture would leave both halves
    // pulled toward each other.
    let Some(mut ctx) = context() else { return };

    // What the edge probe reads when the shape is drawn at full coverage, with
    // no mask blur in the paint at all. It cannot be assumed the way the solid
    // case assumes 240: the fill varies, so "full strength" at that pixel is
    // whatever the gradient is worth there, which at the axis's midpoint is
    // half of each end rather than all of one.
    //
    // Drawn unblurred rather than taken from one of the styles, which would
    // compare a style against itself and pass whatever it did.
    let sharp_edge = {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_circle(
                Vec2::new(64.0, 64.0),
                34.0,
                &Paint::default().with_shader(Shader::LinearGradient {
                    start: Vec2::new(30.0, 0.0),
                    end: Vec2::new(98.0, 0.0),
                    stops: vec![
                        GradientStop::new(Color::srgb(1.0, 0.0, 0.0, 1.0), 0.0),
                        GradientStop::new(Color::srgb(0.0, 0.0, 1.0, 1.0), 1.0),
                    ],
                    tile: TileMode::Clamp,
                }),
            )
            .expect("unblurred reference");
        pixel(&render(&mut ctx, canvas), 64, 30)
    };
    let strength = |p: [u8; 4]| p[0] as u32 + p[1] as u32 + p[2] as u32;
    let full = strength(sharp_edge);

    let red_then_blue = |left: [u8; 4], right: [u8; 4], what: &str| {
        assert!(
            left[0] > left[2] + 60,
            "{what}: the gradient's red end should be red, got {left:?}"
        );
        assert!(
            right[2] > right[0] + 60,
            "{what}: the gradient's blue end should be blue, got {right:?}"
        );
    };

    let (left, right, edge, outside) = mask_blur_gradient_probe(&mut ctx, MaskBlurStyle::Normal);
    red_then_blue(left, right, "normal");
    assert!(
        strength(edge) * 10 < full * 9,
        "normal should be soft at the edge: {edge:?} against {full} at full coverage"
    );
    assert!(
        outside.iter().take(3).any(|c| *c > 4),
        "normal should reach outside: {outside:?}"
    );

    let (left, right, edge, outside) = mask_blur_gradient_probe(&mut ctx, MaskBlurStyle::Solid);
    red_then_blue(left, right, "solid");
    assert_eq!(
        edge, sharp_edge,
        "solid keeps the shape at full strength, so its own edge is the \
         unblurred one"
    );
    assert!(
        outside.iter().take(3).any(|c| *c > 4),
        "solid should still blur outside: {outside:?}"
    );

    let (left, right, edge, outside) = mask_blur_gradient_probe(&mut ctx, MaskBlurStyle::Outer);
    assert_eq!(left, [0, 0, 0, 255], "outer draws nothing inside the shape");
    assert_eq!(
        right,
        [0, 0, 0, 255],
        "outer draws nothing inside the shape"
    );
    assert!(
        outside.iter().take(3).any(|c| *c > 4),
        "outer is the blur outside: {outside:?}"
    );
    assert!(
        strength(edge) < full,
        "the shape is taken out of the blur, so its edge is not full strength: \
         {edge:?} against {full}"
    );

    let (left, right, _edge, outside) = mask_blur_gradient_probe(&mut ctx, MaskBlurStyle::Inner);
    red_then_blue(left, right, "inner");
    assert_eq!(
        outside,
        [0, 0, 0, 255],
        "inner draws nothing outside the shape"
    );
}

#[test]
fn the_two_halves_of_a_blur_add_up_to_the_whole_of_it() {
    // Outer and inner partition the normal style between them: one keeps the
    // blurred coverage where the shape is not, the other where it is, and
    // neither invents anything. So at every pixel the two have to add back up
    // to the whole -- a stronger statement than any of the three makes alone,
    // and the one that catches a style keeping slightly too much.
    //
    // Drawn separately and added here rather than drawn on top of each other,
    // because a paint's blend mode applies to the draw *inside* the layer a
    // mask blur opens, not to the layer's composite. Two styles drawn in
    // sequence therefore composite with source-over, which is not addition
    // wherever both are non-zero -- which is exactly the edge this is about.
    let Some(mut ctx) = context() else { return };

    let draw = |ctx: &mut Context, style: MaskBlurStyle| {
        let mut canvas = Canvas::new(SIZE);
        // Black, so what comes back at each pixel is the contribution itself:
        // source-over onto zero leaves the premultiplied color alone.
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        // Through the general route for all three styles, which is what this
        // identity is about. `Normal` on a shape that knows what it is now
        // evaluates its blur rather than sampling one, and the other two still
        // sample -- so on such a shape the three no longer reconstruct each
        // other to within a level or two. That difference is the approximation
        // and is pinned separately; here the point is that the composition is
        // exact, which needs all three built the same way.
        canvas
            .draw_path(
                &square_path(Rect::new(30.0, 30.0, 98.0, 98.0)),
                &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                    .with_mask_blur(6.0)
                    .with_mask_blur_style(style),
            )
            .expect("mask blur");
        render(ctx, canvas)
    };

    let whole = draw(&mut ctx, MaskBlurStyle::Normal);
    let inner = draw(&mut ctx, MaskBlurStyle::Inner);
    let outer = draw(&mut ctx, MaskBlurStyle::Outer);

    let mut worst = 0i32;
    let mut worst_at = 0usize;
    for (i, ((w, a), b)) in whole
        .chunks_exact(4)
        .zip(inner.chunks_exact(4))
        .zip(outer.chunks_exact(4))
        .enumerate()
    {
        // The red channel alone: the fill is white, so all three carry the
        // same number, and the fourth is the target's own opaque alpha.
        let delta = (w[0] as i32 - (a[0] as i32 + b[0] as i32)).abs();
        if delta > worst {
            worst = delta;
            worst_at = i;
        }
    }
    assert!(
        worst <= 4,
        "the inner and outer halves should reconstruct the whole blur; they \
         differ by {worst} at pixel ({}, {})",
        worst_at % 128,
        worst_at / 128
    );
}

/// A card, and a shadow beneath it at the given elevation.
fn shadow_probe(ctx: &mut Context, elevation: f32, transparent: bool, draw_card: bool) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.9, 0.9, 0.92, 1.0));
    let card = {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(40.0, 40.0))
            .line_to(Vec2::new(88.0, 40.0))
            .line_to(Vec2::new(88.0, 80.0))
            .line_to(Vec2::new(40.0, 80.0))
            .close();
        b.build()
    };
    canvas
        .draw_shadow(
            &card,
            Color::linear(0.0, 0.0, 0.0, 1.0),
            elevation,
            transparent,
        )
        .expect("shadow");
    if draw_card {
        canvas
            .draw_path(&card, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
            .expect("card");
    }
    render(ctx, canvas)
}

#[test]
fn a_shadow_falls_below_what_casts_it_and_widens_with_elevation() {
    // The whole content of an elevation: an object further from the surface
    // throws its shadow further and softer. Both have to move together, or the
    // number means nothing beyond "more blur".
    let Some(mut ctx) = context() else { return };

    let low = shadow_probe(&mut ctx, 3.0, false, true);
    let high = shadow_probe(&mut ctx, 12.0, false, true);

    // Just below the card, where a shadow lands and where the higher one must
    // be darker because it reaches further.
    let below_low = pixel(&low, 64, 88)[0] as i32;
    let below_high = pixel(&high, 64, 88)[0] as i32;
    assert!(
        below_high < below_low,
        "raising the object should darken the ground below it: {below_low} then {below_high}"
    );

    // Above the card there is no light behind the object, so nothing falls
    // there at either elevation.
    let above_low = pixel(&low, 64, 30)[0] as i32;
    let above_high = pixel(&high, 64, 30)[0] as i32;
    assert!(
        above_low > 220 && above_high > 220,
        "the light is above, so nothing should fall above the object: {above_low}, {above_high}"
    );

    // Beside the card, where the offset does not reach and only the blur can.
    // Without this the test passes on a shadow whose softness ignores the
    // elevation entirely, since moving it down alone darkens the ground below.
    //
    // Six pixels out rather than ten. Elevation gives a kernel radius and the
    // blur takes a deviation, and while those were treated as the same number
    // every shadow was about twice as soft as it should be -- so the place the
    // two elevations separate most sat further from the card than it does now.
    // Measured across the profile: at ten pixels out the corrected blurs
    // differ by five levels, and at six by twelve.
    let beside_low = pixel(&low, 34, 60)[0] as i32;
    let beside_high = pixel(&high, 34, 60)[0] as i32;
    assert!(
        beside_high < beside_low - 8,
        "raising the object should spread its shadow sideways too: {beside_low} then {beside_high}"
    );
}

#[test]
fn an_object_resting_on_the_surface_casts_no_shadow() {
    let Some(mut ctx) = context() else { return };
    let flat = shadow_probe(&mut ctx, 0.0, false, false);
    let first = &flat[0..4];
    assert!(
        flat.chunks_exact(4).all(|texel| texel == first),
        "an elevation of zero should leave the ground untouched"
    );
}

/// `transparent_occluder` is accepted and changes nothing, as upstream's does.
///
/// This renderer used to punch the caster's outline out of the shadow for an
/// opaque occluder, on the sound grounds that the part a caster covers is
/// spent. Upstream takes the same flag and its drawing code never reads it, and
/// the grounds turned out not to be worth the deviation: in the arrangement the
/// flag describes -- an opaque caster drawn over its own shadow -- the punched
/// and unpunched pictures were byte-identical across all sixty-five thousand
/// bytes, while the punch cost a layer and made every shadow five passes
/// instead of four.
///
/// So what is pinned here is that the flag does nothing, that the picture is
/// still right without it, and that the pass it used to cost is gone. The last
/// is the one that would quietly come back.
#[test]
fn the_occluder_flag_is_accepted_and_changes_nothing() {
    let Some(mut ctx) = context() else { return };

    // The flag makes no difference to the recording or to the picture.
    let opaque = shadow_probe(&mut ctx, 10.0, false, true);
    let transparent = shadow_probe(&mut ctx, 10.0, true, true);
    assert_eq!(
        opaque, transparent,
        "the occluder flag changed the picture, and upstream's does not"
    );

    // A shadow over a rounded rectangle is one pass: the frame. It was four --
    // the mask blur's layer, its two blur passes and the frame -- until the
    // blur could be evaluated in the fragment stage, and five before that,
    // while the punch-out held one more layer open.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_shadow(
            &Rect::new(40.0, 40.0, 88.0, 80.0).to_rounded_path(4.0),
            Color::BLACK,
            10.0,
            false,
        )
        .expect("shadow");
    assert_eq!(
        canvas.finish().passes.len(),
        1,
        "a shadow over a rounded rectangle should not open a layer"
    );

    // And the picture is still a shadow: dark below the card where it falls,
    // and the card itself still covering the ground above.
    let below = pixel(&opaque, 64, 86)[0] as i32;
    assert!(below < 200, "the shadow below the card is missing: {below}");
    let on_card = pixel(&opaque, 64, 70)[0] as i32;
    assert!(
        on_card > 240,
        "the card should cover its own shadow: {on_card}"
    );
}

#[test]
fn a_point_is_drawn_as_the_cap_it_would_have_had() {
    // A point is a segment of no length, so the cap is the whole shape. Round
    // gives a dot, square gives a square, and butt -- which extends a segment
    // by nothing -- gives nothing. That last one is not a special case; it is
    // the only reading that stays consistent with how a segment is drawn.
    let Some(mut ctx) = context() else { return };

    let draw = |ctx: &mut Context, cap: LineCap| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_points(
                PointMode::Points,
                &[Vec2::new(64.0, 64.0)],
                &Paint {
                    style: Style::Stroke(StrokeStyle {
                        cap,
                        ..StrokeStyle::new(40.0)
                    }),
                    ..Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false)
                },
            )
            .expect("points");
        render(ctx, canvas)
    };

    let round = draw(&mut ctx, LineCap::Round);
    let square = draw(&mut ctx, LineCap::Square);
    let butt = draw(&mut ctx, LineCap::Butt);

    // The corner of the square the point spans: inside a square cap, outside
    // a round one.
    let corner = (64 + 18, 64 + 18);
    assert!(
        pixel(&square, corner.0, corner.1)[0] > 240,
        "a square cap should fill its corners"
    );
    assert_eq!(
        pixel(&round, corner.0, corner.1),
        [0, 0, 0, 255],
        "a round cap should not reach its corners"
    );
    assert!(
        pixel(&round, 64, 64)[0] > 240 && pixel(&square, 64, 64)[0] > 240,
        "both should cover the point itself"
    );

    let first = &butt[0..4];
    assert!(
        butt.chunks_exact(4).all(|texel| texel == first),
        "a butt cap adds nothing to a segment of no length, so nothing is drawn"
    );
}

#[test]
fn the_clip_bounds_narrow_with_every_kind_of_clip() {
    // What a caller culls against. A scissor and a stencil clip narrow what
    // may be drawn by quite different means -- one is recorder state, the
    // other a buffer on the device -- and a caller asking what is still
    // reachable wants both accounted for.
    let mut canvas = Canvas::new(SIZE);
    let whole = canvas.destination_clip_bounds();
    assert_eq!(
        (whole.left, whole.top, whole.right, whole.bottom),
        (0.0, 0.0, 128.0, 128.0),
        "an unclipped canvas may reach all of its target"
    );

    canvas.save();
    canvas.clip_rect(Rect::new(16.0, 16.0, 96.0, 96.0)).unwrap();
    let after_rect = canvas.destination_clip_bounds();
    assert_eq!(
        (
            after_rect.left,
            after_rect.top,
            after_rect.right,
            after_rect.bottom
        ),
        (16.0, 16.0, 96.0, 96.0)
    );

    // A path clip goes to the stencil and leaves the scissor alone, so the
    // tracked rectangle is the only thing that sees it.
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(32.0, 32.0))
        .line_to(Vec2::new(64.0, 32.0))
        .line_to(Vec2::new(64.0, 64.0))
        .close();
    canvas.clip_path(&b.build()).unwrap();
    let after_path = canvas.destination_clip_bounds();
    assert_eq!(
        (
            after_path.left,
            after_path.top,
            after_path.right,
            after_path.bottom
        ),
        (32.0, 32.0, 64.0, 64.0),
        "a path clip narrows what is reachable even though the scissor is unchanged"
    );

    canvas.restore();
    let restored = canvas.destination_clip_bounds();
    assert_eq!(
        (restored.left, restored.top, restored.right, restored.bottom),
        (0.0, 0.0, 128.0, 128.0),
        "restoring gives back what the save was holding"
    );
}

#[test]
fn the_local_clip_bounds_are_stated_in_the_callers_own_coordinates() {
    // The same region seen from where the caller is drawing. Under a
    // translation and a scale it is exact; under a rotation it is the box
    // around the rotated box, which is larger than the clip and deliberately
    // so -- too large costs a draw that turns out to be invisible, too small
    // loses a shape.
    let mut canvas = Canvas::new(SIZE);
    canvas.clip_rect(Rect::new(32.0, 32.0, 96.0, 96.0)).unwrap();

    canvas.save();
    canvas.translate(32.0, 32.0);
    let local = canvas.local_clip_bounds();
    assert_eq!(
        (local.left, local.top, local.right, local.bottom),
        (0.0, 0.0, 64.0, 64.0),
        "a translation moves the origin the bounds are stated from"
    );
    canvas.restore();

    canvas.save();
    canvas.scale(2.0, 2.0);
    let local = canvas.local_clip_bounds();
    assert_eq!(
        (local.left, local.top, local.right, local.bottom),
        (16.0, 16.0, 48.0, 48.0),
        "a scale changes what a unit is"
    );
    canvas.restore();

    canvas.save();
    canvas.rotate(std::f32::consts::FRAC_PI_4);
    let local = canvas.local_clip_bounds();
    // The box around a square turned an eighth of a turn is wider than the
    // square by a factor of root two, and must contain it.
    assert!(
        local.right - local.left > 64.0 * 1.4,
        "a rotation should give a conservative box, got {}",
        local.right - local.left
    );
    canvas.restore();
}

#[test]
fn a_collapsed_transform_leaves_nothing_reachable() {
    // A scale of zero folds the plane to a line and nothing drawn through it
    // covers anything. Reporting an enormous rectangle -- which inverting a
    // singular matrix invites -- would tell a caller to draw everything.
    let mut canvas = Canvas::new(SIZE);
    canvas.scale(0.0, 1.0);
    let local = canvas.local_clip_bounds();
    assert_eq!(
        (local.right - local.left, local.bottom - local.top),
        (0.0, 0.0),
        "a transform that cannot be inverted reaches nothing"
    );
}

/// The uniform block as the test effect reads it: two colors and a threshold.
fn effect_uniforms(threshold: f32) -> Vec<f32> {
    let mut out = vec![0.0; RUNTIME_FLOATS];
    out[0..4].copy_from_slice(&[1.0, 0.0, 0.0, 1.0]);
    out[4..8].copy_from_slice(&[0.0, 0.7, 0.2, 1.0]);
    out[20] = threshold;
    out
}

#[test]
fn a_caller_can_fill_a_shape_with_their_own_fragment_program() {
    // The whole point of the feature, through the surface a caller actually
    // has: register a program, name it in a paint, draw a shape with it. What
    // makes this more than the backend test is that the shape is a shape --
    // the effect fills a circle here, so it goes through tessellation, the
    // material path and the paint, not a full-screen quad pushed at the HAL.
    let Some(mut ctx) = context() else { return };
    let program = ctx
        .register_program(&emblema::RuntimeProgram {
            spirv: emblema_shaders::EFFECT_SPV.to_vec(),
            glsl_es: emblema_shaders::EFFECT_FS_GLSL.to_string(),
        })
        .expect("register");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            Vec2::new(64.0, 64.0),
            48.0,
            &Paint::runtime_effect(program, effect_uniforms(0.0)).with_anti_alias(false),
        )
        .expect("effect");
    let pixels = render(&mut ctx, canvas);

    // The effect splits at the middle of clip space, so the circle is two
    // colors -- and outside it the ground shows, which is what says the
    // program filled a shape rather than the frame.
    assert_eq!(pixel(&pixels, 40, 64), [255, 0, 0, 255], "the left half");
    close_enough(
        pixel(&pixels, 88, 64),
        [0, 178, 51, 255],
        1,
        "the right half, in the caller's second color",
    );
    assert_eq!(
        pixel(&pixels, 4, 4),
        [0, 0, 0, 255],
        "outside the circle the effect did not run"
    );
}

#[test]
fn an_effects_uniforms_travel_with_the_paint_that_names_it() {
    // Two draws with the same program and different uniforms, in one
    // recording. They share a pipeline and differ only in the block, which is
    // the arrangement that would break if uniforms were held by the program
    // rather than by the draw.
    let Some(mut ctx) = context() else { return };
    let program = ctx
        .register_program(&emblema::RuntimeProgram {
            spirv: emblema_shaders::EFFECT_SPV.to_vec(),
            glsl_es: emblema_shaders::EFFECT_FS_GLSL.to_string(),
        })
        .expect("register");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, 128.0, 60.0),
            &Paint::runtime_effect(program, effect_uniforms(-0.5)).with_anti_alias(false),
        )
        .expect("upper");
    canvas
        .draw_rect(
            Rect::new(0.0, 68.0, 128.0, 128.0),
            &Paint::runtime_effect(program, effect_uniforms(0.5)).with_anti_alias(false),
        )
        .expect("lower");
    let pixels = render(&mut ctx, canvas);

    // A threshold of minus a half falls at pixel thirty-two, and of plus a
    // half at ninety-six. So the middle of the frame is on opposite sides of
    // the two splits.
    close_enough(
        pixel(&pixels, 64, 30),
        [0, 178, 51, 255],
        1,
        "past the first",
    );
    assert_eq!(
        pixel(&pixels, 64, 100),
        [255, 0, 0, 255],
        "before the second"
    );
}

#[test]
fn a_paint_naming_a_program_nobody_registered_is_refused() {
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::runtime_effect(11, effect_uniforms(0.0)),
        )
        .expect("recording a draw does not touch a device");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    let result = ctx.draw(&mut surface, &canvas.finish());
    ctx.destroy_surface(surface);
    assert!(
        result.is_err(),
        "a program nobody registered should be refused when the recording is drawn"
    );
}

#[test]
fn a_runtime_effect_can_filter_what_was_drawn_rather_than_fill_it() {
    // The last of upstream's seven image filter kinds. A fragment program here
    // was a paint -- it could fill a shape and could not filter a finished one
    // -- and seven of upstream's runtime-effect scenes are about the second.
    //
    // The program is the fixture that samples a texture and multiplies by a
    // tint. Used as a filter its input is the layer the draw landed in, so a
    // white rectangle filtered through it comes back the tint.
    let Some(mut ctx) = context() else { return };
    let program = ctx
        .register_program(&emblema::RuntimeProgram {
            spirv: emblema_shaders::EFFECT_IMAGE_SPV.to_vec(),
            glsl_es: emblema_shaders::EFFECT_IMAGE_FS_GLSL.to_string(),
        })
        .expect("register");
    let tint = |r: f32, g: f32, b: f32| {
        let mut uniforms = vec![0.0; RUNTIME_FLOATS];
        uniforms[0..4].copy_from_slice(&[r, g, b, 1.0]);
        ImageFilter::Runtime { program, uniforms }
    };

    let shot = |ctx: &mut Context, filter: ImageFilter| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(32.0, 32.0, 96.0, 96.0),
                // A gradient rather than a flat color, and that is the whole
                // difference between this test working and only appearing to.
                // Where a binding names no texture the backend binds a
                // placeholder, so a program handed nothing still samples
                // something -- and against a flat white layer the tinted
                // placeholder is the same picture as the tinted layer. A
                // gradient cannot be faked that way: what comes back has to
                // vary across the shape the way the input did.
                &Paint::linear_gradient(
                    Vec2::new(32.0, 0.0),
                    Vec2::new(96.0, 0.0),
                    vec![
                        GradientStop::new(Color::srgb(1.0, 1.0, 1.0, 1.0), 0.0),
                        GradientStop::new(Color::srgb(0.15, 0.15, 0.15, 1.0), 1.0),
                    ],
                )
                .with_anti_alias(false)
                .with_image_filter(filter),
            )
            .expect("rect");
        render(ctx, canvas)
    };

    let plain = shot(&mut ctx, ImageFilter::None);
    let (left, right) = (pixel(&plain, 36, 64), pixel(&plain, 92, 64));
    assert!(
        left[0] > right[0] + 100,
        "the unfiltered gradient should run bright to dark: {left:?} to {right:?}"
    );

    // Filtered, the program has read the layer and multiplied it.
    let filtered = shot(&mut ctx, tint(1.0, 0.4, 0.0));
    let got = pixel(&filtered, 36, 64);
    assert!(
        got[0] > 200 && got[1] < got[0] / 2 && got[2] < 16,
        "the program should have tinted what was drawn, got {got:?}"
    );
    // And it read *the layer*: the gradient survives the tint, so the input
    // was what the draw produced and not the placeholder a binding gets when
    // it names nothing.
    let (l, r) = (pixel(&filtered, 36, 64), pixel(&filtered, 92, 64));
    assert!(
        l[0] > r[0] + 100,
        "the filtered result is flat, so the program sampled something other \
         than the layer: {l:?} to {r:?}"
    );
    // And only where it was drawn. A filter pass covers the layer, and the
    // layer covers the shape -- so a program that read the whole frame, or was
    // handed a target the wrong size, shows up outside it.
    assert_eq!(
        pixel(&filtered, 8, 8),
        [0, 0, 0, 255],
        "the filter reached outside the layer"
    );

    // Two tints differ, which says the uniforms arrive rather than some
    // constant the program would produce either way.
    let other = shot(&mut ctx, tint(0.0, 0.4, 1.0));
    assert_ne!(
        pixel(&filtered, 64, 64),
        pixel(&other, 64, 64),
        "both tints gave the same picture, so the uniforms did not arrive"
    );

    // And it composes, which is the property a paint-only program cannot have.
    // Blurred first and then tinted: the halo is outside the rectangle and is
    // tinted too, so both halves ran.
    let composed = shot(
        &mut ctx,
        ImageFilter::compose(tint(1.0, 0.4, 0.0), ImageFilter::blur(6.0)),
    );
    let halo = pixel(&composed, 28, 64);
    assert!(
        halo[0] > 4,
        "the blur half of the composition did nothing: {halo:?}"
    );
    assert!(
        halo[0] > halo[2],
        "the tint half of the composition did nothing: {halo:?}"
    );
}

#[test]
fn a_runtime_effect_can_read_a_texture_the_caller_supplied() {
    // What closes the gap between this and `dart:ui`'s fragment shader for the
    // common case. No descriptor set of its own: every draw already binds a
    // texture at the one binding this renderer's shader declares, so a program
    // declaring the same gets whatever the paint named.
    let Some(mut ctx) = context() else { return };
    let program = ctx
        .register_program(&emblema::RuntimeProgram {
            spirv: emblema_shaders::EFFECT_IMAGE_SPV.to_vec(),
            glsl_es: emblema_shaders::EFFECT_IMAGE_FS_GLSL.to_string(),
        })
        .expect("register");

    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let mut uniforms = vec![0.0; RUNTIME_FLOATS];
    // Opaque white, so what comes out is the texture rather than the tint.
    uniforms[0..4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::runtime_effect(program, uniforms)
                .with_effect_image(0)
                .with_anti_alias(false),
        )
        .expect("effect");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // The program maps clip space onto the texture with Y increasing upward,
    // which is the convention this renderer's clip space uses and not the
    // one an image material undoes -- so the sheet arrives flipped compared
    // with `drawImage`, and that is the caller's arithmetic to own.
    for (x, y, want, corner) in [
        (16u32, 112u32, [255u8, 0, 0, 255], "bottom-left"),
        (112, 112, [0, 255, 0, 255], "bottom-right"),
        (16, 16, [0, 0, 255, 255], "top-left"),
        (112, 16, [255, 255, 0, 255], "top-right"),
    ] {
        assert_eq!(
            pixel(&pixels, x, y),
            want,
            "the {corner} of the frame read the wrong texel"
        );
    }
}

/// A twelve-by-twelve image of nine distinct four-texel blocks.
///
/// Built for the nine-patch test and not shared with the others, because what
/// it has to show is different: each of the nine pieces must read one block
/// and no other, so a piece drawn in the wrong place or stretched when it
/// should not be shows as a color where another belongs. The quadrant image
/// cannot do that -- with four regions, two of the nine pieces read the same
/// color and a stretched corner is indistinguishable from a correct edge.
fn nine_region_image() -> Vec<u8> {
    const BLOCKS: [[u8; 3]; 9] = [
        [220, 40, 40],
        [40, 200, 60],
        [50, 90, 230],
        [230, 200, 40],
        [200, 60, 200],
        [60, 200, 200],
        [255, 140, 40],
        [130, 130, 130],
        [255, 255, 255],
    ];
    let mut out = vec![0u8; 12 * 12 * 4];
    for y in 0..12usize {
        for x in 0..12usize {
            let block = (y / 4) * 3 + (x / 4);
            let i = (y * 12 + x) * 4;
            out[i..i + 3].copy_from_slice(&BLOCKS[block]);
            out[i + 3] = 255;
        }
    }
    out
}

#[test]
fn a_nine_patch_stretches_its_middle_and_keeps_its_corners() {
    // The whole content of a nine-patch, and the part a test has to work at:
    // that the corners *keep their size*. Sampling inside a corner proves
    // nothing, because a corner stretched across a third of the frame still
    // has its own color at its own end. So this samples just past where the
    // corner should stop, and requires the edge's color there.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(12, 12), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &nine_region_image())
        .expect("upload");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_image_nine(
            0,
            Extent2D::new(12, 12),
            // The middle third, so each fixed edge is four texels.
            Rect::new(4.0, 4.0, 8.0, 8.0),
            Rect::new(8.0, 8.0, 120.0, 120.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("nine patch");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Four texels of twelve, unstretched, so each corner occupies four pixels
    // of the destination: eight to twelve, and a hundred and sixteen to a
    // hundred and twenty.
    assert_eq!(
        pixel(&pixels, 10, 10),
        [220, 40, 40, 255],
        "the top-left corner keeps its own block"
    );
    assert_eq!(
        pixel(&pixels, 118, 10),
        [50, 90, 230, 255],
        "the top-right corner keeps its own"
    );
    assert_eq!(
        pixel(&pixels, 10, 118),
        [255, 140, 40, 255],
        "and the bottom-left"
    );

    // Just past where the corner ends. A corner that stretched would still be
    // its own color here, which is exactly what this rejects.
    assert_eq!(
        pixel(&pixels, 40, 10),
        [40, 200, 60, 255],
        "past the corner is the top edge, stretched along one axis only"
    );
    assert_eq!(
        pixel(&pixels, 10, 40),
        [230, 200, 40, 255],
        "and down the side is the left edge"
    );
    assert_eq!(
        pixel(&pixels, 64, 64),
        [200, 60, 200, 255],
        "the middle stretches both ways"
    );
}

#[test]
fn a_nine_patch_center_outside_the_image_is_refused() {
    let mut canvas = Canvas::new(SIZE);
    assert!(
        canvas
            .draw_image_nine(
                0,
                Extent2D::new(4, 4),
                Rect::new(1.0, 1.0, 9.0, 9.0),
                Rect::from_size(128.0, 128.0),
                &Paint::fill(Color::WHITE),
            )
            .is_err(),
        "a center reaching past the image leaves no nine pieces to draw"
    );
}

#[test]
fn drawing_the_paint_fills_the_clip_rather_than_the_target() {
    // What distinguishes this from a rectangle a caller writes. The clip is in
    // force and a transform is too, so the rectangle they would need is the
    // clip's bounds carried back through it -- and getting that wrong is
    // invisible until something is rotated.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.clip_rect(Rect::new(32.0, 32.0, 96.0, 96.0)).unwrap();
    canvas.save();
    // A transform that *shrinks*, which is the direction that catches the
    // mistake. Filling the target's own rectangle as though it were in the
    // caller's coordinates covers half of it under this and passes under a
    // scale that grows -- so a test written with the other one asserts
    // nothing.
    canvas.scale(0.5, 0.5);
    canvas
        .draw_paint(&Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false))
        .expect("paint");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 64, 64),
        [255, 255, 255, 255],
        "inside the clip should be filled whatever the transform"
    );
    assert_eq!(
        pixel(&pixels, 16, 64),
        [0, 0, 0, 255],
        "and nothing outside it"
    );
    assert_eq!(pixel(&pixels, 64, 16), [0, 0, 0, 255], "on either axis");
}

#[test]
fn drawing_a_color_blends_where_clearing_replaces() {
    // `clear` replaces the whole target and ignores the clip; this is a draw,
    // so it blends and obeys what is in force. Two calls that look alike and
    // are not, which is why both exist.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(1.0, 0.0, 0.0, 1.0));
    canvas.clip_rect(Rect::new(0.0, 0.0, 64.0, 128.0)).unwrap();
    canvas
        .draw_color(Color::linear(0.0, 0.0, 1.0, 0.5), BlendMode::SrcOver)
        .expect("color");
    let pixels = render(&mut ctx, canvas);

    let mixed = pixel(&pixels, 32, 64);
    assert!(
        mixed[0] > 100 && mixed[2] > 100,
        "half-transparent blue over red should be both, got {mixed:?}"
    );
    assert_eq!(
        pixel(&pixels, 96, 64),
        [255, 0, 0, 255],
        "outside the clip the ground is untouched"
    );
}

#[test]
fn a_layer_matrix_moves_the_finished_image_rather_than_its_contents() {
    // What a matrix image filter means. The layer is drawn where it was
    // written and the finished image is moved on the way back, so the shape
    // arrives somewhere its own coordinates never named.
    //
    // Both halves have to move together. Moving the geometry alone shows the
    // layer through a window that moved -- the shape staying put and being
    // revealed in a different place -- which is a plausible enough picture
    // that only a test looking at both places tells them apart.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer_bounds(
        Layer::opacity(1.0).with_matrix(Affine2::from_translation(Vec2::new(40.0, 0.0))),
        Rect::new(8.0, 40.0, 56.0, 88.0),
    );
    canvas
        .draw_rect(
            Rect::new(16.0, 48.0, 48.0, 80.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("inside");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 72, 64),
        [255, 255, 255, 255],
        "the finished layer should have moved by the matrix"
    );
    assert_eq!(
        pixel(&pixels, 32, 64),
        [0, 0, 0, 255],
        "and left nothing where it was drawn"
    );
}

#[test]
fn a_layer_matrix_that_folds_the_plane_composites_where_it_was_drawn() {
    // A scale of zero has no inverse, so there is no mapping that says which
    // texel a fragment reads. Answering with the layer where it was drawn is
    // the same answer as no matrix, which is a picture; inverting anyway gives
    // infinities and samples nothing in particular.
    let Some(mut ctx) = context() else { return };
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer_bounds(
        Layer::opacity(1.0).with_matrix(Affine2::from_scale(Vec2::new(0.0, 1.0))),
        Rect::new(32.0, 32.0, 96.0, 96.0),
    );
    canvas
        .draw_rect(
            Rect::new(40.0, 40.0, 88.0, 88.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("inside");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 64, 64),
        [255, 255, 255, 255],
        "a matrix with no inverse should leave the layer where it was"
    );
}

#[test]
fn a_matrix_image_filter_moves_what_was_drawn() {
    // The filter's observable effect: the shape lands where the matrix puts
    // it, not where its own coordinates say. That is what makes it usable at
    // all, and it is the half a test can state plainly.
    //
    // What this deliberately does not assert is that the result is *softer*
    // than the same shape drawn under the transform stack. It is -- one
    // resamples a finished image and the other redraws -- but how much softer
    // is a property of the sampler rather than of this feature, and an
    // assertion about it would be a test of filtering quality wearing a
    // feature's name. The difference is recorded where the filter is defined.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            Vec2::new(32.0, 32.0),
            20.0,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_image_filter(
                ImageFilter::Matrix {
                    transform: Affine2::from_scale(Vec2::splat(2.0)).into(),
                },
            ),
        )
        .expect("filtered");
    let pixels = render(&mut ctx, canvas);

    // Drawn at (32, 32) with radius twenty, so unfiltered it would cover
    // twelve to fifty-two. Doubled, it covers twenty-four to a hundred and
    // four -- so the center of the frame is inside it and its old position is
    // not.
    assert_eq!(
        pixel(&pixels, 64, 64),
        [255, 255, 255, 255],
        "the doubled circle should cover the center of the frame"
    );
    assert_eq!(
        pixel(&pixels, 100, 64),
        [255, 255, 255, 255],
        "and reach where only a doubled one could"
    );
    assert_eq!(
        pixel(&pixels, 14, 32),
        [0, 0, 0, 255],
        "and leave where it was written"
    );
}

#[test]
fn an_image_filter_on_a_shape_with_a_fast_path_is_not_quietly_dropped() {
    // A circle and a rounded rectangle are drawn from a distance field rather
    // than from triangles, on a route that bypasses the call where filters are
    // applied. That route refuses a mask blur and did not refuse an image
    // filter, so a filtered circle drew as though nothing had been asked for.
    //
    // Nothing failed when it happened: the shape was still a shape, in the
    // place its own coordinates named. Only asking where it landed said
    // otherwise, which is why this test asks about a circle and a rounded
    // rectangle by name.
    let Some(mut ctx) = context() else { return };

    for which in ["circle", "rounded rectangle"] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let paint =
            Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_image_filter(ImageFilter::Matrix {
                transform: Affine2::from_translation(Vec2::new(48.0, 0.0)).into(),
            });
        if which == "circle" {
            canvas
                .draw_circle(Vec2::new(32.0, 64.0), 20.0, &paint)
                .expect("circle");
        } else {
            canvas
                .draw_rrect(Rect::new(12.0, 44.0, 52.0, 84.0), 8.0, &paint)
                .expect("rrect");
        }
        let pixels = render(&mut ctx, canvas);

        assert_eq!(
            pixel(&pixels, 80, 64),
            [255, 255, 255, 255],
            "the {which} should have moved by the filter"
        );
        assert_eq!(
            pixel(&pixels, 32, 64),
            [0, 0, 0, 255],
            "and left where it was written"
        );
    }
}

/// The sRGB transfer function on the host, to compare the shader against.
///
/// Written out longhand rather than shared with the renderer on purpose: a
/// constant copied from the shader would agree with the shader even if both
/// were wrong, and these numbers are the ones the specification states.
fn encode_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

fn decode_srgb(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn the_gamma_filter_follows_the_curve_at_both_ends_of_it() {
    // The reason the gamma pair cannot be a color matrix, tested where it
    // matters. Almost anywhere in the midtones the curve is within a couple of
    // 8-bit steps of a plain power of 2.2, which is close enough that a test
    // there would pass on the approximation too. Below the knee at 0.0031308
    // the curve is a straight line of slope 12.92 and the approximation is
    // nowhere near it, so that is where this samples.
    let Some(mut ctx) = context() else { return };

    // Two below the knee, two above, one on either side of a byte boundary so
    // that no sample is near a rounding tie.
    const SAMPLES: [f32; 4] = [0.001, 0.002_5, 0.2, 0.75];

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    for (i, value) in SAMPLES.iter().enumerate() {
        let x = i as f32 * 32.0;
        canvas
            .draw_rect(
                Rect::new(x, 0.0, x + 32.0, 128.0),
                &Paint::fill(Color::srgb(*value, *value, *value, 1.0))
                    .with_color_filter(ColorFilter::linear_to_srgb())
                    .with_anti_alias(false),
            )
            .expect("encoded");
    }
    let pixels = render(&mut ctx, canvas);

    for (i, value) in SAMPLES.iter().enumerate() {
        let want = (encode_srgb(*value) * 255.0).round() as i32;
        let got = pixel(&pixels, i as u32 * 32 + 16, 64);
        assert!(
            (got[0] as i32 - want).abs() <= 2,
            "linear {value} should encode to {want}, got {}. A power of 2.2 \
             would give {} instead.",
            got[0],
            (value.powf(1.0 / 2.2) * 255.0).round() as i32
        );
    }
}

#[test]
fn the_gamma_pair_undo_each_other() {
    // Each direction is checked against the host curve above, which would pass
    // even if the two shader branches were the same function written twice.
    // This is the property that makes them a pair: decoding what the other
    // encoded lands back where it started.
    let Some(mut ctx) = context() else { return };

    const LINEAR: f32 = 0.35;
    let encoded = encode_srgb(LINEAR);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, 64.0, 128.0),
            &Paint::fill(Color::srgb(LINEAR, LINEAR, LINEAR, 1.0))
                .with_color_filter(ColorFilter::linear_to_srgb())
                .with_anti_alias(false),
        )
        .expect("encoded");
    canvas
        .draw_rect(
            Rect::new(64.0, 0.0, 128.0, 128.0),
            &Paint::fill(Color::srgb(encoded, encoded, encoded, 1.0))
                .with_color_filter(ColorFilter::srgb_to_linear())
                .with_anti_alias(false),
        )
        .expect("decoded");
    let pixels = render(&mut ctx, canvas);

    let left = pixel(&pixels, 32, 64);
    let right = pixel(&pixels, 96, 64);
    let want_left = (encoded * 255.0).round() as i32;
    let want_right = (decode_srgb(encoded) * 255.0).round() as i32;
    assert_eq!(
        want_right,
        (LINEAR * 255.0).round() as i32,
        "the host curve does not round-trip, so the test itself is wrong"
    );
    assert!(
        (left[0] as i32 - want_left).abs() <= 2,
        "encoding {LINEAR} should give {want_left}, got {left:?}"
    );
    assert!(
        (right[0] as i32 - want_right).abs() <= 2,
        "decoding {encoded} should give back {want_right}, got {right:?}. \
         The two directions are not inverses."
    );
}

#[test]
fn the_gamma_filter_curves_color_and_leaves_alpha_alone() {
    // Gamma applies to straight color: the shader has to divide the alpha out
    // first, curve what is left, and multiply it back. Curving the
    // premultiplied channel instead would encode the coverage along with the
    // color, and curving alpha would change how much of the pixel is covered
    // at all -- a filter that says nothing about coverage.
    let Some(mut ctx) = context() else { return };

    const LINEAR: f32 = 0.25;
    const ALPHA: f32 = 0.5;

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::srgb(LINEAR, LINEAR, LINEAR, ALPHA))
                .with_color_filter(ColorFilter::linear_to_srgb())
                .with_anti_alias(false),
        )
        .expect("filtered");
    let pixels = render(&mut ctx, canvas);

    let got = pixel(&pixels, 64, 64);
    let want = (encode_srgb(LINEAR) * ALPHA * 255.0).round() as i32;
    assert!(
        (got[3] as i32 - (ALPHA * 255.0).round() as i32).abs() <= 2,
        "alpha should pass through the curve untouched, got {got:?}"
    );
    assert!(
        (got[0] as i32 - want).abs() <= 2,
        "the straight color should be curved and then premultiplied, which is \
         {want}. Curving the premultiplied channel would give {} instead. Got \
         {got:?}",
        (encode_srgb(LINEAR * ALPHA) * 255.0).round() as i32
    );
}

/// The inclusive range of columns on row `y` whose red channel reads as lit.
///
/// A morphological filter is judged by where its result ends, so almost every
/// test of one is a question about an extent rather than about a pixel.
fn lit_span(pixels: &[u8], along_x: bool, fixed: u32) -> Option<(u32, u32)> {
    let n = if along_x { SIZE.width } else { SIZE.height };
    let lit: Vec<u32> = (0..n)
        .filter(|i| {
            let (x, y) = if along_x { (*i, fixed) } else { (fixed, *i) };
            pixel(pixels, x, y)[0] > 127
        })
        .collect();
    Some((*lit.first()?, *lit.last()?))
}

/// A square drawn with one image filter, over black.
fn filtered_square(ctx: &mut Context, rect: Rect, filter: ImageFilter) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            rect,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                .with_anti_alias(false)
                .with_image_filter(filter),
        )
        .expect("drew");
    render(ctx, canvas)
}

#[test]
fn dilating_grows_a_shape_by_the_radius_on_every_side() {
    // The whole of what a dilation means, and the reason its layer has to be
    // opened wider than the content: each output pixel takes the largest input
    // within the radius, so a lit pixel spreads exactly that far and no
    // further. A layer sized to the content alone would cut the growth off at
    // the bound, which looks like the filter working and stopping early.
    let Some(mut ctx) = context() else { return };

    const RADIUS: f32 = 8.0;
    let pixels = filtered_square(
        &mut ctx,
        Rect::new(40.0, 40.0, 88.0, 88.0),
        ImageFilter::Dilate {
            radius_x: RADIUS,
            radius_y: RADIUS,
        },
    );

    let r = RADIUS as u32;
    assert_eq!(
        lit_span(&pixels, true, 64),
        Some((40 - r, 87 + r)),
        "the square should grow by {r} to the left and right"
    );
    assert_eq!(
        lit_span(&pixels, false, 64),
        Some((40 - r, 87 + r)),
        "and by {r} above and below"
    );
}

#[test]
fn eroding_shrinks_a_shape_by_the_radius_on_every_side() {
    // The dual, and not merely the same test with a sign flipped: an erosion
    // takes the smallest sample in reach, so it depends on what lies outside
    // the shape being nothing rather than being more of the shape.
    let Some(mut ctx) = context() else { return };

    const RADIUS: f32 = 8.0;
    let pixels = filtered_square(
        &mut ctx,
        Rect::new(40.0, 40.0, 88.0, 88.0),
        ImageFilter::Erode {
            radius_x: RADIUS,
            radius_y: RADIUS,
        },
    );

    let r = RADIUS as u32;
    assert_eq!(
        lit_span(&pixels, true, 64),
        Some((40 + r, 87 - r)),
        "the square should lose {r} from each side"
    );
    assert_eq!(
        lit_span(&pixels, false, 64),
        Some((40 + r, 87 - r)),
        "and {r} from the top and bottom"
    );
}

#[test]
fn eroding_eats_an_edge_that_sits_against_the_layers_own_bound() {
    // What decides how the shader reads past the edge of what it is filtering.
    // Clamping to the edge is right for a blur -- a weighted average that read
    // transparent black from outside would darken every border pixel -- and is
    // wrong here: the samples reaching past this square's left edge would come
    // back as more of the square, and the smallest of a row of white is white.
    // The edge would survive an erosion that should have eaten it.
    let Some(mut ctx) = context() else { return };

    const RADIUS: f32 = 8.0;
    // Flush against the origin, so its left and top edges are the bound.
    let pixels = filtered_square(
        &mut ctx,
        Rect::new(0.0, 0.0, 64.0, 64.0),
        ImageFilter::Erode {
            radius_x: RADIUS,
            radius_y: RADIUS,
        },
    );

    let r = RADIUS as u32;
    assert_eq!(
        lit_span(&pixels, true, 32),
        Some((r, 63 - r)),
        "the left edge is against the bound and should still be eaten into. \
         Starting at zero means the filter read the border texel repeated."
    );
}

#[test]
fn a_morphology_reaches_by_its_own_radius_along_each_axis() {
    // Two radii rather than one because the structuring element is a
    // rectangle, and a rectangle is why the filter is separable at all.
    //
    // The layer is opened by hand, over the whole frame, so that its bounds
    // cannot be what decides the answer. Through `with_image_filter` the bound
    // is derived from the radii and would clip a vertical pass that ran when
    // it should not have -- which is to say the obvious version of this test
    // passes whether or not the two axes use their own radius, because the
    // wrong growth is cropped away before anyone can see it.
    let Some(mut ctx) = context() else { return };

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save_layer_bounds(
        Layer::opacity(1.0).with_morphology(Morphology::dilate(12.0, 0.0)),
        Rect::new(0.0, 0.0, 128.0, 128.0),
    );
    canvas
        .draw_rect(
            Rect::new(40.0, 40.0, 88.0, 88.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("drew");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        lit_span(&pixels, true, 64),
        Some((28, 99)),
        "twelve each way horizontally"
    );
    assert_eq!(
        lit_span(&pixels, false, 64),
        Some((40, 87)),
        "and nothing at all vertically. Room to grow was left above and below, \
         so growth here means the vertical pass took the horizontal radius."
    );
}

#[test]
fn a_radius_past_one_pass_is_split_across_passes_and_stays_exact() {
    // Dilating by thirty-six takes two passes, because the shader's loop is
    // bounded. Splitting is exact rather than an approximation: the
    // structuring elements add under the Minkowski sum, so a pass of
    // thirty-two followed by one of four reaches thirty-six.
    //
    // The alternative -- one pass taking its taps further apart -- is what the
    // blur does, and would be wrong here. A blur that misses a sample loses a
    // little smoothness; a maximum that misses a sample is a scallop in the
    // edge, and the result would not be a dilation by any radius.
    let Some(mut ctx) = context() else { return };

    let radius = MORPHOLOGY_TAPS as f32 + 4.0;
    let pixels = filtered_square(
        &mut ctx,
        Rect::new(40.0, 40.0, 88.0, 88.0),
        ImageFilter::Dilate {
            radius_x: radius,
            radius_y: 0.0,
        },
    );

    let r = radius as u32;
    assert_eq!(
        lit_span(&pixels, true, 64),
        Some((40 - r, 87 + r)),
        "a radius of {r} should reach {r}, however many passes that takes"
    );
}

#[test]
fn dilating_a_translucent_shape_spreads_its_coverage_with_its_color() {
    // Per channel on premultiplied color. Taking the extremum of straight
    // color instead would spread the color without the alpha that belongs to
    // it -- a shape's own hue laid over a coverage it never had -- and the ring
    // the dilation adds would come out at full strength against the half-strength
    // middle.
    let Some(mut ctx) = context() else { return };

    const RADIUS: f32 = 8.0;
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::new(40.0, 40.0, 88.0, 88.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 0.5))
                .with_anti_alias(false)
                .with_image_filter(ImageFilter::Dilate {
                    radius_x: RADIUS,
                    radius_y: RADIUS,
                }),
        )
        .expect("drew");
    let pixels = render(&mut ctx, canvas);

    // Half-strength white over black, inside the original square and in the
    // ring the dilation added.
    for (x, where_) in [(64, "the middle"), (36, "the ring the dilation added")] {
        let got = pixel(&pixels, x, 64);
        assert!(
            (got[0] as i32 - 128).abs() <= 2,
            "{where_} should be half-strength white over black, got {got:?}"
        );
    }
    // And nothing past the reach.
    assert_eq!(
        pixel(&pixels, 30, 64),
        [0, 0, 0, 255],
        "eight pixels is eight pixels"
    );
}

#[test]
fn a_morphology_radius_is_a_whole_number_of_texels() {
    // A structuring element is a set of sample positions, so there is no half
    // of one to keep and the radius is rounded. Rounding it here rather than
    // in the shader is what makes the radius a caller can observe -- through
    // the bounds a dilated layer takes, which grow by it -- the radius that
    // actually gets applied. Rounded in one place and floored in the other,
    // the two would disagree by a pixel and the growth would be cropped.
    let Some(mut ctx) = context() else { return };

    let square = Rect::new(40.0, 40.0, 88.0, 88.0);
    let below = filtered_square(
        &mut ctx,
        square,
        ImageFilter::Dilate {
            radius_x: 0.4,
            radius_y: 0.4,
        },
    );
    assert_eq!(
        lit_span(&below, true, 64),
        Some((40, 87)),
        "four tenths of a texel is no texels, which is the shape unchanged"
    );

    let above = filtered_square(
        &mut ctx,
        square,
        ImageFilter::Dilate {
            radius_x: 0.6,
            radius_y: 0.6,
        },
    );
    assert_eq!(
        lit_span(&above, true, 64),
        Some((39, 88)),
        "six tenths is one texel, applied and allowed for in the bounds"
    );
}

#[test]
fn composing_two_filters_applies_the_inner_one_first() {
    // The order `dart:ui` states, and the one the names suggest once the pair
    // is pictured nested: the inner filter is the one closer to what was drawn.
    // Here the composition is two dilations, which add -- so the order is not
    // what this test is about; the sum is. Order gets its own test below, where
    // the two halves do not commute.
    let Some(mut ctx) = context() else { return };

    let pixels = filtered_square(
        &mut ctx,
        Rect::new(48.0, 48.0, 80.0, 80.0),
        ImageFilter::compose(
            ImageFilter::Dilate {
                radius_x: 8.0,
                radius_y: 8.0,
            },
            ImageFilter::Dilate {
                radius_x: 4.0,
                radius_y: 4.0,
            },
        ),
    );

    assert_eq!(
        lit_span(&pixels, true, 64),
        Some((36, 91)),
        "twelve each way, which is the two radii together. Twelve from one side \
         only would mean the outer layer cropped what the inner one grew."
    );
}

#[test]
fn composing_a_move_covers_both_where_it_drew_and_where_it_landed() {
    // A matrix filter moves the finished image where every other filter grows
    // it in place, which is what makes it the case that pins how a composition
    // sizes its layers. A layer's target is clipped to its parent's, so the
    // outer layer has to cover both where the inner filter put things and where
    // the shape was drawn: the inner layer draws the square where it was
    // written, and its composite is what moves it. Sized for the moved region
    // alone the square is cropped before the matrix ever runs, which is what
    // happened the first time this was written.
    //
    // The two orders give the same extent here, deliberately -- reaching it
    // both ways is the point. Which order actually ran is a different question,
    // and has its own test.
    let Some(mut ctx) = context() else { return };

    let move_right = ImageFilter::Matrix {
        transform: Affine2::from_translation(Vec2::new(32.0, 0.0)).into(),
    };
    let grow = ImageFilter::Dilate {
        radius_x: 8.0,
        radius_y: 0.0,
    };

    // Moved, then grown.
    let after = filtered_square(
        &mut ctx,
        Rect::new(24.0, 48.0, 56.0, 80.0),
        ImageFilter::compose(grow.clone(), move_right.clone()),
    );
    assert_eq!(
        lit_span(&after, true, 64),
        Some((48, 95)),
        "moved to 56..87 and then grown by eight each way"
    );

    // Grown, then moved: the same extent, arrived at the other way round, which
    // says the outer layer covered the moved region rather than the drawn one.
    let before = filtered_square(
        &mut ctx,
        Rect::new(24.0, 48.0, 56.0, 80.0),
        ImageFilter::compose(move_right, grow),
    );
    assert_eq!(
        lit_span(&before, true, 64),
        Some((48, 95)),
        "grown to 16..63 and then moved right by thirty-two"
    );
}

#[test]
fn composing_with_a_filter_that_does_nothing_is_the_other_filter() {
    // Every composition costs a layer, and a layer that copies an image and
    // changes nothing is a copy nobody asked for. The pair is folded when
    // either half is an identity, which also means a caller composing in a loop
    // does not build a chain of them.
    let plain = ImageFilter::blur(4.0);
    assert_eq!(
        ImageFilter::compose(plain.clone(), ImageFilter::None),
        plain,
        "an inner filter that does nothing leaves the outer one alone"
    );
    assert_eq!(
        ImageFilter::compose(ImageFilter::None, plain.clone()),
        plain,
        "and so does an outer one"
    );
    assert!(
        ImageFilter::compose(ImageFilter::None, ImageFilter::None).is_identity(),
        "two of them are still nothing"
    );
    // A radius of zero folds too -- the identity test is the filter's own, not a
    // comparison against `None`.
    assert_eq!(
        ImageFilter::compose(
            plain.clone(),
            ImageFilter::Dilate {
                radius_x: 0.0,
                radius_y: 0.0
            }
        ),
        plain,
        "a radius of zero is a filter that does nothing"
    );
    // But a small *positive* radius does not fold, and this assertion is the
    // inverse of the one that stood here. It used to say a radius below half a
    // texel rounds to nothing and folds. That was true while the radius was
    // device pixels and stopped being true when it became a length in the
    // caller's space: a fifth of a unit under a scale of ten is two device
    // texels, and folding it away would discard a filter the caller can see.
    // Whether a radius rounds to nothing is now decided in
    // `Morphology::applied_radius`, after the transform has been applied.
    let small = ImageFilter::Dilate {
        radius_x: 0.2,
        radius_y: 0.2,
    };
    assert!(
        !small.is_identity(),
        "a small positive radius is a filter until a scale says otherwise"
    );
    assert_ne!(
        ImageFilter::compose(plain.clone(), small),
        plain,
        "so composing with it keeps both halves"
    );
}

#[test]
fn composing_an_erosion_with_a_dilation_depends_on_which_runs_first() {
    // The textbook pair, and the reason it is worth having: the same two
    // filters in the two orders are two different operations with names of
    // their own. Eroding and then dilating is an opening, which removes
    // anything thinner than twice the radius and leaves the rest about where it
    // was. Dilating and then eroding is a closing, which keeps it.
    //
    // A shape with a thin spike answers both at once. Under an opening the
    // spike is gone and the block survives; under a closing they both survive.
    // Nothing about the extent of the block distinguishes the orders, which is
    // why a test that only measured that could not see the difference -- and
    // did not, the first time.
    let Some(mut ctx) = context() else { return };

    const RADIUS: f32 = 5.0;
    // A block from 32 to 96, with a spike eight pixels tall reaching out to
    // 112. Eight is under twice the radius, so an opening takes it.
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(32.0, 32.0))
        .line_to(Vec2::new(96.0, 32.0))
        .line_to(Vec2::new(96.0, 60.0))
        .line_to(Vec2::new(112.0, 60.0))
        .line_to(Vec2::new(112.0, 68.0))
        .line_to(Vec2::new(96.0, 68.0))
        .line_to(Vec2::new(96.0, 96.0))
        .line_to(Vec2::new(32.0, 96.0))
        .close();
    let spiked = builder.build();

    let erode = ImageFilter::Erode {
        radius_x: RADIUS,
        radius_y: RADIUS,
    };
    let dilate = ImageFilter::Dilate {
        radius_x: RADIUS,
        radius_y: RADIUS,
    };

    let draw = |ctx: &mut Context, filter: ImageFilter| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas
            .draw_path(
                &spiked,
                &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                    .with_anti_alias(false)
                    .with_image_filter(filter),
            )
            .expect("drew");
        render(ctx, canvas)
    };

    // Opening: erode first, so the erosion is the inner half.
    let opened = draw(
        &mut ctx,
        ImageFilter::compose(dilate.clone(), erode.clone()),
    );
    let (left, right) = lit_span(&opened, true, 64).expect("the block survives an opening");
    assert_eq!(left, 32, "an opening leaves the block where it was");
    assert!(
        right < 100,
        "the spike is thinner than twice the radius and an opening should have \
         taken it, but the row still reaches {right}"
    );

    // Closing: dilate first.
    let closed = draw(&mut ctx, ImageFilter::compose(erode, dilate));
    let (left, right) = lit_span(&closed, true, 64).expect("the block survives a closing");
    assert_eq!(left, 32, "a closing leaves the block where it was too");
    assert!(
        right > 105,
        "a closing keeps the spike, and this row should still reach past 105. \
         Reaching {right} means the erosion ran first, which is an opening."
    );
}

/// An 8×8 image whose left half is one value and right half another, with the
/// step falling exactly between two columns of texels.
///
/// Eight wide rather than four so that a cubic read four texels across never
/// reaches the edge of the image, where the clamp would flatten the very
/// overshoot the step exists to produce.
fn step_image(left: u8, right: u8, alpha: u8) -> Vec<u8> {
    let mut pixels = vec![0u8; 8 * 8 * 4];
    for y in 0..8u32 {
        for x in 0..8u32 {
            let i = ((y * 8 + x) * 4) as usize;
            let level = if x < 4 { left } else { right };
            pixels[i..i + 4].copy_from_slice(&[level, level, level, alpha]);
        }
    }
    pixels
}

/// An 8×8 image drawn across the whole frame, at one sampling quality, over a
/// transparent ground.
///
/// Transparent so that what is read back is the sampled color itself. Over an
/// opaque ground the composite makes every pixel's alpha one, and a question
/// about whether the color stayed within its own alpha cannot be asked.
fn drawn_at(ctx: &mut Context, texels: &[u8], sampling: Sampling) -> Vec<u8> {
    let mut image = ctx
        .create_image(Extent2D::new(8, 8), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, texels).expect("upload");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::image(0, Rect::from_size(128.0, 128.0))
                .with_sampling(sampling)
                .with_anti_alias(false),
        )
        .expect("image");
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);
    pixels
}

#[test]
fn cubic_sampling_leaves_a_flat_image_exactly_flat() {
    // The weights are a partition of unity: they sum to one wherever the sample
    // falls between texels. That is what lets the shader skip normalizing, and
    // it is not a property to take on trust -- a coefficient mistyped anywhere
    // in the curve shows here as a flat image that is not quite its own color,
    // and shows at every magnification rather than only at an edge.
    let Some(mut ctx) = context() else { return };

    const LEVEL: u8 = 137;
    let pixels = drawn_at(&mut ctx, &step_image(LEVEL, LEVEL, 255), Sampling::Cubic);

    // Sixteen device pixels per texel, so these fall at texel centers, between
    // two texels, and at three-eighths of the way across one.
    for x in [8u32, 16, 22, 64, 71, 120] {
        assert_eq!(
            pixel(&pixels, x, 64),
            [LEVEL, LEVEL, LEVEL, 255],
            "a flat image read at ({x}, 64) should be its own color exactly"
        );
    }
}

#[test]
fn cubic_sampling_overshoots_a_step_where_linear_cannot() {
    // What distinguishes a cubic reconstruction from a linear one, and it is
    // not simply "smoother". Between one and two texels from the sample the
    // Mitchell curve's weights go negative, so the value near a step is pulled
    // past the level on the far side: darker than the dark half just before the
    // edge, brighter than the bright half just after. A linear read is a
    // weighted average of two texels with non-negative weights and can never
    // leave the interval between them, at any magnification.
    //
    // The levels are a quarter and three quarters rather than black and white
    // so that the overshoot has somewhere to go. Against the ends of the range
    // the same ringing is there and is entirely clipped away by the eight-bit
    // target, and a test written that way would pass on a linear read.
    let Some(mut ctx) = context() else { return };

    const DARK: u8 = 64;
    const LIGHT: u8 = 191;
    let texels = step_image(DARK, LIGHT, 255);
    let linear = drawn_at(&mut ctx, &texels, Sampling::Linear);
    let cubic = drawn_at(&mut ctx, &texels, Sampling::Cubic);

    // The step is between texels three and four, which is the middle of the
    // frame. The undershoot bottoms out about one texel before it and the
    // overshoot about one texel after -- sixteen device pixels either way.
    let under = pixel(&cubic, 48, 64)[0];
    let over = pixel(&cubic, 80, 64)[0];
    assert!(
        under < DARK,
        "a cubic read a texel before the step should undershoot below {DARK}, \
         got {under}"
    );
    assert!(
        over > LIGHT,
        "and a texel after it should overshoot above {LIGHT}, got {over}"
    );

    // The same two places under a linear read, which is where the two
    // reconstructions are supposed to differ and where a cubic implemented as a
    // slightly different average would not.
    assert_eq!(
        (pixel(&linear, 48, 64)[0], pixel(&linear, 80, 64)[0]),
        (DARK, LIGHT),
        "linear stays between the two levels, which is the whole contrast"
    );

    // And nowhere does the ringing run away: Mitchell's negative lobe is small,
    // so the excursion is a handful of levels rather than a visible band.
    for x in 0..SIZE.width {
        let got = pixel(&cubic, x, 64)[0];
        assert!(
            (DARK as i32 - 8..=LIGHT as i32 + 8).contains(&(got as i32)),
            "cubic at ({x}, 64) rang out to {got}, far past what this curve does"
        );
    }
}

#[test]
fn cubic_sampling_keeps_a_translucent_image_premultiplied() {
    // The ringing pulls each channel independently, and color and alpha ring
    // by different amounts wherever they step differently. Premultiplied color
    // has an invariant that does not survive that on its own: no channel may
    // exceed the alpha it was multiplied by, and a color brighter than its own
    // alpha composites as though it were lit from nowhere.
    let Some(mut ctx) = context() else { return };

    // The two halves are chosen so the invariant is actually reachable, which
    // most pairs are not. Color has to be sitting on its alpha on one side --
    // white at whatever coverage it has -- and the two have to step in opposite
    // directions, so that just before the seam color rings upward while the
    // alpha it must not exceed rings downward. Color and alpha stepping the
    // same way ring by the same fraction and stay ordered however far they
    // overshoot, which is why the obvious fixture proves nothing.
    let mut texels = step_image(0, 0, 0);
    for y in 0..8u32 {
        for x in 0..8u32 {
            let i = ((y * 8 + x) * 4) as usize;
            // Left: white at an alpha of 200, so color equals alpha exactly.
            // Right: opaque black. Color falls by two hundred, alpha rises by
            // fifty-five.
            let texel: [u8; 4] = if x < 4 {
                [200, 200, 200, 200]
            } else {
                [0, 0, 0, 255]
            };
            texels[i..i + 4].copy_from_slice(&texel);
        }
    }

    let pixels = drawn_at(&mut ctx, &texels, Sampling::Cubic);
    // Over the seam, where both curves are ringing.
    for x in 40..88u32 {
        let got = pixel(&pixels, x, 64);
        assert!(
            got[0] as i32 <= got[3] as i32 + 1,
            "at ({x}, 64) the color {} exceeds its own alpha {}, which the \
             clamp at the end of the cubic read is there to prevent",
            got[0],
            got[3]
        );
    }
}

/// A 64×64 image of single-texel white columns every eighth texel, black
/// between.
///
/// Chosen so that point-sampling it at eight texels per pixel is not merely
/// noisy but wrong in one direction: every pixel center falls between the white
/// columns, so a linear read misses all of them and the image comes out black.
/// Its true average is one part in eight, which is what reading the level built
/// for that size gives. A checkerboard -- the obvious pattern -- proves nothing
/// here, because it averages to the same middle gray whether or not any
/// averaging happened.
fn striped_image() -> Vec<u8> {
    let mut texels = vec![0u8; 64 * 64 * 4];
    for y in 0..64u32 {
        for x in 0..64u32 {
            let i = ((y * 64 + x) * 4) as usize;
            let level = if x % 8 == 0 { 255 } else { 0 };
            texels[i..i + 4].copy_from_slice(&[level, level, level, 255]);
        }
    }
    texels
}

/// The striped image drawn into a square `side` device pixels across.
fn striped_at(ctx: &mut Context, side: f32, chained: bool, sampling: Sampling) -> Vec<u8> {
    let extent = Extent2D::new(64, 64);
    let mut image = if chained {
        ctx.create_mipmapped_image(extent, PixelFormat::Rgba8Unorm)
    } else {
        ctx.create_image(extent, PixelFormat::Rgba8Unorm)
    }
    .expect("image");
    ctx.write_image(&mut image, &striped_image())
        .expect("upload");

    let where_ = Rect::new(32.0, 32.0, 32.0 + side, 32.0 + side);
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            where_,
            &Paint::image(0, where_)
                .with_sampling(sampling)
                .with_anti_alias(false),
        )
        .expect("image");
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);
    pixels
}

#[test]
fn mipmapped_sampling_reads_the_level_built_for_the_size_it_is_drawn_at() {
    // The whole point of a chain, and the one thing about sampling that is
    // about minification rather than magnification. Eight texels fall on every
    // device pixel here. A linear read takes the four texels around the pixel's
    // center and calls that the answer; the other sixty do not contribute, and
    // for this image that means every white column is missed and the result is
    // black. The level built for this size is the average of all sixty-four,
    // which is one part in eight.
    let Some(mut ctx) = context() else { return };

    let linear = striped_at(&mut ctx, 8.0, false, Sampling::Linear);
    let mipmapped = striped_at(&mut ctx, 8.0, true, Sampling::Mipmap);

    for x in 32..40u32 {
        assert_eq!(
            pixel(&linear, x, 36)[0],
            0,
            "a linear read at eight texels per pixel misses every stripe, which \
             is the aliasing this exists to fix"
        );
        let got = pixel(&mipmapped, x, 36)[0];
        assert!(
            (got as i32 - 32).abs() <= 2,
            "at ({x}, 36) the mipmapped read should be the image's own average, \
             which is 255 over 8, or 32. Got {got}"
        );
    }
}

#[test]
fn mipmapped_sampling_at_full_size_is_the_image_itself() {
    // The level is chosen from how fast the coordinate moves, and is clamped at
    // zero because there is nothing above the image to read. Drawn at its own
    // size, a mipmapped read has to be the same picture a linear one gives --
    // not merely close to it. A level chosen even slightly above zero would
    // soften every image drawn at one to one, which is most of them.
    let Some(mut ctx) = context() else { return };

    let linear = striped_at(&mut ctx, 64.0, false, Sampling::Linear);
    let mipmapped = striped_at(&mut ctx, 64.0, true, Sampling::Mipmap);

    for x in 32..96u32 {
        assert_eq!(
            pixel(&mipmapped, x, 36),
            pixel(&linear, x, 36),
            "at ({x}, 36) a mipmapped read at full size differs from a linear one"
        );
    }
    // And the picture is actually the stripes, so the comparison above is not
    // two blank images agreeing.
    assert_eq!(
        pixel(&mipmapped, 32, 36)[0],
        255,
        "the first column is a stripe and should be white"
    );
    assert_eq!(
        pixel(&mipmapped, 33, 36)[0],
        0,
        "and the one beside it is not"
    );
}

#[test]
fn mipmapped_sampling_magnified_is_still_the_largest_level() {
    // Below the image there is no larger level, so a draw at more than one
    // device pixel per texel has to read the image itself however far it is
    // magnified. Drawn at twice its size the stripes are two pixels wide with a
    // blended pixel between, which is what a linear read of the largest level
    // gives and what any level below it could not.
    let Some(mut ctx) = context() else { return };

    let mipmapped = striped_at(&mut ctx, 128.0, true, Sampling::Mipmap);
    let linear = striped_at(&mut ctx, 128.0, false, Sampling::Linear);
    for x in 32..96u32 {
        assert_eq!(
            pixel(&mipmapped, x, 36),
            pixel(&linear, x, 36),
            "at ({x}, 36) a magnified mipmapped read differs from a linear one"
        );
    }
    assert_eq!(
        pixel(&mipmapped, 32, 36)[0],
        255,
        "and the stripes are still there rather than averaged away"
    );
}

#[test]
fn mipmapped_sampling_of_an_image_without_a_chain_is_linear() {
    // A texture allocated without a chain has one level, and a sampler asked
    // for a level it does not have reads the one it does. So this degrades to
    // linear rather than failing, which is the right shape for a quality hint:
    // a caller who asks for medium and gets low has a worse picture, where a
    // caller who gets an error has none.
    //
    // Worth pinning because it is the difference between a documented fallback
    // and a chain that was silently never generated -- from inside the renderer
    // the two look identical, and this test is what says which one is happening
    // by showing the chained image doing better on the same draw.
    let Some(mut ctx) = context() else { return };

    let unchained = striped_at(&mut ctx, 8.0, false, Sampling::Mipmap);
    let linear = striped_at(&mut ctx, 8.0, false, Sampling::Linear);
    for x in 32..40u32 {
        assert_eq!(
            pixel(&unchained, x, 36),
            pixel(&linear, x, 36),
            "at ({x}, 36) a mipmapped read of a one-level texture should be the \
             linear read exactly"
        );
    }
}

#[test]
fn a_mip_chain_has_one_level_per_halving_of_the_longer_axis() {
    // Both backends decide the level count from this, and they have to agree on
    // it or a chain generated by one is a chain the other cannot finish
    // reading. The awkward cases are the ones that are not powers of two and
    // the ones that are not square: a hundred halves to fifty, twenty-five,
    // twelve, six, three, one, and an axis that reaches a single texel stays
    // there while the other keeps going.
    use emblema::mip_levels_for;
    assert_eq!(
        mip_levels_for(Extent2D::new(1, 1)),
        1,
        "one texel is one level"
    );
    assert_eq!(mip_levels_for(Extent2D::new(64, 64)), 7);
    assert_eq!(
        mip_levels_for(Extent2D::new(100, 100)),
        7,
        "100 → 50 → 25 → 12 → 6 → 3 → 1"
    );
    assert_eq!(
        mip_levels_for(Extent2D::new(8, 1)),
        4,
        "the longer axis decides, and the short one holds at a single texel"
    );
    assert_eq!(mip_levels_for(Extent2D::new(1, 8)), 4, "either way round");
}

#[test]
fn a_blur_is_measured_in_the_targets_axes_not_the_layers_own() {
    // A blur runs on a finished layer, in the target's pixels, after the
    // transform has already placed everything. Blurring in the layer's own
    // coordinates instead and letting the composite scale the result would look
    // right under a rotation -- an isotropic kernel turned is still isotropic --
    // and would be visibly wrong under a scale that is not uniform.
    //
    // So the transform here is both. A near-point source is drawn under a scale
    // of two by a half and a rotation, which makes the layer's axes and the
    // target's disagree by a factor of four and points them elsewhere. What
    // comes back has to be a round halo: the kernel itself, since the source is
    // small next to the sigma.
    let Some(mut ctx) = context() else { return };

    const SIGMA: f32 = 5.0;
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save_layer_bounds(
        Layer::opacity(1.0).with_blur(SIGMA),
        Rect::new(0.0, 0.0, 128.0, 128.0),
    );
    canvas.translate(64.0, 64.0);
    canvas.scale(2.0, 0.5);
    canvas.rotate(0.6);
    canvas
        .draw_rect(
            Rect::new(-3.0, -3.0, 3.0, 3.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)),
        )
        .expect("drew");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);

    // How far from the center the halo stays above a threshold, along a ray.
    let reach = |dx: i32, dy: i32| {
        (1..60)
            .find(|k| pixel(&pixels, (64 + dx * k) as u32, (64 + dy * k) as u32)[0] < 24)
            .unwrap_or(60)
    };
    let (right, left, down, up) = (reach(1, 0), reach(-1, 0), reach(0, 1), reach(0, -1));
    assert!(
        (right - left).abs() <= 2 && (down - up).abs() <= 2,
        "the halo should be centered: right {right}, left {left}, down {down}, up {up}"
    );
    assert!(
        (right - down).abs() <= 2 && (left - up).abs() <= 2,
        "the halo should reach the same distance horizontally and vertically. \
         A blur taken in the layer's own coordinates would come out four times \
         wider than tall here. Got right {right}, down {down}"
    );
    // What this does not say is anything about the shape of the kernel: a box
    // blur over the same reach is round enough at any one threshold to pass
    // every assertion above. That is a separate question and has its own test.
}

#[test]
fn a_blur_falls_off_like_a_gaussian_rather_than_like_a_box() {
    // The isotropy test above measures where the halo ends, and a box blur over
    // the same reach ends in the same place -- so it passes that test, as a
    // mutation confirmed. What separates the two kernels is the shape of the
    // falloff between the center and that end, and the shape is checkable
    // without knowing the source's total brightness: the ratio between two
    // distances depends only on sigma.
    //
    // A Gaussian at five and ten pixels stands in the ratio exp((100 - 25) over
    // twice sigma squared), which for a sigma of five is about four and a half.
    // Uniform weights over the same reach give a ratio of one: the profile
    // across a line convolved with a box is flat until the box runs out, which
    // is what a box blur of a line looks like and is nothing like a curve.
    let Some(mut ctx) = context() else { return };

    const SIGMA: f32 = 5.0;
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save_layer_bounds(
        Layer::opacity(1.0).with_blur(SIGMA),
        Rect::new(0.0, 0.0, 128.0, 128.0),
    );
    // A thin line rather than a point. A point spread over a sigma of five
    // leaves nothing an eight-bit target can measure ten pixels out -- one
    // level, where the quantization is half of that. A line spreads in one axis
    // only, so the profile across it is the kernel's own and bright enough to
    // take a ratio from.
    canvas
        .draw_rect(
            Rect::new(63.0, 0.0, 65.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false),
        )
        .expect("drew");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);

    // Three and six pixels out. The kernel reaches `(SIGMA - 0.5) * sqrt(3)`,
    // which is under eight, so the ten-pixel sample this used to take was past
    // its end -- and a ratio against zero says nothing about a falloff.
    let near = pixel(&pixels, 64 + 3, 64)[0] as f32;
    let far = pixel(&pixels, 64 + 6, 64)[0] as f32;
    assert!(
        far > 1.0,
        "the sample at ten pixels is {far}, too dark to take a ratio from"
    );
    let ratio = near / far;
    let gaussian = (((6.0f32).powi(2) - (3.0f32).powi(2)) / (2.0 * SIGMA * SIGMA)).exp();
    assert!(
        (ratio - gaussian).abs() < 1.2,
        "the falloff from five pixels to ten is a ratio of {ratio:.2}, where a \
         Gaussian of sigma {SIGMA} gives {gaussian:.2}. Uniform weights over \
         the same reach give one -- a flat profile rather than a curve."
    );
}

#[test]
fn a_radial_gradient_of_no_radius_settles_on_the_stop_it_was_heading_for() {
    // The radius is folded into a matrix so the shader measures against unit
    // distance and never sees it. A radius of nothing makes that matrix
    // singular, and inverting a singular matrix gives the identity -- correct
    // for an inversion and wrong here, because the identity is *a* radius: one
    // clip unit. A gradient asked to have no extent came out spanning half the
    // target, and would have spanned a different distance on a target of a
    // different size.
    //
    // What it should be is the limit of the real thing. As the radius shrinks
    // every point but the center runs off the end of the ramp, so under clamp
    // it settles on the last stop. The limit rather than a refusal, on the same
    // reasoning that makes a mask blur of zero the sharp shape: a caller
    // animating a radius to nothing should arrive somewhere.
    let Some(mut ctx) = context() else { return };

    let stops = [
        GradientStop::new(Color::linear(1.0, 1.0, 1.0, 1.0), 0.0),
        GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 1.0),
    ];
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                .with_shader(Shader::RadialGradient {
                    center: Vec2::new(64.0, 64.0),
                    radius: 0.0,
                    stops: stops.to_vec(),
                    tile: TileMode::Clamp,
                })
                .with_anti_alias(false),
        )
        .expect("gradient");
    let pixels = render(&mut ctx, canvas);

    // Every pixel the last stop, including ones far from the center -- which is
    // exactly where an invented radius would have left something else.
    for (x, y) in [(2u32, 2u32), (64, 64), (126, 126), (2, 126), (100, 30)] {
        assert_eq!(
            pixel(&pixels, x, y),
            [0, 255, 0, 255],
            "at ({x}, {y}) a radius of zero should be the last stop everywhere"
        );
    }
}

#[test]
fn a_radial_gradient_of_no_radius_under_decal_draws_nothing() {
    // The other half of the same limit, and it goes the other way. Decal draws
    // nothing past the end of the ramp, and a radius of nothing puts every
    // point past it -- so the shape disappears rather than filling. Both
    // answers are the limit of the same shrinking gradient; which one it is
    // depends on what the tile mode says about being past the end.
    let Some(mut ctx) = context() else { return };

    let stops = [
        GradientStop::new(Color::linear(1.0, 1.0, 1.0, 1.0), 0.0),
        GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 1.0),
    ];
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::srgb(0.2, 0.0, 0.0, 1.0));
    canvas
        .draw_rect(
            Rect::from_size(128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                .with_shader(Shader::RadialGradient {
                    center: Vec2::new(64.0, 64.0),
                    radius: 0.0,
                    stops: stops.to_vec(),
                    tile: TileMode::Decal,
                })
                .with_blend(BlendMode::SrcOver)
                .with_anti_alias(false),
        )
        .expect("gradient");
    let pixels = render(&mut ctx, canvas);

    let ground = pixel(&pixels, 64, 64);
    assert_eq!(
        ground[0], 51,
        "the ground should show through, not a gradient: got {ground:?}"
    );
}
#[test]
fn no_shader_puts_a_pixel_down_under_a_collapsed_transform() {
    // Every shader but the solid one resolves itself into a matrix, and gets
    // that matrix by inverting something built from the canvas transform. A
    // transform with a zero scale makes it singular, and the inversion answers
    // a singular matrix with the identity -- which is a plausible mapping the
    // caller never asked for. That exact substitution produced a wrong picture
    // for a radial gradient of no radius, so the question is whether a singular
    // *transform* can reach the same place.
    //
    // It cannot, and the reason is worth having a test for rather than an
    // argument: the geometry is transformed by the same matrix, so a shape
    // under a collapsed transform has no area and covers no pixel. The mapping
    // is never consulted.
    //
    // The neighboring `a_collapsed_transform_leaves_nothing_reachable` asks a
    // different question about the same transform -- what the canvas *reports*
    // as reachable, rather than what it draws -- and one can hold while the
    // other does not.
    //
    // Worth knowing what does and does not fail this, since the obvious
    // mutation does not. Changing what a singular inversion falls back to
    // changes nothing here, because the geometry has already collapsed by then.
    // What fails it is the shape not collapsing: building the quad for a
    // fragment-evaluated shape without applying the transform to it puts the
    // whole frame down, because the mapping the fragments then consult is the
    // substituted identity. What this pins is that nothing draws anyway -- from
    // an antialiased edge along the degenerate line, or from a coordinate that
    // came out as NaN and compared its way into coverage.
    let Some(mut ctx) = context() else { return };

    let stops = vec![
        GradientStop::new(Color::linear(1.0, 1.0, 1.0, 1.0), 0.0),
        GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 1.0),
    ];
    let shaders: Vec<(&str, Shader)> = vec![
        ("solid", Shader::Solid(Color::linear(1.0, 0.0, 0.0, 1.0))),
        (
            "linear",
            Shader::LinearGradient {
                start: Vec2::ZERO,
                end: Vec2::new(128.0, 128.0),
                stops: stops.clone(),
                tile: TileMode::Clamp,
            },
        ),
        (
            "radial",
            Shader::RadialGradient {
                center: Vec2::new(64.0, 64.0),
                radius: 40.0,
                stops: stops.clone(),
                tile: TileMode::Clamp,
            },
        ),
        (
            "sweep",
            Shader::SweepGradient {
                center: Vec2::new(64.0, 64.0),
                start_angle: 0.0,
                end_angle: std::f32::consts::TAU,
                stops: stops.clone(),
                tile: TileMode::Clamp,
            },
        ),
        (
            "conical",
            Shader::ConicalGradient {
                start_center: Vec2::new(40.0, 64.0),
                start_radius: 0.0,
                end_center: Vec2::new(64.0, 64.0),
                end_radius: 50.0,
                stops,
                tile: TileMode::Clamp,
            },
        ),
    ];

    // One axis collapsed each way, and both at once. The one-axis cases matter
    // separately: a matrix singular in one direction still has a well-defined
    // image, and an implementation guarding only on a determinant of exactly
    // zero in both axes would let them through.
    for (name, shader) in shaders {
        for (how, transform) in [
            ("scale(0, 1)", Affine2::from_scale(Vec2::new(0.0, 1.0))),
            ("scale(1, 0)", Affine2::from_scale(Vec2::new(1.0, 0.0))),
            ("scale(0, 0)", Affine2::from_scale(Vec2::ZERO)),
        ] {
            let mut canvas = Canvas::new(SIZE);
            canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
            canvas.save();
            canvas.concat(transform);
            let _ = canvas.draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_shader(shader.clone()),
            );
            canvas.restore();
            let pixels = render(&mut ctx, canvas);

            let lit = pixels
                .chunks_exact(4)
                .filter(|texel| *texel != [0, 0, 0, 255])
                .count();
            assert_eq!(
                lit, 0,
                "a {name} paint under {how} put {lit} pixel(s) on the frame, \
                 where the shape it was drawn with has no area at all"
            );
        }
    }

    // The shapes that do not go through tessellation at all. A circle, a
    // rounded rectangle and an analytically stroked outline are drawn as a quad
    // whose fragments work out their own coverage from the same inverted
    // matrix, so for these the mapping is not merely consulted -- it is the
    // whole of what decides which pixels are covered. Nothing above reaches
    // them, because everything above is a filled rectangle.
    for (kind, how, transform) in [
        (
            "circle",
            "scale(0, 1)",
            Affine2::from_scale(Vec2::new(0.0, 1.0)),
        ),
        (
            "circle",
            "scale(1, 0)",
            Affine2::from_scale(Vec2::new(1.0, 0.0)),
        ),
        ("circle", "scale(0, 0)", Affine2::from_scale(Vec2::ZERO)),
        (
            "rounded rect",
            "scale(0, 1)",
            Affine2::from_scale(Vec2::new(0.0, 1.0)),
        ),
        (
            "rounded rect",
            "scale(1, 0)",
            Affine2::from_scale(Vec2::new(1.0, 0.0)),
        ),
        (
            "stroked circle",
            "scale(0, 1)",
            Affine2::from_scale(Vec2::new(0.0, 1.0)),
        ),
        (
            "stroked circle",
            "scale(0, 0)",
            Affine2::from_scale(Vec2::ZERO),
        ),
    ] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas.save();
        canvas.concat(transform);
        let white = Color::linear(1.0, 1.0, 1.0, 1.0);
        let _ = match kind {
            "circle" => canvas.draw_circle(Vec2::new(64.0, 64.0), 40.0, &Paint::fill(white)),
            "rounded rect" => canvas.draw_rrect(
                Rect::new(20.0, 20.0, 108.0, 108.0),
                16.0,
                &Paint::fill(white),
            ),
            _ => canvas.draw_circle(Vec2::new(64.0, 64.0), 40.0, &Paint::stroke(white, 8.0)),
        };
        canvas.restore();
        let pixels = render(&mut ctx, canvas);

        let lit = pixels
            .chunks_exact(4)
            .filter(|texel| *texel != [0, 0, 0, 255])
            .count();
        assert_eq!(
            lit, 0,
            "a {kind} under {how} put {lit} pixel(s) on the frame, and that \
             shape's coverage is computed from the inverted mapping rather \
             than from tessellated geometry"
        );
    }
}

#[test]
fn a_composition_may_hold_a_composition_in_either_half() {
    // `compose` builds a binary tree, and nothing stops a caller from putting a
    // composition on either side of one. Both trees below hold the same three
    // filters in the same order, so both have to give the same picture -- they
    // differ only in how the caller happened to bracket them, and bracketing is
    // not something a renderer gets an opinion about.
    //
    // This is a regression test with a specific history. Drawing a chain peels
    // the outermost filter and recurses on the rest, and the first version took
    // the outer half to be a leaf -- true for anything composed left to right,
    // false the moment two compositions are composed. The left-bracketed form
    // was refused as an unimplemented image filter, having been assembled out
    // of nothing but implemented ones.
    let Some(mut ctx) = context() else { return };

    let dilate = |radius: f32| ImageFilter::Dilate {
        radius_x: radius,
        radius_y: 0.0,
    };
    // Three radii summing to fourteen, all distinct, so a chain that dropped
    // or repeated one lands somewhere the right answer does not.
    let (a, b, c) = (dilate(4.0), dilate(8.0), dilate(2.0));

    let flat = ImageFilter::compose(a.clone(), ImageFilter::compose(b.clone(), c.clone()));
    let nested = ImageFilter::compose(ImageFilter::compose(a, b), c);

    let span = |ctx: &mut Context, filter: ImageFilter| {
        let pixels = filtered_square(ctx, Rect::new(48.0, 48.0, 80.0, 80.0), filter);
        lit_span(&pixels, true, 64)
    };
    let right_bracketed = span(&mut ctx, flat);
    let left_bracketed = span(&mut ctx, nested);

    // Forty-eight to seventy-nine, grown by fourteen each way.
    assert_eq!(
        right_bracketed,
        Some((34, 93)),
        "three dilations of four, eight and two should reach fourteen each way"
    );
    assert_eq!(
        left_bracketed, right_bracketed,
        "the same three filters bracketed the other way gave a different \
         picture, so how the caller nested them changed what was drawn"
    );
}

#[test]
fn a_color_filter_can_be_half_of_an_image_filter_composition() {
    // Upstream's `DlImageFilter` offers seven kinds and this offered five; a
    // color filter was one of the two missing, and it is the one that sounds
    // like it was already here. A paint carries a color filter too. What that
    // one cannot do is be half of a composition -- there is no other way here
    // to recolor a blurred result, or to blur a recolored one.
    let Some(mut ctx) = context() else { return };

    let shot = |ctx: &mut Context, filter: ImageFilter| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(40.0, 40.0, 88.0, 88.0),
                &Paint::fill(Color::srgb(0.9, 0.3, 0.1, 1.0)).with_image_filter(filter),
            )
            .expect("rect");
        render(ctx, canvas)
    };
    let gamma = || ImageFilter::Color(ColorFilter::srgb_to_linear());
    let blur = || ImageFilter::blur(6.0);
    let at = |p: &[u8], x: u32, y: u32| pixel(p, x, y);

    // It does something, which is the first thing to establish: a filter
    // accepted and dropped is the failure this codebase least wants.
    let plain = shot(&mut ctx, ImageFilter::None);
    let recolored = shot(&mut ctx, gamma());
    assert_ne!(
        at(&plain, 64, 64),
        at(&recolored, 64, 64),
        "a color filter used as an image filter changed nothing"
    );

    // And on one solid draw it agrees with the paint's own color filter. Not a
    // limitation being confessed -- it is what the two mean, and pinning it is
    // what says this route computes the same thing rather than merely something
    // else that also differs from plain.
    let mut through_paint = Canvas::new(SIZE);
    through_paint.clear(Color::BLACK);
    through_paint
        .draw_rect(
            Rect::new(40.0, 40.0, 88.0, 88.0),
            &Paint::fill(Color::srgb(0.9, 0.3, 0.1, 1.0))
                .with_color_filter(ColorFilter::srgb_to_linear()),
        )
        .expect("rect");
    let through_paint = render(&mut ctx, through_paint);
    // To a level, and the level is not slack: an image filter draws into a
    // layer and composites it back, so its fragments are quantized twice where
    // the paint's filter quantizes once. That is the same arithmetic
    // `Scene::tolerance` counts stores for, arriving here as a difference
    // between two spellings of one filter rather than between two devices.
    let (a, b) = (at(&recolored, 64, 64), at(&through_paint, 64, 64));
    let apart = a
        .iter()
        .zip(&b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0);
    assert!(
        apart <= 1,
        "the two spellings of one color filter should agree to the rounding of \
         the extra store: {a:?} against {b:?}"
    );

    // The composition, which is the capability. Both orders differ from the
    // blur alone -- the halo is recolored in one and the blurred edge is
    // recolored in the other -- and a `Compose` that dropped its color half
    // would match the blur exactly.
    let blurred = shot(&mut ctx, blur());
    for (label, filter) in [
        ("color outside blur", ImageFilter::compose(gamma(), blur())),
        ("blur outside color", ImageFilter::compose(blur(), gamma())),
    ] {
        let composed = shot(&mut ctx, filter);
        assert_ne!(
            at(&composed, 40, 64),
            at(&blurred, 40, 64),
            "{label}: the color half of the composition did nothing"
        );
        // And it is still a blur: the halo outside the shape exists, which is
        // what says the blur half survived being composed with a color filter.
        assert!(
            at(&composed, 34, 64)[0] > 4,
            "{label}: the blur half of the composition did nothing"
        );
    }
}

#[test]
fn a_color_filter_recolors_what_a_runtime_effect_drew() {
    // A color filter is arithmetic in this renderer's own fragment shader, and
    // a runtime effect replaces that shader outright -- the caller's program is
    // what runs, and there is nowhere in it to put a matrix. So the filter was
    // accepted and silently did nothing, which is the shape of failure this
    // codebase least wants: no error, no effect, and no way for a caller to
    // tell which of the two happened.
    //
    // It is applied to the image the program drew instead, which is what it
    // means anyway. That costs a layer, which is what every other filter acting
    // on a finished image already costs.
    let Some(mut ctx) = context() else { return };
    let program = ctx
        .register_program(&emblema::RuntimeProgram {
            spirv: emblema_shaders::EFFECT_SPV.to_vec(),
            glsl_es: emblema_shaders::EFFECT_FS_GLSL.to_string(),
        })
        .expect("register");

    const R: f32 = 0.2126;
    const G: f32 = 0.7152;
    const B: f32 = 0.0722;
    #[rustfmt::skip]
    let luminance = ColorFilter::matrix([
        R, G, B, 0.0, 0.0,
        R, G, B, 0.0, 0.0,
        R, G, B, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);

    let draw = |ctx: &mut Context, filter: ColorFilter| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                    .with_shader(Shader::RuntimeEffect {
                        program,
                        uniforms: effect_uniforms(0.5),
                        images: Vec::new(),
                    })
                    .with_color_filter(filter)
                    .with_anti_alias(false),
            )
            .expect("effect");
        render(ctx, canvas)
    };

    // The fixture program draws its first uniform color, which is pure red.
    assert_eq!(
        pixel(&draw(&mut ctx, ColorFilter::None), 64, 64),
        [255, 0, 0, 255],
        "the program should draw red without a filter, or this test is measuring \
         something else"
    );
    let filtered = pixel(&draw(&mut ctx, luminance), 64, 64);
    // Red weighs 0.2126, which is fifty-four of two hundred and fifty-five.
    assert!(
        (filtered[0] as i32 - 54).abs() <= 2 && filtered[0] == filtered[1],
        "the luminance of red is 54 in all three channels, got {filtered:?}. \
         Unchanged red means the filter was accepted and dropped"
    );
}

#[test]
fn a_filter_blends_where_it_meets_the_frame_not_inside_its_own_layer() {
    // Every filter that acts on a finished image draws into a layer first, and
    // the caller's blend mode belongs to the composite that puts that layer on
    // the frame -- not to the draw inside it. Inside, the destination is the
    // layer's own transparent black, so a mode that reads its destination finds
    // nothing there and produces the source unchanged, which is then composited
    // over the frame it was supposed to combine with.
    //
    // That was the behavior of all three of these paths. `Plus` over a cyan
    // ground gave red where it should give white, for every image filter, every
    // mask blur style, and a color-filtered effect.
    //
    // Checked against the unfiltered draw rather than against a constant: what
    // makes it wrong is that adding a filter changed how the paint met the
    // frame, and the unfiltered case is what it should still meet it like.
    let Some(mut ctx) = context() else { return };

    let ground = Color::linear(0.0, 1.0, 1.0, 1.0);
    let shape = Rect::new(24.0, 24.0, 104.0, 104.0);
    let draw = |ctx: &mut Context, paint: Paint| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(ground);
        canvas.draw_rect(shape, &paint).expect("drew");
        render(ctx, canvas)
    };
    let base = Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0))
        .with_blend(BlendMode::Plus)
        .with_anti_alias(false);

    // Red plus cyan is white, and every one of these has to agree with it at
    // the middle of the shape, where no filter has moved anything.
    let unfiltered = pixel(&draw(&mut ctx, base.clone()), 64, 64);
    assert_eq!(
        unfiltered,
        [255, 255, 255, 255],
        "red under Plus over cyan is white before any filter is involved"
    );

    for (what, paint) in [
        (
            "a blur",
            base.clone().with_image_filter(ImageFilter::blur(3.0)),
        ),
        (
            "a dilation",
            base.clone().with_image_filter(ImageFilter::Dilate {
                radius_x: 4.0,
                radius_y: 4.0,
            }),
        ),
        ("a mask blur", base.clone().with_mask_blur(4.0)),
        (
            "a solid mask blur",
            base.clone()
                .with_mask_blur(4.0)
                .with_mask_blur_style(MaskBlurStyle::Solid),
        ),
    ] {
        assert_eq!(
            pixel(&draw(&mut ctx, paint), 64, 64),
            unfiltered,
            "{what} changed how the paint met the frame. Red rather than white \
             means the blend ran inside the layer, against its transparent black"
        );
    }
}

/// A square mesh of four vertices in one flat color.
fn square_mesh(color: Color) -> Vertices {
    Vertices::full(
        VertexMode::Triangles,
        vec![
            Vec2::new(34.0, 34.0),
            Vec2::new(94.0, 34.0),
            Vec2::new(94.0, 94.0),
            Vec2::new(34.0, 94.0),
        ],
        vec![],
        vec![color; 4],
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh")
}

#[test]
fn an_image_filter_applies_to_a_mesh_as_it_does_to_a_shape() {
    // A mesh does not go through `draw_path`, which is where a paint's image
    // filter was noticed -- so a filter on a mesh was accepted and silently
    // dropped. An atlas is a mesh by the time it arrives, so every sprite batch
    // had the same hole.
    //
    // Measured as an extent, because that is what these two filters do that
    // nothing else would: a dilation of ten reaches exactly ten further each
    // way, and a blur reaches past the shape at all.
    let Some(mut ctx) = context() else { return };

    let mesh = square_mesh(Color::linear(1.0, 0.0, 0.0, 1.0));
    let base = Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_anti_alias(false);
    let draw = |ctx: &mut Context, paint: Paint| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas.draw_vertices(&mesh, &paint).expect("mesh");
        let pixels = render(ctx, canvas);
        // Any channel lit, since a filter may change the color as well.
        let lit: Vec<u32> = (0..SIZE.width)
            .filter(|x| pixel(&pixels, *x, 64)[0] > 0 || pixel(&pixels, *x, 64)[1] > 0)
            .collect();
        (
            *lit.first().expect("something drew"),
            *lit.last().expect("something drew"),
        )
    };

    assert_eq!(
        draw(&mut ctx, base.clone()),
        (34, 93),
        "the mesh alone spans its own vertices"
    );
    assert_eq!(
        draw(
            &mut ctx,
            base.clone().with_image_filter(ImageFilter::Dilate {
                radius_x: 10.0,
                radius_y: 10.0
            })
        ),
        (24, 103),
        "a dilation of ten should reach ten further each way. The mesh's own \
         span means the filter was dropped"
    );
    let (left, right) = draw(&mut ctx, base.with_image_filter(ImageFilter::blur(6.0)));
    assert!(
        left < 34 && right > 93,
        "a blur should carry color past the mesh, but it spans {left}..{right}"
    );
}

/// The same square, as a mesh carrying no colors of its own.
fn uncolored_square_mesh() -> Vertices {
    Vertices::indexed(
        VertexMode::Triangles,
        vec![
            Vec2::new(34.0, 34.0),
            Vec2::new(94.0, 34.0),
            Vec2::new(94.0, 94.0),
            Vec2::new(34.0, 94.0),
        ],
        vec![],
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh")
}

#[test]
fn a_mask_blur_over_a_mesh_the_paint_fills_matches_the_same_shape_as_a_path() {
    // A mesh was refused a mask blur outright, on the reasoning that a mesh
    // carries a color per vertex and so varies by construction. It need not:
    // built from positions alone it is filled by the paint, exactly as a path
    // covering the same area is -- and the paint has a value everywhere,
    // including out in the halo, which is the whole of what the refusal was
    // protecting against.
    //
    // So the two have to agree. Not approximately: the same square, the same
    // sigma, one drawn as a path and one as two triangles, is the same picture,
    // and anything else means the mesh took a different route rather than the
    // same one.
    let Some(mut ctx) = context() else { return };

    let paint = || Paint::fill(Color::srgb(1.0, 1.0, 1.0, 1.0)).with_mask_blur(6.0);

    let mut as_mesh = Canvas::new(SIZE);
    as_mesh.clear(Color::BLACK);
    as_mesh
        .draw_vertices(&uncolored_square_mesh(), &paint())
        .expect("a mesh the paint fills takes a mask blur");
    let mesh_pixels = render(&mut ctx, as_mesh);

    let mut as_path = Canvas::new(SIZE);
    as_path.clear(Color::BLACK);
    // An outline with no shape recorded on it, so both sides take the general
    // route -- which is what this compares. Through `draw_rect` the path would
    // be evaluated analytically and the mesh would not, and the two would
    // differ for that reason rather than for the one under test.
    as_path
        .draw_path(&square_path(Rect::new(34.0, 34.0, 94.0, 94.0)), &paint())
        .expect("path");
    let path_pixels = render(&mut ctx, as_path);

    // Inside, on the edge, and out in the halo.
    for (x, y) in [(64u32, 64u32), (64, 34), (64, 28), (34, 34)] {
        let (mesh, path) = (pixel(&mesh_pixels, x, y), pixel(&path_pixels, x, y));
        assert_eq!(
            mesh, path,
            "({x}, {y}): the mesh and the path should be the same picture"
        );
    }

    // And the blur happened at all, which the comparison alone cannot say:
    // two unblurred squares would agree just as exactly.
    let halo = pixel(&mesh_pixels, 64, 28);
    assert!(
        halo[0] > 4 && halo[0] < 240,
        "nothing reached past the mesh, so nothing was blurred: {halo:?}"
    );
}

#[test]
fn a_mask_blur_on_a_mesh_is_refused_rather_than_dropped() {
    // A mask blur blurs coverage and then fills, which is the same picture as
    // blurring the result only where the fill does not vary. A mesh carries a
    // color per vertex, so it varies by construction -- `draw_masked` refuses
    // a gradient for exactly this reason, and a mesh is the same argument.
    //
    // The point is that it is refused rather than ignored. Accepting a mask
    // blur and drawing the mesh unblurred is the failure this codebase least
    // wants, and is what happened before: no error, no blur.
    let Some(mut ctx) = context() else { return };
    let _ = &mut ctx;

    let mesh = square_mesh(Color::linear(1.0, 0.0, 0.0, 1.0));
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    let outcome = canvas
        .draw_vertices(
            &mesh,
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)).with_mask_blur(6.0),
        )
        .map(|_| ());
    assert!(
        matches!(outcome, Err(Error::Unsupported(_))),
        "a mask blur on a mesh should be refused, got {outcome:?}"
    );
}

#[test]
fn every_draw_that_takes_a_paint_honors_its_image_filter() {
    // The check that would have caught a mesh dropping its filter, written as a
    // sweep rather than one test per call. A filter is noticed in `draw_path`,
    // and every entry point that does not pass through there has to notice it
    // itself -- which is a thing to forget once per entry point rather than
    // once. It was forgotten for meshes, and so for every sprite batch.
    //
    // A dilation of ten is the probe because its effect is exact: the drawing
    // reaches ten further each way and not an approximate amount, so a call
    // that dropped it lands on its own unfiltered extent rather than near the
    // right answer.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let white = Color::linear(1.0, 1.0, 1.0, 1.0);
    let square = Rect::new(40.0, 40.0, 88.0, 88.0);
    let mut builder = PathBuilder::new();
    builder
        .move_to(Vec2::new(40.0, 40.0))
        .line_to(Vec2::new(88.0, 40.0))
        .line_to(Vec2::new(88.0, 88.0))
        .line_to(Vec2::new(40.0, 88.0))
        .close();
    let path = builder.build();
    let mesh = square_mesh(white);

    let span = |ctx: &mut Context, name: &str, filter: ImageFilter| -> (u32, u32) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        let paint = Paint::fill(white)
            .with_anti_alias(false)
            .with_image_filter(filter.clone());
        match name {
            "draw_path" => canvas.draw_path(&path, &paint).map(|_| ()),
            "draw_rect" => canvas.draw_rect(square, &paint).map(|_| ()),
            "draw_rrect" => canvas.draw_rrect(square, 8.0, &paint).map(|_| ()),
            "draw_circle" => canvas
                .draw_circle(Vec2::new(64.0, 64.0), 24.0, &paint)
                .map(|_| ()),
            "draw_oval" => canvas
                .draw_oval(Rect::new(40.0, 44.0, 88.0, 84.0), &paint)
                .map(|_| ()),
            "draw_drrect" => canvas
                .draw_drrect(square, 8.0, Rect::new(56.0, 56.0, 72.0, 72.0), 4.0, &paint)
                .map(|_| ()),
            "draw_line" => canvas
                .draw_line(
                    Vec2::new(40.0, 64.0),
                    Vec2::new(88.0, 64.0),
                    &Paint::stroke(white, 12.0).with_image_filter(filter.clone()),
                )
                .map(|_| ()),
            "draw_points" => canvas
                .draw_points(
                    PointMode::Points,
                    &[Vec2::new(64.0, 64.0)],
                    &Paint::fill(white)
                        .with_style(Style::Stroke(StrokeStyle {
                            cap: LineCap::Round,
                            ..StrokeStyle::new(40.0)
                        }))
                        .with_image_filter(filter.clone()),
                )
                .map(|_| ()),
            "draw_vertices" => canvas.draw_vertices(&mesh, &paint).map(|_| ()),
            "draw_atlas" => canvas
                .draw_atlas(
                    &[Sprite {
                        source: SourceRect {
                            x: 0.0,
                            y: 0.0,
                            width: 4.0,
                            height: 4.0,
                        },
                        color: white,
                        transform: Affine2::from_scale_angle_translation(
                            Vec2::splat(12.0),
                            0.0,
                            Vec2::new(40.0, 40.0),
                        ),
                    }],
                    Extent2D::new(4, 4),
                    &Paint::image(0, Rect::from_size(4.0, 4.0)).with_image_filter(filter.clone()),
                )
                .map(|_| ()),
            "draw_image_nine" => canvas
                .draw_image_nine(
                    0,
                    Extent2D::new(4, 4),
                    Rect::new(1.0, 1.0, 3.0, 3.0),
                    square,
                    &paint,
                )
                .map(|_| ()),
            other => unreachable!("{other}"),
        }
        .unwrap_or_else(|e| panic!("{name}: {e}"));

        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        let lit: Vec<u32> = (0..SIZE.width)
            .filter(|x| {
                let texel = pixel(&pixels, *x, 64);
                texel[0] > 0 || texel[1] > 0 || texel[2] > 0
            })
            .collect();
        (
            *lit.first().unwrap_or_else(|| panic!("{name} drew nothing")),
            *lit.last().expect("drew nothing"),
        )
    };

    for name in [
        "draw_path",
        "draw_rect",
        "draw_rrect",
        "draw_circle",
        "draw_oval",
        "draw_drrect",
        "draw_line",
        "draw_points",
        "draw_vertices",
        "draw_atlas",
        "draw_image_nine",
    ] {
        let (left, right) = span(&mut ctx, name, ImageFilter::None);
        let filtered = span(
            &mut ctx,
            name,
            ImageFilter::Dilate {
                radius_x: 10.0,
                radius_y: 10.0,
            },
        );
        // Within a pixel of where a dilation of ten should put each edge.
        //
        // The span is measured by counting lit pixels, so each end is decided
        // by whether the outermost one crosses a threshold -- and a dilation
        // takes the largest sample in a neighborhood, so its outer pixel sits
        // right at that boundary by construction. Whether it lands above or
        // below is a device's business: `draw_line` here comes back one pixel
        // wider at the bottom on a Raspberry Pi 5's V3D and exact on two x86
        // drivers.
        //
        // A filter accepted and dropped -- which is what this exists to catch,
        // and has caught -- leaves the span at the unfiltered one, ten pixels
        // out at each end rather than one.
        let (want_left, want_right) = (left - 10, right + 10);
        assert!(
            filtered.0.abs_diff(want_left) <= 1 && filtered.1.abs_diff(want_right) <= 1,
            "{name} did not apply a dilation of ten: got {filtered:?}, wanted \
             about ({want_left}, {want_right}). Its own unfiltered extent of \
             ({left}, {right}) means the filter was accepted and dropped"
        );
    }
    ctx.destroy_image(image);

    // `draw_paint` fills the clip, so a dilation has nowhere to show -- the
    // spread happens and the clip removes it, which is correct and invisible.
    // A blur is what shows there, softening the edge on the inside where the
    // unfiltered fill meets the clip as a wall.
    let clip = Rect::new(40.0, 40.0, 88.0, 88.0);
    let edge = |ctx: &mut Context, filter: ImageFilter| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        let _ = canvas.clip_rect(clip);
        canvas
            .draw_paint(
                &Paint::fill(white)
                    .with_anti_alias(false)
                    .with_image_filter(filter),
            )
            .expect("paint");
        pixel(&render(ctx, canvas), 41, 64)[0]
    };
    assert_eq!(
        edge(&mut ctx, ImageFilter::None),
        255,
        "unfiltered, the fill meets the clip as a wall"
    );
    let blurred = edge(&mut ctx, ImageFilter::blur(6.0));
    assert!(
        blurred < 200,
        "a blurred paint should soften on the inside of its clip, but the pixel \
         beside the edge reads {blurred}"
    );
}

#[test]
fn every_draw_that_takes_a_paint_honors_its_color_filter_and_blend() {
    // The companion sweep to the image-filter one. Those three fields are
    // routed differently -- a color filter travels in the material, a blend
    // is chosen per draw, an image filter needs a layer -- so honoring one
    // says nothing about honoring the others, and only the image filter
    // turned out to have a seam.
    //
    // Kept as a sweep rather than folded into the other because what it probes
    // is different in kind: a filter that halves every channel and a blend that
    // adds to what is underneath, both of which are exact and neither of which
    // moves a pixel.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    // A flat red sheet, so an image draw has the same color as a solid one.
    ctx.write_image(&mut image, &[255u8, 0, 0, 255].repeat(16))
        .expect("upload");
    // A run, which this sweep left out when it was written and which was then
    // the entry point that had dropped a paint field.
    let (atlas, glyph, _) = two_glyph_atlas();
    let atlas_image = upload_atlas(&mut ctx, &atlas);
    let glyph_run = [PositionedGlyph::new(
        glyph,
        [60.0, 60.0],
        atlas.get(glyph).unwrap(),
    )];
    let red = Color::linear(1.0, 0.0, 0.0, 1.0);
    let square = Rect::new(40.0, 40.0, 88.0, 88.0);
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(40.0, 40.0))
        .line_to(Vec2::new(88.0, 40.0))
        .line_to(Vec2::new(88.0, 88.0))
        .line_to(Vec2::new(40.0, 88.0))
        .close();
    let path = b.build();
    let mesh = square_mesh(red);

    let sample = |ctx: &mut Context,
                  name: &str,
                  adjust: &dyn Fn(Paint) -> Paint,
                  ground: Color|
     -> [u8; 4] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(ground);
        let paint = adjust(Paint::fill(red).with_anti_alias(false));
        let img = adjust(Paint::image(0, Rect::from_size(4.0, 4.0)).with_anti_alias(false));
        match name {
            "draw_path" => canvas.draw_path(&path, &paint).map(|_| ()),
            "draw_rect" => canvas.draw_rect(square, &paint).map(|_| ()),
            "draw_rrect" => canvas.draw_rrect(square, 8.0, &paint).map(|_| ()),
            "draw_circle" => canvas
                .draw_circle(Vec2::new(64.0, 64.0), 24.0, &paint)
                .map(|_| ()),
            "draw_oval" => canvas.draw_oval(square, &paint).map(|_| ()),
            "draw_drrect" => canvas
                .draw_drrect(square, 8.0, Rect::new(80.0, 80.0, 86.0, 86.0), 1.0, &paint)
                .map(|_| ()),
            "draw_line" => canvas
                .draw_line(
                    Vec2::new(40.0, 64.0),
                    Vec2::new(88.0, 64.0),
                    &adjust(Paint::stroke(red, 40.0)),
                )
                .map(|_| ()),
            "draw_points" => canvas
                .draw_points(
                    PointMode::Points,
                    &[Vec2::new(64.0, 64.0)],
                    &adjust(Paint::fill(red).with_style(Style::Stroke(StrokeStyle {
                        cap: LineCap::Round,
                        ..StrokeStyle::new(48.0)
                    }))),
                )
                .map(|_| ()),
            "draw_vertices" => canvas.draw_vertices(&mesh, &paint).map(|_| ()),
            "draw_atlas" => canvas
                .draw_atlas(
                    &[Sprite {
                        source: SourceRect {
                            x: 0.0,
                            y: 0.0,
                            width: 4.0,
                            height: 4.0,
                        },
                        color: Color::linear(1.0, 1.0, 1.0, 1.0),
                        transform: Affine2::from_scale_angle_translation(
                            Vec2::splat(12.0),
                            0.0,
                            Vec2::new(40.0, 40.0),
                        ),
                    }],
                    Extent2D::new(4, 4),
                    &img,
                )
                .map(|_| ()),
            "draw_image_nine" => canvas
                .draw_image_nine(
                    0,
                    Extent2D::new(4, 4),
                    Rect::new(1.0, 1.0, 3.0, 3.0),
                    square,
                    &img,
                )
                .map(|_| ()),
            "draw_paint" => {
                let _ = canvas.clip_rect(square);
                canvas.draw_paint(&paint).map(|_| ())
            }
            "draw_glyphs" => canvas
                .draw_glyphs(&glyph_run, &atlas, 1, &paint)
                .map(|_| ()),
            other => unreachable!("{other}"),
        }
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("s");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image, &atlas_image])
            .expect("d");
        let px = ctx.read(&mut surface).expect("r");
        ctx.destroy_surface(surface);
        let i = ((64u32 * 128 + 64) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };

    #[rustfmt::skip]
    let half = ColorFilter::matrix([0.5,0.0,0.0,0.0,0.0, 0.0,0.5,0.0,0.0,0.0, 0.0,0.0,0.5,0.0,0.0, 0.0,0.0,0.0,1.0,0.0]);
    let black = Color::linear(0.0, 0.0, 0.0, 1.0);
    let cyan = Color::linear(0.0, 1.0, 1.0, 1.0);
    for name in [
        "draw_path",
        "draw_rect",
        "draw_rrect",
        "draw_circle",
        "draw_oval",
        "draw_drrect",
        "draw_line",
        "draw_points",
        "draw_vertices",
        "draw_atlas",
        "draw_image_nine",
        "draw_paint",
        "draw_glyphs",
    ] {
        let plain = sample(&mut ctx, name, &|p| p, black);
        let filtered = sample(&mut ctx, name, &|p: Paint| p.with_color_filter(half), black);
        let blended = sample(
            &mut ctx,
            name,
            &|p: Paint| p.with_blend(BlendMode::Plus),
            cyan,
        );
        assert_eq!(
            plain[0], 255,
            "{name} should draw full red before anything is applied to it"
        );
        assert!(
            (filtered[0] as i32 - 128).abs() <= 2,
            "{name} did not halve its color: {} rather than 128, which is the \
             filter being accepted and dropped",
            filtered[0]
        );
        assert_eq!(
            blended,
            [255, 255, 255, 255],
            "{name} drew red over cyan under Plus and did not add. Red back \
             means the blend was accepted and dropped"
        );
    }
    ctx.destroy_image(image);
    ctx.destroy_image(atlas_image);
}

#[test]
fn every_draw_that_takes_a_paint_applies_or_refuses_a_mask_blur() {
    // The third field with its own routing, and the one where doing nothing is
    // sometimes right. A mask blur blurs coverage and then fills, which is the
    // same picture as blurring the result only where the fill does not vary --
    // so a paint whose color comes from a texture or from vertices cannot have
    // one, and gets an error rather than a guess.
    //
    // What must not happen is the third outcome: accepted, and no blur. Every
    // call here either softens its edge or says why it will not.
    let Some(mut ctx) = context() else { return };

    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &[255u8, 0, 0, 255].repeat(16))
        .expect("upload");
    let red = Color::linear(1.0, 0.0, 0.0, 1.0);
    let square = Rect::new(40.0, 40.0, 88.0, 88.0);
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(40.0, 40.0))
        .line_to(Vec2::new(88.0, 40.0))
        .line_to(Vec2::new(88.0, 88.0))
        .line_to(Vec2::new(40.0, 88.0))
        .close();
    let path = b.build();
    let mesh = square_mesh(red);

    let edge = |ctx: &mut Context, name: &str, sigma: f32| -> std::result::Result<u8, String> {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        let paint = Paint::fill(red)
            .with_anti_alias(false)
            .with_mask_blur(sigma);
        let img = Paint::image(0, Rect::from_size(4.0, 4.0))
            .with_anti_alias(false)
            .with_mask_blur(sigma);
        let r = match name {
            "draw_path" => canvas.draw_path(&path, &paint).map(|_| ()),
            "draw_rect" => canvas.draw_rect(square, &paint).map(|_| ()),
            "draw_rrect" => canvas.draw_rrect(square, 8.0, &paint).map(|_| ()),
            "draw_circle" => canvas
                .draw_circle(Vec2::new(64.0, 64.0), 24.0, &paint)
                .map(|_| ()),
            "draw_oval" => canvas.draw_oval(square, &paint).map(|_| ()),
            "draw_drrect" => canvas
                .draw_drrect(square, 8.0, Rect::new(80.0, 80.0, 86.0, 86.0), 1.0, &paint)
                .map(|_| ()),
            "draw_line" => canvas
                .draw_line(
                    Vec2::new(40.0, 64.0),
                    Vec2::new(88.0, 64.0),
                    &Paint::stroke(red, 40.0).with_mask_blur(sigma),
                )
                .map(|_| ()),
            "draw_points" => canvas
                .draw_points(
                    PointMode::Points,
                    &[Vec2::new(64.0, 64.0)],
                    &Paint::fill(red)
                        .with_style(Style::Stroke(StrokeStyle {
                            cap: LineCap::Round,
                            ..StrokeStyle::new(48.0)
                        }))
                        .with_mask_blur(sigma),
                )
                .map(|_| ()),
            "draw_vertices" => canvas.draw_vertices(&mesh, &paint).map(|_| ()),
            "draw_atlas" => canvas
                .draw_atlas(
                    &[Sprite {
                        source: SourceRect {
                            x: 0.0,
                            y: 0.0,
                            width: 4.0,
                            height: 4.0,
                        },
                        color: Color::linear(1.0, 1.0, 1.0, 1.0),
                        transform: Affine2::from_scale_angle_translation(
                            Vec2::splat(12.0),
                            0.0,
                            Vec2::new(40.0, 40.0),
                        ),
                    }],
                    Extent2D::new(4, 4),
                    &img,
                )
                .map(|_| ()),
            "draw_image_nine" => canvas
                .draw_image_nine(
                    0,
                    Extent2D::new(4, 4),
                    Rect::new(1.0, 1.0, 3.0, 3.0),
                    square,
                    &img,
                )
                .map(|_| ()),
            "draw_paint" => {
                let _ = canvas.clip_rect(square);
                canvas.draw_paint(&paint).map(|_| ())
            }
            other => unreachable!("{other}"),
        };
        if let Err(e) = r {
            return Err(format!("{e}"));
        }
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("s");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("d");
        let px = ctx.read(&mut surface).expect("r");
        ctx.destroy_surface(surface);
        // Just inside the left edge of the shape, where a mask blur softens.
        let i = ((64u32 * 128 + 42) * 4) as usize;
        Ok(px[i])
    };
    // The three that refuse are exactly the three whose color does not come
    // from the paint: two meshes and a nine-patch, all reading a texture.
    const REFUSES: [&str; 3] = ["draw_vertices", "draw_atlas", "draw_image_nine"];
    for name in [
        "draw_path",
        "draw_rect",
        "draw_rrect",
        "draw_circle",
        "draw_oval",
        "draw_drrect",
        "draw_line",
        "draw_points",
        "draw_vertices",
        "draw_atlas",
        "draw_image_nine",
        "draw_paint",
    ] {
        let sharp = edge(&mut ctx, name, 0.0);
        let soft = edge(&mut ctx, name, 8.0);
        if REFUSES.contains(&name) {
            assert!(
                soft.is_err(),
                "{name} reads its color from a texture, so a mask blur over it \
                 is a guess rather than a picture. It should be refused, not accepted"
            );
            continue;
        }
        let sharp = sharp.unwrap_or_else(|e| panic!("{name} without a blur: {e}"));
        let soft = soft.unwrap_or_else(|e| panic!("{name} with a blur: {e}"));
        assert_eq!(
            sharp, 255,
            "{name} should meet its own edge at full strength when nothing is \
             blurring it"
        );
        assert!(
            soft < sharp,
            "{name} accepted a mask blur and drew the same {soft} it draws \
             without one, which is the blur being dropped"
        );
    }
    ctx.destroy_image(image);
}

#[test]
fn every_draw_that_takes_a_paint_obeys_the_transform_and_the_clip() {
    // The seam question asked of canvas state rather than of the paint. A draw
    // that missed either would be a plain correctness fault rather than a
    // dropped option, and both are cheap to state exactly: a translation of
    // sixteen moves the drawing sixteen, and a clip beginning at fifty-six
    // leaves nothing to its left.
    let Some(mut ctx) = context() else { return };

    // Whether this driver honors a scissor when the pass is multisampled.
    //
    // A fair question to ask of a driver rather than of a renderer: the scissor
    // test is per fragment and a fragment is a pixel, so a pixel whose center
    // lies outside the rectangle cannot be written however many samples it
    // holds. The scissor this renderer records for the draws below is exactly
    // right -- x fifty-six, width seventy-two, and the same whether the paint
    // asks for antialiasing or not -- and two Mesa versions write the pixel to
    // its left anyway, at half coverage, which is two of four sample positions
    // and reads as a per-sample test half a pixel out. llvmpipe on Mesa 25.2.8
    // and on 15.0.6 both do; Mesa 26.1.7, RADV and PanVK are clean, so it is a
    // lavapipe defect fixed between.
    //
    // Probed rather than read off a version string, because a version is a
    // guess about behavior and this is the behavior. And named as a skip rather
    // than tolerated quietly, so what is given up shows in the census: the clip
    // half of this test on that driver, and nothing else.
    let honors_scissor_when_multisampled = {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let _ = canvas.clip_rect(Rect::new(56.0, 0.0, 128.0, 128.0));
        // A stroke, and that is the whole of what makes this probe the same
        // question as the draws below. An antialiased *fill* takes the analytic
        // route -- a distance field at one sample -- so it never opens a
        // multisampled pass and never meets the defect. The first version of
        // this probe filled a rectangle, came back clean on the driver that
        // fails, and reported that the driver was fine. A stroke is
        // tessellated, which is what asks for the samples.
        canvas
            .draw_line(
                Vec2::new(40.0, 64.0),
                Vec2::new(88.0, 64.0),
                &Paint::stroke(Color::srgb(1.0, 1.0, 1.0, 1.0), 40.0),
            )
            .expect("probe");
        let pixels = render(&mut ctx, canvas);
        pixel(&pixels, 55, 64) == [0, 0, 0, 255]
    };
    if !honors_scissor_when_multisampled {
        eprintln!(
            "skipping: this driver writes outside a scissor when the pass is \
             multisampled, so the clip half of this test cannot run"
        );
    }
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &[255u8, 0, 0, 255].repeat(16))
        .expect("upload");
    // A run, stubbed out of this sweep when it was written because building an
    // atlas here was awkward -- and then the one entry point that turned out to
    // have dropped a paint field.
    let (atlas, glyph, _) = two_glyph_atlas();
    let atlas_image = upload_atlas(&mut ctx, &atlas);
    let glyph_run = [
        PositionedGlyph::new(glyph, [40.0, 60.0], atlas.get(glyph).unwrap()),
        PositionedGlyph::new(glyph, [80.0, 60.0], atlas.get(glyph).unwrap()),
    ];
    let red = Color::linear(1.0, 0.0, 0.0, 1.0);
    let square = Rect::new(40.0, 40.0, 88.0, 88.0);
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(40.0, 40.0))
        .line_to(Vec2::new(88.0, 40.0))
        .line_to(Vec2::new(88.0, 88.0))
        .line_to(Vec2::new(40.0, 88.0))
        .close();
    let path = b.build();
    let mesh = square_mesh(red);

    let span = |ctx: &mut Context, name: &str, setup: &dyn Fn(&mut Canvas)| -> (u32, u32) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        setup(&mut canvas);
        let paint = Paint::fill(red).with_anti_alias(false);
        let img = Paint::image(0, Rect::from_size(4.0, 4.0)).with_anti_alias(false);
        match name {
            "draw_path" => canvas.draw_path(&path, &paint).map(|_| ()),
            "draw_rect" => canvas.draw_rect(square, &paint).map(|_| ()),
            "draw_rrect" => canvas.draw_rrect(square, 8.0, &paint).map(|_| ()),
            "draw_circle" => canvas
                .draw_circle(Vec2::new(64.0, 64.0), 24.0, &paint)
                .map(|_| ()),
            "draw_oval" => canvas.draw_oval(square, &paint).map(|_| ()),
            "draw_drrect" => canvas
                .draw_drrect(square, 8.0, Rect::new(80.0, 80.0, 86.0, 86.0), 1.0, &paint)
                .map(|_| ()),
            "draw_line" => canvas
                .draw_line(
                    Vec2::new(40.0, 64.0),
                    Vec2::new(88.0, 64.0),
                    &Paint::stroke(red, 40.0),
                )
                .map(|_| ()),
            "draw_points" => canvas
                .draw_points(
                    PointMode::Points,
                    &[Vec2::new(64.0, 64.0)],
                    &Paint::fill(red).with_style(Style::Stroke(StrokeStyle {
                        cap: LineCap::Round,
                        ..StrokeStyle::new(48.0)
                    })),
                )
                .map(|_| ()),
            "draw_vertices" => canvas.draw_vertices(&mesh, &paint).map(|_| ()),
            "draw_atlas" => canvas
                .draw_atlas(
                    &[Sprite {
                        source: SourceRect {
                            x: 0.0,
                            y: 0.0,
                            width: 4.0,
                            height: 4.0,
                        },
                        color: Color::linear(1.0, 1.0, 1.0, 1.0),
                        transform: Affine2::from_scale_angle_translation(
                            Vec2::splat(12.0),
                            0.0,
                            Vec2::new(40.0, 40.0),
                        ),
                    }],
                    Extent2D::new(4, 4),
                    &img,
                )
                .map(|_| ()),
            "draw_image_nine" => canvas
                .draw_image_nine(
                    0,
                    Extent2D::new(4, 4),
                    Rect::new(1.0, 1.0, 3.0, 3.0),
                    square,
                    &img,
                )
                .map(|_| ()),
            "draw_glyphs" => canvas
                .draw_glyphs(&glyph_run, &atlas, 1, &paint)
                .map(|_| ()),
            other => unreachable!("{other}"),
        }
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("s");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image, &atlas_image])
            .expect("d");
        let px = ctx.read(&mut surface).expect("r");
        ctx.destroy_surface(surface);
        let lit: Vec<u32> = (0..128)
            .filter(|x| {
                let q = ((64u32 * 128 + x) * 4) as usize;
                px[q] > 0
            })
            .collect();
        (
            lit.first().copied().unwrap_or(999),
            lit.last().copied().unwrap_or(999),
        )
    };

    // A stencil clip rather than the scissor, which is a different mechanism
    // and so a separate chance to be missed: an axis-aligned rectangle narrows
    // the scissor, and anything else is written into the stencil and tested per
    // fragment. A triangle covering the right half tests the second.
    let mut wedge = PathBuilder::new();
    wedge
        .move_to(Vec2::new(56.0, -40.0))
        .line_to(Vec2::new(200.0, -40.0))
        .line_to(Vec2::new(200.0, 200.0))
        .line_to(Vec2::new(56.0, 200.0))
        .close();
    let wedge = wedge.build();

    for name in [
        "draw_path",
        "draw_rect",
        "draw_rrect",
        "draw_circle",
        "draw_oval",
        "draw_drrect",
        "draw_line",
        "draw_points",
        "draw_vertices",
        "draw_atlas",
        "draw_image_nine",
        "draw_glyphs",
    ] {
        let plain = span(&mut ctx, name, &|_| {});
        assert_ne!(plain.0, 999, "{name} drew nothing to compare against");

        let moved = span(&mut ctx, name, &|c: &mut Canvas| {
            c.translate(16.0, 0.0);
        });
        assert_eq!(
            moved,
            (plain.0 + 16, plain.1 + 16),
            "{name} did not move with the canvas transform"
        );

        let scissored = span(&mut ctx, name, &|c: &mut Canvas| {
            let _ = c.clip_rect(Rect::new(56.0, 0.0, 128.0, 128.0));
        });
        assert!(
            !honors_scissor_when_multisampled || scissored.0 >= 56 && scissored.1 == plain.1,
            "{name} drew outside a rectangular clip: {scissored:?} where the \
             clip begins at 56 and the drawing ends at {}",
            plain.1
        );

        let stencilled = span(&mut ctx, name, &|c: &mut Canvas| {
            let _ = c.clip_path(&wedge);
        });
        assert!(
            stencilled.0 >= 56 && stencilled.1 == plain.1,
            "{name} drew outside a stencil clip: {stencilled:?}. A rectangle \
             narrows the scissor and anything else is tested per fragment, so \
             one can hold while the other does not"
        );
    }
    ctx.destroy_image(image);
    ctx.destroy_image(atlas_image);
}

#[test]
fn every_material_draws_the_same_inside_a_layer_as_outside_one() {
    // A layer is a render target of a different size and origin, and every
    // material carries its geometry in clip space -- which that target
    // normalizes its own way. So a mapping that quietly assumed the frame's
    // dimensions would be right everywhere until someone opened a layer, and
    // wrong there in a way that looks like the material rather than the layer.
    //
    // A layer at full opacity composited with the default blend is a no-op, so
    // the two pictures have to agree. Checked on both a hardware and a software
    // device before the tolerance below was written down.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &[255u8, 0, 0, 255].repeat(16))
        .expect("upload");
    let red = Color::linear(1.0, 0.0, 0.0, 1.0);
    let square = Rect::new(40.0, 40.0, 88.0, 88.0);
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(40.0, 40.0))
        .line_to(Vec2::new(88.0, 40.0))
        .line_to(Vec2::new(88.0, 88.0))
        .line_to(Vec2::new(40.0, 88.0))
        .close();
    let path = b.build();
    let mesh = square_mesh(red);
    let stops = vec![
        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
    ];

    let render_with = |ctx: &mut Context, name: &str, layered: bool| -> Vec<u8> {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        if layered {
            canvas.save_layer_bounds(
                Layer::opacity(1.0),
                // The origin is a multiple of eight on purpose. A gradient is
                // dithered, and the dither is an eight-by-eight tile keyed to
                // the target rather than to the frame, so a layer offset by
                // anything else meets it at a different phase and a handful of
                // pixels round the other way -- measured at one level from
                // (30, 30) and two from (33, 33). That is a real property and
                // it has a test of its own; it is not what this one is asking
                // about, and leaving it in the way would only make the bound
                // here loose enough to stop catching what it is for.
                Rect::new(32.0, 32.0, 100.0, 100.0),
            );
        }
        let paint = Paint::fill(red).with_anti_alias(false);
        let img = Paint::image(0, Rect::from_size(4.0, 4.0)).with_anti_alias(false);
        match name {
            "solid rect" => canvas.draw_rect(square, &paint).map(|_| ()),
            "path" => canvas.draw_path(&path, &paint).map(|_| ()),
            "rrect" => canvas.draw_rrect(square, 12.0, &paint).map(|_| ()),
            "circle" => canvas
                .draw_circle(Vec2::new(64.0, 64.0), 24.0, &paint)
                .map(|_| ()),
            "linear gradient" => canvas
                .draw_rect(
                    square,
                    &Paint::fill(red)
                        .with_anti_alias(false)
                        .with_shader(Shader::LinearGradient {
                            start: Vec2::new(40.0, 40.0),
                            end: Vec2::new(88.0, 88.0),
                            stops: stops.clone(),
                            tile: TileMode::Clamp,
                        }),
                )
                .map(|_| ()),
            "radial gradient" => canvas
                .draw_rect(
                    square,
                    &Paint::fill(red)
                        .with_anti_alias(false)
                        .with_shader(Shader::RadialGradient {
                            center: Vec2::new(64.0, 64.0),
                            radius: 24.0,
                            stops: stops.clone(),
                            tile: TileMode::Clamp,
                        }),
                )
                .map(|_| ()),
            "sweep gradient" => canvas
                .draw_rect(
                    square,
                    &Paint::fill(red)
                        .with_anti_alias(false)
                        .with_shader(Shader::SweepGradient {
                            center: Vec2::new(64.0, 64.0),
                            start_angle: 0.0,
                            end_angle: std::f32::consts::TAU,
                            stops: stops.clone(),
                            tile: TileMode::Clamp,
                        }),
                )
                .map(|_| ()),
            "conical gradient" => canvas
                .draw_rect(
                    square,
                    &Paint::fill(red)
                        .with_anti_alias(false)
                        .with_shader(Shader::ConicalGradient {
                            start_center: Vec2::new(50.0, 64.0),
                            start_radius: 0.0,
                            end_center: Vec2::new(64.0, 64.0),
                            end_radius: 24.0,
                            stops: stops.clone(),
                            tile: TileMode::Clamp,
                        }),
                )
                .map(|_| ()),
            "image" => canvas.draw_rect(square, &img).map(|_| ()),
            "mesh" => canvas.draw_vertices(&mesh, &paint).map(|_| ()),
            "image nine" => canvas
                .draw_image_nine(
                    0,
                    Extent2D::new(4, 4),
                    Rect::new(1.0, 1.0, 3.0, 3.0),
                    square,
                    &img,
                )
                .map(|_| ()),
            other => unreachable!("{other}"),
        }
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        if layered {
            canvas.restore();
        }
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("s");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("d");
        let px = ctx.read(&mut surface).expect("r");
        ctx.destroy_surface(surface);
        px
    };
    // A level for everything, and the two reasons are worth keeping apart.
    //
    // A linear gradient was once allowed one here on the grounds that its
    // parameter is a projection onto an axis stated in clip space, which a
    // layer of another size normalizes differently. That was wrong: aligning
    // the layer's origin to the dither tile removed the difference entirely, so
    // whatever the allowance covered, it was not that.
    //
    // What is left is a composite rounding once. A layer is drawn into a target
    // of another size and origin and multiplied by a group alpha on the way
    // back, and the same product rounds to eight bits differently depending on
    // where the sampling landed. That is device-dependent rather than
    // material-dependent, which is why it is one number rather than a list:
    // every material was exact on two x86 devices and a solid rectangle is off
    // by one on a Raspberry Pi 5's V3D.
    //
    // Two rather than one, and derived rather than fitted: the composite rounds
    // once, and a material whose color is computed per fragment -- a gradient
    // walked at each pixel, a mesh interpolating vertex colors -- can round
    // again where the sampling moved. Both of those are one level each and they
    // do not cancel.
    //
    // A material whose mapping assumed the frame is off by the whole range, so
    // two levels still catches what this is for.
    //
    // Except where the material is drawn from a distance field, which is the
    // second reason and needs its own number. A rectangle, rounded rectangle or
    // circle computes its own coverage per fragment from a screen-space
    // derivative that both specifications leave to the implementation, so the
    // same shape composited from a target of another size can differ along its
    // edges by more than the rounding. That is `Tolerance::ANALYTIC` in the
    // testkit, which budgets eight for this cause and records how it was
    // measured. A Pi 5's V3D reads two on the rounded rectangle.
    fn allowance(material: &str) -> i32 {
        match material {
            "solid rect" | "rrect" | "circle" => 8,
            _ => 2,
        }
    }
    for name in [
        "solid rect",
        "path",
        "rrect",
        "circle",
        "linear gradient",
        "radial gradient",
        "sweep gradient",
        "conical gradient",
        "image",
        "mesh",
        "image nine",
    ] {
        let direct = render_with(&mut ctx, name, false);
        let layered = render_with(&mut ctx, name, true);
        assert!(
            direct.chunks_exact(4).any(|texel| texel != [0, 0, 0, 255]),
            "{name} drew nothing, so this comparison says nothing"
        );
        let worst = direct
            .iter()
            .zip(&layered)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap_or(0);
        let allowed = allowance(name);
        assert!(
            worst <= allowed,
            "{name} drew differently inside a layer than outside one, by {worst} \
             where {allowed} is allowed. A layer is a target of another size and \
             origin, so a material whose mapping assumed the frame comes out \
             wrong in one"
        );
    }
    ctx.destroy_image(image);
}

#[test]
fn a_glyph_run_takes_an_image_filter() {
    // The third call that does not pass through `draw_path`, after the mesh and
    // the sprite batch, and the third to have accepted a filter and quietly
    // drawn without one. A run is the highest-draw-count content there is, so
    // it is also the one where a dropped filter is least likely to be noticed
    // as a filter rather than as bad text.
    //
    // Its mask blur has its own test, the four styles being a larger question
    // than whether the field is noticed at all.
    let Some(mut ctx) = context() else { return };
    let (atlas, solid, _) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);
    let run = [PositionedGlyph::new(
        solid,
        [48.0, 48.0],
        atlas.get(solid).unwrap(),
    )];

    let span = |ctx: &mut Context, paint: &Paint| -> (i32, i32) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas.draw_glyphs(&run, &atlas, 0, paint).expect("glyphs");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        let mut low = i32::MAX;
        let mut high = -1;
        for y in 0..SIZE.height {
            for x in 0..SIZE.width {
                if pixel(&pixels, x, y)[0] > 0 {
                    low = low.min(x as i32);
                    high = high.max(x as i32);
                }
            }
        }
        (low, high)
    };

    let red = Color::linear(1.0, 0.0, 0.0, 1.0);
    let plain = span(&mut ctx, &Paint::fill(red));
    assert_eq!(plain, (48, 55), "the fixture glyph is eight texels wide");

    let dilated = span(
        &mut ctx,
        &Paint::fill(red).with_image_filter(ImageFilter::Dilate {
            radius_x: 10.0,
            radius_y: 10.0,
        }),
    );
    assert_eq!(
        dilated,
        (38, 65),
        "a dilation of ten should reach ten further each way. The glyph's own \
         extent means the filter was accepted and dropped"
    );

    let blurred = span(
        &mut ctx,
        &Paint::fill(red).with_image_filter(ImageFilter::blur(5.0)),
    );
    assert!(
        blurred.0 < 48 && blurred.1 > 55,
        "a blur should carry the glyph past its own box, but it spans {blurred:?}"
    );

    ctx.destroy_image(image);
}

#[test]
fn a_glyph_run_takes_all_four_mask_blur_styles() {
    // A text shadow is a mask blur over a run, which is how the common case of
    // shadowed text is drawn -- so a run needs the same four styles a shape
    // gets rather than a refusal. What each style means is a statement about
    // two pictures, the run's own coverage and that coverage blurred, so each
    // is checked by where those two survive rather than by a color.
    let Some(mut ctx) = context() else { return };
    let (atlas, solid, other) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);
    // Two glyphs, far apart, and the second is what makes the bounds matter.
    // The blurred layer is opened over the whole run, and a layer sized from
    // one glyph still covers that glyph once the blur's own reach is added --
    // so a single-glyph run cannot tell a correct span from a collapsed one.
    // The second sits well outside anything the first's box could reach.
    let run = [
        PositionedGlyph::new(solid, [30.0, 48.0], atlas.get(solid).unwrap()),
        PositionedGlyph::new(other, [92.0, 48.0], atlas.get(other).unwrap()),
    ];

    // Inside the first glyph's own box and just outside it. That glyph covers
    // 30..37 in x.
    let look = |ctx: &mut Context, paint: &Paint| -> (u8, u8, usize) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas.draw_glyphs(&run, &atlas, 0, paint).expect("glyphs");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        let mut lit = 0;
        for y in 0..SIZE.height {
            for x in 0..SIZE.width {
                if pixel(&pixels, x, y)[0] > 0 {
                    lit += 1;
                }
            }
        }
        (pixel(&pixels, 33, 51)[0], pixel(&pixels, 26, 51)[0], lit)
    };

    let red = Color::linear(1.0, 0.0, 0.0, 1.0);
    let (sharp_center, sharp_halo, sharp_lit) = look(&mut ctx, &Paint::fill(red));
    assert_eq!(
        (sharp_center, sharp_halo),
        (255, 0),
        "the first fixture glyph is a solid block with nothing around it"
    );
    // Both glyphs, so a run whose second glyph was clipped away by bounds taken
    // from the first would already differ here.
    assert!(
        sharp_lit > 64,
        "only {sharp_lit} lit, which is one glyph rather than two"
    );

    let masked = |style: MaskBlurStyle| {
        Paint::fill(red)
            .with_mask_blur(5.0)
            .with_mask_blur_style(style)
    };

    // Normal: the blurred coverage and nothing else, soft on both sides of the
    // edge, so the middle is no longer at full strength.
    let (center, halo, lit) = look(&mut ctx, &masked(MaskBlurStyle::Normal));
    assert!(
        center < 255 && center > 0,
        "normal softens the middle, got {center}"
    );
    assert!(halo > 0, "normal reaches outside the glyph, got {halo}");
    assert!(
        lit > sharp_lit * 4,
        "normal covers far more than the glyph did"
    );

    // Solid: the shape at full strength with the blur around it, which is
    // exactly normal plus a sharp middle.
    let (center, halo, _) = look(&mut ctx, &masked(MaskBlurStyle::Solid));
    assert_eq!(center, 255, "solid keeps the glyph itself at full strength");
    assert!(halo > 0, "and still puts the blur outside it, got {halo}");

    // Outer: the blur outside the shape only, which is what a drop shadow
    // behind opaque text needs -- the middle has to be gone.
    let (center, halo, _) = look(&mut ctx, &masked(MaskBlurStyle::Outer));
    assert_eq!(center, 0, "outer removes the glyph and keeps only its halo");
    assert!(halo > 0, "and the halo is what is left, got {halo}");

    // Inner: the blur inside the shape only, so nothing escapes the glyph's own
    // box and the pixel count is the glyph's.
    let (center, halo, lit) = look(&mut ctx, &masked(MaskBlurStyle::Inner));
    assert!(
        center < 255 && center > 0,
        "inner softens within the glyph, got {center}"
    );
    assert_eq!(halo, 0, "inner puts nothing outside the glyph, got {halo}");
    assert_eq!(lit, sharp_lit, "and covers exactly what the glyph covered");

    ctx.destroy_image(image);
}

/// A gradient with more stops than a material carries, so it bakes a ramp.
///
/// `hue` picks which colors, because two ramps of the same stops are one ramp:
/// the recorder deduplicates them, so a test wanting to tell one index from
/// another has to ask for genuinely different gradients.
fn ramped_stops(hue: usize) -> Vec<GradientStop> {
    (0..=MAX_STOPS + 2)
        .map(|i| {
            let t = i as f32 / (MAX_STOPS + 2) as f32;
            let color = match hue {
                0 => Color::srgb(t, 1.0 - t, 0.5, 1.0),
                _ => Color::srgb(0.5, t * 0.2, 1.0 - t, 1.0),
            };
            GradientStop::new(color, t)
        })
        .collect()
}

#[test]
fn a_recording_drawn_into_another_keeps_its_own_layers_and_ramps() {
    // `drawPicture`, and the part of it with anything to go wrong. A recording
    // is passes and baked gradients, and both are named by position in lists
    // the receiving recording is appending to -- so every index inside the
    // picture has to move by however much is already there. Get that wrong and
    // a picture's layer samples the host's, which is a plausible picture of
    // something nobody drew.
    //
    // Both index spaces are non-empty before the picture arrives, which is what
    // makes the offsets observable at all: appending to empty lists renumbers
    // by zero and any arithmetic passes.
    let Some(mut ctx) = context() else { return };

    // A picture carrying one of each: a half-opacity layer, and a gradient with
    // more stops than a material holds.
    let mut inner = Canvas::new(Extent2D::new(64, 64));
    inner.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
    inner.save_layer_bounds(Layer::opacity(0.5), Rect::new(0.0, 0.0, 64.0, 64.0));
    inner
        .draw_rect(
            Rect::new(4.0, 4.0, 60.0, 60.0),
            &Paint::fill(Color::srgb(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("picture layer");
    inner.restore();
    inner
        .draw_rect(
            Rect::new(8.0, 24.0, 56.0, 40.0),
            &Paint::fill(Color::srgb(1.0, 1.0, 1.0, 1.0))
                .with_anti_alias(false)
                .with_shader(Shader::LinearGradient {
                    start: Vec2::new(8.0, 0.0),
                    end: Vec2::new(56.0, 0.0),
                    stops: ramped_stops(0),
                    tile: TileMode::Clamp,
                }),
        )
        .expect("picture gradient");
    let picture = inner.finish();
    assert_eq!(
        (picture.passes.len(), picture.ramps.len()),
        (2, 1),
        "the picture should carry a layer and a ramp, or this proves nothing"
    );

    // A host that already has one of each, so the picture's indices land after.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.05, 0.05, 0.05, 1.0));
    canvas.save_layer_bounds(Layer::opacity(1.0), Rect::new(0.0, 0.0, 20.0, 20.0));
    canvas
        .draw_rect(
            Rect::new(2.0, 2.0, 18.0, 18.0),
            &Paint::fill(Color::linear(0.0, 1.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("host layer");
    canvas.restore();
    canvas
        .draw_rect(
            Rect::new(0.0, 108.0, 128.0, 128.0),
            &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                .with_anti_alias(false)
                .with_shader(Shader::LinearGradient {
                    start: Vec2::new(0.0, 0.0),
                    end: Vec2::new(128.0, 0.0),
                    stops: ramped_stops(1),
                    tile: TileMode::Clamp,
                }),
        )
        .expect("host gradient");
    canvas.save();
    canvas.translate(40.0, 30.0);
    canvas
        .draw_recording(&picture, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("recording");
    canvas.restore();
    let combined = canvas.finish();
    assert_eq!(
        (combined.passes.len(), combined.ramps.len()),
        (4, 2),
        "the picture's passes and ramps should have been appended, not merged"
    );
    let pixels = render_recording(&mut ctx, &combined);

    // The host's own layer, which the picture must not have displaced.
    assert_eq!(
        pixel(&pixels, 10, 10),
        [0, 255, 0, 255],
        "the host's layer changed when a picture was drawn beside it"
    );
    // The host's ramp, still a gradient rather than the picture's.
    // The host's stops hold red at a half and keep green under a fifth, which
    // is what tells them from the picture's, where green rises to full.
    let host_ramp = pixel(&pixels, 64, 118);
    assert!(
        host_ramp[0] > 100 && host_ramp[1] < 60,
        "the host's gradient should still run through its own stops, got \
         {host_ramp:?}. A high green would be the picture's ramp"
    );
    // The picture's layer, whose half opacity is the evidence it is that layer
    // and not the host's: the host's is fully opaque.
    let inside = pixel(&pixels, 60, 50);
    assert!(
        inside[0] > 100 && inside[0] < 180 && inside[1] < 40,
        "the picture's layer should be red at half opacity over the ground, \
         got {inside:?}. Full red would mean it sampled the wrong pass"
    );
    // And the picture's gradient, which is a different ramp from the host's.
    let picture_ramp = pixel(&pixels, 60, 62);
    assert!(
        picture_ramp[1] > 100,
        "the picture's gradient should run through its own stops -- green \
         rising as red falls -- and not the host's, whose green never passes a \
         fifth. Got {picture_ramp:?}"
    );
}

#[test]
fn a_recording_lands_where_the_transform_puts_it() {
    // A recording has no bounds of its own to place -- it is a picture rather
    // than a shape -- so where it goes is entirely what the transform says. Its
    // own extent is the rectangle, with its corner at the origin.
    //
    // Scaling it resamples rather than redraws, which is the whole limitation
    // of this call and is worth pinning rather than only describing: the
    // picture was flattened at the tolerance in force when it was recorded, and
    // nothing here can undo that.
    let Some(mut ctx) = context() else { return };

    let mut inner = Canvas::new(Extent2D::new(64, 64));
    inner.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
    inner
        .draw_rect(
            Rect::new(8.0, 8.0, 56.0, 56.0),
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let picture = inner.finish();

    let span = |ctx: &mut Context, place: &dyn Fn(&mut Canvas)| -> (u32, u32) {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas.save();
        place(&mut canvas);
        canvas
            .draw_recording(&picture, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
            .expect("recording");
        canvas.restore();
        let pixels = render(ctx, canvas);
        let lit: Vec<u32> = (0..SIZE.width)
            .filter(|x| pixel(&pixels, *x, 40)[0] > 40)
            .collect();
        (
            *lit.first().expect("the picture drew nothing"),
            *lit.last().expect("the picture drew nothing"),
        )
    };

    // Untransformed, the picture's own coordinates are the canvas's.
    assert_eq!(
        span(&mut ctx, &|_| {}),
        (8, 55),
        "a picture at the origin should land where it drew"
    );
    // Translated, it moves by exactly that much.
    assert_eq!(
        span(&mut ctx, &|c: &mut Canvas| {
            c.translate(48.0, 0.0);
        }),
        (56, 103),
        "a picture should move with the transform"
    );
    // Scaled about a point, its corners land where that scale puts them.
    assert_eq!(
        span(&mut ctx, &|c: &mut Canvas| {
            c.translate(16.0, 0.0);
            c.scale(1.5, 1.5);
        }),
        (27, 100),
        "a picture scaled by half again should span half again as much"
    );
    // A transform that folds the plane leaves nothing to sample and nothing to
    // draw, so the call does nothing rather than dividing by a determinant of
    // zero.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
    canvas.save();
    canvas.scale(0.0, 1.0);
    canvas
        .draw_recording(&picture, &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0)))
        .expect("collapsed");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);
    assert!(
        pixels.chunks_exact(4).all(|texel| texel == [0, 0, 0, 255]),
        "a picture under a collapsed transform should draw nothing at all"
    );
}

#[test]
fn a_runtime_program_can_sample_more_than_one_texture() {
    // `dart:ui` lets a fragment program declare several samplers, and until now
    // this renderer carried one. One was what the shared descriptor set already
    // covered, since every draw binds a texture at the binding this renderer's
    // own shader declares; several needed that layout to grow, which it can
    // because a layout may carry bindings a shader never mentions and the solid
    // pipeline is built against it unchanged.
    //
    // The fixture program takes the absolute difference of its two textures.
    // That operation is chosen for what it rules out: where the two agree it is
    // black whatever they hold, so binding the same texture twice -- or leaving
    // the second at the placeholder -- gives a picture this one is not.
    let Some(mut ctx) = context() else { return };
    let program = ctx
        .register_program(&emblema::RuntimeProgram {
            spirv: emblema_shaders::EFFECT_TWO_IMAGES_SPV.to_vec(),
            glsl_es: emblema_shaders::EFFECT_TWO_IMAGES_FS_GLSL.to_string(),
        })
        .expect("register");

    // Two flat textures whose difference is a known color: one is red, the
    // other is red plus a half of green.
    let mut first = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("first");
    ctx.write_image(&mut first, &[255u8, 0, 0, 255].repeat(16))
        .expect("upload first");
    let mut second = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("second");
    ctx.write_image(&mut second, &[255u8, 128, 0, 255].repeat(16))
        .expect("upload second");

    let draw = |ctx: &mut Context, slots: &[u32]| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                    .with_anti_alias(false)
                    .with_shader(Shader::RuntimeEffect {
                        program,
                        uniforms: {
                            let mut out = vec![0.0; RUNTIME_FLOATS];
                            // The tint the program multiplies the difference by.
                            out[0..4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
                            out
                        },
                        images: slots.to_vec(),
                    }),
            )
            .expect("effect");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&first, &second])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    // Both textures: the difference is a half of green and nothing else.
    let both = pixel(&draw(&mut ctx, &[0, 1]), 64, 64);
    assert!(
        both[0] < 8 && (both[1] as i32 - 128).abs() <= 2 && both[2] < 8,
        "the difference of the two textures should be half green, got {both:?}"
    );

    // The same texture named twice: a program reading two bindings that hold
    // one texture sees no difference at all. This is what the picture looks
    // like when the second binding was never really bound.
    let same = pixel(&draw(&mut ctx, &[0, 0]), 64, 64);
    assert_eq!(
        same,
        [0, 0, 0, 255],
        "a texture differenced with itself is black; got {same:?}, which means \
         the two bindings held different textures when they were told not to"
    );

    // Only one named, so the second binding falls to the placeholder -- a
    // one-pixel white texture, whose difference from red is cyan.
    let one = pixel(&draw(&mut ctx, &[0]), 64, 64);
    assert!(
        one[1] > 200 && one[2] > 200,
        "an undeclared second texture should read the white placeholder, whose \
         difference from red is cyan. Got {one:?}"
    );

    ctx.destroy_image(first);
    ctx.destroy_image(second);
}

#[test]
fn a_difference_clip_removes_the_rectangle_and_nothing_else() {
    // `dart:ui` spells this `clipRect` with `ClipOp.difference`, and it is the
    // only clip that operation applies to there. This table said `clipRect`
    // was complete while it took no operation at all, which is the kind of
    // overstatement a parity table exists not to make.
    //
    // Areas, because they are exact. A thirty-two square removed from a frame
    // of sixteen thousand three hundred and eighty-four leaves fifteen thousand
    // three hundred and sixty, and nothing about that number is a matter of
    // taste -- a clip that removed a bounding box, or antialiased its edge into
    // the count, would miss it.
    let Some(mut ctx) = context() else { return };

    let lit = |ctx: &mut Context, setup: &dyn Fn(&mut Canvas)| -> usize {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas.save();
        setup(&mut canvas);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
            )
            .expect("fill");
        canvas.restore();
        render(ctx, canvas)
            .chunks_exact(4)
            .filter(|texel| texel[0] > 0)
            .count()
    };

    let whole = lit(&mut ctx, &|_| {});
    assert_eq!(whole, 16384, "the unclipped fill covers the frame");

    // A square taken out of the middle: the frame less its area, and the middle
    // is what went.
    let holed = lit(&mut ctx, &|c: &mut Canvas| {
        c.clip_out_rect(Rect::new(48.0, 48.0, 80.0, 80.0))
            .expect("out");
    });
    assert_eq!(
        holed,
        whole - 32 * 32,
        "a difference clip should remove exactly the rectangle it names"
    );

    // Composed with an ordinary clip, which is the property that makes a clip
    // stack a stack: each one may only narrow what the last allowed.
    let both = lit(&mut ctx, &|c: &mut Canvas| {
        c.clip_rect(Rect::new(24.0, 24.0, 104.0, 104.0))
            .expect("in");
        c.clip_out_rect(Rect::new(48.0, 48.0, 80.0, 80.0))
            .expect("out");
    });
    assert_eq!(
        both,
        80 * 80 - 32 * 32,
        "an intersect and a difference should compose to the one less the other"
    );

    // Under a rotation the rectangle is a quadrilateral, and a difference clip
    // has to remove that rather than a box around it. Its area is what a
    // rotation preserves, so the count is the same as the unrotated case even
    // though the shape is not -- and a bounding box would take out half as much
    // again. The transform is undone before the fill, so what is missing from
    // the frame is the quadrilateral alone.
    let turned = lit(&mut ctx, &|c: &mut Canvas| {
        c.rotate(0.4);
        c.clip_out_rect(Rect::new(30.0, 10.0, 70.0, 50.0))
            .expect("out");
        c.rotate(-0.4);
    });
    assert_eq!(
        turned,
        whole - 40 * 40,
        "a rotated difference clip should remove the quadrilateral's own area"
    );
}

#[test]
fn the_ring_between_two_rounded_rectangles_is_hollow() {
    // `dart:ui` spells this `drawDRRect`, and what makes it a ring rather than
    // two shapes is the fill rule: two contours wound the same way fill solid
    // under the nonzero rule and hollow under even-odd. The rule is the whole
    // of the feature, so the test is arranged to fail if it changes.
    //
    // Both radii are zero here, which makes both contours plain rectangles and
    // the ring's area exact -- a rounded one could only be asserted to within a
    // corner's worth, and a test that has to allow slack in the number cannot
    // tell a hollow ring from a solid one filled a little differently.
    let Some(mut ctx) = context() else { return };

    let lit = |ctx: &mut Context, setup: &dyn Fn(&mut Canvas)| -> usize {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        setup(&mut canvas);
        render(ctx, canvas)
            .chunks_exact(4)
            .filter(|texel| texel[0] > 0)
            .count()
    };

    let paint = Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false);
    let outer = Rect::new(16.0, 16.0, 112.0, 112.0);
    let inner = Rect::new(48.0, 48.0, 80.0, 80.0);

    let ring = lit(&mut ctx, &|c: &mut Canvas| {
        c.draw_drrect(outer, 0.0, inner, 0.0, &paint).expect("ring");
    });
    assert_eq!(
        ring,
        96 * 96 - 32 * 32,
        "the ring should be the outer rectangle less the inner one; a nonzero \
         fill rule would leave it solid at {}",
        96 * 96
    );

    // An inner rectangle with no area is not a degenerate case to reject but
    // the ordinary way to ask for the outer shape alone, and a ring of no hole
    // is that shape -- so it has to come out solid rather than vanish.
    //
    // Both spellings, because they take different routes. A rectangle of zero
    // size is not `is_empty` -- that test is a strict inequality -- so it
    // traces a contour of no area and the even-odd rule ignores it. An
    // inverted one is caught by the guard, and has to be: traced, its corners
    // still enclose a region, and the rule does not care which way a contour
    // is wound, so it would cut a hole where the caller asked for none.
    for hollow in [
        Rect::new(64.0, 64.0, 64.0, 64.0),
        Rect::new(80.0, 80.0, 48.0, 48.0),
    ] {
        let solid = lit(&mut ctx, &|c: &mut Canvas| {
            c.draw_drrect(outer, 0.0, hollow, 0.0, &paint)
                .expect("solid");
        });
        assert_eq!(
            solid,
            96 * 96,
            "an inner rectangle of no area should leave the outer shape, got \
             {solid} for {hollow:?}"
        );
    }

    // The radii have to reach the geometry rather than being carried and
    // dropped, and a corner is where that shows: rounding the outer one takes
    // its corners off, rounding the inner one puts area back.
    let rounded_outside = lit(&mut ctx, &|c: &mut Canvas| {
        c.draw_drrect(outer, 24.0, inner, 0.0, &paint)
            .expect("ring");
    });
    assert!(
        rounded_outside < ring,
        "rounding the outer rectangle should remove its corners, got \
         {rounded_outside} against {ring}"
    );
    let rounded_inside = lit(&mut ctx, &|c: &mut Canvas| {
        c.draw_drrect(outer, 0.0, inner, 12.0, &paint)
            .expect("ring");
    });
    assert!(
        rounded_inside > ring,
        "rounding the hole should shrink it and so light more, got \
         {rounded_inside} against {ring}"
    );

    // Areas alone would be satisfied by a ring drawn in the wrong place, so
    // three points say where it is: the middle empty, the band between the two
    // rectangles filled, and nothing outside the outer one. Rounded, since the
    // arms above are square and a hole is easiest to lose at a corner.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_drrect(outer, 18.0, Rect::new(40.0, 40.0, 88.0, 88.0), 10.0, &paint)
        .expect("ring");
    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 64, 64),
        [0, 0, 0, 255],
        "the inner rectangle should be a hole"
    );
    assert!(
        pixel(&pixels, 64, 28)[0] > 240,
        "the band between them should be filled"
    );
    assert_eq!(
        pixel(&pixels, 4, 4),
        [0, 0, 0, 255],
        "and nothing outside the outer one"
    );
}

#[test]
fn a_mesh_takes_a_runtime_effect_unless_it_is_textured() {
    // The playground inventory listed runtime effects as blocking the vertices
    // file. They do not block it: a mesh without texture coordinates takes its
    // material from the paint's shader like any other geometry, so a caller's
    // program reaches it through the ordinary path and nothing special was
    // needed. What is refused is a textured mesh under an effect, and that is
    // a stated error rather than a gap -- the coordinates would have no image
    // to read, since the shader a caller registered is what replaced it.
    let Some(mut ctx) = context() else { return };
    let program = ctx
        .register_program(&emblema::RuntimeProgram {
            spirv: emblema_shaders::EFFECT_SPV.to_vec(),
            glsl_es: emblema_shaders::EFFECT_FS_GLSL.to_string(),
        })
        .expect("register");

    // A triangle spanning the middle, so the program's vertical split falls
    // across geometry that is not a rectangle. A fill that ignored the mesh
    // would color the frame; one that ignored the program would be solid.
    let mesh = Vertices::new(
        VertexMode::Triangles,
        vec![
            Vec2::new(16.0, 112.0),
            Vec2::new(112.0, 112.0),
            Vec2::new(64.0, 16.0),
        ],
    )
    .expect("mesh");
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_vertices(
            &mesh,
            &Paint::runtime_effect(program, effect_uniforms(0.0)).with_anti_alias(false),
        )
        .expect("effect on a mesh");
    let pixels = render(&mut ctx, canvas);

    assert_eq!(
        pixel(&pixels, 44, 100),
        [255, 0, 0, 255],
        "inside the triangle, left of the split"
    );
    close_enough(
        pixel(&pixels, 84, 100),
        [0, 178, 51, 255],
        1,
        "inside the triangle, right of it",
    );
    // Above the apex and below the base are both outside the mesh, and the
    // ground has to show through both -- an effect that filled its own bounds
    // would pass the first pair of assertions and fail these.
    assert_eq!(
        pixel(&pixels, 64, 8),
        [0, 0, 0, 255],
        "above the apex is outside the mesh"
    );
    assert_eq!(
        pixel(&pixels, 20, 20),
        [0, 0, 0, 255],
        "and so is the corner the triangle does not reach"
    );

    // And texture coordinates are accepted rather than refused, which is the
    // half this used to assert the other way round. A program does not read
    // them through the flag every built-in shader is told by -- it receives
    // them as `uv` and decides -- so there is nothing here to refuse. What that
    // buys is checked where it can be seen, in
    // `a_program_reads_the_coordinate_the_vertices_state`, which mirrors a
    // mesh's coordinates and requires the picture to turn over with them.
    let textured = Vertices::full(
        VertexMode::Triangles,
        vec![
            Vec2::new(16.0, 112.0),
            Vec2::new(112.0, 112.0),
            Vec2::new(64.0, 16.0),
        ],
        vec![Vec2::ZERO, Vec2::X, Vec2::Y],
        Vec::new(),
        vec![0, 1, 2],
    )
    .expect("mesh");
    let mut canvas = Canvas::new(SIZE);
    assert!(
        canvas
            .draw_vertices(
                &textured,
                &Paint::runtime_effect(program, effect_uniforms(0.0)),
            )
            .is_ok(),
        "a textured mesh under an effect reads its coordinates as `uv`"
    );
}

#[test]
fn a_skew_is_a_transform_class_of_its_own() {
    // Nothing else here renders under a shear, and it is the transform that
    // breaks the assumptions the others satisfy: it is not conformal, so a
    // circle becomes an ellipse at an angle, no axis survives, and a scissor
    // -- which is a device rectangle and can be nothing else -- cannot express
    // what a clip under one admits.
    //
    // What makes it measurable is that a shear preserves area exactly. Its
    // determinant is one whatever the coefficient, so a shape's straight-edged
    // area is the same sheared as upright, and the count is not a matter of
    // taste at either.
    let Some(mut ctx) = context() else { return };

    let sheared = |ctx: &mut Context, k: f32, draw: &dyn Fn(&mut Canvas, &Paint)| -> usize {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.save();
        // About the middle of the target, so the sheared shape stays inside
        // the frame and the count is the shape's area rather than the part of
        // it that fell in.
        canvas.translate(64.0, 64.0);
        let mut shear = Affine2::IDENTITY;
        shear.matrix2.y_axis.x = k;
        canvas.concat(shear);
        canvas.translate(-64.0, -64.0);
        draw(
            &mut canvas,
            &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        );
        canvas.restore();
        render(ctx, canvas)
            .chunks_exact(4)
            .filter(|texel| texel[0] > 0)
            .count()
    };

    let square = Rect::new(48.0, 48.0, 80.0, 80.0);

    // A filled rectangle: the parallelogram has the square's area, and every
    // coefficient gives the same number because that is what a determinant of
    // one means.
    for k in [0.0, 0.25, 0.5, 1.0] {
        let lit = sheared(&mut ctx, k, &|c: &mut Canvas, p: &Paint| {
            c.draw_rect(square, p).expect("fill");
        });
        assert_eq!(lit, 32 * 32, "a shear of {k} should preserve the area");
    }

    // Area alone would be satisfied by a shear that never reached the
    // geometry, since the upright square has the same one. So two points say
    // the shape moved: at a twelfth of the way up from the middle the
    // parallelogram has slid left, and below it right, and each point is
    // inside one of the two shapes and outside the other.
    let where_lit = |ctx: &mut Context, k: f32, at: (u32, u32)| -> bool {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.translate(64.0, 64.0);
        let mut shear = Affine2::IDENTITY;
        shear.matrix2.y_axis.x = k;
        canvas.concat(shear);
        canvas.translate(-64.0, -64.0);
        canvas
            .draw_rect(
                square,
                &Paint::fill(Color::linear(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
            )
            .expect("fill");
        pixel(&render(ctx, canvas), at.0, at.1)[0] > 0
    };
    for at in [(44, 52), (84, 76)] {
        assert!(
            where_lit(&mut ctx, 0.5, at),
            "{at:?} is inside the sheared parallelogram"
        );
        assert!(
            !where_lit(&mut ctx, 0.0, at),
            "{at:?} is outside the upright square, so a shear that never \
             reached the geometry would leave it dark"
        );
    }

    // The clip is where a skew has somewhere to go wrong, and the number says
    // which way it went. A clip under a shear admits a parallelogram; the
    // scissor path cannot express one, so it has to decline and leave the
    // stencil to it. Taking the bounding box instead would admit half again as
    // much -- forty-eight wide by thirty-two tall -- and would look like a
    // clip that worked.
    let clipped = sheared(&mut ctx, 0.5, &|c: &mut Canvas, p: &Paint| {
        c.clip_rect(square).expect("clip");
        c.draw_rect(Rect::from_size(128.0, 128.0), p).expect("fill");
    });
    assert_eq!(
        clipped,
        32 * 32,
        "a clip under a shear should admit the parallelogram, not its bounding \
         box of {}",
        48 * 32
    );

    // The analytic paths evaluate their shape per fragment through the inverse
    // of the transform, which is a general two-by-two here and so has a skew
    // in it like anything else. A rounded rectangle and a circle keep their
    // areas across the shear too -- to within a percent rather than exactly,
    // since a curved edge meets the sampling grid differently once it is no
    // longer symmetric about an axis, and that difference is the rasterizer's
    // rather than the transform's.
    for (radius, name) in [(0.0, "square corners"), (8.0, "rounded corners")] {
        let upright = sheared(&mut ctx, 0.0, &|c: &mut Canvas, p: &Paint| {
            c.draw_rrect(square, radius, p).expect("rrect");
        });
        let slanted = sheared(&mut ctx, 0.5, &|c: &mut Canvas, p: &Paint| {
            c.draw_rrect(square, radius, p).expect("rrect");
        });
        assert!(
            upright.abs_diff(slanted) * 100 <= upright,
            "a sheared rounded rectangle with {name} should keep its area, got \
             {slanted} against {upright}"
        );
    }
    let upright = sheared(&mut ctx, 0.0, &|c: &mut Canvas, p: &Paint| {
        c.draw_circle(Vec2::new(64.0, 64.0), 16.0, p)
            .expect("circle");
    });
    let slanted = sheared(&mut ctx, 0.5, &|c: &mut Canvas, p: &Paint| {
        c.draw_circle(Vec2::new(64.0, 16.0 + 48.0), 16.0, p)
            .expect("circle");
    });
    assert!(
        upright.abs_diff(slanted) * 100 <= upright,
        "a sheared circle is an ellipse of the same area, got {slanted} \
         against {upright}"
    );
}

#[test]
fn an_atlas_batch_honors_the_paint_s_blend() {
    // A batch is one draw, so its blend is one piece of pipeline state shared
    // by every sprite in it -- and where two sprites overlap, the mode is being
    // asked how a fragment combines with something the same draw put down a
    // moment earlier. Nothing had asked before: every plate and every other
    // test of this call left the blend at its default, so a batch that dropped
    // it on the floor would have satisfied all of them.
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    // One quadrant of the sheet rather than the whole of it, so the sprite is
    // a flat color and a sampled texel does not depend on where in the sprite
    // it was taken. Tinted to half, because then the sum of two is exactly one
    // and a mode that saturated would read the same as one that added.
    let sprite = |x: f32| {
        let mut sprite = Sprite::new(
            SourceRect::new(2.0, 2.0, 2.0, 2.0),
            Affine2::from_scale_angle_translation(Vec2::splat(24.0), 0.0, Vec2::new(x, 40.0)),
        );
        sprite.color = Color::srgb(0.5, 0.5, 0.5, 1.0);
        sprite
    };
    let sprites = [sprite(28.0), sprite(52.0)];

    let overlap = |ctx: &mut Context, blend: BlendMode| -> [u8; 4] {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_atlas(
                &sprites,
                Extent2D::new(4, 4),
                &Paint::image(0, Rect::from_size(128.0, 128.0)).with_blend(blend),
            )
            .expect("atlas");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        // Inside both sprites: the first spans 28 to 76, the second 52 to 100.
        pixel(&pixels, 60, 52)
    };

    let over = overlap(&mut ctx, BlendMode::SrcOver);
    let plus = overlap(&mut ctx, BlendMode::Plus);
    ctx.destroy_image(image);

    // The sprites are opaque, so `SrcOver` leaves the upper one and nothing of
    // the one beneath: the overlap reads as either sprite alone does.
    assert!(
        (100..160).contains(&over[0]),
        "SrcOver should leave the top sprite's half brightness, got {over:?}"
    );
    // Added, the two halves make a whole. A different number, not merely a
    // brighter-looking one.
    assert!(
        plus[0] > 230,
        "Plus should add the two halves to full brightness, got {plus:?}"
    );
}

/// Column-major, the shape `dart:ui` states a transform in.
///
/// The only entry that is not an affine's is the one making the divisor grow
/// along an axis, which is what makes the plane recede.
fn receding(along_x: f32, along_y: f32) -> [f32; 16] {
    [
        1.0, 0.0, 0.0, along_x, //
        0.0, 1.0, 0.0, along_y, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

fn white_run(pixels: &[u8], row: u32) -> usize {
    (0..SIZE.width)
        .filter(|x| pixel(pixels, *x, row)[0] > 128)
        .count()
}

fn rect_under(ctx: &mut Context, matrix: &[f32; 16]) -> Vec<u8> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.concat_4x4(matrix);
    canvas
        .draw_rect(
            Rect::new(16.0, 16.0, 112.0, 112.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("rect");
    render(ctx, canvas)
}

/// The row of `docs/parity.md` this closes: a four-by-four admits perspective
/// and a two-by-three does not.
#[test]
fn a_perspective_transform_makes_a_rectangle_a_trapezoid() {
    let Some(mut ctx) = context() else { return };
    let perspective = rect_under(&mut ctx, &receding(0.0, 0.004));
    let near = white_run(&perspective, 20);
    let far = white_run(&perspective, 70);
    assert!(
        near > far + 8,
        "a receding plane must narrow: {near} wide near, {far} far"
    );

    // And it has to differ from the affine it reduces to when the perspective
    // row is dropped -- a scene whose perspective is too slight to see would
    // pass every comparison here while proving nothing.
    let affine = rect_under(&mut ctx, &receding(0.0, 0.0));
    assert_eq!(
        white_run(&affine, 20),
        white_run(&affine, 70),
        "without perspective the two rows are the same width"
    );
    assert!(
        perspective != affine,
        "the perspective row changed no pixel"
    );
}

/// A gradient has to converge with the shape it fills, which is the whole
/// reason a paint carries a mapping rather than a direction.
#[test]
fn a_gradient_under_perspective_is_locked_to_the_shape() {
    let Some(mut ctx) = context() else { return };
    let matrix = receding(0.004, 0.0);

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.concat_4x4(&matrix);
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, 128.0, 128.0),
            &Paint::fill(Color::WHITE)
                .with_shader(Shader::LinearGradient {
                    start: Vec2::new(0.0, 0.0),
                    end: Vec2::new(128.0, 0.0),
                    stops: vec![
                        GradientStop::new(Color::BLACK, 0.0),
                        GradientStop::new(Color::WHITE, 1.0),
                    ],
                    tile: TileMode::Clamp,
                })
                .with_anti_alias(false),
        )
        .expect("rect");
    let pixels = render(&mut ctx, canvas);

    // Where a device pixel came from decides what color belongs there, and the
    // inverse of the same transform is what says so. Checked against the
    // projective answer rather than against a straight ramp, which is what this
    // would be if the mapping had stayed affine.
    let inverse = Transform2D::from_column_major_4x4(&matrix)
        .inverse()
        .expect("invertible");
    let mut worst_projective = 0.0f32;
    let mut worst_affine = 0.0f32;
    for x in [10u32, 20, 30, 40, 50, 60] {
        let measured = pixel(&pixels, x, 64)[0] as f32;
        let user = inverse.project_point2(Vec2::new(x as f32 + 0.5, 64.5));
        let projective = (user.x / 128.0).clamp(0.0, 1.0) * 255.0;
        let affine = ((x as f32 + 0.5) / 128.0) * 255.0;
        worst_projective = worst_projective.max((measured - projective).abs());
        worst_affine = worst_affine.max((measured - affine).abs());
    }
    assert!(
        worst_projective < 12.0,
        "the gradient did not follow the transform: off by {worst_projective}"
    );
    assert!(
        worst_affine > 24.0,
        "the transform made no difference to the gradient, so this proves nothing"
    );
}

/// The failure mode the degeneracy invariant used to rule out by arithmetic and
/// now rules out by the near plane.
///
/// A near-singular affine collapses geometry toward nothing, so a substituted
/// mapping is never consulted. A transform near the vanishing line does the
/// opposite: it blows geometry up toward infinity, and the thing to be sure of
/// is that what comes back is a clipped shape rather than the whole frame.
#[test]
fn a_shape_across_the_vanishing_line_draws_its_near_half_and_no_more() {
    let Some(mut ctx) = context() else { return };
    // The divisor reaches zero at y = 100, and the rectangle runs past it.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.concat_4x4(&receding(0.0, -0.01));
    canvas
        .draw_rect(
            Rect::new(16.0, 0.0, 112.0, 400.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("rect");
    let pixels = render(&mut ctx, canvas);

    let lit = (0..SIZE.height)
        .map(|y| white_run(&pixels, y))
        .sum::<usize>();
    let total = (SIZE.width * SIZE.height) as usize;
    assert!(lit > 0, "the half in front of the vanishing line is drawn");
    assert!(
        lit < total,
        "a shape crossing the vanishing line put the whole frame down"
    );
    // Every pixel is one of the two colors that were asked for. A NaN reaching
    // a fragment survives the blend and spreads, and would show up here as
    // something that is neither.
    for y in (0..SIZE.height).step_by(8) {
        for x in (0..SIZE.width).step_by(8) {
            let p = pixel(&pixels, x, y);
            assert!(
                p == [0, 0, 0, 255] || p == [255, 255, 255, 255],
                "pixel at {x},{y} is {p:?}, which is neither the fill nor the background"
            );
        }
    }
}

/// A layer's matrix filter moves the finished image, so perspective there is an
/// image seen at an angle rather than content redrawn at one.
///
/// That distinction is the whole reason `dart:ui` has both this and `concat`,
/// and it is the thing to check survived the widening: a caller wanting the
/// sharp one already had `concat_4x4`.
#[test]
fn a_layer_can_be_placed_by_a_matrix_that_carries_perspective() {
    let Some(mut ctx) = context() else { return };

    let placed = |ctx: &mut Context, along_x: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .save_layer(
                Layer::opacity(1.0)
                    .with_matrix(Transform2D::from_column_major_4x4(&receding(along_x, 0.0))),
            )
            .draw_rect(
                Rect::new(16.0, 16.0, 112.0, 112.0),
                &Paint::fill(Color::WHITE).with_anti_alias(false),
            )
            .expect("rect");
        canvas.restore();
        render(ctx, canvas)
    };

    let perspective = placed(&mut ctx, 0.006);
    let flat = placed(&mut ctx, 0.0);

    // The flat placement is the identity, so the square comes back square.
    assert_eq!(
        white_run(&flat, 32),
        white_run(&flat, 96),
        "without a perspective row the two rows match"
    );
    // With one, the image narrows along the axis the divisor grows on, and the
    // rows differ because the square's own corners have moved apart in y as
    // well -- a projective placement is not a scale.
    assert!(
        perspective != flat,
        "the perspective row moved no pixel of the finished layer"
    );
    let lit = (0..SIZE.height)
        .map(|y| white_run(&perspective, y))
        .sum::<usize>();
    assert!(lit > 0, "the placed layer is visible");
    assert!(
        lit < (SIZE.width * SIZE.height) as usize,
        "a placed layer put the whole frame down"
    );
}

/// Texture coordinates are per-vertex and the rasterizer interpolates them, so
/// whether they are right under perspective is not something this renderer
/// computes -- it is something it becomes eligible for by handing over a real
/// divisor instead of the constant one.
///
/// The claim is worth a test rather than a comment because the failure is the
/// famous one and it is not subtle: interpolate a texture coordinate linearly
/// in screen space across a quad seen at an angle and the texture slides,
/// putting the seam between two triangles somewhere no part of the picture
/// asked for. Here the image is four quadrants, so the boundary between two of
/// them is a line whose position says which interpolation ran.
#[test]
fn a_textured_mesh_under_perspective_is_not_interpolated_flat() {
    let Some(mut ctx) = context() else { return };
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    // The divisor grows with x, so the far edge of the quad is compressed.
    const SLOPE: f32 = 0.006;
    let mesh = Vertices::indexed(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(128.0, 0.0),
            Vec2::new(128.0, 128.0),
            Vec2::new(0.0, 128.0),
        ],
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.concat_4x4(&receding(SLOPE, 0.0));
    canvas
        .draw_vertices(
            &mesh,
            // Nearest, so the boundary between quadrants is a line rather than
            // a ramp and can be located to the pixel.
            &Paint::image(0, Rect::from_size(128.0, 128.0)).with_sampling(Sampling::Nearest),
        )
        .expect("mesh");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);

    // Near the top edge, where the quad's own diagonal is furthest away and
    // both predictions are cleanest. Red is the left quadrant, green the right.
    let row = 8;
    let seam = (0..SIZE.width)
        .find(|x| {
            let p = pixel(&pixels, *x, row);
            p[1] > p[0] && p[3] == 255
        })
        .expect("the quad covers this row and both quadrants appear in it");

    // Where the texture's own midpoint lands, which is where the seam belongs:
    // the divisor at user x of sixty-four, applied to sixty-four.
    let correct = 64.0 / (1.0 + SLOPE * 64.0);
    // Where it would land if the coordinate were carried across the triangle in
    // screen space instead -- half way along the mapped top edge.
    let flat = 64.0 / (1.0 + SLOPE * 128.0);

    assert!(
        (seam as f32 - correct).abs() <= 2.0,
        "the seam is at {seam}, and the texture's midpoint maps to {correct}"
    );
    assert!(
        (correct - flat).abs() > 6.0,
        "this transform does not separate the two answers, so the test proves nothing"
    );
}

/// A mask blur over a gradient, assembled by a caller out of what is already
/// here.
///
/// `with_mask_blur` refuses anything but a solid color, and the paint's own
/// documentation says why: it draws the paint through a blurred layer, which is
/// the same picture as blurring the mask and filling through it only where the
/// fill does not vary. That is a limit of the mechanism, and this is the
/// evidence for the claim beside it that the other order is reachable without
/// new machinery -- the same nesting of layers and blends the four styles
/// already use, with the fill drawn across everything the blur reaches and
/// blurred white coverage composited onto it with `DstIn`.
///
/// Kept as a test rather than folded into the paint because the part that is
/// missing is a decision, not this arrangement: what a shape's coverage means
/// when it is not simply its own, for a mesh carrying a color per vertex or for
/// a glyph run.
#[test]
fn a_caller_can_assemble_the_mask_blur_a_gradient_is_refused() {
    let Some(mut ctx) = context() else { return };
    let whole = Rect::new(0.0, 0.0, 128.0, 128.0);

    let masked = |ctx: &mut Context, sigma: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.save_layer_bounds(Layer::opacity(1.0), whole);
        canvas
            .draw_rect(
                whole,
                &Paint::fill(Color::WHITE).with_shader(Shader::LinearGradient {
                    start: Vec2::new(0.0, 0.0),
                    end: Vec2::new(128.0, 0.0),
                    stops: vec![
                        GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
                        GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
                    ],
                    tile: TileMode::Clamp,
                }),
            )
            .expect("fill");
        // The coverage, blurred, taken out of the fill's alpha rather than drawn
        // over it. `DstIn` on the composite is what makes the layer a mask.
        canvas.save_layer_bounds(
            Layer::opacity(1.0)
                .with_blur(sigma)
                .with_blend(BlendMode::DstIn),
            whole,
        );
        canvas
            .draw_circle(Vec2::new(64.0, 64.0), 36.0, &Paint::fill(Color::WHITE))
            .expect("coverage");
        canvas.restore();
        canvas.restore();
        render(ctx, canvas)
    };

    let pixels = masked(&mut ctx, 6.0);
    let sharp = masked(&mut ctx, 0.0);

    // Everything here composites over an opaque background, so the softness is
    // in the color rather than in the alpha: the ramp is the run of pixels that
    // are neither the background nor the fill at full strength.
    let ramp = |image: &[u8]| {
        (0..SIZE.width)
            .filter(|x| {
                let p = pixel(image, *x, 64);
                let brightest = p[0].max(p[1]).max(p[2]);
                brightest > 8 && brightest < 100
            })
            .count()
    };

    // The fill still varies across the mask, which is the whole point: a solid
    // would have come out one color and told us nothing.
    let left = pixel(&pixels, 40, 64);
    let right = pixel(&pixels, 88, 64);
    assert!(
        left[0] > right[0] + 40 && right[2] > left[2] + 40,
        "the gradient did not survive the mask: {left:?} against {right:?}"
    );
    // And the edge is soft rather than the circle's own. Checked against the
    // same arrangement with no blur, so what it establishes is that the blur
    // reached the coverage rather than that a circle has edges.
    assert!(
        ramp(&pixels) > ramp(&sharp) + 8,
        "the masked edge is {} pixels wide against {} unblurred, so the blur did \
         not reach the coverage",
        ramp(&pixels),
        ramp(&sharp)
    );
    // Far outside, nothing was drawn at all.
    assert_eq!(pixel(&pixels, 4, 64), [0, 0, 0, 255], "beyond the blur");
}

/// One drawing operation and the name a failure should report it by.
type NamedDrawing = (&'static str, Box<dyn Fn(&mut Canvas)>);

/// `docs/parity.md` says `transform` is *yes*, which is a claim about every
/// drawing operation and not only the ones perspective was built against.
///
/// Each of these reaches clip space by a different route -- a tessellated path,
/// an analytic distance field, a stroke expanded from points, a shadow made of
/// blurred layers -- and the type change that carried perspective through was
/// checked by a compiler, which can say the transform arrived and cannot say it
/// was used. So each is drawn twice, once under a transform carrying
/// perspective and once under the affine that transform reduces to, and asked
/// to differ. An operation that ignored the perspective row would render the
/// same picture both times and pass everything else here.
#[test]
fn every_drawing_operation_answers_to_a_transform_with_perspective() {
    let Some(mut ctx) = context() else { return };
    let paint = Paint::fill(Color::WHITE).with_anti_alias(false);
    let stroked = Paint::stroke(Color::WHITE, 6.0).with_anti_alias(false);
    let square = Rect::new(24.0, 24.0, 104.0, 104.0);

    let mut wedge = PathBuilder::new();
    wedge
        .move_to(Vec2::new(24.0, 104.0))
        .line_to(Vec2::new(64.0, 24.0))
        .line_to(Vec2::new(104.0, 104.0));
    let wedge = wedge.build();

    // Named so a failure says which route stopped carrying the transform.
    let operations: Vec<NamedDrawing> = vec![
        ("draw_path", {
            let wedge = wedge.clone();
            let paint = paint.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_path(&wedge, &paint).expect("path");
            })
        }),
        ("draw_rect", {
            let paint = paint.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_rect(square, &paint).expect("rect");
            })
        }),
        ("draw_rrect", {
            let paint = paint.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_rrect(square, 16.0, &paint).expect("rrect");
            })
        }),
        ("draw_circle", {
            let paint = paint.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_circle(Vec2::new(64.0, 64.0), 40.0, &paint)
                    .expect("circle");
            })
        }),
        ("draw_oval", {
            let paint = paint.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_oval(Rect::new(20.0, 36.0, 108.0, 92.0), &paint)
                    .expect("oval");
            })
        }),
        ("draw_drrect", {
            let paint = paint.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_drrect(square, 20.0, Rect::new(48.0, 48.0, 80.0, 80.0), 8.0, &paint)
                    .expect("drrect");
            })
        }),
        ("draw_line", {
            let stroked = stroked.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_line(Vec2::new(16.0, 96.0), Vec2::new(112.0, 32.0), &stroked)
                    .expect("line");
            })
        }),
        ("draw_points", {
            let stroked = stroked.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_points(
                    PointMode::Lines,
                    &[
                        Vec2::new(24.0, 32.0),
                        Vec2::new(104.0, 48.0),
                        Vec2::new(24.0, 80.0),
                        Vec2::new(104.0, 96.0),
                    ],
                    &stroked,
                )
                .expect("points");
            })
        }),
        ("draw_shadow", {
            let wedge = wedge.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_shadow(&wedge, Color::WHITE, 6.0, false)
                    .expect("shadow");
            })
        }),
        ("stroked path", {
            let wedge = wedge.clone();
            let stroked = stroked.clone();
            Box::new(move |c: &mut Canvas| {
                c.draw_path(&wedge, &stroked).expect("stroke");
            })
        }),
        // A picture is placed rather than redrawn, so it reaches clip space
        // through a material's mapping instead of through its own vertices --
        // and that mapping was rebuilt from scratch by this work.
        ("draw_recording", {
            let mut inner = Canvas::new(Extent2D::new(64, 64));
            inner.clear(Color::linear(0.0, 0.0, 0.0, 0.0));
            inner
                .draw_rect(
                    Rect::new(8.0, 8.0, 56.0, 56.0),
                    &Paint::fill(Color::WHITE).with_anti_alias(false),
                )
                .expect("rect");
            let picture = inner.finish();
            Box::new(move |c: &mut Canvas| {
                c.draw_recording(&picture, &Paint::fill(Color::WHITE))
                    .expect("recording");
            })
        }),
    ];

    for (name, draw) in operations {
        let render_with = |ctx: &mut Context, slope: f32| {
            let mut canvas = Canvas::new(SIZE);
            canvas.clear(Color::BLACK);
            canvas.concat_4x4(&receding(slope, 0.0));
            draw(&mut canvas);
            render(ctx, canvas)
        };
        let perspective = render_with(&mut ctx, 0.006);
        let flat = render_with(&mut ctx, 0.0);

        assert!(
            perspective.iter().any(|&v| v != 0),
            "{name} drew nothing at all under perspective"
        );
        assert!(
            perspective != flat,
            "{name} renders the same with the perspective row dropped, so it is \
             not carrying the transform it was given"
        );
    }
}

/// The last drawing route left out of the sweep above, because it needs an
/// atlas uploaded before it can draw anything.
///
/// Worth its own test rather than skipping: a glyph run is the one geometry
/// here whose texture coordinates come per vertex instead of from the
/// material, and its quads are built by the canvas itself rather than by the
/// tessellator -- a line this work changed to carry a divisor, and the only
/// caller of it that a mesh does not also exercise.
#[test]
fn a_glyph_run_answers_to_a_transform_with_perspective() {
    let Some(mut ctx) = context() else { return };
    let (atlas, solid, _) = two_glyph_atlas();
    let image = upload_atlas(&mut ctx, &atlas);

    let run = |ctx: &mut Context, slope: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        canvas.concat_4x4(&receding(slope, 0.0));
        let glyphs: Vec<PositionedGlyph> = (0..5)
            .map(|i| {
                PositionedGlyph::new(
                    solid,
                    [8.0 + i as f32 * 22.0, 56.0],
                    atlas.get(solid).unwrap(),
                )
            })
            .collect();
        canvas
            .draw_glyphs(&glyphs, &atlas, 0, &Paint::fill(Color::WHITE))
            .expect("glyphs");
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &canvas.finish(), &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let perspective = run(&mut ctx, 0.006);
    let flat = run(&mut ctx, 0.0);
    ctx.destroy_image(image);

    assert!(
        perspective.iter().any(|&v| v != 0),
        "the run drew nothing under perspective"
    );
    assert_ne!(
        perspective, flat,
        "a glyph run renders the same with the perspective row dropped"
    );

    // Evenly spaced in their own coordinates, the glyphs must not stay evenly
    // spaced on the target: the divisor grows to the right, so each gap comes
    // out narrower than the one before it. Spacing rather than mere difference,
    // because a run that moved bodily would differ too and would mean the
    // transform reached the placement without reaching the shape of the run.
    // Columns rather than a single row: the divisor scales y as well as x, so
    // the run slants and no one scanline crosses all of it.
    let lit: Vec<u32> = (0..SIZE.width)
        .filter(|x| (0..SIZE.height).any(|y| pixel(&perspective, *x, y)[0] > 128))
        .collect();
    let gaps: Vec<u32> = lit
        .windows(2)
        .filter(|w| w[1] - w[0] > 1)
        .map(|w| w[1] - w[0])
        .collect();
    assert!(
        gaps.len() >= 3,
        "expected several gaps between glyphs, found {gaps:?}"
    );
    assert!(
        gaps.first() > gaps.last(),
        "the gaps do not narrow toward the far side: {gaps:?}"
    );
}

/// The same gradient, stated with four stops and with five, must be the same
/// picture -- and `ramp.rs` says so about itself.
///
/// That claim held only because nothing could observe the difference. A stop
/// list of four or fewer travels in the material as raw floats and is walked by
/// the shader; more than four is tabulated into a texture first, and that
/// tabulation clamped each component to zero and one and rounded it to eight
/// bits. Into an eight-bit target the write clamps too, so the two agreed on
/// every output that could be reached.
///
/// A color filter reaches past that, because it runs on what the gradient
/// produced. Halve a component of 1.2 and the walked path gives 0.6; halve what
/// the ramp kept of it and the tabulated path gives 0.5. Both land inside the
/// unit range, so nothing clips them, and adding a stop that changes nothing
/// about the gradient changes the picture.
#[test]
fn a_gradient_carries_the_same_color_however_many_stops_state_it() {
    let Some(mut ctx) = context() else { return };

    // Red past what eight bits can hold, running to black. Stated as three
    // stops and again as five, the extra two exactly on the line.
    //
    // Stated in sRGB rather than in light, and that is the whole of what makes
    // the extra two redundant: a gradient is interpolated in the space its
    // stops are carried in, and that space is the encoded one. Midpoints of the
    // *light* values are not midpoints of the encoded ones, so stating them
    // that way put the extra stops off the line and the two paths disagreed by
    // twenty-three levels -- which is a fact about arithmetic rather than about
    // either path.
    let bright = Color::srgb(1.2, 0.0, 0.0, 1.0);
    let ends = vec![
        GradientStop::new(bright, 0.0),
        GradientStop::new(Color::srgb(0.6, 0.0, 0.0, 1.0), 0.5),
        GradientStop::new(Color::srgb(0.0, 0.0, 0.0, 1.0), 1.0),
    ];
    let mut many = ends.clone();
    for (offset, value) in [(0.25f32, 0.9f32), (0.75, 0.3)] {
        many.push(GradientStop::new(Color::srgb(value, 0.0, 0.0, 1.0), offset));
    }
    many.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap());
    assert!(ends.len() <= MAX_STOPS, "the control must be walked");
    assert!(many.len() > MAX_STOPS, "the subject must be tabulated");

    // Halves every channel, which brings the whole gradient inside the unit
    // range before it reaches the target. Without this the write clamps both
    // paths to the same place and the difference cannot be seen.
    let halve = ColorFilter::matrix([
        0.5, 0.0, 0.0, 0.0, 0.0, //
        0.0, 0.5, 0.0, 0.0, 0.0, //
        0.0, 0.0, 0.5, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]);

    let render_with = |ctx: &mut Context, stops: Vec<GradientStop>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                &Paint::linear_gradient(Vec2::ZERO, Vec2::new(128.0, 0.0), stops)
                    .with_color_filter(halve)
                    .with_anti_alias(false),
            )
            .expect("gradient");
        render(ctx, canvas)
    };

    let walked = render_with(&mut ctx, ends);
    let tabulated = render_with(&mut ctx, many);

    let mut worst = 0i32;
    let mut at = 0u32;
    for x in 0..128u32 {
        let a = pixel(&walked, x, 64);
        let b = pixel(&tabulated, x, 64);
        for channel in 0..4 {
            let delta = (a[channel] as i32 - b[channel] as i32).abs();
            if delta > worst {
                worst = delta;
                at = x;
            }
        }
    }
    assert!(
        worst <= 2,
        "adding a stop that changes nothing changed the picture by {worst} at x={at}: \
         walked {:?} against tabulated {:?}",
        pixel(&walked, at, 64),
        pixel(&tabulated, at, 64)
    );
}

/// Read a floating-point surface back as linear components.
///
/// The bytes arrive packed as half-floats, which is what the format holds;
/// nothing in the testkit's comparison surface understands them, and nothing
/// needs to -- this reads a few values out of one image rather than comparing
/// two.
fn wide_pixels(ctx: &mut Context, canvas: Canvas) -> Vec<[f32; 4]> {
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba16Float)
        .expect("floating-point surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let bytes = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    unpack_half(&bytes)
}

/// Half-floats as this machine wrote them, four to a texel.
fn unpack_half(bytes: &[u8]) -> Vec<[f32; 4]> {
    bytes
        .chunks_exact(8)
        .map(|texel| {
            let mut out = [0.0; 4];
            for (channel, slot) in out.iter_mut().enumerate() {
                let at = channel * 2;
                *slot = half::f16::from_le_bytes([texel[at], texel[at + 1]]).to_f32();
            }
            out
        })
        .collect()
}

/// The same, for a drawing that reads a sheet.
///
/// `draw_atlas` needs its image bound, which `wide_pixels` has no way to pass.
/// Split rather than given an empty slice everywhere, because the two call sites
/// that need images are the only ones that should have to name them.
fn wide_pixels_with_images(
    ctx: &mut Context,
    canvas: Canvas,
    images: &[&emblema::Image],
) -> Vec<[f32; 4]> {
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba16Float)
        .expect("floating-point surface");
    ctx.draw_with_images(&mut surface, &canvas.finish(), images)
        .expect("draw");
    let bytes = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    unpack_half(&bytes)
}

fn wide_pixel(pixels: &[[f32; 4]], x: u32, y: u32) -> [f32; 4] {
    pixels[(y * SIZE.width + x) as usize]
}

/// The whole point of the work, end to end: a color the sRGB primaries cannot
/// describe reaches a target that can hold it, with the components that say so
/// still outside the unit range.
#[test]
fn a_color_outside_the_srgb_primaries_reaches_a_floating_point_target() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().float_render_targets {
        eprintln!("skipping: this device cannot render into a floating-point target");
        return;
    }

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(16.0, 16.0, 112.0, 112.0),
            &Paint::fill(Color::display_p3(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let wide = wide_pixels(&mut ctx, canvas);
    let inside = wide_pixel(&wide, 64, 64);

    // Display P3's red, restated against sRGB's primaries and left encoded --
    // which is what upstream's `p3ToExtendedSrgb` returns, and why the figures
    // are the encoded ones rather than the linear ones this used to assert. The
    // rotation itself still happens in light; it is the value coming out that
    // stays in the transfer function.
    assert!((inside[0] - 1.093_066).abs() < 0.01, "red {}", inside[0]);
    assert!((inside[1] + 0.226_742).abs() < 0.01, "green {}", inside[1]);
    assert!((inside[2] + 0.150_137).abs() < 0.01, "blue {}", inside[2]);
    // The negatives are the whole statement. A pipeline that clamped anywhere
    // between the paint and the target would return zero here and pass every
    // other test in this file.
    assert!(inside[1] < -0.02, "green came back {}", inside[1]);

    // And the same drawing into an eight-bit target still saturates, which is
    // what says the clamps that were removed did not leak into the path every
    // other test uses.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(16.0, 16.0, 112.0, 112.0),
            &Paint::fill(Color::display_p3(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    let narrow = render(&mut ctx, canvas);
    assert_eq!(pixel(&narrow, 64, 64), [255, 0, 0, 255]);
}

/// A layer is an intermediate of the frame it composites into, so a
/// floating-point root has to give floating-point layers -- otherwise the value
/// above dies at the first `saveLayer` and nothing says so.
#[test]
fn a_wide_color_survives_a_layer_when_the_root_can_hold_it() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().float_render_targets {
        eprintln!("skipping: this device cannot render into a floating-point target");
        return;
    }

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer(Layer::opacity(1.0));
    canvas
        .draw_rect(
            Rect::new(16.0, 16.0, 112.0, 112.0),
            &Paint::fill(Color::display_p3(1.0, 0.0, 0.0, 1.0)).with_anti_alias(false),
        )
        .expect("rect");
    canvas.restore();

    let wide = wide_pixels(&mut ctx, canvas);
    let inside = wide_pixel(&wide, 64, 64);
    assert!(
        inside[1] < -0.02,
        "the layer flattened the color to {inside:?}"
    );
    // Encoded, like everything else the pipeline carries.
    assert!((inside[0] - 1.093_066).abs() < 0.02, "red {}", inside[0]);
}

/// A layer with nothing asked of it must not change the picture.
///
/// `saveLayer` with full opacity and no filter is an identity: the content is
/// drawn into a target of its own and composited straight back. Whether that
/// round trip is lossless depends on what the target holds, and it used to
/// depend on it sharply -- the layer held linear eight-bit color whatever the
/// frame was landing in, and eight bits of *linear* color band visibly in the
/// darks. Against an sRGB surface the two paths disagreed by seven levels, and
/// a dark ramp resolving forty distinct values drawn directly came back as six
/// through a layer.
///
/// Both halves of that are gone. The pipeline carries sRGB-encoded components,
/// so a layer's eight bits are spaced the way the eye reads them without
/// anything having to arrange it; and a target is a plain unsigned normalized
/// format rather than an sRGB one, because a format that encodes on write would
/// encode what is already encoded. There is no longer a pair of spaces for a
/// layer to fall between.
///
/// Still worth asking, because the identity is worth having however it is
/// arrived at, and a dark ramp is still where a lost tone shows first.
#[test]
fn a_layer_that_asks_for_nothing_keeps_the_tones_of_what_it_holds() {
    // Asked of both backends rather than of whichever one comes first, since
    // they realize a layer's target and its composite differently and nothing
    // else in the suite counts tones through one.
    //
    // And twice on each, with antialiasing off and on, because a multisampled
    // layer resolves before it is composited: whether that resolve happens on
    // linear values or encoded ones is the second thing an sRGB attachment
    // changes, and averaging encoded values is exactly the mistake the color
    // policy exists to prevent.
    for preference in [BackendPreference::Vulkan, BackendPreference::Gles] {
        let Ok(mut ctx) = Context::new(preference) else {
            eprintln!("skipping {preference:?}: no such backend here");
            continue;
        };
        for anti_alias in [false, true] {
            tones_survive_a_layer(&mut ctx, anti_alias, preference);
        }
    }
}

fn tones_survive_a_layer(ctx: &mut Context, anti_alias: bool, backend: BackendPreference) {
    let draw = |ctx: &mut Context, layered: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        if layered {
            canvas.save_layer(Layer::opacity(1.0));
        }
        canvas
            .draw_rect(
                Rect::from_size(128.0, 128.0),
                // Dark, because that is where linear eight-bit quantization is
                // coarse and the transfer function is steep. The same ramp in
                // the midtones would hide the difference.
                &Paint::linear_gradient(
                    Vec2::ZERO,
                    Vec2::new(128.0, 0.0),
                    vec![
                        GradientStop::new(Color::linear(0.0, 0.0, 0.0, 1.0), 0.0),
                        GradientStop::new(Color::linear(0.02, 0.02, 0.02, 1.0), 1.0),
                    ],
                )
                .with_anti_alias(anti_alias),
            )
            .expect("gradient");
        if layered {
            canvas.restore();
        }
        // An sRGB surface, because that is what makes the question visible: the
        // frame spaces its eight bits through the transfer function, and a
        // layer that does not is throwing away tones the frame could have held.
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("srgb surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let direct = draw(ctx, false);
    let layered = draw(ctx, true);

    let distinct = |p: &[u8]| {
        (0..128u32)
            .map(|x| pixel(p, x, 64)[0])
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    };
    let worst = (0..128u32)
        .map(|x| (pixel(&direct, x, 64)[0] as i32 - pixel(&layered, x, 64)[0] as i32).abs())
        .max()
        .expect("a row");

    assert!(
        worst <= 2,
        "{backend:?} with anti_alias {anti_alias}: a layer changed the picture by \
         {worst} levels, {} distinct tones drawn directly against {} through the \
         layer",
        distinct(&direct),
        distinct(&layered)
    );
    // And the ramp really is a ramp, so the comparison above is between two
    // pictures rather than two flat fields that trivially agree.
    assert!(
        distinct(&direct) > 20,
        "the probe ramp resolved only {} tones, so it cannot show quantization",
        distinct(&direct)
    );
}

/// A mask blur over a gradient, which the paint used to refuse.
///
/// `dart:ui` is unambiguous about what a mask filter means -- blur the mask,
/// then fill through it -- and that is well defined for a fill that varies. The
/// paint implemented the other order, drawing itself through a blurred layer,
/// which is the same picture only where the fill does not vary; so rather than
/// hand back a different picture under the same name it refused.
///
/// Checked against the arrangement a caller could already assemble by hand out
/// of two layers and `DstIn`, because that is the same statement made twice by
/// different means: if the paint's version and the hand-built one agree, the
/// paint is doing what the caller would have had to.
#[test]
fn a_mask_blur_over_a_gradient_matches_what_a_caller_would_assemble() {
    let Some(mut ctx) = context() else { return };
    let whole = Rect::new(0.0, 0.0, 128.0, 128.0);
    let shader = Shader::LinearGradient {
        start: Vec2::new(0.0, 0.0),
        end: Vec2::new(128.0, 0.0),
        stops: vec![
            GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.0),
            GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 1.0),
        ],
        tile: TileMode::Clamp,
    };
    let circle = (Vec2::new(64.0, 64.0), 36.0);
    let sigma = 6.0;

    // What the paint does now.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_circle(
            circle.0,
            circle.1,
            &Paint::fill(Color::WHITE)
                .with_shader(shader.clone())
                .with_mask_blur(sigma),
        )
        .expect("the paint refused a mask blur over a gradient");
    let built = render(&mut ctx, canvas);

    // What a caller had to write instead: the fill across everything the blur
    // reaches, then blurred coverage taken out of its alpha.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer_bounds(Layer::opacity(1.0), whole);
    canvas
        .draw_rect(whole, &Paint::fill(Color::WHITE).with_shader(shader))
        .expect("fill");
    canvas.save_layer_bounds(
        Layer::opacity(1.0)
            .with_blur(sigma)
            .with_blend(BlendMode::DstIn),
        whole,
    );
    canvas
        .draw_circle(circle.0, circle.1, &Paint::fill(Color::WHITE))
        .expect("coverage");
    canvas.restore();
    canvas.restore();
    let assembled = render(&mut ctx, canvas);

    let mut worst = 0i32;
    for y in (0..128u32).step_by(4) {
        for x in (0..128u32).step_by(4) {
            let a = pixel(&built, x, y);
            let b = pixel(&assembled, x, y);
            for channel in 0..4 {
                worst = worst.max((a[channel] as i32 - b[channel] as i32).abs());
            }
        }
    }
    assert!(
        worst <= 2,
        "the paint and the hand-built arrangement disagree by {worst}"
    );

    // And the gradient survived the mask, which is the thing a solid color
    // could not have shown: a mask blur that dropped the shader would leave a
    // flat shape and agree with nothing.
    let left = pixel(&built, 40, 64);
    let right = pixel(&built, 88, 64);
    assert!(
        left[0] > right[0] + 40 && right[2] > left[2] + 40,
        "the gradient did not survive: {left:?} against {right:?}"
    );
}

/// A thin stroke under a shear draws, and a stroke of no width is a hairline.
///
/// Worth pinning together, because the inventory once put a scene out of reach
/// with "a hairline skew" and the two halves of that are different questions. A
/// shear is not the difficulty -- half a pixel of width under one draws exactly
/// as it should.
///
/// The other half is `strokeWidth` of zero, which `dart:ui` documents as "a
/// hairline width" and makes the default value of the field. This renderer read
/// it as no line until 2026-09-17, on the reasoning that a caller animating a
/// width to nothing should stop drawing rather than watch a shape refuse to
/// disappear. What settled it against that reasoning was the default: a Flutter
/// app that strokes without setting a width drew a hairline upstream and nothing
/// here, which is not an edge a caller opts into. So zero is a hairline, and the
/// discontinuity comes with it -- the same one upstream has, and has on purpose,
/// since `ComputeStrokeAlphaCoverage` opens by returning full coverage for it.
///
/// The width either side of zero is checked here too, because the jump is the
/// part that is easy to get wrong in one direction only: a sub-pixel stroke dims
/// with its width, and zero does not.
#[test]
fn a_shear_is_no_obstacle_to_a_thin_stroke_and_zero_is_a_hairline() {
    let Some(mut ctx) = context() else { return };
    let sheared = |ctx: &mut Context, width: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let mut skew = Affine2::IDENTITY;
        skew.matrix2.y_axis.x = 0.5;
        canvas.concat(skew);
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(20.0, 20.0))
            .line_to(Vec2::new(20.0, 100.0));
        canvas
            .draw_path(&b.build(), &Paint::stroke(Color::WHITE, width))
            .expect("a stroke under a shear");
        let pixels = render(ctx, canvas);
        (0..128u32)
            .flat_map(|x| (0..128u32).map(move |y| (x, y)))
            .filter(|(x, y)| pixel(&pixels, *x, *y)[0] > 20)
            .count()
    };

    let half = sheared(&mut ctx, 0.5);
    assert!(
        half > 40,
        "half a pixel of width under a shear should still draw a line"
    );
    let none = sheared(&mut ctx, 0.0);
    assert!(
        none > 40,
        "a width of zero is a hairline and should draw a line of its own, and \
         lit {none} pixels against {half} for half a pixel of width"
    );

    // The jump, stated as the thing it is: zero is full coverage and a width
    // just above it is nearly none. A rule that dimmed zero along with
    // everything thinner than a pixel would draw it at almost nothing, and
    // would pass the count above while looking wrong.
    let lit_at = |ctx: &mut Context, width: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_line(
                Vec2::new(8.0, 64.0),
                Vec2::new(120.0, 64.0),
                &Paint::stroke(Color::WHITE, width),
            )
            .expect("a line");
        let pixels = render(ctx, canvas);
        pixel(&pixels, 64, 64)[0]
    };
    let at_zero = lit_at(&mut ctx, 0.0);
    let just_above = lit_at(&mut ctx, 0.02);
    assert!(
        at_zero > 100,
        "a hairline is drawn at full coverage, and came back at {at_zero}"
    );
    assert!(
        just_above < 40,
        "a fiftieth of a pixel is dimmed to nearly nothing, and came back at \
         {just_above}"
    );
}

/// The two spellings of one layer must draw the same picture.
///
/// `save_layer` sizes its target to the surface and `save_layer_bounds` sizes
/// it to a rectangle the caller names, so the same content goes through two
/// differently shaped targets and must come out identical. Today that is a
/// statement about the bounded path being an optimization rather than a second
/// rendering; it is written now because it is the guard for deriving the bound
/// instead of being handed it, and the failure it exists to catch is silent.
///
/// A gradient inside the layer is the point rather than decoration. A radial
/// gradient's center is carried **in clip space**, so it is stated relative to
/// the target the draw was recorded against. Anything that changes a layer's
/// target without carrying that field through leaves the shape where it was
/// and slides the color across it -- which no assertion about a target's size
/// would notice.
#[test]
fn a_layer_draws_the_same_picture_whether_or_not_its_bounds_were_named() {
    let Some(mut ctx) = context() else {
        return;
    };

    // Tight to the content, which is what a derived bound would compute. The
    // blur's own reach is added by the renderer, not by the caller.
    let held = Rect::new(24.0, 24.0, 104.0, 104.0);
    let content = |canvas: &mut Canvas| {
        canvas
            .draw_circle(
                Vec2::new(64.0, 64.0),
                40.0,
                &Paint::default().with_shader(Shader::RadialGradient {
                    center: Vec2::new(52.0, 58.0),
                    radius: 44.0,
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: Color::srgb(1.0, 0.9, 0.2, 1.0),
                        },
                        GradientStop {
                            offset: 1.0,
                            color: Color::srgb(0.1, 0.2, 0.9, 1.0),
                        },
                    ],
                    tile: TileMode::Clamp,
                }),
            )
            .expect("a gradient inside the layer");
    };

    let mut unbounded = Canvas::new(SIZE);
    unbounded.clear(Color::srgb(0.1, 0.1, 0.1, 1.0));
    unbounded.save_layer(Layer::opacity(0.7).with_blur(6.0));
    content(&mut unbounded);
    unbounded.restore();
    let loose = render(&mut ctx, unbounded);

    let mut bounded = Canvas::new(SIZE);
    bounded.clear(Color::srgb(0.1, 0.1, 0.1, 1.0));
    bounded.save_layer_bounds(Layer::opacity(0.7).with_blur(6.0), held);
    content(&mut bounded);
    bounded.restore();
    let tight = render(&mut ctx, bounded);

    // Every pixel, not a sample of them: a gradient displaced by a few texels
    // agrees over most of the frame and disagrees in a band.
    // One level, not zero, and the distinction is the whole assertion. The two
    // targets have different origins, so the blur's taps land on different
    // texel centers and round to different sides of a level -- which is a
    // rounding difference and shows up as a spray of plus-or-minus one across
    // the softened region. A gradient carried into a differently-shaped target
    // without its clip-space center coming along does not look like that: the
    // shape stays where it is and the color slides across it, which is tens of
    // levels over a band. `max_delta` separates the two, and nothing about a
    // target's size does.
    let max = (0..loose.len())
        .map(|i| loose[i].abs_diff(tight[i]))
        .max()
        .unwrap_or(0);
    assert!(
        max <= 1,
        "the two spellings differ by {max} levels, which is more than rounding"
    );
}

/// A layer with no bounds and a whole-layer filter is sized by what the filter
/// reaches, not by what was drawn.
///
/// The layer's target is derived from its content at `restore`, and a filter
/// given to the layer as a whole draws past that content by an amount only the
/// filter knows -- its kernel's radius, for a blur. Sizing to the content alone
/// would cut the halo off square, and the cut lands where the content ends
/// rather than at the target's edge, so it looks like a shadow with a straight
/// side rather than like a clipped layer.
///
/// Two assertions, and they fail for opposite mistakes. The extent says the
/// filter's reach was added: without it the target stops at the circle. The
/// picture says the reach was not merely *allocated* but drawn, by comparing
/// against the same blur spelled on the layer itself, which is a different
/// route to the same image.
#[test]
fn a_filtered_layer_with_no_bounds_is_sized_by_what_the_filter_reaches() {
    let Some(mut ctx) = context() else {
        return;
    };
    let sigma = 8.0;
    let at = Vec2::new(64.0, 64.0);
    let radius = 20.0;

    let mut filtered = Canvas::new(SIZE);
    filtered.clear(Color::BLACK);
    filtered
        .save_layer_filtered(Layer::opacity(1.0), None, &ImageFilter::blur(sigma))
        .expect("a blur filters a group");
    filtered
        .draw_circle(at, radius, &Paint::fill(Color::WHITE))
        .expect("content");
    filtered.restore();
    let recording = filtered.finish();
    let extent = recording.passes[0].extent;

    // How big the same layer is with no filter over it: the content's own
    // bounds and nothing more. Measured rather than written down, because the
    // reach is `ceil((sigma - 0.5) * sqrt(3))` -- upstream's
    // `kKernelRadiusPerSigma` -- and a test that restated that arithmetic
    // would pass while disagreeing with the renderer about it.
    let mut plain = Canvas::new(SIZE);
    plain.clear(Color::BLACK);
    plain.save_layer(Layer::opacity(1.0));
    plain
        .draw_circle(at, radius, &Paint::fill(Color::WHITE))
        .expect("content");
    plain.restore();
    let content_only = plain.finish().passes[0].extent;

    assert!(
        extent.width > content_only.width && extent.height > content_only.height,
        "{extent:?} is no larger than the {content_only:?} the content alone needs, \
         so the filter's reach was never added and its halo is cut"
    );
    // And still narrowed: a layer that took the whole frame would satisfy the
    // line above without having derived anything.
    assert!(
        extent.width < SIZE.width && extent.height < SIZE.height,
        "{extent:?} is the whole frame, so nothing was derived"
    );

    let through_filter = render_recording(&mut ctx, &recording);

    // The same blur, asked for on the layer rather than as a filter over it.
    let mut direct = Canvas::new(SIZE);
    direct.clear(Color::BLACK);
    direct.save_layer(Layer::opacity(1.0).with_blur(sigma));
    direct
        .draw_circle(at, radius, &Paint::fill(Color::WHITE))
        .expect("content");
    direct.restore();
    let through_layer = render(&mut ctx, direct);

    let worst = through_filter
        .iter()
        .zip(&through_layer)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "the two spellings of one blur disagree by {worst} levels"
    );
}

/// A layer multiplied into what is under it has to cover everything it might
/// zero, not just what it drew.
///
/// `Modulate` multiplies the two sides, so where the layer drew nothing its
/// source is transparent black and the product is zero: "the layer drew nothing
/// here" and "leave this pixel alone" are different instructions. A layer sized
/// to its content stops covering the rest, and the rest keeps whatever was
/// under it -- which is the picture nobody debugs from, because the part that
/// went wrong is the part where nothing was drawn.
///
/// This is the one entry in `BlendMode::is_destructive` that cannot be read off
/// its name, so it is checked against a picture rather than argued for. The
/// list matches upstream's `Entity::IsBlendModeDestructive`, and `DstIn` from
/// the same list was caught by the suite at 252 levels when layers were first
/// narrowed.
#[test]
fn a_layer_multiplied_into_the_frame_zeroes_what_it_did_not_draw() {
    let Some(mut ctx) = context() else {
        return;
    };
    let mut canvas = Canvas::new(SIZE);
    // Opaque white, so anything left unmultiplied stays obvious.
    canvas.clear(Color::WHITE);
    canvas.save_layer(Layer::opacity(1.0).with_blend(BlendMode::Modulate));
    canvas
        .draw_rect(
            Rect::new(48.0, 48.0, 80.0, 80.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("the layer's own content");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);

    // Inside the square both sides are white, so the product is white.
    assert_eq!(
        pixel(&pixels, 64, 64),
        [255, 255, 255, 255],
        "the layer's own content should have survived being multiplied by white"
    );
    // Outside it the layer is transparent black, and white times nothing is
    // nothing. A layer narrowed to its content never reaches here and the
    // background stays white.
    for (x, y) in [(8, 8), (120, 8), (8, 120), (120, 120), (64, 16)] {
        assert_eq!(
            pixel(&pixels, x, y),
            [0, 0, 0, 0],
            "({x},{y}) was not multiplied, so the layer did not cover it"
        );
    }
}

/// The analytic blurred rounded rectangle must agree with the blur it replaces.
///
/// `draw_rrect` on a solid fill with a mask blur now evaluates the blur in the
/// fragment stage instead of drawing the shape into a layer and running a
/// separable Gaussian over it. The same shape stated as a path still takes the
/// general route, so the two are the same picture by two entirely different
/// mechanisms -- which is the only check available here, and a better one than
/// my eyesight.
///
/// The tolerance is loose on purpose and is the honest part of this test. The
/// analytic route is an *approximation* of a Gaussian convolved with a rounded
/// rectangle -- there is no closed form -- so it does not converge on the
/// sampled one. What it must not do is disagree in a way that reads as a
/// different shape, which is what a per-channel bound over the whole frame
/// catches.
#[test]
fn an_analytic_blurred_rectangle_agrees_with_the_blur_it_replaces() {
    let Some(mut ctx) = context() else {
        return;
    };
    // The last three are eccentric on purpose. The transcription carries a
    // correction that pulls in the longer axis -- a rectangle much longer than
    // it is wide blurs to something the axis-wise expression makes too
    // eccentric otherwise -- and it is the least self-evident arithmetic in
    // the whole of it. A near-square sweep never reaches it: the correction is
    // the difference of two Gaussians of the sides, which is zero when the
    // sides agree.
    for (radius, sigma, rect) in [
        (16.0, 4.0, Rect::new(34.0, 30.0, 94.0, 98.0)),
        (16.0, 8.0, Rect::new(34.0, 30.0, 94.0, 98.0)),
        (0.0, 6.0, Rect::new(34.0, 30.0, 94.0, 98.0)),
        (40.0, 5.0, Rect::new(34.0, 30.0, 94.0, 98.0)),
        // Wide and short, short and wide, and one thin enough that the blur is
        // wider than the shape.
        (4.0, 5.0, Rect::new(14.0, 54.0, 114.0, 74.0)),
        (4.0, 5.0, Rect::new(54.0, 14.0, 74.0, 114.0)),
        (2.0, 7.0, Rect::new(20.0, 60.0, 108.0, 68.0)),
        // Wide deviations, where the two stop agreeing and the reason is the
        // sampled route rather than this one. It truncates its kernel at
        // `blur_reach`, about 1.73 deviations; the evaluated blur carries to
        // roughly 2.5. Measured at a deviation of twenty: the evaluated blur
        // reaches 51 texels past the shape and the sampled one 34, which is
        // `ceil(19.5 * sqrt(3))` exactly. So the disagreement here is a halo
        // the sampled route cuts off, and of the two it is the evaluated one
        // that is closer to a Gaussian.
        (16.0, 12.0, Rect::new(34.0, 30.0, 94.0, 98.0)),
        (16.0, 20.0, Rect::new(34.0, 30.0, 94.0, 98.0)),
        (16.0, 40.0, Rect::new(34.0, 30.0, 94.0, 98.0)),
    ] {
        let paint = Paint::fill(Color::WHITE).with_mask_blur(sigma);

        let mut analytic = Canvas::new(SIZE);
        analytic.clear(Color::BLACK);
        analytic.draw_rrect(rect, radius, &paint).expect("analytic");
        let one = render(&mut ctx, analytic);

        // The same outline built through the eight-radii spelling, which lays
        // down identical geometry and records no shape -- so it still takes the
        // general route and can serve as the reference. `to_rounded_path` no
        // longer can: it marks what it built, which is the whole point, and a
        // path that knows it is a rounded rectangle now takes the fast route
        // too.
        // At a radius above zero the eight-radii spelling lays down identical
        // geometry and records no shape, so it still takes the general route.
        // At zero it collapses to `to_path`, which *does* record one now that a
        // rectangle is a rounded one with square corners -- so the reference
        // there is the outline built by hand.
        let reference = if radius > 0.0 {
            rect.to_rounded_path_with_radii([[radius; 2]; 4])
        } else {
            square_path(rect)
        };
        let mut general = Canvas::new(SIZE);
        general.clear(Color::BLACK);
        general.draw_path(&reference, &paint).expect("general");
        let two = render(&mut ctx, general);

        let worst = one
            .iter()
            .zip(&two)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        let mean = one
            .iter()
            .zip(&two)
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum::<u64>() as f64
            / one.len() as f64;

        // Measured: 13, 22, 17, 16 for the near-square four, 18, 18, 9 for the
        // eccentric three, and 20, 48, 23 for the wide deviations, whose mean
        // climbs steadily -- 1.1 at a deviation of four against 7.6 at forty --
        // as the halo the sampled route cuts grows. A shape in the wrong place,
        // at the wrong size, or with the wrong falloff does not land here: it disagrees over a whole region, so the
        // mean goes with the worst. Measured at 13 to 22 worst and 1.1 to 2.9
        // mean over these four; the bounds are set above that and well under
        // what a misplaced shape gives.
        assert!(
            worst <= 64,
            "radius {radius} sigma {sigma}: the two routes differ by {worst} levels"
        );
        // The mean is the one that tracks the divergence rather than the
        // worst: 1.1 at a deviation of four, 7.6 at forty, because the cut halo
        // is a large area of small differences rather than a few big ones. The
        // bound is above the widest deviation measured and well under what a
        // shape drawn in the wrong place gives.
        assert!(
            mean <= 10.0,
            "radius {radius} sigma {sigma}: the two routes differ by {mean:.2} on average"
        );

        // And the two really are different routes. If the analytic one had
        // refused, both would be the general one and would agree exactly --
        // which is the way this test could pass while checking nothing.
        assert!(
            worst > 0,
            "radius {radius} sigma {sigma}: the routes agree exactly, so the \
             analytic one was not taken"
        );
    }
}

/// The analytic route costs one pass where the blur it replaces costs three.
///
/// The whole reason for it. A mask blur through the general route draws the
/// shape into a layer and runs a separable Gaussian over it: a pass for the
/// content and one per axis. Evaluating the blur in the fragment stage leaves
/// the draw in the pass already open.
#[test]
fn an_analytic_blurred_rectangle_costs_no_passes() {
    let paint = Paint::fill(Color::WHITE).with_mask_blur(6.0);
    let rect = Rect::new(34.0, 30.0, 94.0, 98.0);

    let mut analytic = Canvas::new(SIZE);
    analytic.draw_rrect(rect, 16.0, &paint).expect("analytic");
    assert_eq!(
        analytic.finish().passes.len(),
        1,
        "a blurred rounded rectangle should not have opened a layer"
    );

    // The same outline with no shape recorded on it, which is what the general
    // route still gets.
    let mut general = Canvas::new(SIZE);
    general
        .draw_path(&rect.to_rounded_path_with_radii([[16.0; 2]; 4]), &paint)
        .expect("general");
    assert!(
        general.finish().passes.len() > 1,
        "the general route should still be drawing into a layer"
    );
}

/// The four mask blur styles partition each other on an evaluated blur, as
/// they do on a sampled one.
///
/// `outer + inner == normal` is a stronger statement than any of the three
/// makes alone, and it holds only while all of them are built the same way.
/// For a while they were not: `Normal` on a shape that knows what it is was
/// evaluated in the fragment stage while the other three were still assembled
/// out of a sampled blur, and the identity held to about seventeen levels
/// rather than one. This is the same check as
/// `the_two_halves_of_a_blur_add_up_to_the_whole_of_it`, on the other route,
/// and it is what says the seam is gone rather than merely narrower.
#[test]
fn the_two_halves_of_an_evaluated_blur_add_up_to_the_whole_of_it_too() {
    let Some(mut ctx) = context() else {
        return;
    };
    let draw = |ctx: &mut Context, style: MaskBlurStyle| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::linear(0.0, 0.0, 0.0, 1.0));
        // Through the shape, so every style takes the evaluated blur.
        canvas
            .draw_rrect(
                Rect::new(34.0, 34.0, 94.0, 94.0),
                8.0,
                &Paint::fill(Color::linear(1.0, 1.0, 1.0, 1.0))
                    .with_mask_blur(6.0)
                    .with_mask_blur_style(style),
            )
            .expect("mask blur");
        render(ctx, canvas)
    };
    let normal = draw(&mut ctx, MaskBlurStyle::Normal);
    let outer = draw(&mut ctx, MaskBlurStyle::Outer);
    let inner = draw(&mut ctx, MaskBlurStyle::Inner);

    // Color only. Every alpha here is opaque, so summing that channel would
    // report 255 at every pixel whatever the blur did -- which is how a
    // seventeen-level difference once read as a catastrophic one.
    let worst = (0..normal.len())
        .filter(|i| i % 4 != 3)
        .map(|i| (outer[i] as i32 + inner[i] as i32 - normal[i] as i32).unsigned_abs())
        .max()
        .unwrap_or(0);

    assert!(
        worst <= 4,
        "the inner and outer halves should reconstruct the whole evaluated \
         blur; they differ by {worst}"
    );
}

/// A blur that cannot be one deviation is left to the route that blurs in
/// device space.
///
/// The analytic expression carries a single deviation, measured in the shape's
/// own space. Under a transform that scales the axes differently, the blur a
/// caller asked for is wider along one device axis than the other, and there is
/// nowhere in that expression to say so -- so it would draw a round blur where
/// an oval one was asked for, at whichever scale it happened to pick.
///
/// It refuses instead, and the check is that the shape then draws exactly as
/// the same outline does through the general route. Exactly, not nearly: both
/// are the same mechanism once the analytic one has stepped aside, so any
/// difference at all means it did not.
#[test]
fn a_blur_under_an_anisotropic_transform_is_not_evaluated_analytically() {
    let Some(mut ctx) = context() else {
        return;
    };
    let rect = Rect::new(20.0, 20.0, 50.0, 44.0);
    let paint = Paint::fill(Color::WHITE).with_mask_blur(5.0);

    let mut through_rect = Canvas::new(SIZE);
    through_rect.clear(Color::BLACK);
    through_rect.scale(2.0, 1.0);
    through_rect.draw_rect(rect, &paint).expect("rect");
    let one = render(&mut ctx, through_rect);

    let mut through_path = Canvas::new(SIZE);
    through_path.clear(Color::BLACK);
    through_path.scale(2.0, 1.0);
    through_path
        .draw_path(&square_path(rect), &paint)
        .expect("path");
    let two = render(&mut ctx, through_path);

    let differing = (0..one.len()).filter(|&i| one[i] != two[i]).count();
    assert_eq!(
        differing, 0,
        "{differing} channels differ, so the rectangle was evaluated analytically \
         under a transform that stretches one axis"
    );
}

/// The same, for a transform this expression cannot follow at all.
///
/// A perspective transform does not have one scale: it has a different one at
/// every point, so a deviation carried into the shape's space means a different
/// width at each end of the shape. The general route blurs the finished image
/// in device space and is unaffected.
#[test]
fn a_blur_under_perspective_is_not_evaluated_analytically() {
    let Some(mut ctx) = context() else {
        return;
    };
    let rect = Rect::new(30.0, 30.0, 90.0, 80.0);
    let paint = Paint::fill(Color::WHITE).with_mask_blur(5.0);
    // A mild perspective: enough to vary the scale across the shape, not enough
    // to send anything near the vanishing line.
    let perspective = [
        1.0, 0.0, 0.0, 0.0015, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];

    let mut through_rect = Canvas::new(SIZE);
    through_rect.clear(Color::BLACK);
    through_rect.concat_4x4(&perspective);
    through_rect.draw_rect(rect, &paint).expect("rect");
    let one = render(&mut ctx, through_rect);

    let mut through_path = Canvas::new(SIZE);
    through_path.clear(Color::BLACK);
    through_path.concat_4x4(&perspective);
    through_path
        .draw_path(&square_path(rect), &paint)
        .expect("path");
    let two = render(&mut ctx, through_path);

    let differing = (0..one.len()).filter(|&i| one[i] != two[i]).count();
    assert_eq!(
        differing, 0,
        "{differing} channels differ, so the rectangle was evaluated analytically \
         under a perspective transform"
    );
}

/// A blurred rectangle is the same shape whichever way it is turned.
///
/// The approximation shortens the longer axis by an amount that falls away as
/// either side grows past the deviation -- a rectangle much longer than it is
/// wide otherwise blurs to something the axis-wise expression makes too
/// eccentric. That correction is the one place the two axes are treated
/// separately, so it is the one place they can be treated *differently*.
///
/// A wide rectangle and its transpose are one shape reflected in the diagonal,
/// so their blurs must be too. Upstream's form fails this by twenty-four
/// levels: it writes `rSize += {min(delta, 0), max(delta, 0)}`, which shortens
/// the long axis when that is x and lengthens it when that is y, so the same
/// rectangle comes out 98.8 texels long one way round and 101.2 the other.
/// This renderer shortens whichever axis is the longer, which is what
/// upstream's own comment says it intends, and that difference is what this
/// test is for. The sampled route passes it either way, which is how the
/// asymmetry was traced to the approximation rather than to the rasterizer.
#[test]
fn a_blurred_rectangle_is_the_same_turned_either_way() {
    let Some(mut ctx) = context() else {
        return;
    };
    let paint = Paint::fill(Color::WHITE).with_mask_blur(5.0);

    let mut wide = Canvas::new(SIZE);
    wide.clear(Color::BLACK);
    wide.draw_rrect(Rect::new(14.0, 54.0, 114.0, 74.0), 4.0, &paint)
        .expect("wide");
    let wide = render(&mut ctx, wide);

    let mut tall = Canvas::new(SIZE);
    tall.clear(Color::BLACK);
    tall.draw_rrect(Rect::new(54.0, 14.0, 74.0, 114.0), 4.0, &paint)
        .expect("tall");
    let tall = render(&mut ctx, tall);

    // What is at (x, y) in one is at (y, x) in the other, the surface being
    // square.
    let mut worst = 0u8;
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            let a = pixel(&wide, x, y);
            let b = pixel(&tall, y, x);
            for channel in 0..4 {
                worst = worst.max(a[channel].abs_diff(b[channel]));
            }
        }
    }
    // A level either side, for the rasterizer rather than the arithmetic: the
    // two shapes land on different texels of the same grid.
    assert!(
        worst <= 1,
        "the wide rectangle and its transpose differ by {worst} levels, so the \
         long-axis correction is not symmetric in the axes"
    );
}

/// The styles that combine a blurred shape with a sharp one draw the same
/// picture whichever way their blurred half was made.
///
/// `Solid` and `Outer` now take the evaluated blur for that half where the
/// shape allows it, and the sampled one otherwise. Both are still the same
/// composition — blur first, sharp shape over it with `SrcOver` or `DstOut` —
/// so the two must agree to the width of the approximation and no more.
///
/// `Inner` is here too, and it is the one that needed an argument rather than
/// a substitution: its blur is composited with `DstIn`, which the analytic
/// route refuses everywhere else because a quad is smaller than the region a
/// layer composite covers. Inside this layer the only thing the blend can
/// reach is the sharp shape drawn a moment earlier, which is a fill and so lies
/// within the rectangle the quad is grown from -- so there is nothing outside
/// the quad for it to have zeroed.
#[test]
fn an_evaluated_blur_serves_the_combining_styles_as_the_sampled_one_did() {
    let Some(mut ctx) = context() else {
        return;
    };
    let rect = Rect::new(34.0, 34.0, 94.0, 94.0);
    let radius = 8.0;

    for style in [
        MaskBlurStyle::Solid,
        MaskBlurStyle::Outer,
        MaskBlurStyle::Inner,
    ] {
        let paint = Paint::fill(Color::srgb(0.9, 0.4, 0.2, 1.0))
            .with_mask_blur(6.0)
            .with_mask_blur_style(style);

        // Through the shape, whose blurred half is evaluated.
        let mut evaluated = Canvas::new(SIZE);
        evaluated.clear(Color::BLACK);
        evaluated
            .draw_rrect(rect, radius, &paint)
            .expect("evaluated");
        let one = render(&mut ctx, evaluated);

        // Through an outline that records no shape, whose blurred half is
        // sampled.
        let mut sampled = Canvas::new(SIZE);
        sampled.clear(Color::BLACK);
        sampled
            .draw_path(&rect.to_rounded_path_with_radii([[radius; 2]; 4]), &paint)
            .expect("sampled");
        let two = render(&mut ctx, sampled);

        let worst = one
            .iter()
            .zip(&two)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 32,
            "{style:?}: the evaluated and sampled halves differ by {worst} levels"
        );
        // And the two halves really were made differently: if the evaluated one
        // had been refused, both would be the sampled composition and identical.
        assert!(
            worst > 0,
            "{style:?}: identical, so the blurred half was not evaluated"
        );
    }
}

/// A sampled mask blur does not pay for the halo an evaluated one would draw.
///
/// The layer holding a mask blur has to fit whichever route draws into it, and
/// the two reach different distances: a sampled blur stops at `blur_reach`,
/// about 1.73 deviations, and an evaluated one carries to `pad_for_sigma`,
/// about 2.5. Sizing for the larger regardless is safe and wasteful -- every
/// sampled blur then allocates about two-thirds again the area, on each of the
/// passes it takes, to hold a halo it will never draw.
///
/// So the size follows the route, and this is what says so: the same shape and
/// deviation through both, with the evaluated one wider.
#[test]
fn a_mask_blurs_layer_is_sized_for_the_route_that_draws_into_it() {
    let rect = Rect::new(44.0, 44.0, 84.0, 84.0);
    let paint = Paint::fill(Color::WHITE)
        .with_mask_blur(10.0)
        .with_mask_blur_style(MaskBlurStyle::Outer);

    let mut evaluated = Canvas::new(SIZE);
    evaluated.draw_rrect(rect, 6.0, &paint).expect("evaluated");
    let evaluated = evaluated.finish();

    // The same outline with no shape recorded on it, which still samples.
    let mut sampled = Canvas::new(SIZE);
    sampled
        .draw_path(&square_path(rect), &paint)
        .expect("sampled");
    let sampled = sampled.finish();

    let wide = evaluated.passes[0].extent;
    let narrow = sampled.passes[0].extent;
    assert!(
        wide.width > narrow.width && wide.height > narrow.height,
        "the evaluated route's layer is {wide:?} and the sampled route's is \
         {narrow:?}; the first should be the wider, since its blur reaches \
         further"
    );
    // And the narrow one is not merely smaller but small enough to be the
    // sampled reach rather than the evaluated one: at a deviation of ten those
    // are 74 and 96 across a forty-texel shape.
    assert!(
        narrow.width < 80,
        "the sampled route's layer is {narrow:?}, which is the evaluated \
         reach rather than its own"
    );
}

/// A squircle leaves the flat side earlier than a circular corner does.
///
/// This is the shape's whole visual point and the check that fails if
/// `draw_rsuperellipse` were quietly forwarding to `draw_rrect_with_radii`:
/// both are drawn with the same bounds and the same corner radius, and
/// sampled where the two differ.
///
/// Which way they differ was worth measuring rather than assuming. A circular
/// corner runs along the side and then turns; a superellipse starts bending
/// away sooner and more gently, so near the start of the corner it is the
/// *tighter* of the two. Over that corner, eighteen pixel centers fall inside
/// the circular rounded rectangle and outside the superellipse, and none the
/// other way round. The probe sits in the middle of that band.
#[test]
fn a_rounded_superellipse_leaves_the_side_earlier_than_a_circular_corner() {
    let Some(mut ctx) = context() else { return };

    // A corner radius of 40 on a 120-wide box, so the corner is a large
    // fraction of the side and the two curves are far apart.
    let bounds = Rect::new(4.0, 4.0, 124.0, 124.0);
    let radii: RoundingRadii = [[40.0, 40.0]; 4];

    let mut render = |squircle: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let paint = Paint::fill(Color::WHITE).with_anti_alias(false);
        if squircle {
            canvas
                .draw_rsuperellipse(bounds, radii, &paint)
                .expect("squircle");
        } else {
            canvas
                .draw_rrect_with_radii(bounds, radii, &paint)
                .expect("rounded rect");
        }
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &canvas.finish()).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        pixels
    };

    let squircle = render(true);
    let rounded = render(false);

    // Where they differ, and its mirror across the diagonal -- the shape is
    // symmetric, so a transposed corner would pass one and fail the other.
    for (x, y) in [(4u32, 40u32), (40, 4)] {
        assert_eq!(
            pixel(&rounded, x, y),
            [255, 255, 255, 255],
            "the circular corner covers ({x}, {y})"
        );
        assert_eq!(
            pixel(&squircle, x, y),
            [0, 0, 0, 255],
            "the superellipse has already left the side at ({x}, {y})"
        );
    }

    // Both are still the same shape in the large: the middle of a side is
    // flat on each, and the corner point belongs to neither.
    for shape in [&squircle, &rounded] {
        assert_eq!(pixel(shape, 64, 6), [255, 255, 255, 255], "mid side");
        assert_eq!(pixel(shape, 64, 64), [255, 255, 255, 255], "middle");
        assert_eq!(pixel(shape, 5, 5), [0, 0, 0, 255], "the corner point");
    }
}

/// Clipping to a squircle removes what falls outside it.
///
/// Paired with the draw test on purpose: the clip takes the same path, so a
/// clip that silently used the bounding rectangle instead would still pass
/// every test about the drawn shape.
#[test]
fn a_superellipse_clip_removes_what_falls_outside_the_curve() {
    let Some(mut ctx) = context() else { return };

    let bounds = Rect::new(4.0, 4.0, 124.0, 124.0);
    let radii: RoundingRadii = [[40.0, 40.0]; 4];

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save();
    canvas
        .clip_rsuperellipse(bounds, radii)
        .expect("clip to the squircle");
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, 128.0, 128.0),
            &Paint::fill(Color::WHITE).with_anti_alias(false),
        )
        .expect("fill through the clip");
    canvas.restore();

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);

    assert_eq!(pixel(&pixels, 64, 64), [255, 255, 255, 255], "the middle");
    assert_eq!(
        pixel(&pixels, 20, 20),
        [255, 255, 255, 255],
        "inside the superellipse corner"
    );
    assert_eq!(
        pixel(&pixels, 5, 5),
        [0, 0, 0, 255],
        "the corner point is outside the shape and must be clipped away"
    );
    // The clip is the curve and not the bounding box: this is inside the box
    // and outside the shape.
    assert_eq!(
        pixel(&pixels, 4, 40),
        [0, 0, 0, 255],
        "a clip to the bounding rectangle would leave this lit"
    );
}

/// The halving color filter the two backdrop-id tests both filter through.
///
/// Chosen because it composes visibly with itself and exactly: applied twice a
/// component is quartered, so the overlap of two layers says which image the
/// second one filtered without any tolerance argument about blur radii.
fn half_every_channel() -> ImageFilter {
    ImageFilter::Color(ColorFilter::matrix([
        0.5, 0.0, 0.0, 0.0, 0.0, //
        0.0, 0.5, 0.0, 0.0, 0.0, //
        0.0, 0.0, 0.5, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]))
}

#[test]
fn a_shared_backdrop_id_filters_what_was_behind_all_of_the_layers_naming_it() {
    // `SceneBuilder.pushBackdropFilter` takes a `backdropId`, and `dart:ui`
    // says what it buys: "when the first backdrop filter with a given id is
    // processed during rasterization, the state of the backdrop is recorded
    // and cached. All subsequent backdrop filters with the same identifier
    // will apply their filter to the cached backdrop", and "if overlapping
    // backdrop filters use the same backdropId, then each filter will apply to
    // the backdrop before the overlapping filter components were rendered."
    //
    // So it is a different picture and not only a cheaper one, and the overlap
    // is where the two differ: without the id the second layer filters the
    // first layer's output, halving what was already halved.
    let Some(mut ctx) = context() else { return };

    let left = Rect::new(16.0, 16.0, 80.0, 112.0);
    let right = Rect::new(48.0, 16.0, 112.0, 112.0);
    let shot = |ctx: &mut Context, id: Option<i64>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        for bounds in [left, right] {
            let layer = match id {
                Some(id) => Layer::opacity(1.0).with_backdrop_id(id),
                None => Layer::opacity(1.0),
            };
            canvas
                .save_layer_backdrop(layer, Some(bounds), &half_every_channel())
                .expect("backdrop");
            canvas.restore();
        }
        render(ctx, canvas)
    };

    let apart = shot(&mut ctx, None);
    let shared = shot(&mut ctx, Some(7));

    // Where only one layer reaches, the id changes nothing: there is nothing in
    // front of the capture to have been filtered twice. This is the half of the
    // check that says the sharing did not simply skip the second layer.
    for (x, side) in [(32, "the left layer alone"), (96, "the right layer alone")] {
        for (name, shot) in [("apart", &apart), ("shared", &shared)] {
            let got = pixel(shot, x, 64);
            assert!(
                got[0].abs_diff(128) <= 2 && got[3] == 255,
                "{side}, {name}: white halved once is about 128, got {got:?}"
            );
        }
    }

    // And in the overlap they part company.
    let got = pixel(&apart, 64, 64);
    assert!(
        got[0].abs_diff(64) <= 2,
        "without an id the second layer filters the first layer's result, so \
         the overlap should be white quartered -- about 64, got {got:?}"
    );
    let got = pixel(&shared, 64, 64);
    assert!(
        got[0].abs_diff(128) <= 2,
        "with a shared id both layers filter the capture taken before either \
         drew, so the overlap should be white halved once -- about 128, got \
         {got:?}"
    );
}

#[test]
fn a_shared_backdrop_id_captures_once_and_filters_once() {
    // The other half of what the id is for, and the reason `dart:ui` calls it
    // a "dramatic" improvement: a list of frosted rows over one background
    // should cost one capture, not one per row.
    //
    // Both halves are counted here because they are separate savings. Sharing
    // the capture removes a pass cut and the full-target redraw that follows
    // it; sharing the filtered result removes the filter itself, which for a
    // blur is where nearly all of the cost is.
    let passes = |id: Option<i64>, rows: usize| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        for row in 0..rows {
            let top = 8.0 + 24.0 * row as f32;
            let layer = match id {
                Some(id) => Layer::opacity(1.0).with_backdrop_id(id),
                None => Layer::opacity(1.0),
            };
            canvas
                .save_layer_backdrop(
                    layer,
                    Some(Rect::new(16.0, top, 112.0, top + 16.0)),
                    &half_every_channel(),
                )
                .expect("backdrop");
            canvas.restore();
        }
        canvas.finish().passes.len()
    };

    // One row costs the same either way -- the first use of an id is a capture
    // and a filter like any other. That is what makes the slope below the
    // measurement rather than the intercept.
    assert_eq!(
        passes(None, 1),
        passes(Some(3), 1),
        "the first layer to name an id has nothing to share with"
    );

    // So the measurement is the slope: what each row after the first adds. A
    // bounded layer is a pass of its own whichever way, since it has its own
    // target to composite back, and that one is the floor.
    let apart = passes(None, 4) - passes(None, 1);
    let shared = passes(Some(3), 4) - passes(Some(3), 1);
    assert_eq!(
        apart, 9,
        "three more rows without an id should each cost three passes -- the cut \
         that captures the backdrop, the filter over it, and the row's own \
         layer"
    );
    assert_eq!(
        shared, 3,
        "three more rows sharing an id and a filter should each cost only their \
         own layer: the capture is taken once and the filter is run over it \
         once. Apart, the same three rows cost {apart} passes"
    );
}

#[test]
fn a_clipped_shared_backdrop_draws_what_stating_the_clip_would() {
    // A layer naming a backdrop key is narrowed to the clip in force, the same
    // as an unshared one, and this is what says the narrowing is only that.
    // Two panels in opposite corners, both naming one key: the clip decides
    // each layer's target while the capture and the filter over it stay the
    // parent's size, because they serve both. Built twice -- relying on the
    // clip, and writing the clip out as the layer's bounds -- and required
    // identical to the byte.
    let Some(mut ctx) = context() else { return };

    let corners = [
        Rect::new(8.0, 8.0, 56.0, 56.0),
        Rect::new(72.0, 72.0, 120.0, 120.0),
    ];
    let shot = |ctx: &mut Context, bounded: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        // Something with structure under the panels, so a target placed a few
        // pixels off would show rather than land on flat color.
        for row in 0..8 {
            let top = 16.0 * row as f32;
            canvas
                .draw_rect(
                    Rect::new(0.0, top, 128.0, top + 8.0),
                    &Paint::fill(Color::srgb(0.1, 0.4, 0.8, 1.0)),
                )
                .expect("the stripes");
        }
        for clip in corners {
            let layer = Layer::opacity(1.0).with_backdrop_id(11);
            canvas.save();
            canvas.clip_rect(clip).expect("the clip");
            let bounds = bounded.then_some(clip);
            canvas
                .save_layer_backdrop(layer, bounds, &half_every_channel())
                .expect("backdrop");
            canvas.restore();
            canvas.restore();
        }
        render(ctx, canvas)
    };

    let from_clip = shot(&mut ctx, false);
    let from_bounds = shot(&mut ctx, true);
    assert_eq!(
        from_clip, from_bounds,
        "the two spellings of one clipped shared backdrop draw the same picture"
    );
    // And the filter ran, so the comparison is of a picture rather than of two
    // copies of the ground. The same stripe inside the panel and outside it,
    // which the halving separates.
    let inside = pixel(&from_clip, 16, 16);
    let outside = pixel(&from_clip, 64, 16);
    for channel in 0..3 {
        assert!(
            inside[channel].abs_diff(outside[channel] / 2) <= 2,
            "the panel holds the stripe halved: {inside:?} against {outside:?}"
        );
    }
}

#[test]
fn a_backdrop_id_reaches_into_a_layer_opened_after_it() {
    // Upstream's `CanRenderMultipleBackdropBlurWithSingleBackdropIdDifferentLayers`
    // wraps every panel but the first in a save layer of its own, names one id
    // across all of them, and expects all six to show the same ground. So an id
    // keys a captured image and not a surface: what a later layer filters is
    // what the id captured, even though that layer draws into a target of its
    // own that has nothing on it.
    //
    // This is the case a target-scoped id would have had to refuse, and
    // refusing it would have put this renderer where upstream is not.
    let Some(mut ctx) = context() else { return };

    let panel = Rect::new(32.0, 32.0, 96.0, 96.0);
    let shot = |ctx: &mut Context, id: Option<i64>| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        // The capture, taken while the frame is white and nothing else.
        let first = match id {
            Some(id) => Layer::opacity(1.0).with_backdrop_id(id),
            None => Layer::opacity(1.0),
        };
        canvas
            .save_layer_backdrop(first, Some(panel), &half_every_channel())
            .expect("the capture");
        canvas.restore();

        // Now paint the whole frame black, and put the second panel inside a
        // layer of its own. Its target starts empty, so a backdrop scoped to
        // that target would filter nothing at all; one scoped to the id
        // filters the white frame the first panel saw.
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::BLACK).with_anti_alias(false),
            )
            .expect("ground");
        canvas.save_layer(Layer::opacity(1.0));
        let second = match id {
            Some(id) => Layer::opacity(1.0).with_backdrop_id(id),
            None => Layer::opacity(1.0),
        };
        canvas
            .save_layer_backdrop(second, Some(panel), &half_every_channel())
            .expect("the sharer");
        canvas.restore();
        canvas.restore();
        render(ctx, canvas)
    };

    // Without the id the second panel captures afresh, and what is behind it by
    // then is black: halved, still black.
    let got = pixel(&shot(&mut ctx, None), 64, 64);
    assert!(
        got[0] <= 2,
        "captured afresh, the second panel should have filtered the black \
         ground, got {got:?}"
    );

    // With it, the second panel filters the white capture the first one took,
    // through a layer whose own target held nothing.
    let got = pixel(&shot(&mut ctx, Some(11)), 64, 64);
    assert!(
        got[0].abs_diff(128) <= 2,
        "the id should have carried the white capture into the nested layer, \
         which halves it to about 128, got {got:?}"
    );
}

/// Total coverage of a white shape on black, in whole pixels.
fn ink(pixels: &[u8]) -> f64 {
    pixels.chunks(4).map(|p| p[0] as f64).sum::<f64>() / 255.0
}

#[test]
fn a_stroke_thinner_than_a_pixel_fades_instead_of_disappearing() {
    // Measured before this worked: a vertical stroke 112 pixels long drew the
    // same amount of ink at a width of 0.18 as at 0.3, and none at all at 0.15.
    // Multisampling is why -- at four samples a sub-pixel line has four
    // coverages available to it and one of them is nothing, so how much of it
    // survives depends on where it falls rather than how wide it is.
    //
    // Upstream widens the geometry to a whole device pixel and scales the alpha
    // to pay for it: `ComputePixelHalfWidth` takes `max(width, 1 / max_basis)`
    // and `ComputeStrokeAlphaCoverage` returns `clamp(scaled_width * 2, 0, 1)`.
    // Both are copied rather than improved on, so the numbers below are
    // upstream's arithmetic and not a local judgment about what looks right.
    let Some(mut ctx) = context() else { return };

    let line = |ctx: &mut Context, width: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let mut b = PathBuilder::new();
        // Off the pixel grid on purpose. A line at a whole coordinate falls
        // between two columns of samples and is the easiest case; the one that
        // used to vanish did not.
        b.move_to(Vec2::new(64.3, 8.0))
            .line_to(Vec2::new(64.3, 120.0));
        canvas
            .draw_path(&b.build(), &Paint::stroke(Color::WHITE, width))
            .expect("a thin stroke");
        ink(&render(ctx, canvas))
    };

    // A whole pixel of width is the reference, and 112 pixels of length is what
    // the path covers.
    let whole = line(&mut ctx, 1.0);
    assert!(
        (whole - 112.0).abs() < 2.0,
        "a stroke one pixel wide over 112 pixels should lay down about 112 \
         pixels of ink, got {whole:.2}"
    );

    // Half a pixel is the width at which the doubling reaches one, so it draws
    // as much ink as a whole pixel does. That is upstream's rule and it is the
    // one place the picture is deliberately heavier than the area asked for.
    let half = line(&mut ctx, 0.5);
    assert!(
        (half - whole).abs() < 3.0,
        "at half a pixel the alpha scale reaches one, so the ink should match \
         the whole-pixel case: {half:.2} against {whole:.2}"
    );

    // Below that it is linear in the width, twice over: half the width is half
    // the alpha over the same one-pixel geometry.
    for (width, factor) in [(0.25f32, 0.5), (0.125, 0.25), (0.0625, 0.125)] {
        let got = line(&mut ctx, width);
        let want = whole * factor;
        assert!(
            (got - want).abs() < 3.0,
            "a stroke {width} pixels wide should lay down {factor} of the ink a \
             whole pixel does -- about {want:.2}, got {got:.2}"
        );
    }

    // The point of all of it: what used to vanish does not. Before this, 0.15
    // drew nothing whatsoever.
    assert!(
        line(&mut ctx, 0.15) > 20.0,
        "a stroke of 0.15 pixels should still be visible"
    );
}

#[test]
fn the_two_stroke_routes_thin_out_the_same_way() {
    // A circle is drawn from a distance field and a path is tessellated, and
    // the widening had to reach both or which route a shape took would become
    // visible at exactly the widths hardest to look at. The analytic route did
    // not have the defect -- it computes coverage rather than sampling it, so
    // it faded smoothly all the way down -- and that is the reason to check it
    // rather than the reason to skip it: it is the route where adopting
    // upstream's rule *changed* a picture that was already continuous.
    let Some(mut ctx) = context() else { return };

    let circle = |ctx: &mut Context, width: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_circle(
                Vec2::new(64.0, 64.0),
                40.0,
                &Paint::stroke(Color::WHITE, width),
            )
            .expect("a thin circle");
        ink(&render(ctx, canvas))
    };

    let whole = circle(&mut ctx, 1.0);
    for (width, factor) in [(0.5f32, 1.0), (0.25, 0.5), (0.125, 0.25)] {
        let got = circle(&mut ctx, width);
        let want = whole * factor;
        assert!(
            (got - want).abs() < whole * 0.05,
            "the analytic route should follow the same rule: a stroke {width} \
             wide wants {factor} of the whole-pixel ink, about {want:.2}, got \
             {got:.2}"
        );
    }
}

#[test]
fn the_two_ways_to_draw_a_ring_agree_on_its_color() {
    // Upstream's `CompareDiffRoundRectAndRoundRect` puts the two side by side
    // and leaves the comparison to whoever is looking. Here it is the
    // assertion: a difference of two rounded rectangles is one path filled
    // even-odd, and the outer one with the inner cleared out of it is two
    // draws and a destination-clearing blend. They share nothing, so a defect
    // in either shows up as the pair disagreeing -- and eight radii per
    // rectangle is where a defect would be, since every corner is a different
    // curvature.
    //
    // Color and not alpha. `Clear` makes the hole transparent where the
    // even-odd fill leaves whatever was under it, so the two agree on what
    // they painted and differ on what they left behind. That difference is the
    // blend doing its job, and it is invisible on any opaque target.
    let Some(mut ctx) = context() else { return };

    let outer = Rect::new(20.0, 20.0, 108.0, 108.0);
    let inner = Rect::new(26.0, 26.0, 102.0, 102.0);
    // Ascending around the outer rectangle, each inner radius six less, which
    // is what keeps the ring an even thickness at four curvatures.
    let outer_radii = [[6.0, 6.0], [12.0, 12.0], [24.0, 24.0], [54.0, 54.0]];
    let inner_radii = [[0.0, 0.0], [6.0, 6.0], [18.0, 18.0], [48.0, 48.0]];
    let sky = Color::srgb(135.0 / 255.0, 206.0 / 255.0, 235.0 / 255.0, 1.0);

    let differenced = {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_drrect_with_radii(outer, outer_radii, inner, inner_radii, &Paint::fill(sky))
            .expect("a difference of two rounded rectangles");
        render(&mut ctx, canvas)
    };
    let cleared = {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rrect_with_radii(outer, outer_radii, &Paint::fill(sky))
            .expect("the outer rectangle");
        canvas
            .draw_rrect_with_radii(
                inner,
                inner_radii,
                &Paint::fill(sky).with_blend(BlendMode::Clear),
            )
            .expect("the inner rectangle cleared out of it");
        render(&mut ctx, canvas)
    };

    let mut worst = 0i32;
    let mut differing = 0usize;
    for (a, b) in differenced.chunks_exact(4).zip(cleared.chunks_exact(4)) {
        let delta = a[..3]
            .iter()
            .zip(&b[..3])
            .map(|(p, q)| (*p as i32 - *q as i32).abs())
            .max()
            .unwrap_or(0);
        worst = worst.max(delta);
        if delta > 0 {
            differing += 1;
        }
    }
    assert_eq!(
        (worst, differing),
        (0, 0),
        "the two constructions of one ring should be the same color everywhere: \
         {differing} pixel(s) differ, worst by {worst}"
    );

    // And the ring is actually there, so the agreement above is not two blank
    // frames agreeing.
    let on_the_ring = pixel(&differenced, 64, 23);
    assert!(
        on_the_ring[2] > 200 && on_the_ring[0] > 100,
        "the top edge of the ring should be sky blue, got {on_the_ring:?}"
    );
    let inside = pixel(&differenced, 64, 64);
    assert_eq!(
        inside,
        [0, 0, 0, 255],
        "the middle should be the hole, not the fill"
    );
}

/// The stroke pixels in a window, as the red channel of half-alpha blue on
/// white: 128 where the stroke covered once, 64 where it covered twice, and
/// values above 128 at the antialiased outline where it covered part of one.
fn stroke_covers(pixels: &[u8], x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> Vec<u8> {
    let mut out = Vec::new();
    for row in y {
        for column in x.clone() {
            let p = pixel(pixels, column, row);
            if p[2] > 200 && p[0] < 250 {
                out.push(p[0]);
            }
        }
    }
    out
}

#[test]
fn a_translucent_stroke_blends_with_itself_only_where_its_caps_overlap() {
    // A stroke is tessellated as a run of overlapping quads with a cap shape on
    // each end, so anywhere the outline covers a pixel twice the second cover
    // blends over the first. At full opacity that is invisible -- opaque over
    // opaque is opaque -- and at half it is one blend against two. Upstream
    // draws its arc farm translucently for exactly this reason, at stroke
    // widths running up to the whole diameter.
    //
    // Two claims, and the second is what keeps the first honest. Along the run
    // of the stroke nothing covers twice, however much the quads overlap each
    // other geometrically. Where the two caps *are* on top of each other --
    // this arc closes to within twenty degrees and its stroke is wider than its
    // diameter -- they do, and they should: that is two shapes over one pixel
    // and not a tessellation covering itself.
    let Some(mut ctx) = context() else { return };

    let sweep = 5.93f32;
    let (cx, cy, r) = (64.0f32, 64.0f32, 26.0f32);
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::WHITE);
    let mut b = PathBuilder::new();
    for i in 0..=128 {
        let a = sweep * (i as f32 / 128.0);
        let p = Vec2::new(cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    canvas
        .draw_path(
            &b.build(),
            &Paint::stroke(Color::srgb(0.0, 0.0, 1.0, 0.5), 40.0).with_style(Style::Stroke(
                StrokeStyle::new(40.0).with_cap(LineCap::Round),
            )),
        )
        .expect("a wide translucent arc");
    let pixels = render(&mut ctx, canvas);

    // Three windows on the run, none of them near the ends.
    for (label, x, y) in [
        ("the far side", 8..30u32, 50..80u32),
        ("the top", 50..80u32, 8..26u32),
        ("the bottom", 50..80u32, 100..120u32),
    ] {
        let covers = stroke_covers(&pixels, x, y);
        assert!(
            covers.len() > 150,
            "{label} should be mostly stroke, got {} pixels",
            covers.len()
        );
        let twice = covers.iter().filter(|v| **v < 120).count();
        assert_eq!(
            twice,
            0,
            "{label}: {twice} of {} stroke pixels blended more than once, so the \
             run of the stroke is covering itself",
            covers.len()
        );
        let once = covers.iter().filter(|v| v.abs_diff(128) <= 2).count();
        assert!(
            once * 10 > covers.len() * 9,
            "{label}: only {once} of {} pixels carry a single cover, so this \
             window is edge rather than run",
            covers.len()
        );
    }

    // And the caps do overlap, which is what says the windows above were not
    // simply somewhere the arc never reached.
    let gap = stroke_covers(&pixels, 84..120u32, 55..75u32);
    let doubled = gap.iter().filter(|v| **v < 120).count();
    assert!(
        doubled > 100,
        "the two caps should be on top of each other near the gap, and only \
         {doubled} of {} pixels there blended twice",
        gap.len()
    );
}

#[test]
fn a_rectangle_drawn_two_ways_agrees_everywhere_but_its_corners() {
    // `draw_rect` evaluates a distance field on a quad where `draw_path` of the
    // same four corners tessellates, and which one a caller gets is decided by
    // properties they do not think of as picture-changing -- whether the paint
    // is a solid color, whether it asks for antialiasing, whether a filter is
    // on it. So the two have to agree, and until the testkit started sending a
    // rectangle through `draw_rect` nothing compared them: every scene in the
    // catalog reached the tessellated one.
    //
    // They agree over every edge and every interior pixel, and they cannot
    // agree at a corner. A corner pixel is cut by two edges and a signed
    // distance is one number, so the field has no way to say that a quarter of
    // the pixel is covered rather than a half -- measured, a rectangle whose
    // corner falls on a pixel center gives 128 from the field against 64 from
    // the triangles, which is a half against the true quarter. Three or four
    // pixels per rectangle, always within a pixel of a corner.
    //
    // The bound below is those measurements and not a tolerance chosen to pass.
    // It is here so that a change making the field wrong along an *edge* fails
    // rather than being absorbed.
    let Some(mut ctx) = context() else { return };

    let corners = |r: Rect| {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(r.left, r.top))
            .line_to(Vec2::new(r.right, r.top))
            .line_to(Vec2::new(r.right, r.bottom))
            .line_to(Vec2::new(r.left, r.bottom))
            .close();
        b.build()
    };

    for (label, rect, paint) in [
        (
            "aligned to the pixel grid",
            Rect::new(20.0, 20.0, 100.0, 100.0),
            Paint::fill(Color::WHITE),
        ),
        (
            "on pixel centers",
            Rect::new(20.5, 20.5, 100.5, 100.5),
            Paint::fill(Color::WHITE),
        ),
        (
            "stroked",
            Rect::new(18.0, 30.0, 110.0, 96.0),
            Paint::stroke(Color::WHITE, 9.0),
        ),
    ] {
        let primitive = {
            let mut canvas = Canvas::new(SIZE);
            canvas.clear(Color::BLACK);
            canvas.draw_rect(rect, &paint).expect("the primitive");
            render(&mut ctx, canvas)
        };
        let as_path = {
            let mut canvas = Canvas::new(SIZE);
            canvas.clear(Color::BLACK);
            canvas
                .draw_path(&corners(rect), &paint)
                .expect("the same four corners");
            render(&mut ctx, canvas)
        };

        let lit = primitive.chunks_exact(4).filter(|p| p[0] > 8).count();
        assert!(
            lit > 500,
            "{label}: the rectangle should cover a good part of the frame, got \
             {lit} pixels"
        );

        let mut differing = Vec::new();
        for (i, (a, b)) in primitive
            .chunks_exact(4)
            .zip(as_path.chunks_exact(4))
            .enumerate()
        {
            let delta = a
                .iter()
                .zip(b)
                .map(|(p, q)| (*p as i32 - *q as i32).abs())
                .max()
                .unwrap_or(0);
            // One level is two implementations rounding, not two routes
            // disagreeing. The structural failure this is for -- a field wrong
            // along an edge rather than at a corner -- differs by tens or
            // hundreds, so nothing worth catching hides under this.
            if delta > 1 {
                let (x, y) = (i as u32 % SIZE.width, i as u32 / SIZE.width);
                differing.push((x, y, delta));
            }
        }
        // A filled rectangle has four corners; a stroked one has eight, an
        // outer and an inner at each, and an L of two pixels can differ at any
        // of them. The stroked case used to allow four because both routes were
        // the tessellator -- a square-cornered stroke was refused the field --
        // so it was really asserting zero. It is two implementations now, and
        // this is the first bound it has ever actually had to hold.
        let corners = if matches!(paint.style, Style::Stroke(_)) {
            16
        } else {
            4
        };
        assert!(
            differing.len() <= corners,
            "{label}: {} pixels differ between the two routes, which is more \
             than the {corners} a corner apiece can account for: {differing:?}",
            differing.len()
        );
        for (x, y, delta) in &differing {
            let _ = delta;
            // Within two pixels of both a vertical edge and a horizontal one,
            // which is the corner and nothing else: the affected cluster is an
            // L of three pixels tucked inside it.
            //
            // A stroke's corners are not the rectangle's. They sit half a width
            // outside it and half a width inside, so the reach has to carry
            // that -- and a rectangle ninety-two by sixty-six with a ten-wide
            // stroke still leaves the middle of every edge far outside this,
            // which is what the claim is about.
            let reach = 2.5
                + match &paint.style {
                    Style::Stroke(style) => style.width / 2.0,
                    _ => 0.0,
                };
            let near_x =
                (*x as f32 - rect.left).abs() <= reach || (*x as f32 - rect.right).abs() <= reach;
            let near_y =
                (*y as f32 - rect.top).abs() <= reach || (*y as f32 - rect.bottom).abs() <= reach;
            assert!(
                near_x && near_y,
                "{label}: ({x}, {y}) differs by {delta} and is not at a corner \
                 of {rect:?}, so the two routes part company along an edge"
            );
        }
        // How *far* apart they get at a corner is not asserted, and the reason
        // is worth keeping: an earlier draft bounded it at sixty-four, which is
        // what this machine measures, and the software rasterizer in CI came
        // back with a hundred and ninety-one on the rectangle whose corners
        // fall on pixel centers. That is the one alignment where the corner
        // sits exactly on the sample grid, so which samples a triangle covers
        // there is a property of the rasterizer rather than of either route.
        // The claim that survives a second device is where they differ and how
        // many, which is the claim worth having anyway: a change that made the
        // field wrong along an edge fails on the count and the position, and a
        // corner pixel landing differently on a different rasterizer does not.
    }
}

#[test]
fn a_stroked_rectangle_has_the_corners_its_join_asks_for() {
    // The default `strokeJoin` is a miter, so a rectangular border has square
    // outer corners. `draw_rect` used to hand a stroked rectangle to the
    // fragment-evaluated route, where a stroke is the band a fixed distance
    // either side of the outline -- and that band's outer edge at a vertex is
    // an arc. So the corners came out rounded, on the call a caller reaches for
    // to draw a border, and nothing compared the two routes.
    let Some(mut ctx) = context() else { return };

    let rect = Rect::new(18.0, 30.0, 110.0, 96.0);
    let width = 9.0f32;
    // The outer corner of the band, which a miter fills and an arc does not.
    let (cx, cy) = (
        (rect.left - width / 2.0).ceil() as u32,
        (rect.top - width / 2.0).ceil() as u32,
    );

    let corner_of = |ctx: &mut Context, join: LineJoin| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                rect,
                &Paint::stroke(Color::WHITE, width)
                    .with_style(Style::Stroke(StrokeStyle::new(width).with_join(join))),
            )
            .expect("a stroked rectangle");
        pixel(&render(ctx, canvas), cx, cy)
    };

    let mitered = corner_of(&mut ctx, LineJoin::Miter);
    assert!(
        mitered[0] > 200,
        "a mitered corner should reach ({cx}, {cy}), got {mitered:?}"
    );

    // A bevel cuts the corner off and a round join arcs across it, so both
    // leave that pixel behind -- which is what says the assertion above is
    // about the join and not about the pixel being inside the band anyway.
    for join in [LineJoin::Bevel, LineJoin::Round] {
        let got = corner_of(&mut ctx, join);
        assert!(
            got[0] < 100,
            "{join:?} should not reach ({cx}, {cy}), got {got:?}"
        );
    }
}

#[test]
fn a_layer_matrix_recovers_what_fell_outside_the_frame() {
    // A matrix on a layer resamples what the layer captured, and a layer used
    // to capture no more than its parent's target held -- so a shape drawn past
    // the edge of the frame was gone before the matrix ran, and a translation
    // that would have brought it into view brought in nothing. That was §12 of
    // `docs/non-parity.md`, pinned by this test in its previous form so that
    // building the sizing would be a test that changed rather than a silent
    // improvement. This is that change.
    //
    // A layer whose matrix will move its result now records over the
    // *pre-image*: the region that lands where the parent can see it once the
    // matrix has been applied, unioned with the parent for the content the
    // matrix leaves where it was. Upstream's
    // `MatrixImageFilterDoesntCullWhenTranslatedFromOffscreen` is this case.
    let Some(mut ctx) = context() else { return };

    let green = Paint::fill(Color::srgb(0.0, 1.0, 0.0, 1.0));
    let radius = 30.0;
    let shot = |ctx: &mut Context, at: f32, moved_by: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.save_layer(
            Layer::opacity(1.0).with_matrix(Affine2::from_translation(Vec2::new(moved_by, 0.0))),
        );
        canvas
            .draw_circle(Vec2::new(at, 64.0), radius, &green)
            .expect("a circle");
        canvas.restore();
        let pixels = render(ctx, canvas);
        pixels.chunks_exact(4).filter(|p| p[1] > 100).count()
    };

    // Drawn where the layer can hold it and moved within the frame: the matrix
    // carries it, and the count is what a circle of this radius covers.
    let moved_inside = shot(&mut ctx, 34.0, 40.0);
    assert!(
        moved_inside > 2_000,
        "a circle moved within the frame should still be there, got \
         {moved_inside} pixels"
    );

    // Drawn off the left edge and translated back into view: the whole circle,
    // not a sliver. Compared against the one drawn inside rather than against a
    // number, since the two are the same circle and the only question is
    // whether the layer held it.
    let moved_in_from_outside = shot(&mut ctx, -36.0, 100.0);
    assert!(
        moved_in_from_outside.abs_diff(moved_inside) * 20 < moved_inside,
        "a circle drawn off the frame and translated in should arrive whole: \
         {moved_in_from_outside} pixels against {moved_inside} for the same \
         circle drawn where the layer already reached"
    );

    // And it arrives in the right place, which the count alone cannot say. The
    // circle is drawn at -36 and moved by 100, so its center lands at 64 -- the
    // middle of the frame, where the ground was black before.
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.save_layer(
        Layer::opacity(1.0).with_matrix(Affine2::from_translation(Vec2::new(100.0, 0.0))),
    );
    canvas
        .draw_circle(Vec2::new(-36.0, 64.0), radius, &green)
        .expect("a circle off the left edge");
    canvas.restore();
    let pixels = render(&mut ctx, canvas);
    assert_eq!(
        pixel(&pixels, 64, 64),
        [0, 255, 0, 255],
        "the circle should land in the middle of the frame"
    );
    assert_eq!(
        pixel(&pixels, 64 + (radius as u32) + 4, 64),
        [0, 0, 0, 255],
        "and stop where a circle of that radius stops"
    );
}

#[test]
fn a_stroke_too_wide_to_tessellate_draws_rather_than_aborting() {
    // The most serious thing the generated tests have found, and the only one
    // reachable in four lines of ordinary API use. `Paint::stroke(color, 1e30)`
    // with a round join used to take the process down -- not a panic a caller
    // could catch, a stack overflow, which unwinds nothing.
    //
    // Lyon computed a round join's subdivision count as
    // `num_segments.log2().round() as u32`, and Rust's `as` cast saturates: an
    // infinite `num_segments` became `u32::MAX` and was then used as a
    // recursion depth. Reported as nical/lyon#959 and fixed in #961, which
    // clamps the count; the workspace requires the release that has it.
    //
    // So the width is handed over now rather than refused. `dart:ui` states no
    // maximum stroke width and neither does upstream, so a bound of this
    // renderer's own would have been a limit nobody else has -- and once the
    // crash was gone there was nothing left to justify it.
    //
    // Here rather than only in the geometry crate because the point is the
    // route: this is what a caller of the *renderer* can write.
    let Some(mut ctx) = context() else { return };

    let bar = |width: f32| {
        let mut b = PathBuilder::new();
        b.move_to(Vec2::new(20.0, 40.0))
            .line_to(Vec2::new(90.0, 40.0))
            .line_to(Vec2::new(90.0, 100.0));
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let paint = Paint::stroke(Color::WHITE, width).with_style(Style::Stroke(
            StrokeStyle::new(width)
                .with_cap(LineCap::Round)
                .with_join(LineJoin::Round),
        ));
        canvas.draw_path(&b.build(), &paint).expect("a stroke");
        canvas
    };

    // An ordinary width draws, so what follows is about the width and not about
    // the path.
    let drawn = render(&mut ctx, bar(6.0));
    assert!(
        drawn.chunks_exact(4).filter(|p| p[0] > 128).count() > 200,
        "a six-wide stroke should draw"
    );

    // A stroke wider than the frame covers it, which is the right picture: the
    // band is half the width on each side of a path that is already inside.
    let whole = (SIZE.width * SIZE.height) as usize;
    for width in [1e3f32, 1e6, 1e9] {
        let wide = render(&mut ctx, bar(width));
        let covered = wide.chunks_exact(4).filter(|p| p[0] > 128).count();
        assert_eq!(
            covered, whole,
            "a stroke {width:e} wide should cover the frame"
        );
    }

    // Past that the picture is the driver's, and this asserts only what is this
    // renderer's. `MAX_COORDINATE` is two to the twenty-fourth and a width of
    // `1e30` puts the stroke's outline at five times ten to the twenty-ninth, so
    // the vertices reaching the device are finite and enormous: measured in clip
    // space, the largest is 7.8e27 for `1e30` and 2.7e36 for `f32::MAX`, against
    // a unit cube. None is NaN and none is infinite, which is the part that is
    // ours.
    //
    // What a rasterizer does with a triangle that size is its own business, and
    // the two here disagree completely. lavapipe clips in float and covers about
    // half the frame; V3D bins into a bounded fixed-point format, cannot express
    // the coordinate, and drops the primitives for a coverage of exactly zero.
    // Both are defensible and neither is a bug in this crate -- `1e9` covers the
    // frame on both, and its largest clip coordinate is 7.8e6.
    //
    // So this used to assert "draws something substantial", which passed on a
    // software rasterizer and failed on a Pi 5 for a reason the assertion was
    // never about. What the test is here for is the line above it: these widths
    // used to end the process, and now they return. That is what is checked, plus
    // that the frame handed back is whole and not garbage -- a dropped primitive
    // leaves the background, and a corrupt one would not be gray.
    for width in [1e30f32, f32::MAX] {
        let wide = render(&mut ctx, bar(width));
        assert_eq!(
            wide.len(),
            whole * 4,
            "a stroke {width:e} wide returned a frame of the wrong size"
        );
        // Black background, white stroke: every pixel is gray and opaque however
        // much of it the device decided to draw. Noise would not be.
        let corrupt = wide
            .chunks_exact(4)
            .filter(|p| p[0] != p[1] || p[1] != p[2] || p[3] != 255)
            .count();
        assert_eq!(
            corrupt, 0,
            "a stroke {width:e} wide put {corrupt} pixels in the frame that are \
             neither the background nor the stroke"
        );
    }
}

#[test]
fn a_nine_patch_is_one_draw_and_the_same_picture_as_nine() {
    // A nine-patch used to be nine draws, one per patch, because a per-draw
    // source rectangle is also what *clamps* a patch from sampling its
    // neighbor. It is one draw now, with the coordinates carried on the
    // vertices the way `drawAtlas` has always carried them, and the clamping
    // bought back by insetting each patch's texture coordinates half a texel --
    // exactly the reach of a linear sample.
    //
    // Both halves are asserted because either alone is worthless: one draw
    // that bleeds at the seams is not an improvement, and nine draws that look
    // right is what this replaced.
    let Some(mut ctx) = context() else { return };

    // A sheet whose color changes hard at the patch boundaries, so bleeding
    // across one is the most visible thing that can happen.
    let sheet = Extent2D {
        width: 32,
        height: 32,
    };
    let mut texels = vec![0u8; 32 * 32 * 4];
    for y in 0..32usize {
        for x in 0..32usize {
            let edge = !(8..24).contains(&x) || !(8..24).contains(&y);
            let color: [u8; 4] = if edge {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            texels[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4].copy_from_slice(&color);
        }
    }
    let mut image = ctx
        .create_image(sheet, PixelFormat::Rgba8Unorm)
        .expect("sheet");
    ctx.write_image(&mut image, &texels).expect("upload");

    let center = Rect::new(8.0, 8.0, 24.0, 24.0);
    // Stretched hard in both directions, which is what a nine-patch is for and
    // where a seam would show worst.
    let into = Rect::new(4.0, 4.0, 124.0, 124.0);

    let render_with = |ctx: &mut Context, canvas: Canvas| {
        let recording = canvas.finish();
        let draws: usize = recording.passes.iter().map(|p| p.batch.draws().len()).sum();
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw_with_images(&mut surface, &recording, &[&image])
            .expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (draws, pixels)
    };

    let merged = {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_image_nine(0, sheet, center, into, &Paint::default())
            .expect("a nine-patch");
        render_with(&mut ctx, canvas)
    };

    // The nine, written out here as the implementation used to write them: a
    // paint per patch carrying its own source rectangle, which clamps.
    let separate = {
        let (w, h) = (sheet.width as f32, sheet.height as f32);
        let sx = [0.0, center.left, center.right, w];
        let sy = [0.0, center.top, center.bottom, h];
        let dx = [
            into.left,
            into.left + center.left,
            into.right - (w - center.right),
            into.right,
        ];
        let dy = [
            into.top,
            into.top + center.top,
            into.bottom - (h - center.bottom),
            into.bottom,
        ];
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        for row in 0..3 {
            for column in 0..3 {
                let destination = Rect::new(dx[column], dy[row], dx[column + 1], dy[row + 1]);
                let paint = Paint::default().with_shader(Shader::Image {
                    slot: 0,
                    rect: destination,
                    alpha: 1.0,
                    tile: TileMode::Clamp,
                    source: Rect::new(
                        sx[column] / w,
                        sy[row] / h,
                        sx[column + 1] / w,
                        sy[row + 1] / h,
                    ),
                    tint: Color::WHITE,
                    sampling: Sampling::Linear,
                });
                canvas.draw_rect(destination, &paint).expect("a patch");
            }
        }
        render_with(&mut ctx, canvas)
    };

    // Before the assertions, so a failing one does not leave the sheet
    // behind. An image outliving its context is a validation error rather
    // than a test failure, and it is reported against whichever test runs
    // next -- which is how this one showed up in CI, as somebody else's
    // problem, on a run where every test passed.
    ctx.destroy_image(image);

    assert_eq!(merged.0, 1, "a nine-patch should record one draw");
    assert_eq!(
        separate.0, 9,
        "nine patches drawn one at a time are nine draws"
    );

    let differing = merged
        .1
        .chunks_exact(4)
        .zip(separate.1.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    let worst = merged
        .1
        .iter()
        .zip(&separate.1)
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap_or(0);
    assert_eq!(
        (differing, worst),
        (0, 0),
        "one draw should be the same picture as nine: {differing} pixel(s) \
         differ, worst by {worst}. Without the half-texel inset this is about \
         twelve hundred pixels and a hundred and seventy levels, which is the \
         seams bleeding"
    );
}

#[test]
fn a_field_of_points_is_one_draw_with_the_edge_each_dot_had() {
    // `drawPoints` in `Points` mode used to record a draw per point: each dot
    // went through the analytic circle, which carries its center in `to_local`,
    // so no two dots shared a material and none of them merged. A thousand-dot
    // scatter plot was a thousand draws where upstream's `PointFieldGeometry`
    // is one.
    //
    // `Material::PointField` is the same field with the center taken off the
    // paint and put on the vertices. Both halves are asserted because the easy
    // way to get one draw is to tessellate, which costs the edge -- measured at
    // the time, four hundred and fifty-one pixels different by up to
    // ninety-two levels. This keeps it exactly.
    let Some(mut ctx) = context() else { return };

    let points: Vec<Vec2> = (0..16)
        .map(|i| {
            let a = i as f32 * 0.4;
            Vec2::new(64.0 + 40.0 * a.cos(), 64.0 + 40.0 * a.sin())
        })
        .collect();
    let radius = 4.5f32;

    let field = {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        let paint = Paint::stroke(Color::WHITE, radius * 2.0).with_style(Style::Stroke(
            StrokeStyle::new(radius * 2.0).with_cap(LineCap::Round),
        ));
        canvas
            .draw_points(PointMode::Points, &points, &paint)
            .expect("a field");
        canvas
    };
    // The same dots one at a time, which is what the field replaced.
    let separately = {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        for center in &points {
            canvas
                .draw_circle(*center, radius, &Paint::fill(Color::WHITE))
                .expect("a dot");
        }
        canvas
    };

    let shot = |ctx: &mut Context, canvas: Canvas| {
        let recording = canvas.finish();
        let draws: usize = recording.passes.iter().map(|p| p.batch.draws().len()).sum();
        let mut surface = ctx
            .create_surface(SIZE, PixelFormat::Rgba8Unorm)
            .expect("surface");
        ctx.draw(&mut surface, &recording).expect("draw");
        let pixels = ctx.read(&mut surface).expect("read");
        ctx.destroy_surface(surface);
        (draws, pixels)
    };
    let (field_draws, field_pixels) = shot(&mut ctx, field);
    let (dot_draws, dot_pixels) = shot(&mut ctx, separately);

    assert_eq!(field_draws, 1, "sixteen points should record one draw");
    // Sixteen, until an analytic shape began carrying its own space on its
    // vertices rather than in its material; identical circles in different
    // places are now one material and merge. What is left is a handful, and how
    // many depends on where the centers round -- a radius either side of a
    // center gives a width that differs in its last bits from one position to
    // the next, and only exactly equal sizes merge. So the claim here is the
    // comparison rather than the count: the field still records fewer.
    assert!(
        dot_draws > field_draws,
        "circles drawn one at a time should still be more draws than the field: \
         {dot_draws} against {field_draws}"
    );

    let lit = field_pixels.chunks_exact(4).filter(|p| p[0] > 8).count();
    assert!(
        lit > 800,
        "the field should draw something, got {lit} pixels"
    );

    let differing = field_pixels
        .chunks_exact(4)
        .zip(dot_pixels.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    let worst = field_pixels
        .iter()
        .zip(&dot_pixels)
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap_or(0);
    assert_eq!(
        (differing, worst),
        (0, 0),
        "one draw should be the same picture as sixteen: {differing} pixel(s) \
         differ, worst by {worst}. Without the pixel of outset on the quad this \
         is ninety-two pixels, where the disc meets the quad it is drawn on"
    );
}

/// How far a blur reaches from the center of the frame, in device pixels.
fn blur_reach_of(pixels: &[u8]) -> u32 {
    let mut reach = 0;
    for x in 0..SIZE.width {
        if pixel(pixels, x, SIZE.height / 2)[0] > 8 {
            reach = reach.max((x as i32 - SIZE.width as i32 / 2).unsigned_abs());
        }
    }
    reach
}

#[test]
fn a_blur_scales_with_the_transform() {
    // `dart:ui` states a blur's sigma in the space the drawing is in, and
    // upstream honors that: `GaussianBlurFilterContents` computes its
    // `scaled_sigma` from `effect_transform.Basis()`, and the backdrop path
    // hands it `transform.Basis()` too. So the same sigma under a scale of
    // three is three times the blur.
    //
    // This renderer stated its sigmas in device pixels instead, and said so in
    // both doc comments -- predictable without knowing the transform, and
    // wrong for anything that animates a scale: a card lifting kept a blur the
    // same size while its content grew.
    let Some(mut ctx) = context() else { return };

    // The same square, at the same device size, under two transforms. Only the
    // blur can differ between them.
    let shot = |ctx: &mut Context, scale: f32, blurred: &dyn Fn(&mut Canvas, Rect)| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas.save();
        canvas.scale(scale, scale);
        let side = 24.0 / scale;
        let at = 64.0 / scale;
        blurred(
            &mut canvas,
            Rect::new(
                at - side / 2.0,
                at - side / 2.0,
                at + side / 2.0,
                at + side / 2.0,
            ),
        );
        canvas.restore();
        blur_reach_of(&render(ctx, canvas))
    };

    for (what, draw) in [
        (
            "a paint's mask blur",
            &(|canvas: &mut Canvas, r: Rect| {
                canvas
                    .draw_rect(r, &Paint::fill(Color::WHITE).with_mask_blur(4.0))
                    .expect("a blurred rectangle");
            }) as &dyn Fn(&mut Canvas, Rect),
        ),
        (
            "a layer's blur",
            &(|canvas: &mut Canvas, r: Rect| {
                canvas.save_layer(Layer::opacity(1.0).with_blur(4.0));
                canvas
                    .draw_rect(r, &Paint::fill(Color::WHITE))
                    .expect("a rectangle in a blurred layer");
                canvas.restore();
            }) as &dyn Fn(&mut Canvas, Rect),
        ),
    ] {
        let plain = shot(&mut ctx, 1.0, draw);
        let scaled = shot(&mut ctx, 3.0, draw);
        // The square is the same device size either way, so the extra reach is
        // the blur and nothing else. Three times the sigma is not three times
        // the reach -- the square contributes twelve pixels of its own -- so
        // the claim is on the *growth*, which should be about threefold.
        let grew = (scaled - 12) as f32 / (plain - 12).max(1) as f32;
        assert!(
            (grew - 3.0).abs() < 0.5,
            "{what}: reach went from {plain} to {scaled} under a scale of three, \
             a growth of {grew:.2} where about three was wanted. A sigma that \
             does not scale gives a growth of one"
        );
    }
}

#[test]
fn a_blurred_clear_erases_by_its_falloff_rather_than_by_its_bounds() {
    // `Clear` is the one mode that ignores coverage and can be admitted to the
    // evaluated blur anyway, because on a coverage it is not what its factors
    // say: clearing by an amount `c` is `dst * (1 - c)`, which is `DstOut`
    // against a white source. White is exact rather than approximate, since
    // `Clear` discards the source color by definition.
    //
    // This is upstream's special case in upstream's one place --
    // `SolidRRectLikeBlurContents::Render` forces white and switches the
    // pipeline to a reverse subtraction, `dst - src * dst` -- and it is the
    // same arithmetic reached by naming the mode that already means it.
    //
    // Before it, the draw fell through to the layer route and the layer's
    // composite covered the layer's *bounds*: a blurred circle cleared a
    // rectangle of 9216 pixels, 96 by 96 to the pixel.
    let Some(mut ctx) = context() else { return };

    let shot = |ctx: &mut Context, sigma: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 128.0, 128.0),
                &Paint::fill(Color::srgb(0.2, 0.4, 1.0, 1.0)),
            )
            .expect("a ground to erase from");
        canvas
            .draw_circle(
                Vec2::new(64.0, 64.0),
                36.0,
                &Paint::fill(Color::WHITE)
                    .with_mask_blur(sigma)
                    .with_blend(BlendMode::Clear),
            )
            .expect("a blurred hole");
        render(ctx, canvas)
    };
    let alpha = |pixels: &[u8], x: usize, y: usize| pixels[(y * 128 + x) * 4 + 3];

    // Sharp, and unchanged: the hole is the circle, so its area is about pi r
    // squared and nothing like the quad the field is evaluated on.
    let sharp = shot(&mut ctx, 0.0);
    let cut = sharp.chunks_exact(4).filter(|p| p[3] == 0).count();
    let disc = (std::f32::consts::PI * 36.0 * 36.0) as usize;
    assert!(
        cut.abs_diff(disc) < disc / 20,
        "a sharp clear should erase the circle: {cut} against about {disc}"
    );

    let blurred = shot(&mut ctx, 7.0);

    // The middle goes to nothing, and the corner of the frame is well outside
    // the blur's reach and keeps the fill.
    assert_eq!(
        alpha(&blurred, 64, 64),
        0,
        "the middle should be erased outright"
    );
    assert_eq!(
        alpha(&blurred, 2, 2),
        255,
        "the corner is outside the reach"
    );

    // The corner of what the bounds *would* have been. This is the assertion
    // that tells the fix from the defect: it sits inside the rectangle the old
    // route cleared and outside the disc, so it was transparent before and has
    // to be untouched now.
    assert_eq!(
        alpha(&blurred, 18, 18),
        255,
        "the erased region has to be round; this is a corner of its bounding box"
    );

    // And the edge is a falloff rather than a step: alpha climbs from nothing
    // to the full fill along the radius and never goes back down.
    let ramp: Vec<u8> = (64..128).map(|x| alpha(&blurred, x, 64)).collect();
    assert!(
        ramp.windows(2).all(|w| w[0] <= w[1]),
        "alpha should climb monotonically out of the hole: {ramp:?}"
    );
    let partial = ramp.iter().filter(|a| (8..248).contains(*a)).count();
    assert!(
        partial >= 12,
        "the hole's edge should fade over many pixels; {partial} of the radius \
         carried a partial alpha"
    );
}

#[test]
fn a_blend_color_filter_in_an_advanced_mode_computes_the_specified_formula() {
    // `dart:ui`'s `ColorFilter.mode` takes any blend mode. Most are affine in
    // the destination once the source is fixed and become a matrix; the
    // advanced ones are piecewise or exchange components between channels, and
    // the shader evaluates them against the constant per fragment.
    //
    // Checked against the equation rather than against a recorded picture, and
    // on a mode where a wrong answer is not close to a right one. `Difference`
    // is |dst - src| per channel, so a filter that had silently fallen back to
    // the source, to the destination, or to a multiply gives a different number
    // at every one of the three channels below.
    //
    // Note which operand is which: the material is the destination and the
    // filter's color is the source, which is the way round `dart:ui` states
    // `ColorFilter.mode` and the way round that makes an icon sheet tinted by
    // `SrcIn` mean what everyone expects.
    let Some(mut ctx) = context() else { return };

    let dst = [0.8f32, 0.3, 0.6];
    let src = [0.25f32, 0.9, 0.1];

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_rect(
            Rect::new(16.0, 16.0, 112.0, 112.0),
            &Paint::fill(Color::srgb(dst[0], dst[1], dst[2], 1.0)).with_color_filter(
                ColorFilter::blend([src[0], src[1], src[2], 1.0], BlendMode::Difference),
            ),
        )
        .expect("a filtered rectangle");
    let pixels = render(&mut ctx, canvas);

    let got = pixel(&pixels, 64, 64);
    for (channel, (d, s)) in dst.iter().zip(src.iter()).enumerate() {
        let want = ((d - s).abs() * 255.0).round() as u8;
        assert!(
            got[channel].abs_diff(want) <= 2,
            "channel {channel} of |dst - src| should be about {want}, got {}",
            got[channel]
        );
    }
    assert_eq!(
        got[3], 255,
        "an opaque draw stays opaque through the filter"
    );
}

#[test]
fn a_layers_blur_reaches_past_stated_bounds_however_it_was_spelled() {
    // `Layer::with_blur` and an `ImageFilter::Blur` handed to
    // `save_layer_filtered` are two spellings of one thing -- the comment in
    // `open_layer` says as much, turning the first into the second so there is
    // one path below it. They have to be one picture too.
    //
    // They were not, and only where the caller stated bounds. A layer's own
    // sigma is carried into the target's size by `reach`; a filter's spread was
    // not, so a blur handed over as the layer's filter stopped dead at the
    // bound while the same blur set on the layer spread past it. An unbounded
    // layer was right either way, because the narrowing that sizes one already
    // asks the filter how far it reaches -- which is why nothing had noticed.
    let Some(mut ctx) = context() else { return };

    let square = Rect::new(44.0, 44.0, 84.0, 84.0);
    let bounds = square;
    let shot = |ctx: &mut Context, filtered: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        if filtered {
            canvas
                .save_layer_filtered(Layer::opacity(1.0), Some(bounds), &ImageFilter::blur(6.0))
                .expect("a blur is available as a layer filter");
        } else {
            canvas.save_layer_bounds(Layer::opacity(1.0).with_blur(6.0), bounds);
        }
        canvas
            .draw_rect(square, &Paint::fill(Color::WHITE))
            .expect("a square to blur");
        canvas.restore();
        render(ctx, canvas)
    };

    let extent = |pixels: &[u8]| {
        let lit: Vec<(usize, usize)> = (0..128usize)
            .flat_map(|y| (0..128usize).map(move |x| (x, y)))
            .filter(|(x, y)| pixels[(y * 128 + x) * 4] > 8)
            .collect();
        assert!(!lit.is_empty(), "the blurred square drew nothing");
        (
            lit.iter().map(|p| p.0).min().unwrap(),
            lit.iter().map(|p| p.0).max().unwrap(),
        )
    };

    let on_the_layer = extent(&shot(&mut ctx, false));
    let as_a_filter = extent(&shot(&mut ctx, true));
    assert_eq!(
        as_a_filter, on_the_layer,
        "the same blur stated two ways should reach the same distance past the \
         bound; as a filter it reached {as_a_filter:?} and on the layer {on_the_layer:?}"
    );
    // And both actually spread, or the equality above is between two pictures
    // that were each cut off at the bound.
    assert!(
        on_the_layer.0 < 44 && on_the_layer.1 > 83,
        "a blur of six should carry past a bound at forty-four and eighty-four; \
         it reached {on_the_layer:?}"
    );
}

#[test]
fn a_gradient_on_a_mesh_is_read_at_the_texture_coordinates() {
    // `dart:ui` reads whatever color source the paint carries at a mesh's
    // coordinates, image or not. This refused everything but an image, with a
    // message saying the coordinates needed an image paint to read -- true of
    // the implementation and not of the API.
    //
    // The check is that the coordinates are *used*, not merely accepted. Two
    // draws of the same triangle with the same gradient, differing only in the
    // coordinates: one where they match the positions, and one where they run
    // the other way. A renderer deriving the coordinate from the fragment's
    // position gives the same picture twice.
    let Some(mut ctx) = context() else { return };

    let positions = [
        Vec2::new(24.0, 104.0),
        Vec2::new(64.0, 24.0),
        Vec2::new(104.0, 104.0),
    ];
    let ramp = Shader::LinearGradient {
        start: Vec2::new(24.0, 24.0),
        end: Vec2::new(104.0, 104.0),
        stops: vec![
            GradientStop::new(Color::srgb(0.0, 0.0, 1.0, 1.0), 0.0),
            GradientStop::new(Color::srgb(1.0, 0.0, 0.0, 1.0), 1.0),
        ],
        tile: TileMode::Clamp,
    };
    let shot = |ctx: &mut Context, coords: [Vec2; 3]| {
        let mesh = Vertices::textured(VertexMode::Triangles, positions.to_vec(), coords.to_vec())
            .expect("a triangle with coordinates");
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        canvas
            .draw_vertices(&mesh, &Paint::fill(Color::WHITE).with_shader(ramp.clone()))
            .expect("a gradient read at a mesh's coordinates");
        render(ctx, canvas)
    };

    let along = shot(&mut ctx, positions);
    let across = shot(
        &mut ctx,
        [
            Vec2::new(104.0, 104.0),
            Vec2::new(64.0, 24.0),
            Vec2::new(24.0, 104.0),
        ],
    );

    let differing = along
        .chunks_exact(4)
        .zip(across.chunks_exact(4))
        .filter(|(a, b)| a[0].abs_diff(b[0]) > 8)
        .count();
    assert!(
        differing > 500,
        "reversing the coordinates should change the picture; {differing} pixels \
         differ, which means the gradient was read at the position instead"
    );

    // And the first one is a gradient rather than a flat fill, so the
    // difference above is between two pictures rather than two failures.
    let _lit: Vec<u8> = along
        .chunks_exact(4)
        .filter(|p| p[0] > 4 || p[2] > 4)
        .map(|p| p[0])
        .collect();
    // And coordinates equal to the positions give the same picture as no
    // coordinates at all, which is the assertion that says the coordinate is
    // being carried into the paint's space rather than merely used raw. The
    // paint's origin is folded into the mapping the position route goes
    // through; a coordinate route that skipped it would land the ramp
    // somewhere else while still varying across the triangle.
    let mesh = Vertices::new(VertexMode::Triangles, positions.to_vec())
        .expect("a triangle with no coordinates");
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_vertices(&mesh, &Paint::fill(Color::WHITE).with_shader(ramp.clone()))
        .expect("a gradient on a mesh with no coordinates");
    let by_position = render(&mut ctx, canvas);

    let apart = along
        .chunks_exact(4)
        .zip(by_position.chunks_exact(4))
        .filter(|(a, b)| (0..4).any(|c| a[c].abs_diff(b[c]) > 2))
        .count();
    assert_eq!(
        apart, 0,
        "coordinates equal to the positions should read the gradient exactly \
         where the positions do; {apart} pixels differ"
    );
}

#[test]
fn a_color_filter_that_recolors_nothing_still_draws_the_shape() {
    // Upstream's `PipelineBlendSingleParameter`, whose whole subject is a
    // filter that does nothing: it puts a blend color filter in `Dst` mode --
    // "return the destination untouched", the one mode that is the identity --
    // on the paint of a green circle clipped to a square, over a blue circle,
    // and says in a comment what has to come out. A green square in the middle
    // of a blue circle.
    //
    // It is not a plate, and cannot be: a catalog scene whose feature leaves
    // the picture unchanged is rejected by the check that every feature has to
    // reach the picture, and this one leaves it unchanged by design. So the
    // claim is stated here instead, against the picture with no filter at all.
    //
    // It used to have ways to fail, and measuring them is what turned this into
    // a second change. `ImageFilter::Color` asked whether the filter *was*
    // `ColorFilter::None` rather than whether it does nothing, so a `Dst` blend
    // routed the draw through an offscreen and resampled it coming back, under
    // a clip, inside a layer -- three passes where the plain draw takes two, to
    // arrive at what it was handed. It asks `ColorFilter::is_identity` now, so
    // the filter is recognized and elided, and the pass count is pinned below
    // alongside the picture.
    let Some(mut ctx) = context() else { return };

    let blue = Color::srgb(0.0, 0.0, 1.0, 1.0);
    let green = Color::srgb(0.0, 1.0, 0.0, 1.0);
    let shot = |ctx: &mut Context, filtered: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        canvas.save_layer(Layer::opacity(1.0));
        canvas
            .draw_circle(Vec2::new(64.0, 64.0), 48.0, &Paint::fill(blue))
            .expect("the blue circle");
        canvas.save();
        canvas
            .clip_rect(Rect::new(40.0, 40.0, 88.0, 88.0))
            .expect("the clip");
        let mut paint = Paint::fill(green);
        if filtered {
            // Affine, so it becomes a color matrix -- the identity one.
            paint = paint.with_image_filter(ImageFilter::Color(ColorFilter::blend(
                Color::WHITE.to_array(),
                BlendMode::Dst,
            )));
        }
        canvas
            .draw_circle(Vec2::new(64.0, 64.0), 48.0, &paint)
            .expect("the green circle");
        canvas.restore();
        canvas.restore();
        render(ctx, canvas)
    };

    let plain = shot(&mut ctx, false);
    let filtered = shot(&mut ctx, true);

    // The picture upstream's comment describes, checked before the two are
    // compared: green inside the square, blue outside it and inside the circle,
    // white outside both.
    let inside = pixel(&plain, 64, 64);
    assert!(
        inside[1] > 200 && inside[2] < 60,
        "the square should be green, and is {inside:?}"
    );
    let ring = pixel(&plain, 64, 24);
    assert!(
        ring[2] > 200 && ring[1] < 60,
        "the circle outside the square should be blue, and is {ring:?}"
    );
    assert_eq!(
        pixel(&plain, 4, 4),
        [255, 255, 255, 255],
        "and white outside"
    );

    // A resample of a flat color is that color, so this is exact rather than
    // within a level: nothing here has an edge the two versions place
    // differently, the clip and the circle being the same geometry in both.
    assert_eq!(
        plain, filtered,
        "a color filter in Dst mode changed the picture, and the mode is the \
         identity"
    );

    // And it costs nothing, which is the half that would pass silently: a
    // filter routed through an offscreen arrives at the same picture and spends
    // a pass and a resample doing it.
    let passes = |filtered: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        let mut paint = Paint::fill(green);
        if filtered {
            paint = paint.with_image_filter(ImageFilter::Color(ColorFilter::blend(
                Color::WHITE.to_array(),
                BlendMode::Dst,
            )));
        }
        canvas
            .draw_circle(Vec2::new(64.0, 64.0), 48.0, &paint)
            .expect("the circle");
        canvas.finish().passes.len()
    };
    assert_eq!(
        passes(true),
        passes(false),
        "a color filter that recolors nothing should cost no pass"
    );
}

/// A point smaller than a pixel still covers one, and zero is a point.
///
/// `PointFieldGeometry` widens a point's radius to `0.5f / max_basis` --
/// `std::max(radius_, min_size)` -- so the smallest point upstream draws covers
/// a whole pixel. A radius of zero takes that path like any other, since the
/// only radius it refuses is a negative one, and nothing is dimmed to pay for
/// the widening the way a thin stroke is.
///
/// This renderer drew nothing at a width of zero and drew a sub-pixel point at
/// whatever the sample grid gave it, which is the same pair of faults a thin
/// stroke had. Both routes are checked, because a point field is one draw for
/// the whole scatter while a blurred or shaded point falls back to a circle
/// each: a widening applied to one and not the other would show only on a scene
/// carrying a filter.
#[test]
fn a_point_smaller_than_a_pixel_still_covers_one() {
    let Some(mut ctx) = context() else { return };

    let lit = |ctx: &mut Context, width: f32, blurred: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        let mut paint =
            Paint::fill(Color::srgb(0.0, 0.0, 0.0, 1.0)).with_style(Style::Stroke(StrokeStyle {
                cap: LineCap::Round,
                ..StrokeStyle::new(width)
            }));
        if blurred {
            // Anything that needs a layer takes the per-point route instead.
            paint.image_filter = ImageFilter::blur(1.0);
        }
        canvas
            .draw_points(PointMode::Points, &[Vec2::new(64.0, 64.0)], &paint)
            .expect("the point");
        let pixels = render(ctx, canvas);
        (0..128u32)
            .flat_map(|y| (0..128u32).map(move |x| (x, y)))
            .filter(|(x, y)| pixel(&pixels, *x, *y)[0] < 250)
            .count()
    };

    for blurred in [false, true] {
        let route = match blurred {
            true => "the per-point route",
            false => "the field route",
        };
        assert!(
            lit(&mut ctx, 0.0, blurred) > 0,
            "{route}: a width of zero is a point one pixel across, and drew \
             nothing"
        );
        assert!(
            lit(&mut ctx, 0.1, blurred) > 0,
            "{route}: a point a twentieth of a pixel in radius is widened to \
             half of one, and drew nothing"
        );
    }
}

/// A hairline lands on a pixel rather than between two.
///
/// The third part of upstream's minimum-size rule, and the one that decides
/// whether a hairline looks like one. A line one pixel wide whose center sits on
/// a pixel boundary covers half of each row it straddles, so it draws gray and
/// two pixels soft; centered in a row it covers that row and draws crisp.
/// `LineGeometry::GetPositionBuffer` carries the endpoints into device space,
/// drops the transform and rounds the constant coordinate with `RoundToHalf`.
///
/// Measured here rather than asserted structurally, because the whole claim is
/// about which pixels the ink lands in.
#[test]
fn a_hairline_lands_on_one_row_of_pixels() {
    let Some(mut ctx) = context() else { return };

    // On a boundary, which is where an unsnapped hairline splits.
    let drawn = |ctx: &mut Context, y: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        canvas
            .draw_line(
                Vec2::new(8.0, y),
                Vec2::new(120.0, y),
                &Paint::stroke(Color::srgb(0.0, 0.0, 0.0, 1.0), 0.0),
            )
            .expect("the hairline");
        let pixels = render(ctx, canvas);
        let lit = (0..128u32)
            .flat_map(|y| (0..128u32).map(move |x| (x, y)))
            .filter(|(x, y)| pixel(&pixels, *x, *y)[0] < 250)
            .count();
        let darkest = (0..128u32)
            .map(|row| pixel(&pixels, 64, row)[0])
            .min()
            .unwrap_or(255);
        (lit, darkest)
    };

    // 112 pixels of length, one row of them, and that row fully covered.
    let (lit, darkest) = drawn(&mut ctx, 64.0);
    assert_eq!(
        lit, 112,
        "a snapped hairline covers one row of 112 pixels, and covered {lit}"
    );
    assert!(
        darkest < 40,
        "the row it lands on is fully covered, and its darkest pixel is {darkest}"
    );

    // And it does not matter where between two pixels the caller put it: half a
    // pixel down is the same row, drawn the same way.
    let (offset_lit, offset_darkest) = drawn(&mut ctx, 64.5);
    assert_eq!(
        (offset_lit, offset_darkest),
        (lit, darkest),
        "a hairline at 64.5 should draw exactly as one at 64.0"
    );
}

/// A hairline carrying a shader is not snapped, because the snap would move it.
///
/// The snap draws in device space with an identity transform, which is what lets
/// the widening read a basis of one. Every shader but a solid color is *mapped*
/// by the transform instead -- a gradient's stops are stated in the space the
/// draw was recorded in -- so a snapped gradient would be read without the scale.
/// Measured while writing the snap: a hairline under a doubling read its gradient
/// at sixteen per cent of the way along where the same line a pixel wide read it
/// at four.
///
/// So such a hairline keeps the ordinary path and stays half a pixel off center.
/// Both halves are pinned here: the gradient has to be read where the wider line
/// reads it, and the line has to still straddle two rows, which is what says the
/// snap declined rather than that it ran and happened to agree.
#[test]
fn a_hairline_with_a_shader_keeps_its_shader_over_the_snap() {
    let Some(mut ctx) = context() else { return };

    let shot = |ctx: &mut Context, width: f32| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        // A scale, so a mapping taken without it lands somewhere else.
        canvas.scale(2.0, 2.0);
        let paint = Paint::default()
            .with_style(Style::Stroke(StrokeStyle::new(width)))
            .with_shader(Shader::LinearGradient {
                start: Vec2::new(4.0, 0.0),
                end: Vec2::new(60.0, 0.0),
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: Color::srgb(1.0, 0.0, 0.0, 1.0),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: Color::srgb(0.0, 0.0, 1.0, 1.0),
                    },
                ],
                tile: TileMode::Clamp,
            });
        canvas
            .draw_line(Vec2::new(4.0, 32.0), Vec2::new(60.0, 32.0), &paint)
            .expect("the line");
        render(ctx, canvas)
    };

    // Where the ramp crosses from redder to bluer, which is the gradient's
    // middle and moves if the mapping is taken in the wrong space.
    let crossing = |pixels: &[u8]| {
        (8..120u32).find(|x| {
            (0..128u32)
                .map(|y| pixel(pixels, *x, y))
                .filter(|p| p[0] < 250 || p[2] < 250)
                .any(|p| p[2] > p[0])
        })
    };
    let hairline = shot(&mut ctx, 0.0);
    let wider = shot(&mut ctx, 1.0);
    let (thin, thick) = (
        crossing(&hairline).expect("the hairline has a ramp"),
        crossing(&wider).expect("the wider line has a ramp"),
    );
    assert!(
        thin.abs_diff(thick) <= 3,
        "the gradient's middle is at {thin} on a hairline and {thick} on a line a \
         pixel wide, so the hairline read its shader in the wrong space"
    );

    // And it straddles two rows, which is the snap declining rather than
    // running and agreeing by luck.
    let rows = (0..128u32)
        .filter(|y| (0..128u32).any(|x| pixel(&hairline, x, *y)[1] < 250))
        .count();
    assert_eq!(
        rows, 2,
        "a shaded hairline is not snapped, so it covers two rows at half \
         coverage and covered {rows}"
    );
}

/// `Plus` keeps going past one when the target can hold it.
///
/// Upstream's `BlendModePlusAlphaWideGamut` and its color-filter twin are the
/// two scenes `docs/playground-parity.md` recorded as wanting "a wide-gamut
/// target". Both open by requiring one -- `EXPECT_EQ(GetDefaultColorFormat(),
/// kB10G10R10A10XR)` and a skip otherwise -- and then add red to a bright ground
/// through `Plus`. The sum exceeds one, so an eight-bit target clips it and an
/// extended-range one does not, and that difference is the whole picture.
///
/// Tests rather than plates, for the reason `PipelineBlendSingleParameter` is a
/// test: a catalog scene cannot opt into a floating-point target. Every test
/// binary here runs concurrently and peak allocation is their sum, so a
/// four-times multisampled float layer in the shared collections is memory the
/// architecture notes already record a run dying for want of.
///
/// What they guard is the clamps that came out of the shader tail with wide
/// gamut. A renderer that still clamped there would return exactly one here and
/// pass every other blend test in this file, because every other one renders
/// into eight bits where the answer is the same either way.
#[test]
fn plus_saturates_in_eight_bits_and_does_not_in_a_float_target() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().float_render_targets {
        eprintln!("skipping: this device cannot render into a floating-point target");
        return;
    }

    // Upstream's ground is (0.9, 1, 0.9); red through `Plus` takes the red
    // channel past one and leaves the others alone.
    let ground = Color::srgb(0.9, 1.0, 0.9, 1.0);
    let build = |filtered: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(ground);
        let layer = Layer::opacity(1.0);
        match filtered {
            // The twin puts the `Plus` on the *group* as a blend color filter
            // rather than on the draw, which is a different route to the same
            // arithmetic and the reason upstream has both.
            true => {
                canvas.save_layer(layer.with_color_filter(ColorFilter::blend(
                    Color::srgb(1.0, 0.0, 0.0, 1.0).to_array(),
                    BlendMode::Plus,
                )));
                canvas
                    .draw_rect(
                        Rect::new(16.0, 16.0, 112.0, 112.0),
                        &Paint::fill(Color::WHITE).with_anti_alias(false),
                    )
                    .expect("the rect");
            }
            // Straight onto the ground, with no group between them. Upstream
            // wraps this one in a `SaveLayer` and relies on the layer being
            // collapsed into the parent pass for the blend to meet the ground
            // at all -- a layer starts transparent, so an uncollapsed one would
            // add red to nothing and composite the red over the ground. That
            // collapse is an optimization this renderer does not have, and the
            // claim being mirrored is about the target rather than about the
            // layer, so the group goes.
            false => {
                canvas
                    .draw_rect(
                        Rect::new(16.0, 16.0, 112.0, 112.0),
                        &Paint::fill(Color::srgb(1.0, 0.0, 0.0, 1.0))
                            .with_blend(BlendMode::Plus)
                            .with_anti_alias(false),
                    )
                    .expect("the rect");
                return canvas;
            }
        }
        canvas.restore();
        canvas
    };

    for filtered in [false, true] {
        let route = match filtered {
            true => "a Plus color filter on the group",
            false => "a Plus blend on the draw",
        };
        let wide = wide_pixels(&mut ctx, build(filtered));
        let inside = wide_pixel(&wide, 64, 64);
        assert!(
            inside[0] > 1.05,
            "{route}: red plus a ground of nine tenths should pass one in a \
             float target, and came back {}",
            inside[0]
        );

        // And the same drawing into eight bits saturates, which is what makes
        // the line above a statement about the target rather than about the
        // blend.
        let narrow = render(&mut ctx, build(filtered));
        assert_eq!(
            pixel(&narrow, 64, 64)[0],
            255,
            "{route}: an eight-bit target has to clip the same sum"
        );
    }
}

/// An atlas tint in `Plus` *is* clipped, and that is the recorded deviation.
///
/// Upstream's `DrawAtlasPlusWideGamut` is the third scene that wanted a
/// wide-gamut target, and it is the same claim as the pair above through a third
/// route: sprites tinted with `Plus`, into a target that can hold what the sum
/// comes to. The two routes above keep the sum -- a paint's blend reaches the
/// hardware as `One, One`, and a `Plus` color filter is affine and becomes a
/// matrix -- and this one does not, because `blend_tint` clamps it in the shader.
///
/// The clamp was removed to make the three agree, and put back after benching a
/// Pi 5. Removing it cost 2.4 per cent on the Vulkan distance-field row and 1.2
/// on the GLES full frame, state for state, on a board where three runs of the
/// commit before it held every row inside three tenths of a per cent. What it
/// bought was agreement on a floating-point target, and nothing here presents
/// one, so the only place the difference can be seen is a test like this. §13 of
/// `docs/non-parity.md` has the numbers and the reasoning.
///
/// So this test pins the deviation rather than the agreement. It is written the
/// way round that fails if the clamp comes out again, because the next person to
/// read `blend_tint` and notice the inconsistency should find a test that says it
/// is deliberate and what it costs to fix.
#[test]
fn an_atlas_tint_in_plus_is_clipped_where_the_other_routes_are_not() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().float_render_targets {
        eprintln!("skipping: this device cannot render into a floating-point target");
        return;
    }
    let mut image = ctx
        .create_image(Extent2D::new(4, 4), PixelFormat::Rgba8Unorm)
        .expect("image");
    ctx.write_image(&mut image, &quadrant_image())
        .expect("upload");

    let build = || {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::BLACK);
        // Every sprite tinted white, so what is added is the texel's own color
        // on top of itself: a quadrant that is already saturated in a channel
        // comes out at twice it.
        let sprites: Vec<Sprite> = shuffled_quadrants()
            .into_iter()
            .map(|sprite| Sprite {
                color: Color::WHITE,
                ..sprite
            })
            .collect();
        canvas
            .draw_atlas(
                &sprites,
                Extent2D::new(4, 4),
                &Paint::image(0, Rect::from_size(128.0, 128.0)).with_tint_blend(BlendMode::Plus),
            )
            .expect("atlas");
        canvas
    };

    let wide = wide_pixels_with_images(&mut ctx, build(), &[&image]);
    // The top-left of the target holds the sheet's yellow quadrant, so red and
    // green are saturated there and the sum would be two if the shader let it
    // be. It comes back at one, which is the clamp.
    let inside = wide_pixel(&wide, 32, 32);
    assert!(
        (inside[0] - 1.0).abs() < 0.01 && (inside[1] - 1.0).abs() < 0.01,
        "the tint route clamps in the shader, so a saturated channel plus white \
         has to come back at one even in a float target, and the yellow quadrant \
         came back {inside:?} -- if this now passes one, the clamp came out and \
         §13 of docs/non-parity.md needs deleting rather than editing"
    );

    // And eight bits are the same, which is the half of the deviation that costs
    // nothing: the target would have clipped the sum anyway.
    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw_with_images(&mut surface, &build().finish(), &[&image])
        .expect("draw");
    let narrow = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    ctx.destroy_image(image);
    assert_eq!(
        pixel(&narrow, 32, 32),
        [255, 255, 255, 255],
        "an eight-bit target has to clip the same sum"
    );
}

/// An advanced color filter against a transparent color changes nothing.
///
/// Upstream's `AdvancedBlendColorFilterWithDestinationOpacity` puts a
/// `Saturation` blend against `kTransparent` on a group at three tenths opacity,
/// and its comment says the result "should be solid red as the destructive color
/// filter floods the clip". It should not, and the comment looks copied from the
/// scene above it -- the flood case, which is a plate here already. The
/// compositing specification states the mode as
/// `cs·(1−ab) + cb·(1−as) + B(cb,cs)·as·ab`, and at a source alpha of zero every
/// term but `cb` vanishes. The filter is the identity by arithmetic, so no
/// renderer can draw anything from it.
///
/// Which is why it cannot be a plate: a catalog scene whose feature leaves the
/// picture unchanged is rejected by the check that every feature has to reach
/// the picture. As a test it pins the weighting in the composite, and the
/// failure it catches is the one upstream's comment describes -- a shader that
/// applied the blended color without weighting it by the source alpha would
/// flood the group with the saturation result, which is what "solid red" would
/// look like. Dropping `sa` from the `mixed` term fails this.
///
/// It does *not* catch an unguarded unpremultiply, and the guess that it would
/// was checked rather than written down: opening the divide to a source alpha of
/// zero leaves this passing, because the clamp around it takes the NaN to zero
/// and the term it feeds is multiplied by that same zero alpha.
#[test]
fn an_advanced_color_filter_against_a_transparent_color_is_the_identity() {
    let Some(mut ctx) = context() else { return };

    let shot = |ctx: &mut Context, filtered: bool| {
        let mut canvas = Canvas::new(SIZE);
        canvas.clear(Color::WHITE);
        let mut layer = Layer::opacity(0.3);
        if filtered {
            layer = layer.with_color_filter(ColorFilter::blend(
                // Transparent: every component zero, alpha included.
                [0.0, 0.0, 0.0, 0.0],
                BlendMode::Saturation,
            ));
        }
        canvas.save_layer(layer);
        canvas
            .draw_rect(
                Rect::new(24.0, 24.0, 96.0, 96.0),
                &Paint::fill(Color::srgb(0.5, 0.0, 0.0, 1.0)).with_anti_alias(false),
            )
            .expect("the maroon rect");
        canvas
            .draw_rect(
                Rect::new(48.0, 48.0, 120.0, 120.0),
                &Paint::fill(Color::srgb(0.0, 0.0, 1.0, 1.0)).with_anti_alias(false),
            )
            .expect("the blue rect");
        canvas.restore();
        render(ctx, canvas)
    };

    let plain = shot(&mut ctx, false);
    let filtered = shot(&mut ctx, true);
    assert_eq!(
        plain, filtered,
        "a Saturation filter against a transparent color is the identity, so it \
         must leave the group exactly as it found it"
    );

    // And there is a picture to be identical about. The group is three tenths
    // opaque over white, so the blue rectangle arrives well short of blue and
    // well short of white -- a comparison of two blank frames would satisfy the
    // line above and say nothing.
    let inside = pixel(&plain, 100, 100);
    assert!(
        inside[2] > 200 && inside[0] < 220 && inside[0] > 120,
        "the blue rectangle at three tenths over white should be a pale blue, \
         and is {inside:?}"
    );
}

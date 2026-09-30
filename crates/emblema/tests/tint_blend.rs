//! How a per-vertex or per-sprite color combines with what the paint produced.
//!
//! `dart:ui` passes a blend mode to `drawVertices` and `drawAtlas` saying this,
//! and both operations were marked complete here while supporting exactly one
//! of the modes -- a multiply, which is `Modulate` and was not a choice.
//!
//! The shader's formulas are checked against `emblema_hal`'s, which existed
//! first and are the software reference the conformance tests already compare
//! hardware to. Neither was derived from the other, which is what makes the
//! comparison worth making: a transcription error in the shader shows as a
//! disagreement rather than as two copies of the same mistake.

use emblema::{
    BackendPreference, BlendMode, Canvas, Color, Context, Extent2D, Paint, PixelFormat, Vec2,
    VertexMode, Vertices,
};
use emblema_hal::blend::blend_advanced;

const SIZE: Extent2D = Extent2D {
    width: 64,
    height: 64,
};

fn context() -> Option<Context> {
    match Context::new(BackendPreference::Auto) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no context ({e})");
            None
        }
    }
}

/// Draw one full-target triangle pair whose vertices all carry `tint`, over a
/// paint of `under`, combining them with `mode`. Returns the middle texel.
fn combined(ctx: &mut Context, mode: BlendMode, tint: Color, under: Color) -> [u8; 4] {
    let mesh = Vertices::full(
        VertexMode::Triangles,
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 0.0),
            Vec2::new(64.0, 64.0),
            Vec2::new(0.0, 64.0),
        ],
        Vec::new(),
        vec![tint; 4],
        vec![0, 1, 2, 0, 2, 3],
    )
    .expect("mesh");

    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas
        .draw_vertices(
            &mesh,
            &Paint::fill(under)
                .with_tint_blend(mode)
                // `Src` so the target keeps what this draw computed and the
                // reading is of the combination rather than of it composited
                // over the ground.
                .with_blend(BlendMode::Src)
                .with_anti_alias(false),
        )
        .expect("mesh");

    let mut surface = ctx
        .create_surface(SIZE, PixelFormat::Rgba8Unorm)
        .expect("surface");
    ctx.draw(&mut surface, &canvas.finish()).expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");
    ctx.destroy_surface(surface);
    let i = ((32 * SIZE.width + 32) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn every_advanced_mode_agrees_with_the_reference_formulas() {
    // Stated in sRGB, which is the space the blend happens in and therefore the
    // space the reference below has to be computed in. `Color::linear` encodes
    // on the way in, so a color written that way would be blended as a
    // different number than the one the reference used.
    let Some(mut ctx) = context() else { return };

    // Opaque, because an advanced mode's compositing terms and its blend
    // function are separable only when both sides cover: with alpha in play a
    // disagreement could come from either and the test would not say which.
    let tint = Color::srgb(0.8, 0.35, 0.15, 1.0);
    let under = Color::srgb(0.25, 0.6, 0.9, 1.0);

    let advanced = [
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    for mode in advanced {
        let got = combined(&mut ctx, mode, tint, under);
        // The reference takes premultiplied color, and both sides are opaque.
        let want = blend_advanced(mode, [0.8, 0.35, 0.15, 1.0], [0.25, 0.6, 0.9, 1.0])
            .unwrap_or_else(|| panic!("{} is advanced", mode.name()));
        for channel in 0..3 {
            let expected = (want[channel].clamp(0.0, 1.0) * 255.0).round() as i32;
            let actual = got[channel] as i32;
            assert!(
                (expected - actual).abs() <= 2,
                "{}: channel {channel} was {actual}, the reference says {expected} \
                 (got {got:?}, want {want:?})",
                mode.name()
            );
        }
    }
}

#[test]
fn an_advanced_mode_composites_as_well_as_blends() {
    // The test above uses opaque colors on purpose, and that leaves half the
    // work unchecked: with both sides covering, the compositing terms vanish
    // and the result is the blend function alone. Replacing the whole formula
    // with its middle term passes that test. So this one is translucent on both
    // sides, where the terms that weigh the blend against the two colors
    // separately are the difference between right and plausible.
    let Some(mut ctx) = context() else { return };

    let tint = Color::srgb(0.8, 0.2, 0.1, 0.6);
    let under = Color::srgb(0.2, 0.7, 0.9, 0.4);
    // Premultiplied, which is what the reference takes and the target stores.
    let src = [0.8 * 0.6, 0.2 * 0.6, 0.1 * 0.6, 0.6];
    let dst = [0.2 * 0.4, 0.7 * 0.4, 0.9 * 0.4, 0.4];

    for mode in [
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Difference,
        BlendMode::Luminosity,
    ] {
        let got = combined(&mut ctx, mode, tint, under);
        let want =
            blend_advanced(mode, src, dst).unwrap_or_else(|| panic!("{} is advanced", mode.name()));
        for channel in 0..4 {
            let expected = (want[channel].clamp(0.0, 1.0) * 255.0).round() as i32;
            let actual = got[channel] as i32;
            assert!(
                (expected - actual).abs() <= 2,
                "{}: channel {channel} was {actual}, the reference says {expected} \
                 (got {got:?}, want {want:?})",
                mode.name()
            );
        }
    }
}

#[test]
fn the_porter_duff_modes_do_what_their_definitions_say() {
    let Some(mut ctx) = context() else { return };

    // Half-transparent on both sides, because these modes are defined by what
    // they do with coverage and two opaque colors make most of them agree.
    let tint = Color::linear(1.0, 0.0, 0.0, 0.5);
    let under = Color::linear(0.0, 0.0, 1.0, 0.5);

    // Premultiplied, which is what the target stores and what these formulas
    // are stated on: src is (0.5, 0, 0, 0.5) and dst is (0, 0, 0.5, 0.5).
    for (mode, want) in [
        (BlendMode::Clear, [0.0f32, 0.0, 0.0, 0.0]),
        (BlendMode::Src, [0.5, 0.0, 0.0, 0.5]),
        (BlendMode::Dst, [0.0, 0.0, 0.5, 0.5]),
        (BlendMode::SrcOver, [0.5, 0.0, 0.25, 0.75]),
        (BlendMode::DstOver, [0.25, 0.0, 0.5, 0.75]),
        (BlendMode::SrcIn, [0.25, 0.0, 0.0, 0.25]),
        (BlendMode::DstIn, [0.0, 0.0, 0.25, 0.25]),
        (BlendMode::SrcOut, [0.25, 0.0, 0.0, 0.25]),
        (BlendMode::DstOut, [0.0, 0.0, 0.25, 0.25]),
        (BlendMode::SrcATop, [0.25, 0.0, 0.25, 0.5]),
        (BlendMode::DstATop, [0.25, 0.0, 0.25, 0.5]),
        (BlendMode::Xor, [0.25, 0.0, 0.25, 0.5]),
        (BlendMode::Plus, [0.5, 0.0, 0.5, 1.0]),
        (BlendMode::Modulate, [0.0, 0.0, 0.0, 0.25]),
    ] {
        let got = combined(&mut ctx, mode, tint, under);
        for channel in 0..4 {
            let expected = (want[channel] * 255.0).round() as i32;
            let actual = got[channel] as i32;
            assert!(
                (expected - actual).abs() <= 2,
                "{}: channel {channel} was {actual}, expected {expected} \
                 (got {got:?})",
                mode.name()
            );
        }
    }
}

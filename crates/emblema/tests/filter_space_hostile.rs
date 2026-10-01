//! Generated transforms against the per-axis length conversion.
//!
//! `filter_space.rs` holds the five spellings of a filter against each other at
//! chosen values. This drives the same conversion through the public API with
//! transforms a caller could build and this renderer would rather not meet.
//!
//! The conversion changed shape four times across 2026-09-29 and 2026-09-30:
//! `axis_scales_of`, both `scaled_by` spellings, `morphology_passes` walking
//! `BlurBasis`'s directions, `open_layer` taking the backdrop blur. Each multiplies
//! a caller's float by a length read off a transform, which is where a finite input
//! becomes infinite. A radius of `1e20` once decided how many passes a recording
//! held and exhausted memory; generated input found it.
//!
//! Asserted: it returns, every length a material carries is finite and not
//! negative, and the pass count is bounded. Recording only, no device.
//!
//! # What the mutations say
//!
//! Twelve runs of 8,192 cases on fresh seeds, clean every time -- so three guards
//! were removed one at a time to see which the properties rest on.
//!
//! Redundant: `axis_scales_of`'s non-finite fallback, and `applied_radius`'s
//! `is_finite` arm. Removing either changes nothing the sweep sees, because a
//! non-finite radius is caught again downstream. Same belt-and-braces shape
//! `hostile_api.rs` records.
//!
//! Load-bearing: `applied_radius`'s clamp to the target extent. Without it a radius
//! of `1e20` reaches a loop emitting a pass per `MORPHOLOGY_TAPS` texels and does
//! not return -- the case ran past sixty seconds and was killed. **A hang is this
//! file's failure mode, not a wrong length**, which is why `log_case` exists.

use emblema::*;
use emblema_hal::Material;
use proptest::prelude::*;

/// Floats a conversion comes apart on, plus ordinary ones.
///
/// Edge-weighted: a uniform sample almost never produces an infinity, and
/// infinities are the half this arithmetic creates from finite inputs.
fn hostile() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => prop_oneof![
            Just(0.0f32),
            Just(-0.0f32),
            Just(1.0f32),
            Just(-1.0f32),
            Just(f32::MIN_POSITIVE),
            Just(f32::MAX),
            Just(f32::MIN),
            Just(f32::EPSILON),
            Just(f32::INFINITY),
            Just(f32::NEG_INFINITY),
            Just(f32::NAN),
            Just(1e20f32),
            Just(-1e20f32),
            Just(1e-20f32),
        ],
        2 => -2048.0f32..2048.0f32,
    ]
}

/// Every length a finished recording carries, from both filters.
fn lengths(recording: &Recording) -> Vec<(&'static str, f32)> {
    recording
        .passes
        .iter()
        .flat_map(|pass| pass.batch.draws())
        .flat_map(|draw| match draw.material {
            Material::Blur { sigma, step, .. } => {
                vec![
                    ("blur sigma", sigma),
                    ("blur step x", step[0]),
                    ("blur step y", step[1]),
                ]
            }
            Material::Morphology { radius, step, .. } => vec![
                ("morphology radius", radius),
                ("morphology step x", step[0]),
                ("morphology step y", step[1]),
            ],
            _ => Vec::new(),
        })
        .collect()
}

/// Every length finite and not negative, and the recording bounded.
///
/// `what` names the spelling, so a failure says which path without shrinking twice.
fn assert_sane(what: &str, recording: &Recording) {
    for (name, value) in lengths(recording) {
        assert!(
            value.is_finite(),
            "{what}: a {name} of {value} reached a material, and a pass walking it \
             has no stop that is not the extent"
        );
        if name.ends_with("radius") || name.ends_with("sigma") {
            assert!(
                value >= 0.0,
                "{what}: a {name} of {value} is a distance in the wrong direction"
            );
        }
    }
    // Thirty-two taps a pass and a halving per octave of deviation, over four
    // filters and two axes, cannot reach three figures from any finite length the
    // conversion is allowed to produce. A thousand is slack enough to be a
    // statement about unboundedness rather than about this scene.
    assert!(
        recording.passes.len() < 1000,
        "{what}: {} passes, which is a length that survived the clamp",
        recording.passes.len()
    );
}

/// Overwrite `EMBLEMA_PROPTEST_LOG` with the case about to run.
///
/// A failure shrinks and prints itself; a hang does neither, and a hang is this
/// file's failure mode. After one, the file holds the input that did not finish.
/// Off by default.
fn log_case(what: &str) {
    if let Some(path) = std::env::var_os("EMBLEMA_PROPTEST_LOG") {
        let _ = std::fs::write(path, what);
    }
}

/// Apply a generated transform, in the order stated here.
///
/// The last two entries shear and add perspective. They are here because
/// `axis_scales_of` and `BlurBasis::of` both have a fallback for a transform they
/// cannot decompose, `non-parity.md` 17 and 20 record it as the remaining upstream
/// difference, and nothing reached it: translate, scale and rotate always
/// decompose.
fn aim(canvas: &mut Canvas, t: [f32; 7]) {
    canvas.translate(t[0], t[1]);
    canvas.scale(t[2], t[3]);
    canvas.rotate(t[4]);
    // Column-major 4x4, so index 4 is the y-of-x shear and 3 and 7 are the
    // perspective divisors. Identity elsewhere.
    canvas.concat(Transform2D::from_column_major_4x4(&[
        1.0, 0.0, 0.0, t[6], //
        t[5], 1.0, 0.0, t[6], //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]));
}

fn content(canvas: &mut Canvas) {
    let _ = canvas.draw_rect(
        Rect::new(10.0, 10.0, 90.0, 70.0),
        &Paint::fill(Color::srgb(1.0, 0.8, 0.3, 1.0)),
    );
}

fn target() -> Canvas {
    Canvas::new(Extent2D {
        width: 256,
        height: 256,
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    /// A layer's own lengths under a hostile transform.
    ///
    /// Three multiplications of a caller's float by one read off the transform:
    /// the blur pair and the morphology pair in `Layer::scaled_by`, the backdrop
    /// blur in `open_layer`.
    #[test]
    fn a_layer_under_any_transform_records_only_finite_lengths(
        t in prop::array::uniform7(hostile()),
        blur_x in hostile(),
        blur_y in hostile(),
        backdrop in hostile(),
        radius_x in hostile(),
        radius_y in hostile(),
        dilate in any::<bool>(),
        alpha in hostile(),
    ) {
        log_case(&format!(
            "layer: transform {t:?} blur ({blur_x}, {blur_y}) backdrop {backdrop} \
             radius ({radius_x}, {radius_y}) dilate {dilate} alpha {alpha}"
        ));
        let mut canvas = target();
        // Something under the layer, so a backdrop blur has a backdrop to take.
        content(&mut canvas);
        canvas.save();
        aim(&mut canvas, t);
        let mut layer = Layer::opacity(alpha)
            .with_blur_xy(blur_x, blur_y)
            .with_backdrop_blur(backdrop);
        layer.morphology = Some(if dilate {
            Morphology::dilate(radius_x, radius_y)
        } else {
            Morphology::erode(radius_x, radius_y)
        });
        canvas.save_layer(layer);
        content(&mut canvas);
        canvas.restore();
        canvas.restore();
        assert_sane("layer", &canvas.finish());
    }

    /// The same lengths through the filter spelling, including a composition.
    ///
    /// `ImageFilter::scaled_by` recurses through `Compose`, so a nested filter
    /// multiplies at every level.
    #[test]
    fn a_filter_under_any_transform_records_only_finite_lengths(
        t in prop::array::uniform7(hostile()),
        a in hostile(),
        b in hostile(),
        c in hostile(),
        d in hostile(),
        which in 0usize..3,
        nest in any::<bool>(),
    ) {
        let inner = match which {
            0 => ImageFilter::Blur { sigma_x: a, sigma_y: b },
            1 => ImageFilter::Dilate { radius_x: a, radius_y: b },
            _ => ImageFilter::Erode { radius_x: a, radius_y: b },
        };
        let filter = if nest {
            ImageFilter::Compose {
                outer: Box::new(ImageFilter::Blur { sigma_x: c, sigma_y: d }),
                inner: Box::new(inner),
            }
        } else {
            inner
        };

        log_case(&format!(
            "filter: transform {t:?} a {a} b {b} c {c} d {d} which {which} nest {nest}"
        ));
        let mut canvas = target();
        content(&mut canvas);
        canvas.save();
        aim(&mut canvas, t);
        // The filter path can refuse -- a Matrix filter inside a composition does
        // -- and a refusal is an answer rather than a failure. What must not happen
        // is a panic or an unbounded recording.
        let _ = canvas.save_layer_filtered(Layer::opacity(1.0), None, &filter);
        content(&mut canvas);
        canvas.restore();
        canvas.restore();
        assert_sane("filter", &canvas.finish());
    }
}

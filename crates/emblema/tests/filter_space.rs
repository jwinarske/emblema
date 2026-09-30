//! Every spelling of "filter this" measures its lengths in the same space.
//!
//! A caller's blur sigma is stated in the space they were drawing in -- what
//! `dart:ui` means by it, and what upstream honors by scaling with
//! `effect_transform.Basis()`. `docs/architecture.md` records the conversion of
//! this renderer's sigmas out of device space for that reason, and says it happens
//! once, when a layer is opened.
//!
//! It happened to the layer only. The `ImageFilter` handed alongside was not
//! converted, so under a scale of two the same eight-pixel blur came out as
//! sixteen through three spellings and eight through two:
//!
//! | spelling | was | now |
//! |---|---|---|
//! | `Layer::with_blur` | 16 | 16 |
//! | `Paint::with_image_filter` | 16 | 16 |
//! | `Layer::with_backdrop_blur` | 16 | 16 |
//! | `Canvas::save_layer_filtered` | **8** | 16 |
//! | `Canvas::save_layer_backdrop` | **8** | 16 |
//!
//! What makes that a defect rather than two documented conventions is the third
//! row against the fourth: both hand an `ImageFilter::Blur` to a layer, and the
//! one that goes through a paint was scaled while the one that goes through
//! `save_layer_filtered` was not. One value in one type meant two different
//! things depending on which call received it.
//!
//! # What deliberately still differs
//!
//! A morphology radius is device pixels in every spelling, which is a divergence
//! from upstream recorded as `docs/non-parity.md` 17 and pinned by
//! `emblema-testkit`'s `the_dilation_under_a_scale_reaches_the_same_distance`.
//! `ImageFilter::scaled_by` leaves it alone on purpose: scaling it here while
//! `Layer::scaled_by` does not would trade a stated divergence for an unstated
//! inconsistency. So this file asserts the radii *agree across spellings* without
//! asserting they scale.
//!
//! Recording only, so it needs no device.

use emblema::*;

const SIZE: Extent2D = Extent2D {
    width: 256,
    height: 256,
};

/// The blur deviations a recording carries, per pass, at a scale of two.
fn sigmas(build: impl FnOnce(&mut Canvas)) -> Vec<f32> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.scale(2.0, 2.0);
    build(&mut canvas);
    let _ = canvas.draw_rect(Rect::new(8.0, 8.0, 40.0, 40.0), &Paint::fill(Color::WHITE));
    canvas.restore();
    let mut found: Vec<f32> = canvas
        .finish()
        .passes
        .iter()
        .flat_map(|pass| pass.batch.draws())
        .filter_map(|draw| match draw.material {
            emblema_hal::Material::Blur { sigma, .. } => Some(sigma),
            _ => None,
        })
        .collect();
    found.sort_by(f32::total_cmp);
    found
}

/// The morphology distances a recording carries, at a scale of two.
fn radii(build: impl FnOnce(&mut Canvas)) -> Vec<f32> {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::BLACK);
    canvas.scale(2.0, 2.0);
    build(&mut canvas);
    let _ = canvas.draw_rect(Rect::new(8.0, 8.0, 40.0, 40.0), &Paint::fill(Color::WHITE));
    canvas.restore();
    let mut found: Vec<f32> = canvas
        .finish()
        .passes
        .iter()
        .flat_map(|pass| pass.batch.draws())
        .filter_map(|draw| match draw.material {
            emblema_hal::Material::Morphology { radius, .. } => Some(radius),
            _ => None,
        })
        .collect();
    found.sort_by(f32::total_cmp);
    found
}

const SIGMA: f32 = 8.0;

fn blur() -> ImageFilter {
    ImageFilter::Blur {
        sigma_x: SIGMA,
        sigma_y: SIGMA,
    }
}

/// A layer's own blur and the same blur handed over as a filter agree.
///
/// The two spellings `save_layer_filtered`'s own documentation calls the same
/// operation. Under a scale of two an eight-pixel sigma is sixteen device pixels,
/// and both must say so.
#[test]
fn a_layers_blur_and_a_filters_blur_are_the_same_length() {
    let own = sigmas(|canvas| {
        canvas.save_layer(Layer::opacity(1.0).with_blur(SIGMA));
    });
    let handed = sigmas(|canvas| {
        canvas
            .save_layer_filtered(Layer::opacity(1.0), None, &blur())
            .expect("a blur is a filter a layer accepts");
    });
    assert!(
        !own.is_empty(),
        "no blur material recorded, so this compared nothing"
    );
    assert_eq!(
        own, handed,
        "a layer's own blur and the same blur as an ImageFilter came out different \
         lengths under a scale"
    );
    assert_eq!(
        own,
        vec![SIGMA * 2.0, SIGMA * 2.0],
        "a sigma of {SIGMA} under a scale of two is {} device pixels",
        SIGMA * 2.0
    );
}

/// A backdrop blur agrees with itself across its two spellings.
///
/// `Layer::backdrop_blur` is documented as "the same operation with the one filter
/// a `Copy` layer can hold", so the two have to measure alike.
#[test]
fn a_backdrop_blur_is_the_same_length_either_way() {
    let own = sigmas(|canvas| {
        canvas.save_layer(Layer::opacity(1.0).with_backdrop_blur(SIGMA));
    });
    let handed = sigmas(|canvas| {
        canvas
            .save_layer_backdrop(Layer::opacity(1.0), None, &blur())
            .expect("a blur is a backdrop filter");
    });
    assert!(!own.is_empty(), "no backdrop blur recorded");
    assert_eq!(
        own, handed,
        "a layer's backdrop blur and the same blur as an ImageFilter came out \
         different lengths under a scale"
    );
}

/// A paint's image filter agrees with a layer's, which it already did.
///
/// Here because it is the row that made the others a defect rather than a
/// convention: this spelling hands the very same `ImageFilter` to a layer and was
/// always scaled, because it becomes a `Layer` on the way.
#[test]
fn a_paints_image_filter_is_the_same_length_as_a_layers() {
    let paint = sigmas(|canvas| {
        let _ = canvas.draw_rect(
            Rect::new(8.0, 8.0, 40.0, 40.0),
            &Paint::fill(Color::WHITE).with_image_filter(blur()),
        );
        canvas.save();
    });
    let layer = sigmas(|canvas| {
        canvas.save_layer(Layer::opacity(1.0).with_blur(SIGMA));
    });
    assert_eq!(
        paint, layer,
        "a paint's blur and a layer's blur are different lengths under a scale"
    );
}

/// Composing a blur scales the halves, not just the outside.
///
/// A `Compose` holds filters rather than lengths, so the conversion has to reach
/// through it. Two blurs, so a recursion that converted only one half would leave
/// a pair that disagree.
#[test]
fn a_composed_blur_scales_both_halves() {
    let composed = sigmas(|canvas| {
        canvas
            .save_layer_filtered(
                Layer::opacity(1.0),
                None,
                &ImageFilter::Compose {
                    outer: Box::new(blur()),
                    inner: Box::new(ImageFilter::Blur {
                        sigma_x: SIGMA / 2.0,
                        sigma_y: SIGMA / 2.0,
                    }),
                },
            )
            .expect("a composition of blurs is a filter");
    });
    let expected = [SIGMA, SIGMA, SIGMA * 2.0, SIGMA * 2.0];
    assert_eq!(
        composed,
        expected.to_vec(),
        "a composed pair of blurs did not both double under a scale of two"
    );
}

/// A morphology radius is the same length in both spellings, and scales.
///
/// It did not scale until 2026-09-29, which made it the one length here that
/// was device pixels while every other was the caller's. The entry that
/// recorded the divergence rested on upstream having no morphology to be in
/// parity with, and upstream has `dilate` and `erode`.
///
/// Both halves matter and they fail differently. The two spellings agreeing is
/// what keeps a dilation from meaning one thing through a `Layer` field and
/// another through an `ImageFilter` -- the defect this whole file exists for.
/// The doubling is what keeps the radius a length in the caller's space.
#[test]
fn a_morphology_radius_is_the_same_length_either_way_and_scales() {
    let radius = 12.0f32;
    let own = radii(|canvas| {
        canvas.save_layer(Layer::opacity(1.0).with_morphology(Morphology::dilate(radius, radius)));
    });
    let handed = radii(|canvas| {
        canvas
            .save_layer_filtered(
                Layer::opacity(1.0),
                None,
                &ImageFilter::Dilate {
                    radius_x: radius,
                    radius_y: radius,
                },
            )
            .expect("a dilation is a filter a layer accepts");
    });
    assert!(!own.is_empty(), "no morphology material recorded");
    assert_eq!(
        own, handed,
        "a layer's own morphology and the same dilation as an ImageFilter came out \
         different lengths"
    );
    assert!(
        own.iter().all(|reached| *reached == radius * 2.0),
        "the radius is a length in the caller's space, so a scale of two makes \
         {radius} reach {}: {own:?}. If it was deliberately returned to device \
         pixels, docs/non-parity.md 17 and \
         the_dilation_under_a_scale_reaches_twice_as_far record the convention \
         this asserts.",
        radius * 2.0
    );
}

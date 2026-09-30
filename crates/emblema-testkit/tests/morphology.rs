//! What space a morphology radius is in, pinned.
//!
//! `dilate` and `erode` take a radius, and it is a length in the caller's space
//! in both renderers -- since 2026-09-29 here, and always upstream, where
//! `entity.GetTransform() * effect_transform.Basis()` scales it at the pass. It
//! used to be device pixels here, which is what the first test below was written
//! backwards to pin and now pins forwards.
//!
//! What still differs is the *shape* of the conversion, not its units: one factor
//! here against a transformed direction vector per pass upstream. That is the
//! remaining half of `docs/non-parity.md` 17, and the second test below is its
//! pin.
//!
//! This file exists so that changing either is *visible*. The corpus has a pair
//! of scenes built to differ in nothing but the transform -- the same cross at
//! half the size under a scale of two, landing on exactly the pixels the unscaled
//! one covers -- so the dilation distance is the only thing left that can
//! separate them, and a third under an anisotropic scale for the axis a single
//! factor cannot carry.
//!
//! # Why it is not the cost baseline that catches this
//!
//! It looks as though it should be, and it is not, which is worth writing down
//! before someone relies on it. The two scenes record *identical* cost rows: four
//! passes, four draws, twenty vertices, thirty indices, three sources. A pass is
//! emitted per `MORPHOLOGY_TAPS` texels and that constant is thirty-two, so a
//! radius of eight and a radius of sixteen both fit in one pass each way and the
//! pass count does not move. The cost table counts passes and does not read what
//! they carry.
//!
//! So the radius has to be read out of the material, which is what this does. The
//! same reasoning says the two scenes are very likely pixel-identical as well, so
//! the corpus image comparison gains nothing from the pair either -- the second
//! scene earns its place as this pin and as coverage of a filter under a
//! transform, not as another picture.
//!
//! Recording only, so it runs with no device and on every merge.

use emblema_hal::Material;
use emblema_testkit::{corpus, record_scene};

/// The morphology radii a scene's recording ends up carrying, in pass order.
fn radii(name: &str) -> Vec<[f32; 2]> {
    // `ok_or` then `expect` rather than `unwrap_or_else` with a `panic!`: the
    // workspace denies `panic` and allows `expect`, and carrying the name through
    // as the error keeps it in the message.
    let scene = corpus()
        .into_iter()
        .find(|scene| scene.name == name)
        .ok_or(name)
        .expect("the corpus has no scene by this name");
    let recording = record_scene(&scene).expect("a corpus scene records");
    recording
        .passes
        .iter()
        .flat_map(|pass| pass.batch.draws())
        .filter_map(|draw| match draw.material {
            // `step` is the direction and reciprocal extent, `radius` the
            // distance along it. Both are needed: a pass that ran the same
            // distance along both axes would be indistinguishable from one that
            // ran the right distance if only the radius were read.
            Material::Morphology { step, radius, .. } => {
                Some([step[0], step[1]].map(|along| if along == 0.0 { 0.0 } else { radius }))
            }
            _ => None,
        })
        .collect()
}

/// A dilation under a scale of two reaches twice as far.
///
/// The radius is a length in the caller's space, so the transform converts it
/// like any other length. That is what upstream does -- read at master
/// `fab99153`: `RenderFilter` applies
/// `entity.GetTransform() * effect_transform.Basis()` to the radius and takes
/// the length of the result -- and this renderer did not, until 2026-09-29.
///
/// The pair is built to make that visible and nothing else: the same cross at
/// half the size under a scale of two lands on exactly the pixels the unscaled
/// scene covers, so the dilation distance is all that can separate them.
///
/// This assertion is the inverse of the one that stood here before, which said
/// the two reached the *same* distance and existed to make the old convention
/// impossible to change by accident. It did its job: it failed the moment the
/// radius became local, with a message naming the replacement. Kept rather than
/// deleted, because the convention still has to be impossible to change by
/// accident -- in either direction.
#[test]
fn the_dilation_under_a_scale_reaches_twice_as_far() {
    let unscaled = radii("layer-dilated");
    let scaled = radii("layer-dilated-under-scale");

    assert!(
        !unscaled.is_empty(),
        "no morphology material in layer-dilated, so this compared nothing"
    );

    let doubled: Vec<[f32; 2]> = unscaled
        .iter()
        .map(|pair| [pair[0] * 2.0, pair[1] * 2.0])
        .collect();
    assert_eq!(
        scaled, doubled,
        "the scene under a scale of two should dilate twice as far. If the radius \
         was deliberately returned to device pixels, this test is what says so -- \
         assert equality here again and update docs/non-parity.md 17."
    );

    // And the unscaled scene reaches what it asked for, so the relation above is
    // two scenes agreeing on the right answer rather than on nothing.
    let reached = |rows: &[[f32; 2]]| {
        let mut v: Vec<f32> = rows
            .iter()
            .flat_map(|pair| pair.iter().copied())
            .filter(|d| *d > 0.0)
            .collect();
        v.sort_by(f32::total_cmp);
        v
    };
    assert_eq!(
        reached(&unscaled),
        vec![3.0, 8.0],
        "the scene asks for radii of eight and three"
    );
    assert_eq!(
        reached(&scaled),
        vec![6.0, 16.0],
        "the same radii under a scale of two are sixteen and six"
    );
}

/// An anisotropic scale is carried per axis, as upstream carries it.
///
/// `scale(2, 5)` on radii of eight and three. Upstream transforms one direction
/// vector per pass and takes a length from each -- `TransformDirection((1,0) * 8)`
/// is sixteen long, `TransformDirection((0,1) * 3)` is fifteen -- and since the
/// conversion here became per axis, so does this: `axis_scales_of` takes the
/// lengths of the transformed basis vectors, which is what upstream's
/// `ExtractScale` takes.
///
/// This assertion is the inverse of the one that stood here first, in the same
/// way the scaled pair's is. That one asserted forty and fifteen -- both radii
/// times the larger factor -- and existed so the per-axis conversion could not
/// land quietly. It failed with exactly the pair below and named its own
/// replacement, which is what writing the scene before the fix buys.
///
/// The pass count is part of it. `MORPHOLOGY_TAPS` is thirty-two, so forty needed
/// two passes along x and sixteen needs one: three morphology passes became two,
/// and the cost baseline moved without anyone reading a radius.
#[test]
fn the_dilation_under_an_anisotropic_scale_converts_per_axis() {
    let rows = radii("layer-dilated-under-anisotropic-scale");
    assert!(
        !rows.is_empty(),
        "no morphology material in the scene, so this compared nothing"
    );

    let along = |axis: usize| -> f32 { rows.iter().map(|pair| pair[axis]).sum() };
    let (x, y) = (along(0), along(1));

    assert_eq!(
        [x, y],
        [16.0, 15.0],
        "a radius of eight and three under scale(2, 5) should reach eight times \
         two along x and three times five along y. This reads {x} and {y}. Forty \
         and fifteen means the conversion went back to a single factor -- the \
         larger of the two -- which is what docs/non-parity.md 17 was about."
    );

    assert_eq!(
        rows.len(),
        2,
        "expected one morphology pass per axis: sixteen and fifteen both fit in \
         MORPHOLOGY_TAPS, where the forty a single factor gave did not: {rows:?}"
    );
}

/// A dilation turns with its caller, which no picture here could have caught.
///
/// The radius is unchanged under a rotation -- `axis_scales_of` takes the lengths
/// of the transformed basis vectors and a rotation stretches neither -- so nothing
/// a material carries differs except the direction each pass walks. That is why
/// this is an assertion on `step` and not a scene: `docs/non-parity.md` 17 says
/// the corpus cannot see a conversion at all, since every comparison it makes is
/// between two backends or two devices and the conversion sits above both, so both
/// sides would be wrong together and agree.
///
/// Built from a `Canvas` rather than the corpus for the same reason the corpus
/// cannot check it. A scene would only be a picture.
///
/// Under a quarter turn the caller's x points along device y and the caller's y
/// along device negative x, so the radii stay eight and three and travel there
/// instead. Before this, both passes walked the target's own axes and a rotated
/// square dilated to a square in the wrong orientation.
#[test]
fn a_dilation_walks_the_callers_axes_after_a_quarter_turn() {
    use emblema_core::{Canvas, Color, Extent2D, Layer, Morphology, Paint, Rect};

    let mut canvas = Canvas::new(Extent2D {
        width: 256,
        height: 256,
    });
    canvas.save();
    canvas.rotate(std::f32::consts::FRAC_PI_2);
    let mut layer = Layer::opacity(1.0);
    layer.morphology = Some(Morphology::dilate(8.0, 3.0));
    canvas.save_layer(layer);
    canvas
        .draw_rect(
            Rect::new(-60.0, 20.0, -20.0, 60.0),
            &Paint::fill(Color::WHITE),
        )
        .expect("a rect inside the turned space");
    canvas.restore();
    canvas.restore();

    let recording = canvas.finish();
    let passes: Vec<([f32; 2], f32)> = recording
        .passes
        .iter()
        .flat_map(|pass| pass.batch.draws())
        .filter_map(|draw| match draw.material {
            Material::Morphology { step, radius, .. } => Some((step, radius)),
            _ => None,
        })
        .collect();

    assert_eq!(
        passes.len(),
        2,
        "one pass per axis, eight and three both inside MORPHOLOGY_TAPS: {passes:?}"
    );

    // The step is a direction divided componentwise by the extent, so its
    // *orientation* is what carries the turn. Compared by which component
    // dominates rather than against a number, because the extent that divides it
    // is the layer's and not the frame's.
    let dominant = |step: [f32; 2]| {
        if step[0].abs() > step[1].abs() {
            'x'
        } else {
            'y'
        }
    };
    assert_eq!(
        (dominant(passes[0].0), passes[0].1),
        ('y', 8.0),
        "the caller's x axis points along device y after a quarter turn, and \
         carries the x radius: {passes:?}"
    );
    assert_eq!(
        (dominant(passes[1].0), passes[1].1),
        ('x', 3.0),
        "and the caller's y axis points along device x: {passes:?}"
    );
    assert!(
        passes[1].0[0] < 0.0,
        "along device negative x, since a quarter turn takes y to -x: {passes:?}"
    );
}

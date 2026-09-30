//! What space a morphology radius is in, pinned.
//!
//! `dilate` and `erode` take a radius, and the two renderers disagree about what
//! it measures. Here it is device pixels and nothing scales it. Upstream's is a
//! local length that `entity.GetTransform() * effect_transform.Basis()` scales at
//! the pass, so a dilated layer under a scale of three spreads three times as
//! far there and the same distance here. `docs/non-parity.md` 17 is the entry,
//! with what a fix costs and why it is not the multiplication it looks like.
//!
//! This file exists so that flipping the convention is *visible*. The corpus has
//! a pair of scenes built to differ in nothing but the transform -- the same
//! cross at half the size under a scale of two, landing on exactly the pixels the
//! unscaled one covers -- so the dilation distance is the only thing left that
//! can separate them.
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

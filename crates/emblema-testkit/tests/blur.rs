//! Which factor converts a blur's deviations, pinned.
//!
//! `morphology.rs` is this file's other half, and the question is the same one:
//! `dart:ui` states a length per axis in the space the caller was drawing in, and
//! something has to put it in device pixels. Upstream uses `ExtractScale`, which
//! takes the length of each transformed basis vector. The morphology radius took a
//! single factor -- the larger of those two -- until the conversion became per axis;
//! a blur's deviations still take it.
//!
//! `docs/non-parity.md` 20 is the entry, with the measurement and why it is recorded
//! rather than fixed in the change that fixed morphology.
//!
//! # Why this cannot be a picture or a cost row
//!
//! The corpus compares two backends against each other and two devices against each
//! other. The conversion happens above both, in `Layer::scaled_by` and
//! `ImageFilter::scaled_by`, so every side of every comparison is wrong together and
//! agrees -- exactly the trap `layer-composed-filter-under-scale` records for the
//! filter conversion it was written for.
//!
//! The cost row is no better here, and that took care to arrange rather than luck.
//! A deviation drives the reduction of entry 6 as well as the blur: past a kernel
//! radius of `BLUR_MAX_TAPS` a pass halves its target and divides the deviation,
//! which moves the pass count. The scene's deviations are small enough that neither
//! the inflated pair nor the corrected one reduces, so the pass count is the same
//! either way and cannot stand in for reading the numbers.
//!
//! So the deviations are read out of the material, which is what this does.
//!
//! Recording only, so it runs with no device and on every merge.

use emblema_hal::Material;
use emblema_testkit::{corpus, record_scene};

/// The blur deviations a scene's recording ends up carrying, per axis.
///
/// Keyed by which axis the pass steps along rather than by pass order, because the
/// order is an implementation detail of `blur_passes` and the question is which
/// deviation went with which axis.
fn sigmas(name: &str) -> Vec<(char, f32)> {
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
            Material::Blur { step, sigma, .. } => Some((
                if step[0].abs() > step[1].abs() {
                    'x'
                } else {
                    'y'
                },
                sigma,
            )),
            _ => None,
        })
        .collect()
}

/// An anisotropic scale is carried by one factor, where upstream carries two.
///
/// `scale(2, 3)` on deviations of six and two. Upstream reaches twelve along x and
/// six along y, one factor per axis. Here `max_scale_of` gives three for both, so x
/// reaches eighteen -- and `scale(3, 2)` would give eighteen and six as well, which
/// is the part worth stating plainly: two transforms that transpose each other
/// produce an identical blur.
///
/// **This asserts today's behavior, deliberately**, the way the morphology pair's
/// tests did before each of those landed. `docs/non-parity.md` 20 records the defect
/// and says the scene comes before the fix; this is that scene's pin, so the fix has
/// to fail here and say what replaces it.
#[test]
fn a_blur_under_an_anisotropic_scale_uses_one_factor() {
    let found = sigmas("layer-blurred-under-anisotropic-scale");
    assert_eq!(
        found,
        vec![('x', 18.0), ('y', 6.0)],
        "deviations of six and two under scale(2, 3) should reach six times two \
         along x and two times three along y, which is what upstream reaches. Both \
         times three is what a single `max_scale_of` gives. If this now reads twelve \
         and six, the per-axis conversion landed -- which is the intended change: \
         assert that instead, and close docs/non-parity.md 20."
    );
}

/// And the deviations stay clear of the reduction, which is what lets the test above
/// mean what it says.
///
/// Past a kernel radius of `BLUR_MAX_TAPS` a blur halves its target and divides the
/// deviation, so the pass count grows and the sigma read back is a reduced one.
/// Eighteen is the largest deviation in play and the threshold is a little over
/// nineteen, so nothing here reduces -- and the corrected twelve is further under it
/// still, which is the property that keeps the fix's cost row still.
///
/// Asserted on the *whole* recording rather than on the blur materials, because a
/// halving is a pass of its own carrying a different material: counting blurs would
/// read two either way and prove nothing.
#[test]
fn the_scene_does_not_reach_the_reduction_either_way() {
    let scene = corpus()
        .into_iter()
        .find(|scene| scene.name == "layer-blurred-under-anisotropic-scale")
        .ok_or("layer-blurred-under-anisotropic-scale")
        .expect("the corpus has this scene");
    let recording = record_scene(&scene).expect("a corpus scene records");
    assert_eq!(
        recording.passes.len(),
        4,
        "the root, the layer, and one blur pass per axis. More means a deviation \
         reached the reduction and the recording gained a halving, which would make \
         `a_blur_under_an_anisotropic_scale_uses_one_factor` a test about the \
         halving instead of about the conversion. Give the scene smaller deviations \
         rather than updating this number."
    );
}

//! Which factor converts a blur's deviations, pinned.
//!
//! `morphology.rs` is this file's other half, and the question is the same one:
//! `dart:ui` states a length per axis in the space the caller was drawing in, and
//! something has to put it in device pixels. Upstream uses `ExtractScale`, which
//! takes the length of each transformed basis vector. Both pairs here now do the
//! same; a blur's deviations took a single factor -- the larger of the two -- until
//! `docs/non-parity.md` 20 was closed.
//!
//! What is left is `Layer::backdrop_blur`, which is one number rather than a pair,
//! and the last test below pins it.
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

/// An anisotropic scale is carried per axis, as upstream carries it.
///
/// `scale(2, 3)` on deviations of six and two reaches twelve along x and six along
/// y: each component takes the length of its own transformed basis vector, which is
/// what upstream's `ExtractScale` takes.
///
/// This assertion is the inverse of the one that stood here first, which read
/// eighteen and six -- both deviations times the larger factor -- and existed so the
/// per-axis conversion could not land quietly. It failed with exactly the pair below
/// and named its own replacement, which is what writing the scene before the fix
/// buys. The morphology pair's tests were written the same way for the same reason.
#[test]
fn a_blur_under_an_anisotropic_scale_converts_per_axis() {
    let found = sigmas("layer-blurred-under-anisotropic-scale");
    assert_eq!(
        found,
        vec![('x', 12.0), ('y', 6.0)],
        "deviations of six and two under scale(2, 3) should reach six times two \
         along x and two times three along y. Eighteen along x means the conversion \
         went back to a single factor -- the larger of the two -- which is what \
         docs/non-parity.md 20 was about."
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

/// Two transforms that transpose each other now produce transposed blurs.
///
/// This is the property the single factor destroyed, and the clearest statement of
/// why it was a defect rather than an approximation: under `scale(2, 3)` and
/// `scale(3, 2)` a blur here came out *identical*, because the larger factor is
/// three either way. A caller cannot have meant that -- the two transforms are
/// mirror images and so are the pictures they should make.
///
/// Built from a `Canvas` rather than a second scene, because the two halves have to
/// be recorded in one test to be compared at all, and a corpus scene could not
/// compare them: every comparison the corpus makes is between two backends or two
/// devices, and the conversion sits above both.
#[test]
fn transposing_the_scale_transposes_the_deviations() {
    use emblema_core::{Canvas, Color, Extent2D, Layer, Paint, Rect};

    let recorded = |sx: f32, sy: f32| -> Vec<(char, f32)> {
        let mut canvas = Canvas::new(Extent2D {
            width: 256,
            height: 256,
        });
        canvas.save();
        canvas.scale(sx, sy);
        canvas.save_layer(Layer::opacity(1.0).with_blur_xy(6.0, 2.0));
        canvas
            .draw_rect(
                Rect::new(10.0, 10.0, 50.0, 40.0),
                &Paint::fill(Color::WHITE),
            )
            .expect("a rect inside the layer");
        canvas.restore();
        canvas.restore();
        canvas
            .finish()
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
    };

    let wide = recorded(2.0, 3.0);
    let tall = recorded(3.0, 2.0);
    assert_eq!(
        wide,
        vec![('x', 12.0), ('y', 6.0)],
        "six by two under scale(2, 3)"
    );
    assert_eq!(tall, vec![('x', 18.0), ('y', 4.0)], "and under scale(3, 2)");
    assert_ne!(
        wide, tall,
        "transposing the scale has to change the blur. Equal here is the defect \
         docs/non-parity.md 20 recorded: one factor cannot tell two transposed \
         transforms apart"
    );
}

/// A backdrop blur is converted per axis too, and the field never had to change.
///
/// `Layer::backdrop_blur` is one `f32` and `with_backdrop_blur` takes one sigma, so a
/// caller states a blur that is round *in their own space*. Under a transform whose
/// axes scale differently that is an oval in device space, and one number cannot hold
/// an oval -- which is why this used to come out round, at the larger factor on both
/// axes, where the same deviations on the layer itself stretched.
///
/// `docs/non-parity.md` 20 said closing this meant making the field a pair and
/// changing a published type. It did not. The field holds the caller's number and
/// always did; what was wrong was converting it with one factor in
/// `Layer::scaled_by`. It is now converted where it becomes an `ImageFilter::Blur`,
/// which has two components to put the answer in, and the public API is untouched.
///
/// Six under `scale(2, 3)` reaches twelve across and eighteen down; under
/// `scale(3, 2)` it reaches eighteen and twelve. Transposing the transform transposes
/// the blur, which is the property that says one number is no longer collapsing two.
#[test]
fn a_backdrop_blur_is_converted_per_axis() {
    use emblema_core::{Canvas, Color, Extent2D, Layer, Paint, Rect};

    let recorded = |sx: f32, sy: f32| -> Vec<f32> {
        let mut canvas = Canvas::new(Extent2D {
            width: 256,
            height: 256,
        });
        canvas
            .draw_rect(
                Rect::new(0.0, 0.0, 200.0, 200.0),
                &Paint::fill(Color::srgb(0.2, 0.3, 0.4, 1.0)),
            )
            .expect("something to blur");
        canvas.save();
        canvas.scale(sx, sy);
        canvas.save_layer(Layer::opacity(1.0).with_backdrop_blur(6.0));
        canvas
            .draw_rect(
                Rect::new(10.0, 10.0, 50.0, 40.0),
                &Paint::fill(Color::WHITE),
            )
            .expect("a rect over it");
        canvas.restore();
        canvas.restore();
        canvas
            .finish()
            .passes
            .iter()
            .flat_map(|pass| pass.batch.draws())
            .filter_map(|draw| match draw.material {
                Material::Blur { sigma, .. } => Some(sigma),
                _ => None,
            })
            .collect()
    };

    let wide = recorded(2.0, 3.0);
    let tall = recorded(3.0, 2.0);
    assert_eq!(
        wide,
        vec![12.0, 18.0],
        "six by two across and by three down"
    );
    assert_eq!(tall, vec![18.0, 12.0], "and the other way round");
    assert_ne!(
        wide, tall,
        "transposing the scale has to transpose the backdrop blur. Equal here, at the \
         larger factor on both axes, is what docs/non-parity.md 20 recorded before \
         this was converted where it becomes a filter"
    );
}

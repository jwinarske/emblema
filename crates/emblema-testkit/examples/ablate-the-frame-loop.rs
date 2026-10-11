//! Which ingredient of #180's case makes a scene unstable across frames.
//!
//! `a_generated_scene_survives_other_frames` found a subject that renders
//! differently after other frames on GLES, and
//! `reaches_the_gles_instability` did not exclude it. Three things could
//! explain that, and the predicate cannot be widened honestly until it is
//! known which:
//!
//! 1. the subject is **single-sample**, where the predicate requires
//!    `samples > 1`;
//! 2. the subject's blur is a **layer blur** -- `LayerSpec::blur` -- which
//!    `has_mask_blur` never looks at, since it matches a `Draw`'s `mask_blur`
//!    and recurses only into a layer's children;
//! 3. the predicate examines **only the subject**, and here it is the *other*
//!    frames that are multisampled with a mask blur and an advanced blend.
//!
//! So this is the shrunk case reduced by hand, with each of those removable on
//! its own. `as-found` is the shape the generator produced; every other
//! variant takes one thing out of it. A variant that reads zero names an
//! ingredient the instability needs.
//!
//! ```text
//! cargo run -p emblema-testkit --example ablate-the-frame-loop
//! ```
//!
//! One process renders every variant, each in a context of its own, because
//! the thing being measured is what one frame leaves for the next -- a shared
//! context would read the variant before it as much as the variant.

// A probe is a program, and it reports by printing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use emblema_hal::{BlendMode, Hal};
use emblema_hal_gles::{DisplayTarget, GlesHal, Validated as GlesValidated};
use emblema_testkit::scene::Fill;
use emblema_testkit::{render_scene, Item, LayerSpec, Node, Scene, Shape, Transform};

/// What a variant leaves in.
#[derive(Clone, Copy)]
struct Shape_ {
    /// Samples on the subject. The predicate requires more than one.
    subject_samples: u32,
    /// The subject's layer carries a blur, which is the kind `has_mask_blur`
    /// does not see.
    subject_layer_blur: bool,
    /// The subject's layer and one of its children blend `Overlay`.
    subject_advanced: bool,
    /// The other frames are multisampled.
    others_samples: u32,
    /// One of the other frames has a draw with a mask blur.
    others_mask_blur: bool,
    /// The other frames reach an advanced equation.
    others_advanced: bool,
    /// Every `clip` and `clip_out` the case carries, on both sides.
    ///
    /// Last to be suspected and first in this file's own history: the failure
    /// that put `frame_loop.rs` here was "a clip left the scissor enabled, and
    /// the multisample resolve -- a blit, and blits are scissored -- copied
    /// only the part of the frame the previous draw could touch."
    clips: bool,
}

const AS_FOUND: Shape_ = Shape_ {
    subject_samples: 1,
    subject_layer_blur: true,
    subject_advanced: true,
    others_samples: 4,
    others_mask_blur: true,
    others_advanced: true,
    clips: true,
};

const VARIANTS: &[(&str, Shape_, &str)] = &[
    ("as-found", AS_FOUND, "the shape the generator produced"),
    (
        "subject-multisampled",
        Shape_ {
            subject_samples: 4,
            ..AS_FOUND
        },
        "the subject at four samples, which the predicate would exclude",
    ),
    (
        "subject-no-layer-blur",
        Shape_ {
            subject_layer_blur: false,
            ..AS_FOUND
        },
        "the subject's layer carries no blur",
    ),
    (
        "subject-no-advanced",
        Shape_ {
            subject_advanced: false,
            ..AS_FOUND
        },
        "the subject blends ordinarily throughout",
    ),
    (
        "others-plain",
        Shape_ {
            others_samples: 1,
            others_mask_blur: false,
            others_advanced: false,
            ..AS_FOUND
        },
        "the other frames are a plain single-sample fill -- the test of \
         whether the subject is unstable by itself",
    ),
    (
        "others-single-sample",
        Shape_ {
            others_samples: 1,
            ..AS_FOUND
        },
        "the other frames keep the blur and the equation, at one sample",
    ),
    (
        "others-no-mask-blur",
        Shape_ {
            others_mask_blur: false,
            ..AS_FOUND
        },
        "the other frames keep the samples and the equation, with no blur",
    ),
    (
        "no-clips",
        Shape_ {
            clips: false,
            ..AS_FOUND
        },
        "every clip and difference clip taken out, on both sides",
    ),
    (
        "others-no-advanced",
        Shape_ {
            others_advanced: false,
            ..AS_FOUND
        },
        "the other frames keep the samples and the blur, blending ordinarily",
    ),
];

const SIDE: u32 = 128;

/// The subject: a fill, then a layer holding two draws.
///
/// The rectangles are the shrunk case's, rounded to whole pixels. What they
/// are is not the point -- that the layer overlaps the fill under it is.
fn subject(what: Shape_) -> Scene {
    let advanced = if what.subject_advanced {
        BlendMode::Overlay
    } else {
        BlendMode::SrcOver
    };
    let mut under = Item::filled(
        Shape::Rect {
            min: [6.0, 26.0],
            max: [49.0, 43.0],
        },
        Fill::Solid([0.547_580_6, 0.339_360_3, 0.171_661_6, 1.0]),
    );
    let mut first = Item::filled(
        Shape::Rect {
            min: [31.0, 21.0],
            max: [62.0, 42.0],
        },
        Fill::Solid([0.822_578_67, 0.279_028_27, 0.669_239_34, 1.0]),
    );
    let mut second = Item::filled(
        Shape::Rect {
            min: [10.0, 23.0],
            max: [44.0, 64.0],
        },
        Fill::Solid([0.167_555_4, 0.442_319_3, 0.497_562_4, 0.772_613_64]),
    )
    .with_blend(advanced);
    if what.clips {
        under = under.with_clip_out([38.0, 29.0, 89.0, 81.0]);
        first = first
            .with_clip([39.0, 17.0, 68.0, 57.0])
            .with_clip_out([16.0, 2.0, 40.0, 44.0]);
        second = second.with_clip([17.0, 25.0, 39.0, 74.0]);
    }
    let mut spec = LayerSpec::opacity(0.680_524_2).with_blend(advanced);
    if what.subject_layer_blur {
        spec = spec.with_blur(3.128_923_4);
    }
    Scene::tree(
        "subject",
        vec![
            Node::Draw(Box::new(under)),
            Node::Layer {
                layer: Box::new(spec),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(first)), Node::Draw(Box::new(second))],
            },
        ],
    )
    .with_samples(what.subject_samples)
}

/// The three frames between the subject's two renders, as the case has them.
///
/// Reduced to one at first, which did not reproduce. All three, with their
/// clips, do -- so the list is kept whole and what varies is the ingredients.
fn others(what: Shape_) -> Vec<Scene> {
    let advanced = if what.others_advanced {
        BlendMode::Difference
    } else {
        BlendMode::SrcOver
    };
    // The first: ordinary, with a `Src` draw in it.
    let mut a0 = Item::filled(
        Shape::Rect {
            min: [31.0, 17.0],
            max: [54.0, 62.0],
        },
        Fill::Solid([0.340_637_24, 0.093_571_63, 0.658_745_4, 0.227_305_92]),
    );
    let mut a1 = Item::filled(
        Shape::Rect {
            min: [14.0, 39.0],
            max: [71.0, 55.0],
        },
        Fill::Solid([0.253_140_78, 0.868_054_4, 0.682_007_6, 1.0]),
    )
    .with_blend(BlendMode::Src);
    // The second: multisampled, and the one carrying the mask blur.
    let b0 = Item::filled(
        Shape::Rect {
            min: [9.0, 33.0],
            max: [25.0, 71.0],
        },
        Fill::Solid([0.296_560_17, 0.033_283_897, 0.336_666_2, 0.894_695_6]),
    );
    let mut b1 = Item::filled(
        Shape::Rect {
            min: [22.0, 27.0],
            max: [54.0, 56.0],
        },
        Fill::Solid([0.700_353_74, 0.590_603_65, 0.287_766_25, 1.0]),
    );
    if what.others_mask_blur {
        b1 = b1.with_mask_blur(4.018_061_6);
    }
    // The third: multisampled, a layer whose child reaches the equation.
    let mut c0 = Item::filled(
        Shape::Rect {
            min: [36.0, 17.0],
            max: [67.0, 42.0],
        },
        Fill::Solid([0.249_706_09, 0.282_845_02, 0.295_750_5, 1.0]),
    )
    .with_blend(advanced);
    if what.clips {
        a0 = a0.with_clip_out([9.0, 4.0, 66.0, 36.0]);
        a1 = a1
            .with_clip([32.0, 35.0, 91.0, 55.0])
            .with_clip_out([11.0, 6.0, 57.0, 48.0]);
        b1 = b1
            .with_clip([34.0, 4.0, 62.0, 35.0])
            .with_clip_out([19.0, 30.0, 46.0, 53.0]);
        c0 = c0.with_clip_out([15.0, 28.0, 34.0, 53.0]);
    }
    vec![
        Scene::tree(
            "other",
            vec![Node::Draw(Box::new(a0)), Node::Draw(Box::new(a1))],
        )
        .with_samples(1),
        Scene::tree(
            "other",
            vec![Node::Draw(Box::new(b0)), Node::Draw(Box::new(b1))],
        )
        .with_samples(what.others_samples),
        Scene::tree(
            "other",
            vec![Node::Layer {
                layer: Box::new(LayerSpec::opacity(0.984_054_4).with_blend(BlendMode::SrcOver)),
                bounds: None,
                transform: Transform::default(),
                children: vec![Node::Draw(Box::new(c0))],
            }],
        )
        .with_samples(what.others_samples),
    ]
}

/// Render the subject, then the others, then the subject again.
///
/// `frame_loop.rs`'s `repeat`, which is a test and cannot be called from here.
fn repeat(ctx: &mut <GlesHal as Hal>::Context, subject: &Scene, others: &[Scene]) -> Option<i32> {
    if !subject.supported_by(ctx.capabilities()) {
        return None;
    }
    let first = render_scene::<GlesHal>(ctx, subject).ok()?;
    for other in others {
        if other.supported_by(ctx.capabilities()) {
            let _ = render_scene::<GlesHal>(ctx, other);
        }
    }
    let again = render_scene::<GlesHal>(ctx, subject).ok()?;
    Some(
        first
            .pixels
            .iter()
            .zip(again.pixels.iter())
            .map(|(a, b)| i32::from(*a) - i32::from(*b))
            .map(i32::abs)
            .max()
            .unwrap_or(0),
    )
}

fn main() {
    let wanted: Option<String> = std::env::args().nth(1);
    if wanted.as_deref() == Some("--list") {
        println!("variants:");
        for (name, _, what) in VARIANTS {
            println!("  {name:<24} {what}");
        }
        return;
    }
    println!("side: {SIDE}");

    for (name, what, _) in VARIANTS {
        if let Some(only) = wanted.as_deref() {
            if only != *name {
                continue;
            }
        }
        // A context per variant: what is being measured is what one frame
        // leaves for the next, so a shared one would carry the variant before.
        let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
            println!("skipping: no GLES context");
            return;
        };
        if !gles.capabilities().advanced_blend {
            println!("skipping: no advanced blending");
            return;
        }
        let subject = subject(*what);
        let others = others(*what);
        match repeat(&mut gles, &subject, &others) {
            Some(worst) => println!("{name:<24} worst {worst:>3} level(s)"),
            None => println!("{name:<24} declined"),
        }
    }
}

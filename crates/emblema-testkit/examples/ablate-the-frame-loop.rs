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
//! `SUBJECT_MODE` and `OTHERS_MODE` name the advanced equation each side
//! blends with -- `overlay`, `hardlight`, `difference` and the rest -- which
//! is how the miscompile was narrowed to one of them. `MAP=1` draws the
//! differing pixels and `AT=x,y` prints one from both renders.
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
    /// Only the layer's own composite blends it, not the child draw.
    subject_advanced_layer_only: bool,
    /// Only the child draw blends it, not the layer's composite.
    subject_advanced_draw_only: bool,
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
    subject_advanced_layer_only: false,
    subject_advanced_draw_only: false,
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
        "subject-layer-blend-only",
        Shape_ {
            subject_advanced_layer_only: true,
            ..AS_FOUND
        },
        "the layer composites `Overlay` and the draw inside it blends ordinarily",
    ),
    (
        "subject-draw-blend-only",
        Shape_ {
            subject_advanced_draw_only: true,
            ..AS_FOUND
        },
        "the draw inside blends `Overlay` and the layer composites ordinarily",
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

/// Which advanced equation a side blends with, from the environment.
///
/// The case was found with `Overlay` on the subject and `Difference` on the
/// other frames, and those are the defaults. They are selectable because the
/// subject's equation turned out to be the whole of what the miscompile is
/// about: of the eleven advanced modes, only `overlay` and `hardlight`
/// reproduce, and those two are one equation with its operands swapped. The
/// other frames' equation does not matter -- every one of the eleven
/// reproduces there -- so what they owe is an advanced blend and not a
/// particular one. See `docs/on-a-board.md`.
fn mode_from(var: &str, fallback: BlendMode) -> BlendMode {
    match std::env::var(var).unwrap_or_default().as_str() {
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "colordodge" => BlendMode::ColorDodge,
        "colorburn" => BlendMode::ColorBurn,
        "hardlight" => BlendMode::HardLight,
        "softlight" => BlendMode::SoftLight,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        _ => fallback,
    }
}

/// The subject's layer blur, default the shrunk case's own 3.1289234.
///
/// A knob because the deviation decides the *trip count* of the blur's tap
/// loop, and the trip count turns out to be the ingredient: `solid.wgsl` reaches
/// `(sigma - 0.5) * sqrt(3)` each way, so this value gives five taps each side
/// and eleven iterations. Eleven to seventeen reproduce and nothing outside
/// that does -- see `docs/on-a-board.md` and
/// `probe-the-layer-composite`, where the window was found.
fn blur_sigma() -> f32 {
    std::env::var("SIGMA")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3.128_923_4)
}

const SIDE: u32 = 128;

/// The subject: a fill, then a layer holding two draws.
///
/// The rectangles are the shrunk case's, rounded to whole pixels. What they
/// are is not the point -- that the layer overlaps the fill under it is.
fn subject(what: Shape_) -> Scene {
    let advanced = if what.subject_advanced {
        mode_from("SUBJECT_MODE", BlendMode::Overlay)
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
    // Which of the subject's two advanced blends is in force. `as-found` has
    // both; the two variants below keep one each, which is what says whether
    // the defect rides on the layer's composite or on the draw inside it --
    // and the composite is drawn by this renderer's own shader where the draw
    // can be given a caller's.
    let draw_blend = if what.subject_advanced_layer_only {
        BlendMode::SrcOver
    } else {
        advanced
    };
    let layer_blend = if what.subject_advanced_draw_only {
        BlendMode::SrcOver
    } else {
        advanced
    };
    let mut second = Item::filled(
        Shape::Rect {
            min: [10.0, 23.0],
            max: [44.0, 64.0],
        },
        Fill::Solid([0.167_555_4, 0.442_319_3, 0.497_562_4, 0.772_613_64]),
    )
    .with_blend(draw_blend);
    if what.clips {
        under = under.with_clip_out([38.0, 29.0, 89.0, 81.0]);
        first = first
            .with_clip([39.0, 17.0, 68.0, 57.0])
            .with_clip_out([16.0, 2.0, 40.0, 44.0]);
        second = second.with_clip([17.0, 25.0, 39.0, 74.0]);
    }
    let mut spec = LayerSpec::opacity(0.680_524_2).with_blend(layer_blend);
    if what.subject_layer_blur {
        spec = spec.with_blur(blur_sigma());
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
        mode_from("OTHERS_MODE", BlendMode::Difference)
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
    let worst = first
        .pixels
        .iter()
        .zip(again.pixels.iter())
        .map(|(a, b)| i32::from(*a) - i32::from(*b))
        .map(i32::abs)
        .max()
        .unwrap_or(0);

    // `MAP=1` draws which pixels differ and names the worst, with both
    // renders' value there. A count of levels says how far wrong without
    // saying where, and a difference cannot say which of the two renders is
    // the correct one -- the value against a driver that agrees with the
    // others can. Both measurements are what named the first instability's
    // mechanism, and neither had been taken here.
    // `AT=x,y` prints that pixel from both renders whether or not they differ,
    // which is what a clean driver has to be asked for: its answer is the one
    // the two are measured against, and it has no difference to report.
    if let Some((x, y)) = std::env::var("AT").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((
            a.trim().parse::<usize>().ok()?,
            b.trim().parse::<usize>().ok()?,
        ))
    }) {
        let rgba = |p: &[u8]| {
            let i = ((y * SIDE as usize) + x) * 4;
            format!("[{:3},{:3},{:3},{:3}]", p[i], p[i + 1], p[i + 2], p[i + 3])
        };
        println!(
            "  at ({x},{y}): first {} again {}",
            rgba(&first.pixels),
            rgba(&again.pixels)
        );
    }
    if worst > 0 && std::env::var_os("MAP").is_some() {
        let differing = first
            .pixels
            .iter()
            .zip(again.pixels.iter())
            .filter(|(a, b)| a != b)
            .count();
        // Which way, and in which channel. A one-sided error is a different
        // claim from a wobble: dither or rounding scatters both ways, and a
        // contribution that is lost or doubled does not.
        let (mut lower, mut higher) = (0usize, 0usize);
        let mut channels = [0usize; 4];
        for (i, (a, b)) in first.pixels.iter().zip(again.pixels.iter()).enumerate() {
            match b.cmp(a) {
                std::cmp::Ordering::Less => {
                    lower += 1;
                    channels[i % 4] += 1;
                }
                std::cmp::Ordering::Greater => {
                    higher += 1;
                    channels[i % 4] += 1;
                }
                std::cmp::Ordering::Equal => {}
            }
        }
        println!("  {differing} byte(s) of {} differ", first.pixels.len());
        println!(
            "  the second render is lower in {lower} and higher in {higher}; \
             by channel r{} g{} b{} a{}",
            channels[0], channels[1], channels[2], channels[3]
        );
        for y in 0..SIDE as usize {
            let mut row = String::new();
            let mut any = false;
            for x in 0..SIDE as usize {
                let at = ((y * SIDE as usize) + x) * 4;
                let d = (0..4)
                    .map(|c| {
                        (i32::from(first.pixels[at + c]) - i32::from(again.pixels[at + c])).abs()
                    })
                    .max()
                    .unwrap_or(0);
                any |= d > 0;
                row.push(match d {
                    0 => '.',
                    1 => '-',
                    2..=4 => '+',
                    _ => '#',
                });
            }
            if any {
                println!("  y={y:3} {row}");
            }
        }
        if let Some(at) = first
            .pixels
            .iter()
            .zip(again.pixels.iter())
            .position(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() == worst)
        {
            let (x, y) = ((at / 4) % SIDE as usize, (at / 4) / SIDE as usize);
            let rgba = |p: &[u8]| {
                let i = ((y * SIDE as usize) + x) * 4;
                format!("[{:3},{:3},{:3},{:3}]", p[i], p[i + 1], p[i + 2], p[i + 3])
            };
            println!(
                "  worst at ({x},{y}): first {} again {}",
                rgba(&first.pixels),
                rgba(&again.pixels)
            );
        }
    }
    Some(worst)
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

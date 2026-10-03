//! The corpus, pinned as images.
//!
//! Every other comparison in this workspace is between two renders of *this* renderer --
//! two backends, two devices, or the same scene twice. None of them can see a change that is
//! wrong the same way on both sides, and that is not hypothetical: the rectangle route flip
//! and occlusion culling both altered which pixels get drawn, and each was checked by a
//! comparison written by hand for that one change. `emblema-shaders`' snapshot test says the
//! same thing about generated code; this says it about pixels.
//!
//! **It answers "did this change the picture", not "is the picture right."** There is no
//! external reference here and deliberately so -- `docs/architecture.md`'s L2 row records why
//! a Skia reference was rejected, and the reasons stand. A golden that moves is a question for
//! whoever moved it, which is the whole of what it is for.
//!
//! Set `UPDATE_GOLDEN_IMAGES=1` to rewrite them. That is the reviewed event and the diff is
//! the claim, exactly as `UPDATE_COST_BASELINE` and `UPDATE_SHADER_SNAPSHOTS` are.
//!
//! Pinned to the software reference, which is the device `docs/architecture.md` names for this
//! rung and the one CI has. A hardware device is not held to these: the tolerances below are
//! wide enough for a conformant driver and no wider, and `conformance.rs` is what compares
//! hardware against software.

// A golden that does not match is a failure and a panic is how a test reports one. The
// comparison runs from helpers, so clippy's test-code exemption does not reach them.
#![allow(clippy::panic)]

use emblema_hal_vulkan::validation::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use emblema_testkit::{
    accepts, compare, corpus, record_scene, render_scene, Image, Item, Scene, Shape, StrokeSpec,
};
use std::path::{Path, PathBuf};

/// Where the images live: workspace level, beside `tests/bench-baselines`.
///
/// Outside this crate for the reason the shader snapshots are outside theirs -- what they pin
/// is a property of the whole renderer rather than of the crate whose test reads them.
fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/golden")
}

fn path_for(scene: &str) -> PathBuf {
    golden_dir().join(format!("{scene}.png"))
}

/// Eight-bit RGBA, no interlacing, default compression.
///
/// The fewest knobs that encode the pixels, so the same image always gives the same bytes and
/// a regeneration that changes nothing writes nothing.
fn write_png(path: &Path, image: &Image) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("golden directory");
    }
    let file = std::fs::File::create(path).expect("create golden");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&image.pixels).expect("png data");
}

fn read_png(path: &Path) -> Image {
    let file = std::fs::File::open(path).expect("open golden");
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().expect("png info");
    let mut pixels = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).expect("png frame");
    pixels.truncate(info.buffer_size());
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "{}: goldens are RGBA; regenerate with UPDATE_GOLDEN_IMAGES=1",
        path.display()
    );
    Image::new(info.width, info.height, pixels)
}

/// Whether this driver keeps a scissor when the pass is multisampled.
///
/// lavapipe on Mesa 25.2.8 and 15.0.6 writes the pixel to the left of a scissor at half
/// coverage, and 26.1.7, RADV and PanVK do not -- `emblema`'s `public_api.rs` probes for the
/// same defect and records the measurement. A golden cannot be trusted on a driver that does
/// it, and the answer is to say which scenes were given up rather than to widen a tolerance
/// until the defect fits: `image.rs` is explicit that a tolerance states where the
/// specification allows a difference, and nothing permits this one.
///
/// Probed rather than read from a version string, because a version is a guess about behavior
/// and this is the behavior.
fn honors_scissor_when_multisampled(ctx: &mut Validated) -> bool {
    // A stroke rather than a fill, which is the half of this that took finding: an
    // antialiased *fill* takes the analytic route at one sample, never opens a multisampled
    // pass, and never meets the defect. A stroke is tessellated, which is what asks for the
    // samples. `public_api.rs`'s probe records the same lesson.
    let probe = Scene::new(
        "scissor-probe",
        vec![Item::stroke(
            Shape::Line {
                from: [40.0, 64.0],
                to: [88.0, 64.0],
            },
            StrokeSpec::new(40.0),
            [1.0, 1.0, 1.0, 1.0],
        )
        .with_clip([56.0, 0.0, 128.0, 128.0])],
    )
    .with_samples(4);
    match render_scene::<VulkanHal>(ctx, &probe) {
        // The column just left of the clip, which stays background on a driver that honors it.
        Ok(image) => image.pixel(55, 64) == [0, 0, 0, 255],
        // Unanswerable rather than answered: a device that cannot draw the probe is held to
        // the goldens as usual, and whatever it does with them is reported there.
        Err(_) => true,
    }
}

#[test]
fn every_corpus_scene_matches_its_golden() {
    let updating = std::env::var_os("UPDATE_GOLDEN_IMAGES").is_some();
    let mut ctx = match Validated::new(DevicePreference::Software) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no software reference ({e})");
            return;
        }
    };
    let scenes = corpus();
    let honors_scissor = honors_scissor_when_multisampled(&mut ctx);

    let mut written = 0usize;
    let mut compared = 0usize;
    let mut skipped: Vec<&str> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();

    for scene in &scenes {
        let path = path_for(scene.name);
        let rendered = match render_scene::<VulkanHal>(&mut ctx, scene) {
            Ok(image) => image,
            Err(e) => {
                // A scene this device cannot draw is a skip, and a skip is named.
                eprintln!("skipping {}: {e}", scene.name);
                skipped.push(scene.name);
                continue;
            }
        };

        if updating {
            write_png(&path, &rendered);
            written += 1;
            continue;
        }

        if !path.exists() {
            missing.push(scene.name);
            continue;
        }
        if !honors_scissor && clips_and_multisamples(scene) {
            skipped.push(scene.name);
            continue;
        }

        let stored = read_png(&path);
        let tolerance = scene.tolerance();
        match compare(&rendered, &stored) {
            Ok(difference) if accepts(&difference, tolerance) => compared += 1,
            Ok(difference) => wrong.push(format!(
                "{}: {} pixels differ, worst {} levels, tolerance {} with {:.4} outliers allowed",
                scene.name,
                difference.differing,
                difference.max_delta,
                tolerance.per_channel,
                tolerance.outlier_fraction
            )),
            Err(e) => wrong.push(format!("{}: {e}", scene.name)),
        }
    }

    if updating {
        eprintln!(
            "wrote {written} golden image(s) to {}",
            golden_dir().display()
        );
        return;
    }

    // Stored images with no scene, so a deletion is as deliberate as a change. Without this a
    // renamed scene leaves its old image behind, passing forever while covering nothing.
    let orphans = orphaned_goldens(&scenes);

    if !skipped.is_empty() {
        eprintln!(
            "skipping {} scene(s) this driver cannot be held to: {}",
            skipped.len(),
            skipped.join(", ")
        );
    }
    eprintln!(
        "compared {compared} of {} corpus scenes against stored images",
        scenes.len()
    );

    assert!(
        missing.is_empty(),
        "no stored image for {} scene(s): {}. A scene with no golden is covered by nothing; \
         regenerate with UPDATE_GOLDEN_IMAGES=1 and review the diff",
        missing.len(),
        missing.join(", ")
    );
    assert!(
        orphans.is_empty(),
        "stored image(s) with no scene: {}. Delete them, or restore the scene they were \
         recorded for",
        orphans.join(", ")
    );
    assert!(
        wrong.is_empty(),
        "{} scene(s) no longer match their stored image:\n  {}\n\
         If that was the point, regenerate with UPDATE_GOLDEN_IMAGES=1 -- the diff is the claim",
        wrong.len(),
        wrong.join("\n  ")
    );
}

/// Scenes that both clip and multisample, which is what the scissor defect reaches.
///
/// Asked of the recording rather than of the scene description, because a scissor is what the
/// defect acts on and the recording is where scissors exist. Walking the scene's own nodes
/// would mean enumerating every spec that can carry a clip and keeping that list right.
///
/// A scene whose recording cannot be built counts as clipping, so an unknown is skipped rather
/// than asserted.
fn clips_and_multisamples(scene: &Scene) -> bool {
    if scene.samples <= 1 {
        return false;
    }
    match record_scene(scene) {
        Ok(recording) => recording.passes.iter().any(|pass| {
            pass.batch.draws().iter().any(|draw| {
                draw.clip.is_some() || draw.stencil != emblema_hal::ClipState::UNCLIPPED
            })
        }),
        Err(_) => true,
    }
}

/// Stored images naming no scene in the corpus.
fn orphaned_goldens(scenes: &[Scene]) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(golden_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "png") {
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if !scenes.iter().any(|s| s.name == stem) {
                out.push(stem);
            }
        }
    }
    out.sort();
    out
}

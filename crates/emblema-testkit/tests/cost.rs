//! What each corpus scene costs to record, as numbers that cannot drift.
//!
//! A frame's cost has two halves and only one of them can be gated here.
//! Wall-clock time needs a quiet machine and a real device, which is what
//! `cargo xtask bench --check` and `tests/bench-baselines/` are for and why
//! they are run by hand on a bench board rather than in CI. What a frame *does*
//! is a different quantity: how many passes it needs, how many draws go in
//! them, how much geometry those draws carry. Those are decided while
//! recording, before any device is involved, so they are the same number on
//! lavapipe and on V3D and on a machine with no GPU at all -- which means they
//! can be recorded once and checked on every commit, at no variance.
//!
//! That is the half that catches an *algorithmic* regression, and most
//! regressions worth catching are algorithmic. A change that stops sharing a
//! backdrop capture, or tessellates a stroke twice as finely, or splits a pass
//! that used to be one, moves these numbers and moves them by the same amount
//! everywhere. A change that costs ten per cent of a millisecond on one driver
//! does not, and no amount of timing in CI would separate it from the noise.
//!
//! # What it does not claim
//!
//! Device-independent is established by construction. Machine-independent is
//! not, and is a weaker claim than it sounds: a vertex count comes from a
//! segment count, which comes from float arithmetic, and while Rust does not
//! contract multiplies and adds behind your back there is no *guarantee* the
//! answer is identical on another architecture.
//!
//! So it was measured rather than argued. This file's baseline was recorded on
//! x86-64 and the same binary's table, cross-built for aarch64 and run on a
//! Raspberry Pi 5, matched it byte for byte on 2026-08-28. One board and one
//! date, which is not a proof and is a great deal better than a paragraph
//! explaining why it ought to hold. If a row ever moves on a board and nowhere
//! else, that is the thing to suspect -- and it is worth knowing rather than
//! worth suppressing.
//!
//! # Why the corpus and not the catalog
//!
//! Fifty-nine rows against nearly three hundred. The corpus is the collection
//! where every scene is there because it exercises something no other scene
//! does, which is the property that makes a moved row mean something; the
//! catalog is broad on purpose and a table of it would be mostly noise in the
//! diff of every commit that adds a plate.
//!
//! # Updating this
//!
//! `UPDATE_COST_BASELINE=1 cargo test -p emblema-testkit --test cost`, and
//! then read the diff: it is the claim that every number that moved was meant
//! to. The same posture the shader snapshots take, for the same reason -- a
//! file that is regenerated without being read is a file that records whatever
//! happened rather than what was intended.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it. A failed assumption in a test should stop the
// run; the workspace denies these because a *library* must not.
#![allow(clippy::panic)]

use emblema_testkit::{corpus, record_scene};
use std::fmt::Write as _;
use std::path::PathBuf;

/// What one scene costs, in quantities a device does not affect.
struct Cost {
    name: &'static str,
    passes: usize,
    draws: usize,
    vertices: usize,
    indices: usize,
    /// Texture bindings across every pass, which is what a slot table costs
    /// and what a draw has to rebind between.
    sources: usize,
    /// Gradients that did not fit in a material and were tabulated into an
    /// image, which is an upload per frame rather than four floats.
    ramps: usize,
}

fn costs() -> Vec<Cost> {
    corpus()
        .into_iter()
        .map(|scene| {
            let recording =
                record_scene(&scene).unwrap_or_else(|e| panic!("recording {}: {e}", scene.name));
            Cost {
                name: scene.name,
                passes: recording.passes.len(),
                draws: recording.passes.iter().map(|p| p.batch.draws().len()).sum(),
                vertices: recording
                    .passes
                    .iter()
                    .map(|p| p.batch.vertices().len())
                    .sum(),
                indices: recording
                    .passes
                    .iter()
                    .map(|p| p.batch.indices().len())
                    .sum(),
                sources: recording.passes.iter().map(|p| p.sources.len()).sum(),
                ramps: recording.ramps.len(),
            }
        })
        .collect()
}

/// The table as it is written to disk: one line a scene, columns aligned so a
/// diff shows which number moved rather than that a line changed.
fn render(costs: &[Cost]) -> String {
    let width = costs.iter().map(|c| c.name.len()).max().unwrap_or(0).max(5);
    let mut out = String::new();
    out.push_str(
        "# What each corpus scene costs to record. Device-independent by\n\
         # construction: nothing here is measured, it is counted.\n\
         #\n\
         # Regenerate with UPDATE_COST_BASELINE=1 and read the diff.\n\
         #\n",
    );
    let _ = writeln!(
        out,
        "# {:<w$}  {:>6} {:>6} {:>8} {:>8} {:>7} {:>5}",
        "scene",
        "passes",
        "draws",
        "vertices",
        "indices",
        "sources",
        "ramps",
        w = width
    );
    for c in costs {
        let _ = writeln!(
            out,
            "  {:<w$}  {:>6} {:>6} {:>8} {:>8} {:>7} {:>5}",
            c.name,
            c.passes,
            c.draws,
            c.vertices,
            c.indices,
            c.sources,
            c.ramps,
            w = width
        );
    }
    out
}

/// Where the baseline is, which a board run has to be told.
///
/// `CARGO_MANIFEST_DIR` is baked in when the binary is compiled and names a
/// path on the machine that compiled it. A cross-built test binary shipped to
/// a board finds nothing there, which is the trap `docs/on-a-board.md` records
/// for the shader snapshots -- and this would have been the third test that
/// cannot run from a bare binary. `EMBLEMA_COST_BASELINE` names the file
/// instead, the way `EMBLEMA_SHADER_SNAPSHOTS` names its directory.
fn baseline_path() -> PathBuf {
    if let Some(path) = std::env::var_os("EMBLEMA_COST_BASELINE") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cost-baseline.txt")
}

#[test]
fn every_corpus_scene_costs_what_it_was_recorded_as() {
    let table = render(&costs());
    let path = baseline_path();

    if std::env::var_os("UPDATE_COST_BASELINE").is_some() {
        std::fs::write(&path, &table).expect("write the cost baseline");
        return;
    }

    let Ok(expected) = std::fs::read_to_string(&path) else {
        panic!(
            "{} does not exist. Run with UPDATE_COST_BASELINE=1 to write it, \
             and review what it contains.",
            path.display()
        );
    };
    if expected == table {
        return;
    }

    // Report the rows that moved rather than the whole file, since the whole
    // file is sixty lines and the interesting part is usually one of them.
    let mut moved: Vec<String> = Vec::new();
    let was: Vec<&str> = expected.lines().filter(|l| !l.starts_with('#')).collect();
    let now: Vec<&str> = table.lines().filter(|l| !l.starts_with('#')).collect();
    for pair in was.iter().zip(&now) {
        if pair.0 != pair.1 {
            moved.push(format!("  was {}\n  now {}", pair.0.trim(), pair.1.trim()));
        }
    }
    let count = if was.len() == now.len() {
        format!("{} row(s) differ", moved.len())
    } else {
        format!(
            "the corpus holds {} scenes where the baseline has {}",
            now.len(),
            was.len()
        )
    };
    panic!(
        "a recorded frame costs something different than it did: {count}.\n{}\n\n\
         Nothing here is timed, so none of it is noise -- every one of these \
         moved because a recording changed shape. If that was the point, \
         rerun with UPDATE_COST_BASELINE=1 and let the diff make the claim.",
        moved.join("\n")
    );
}

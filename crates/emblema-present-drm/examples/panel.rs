//! Draw a moving scene straight to a panel, with no compositor.
//!
//! The tests beside this one prove the path works by pushing solid quads at it
//! and reading counters. That is the right shape for a test and shows nothing:
//! the point of direct scanout is that a picture reaches a display, and nobody
//! has been able to look at one until there was a board to look at.
//!
//! ```text
//! cargo run -p emblema-present-drm --example panel
//! EMBLEMA_DRM_CARD=/dev/dri/card1 SECONDS=30 cargo run ... --example panel
//! ```
//!
//! It takes DRM master, so nothing else may be driving the display -- on a
//! Raspberry Pi that means stopping the compositor, or never starting one.
//! `cargo xtask drm` says whether this machine can.
//!
//! `EMBLEMA_DRM_CARD` matters on a board with more than one display
//! controller. A Raspberry Pi 5 has two, and only one of them can currently
//! scan out what this renderer exports; without naming a card you get whichever
//! `/dev/dri` lists first. The crate documentation has the table.
//!
//! # Reading the report
//!
//! It ends with what it paced: frames presented, the vertical blanks that passed
//! while it did, and how many of those blanks the display latched nothing new on.
//! The last is the number worth having, and it is counted from the sequence the
//! kernel reports with each flip rather than from a clock. `crate::pacing` says
//! why.
//!
//! Three things that number is not. It is not a property of the renderer alone --
//! a deeper ring hides a slow frame instead of missing a blank, so the ring depth
//! is printed beside it. It is not comparable with `cargo xtask bench`'s
//! `full frame, mixed content` row, which is a different scene at a different size
//! and counts neither the recording nor the present. And it means little from a
//! debug build: **build this release** and run it from a filesystem the numbers
//! were taken on, which for this project's board means
//!
//! ```text
//! cargo build -p emblema-present-drm --release --example panel
//! scp target/.../examples/panel board:/tmp/
//! ssh board 'cd /tmp && EMBLEMA_DRM_CARD=/dev/dri/card0 ./panel'
//! ```
//!
//! `docs/on-a-board.md` has the cross-compilation recipe and the preconditions a
//! number taken here has to state.
//!
//! `DEPTH=n` sets the ring depth, and two is the interesting value. A three-deep
//! ring can absorb a frame that overran its period by holding a finished buffer
//! back, so a run that misses nothing at three has kept up without saying whether it
//! had room to spare. A two-deep ring has nowhere to put that frame, so the same
//! result at two is the stronger claim.
//!
//! Two knobs exist to make the miss count say something rather than read zero.
//! `CARDS=n` scales the scene, since a counter that has only ever read zero is not
//! known to work. `STALL=n` makes every nth frame late by two frame periods, and
//! should cost one missed blank each time -- so `STALL=10` over a hundred and
//! sixty-five frames gives sixteen, which is what it gave when this was calibrated.
//!
//! Two periods rather than one, and the reason is worth knowing before reading any
//! number here. This loop already waits for the flip it committed before committing
//! again, so on a scene the renderer finishes early there is a whole period of slack
//! in every frame. Sleeping one period spends that slack and misses nothing --
//! measured, at `STALL=2`, which held sixty frames a second and zero misses. Only
//! the second period overruns the blank. A frame being late is not the same as a
//! frame being late *enough*, and a miss count is the difference.

use emblema_core::{Canvas, Color, GradientStop, Layer, Paint, Rect, Shader, TileMode, Vec2};
use emblema_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use emblema_present::PresentTarget;
use emblema_present_drm::{DrmScanoutTarget, KmsOutput, ScanoutOutput};

fn card() -> Option<KmsOutput> {
    if let Ok(path) = std::env::var("EMBLEMA_DRM_CARD") {
        return match KmsOutput::open(&path) {
            Ok(output) => Some(output),
            Err(e) => {
                eprintln!("{path}: {e}");
                None
            }
        };
    }
    let mut refused = Vec::new();
    for entry in std::fs::read_dir("/dev/dri").ok()?.flatten() {
        let name = entry.file_name().into_string().ok()?;
        if !name.starts_with("card") {
            continue;
        }
        let path = entry.path().to_string_lossy().into_owned();
        match KmsOutput::open(&path) {
            Ok(output) => {
                eprintln!("driving {path}");
                return Some(output);
            }
            Err(e) => refused.push(format!("{path}: {e}")),
        }
    }
    eprintln!("no card this process can drive ({})", refused.join("; "));
    None
}

/// One frame of the scene, at `t` seconds.
///
/// Nothing here is chosen to be easy on the renderer. The ground is a gradient
/// so banding would show, the cards carry shadows so the blur runs every frame,
/// and the whole thing moves so a frame held by mistake is obvious.
fn frame(width: f32, height: f32, t: f32, cards: u32) -> emblema_core::Recording {
    let mut canvas = Canvas::new(emblema_core::Extent2D::new(width as u32, height as u32));
    canvas.clear(Color::srgb(0.05, 0.06, 0.09, 1.0));

    // A slow wash behind everything, wide enough that a step in it would be
    // visible from across a room.
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, width, height),
            // Not antialiased, and it costs nothing: this covers the whole
            // frame, so it has no edge to soften. It matters because a gradient
            // fill is tessellated rather than evaluated per fragment, so with
            // antialiasing on it would make the frame's pass multisampled --
            // and `execute_deferred` cannot submit one of those, which is how
            // the fence reaches the kernel.
            &Paint::default()
                .with_anti_alias(false)
                .with_shader(Shader::LinearGradient {
                    start: Vec2::new(0.0, 0.0),
                    end: Vec2::new(width, height),
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: Color::srgb(0.08, 0.10, 0.18, 1.0),
                        },
                        GradientStop {
                            offset: 1.0,
                            color: Color::srgb(0.22, 0.10, 0.20, 1.0),
                        },
                    ],
                    tile: TileMode::Clamp,
                }),
        )
        .expect("ground");

    let center = Vec2::new(width * 0.5, height * 0.5);
    let orbit = width.min(height) * 0.26;
    let side = width.min(height) * 0.20;

    for i in 0..cards {
        let phase = t * 0.6 + i as f32 * std::f32::consts::TAU / cards as f32;
        let at = Vec2::new(
            center.x + orbit * phase.cos(),
            center.y + orbit * phase.sin() * 0.55,
        );
        let card = Rect::new(
            at.x - side * 0.5,
            at.y - side * 0.5,
            at.x + side * 0.5,
            at.y + side * 0.5,
        );

        // The shadow first, then the card over it, which is the arrangement the
        // occluder flag describes and the one every real caller uses.
        canvas
            .draw_shadow(&card.to_rounded_path(side * 0.18), Color::BLACK, 8.0, false)
            .expect("shadow");

        let hue = [
            Color::srgb(0.98, 0.42, 0.28, 1.0),
            Color::srgb(0.36, 0.82, 0.62, 1.0),
            Color::srgb(0.42, 0.58, 0.98, 1.0),
            // Wrapped rather than extended: scaling the scene is about how much work a
            // frame is, and three hues at four cards is the same work as four would be.
        ][i as usize % 3];
        canvas
            .draw_rrect(card, side * 0.18, &Paint::fill(hue))
            .expect("card");
    }

    // A blurred highlight sweeping across, so the layer and blur paths run on
    // every frame rather than only in the tests.
    let sweep = ((t * 0.35).sin() * 0.5 + 0.5) * width;
    canvas.save_layer(Layer::opacity(0.5).with_blur(24.0));
    canvas
        .draw_circle(
            Vec2::new(sweep, height * 0.5),
            width.min(height) * 0.10,
            &Paint::fill(Color::srgb(1.0, 0.95, 0.80, 1.0)),
        )
        .expect("highlight");
    canvas.restore();

    canvas.finish()
}

fn main() {
    let Some(output) = card() else {
        std::process::exit(1);
    };
    let mode = output.mode();
    let (width, height) = (mode.extent.width, mode.extent.height);
    println!(
        "{}x{} at {:.2} Hz",
        width,
        height,
        mode.refresh_mhz as f32 / 1000.0
    );

    let mut ctx = match VulkanContext::new(DevicePreference::Auto) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("no Vulkan device: {e}");
            std::process::exit(1);
        }
    };

    // Three is what a frame loop wants: one on screen, one being flipped to, one
    // being drawn. `DEPTH=2` is the measurement that separates keeping up from
    // having headroom -- a two-deep ring cannot absorb a frame that overran, so a
    // run that misses nothing at two had time to spare rather than a ring covering
    // for it. Two is the floor the target enforces anyway.
    let depth: usize = std::env::var("DEPTH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(emblema_present_drm::DEFAULT_RING_DEPTH)
        .max(2);
    let mut target = match DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, depth) {
        Ok(target) => target,
        Err(e) => {
            eprintln!("this display controller cannot scan out what the renderer exports: {e}");
            eprintln!(
                "see the crate documentation: not every controller can, and the reasons differ"
            );
            std::process::exit(1);
        }
    };

    let seconds: f32 = std::env::var("SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20.0);
    // Three is what the scene was drawn for. More is how a miss count is made to
    // read something other than zero, which is the only way to know it works.
    let cards: u32 = std::env::var("CARDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .max(1);
    // Every nth frame sleeps a frame period, which should cost exactly one blank.
    // A counter that does not rise by about one per stall is not counting blanks.
    let stall: u64 = std::env::var("STALL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let period = target.output().exact_frame_nanos();
    let start = std::time::Instant::now();
    let mut frames = 0u64;

    while start.elapsed().as_secs_f32() < seconds {
        let t = start.elapsed().as_secs_f32();
        let recording = frame(width as f32, height as f32, t, cards);

        // Acquired for the side effect; the target draws into the slot it just
        // took, so the image itself is not wanted here.
        if let Err(e) = target.acquire(&mut ctx).map(|_| ()) {
            eprintln!("acquire: {e}");
            break;
        }
        // `submit_recording` rather than `execute_deferred` and a hand-rolled
        // pair, and the difference is not tidiness. Both halves of that call's
        // return travel together until the fence retires -- the layer targets are
        // what the submission is still sampling -- and this scene has a blurred
        // `save_layer`, so it produces one every frame. Destroying them straight
        // after the call, which is what this loop did, frees them while the GPU
        // may still be reading them. The target keeps them in the slot and drops
        // them when a flip says the slot is free, which is the whole reason it
        // holds them.
        if let Err(e) = target.submit_recording(&mut ctx, &recording, &[]) {
            eprintln!("draw: {e}");
            break;
        }
        if let Err(e) = target.present(&mut ctx) {
            eprintln!("present: {e}");
            break;
        }
        frames += 1;

        if stall > 0 && frames % stall == 0 {
            // Two periods: the first is absorbed by the wait `present` already does
            // before it commits, and only the second overruns the blank. One period
            // held sixty frames a second with nothing missed, which is what says the
            // slack is there rather than that the counter is asleep.
            //
            // Sleeping rather than spinning, because what is wanted is a late frame
            // and not a processor competing with the one drawing it.
            if let Some(nanos) = period {
                std::thread::sleep(std::time::Duration::from_nanos(nanos * 2));
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f32();
    println!(
        "{frames} frames in {elapsed:.1}s, {:.1} per second",
        frames as f32 / elapsed
    );

    // What the frame rate above cannot say. A rate is the same whether every blank
    // was latched or every second one was; this is the difference.
    let pacing = target.output().pacing();
    print!(
        "{} flips over {} blanks",
        pacing.flips(),
        pacing.elapsed_vblanks()
    );
    match pacing.missed() {
        Some(missed) => print!(", {missed} missed"),
        None => print!(", missed unknown: this driver's flip sequence did not advance usably"),
    }
    println!(
        ", {} cpu wait(s), ring depth {}, {} card(s)",
        target.cpu_waits(),
        target.ring_depth(),
        cards
    );

    // The cross-check, from the kernel's own timestamps. If the blanks counted do
    // not account for the time the flips arrived over, the sequence numbers are not
    // what this takes them for and the miss count above means nothing.
    if let (Some(nanos), true) = (period, pacing.usable()) {
        let expected = nanos * pacing.elapsed_vblanks();
        let span = pacing.span().as_nanos() as u64;
        println!(
            "  blanks account for {:.3}s of the {:.3}s the flips arrived over, at {:.3} ms a blank",
            expected as f64 / 1e9,
            span as f64 / 1e9,
            nanos as f64 / 1e6,
        );
    }

    target.destroy(&mut ctx);
}

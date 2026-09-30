//! The measurement `docs/architecture.md` asks for and could not make.
//!
//! One number in that document decides a design: at 1920x1080 with a hundred
//! and sixty rounded rectangles, the analytic distance field at one sample was
//! measured at 0.26 ms against 0.64 ms for the same shapes tessellated at four
//! and 0.20 ms tessellated at one -- from which "the whole of the gain is in
//! not multisampling", and from which the field is *slower* than the triangles
//! it replaces at equal sample count. The document then says the balance is
//! hardware-dependent in the direction this project cares about, and that it
//! "should be measured on a board before the field is assumed to be the faster
//! path everywhere".
//!
//! Nothing in the tree could make that measurement. This can.
//!
//! # What it is not
//!
//! Not a regression gate *by default*, and the distinction is the whole of the
//! design. A plain run prints numbers and returns success whatever they say,
//! because a threshold nobody has calibrated on a machine nobody has
//! characterized is a build failure waiting to happen on a busy laptop.
//!
//! `--record <path>` writes what a run measured, keyed by device and
//! configuration; `--check <path>` measures again and compares, reporting every
//! row and exiting non-zero if any is slower than `--tolerance` (one percent by
//! default) or if either side has a row the other lacks. Both are opt-in, so
//! the objection above still holds for every run that did not ask for one.
//!
//! The default tolerance is measured rather than chosen, and so is the reason
//! it is not one number for every row. On a Raspberry Pi 5's V3D, seven of the
//! eight rows repeat to within three tenths of a percent across thirty-one
//! runs. The eighth -- the Vulkan distance-field figure -- lands in one of two
//! states three percent apart from one process to the next, because recording
//! a command costs differently in each; `docs/on-a-board.md` has the numbers.
//!
//! A single tolerance has to be as loose as the worst row it covers, so that
//! one row set the bar for all eight and a three percent regression on any of
//! the other seven passed unremarked. A baseline row may therefore carry its
//! own tolerance as an optional fourth field, which puts the slack on the row
//! that needs it, next to the measurement that justifies it, and lets the
//! default be tight enough to mean something.
//!
//! This does not make a busy machine a quiet one. Checking an unchanged build
//! against its own baseline on the workstation this was written on reported
//! three of eight rows regressed, one of them by twelve percent. The test-lane
//! table still records gating as belonging to nightly runs on quiet runners;
//! what has changed is that the machinery now exists for one.
//!
//! Not a comparison against the numbers above either. Those were taken on a
//! desktop discrete part; a run here measures whatever this machine is, which
//! is why every result names its device.
//!
//! The device rows are not a whole frame's work: each recording is built once,
//! before the clock starts, and only `execute` is timed. An application records
//! a frame per frame, so what it costs to *build* one used to be invisible to
//! every number this printed -- named as a blind spot, with its size guessed at
//! from two hand measurements and left outside.
//!
//! It is measured now, as the `recording` section, and the guess was the wrong
//! shape rather than the wrong size. Two of the six builds are indeed under a
//! per cent of their draw: a frame of mixed content builds in 0.006 ms against
//! 14 to draw. But a *stroked* path builds in 0.240 ms and draws in 1.2, which
//! is a fifth of the cost on the side nothing was watching -- and it is the
//! cheapest row in the file to draw, so timing only the drawing made the
//! stroker look free. The two sections add, since a frame pays for both.
//!
//! That section needs no device, which is why it comes first and why it is
//! measured even where every backend was skipped. A `Canvas` opens nothing.
//!
//! It is also the one part of this file with a check that runs everywhere.
//! Absolute milliseconds need a board and a baseline, so they catch a regression
//! only when someone benches; a *ratio* of one build to another cancels the
//! processor and holds on any machine --  measured at 3.20 on a Ryzen 9 at 5.3 GHz
//! and 3.07 on an A76 at 2.4, four per cent apart across a threefold difference in
//! speed. `building_a_stroke_costs_what_it_should_relative_to_a_fill` gates those
//! in the ordinary suite, which is where a stroker that doubled its output would
//! now be caught without a board at all.

use emblema_core::Recording;
use emblema_hal::{Hal, HalContext, PixelFormat, TextureDescriptor};
use emblema_hal_gles::{DisplayTarget, GlesContext, GlesHal};
use emblema_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
mod frames;

use frames::{
    frame, stacked, Frame, Stacked, EXTENT, FRAMES, FRAME_STAGES, SHAPES, STACKED_STAGES, WARMUP,
};
pub use frames::{recording, Path};

use std::io::{self, Write};
use std::time::{Duration, Instant};

/// One configuration's result, as the event that reports it either way.
fn outcome(what: &'static str, timed: Result<Timing, String>) -> Event {
    match timed {
        Ok(timing) => Event::Measured(timing),
        Err(why) => Event::Failed { what, why },
    }
}

/// What a run of one configuration on one device came to.
pub struct Timing {
    /// What was timed, already spelled for the reader.
    ///
    /// A label rather than a [`Path`] for the reason the device is not here
    /// either: not everything this times is one of the two routes. A whole
    /// frame of mixed content is a budget rather than a comparison, and a
    /// field that could only say which route it was would have to lie about
    /// it.
    pub what: &'static str,
    /// Whether this row timed *building* a frame rather than drawing one.
    ///
    /// Only the presentation needs it, and it needs it for a reason worth more
    /// than the field: a build row's reciprocal is not a frame rate. A frame
    /// pays for its recording *and* its execution, one after the other, so
    /// neither half's rate is achievable on its own and printing "148965 fps"
    /// beside a seven-microsecond build invites exactly the wrong arithmetic.
    /// The column is left blank for these instead.
    pub builds: bool,
    /// How many draws the frame came to.
    ///
    /// Reported because it is not the same between the two paths and the
    /// difference is not incidental. Tessellated shapes all carry one solid
    /// material, so the batch merges them into a single draw; an analytic one
    /// carries its own geometry in its material and cannot merge with its
    /// neighbor. The same hundred and sixty shapes are one draw down one path
    /// and a hundred and sixty down the other, and a timing that did not say so
    /// would look like a comparison of fragment work.
    pub draws: usize,
    /// The middle frame, which is what to read: a mean folds in whatever else
    /// the machine was doing during the slowest one.
    pub median: Duration,
    /// The frame ninety-nine hundredths of them came in under.
    ///
    /// Reported beside the median because that pair is what a frame budget is
    /// written against: a rate *and* a bound on the tail, since a renderer that
    /// hits an average while missing one frame in fifty is not the same thing as
    /// one that does not. Twice the median is the figure this project has used for
    /// that bound.
    /// The slowest frame is a different statement: one interruption owns it,
    /// and on a machine doing anything else there is always one.
    pub p99: Duration,
    pub fastest: Duration,
    pub slowest: Duration,
}

impl Timing {
    /// Whether the spread is wide enough that the median should not be trusted
    /// on its own.
    ///
    /// Reported rather than acted on. A shared machine gives noisy answers and
    /// the honest response is to say so next to the number, not to retry until
    /// it looks quiet.
    pub fn noisy(&self) -> bool {
        self.slowest > self.fastest * 2
    }

    /// Frames a second, if every frame took the median.
    ///
    /// A rate is what the targets are written in and a duration is what was
    /// measured, so the conversion belongs here rather than in a reader's head.
    pub fn rate(&self) -> f64 {
        let seconds = self.median.as_secs_f64();
        if seconds > 0.0 {
            1.0 / seconds
        } else {
            f64::INFINITY
        }
    }
}

/// The sample `fraction` of the way through a sorted run, by nearest rank.
///
/// Nearest rank rather than an interpolation: every value here is a frame that
/// happened, and a p99 that lands between two of them is a number no frame
/// took. Panics on an empty slice, which the caller cannot produce -- the run
/// that fills it returns `None` before this is reached.
fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Time one configuration, forcing the device to finish each frame.
///
/// `finish` is what makes the number a frame rather than a submission. The
/// Vulkan path already waits on a fence inside the submit, so its barrier is
/// nothing; the GLES path returns as soon as the commands are queued, and
/// timing that measured the driver's willingness to accept work -- a hundred
/// and sixty rounded rectangles at this size came to nineteen microseconds,
/// with an occasional frame a hundred times that when the queue backed up.
/// How long it takes to *build* one frame's recording, which nothing timed.
///
/// Every other row here starts its clock after the recording exists. That is the
/// right shape for measuring a backend -- what a driver does with a fixed list of
/// draws is the question -- and it leaves a whole half of the renderer unmeasured,
/// because an application records a frame per frame. Tessellation, stroking, the
/// thin-stroke rule, clip bookkeeping and batching all happen on this side of the
/// clock.
///
/// No device, and that is the point rather than a convenience: a `Canvas` needs
/// none, so these rows are measured even where every backend was skipped, and a
/// regression in them is a regression on any machine rather than on this GPU.
/// They are keyed as their own section for the same reason.
///
/// The builder is a function pointer rather than a closure over a prepared
/// recording, because what is being timed is the building. Handing this a
/// `&Recording` would time nothing at all, which is a mistake easy enough to make
/// that it is worth the type saying so.
fn time_building(what: &'static str, build: fn() -> Recording) -> Timing {
    let run = |count: usize| -> (Vec<Duration>, usize) {
        let mut samples = Vec::with_capacity(count);
        let mut draws = 0;
        for _ in 0..count {
            let started = Instant::now();
            let recording = build();
            samples.push(started.elapsed());
            // Read after the clock stops, and read at all so the optimizer
            // cannot decide the recording was never wanted.
            draws = recording.draw_count();
        }
        (samples, draws)
    };

    // The first builds pay for the allocator's first pages, which an application
    // recording its thousandth frame does not.
    let _ = run(WARMUP);
    let (mut samples, draws) = run(FRAMES);
    samples.sort();
    Timing {
        what,
        builds: true,
        draws,
        median: percentile(&samples, 0.5),
        p99: percentile(&samples, 0.99),
        fastest: samples[0],
        slowest: samples[samples.len() - 1],
    }
}

fn time_frames<H: Hal>(
    ctx: &mut H::Context,
    what: &'static str,
    recording: &Recording,
    finish: fn(&mut H::Context),
) -> Result<Timing, String>
where
    H::Context: HalContext<Hal = H>,
{
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(
            EXTENT,
            PixelFormat::Rgba8Unorm,
        ))
        .map_err(|e| format!("no target: {e}"))?;

    let mut run = |count: usize| -> Result<Vec<Duration>, String> {
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            let started = Instant::now();
            let outcome = emblema_core::execute::<H>(ctx, &mut target, recording, &[]);
            finish(ctx);
            samples.push(started.elapsed());
            outcome.map_err(|e| e.to_string())?;
        }
        Ok(samples)
    };

    let outcome = run(WARMUP).and_then(|_| run(FRAMES));
    ctx.destroy_texture(target);

    let mut samples = outcome?;
    samples.sort();
    Ok(Timing {
        what,
        builds: false,
        draws: recording.draw_count(),
        median: percentile(&samples, 0.5),
        p99: percentile(&samples, 0.99),
        fastest: samples[0],
        slowest: samples[samples.len() - 1],
    })
}

/// Run every configuration on every device this machine offers, handing each
/// result to `report` as soon as it is measured.
///
/// The observer rather than a returned vector because a full run takes minutes
/// on a slow device, and one that is interrupted partway should be a partial
/// result rather than no result at all. Collecting everything and formatting at
/// the end lost three runs on a board that left the network mid-run, each of
/// which had measured several configurations and printed none of them.
/// Whether `device` is one the caller asked to leave alone.
///
/// A substring rather than an exact name because a device names itself at
/// length and differently on every driver -- `vulkan:1 llvmpipe (LLVM 22.1.8,
/// 256 bits)` here and a different parenthesis on the next machine -- so a
/// caller who has to reproduce one exactly will get it wrong and be told
/// nothing. Case-insensitive for the same reason.
fn skipped(device: &str, skip: &[String]) -> bool {
    let device = device.to_lowercase();
    skip.iter().any(|s| device.contains(&s.to_lowercase()))
}

/// `skip` names devices to open and then leave alone, matched as
/// case-insensitive substrings of the device name. It exists because a device
/// can be measurable and still not safe to measure: a software rasterizer at
/// this size holds every core of a small board flat out for minutes, and a
/// Raspberry Pi 5 locked up at that stage of the run four times in a row,
/// having come through the hardware stages before it each time. Skipping it
/// leaves the board's own GPU measurable, which is the number worth having.
pub fn gather(skip: &[String], report: &mut dyn FnMut(Event)) {
    let paths = [
        Path::Analytic,
        Path::TessellatedMultisampled,
        Path::TessellatedSingleSampled,
        Path::StrokedAnalytic,
        Path::StrokedTessellated,
    ];

    // Building comes first, in the order a frame happens and in the order that
    // needs nothing: this section is measured even when every backend is skipped
    // or absent.
    const BUILDING: &str = "recording";
    if skipped(BUILDING, skip) {
        report(Event::Skipped(BUILDING.to_string()));
    } else {
        report(Event::Device(BUILDING.to_string()));
        for path in paths {
            report(Event::Measured(time_building(
                path.name(),
                match path {
                    Path::Analytic => || recording(Path::Analytic),
                    Path::TessellatedMultisampled => || recording(Path::TessellatedMultisampled),
                    Path::TessellatedSingleSampled => || recording(Path::TessellatedSingleSampled),
                    Path::StrokedAnalytic => || recording(Path::StrokedAnalytic),
                    Path::StrokedTessellated => || recording(Path::StrokedTessellated),
                },
            )));
        }
        for stage in FRAME_STAGES {
            // Spelled out per stage for the reason the paths above are: the
            // builder is a function pointer so that a prepared recording cannot
            // be handed to it, and a closure capturing `stage` is not one.
            report(Event::Measured(time_building(
                stage.name(),
                match stage {
                    Frame::Ground => || frame(Frame::Ground),
                    Frame::Cards => || frame(Frame::Cards),
                    Frame::Shadows => || frame(Frame::Shadows),
                    Frame::All => || frame(Frame::All),
                },
            )));
        }
        for stage in STACKED_STAGES {
            report(Event::Measured(time_building(
                stage.name(),
                match stage {
                    Stacked::Wash => || stacked(Stacked::Wash),
                    Stacked::Panels => || stacked(Stacked::Panels),
                    Stacked::All => || stacked(Stacked::All),
                },
            )));
        }
    }

    for index in 0.. {
        let Ok(mut ctx) = VulkanContext::new(DevicePreference::Index(index)) else {
            break;
        };
        let device = format!("vulkan:{index} {}", ctx.capabilities().device_name);
        if skipped(&device, skip) {
            report(Event::Skipped(device));
            continue;
        }
        report(Event::Device(device));
        for path in paths {
            let recording = recording(path);
            // Nothing: the Vulkan submit waits on its own fence before it
            // returns, so the frame is already over when the clock stops.
            report(outcome(
                path.name(),
                time_frames::<VulkanHal>(&mut ctx, path.name(), &recording, |_| {}),
            ));
        }
        for stage in FRAME_STAGES {
            let recording = frame(stage);
            report(outcome(
                stage.name(),
                time_frames::<VulkanHal>(&mut ctx, stage.name(), &recording, |_| {}),
            ));
        }
        for stage in STACKED_STAGES {
            let recording = stacked(stage);
            report(outcome(
                stage.name(),
                time_frames::<VulkanHal>(&mut ctx, stage.name(), &recording, |_| {}),
            ));
        }
    }

    if let Ok(mut ctx) = GlesContext::new(DisplayTarget::Surfaceless) {
        let device = format!("gles {}", ctx.capabilities().device_name);
        if skipped(&device, skip) {
            report(Event::Skipped(device));
            return;
        }
        report(Event::Device(device));
        for path in paths {
            let recording = recording(path);
            let timed = time_frames::<GlesHal>(&mut ctx, path.name(), &recording, |ctx| {
                // SAFETY: a context is current on this thread.
                unsafe { glow::HasContext::finish(ctx.raw_gl()) }
            });
            report(outcome(path.name(), timed));
        }
        for stage in FRAME_STAGES {
            let recording = frame(stage);
            let timed = time_frames::<GlesHal>(&mut ctx, stage.name(), &recording, |ctx| {
                // SAFETY: a context is current on this thread.
                unsafe { glow::HasContext::finish(ctx.raw_gl()) }
            });
            report(outcome(stage.name(), timed));
        }
        for stage in STACKED_STAGES {
            let recording = stacked(stage);
            let timed = time_frames::<GlesHal>(&mut ctx, stage.name(), &recording, |ctx| {
                // SAFETY: a context is current on this thread.
                unsafe { glow::HasContext::finish(ctx.raw_gl()) }
            });
            report(outcome(stage.name(), timed));
        }
    }
}

fn millis(duration: Duration) -> String {
    format!("{:.3} ms", duration.as_secs_f64() * 1000.0)
}

/// Why an unoptimized build is refused rather than measured.
///
/// `cargo xtask bench` runs through an alias that does not pass `--release`,
/// so for as long as this printed numbers it printed them for unoptimized
/// code. That is not a smaller version of the right answer, it is a different
/// one, and the difference falls almost entirely on one of the two paths being
/// compared: the analytic route submits a hundred and sixty draws where the
/// tessellated route submits one, so debug-build per-draw cost lands on the
/// analytic side and nowhere else.
///
/// It went wrong exactly that way. A commit that added a public method nothing
/// in the benchmark calls moved the analytic path from 13.16 ms to 13.82 ms on
/// a Raspberry Pi 5 -- reproducibly, on both Vulkan and GLES, with the
/// tessellated paths unmoved -- and deleting that uncalled method put it back.
/// A function nobody calls cannot cost GPU time; what it can do is shift code
/// layout in a build with no optimizer to absorb it. Built with `--release`
/// the same commit measures level with the tessellated path, which is what
/// this document's board figures say and what the design rests on.
///
/// `debug_assertions` is the test because the workspace builds under one
/// profile: if this binary is unoptimized then so is the renderer it times.
pub fn why_not_debug() -> &'static str {
    "bench: this is an unoptimized build, and timing one is worse than not \n\
     timing at all -- the cost falls on whichever path submits more draws, \n\
     which is the comparison this is for.\n\
     \n\
     Run it optimized:\n\
     \n\
     \x20   cargo run --release --package xtask -- bench\n\
     \n\
     For a board, cross-build with --release and copy that binary across.\n"
}

/// What a run that measured nothing says.
const NOTHING: &str = "no device on this machine could render the frame\n";

fn header() -> String {
    format!(
        "{SHAPES} rounded rectangles at {}x{}, then one frame of mixed \
         content at the same size in {} stages, then an interface that \
         overdraws in {}.\n{FRAMES} frames each after {WARMUP} warm-up{}\n",
        EXTENT.width,
        EXTENT.height,
        FRAME_STAGES.len(),
        STACKED_STAGES.len(),
        governor()
            .map(|g| format!(", cpu governor {g}"))
            .unwrap_or_default()
    )
}

/// Which frequency governor the processor is under, where that is knowable.
///
/// Printed because the numbers depend on it and a reader cannot see it. On a
/// Raspberry Pi 5 under `ondemand` the recording rows land in two or three states
/// up to forty per cent apart, and pinning the governor to `performance` brings
/// every one of them inside one per cent -- so a baseline recorded under one
/// governor is not comparable with a run under the other, and nothing in the
/// output said which had happened. Two GPU rows move with it as well, the ones
/// with enough processor work in them to notice: the cheap stroked path and the
/// twelve-draw frame.
///
/// `None` where the question does not apply or cannot be answered -- no cpufreq,
/// a different operating system, a container without the sysfs path -- which is
/// reported as nothing rather than as a guess. It reads policy zero because every
/// core on the boards here shares one policy; a machine with more would want the
/// set, and would notice this is not that.
fn governor() -> Option<String> {
    let text =
        std::fs::read_to_string("/sys/devices/system/cpu/cpufreq/policy0/scaling_governor").ok()?;
    let named = text.trim();
    match named.is_empty() {
        true => None,
        false => Some(named.to_string()),
    }
}

/// What a run emits as it goes.
///
/// A device is announced when it is *opened* rather than when its first result
/// arrives, and that distinction is the reason this is an enum rather than a
/// stream of timings. A run on a Raspberry Pi 5 measured all three of V3D's
/// configurations and then took the board off the network before printing
/// anything about the next device -- and from the output alone there was no
/// way to tell whether it had died opening that device or measuring on it,
/// because both look like silence.
pub enum Event {
    /// A device was opened and its configurations are about to be measured.
    Device(String),
    /// One configuration finished.
    Measured(Timing),
    /// A configuration was attempted and could not be measured.
    ///
    /// Reported rather than dropped. `time_frames` used to answer `None` for
    /// every reason it could fail, and the caller had nowhere to put that, so
    /// a configuration the device refused simply did not appear -- and a row
    /// that is absent reads exactly like a row nobody asked for.
    ///
    /// A row *was* missing while this was written, and the cause was an
    /// unwritten call rather than a refusal. That is the argument rather than
    /// against it: the two are indistinguishable in a report that can only
    /// show what succeeded, and one of them was mistaken for the other for
    /// twenty minutes.
    Failed { what: &'static str, why: String },
    /// A device was opened and then not measured, because the caller asked for
    /// it to be left alone.
    ///
    /// Reported rather than passed over quietly. A run that measured two
    /// devices where the reader expected three, and said nothing about the
    /// third, is a run whose coverage cannot be read off its own output.
    Skipped(String),
}

/// One event's contribution to the report.
///
/// Both the streamed run and the assembled one go through here, so the format
/// cannot change in one and not the other.
fn render(event: &Event) -> String {
    let timing = match event {
        Event::Device(device) => return format!("\n{device}\n"),
        Event::Skipped(device) => return format!("\n{device}\n  not measured, as asked\n"),
        Event::Failed { what, why } => return format!("  {what:<24} not measured: {why}\n"),
        Event::Measured(timing) => timing,
    };
    let mut out = String::new();
    // A dash rather than blank, and not only to keep the column's width: a run
    // of spaces in a source string is what a dropped line continuation looks
    // like, and a test reads the sources for exactly that. This says "not a
    // frame rate" where blank would say "forgot to fill it in".
    let rate = match timing.builds {
        true => format!("{:>6}", "--"),
        false => format!("{:>6.0}", timing.rate()),
    };
    out.push_str(&format!(
        "  {:<24} {:>10} ({} fps)   p99 {:>10}   fastest {:>10}   \
         slowest {:>10}   {:>4} draws{}\n",
        timing.what,
        millis(timing.median),
        rate,
        millis(timing.p99),
        millis(timing.fastest),
        millis(timing.slowest),
        timing.draws,
        if timing.noisy() { "   (noisy)" } else { "" }
    ));
    out
}

fn epilogue() -> &'static str {
    "\nEach device's lines are two comparisons and two budgets. The first three \n\
     are one shape filled two ways and the next two are the same shape \n\
     stroked two ways, so within a group the difference is the route and not \n\
     the content -- and a fill's cost is not an outline's, which is why they \n\
     are not read across. The last four are not part of either. They are one \n\
     frame of mixed content, built up an element at a time -- a tabulated \n\
     gradient, then shadowed cards over it, then a blurred layer on top -- and \n\
     it is there because a renderer can be quick at a hundred and sixty \n\
     identical rectangles and slow at everything an interface is made of.\n\
     \n\
     Those four are a budget and its parts: each row is the one above it plus \n\
     one element, so the *difference* between two of them is what that element \n\
     cost, and the last is the whole frame. Subtract only within one device on \n\
     one run -- across runs the noise is larger than most of the differences. \n\
     A difference is also this scene's cost for that element at this size, not \n\
     the element's cost in general: the ground is one draw over every pixel and \n\
     the cards cover a fraction of it, so the two are not comparable per \n\
     element. And the shadows row moves with the cards above it, since a \n\
     shadow is drawn under each card that exists.\n\
     \n\
     The last three are a second frame and answer what the first cannot. That \n\
     one's ground is a full-screen gradient almost nothing covers, so it has \n\
     no overdraw to speak of and its wash is diagonal. This one is an \n\
     interface's shape: a vertical wash, then a bar, a sidebar and a content \n\
     panel hiding 94 per cent of it, then eight rows hiding most of the panel \n\
     again -- 2.53 frames of paint for one frame of picture. Read the \n\
     differences the same way, and read them against the mixed frame only as \n\
     two budgets rather than as a comparison: they draw different things. \n\
     Solid fills throughout, so what the deltas measure is coverage.\n\
     \n\
     The `recording` section is the other half, and it needs no device: it is \n\
     what building each of those frames costs before anything is submitted, \n\
     which is where tessellation, stroking, the thin-stroke rule and the \n\
     batching live. A frame pays for its recording and then for its \n\
     execution, so the two sections add -- and neither half's reciprocal is a \n\
     frame rate, which is why a building row leaves that column blank.\n\
     \n\
     Read the two halves against each other and one row inverts. Stroking a \n\
     path is the most expensive frame in the file to build and among the \n\
     cheapest to draw: the stroker emits two contours where a fill emits one, \n\
     and then an outline covers a band of fragments where a fill covers an \n\
     interior. A bench that timed only the drawing made that route look free.\n\
     \n\
     Medians, with the frame ninety-nine hundredths came in under beside \n\
     them. Nothing here passes or fails: these are what this machine did, \n\
     and the balance between the two paths is hardware-dependent by design \n\
     -- see the distance-field section of docs/architecture.md.\n\
     \n\
     The rate is what a median frame would sustain with nothing else in it: \n\
     no present and no vertical blank. On the comparison rows the scene is a \n\
     hundred and sixty rectangles rather than an interface, so those rates are \n\
     numbers to read against each other. The stacked rows are shaped like an \n\
     interface and still are not a frame rate: a figure for what a frame costs \n\
     names a scene and counts a present, and this counts neither.\n\
     \n\
     Read each p99 against the median on its own line before reading it as \n\
     a renderer's tail. On a machine with a desktop on it a configuration \n\
     can come back with a median under three milliseconds and a ninety-ninth \n\
     percentile near thirty -- a tail ten times the frame it is the tail of, \n\
     which is this process being descheduled rather than the frame taking \n\
     that long. Which lines do it changes from run to run, and they are the \n\
     ones marked noisy. A p99 worth trusting wants a quiet runner, which is \n\
     where the regression gating this deliberately is not belongs too.\n"
}

/// Render the events a run would have emitted.
///
/// Only the tests reach this, because only they have events without a device
/// to measure them on; a real run takes [`stream`]. Both go through `render`,
/// so what the assertions below hold over is what a run prints rather than a
/// second rendering that happens to agree today.
#[cfg(test)]
pub fn text(events: &[Event]) -> String {
    if !events.iter().any(|e| matches!(e, Event::Measured(_))) {
        return NOTHING.to_string();
    }
    let mut out = header();
    for event in events {
        out.push_str(&render(event));
    }
    out.push_str(epilogue());
    out
}

/// Measure every configuration, writing each line as it is measured.
///
/// The header goes out before the first device is opened, so a run that is
/// killed before anything finishes still says what it was attempting -- and
/// every line is flushed, because a buffer that is never drained is the same
/// as not having printed at all.
/// Measure everything, write the report, and hand back what was measured.
///
/// The rows come back so a caller can record or check them. The report is
/// written either way -- a comparison is something done *with* the numbers,
/// never instead of printing them.
pub fn stream_collecting(
    skip: &[String],
    out: &mut impl Write,
) -> io::Result<Vec<(String, String, f64)>> {
    write!(out, "{}", header())?;
    out.flush()?;

    let mut rows: Vec<(String, String, f64)> = Vec::new();
    let mut device = String::new();
    let mut measured = 0usize;
    let mut failed = None;
    gather(skip, &mut |event| {
        if failed.is_some() {
            return;
        }
        match &event {
            Event::Device(name) => device.clone_from(name),
            Event::Measured(timing) => {
                measured += 1;
                rows.push((
                    device.clone(),
                    timing.what.to_string(),
                    timing.median.as_secs_f64() * 1000.0,
                ));
            }
            _ => {}
        }
        let line = render(&event);
        if let Err(e) = write!(out, "{line}").and_then(|()| out.flush()) {
            failed = Some(e);
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }

    write!(out, "{}", if measured == 0 { NOTHING } else { epilogue() })?;
    out.flush()?;
    Ok(rows)
}

/// A recorded run, to compare a later one against.
///
/// Keyed by device *and* configuration, because a baseline from one device says
/// nothing about another: the whole point of the report naming its device is
/// that these numbers are not portable. A file recorded on a Pi and checked on
/// a workstation compares nothing, and this says so rather than passing.
///
/// Stored as text, one row per line, because a baseline that cannot be read in
/// a diff is a baseline nobody will question when it changes.
pub struct Baseline {
    rows: Vec<Row>,
}

/// One recorded configuration, and how much it is allowed to drift.
///
/// `slack` is a per-row override in fractional terms, and it exists because a
/// single tolerance has to be as loose as the *worst* row it covers. On a
/// Raspberry Pi 5 that is one row out of eight -- the Vulkan distance-field
/// figure, which lands in one of two states three percent apart -- while the
/// other seven repeat to within three tenths of a percent. Covering all eight
/// with one number meant a three percent regression on any of the seven passed
/// unremarked. A row that needs slack now says so itself, and says it next to
/// the measurement that justifies it.
struct Row {
    device: String,
    what: String,
    ms: f64,
    slack: Option<f64>,
}

impl Baseline {
    /// `device<TAB>what<TAB>median_ms` with an optional fourth field, a
    /// per-row tolerance in percent. Lines that are neither are an error
    /// rather than a skip -- a baseline half-read would gate on half the
    /// configurations and say nothing about the rest.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rows = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('\t');
            let (Some(device), Some(what), Some(ms), slack, None) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) else {
                return Err(format!(
                    "line {}: expected three tab-separated fields, or four with a tolerance",
                    number + 1
                ));
            };
            let ms: f64 = ms.trim().parse().map_err(|_| {
                format!(
                    "line {}: {ms:?} is not a number of milliseconds",
                    number + 1
                )
            })?;
            let slack = match slack {
                None => None,
                Some(text) => {
                    let percent: f64 = text.trim().parse().map_err(|_| {
                        format!(
                            "line {}: {text:?} is not a tolerance in percent",
                            number + 1
                        )
                    })?;
                    if !(percent.is_finite() && percent >= 0.0) {
                        return Err(format!(
                            "line {}: a tolerance of {percent} is not a fraction of a measurement",
                            number + 1
                        ));
                    }
                    Some(percent / 100.0)
                }
            };
            rows.push(Row {
                device: device.to_string(),
                what: what.to_string(),
                ms,
                slack,
            });
        }
        Ok(Self { rows })
    }

    pub fn render(&self) -> String {
        let mut out = String::from(
            "# cargo xtask bench --record\n\
             # device\tconfiguration\tmedian in milliseconds\t[tolerance %]\n",
        );
        for row in &self.rows {
            out.push_str(&format!("{}\t{}\t{:.3}", row.device, row.what, row.ms));
            if let Some(slack) = row.slack {
                out.push_str(&format!("\t{:.1}", slack * 100.0));
            }
            out.push('\n');
        }
        out
    }

    pub fn from_run(rows: &[(String, String, f64)]) -> Self {
        Self {
            rows: rows
                .iter()
                .map(|(device, what, ms)| Row {
                    device: device.clone(),
                    what: what.clone(),
                    ms: *ms,
                    slack: None,
                })
                .collect(),
        }
    }

    /// Carry each row's tolerance over from an earlier baseline.
    ///
    /// `--record` overwrites a file that a person edited: the per-row slack is
    /// a judgment about how repeatable a configuration is, not a measurement,
    /// and re-recording must not silently drop it. Dropping it would tighten
    /// the gate on exactly the row known to be unrepeatable, so the next run
    /// would fail for the reason the annotation existed to excuse.
    pub fn keeping_slack_from(mut self, previous: &Baseline) -> Self {
        for row in &mut self.rows {
            row.slack = previous
                .rows
                .iter()
                .find(|p| p.device == row.device && p.what == row.what)
                .and_then(|p| p.slack);
        }
        self
    }

    fn find(&self, device: &str, what: &str) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.device == device && r.what == what)
    }
}

/// What a comparison found, per row and in total.
pub struct Comparison {
    pub lines: Vec<String>,
    pub regressed: usize,
    /// Rows on one side and not the other. Counted rather than ignored: a
    /// baseline recorded on a machine with two devices and checked on one with
    /// one has not been satisfied, it has been half-read.
    pub unmatched: usize,
}

/// Compare a run against a baseline, tolerating `tolerance` as a fraction.
///
/// The tolerance is a fraction rather than a fixed number of milliseconds
/// because these span one millisecond to thirty depending on the device, and a
/// budget that means something on one would be noise or a hair trigger on the
/// other.
pub fn compare(baseline: &Baseline, run: &[(String, String, f64)], tolerance: f64) -> Comparison {
    let mut lines = Vec::new();
    let mut regressed = 0;
    let mut unmatched = 0;

    for (device, what, now) in run {
        match baseline.find(device, what) {
            Some(row) => {
                let then = row.ms;
                let allowed = row.slack.unwrap_or(tolerance);
                let delta = (now - then) / then;
                let over = delta > allowed;
                if over {
                    regressed += 1;
                }
                lines.push(format!(
                    "  {:<26} {:>9.3} was {:>9.3}  {:+6.1}%{}{}",
                    what,
                    now,
                    then,
                    delta * 100.0,
                    // Only where it differs from the default, so a reader sees
                    // at a glance which rows are being held to a looser bar.
                    match row.slack {
                        Some(s) => format!("  (tolerating {:.1}%)", s * 100.0),
                        None => String::new(),
                    },
                    if over { "   REGRESSED" } else { "" }
                ));
            }
            None => {
                unmatched += 1;
                lines.push(format!(
                    "  {what:<26} {now:>9.3}  no baseline for this configuration on {device}"
                ));
            }
        }
    }
    for row in &baseline.rows {
        let (device, what, then) = (&row.device, &row.what, row.ms);
        if !run.iter().any(|(d, w, _)| d == device && w == what) {
            unmatched += 1;
            lines.push(format!(
                "  {what:<26} {:>9}  was {then:>9.3} in the baseline for {device}, \
                 not measured now",
                "-"
            ));
        }
    }
    Comparison {
        lines,
        regressed,
        unmatched,
    }
}

/// The commit the recorded timings were last checked against, if a baseline
/// says so.
///
/// One file rather than all of them: a second board would need its own line and
/// its own count, and there is one board.
fn verified_at() -> Option<String> {
    let text = std::fs::read_to_string("tests/bench-baselines/raspberry-pi-5-v3d.txt").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("# Last checked against the board: "))
        .map(|sha| sha.trim().to_string())
        .filter(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()))
}

/// How far the recorded timings have drifted from the code, in commits.
///
/// The gate cannot check timing: it takes a quiet machine, and the one this
/// runs on spreads its own medians by up to half. So the timing baseline is
/// checked by hand on a board, and the failure that follows from that is not a
/// wrong number but a forgotten one -- sixteen renderer commits once went by
/// between two checks, and the ten and a half per cent they had cost was
/// invisible from every diff and every green gate.
///
/// This is the cheapest thing that would have surfaced it: not a threshold, not
/// a failure, just the count, printed where the skip census is printed and read
/// the same way. What it counts is commits touching the crates the bench times --
/// and the bench itself, since what it measures is as able to change as what it
/// measures it on -- since the board last agreed.
///
/// `None` where the question cannot be asked -- no git, no baseline, a
/// checkout without history. A tarball build is not a build that has drifted.
/// What the bench times, end to end through both backends -- and the bench
/// itself, which is the entry this list was missing.
///
/// A change to `bench.rs` can move a recorded number with no renderer change
/// at all, and did: adding the two stroked rows on 2026-09-18 took the Vulkan
/// frame row from 13.955 to 14.135, one and three tenths per cent, because two
/// more configurations now run before it in the same process. A cold board
/// reproduced the new value to the thousandth. Without this entry the count
/// read zero across that commit -- the counter saying the board's agreement
/// still stood when what the board had agreed about had changed.
///
/// It makes the count noisier: a test added beside `recording()` flags it as
/// surely as `recording()` itself, and one did. That is the right side to err
/// on for a line that is printed rather than enforced. The cost of a false
/// positive is reading a sentence; the cost of a false negative is a baseline
/// claiming a board agreed with numbers it never saw.
const TIMED: &[&str] = &[
    "crates/emblema-core/src",
    "crates/emblema-shaders/shaders",
    "crates/emblema-hal/src",
    "crates/emblema-hal-vulkan/src",
    "crates/emblema-hal-gles/src",
    "xtask/src/bench/frames.rs",
];

pub fn baseline_drift() -> Option<(usize, &'static str)> {
    const BASELINE: &str = "tests/bench-baselines";

    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    // The commit a board run last passed against, which the baseline records
    // itself. Falling back to when the file last changed would count the
    // commits since the numbers were *written*, and a check that passed without
    // re-recording -- which is the ordinary outcome -- would not be counted at
    // all. "I ran it" has to be a fact in the tree or it is not a fact.
    let recorded = match verified_at() {
        Some(sha) => sha,
        None => git(&["log", "-1", "--format=%H", "--", BASELINE])?,
    };
    if recorded.is_empty() {
        return None;
    }
    let mut args = vec!["log", "--oneline", &format!("{recorded}..HEAD"), "--"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    args.extend(TIMED.iter().map(|p| p.to_string()));
    let listed = git(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    let count = listed.lines().filter(|l| !l.is_empty()).count();
    Some((count, BASELINE))
}

/// A path inside the repository, from wherever the tests are run.
///
/// `CARGO_MANIFEST_DIR` is `xtask`, so the repository root is its parent. Worth a
/// function rather than a literal because getting it wrong makes an existence
/// check pass by looking in the wrong place, which is worse than not checking.
#[cfg(test)]
fn repo_relative(path: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent")
        .join(path)
}

/// The line the gate prints for [`baseline_drift`].
pub fn drift_line() -> Option<String> {
    let (count, path) = baseline_drift()?;
    Some(match count {
        0 => format!("the timing baseline in {path} is current\n"),
        1 => format!(
            "1 commit has touched what the bench times since {path} was \
             recorded. Timing is not gated here; `cargo xtask bench --check` on \
             a board is what would say.\n"
        ),
        n => format!(
            "{n} commits have touched what the bench times since {path} was \
             recorded. Timing is not gated here; `cargo xtask bench --check` on \
             a board is what would say.\n"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::frames::shapes;
    use super::*;

    /// A timing with unremarkable numbers, for the tests that are about the
    /// report rather than about what was measured.
    fn a_timing(what: &'static str) -> Timing {
        Timing {
            what,
            builds: false,
            draws: SHAPES,
            median: Duration::from_micros(100),
            p99: Duration::from_micros(140),
            fastest: Duration::from_micros(90),
            slowest: Duration::from_micros(110),
        }
    }

    /// The stacked frame covers what its documentation says it covers.
    ///
    /// This scene exists for two numbers -- that opaque content hides 94 per cent
    /// of the wash beneath it, and that the whole frame paints 2.53 times its own
    /// area -- and both are stated in prose above `stacked`. Prose is where this
    /// tree's defects live, so they are recomputed here from the same constants
    /// the scene draws with. A change to the layout that quietly stopped the frame
    /// from overdrawing would make it a different scene while still looking like
    /// this one.
    #[test]
    fn the_stacked_frame_overdraws_by_what_its_prose_claims() {
        let (w, h) = (frames::EXTENT.width as f32, frames::EXTENT.height as f32);
        let frame = w * h;

        let bar = w * frames::BAR_H;
        let sidebar = frames::SIDE_W * (h - frames::BAR_H);
        let cw = w - frames::SIDE_W - 2.0 * frames::INSET;
        let ch = h - frames::BAR_H - 2.0 * frames::INSET;
        let content = cw * ch;

        let covered = (bar + sidebar + content) / frame * 100.0;
        assert!(
            (covered - 94.1).abs() < 0.1,
            "the bar, sidebar and panel should hide 94.1 per cent of the wash, \
             not {covered:.1}"
        );

        let rows = frames::ROWS as f32;
        let row_h = (ch - frames::GAP * (rows + 1.0)) / rows;
        let row_area = (cw - 2.0 * frames::GAP) * row_h * rows;
        let painted = (frame + bar + sidebar + content + row_area) / frame;
        assert!(
            (painted - 2.53).abs() < 0.01,
            "the frame should paint 2.53 times its own area, not {painted:.2}"
        );

        // And the stages are what they say: one draw for the wash, three opaque
        // rectangles over it, then the rows.
        let draws = |stage| frames::stacked(stage).draw_count();
        assert_eq!(draws(frames::Stacked::Wash), 1, "the wash alone");
        assert_eq!(
            draws(frames::Stacked::Panels),
            4,
            "bar, sidebar, panel over it"
        );
        assert_eq!(
            draws(frames::Stacked::All),
            4 + frames::ROWS,
            "one draw per list row on top"
        );

        // No layer anywhere in it, which is what keeps the frame a fill
        // measurement. A blur or a group would make the deltas answer a different
        // question, and `FRAME` is already the scene that asks that one.
        assert_eq!(
            frames::stacked(frames::Stacked::All).passes.len(),
            1,
            "the stacked frame opens no layer"
        );
        assert_eq!(frames::Stacked::All.name(), frames::STACKED);
    }

    /// Each stage of the frame adds what it says, and the last is the frame.
    ///
    /// The point of the decomposition is that the *difference* between two rows
    /// is one element's cost, which only holds if each stage is the one before
    /// it plus one thing. Counted rather than trusted, because a stage that
    /// silently drew nothing extra would make its delta zero and read as an
    /// element that costs nothing.
    ///
    /// The last row also has to stay what it was: two baselines carry
    /// `full frame, mixed content`, and a decomposition that changed the whole
    /// frame while adding rows would move a number nobody asked to move.
    #[test]
    fn each_frame_stage_adds_one_element_and_the_last_is_the_whole_frame() {
        let draws = |stage| frames::frame(stage).draw_count();
        let ground = draws(frames::Frame::Ground);
        let cards = draws(frames::Frame::Cards);
        let shadows = draws(frames::Frame::Shadows);
        let all = draws(frames::Frame::All);

        // One, not two: the clear is a pass attribute rather than a draw.
        assert_eq!(ground, 1, "the gradient wash");
        assert_eq!(cards, ground + 3, "three cards over it");
        assert_eq!(shadows, cards + 3, "a shadow under each card");
        assert!(
            all > shadows,
            "the blurred layer draws something: {all} against {shadows}"
        );
        assert_eq!(
            all, 12,
            "the whole frame is twelve draws, which is what the baselines were \
             recorded against"
        );
        assert_eq!(
            frames::Frame::All.name(),
            frames::FRAME,
            "the last stage keeps the name two baselines already carry"
        );
    }

    /// The two paths do not cost the same number of draws, and the difference
    /// is the first thing to know about the comparison rather than a detail.
    ///
    /// This was written expecting them to match, on the reasoning that the same
    /// shapes drawn two ways should record the same work. They do not. Every
    /// tessellated shape carries the same solid material, so the batch merges
    /// all hundred and sixty into one draw; an analytic shape merges only with
    /// another of exactly its size. So the timing below is not fragment work
    /// against fragment work -- it is one draw of many triangles against a
    /// handful of draws of two, and reading it as anything else would be
    /// reading it wrong.
    ///
    /// It was a hundred and sixty until the analytic route began carrying the
    /// shape's own space on its vertices rather than in its material. What is
    /// left is the grid: `shapes()` builds each rectangle as `left + width` less
    /// `left`, which in f32 gives three distinct widths and three heights across
    /// the sixteen columns and ten rows, and only exactly equal sizes merge.
    #[test]
    fn the_two_paths_differ_in_draw_count_and_the_output_says_so() {
        let draws = |path| recording(path).draw_count();
        let analytic = draws(Path::Analytic);
        assert!(
            analytic > 1 && analytic < SHAPES,
            "an analytic shape merges with others of its exact size and not with \
             the rest, so this is neither one draw nor one per shape: {analytic}"
        );
        assert_eq!(
            draws(Path::TessellatedMultisampled),
            1,
            "tessellated shapes share one solid material and merge into one draw"
        );
        assert_eq!(draws(Path::TessellatedSingleSampled), 1);

        // And the number reaches the reader, because a comparison that hid it
        // would look like a measurement of shading alone.
        let rendered = text(&[Event::Measured(a_timing(Path::Analytic.name()))]);
        assert!(rendered.contains("160 draws"), "{rendered}");
    }

    #[test]
    fn the_header_names_the_governor_when_there_is_one_to_name() {
        // Conditional on purpose: a machine without cpufreq has no answer, and
        // reporting nothing is right there. What must not happen is knowing the
        // governor and not saying so, since the recording rows depend on it and a
        // reader comparing two runs cannot see it.
        let header = header();
        match governor() {
            Some(named) => {
                assert!(
                    header.contains("cpu governor") && header.contains(&named),
                    "the governor is {named} and the header does not say so: {header}"
                );
            }
            None => assert!(
                !header.contains("cpu governor"),
                "the header names a governor that could not be read: {header}"
            ),
        }
    }

    /// What building costs *relative to itself*, which no board is needed for.
    ///
    /// The `recording` rows are gated on a Pi 5 in absolute milliseconds, and that
    /// catches a regression only where someone runs a board. A ratio of one build
    /// to another cancels the processor, so it can be gated in the suite: measured
    /// on a Ryzen 9 at 5.3 GHz and on an A76 at 2.4, the stroke-to-fill ratio came
    /// to 3.18 and 3.07 across a threefold difference in speed.
    ///
    /// It does not cancel everything, and the bounds are wide for a reason found
    /// rather than guessed. The *same byte-identical binary* gives 3.18 run from
    /// ext4 and 3.75 run from tmpfs -- the stroked build eighteen per cent dearer
    /// out of tmpfs while the shorter rows do not move at all. That is consistent
    /// with how the text pages are mapped, and the mechanism is not established
    /// from here; what matters for this test is that the observed range is 3.07 to
    /// 3.75 rather than the 3.0 to 3.2 an earlier version of this comment claimed
    /// from one filesystem.
    ///
    /// So the ceiling is 5.5, which leaves half again above the highest figure any
    /// machine has produced. What it still catches is a step change: a stroker
    /// emitting seventy per cent more geometry leaves the range, and that is the
    /// failure worth having. Tighter than this would be gating where the binary
    /// happens to sit.
    /// The drift counter has to watch the measured frames, and this says it does.
    ///
    /// The entry was missing entirely until 2026-09-18, and the omission was
    /// invisible: the count read zero across a commit that moved the Vulkan frame
    /// row by one and three tenths per cent, because what changed was the bench's
    /// own shape rather than the renderer. It then pointed at `bench.rs` whole for
    /// two commits, which flagged a test and a documentation fix as though they had
    /// moved a number. Now it names `bench/frames.rs`, which holds what is measured
    /// and nothing else.
    ///
    /// A list of paths is exactly the kind of thing a later edit shortens without
    /// noticing, and a *renamed* file is how this one would break next -- nothing in
    /// the tree would say so, which is why the path is asserted rather than trusted.
    #[test]
    fn the_drift_counter_watches_the_frames_as_well_as_the_renderer() {
        const FRAMES_PATH: &str = "xtask/src/bench/frames.rs";
        assert!(
            TIMED.contains(&FRAMES_PATH),
            "the drift counter no longer watches the measured frames, so a change \
             to what the bench measures will not be counted: {TIMED:?}"
        );
        // And the path it names has to exist, since a move would leave the entry
        // pointing at nothing and the counter silently watching one thing fewer.
        assert!(
            std::path::Path::new(&repo_relative(FRAMES_PATH)).exists(),
            "{FRAMES_PATH} is watched and is not there"
        );
        // And still watches the renderer, which is the half that was never wrong.
        for expected in ["crates/emblema-core/src", "crates/emblema-shaders/shaders"] {
            assert!(TIMED.contains(&expected), "{expected} is no longer watched");
        }
    }

    #[test]
    fn building_a_stroke_costs_what_it_should_relative_to_a_fill() {
        // Medians of a handful each, alternating, so a burst of load on the
        // machine lands on both sides rather than on one. Twenty-five builds of
        // the dearest path is under twenty milliseconds even on a board.
        //
        // The alternation is the point and it was a comment rather than code
        // until CI came back at 1.99 against a floor of 2.0. One path was built
        // twenty-five times and then the other, so a runner that got busy during
        // the second block inflated the denominator alone and the ratio fell out
        // of bounds from below -- the one direction load was supposed to be
        // unable to produce. So the two paths are interleaved here, and both
        // ratios below go through this rather than dividing two separate runs.
        const ROUNDS: usize = 25;
        let medians_of = |a: Path, b: Path| -> (f64, f64) {
            let mut first = Vec::with_capacity(ROUNDS);
            let mut second = Vec::with_capacity(ROUNDS);
            for _ in 0..ROUNDS {
                for (path, into) in [(a, &mut first), (b, &mut second)] {
                    let started = Instant::now();
                    let recording = recording(path);
                    let elapsed = started.elapsed();
                    // Read it, so nothing is optimized away.
                    assert!(recording.draw_count() > 0);
                    into.push(elapsed);
                }
            }
            let median = |mut samples: Vec<Duration>| {
                samples.sort();
                samples[samples.len() / 2].as_secs_f64()
            };
            (median(first), median(second))
        };

        // Stroking a path builds two contours where filling builds one, and adds
        // the joins and the caps between them. Three times, near enough.
        let (stroked, filled) =
            medians_of(Path::StrokedTessellated, Path::TessellatedSingleSampled);
        let ratio = stroked / filled;
        assert!(
            (2.0..=5.5).contains(&ratio),
            "building a stroked path costs {ratio:.2} times building a filled one, \
             and every machine and filesystem measured so far lands between 3.07 \
             and 3.75 -- outside two to five and a half is a change in what the \
             stroker emits rather than in where this binary happens to live"
        );

        // And the analytic route does not care which it is asked for: a stroke
        // width goes into the material, not into geometry, so the field builds a
        // stroked shape for what it builds a filled one. Wider bounds than above
        // because both sides are small enough for a processor changing frequency
        // mid-run to skew them -- 0.68 was observed that way on an unpinned board,
        // against 0.97 and 0.99 pinned. What it still catches is the field falling
        // back to the tessellator, which would put this at three or more.
        let (stroked_field, filled_field) = medians_of(Path::StrokedAnalytic, Path::Analytic);
        let analytic = stroked_field / filled_field;
        assert!(
            (0.4..=1.8).contains(&analytic),
            "the analytic route builds a stroked shape for {analytic:.2} times what \
             it builds a filled one, and a stroke width belongs in the material \
             rather than the geometry -- near three would mean it started \
             tessellating"
        );
    }

    #[test]
    fn the_two_tessellated_rows_build_the_same_recording() {
        // Antialiasing decides how many samples the *pass* takes. It does not
        // decide what geometry is built, so these two have to cost the same to
        // build -- and they do, to a thousandth, once the processor is held at one
        // frequency. Under a scaling governor they read 0.341 and 0.244 on a Pi 5
        // and the difference was the clock rather than the work, which is how this
        // check came to be written.
        //
        // If it ever fails, something started depending on the sample count at
        // record time, and the two rows stopped being one comparison.
        let geometry = |path| {
            let recording = recording(path);
            let batch = &recording.passes[0].batch;
            (batch.vertices().len(), recording.draw_count())
        };
        assert_eq!(
            geometry(Path::TessellatedMultisampled),
            geometry(Path::TessellatedSingleSampled),
            "the two tessellated rows submit different geometry, so their build \
             cost is no longer one number measured twice"
        );
    }

    #[test]
    fn a_building_row_times_building_and_says_it_is_not_a_frame_rate() {
        // The way this measurement goes wrong is by not being one: hand the
        // timer a recording that already exists and it measures nothing, which
        // looks like a very fast renderer rather than like a mistake. So the
        // builder is a function, and what it built has to come back.
        let timed = time_building("probe", || recording(Path::StrokedTessellated));
        assert_eq!(
            timed.draws, 1,
            "the draw count comes from the recording the timer built, so a zero \
             here means nothing was built"
        );
        // A floor rather than "more than zero", which was the first version of
        // this line and did not hold: an `Instant::now()` immediately followed by
        // `elapsed()` answers tens of nanoseconds, so a timer that bracketed
        // nothing still passed. Ten microseconds is far above that pair and far
        // below any real build of a hundred and sixty stroked shapes -- a
        // twenty-fifth of what the machine this was written on takes, and the
        // slower the processor the wider the margin.
        assert!(
            timed.median > Duration::from_micros(10),
            "building a hundred and sixty stroked shapes came back at {:?}, which \
             is the cost of reading the clock twice rather than of building \
             anything",
            timed.median
        );
        assert!(timed.builds, "a building row has to say that it builds");

        // And the rate column is blank for it. A frame pays for its recording
        // and then for its execution, so neither half's reciprocal is a frame
        // rate, and printing one invites adding two numbers that do not add.
        let rendered = render(&Event::Measured(timed));
        assert!(
            rendered.contains("-- fps"),
            "a building row printed a frame rate: {rendered}"
        );
        let drawing = render(&Event::Measured(a_timing("probe")));
        assert!(
            !drawing.contains("-- fps"),
            "a drawing row lost its frame rate: {drawing}"
        );
    }

    #[test]
    fn a_stroked_row_strokes_and_takes_the_route_it_is_named_for() {
        // The two stroked rows exist to be a comparison, which they are only if
        // each takes the route its name claims. Read off the draw count, which
        // is what separates the two: the analytic field merges only shapes of
        // exactly one size, so the grid comes out as several draws, and the
        // tessellated one puts every outline through a single solid material
        // and comes out as one. A stroked row that had quietly fallen back to
        // tessellation would report one draw here and measure the same thing
        // twice.
        let stroked = recording(Path::StrokedAnalytic).draw_count();
        assert!(
            stroked > 1,
            "a stroked field is more than one draw over this grid, so falling \
             back to the tessellator would show here: {stroked}"
        );
        assert_eq!(
            recording(Path::StrokedTessellated).draw_count(),
            1,
            "stroked outlines share one material and merge"
        );

        // And they stroke. A fill and an outline of the same rounded rectangle
        // differ in the geometry submitted, so the vertex counts cannot match --
        // an outline is two contours where a fill is one.
        let vertices = |path| recording(path).passes[0].batch.vertices().len();
        assert_ne!(
            vertices(Path::StrokedTessellated),
            vertices(Path::TessellatedSingleSampled),
            "a stroked path and a filled one submitted the same geometry, so one \
             of them is not doing what it says"
        );
    }

    #[test]
    fn every_configuration_draws_the_same_shapes() {
        // What does hold: the geometry is the same in all three, which is what
        // makes the comparison about the path rather than about the content.
        let first: Vec<_> = shapes().collect();
        assert_eq!(first.len(), SHAPES);
        assert!(first
            .iter()
            .all(|r| r.width() > 0.0 && r.height() > 0.0 && r.right <= EXTENT.width as f32));
    }

    #[test]
    fn a_percentile_is_a_frame_that_happened() {
        // Nearest rank, so every answer is a sample rather than a point
        // between two of them: these are frames that were rendered, and a
        // duration nothing took is not a frame time.
        let run: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&run, 0.5), Duration::from_millis(50));
        assert_eq!(percentile(&run, 0.99), Duration::from_millis(99));
        assert_eq!(percentile(&run, 1.0), Duration::from_millis(100));
        // A length the fractions do not divide, which is what tells nearest
        // rank from the alternatives. At a hundred samples `0.5 * 100` is a
        // whole number and every rounding agrees; at seven it is three and a
        // half, and rounding it down picks the third of seven where the rank is
        // the fourth. The first version of this test used only round hundreds
        // and passed with the rounding reversed.
        let seven: Vec<Duration> = (1..=7).map(Duration::from_millis).collect();
        assert_eq!(percentile(&seven, 0.5), Duration::from_millis(4));
        assert_eq!(percentile(&seven, 0.99), Duration::from_millis(7));
        assert_eq!(percentile(&seven, 0.25), Duration::from_millis(2));

        // And the ends hold: nothing indexes past either edge.
        assert_eq!(percentile(&run, 0.0), Duration::from_millis(1));
        assert_eq!(
            percentile(&[Duration::from_millis(7)], 0.99),
            Duration::from_millis(7)
        );
    }

    #[test]
    fn a_ninety_ninth_percentile_is_not_the_slowest_frame() {
        // The reason `FRAMES` is what it is. At thirty samples `ceil(0.99 * 30)`
        // is thirty, so a p99 would be the maximum wearing another name -- one
        // interruption, reported as a distribution. At two hundred it is the
        // third from the end, which a single stall cannot reach.
        let mut run: Vec<Duration> = vec![Duration::from_millis(1); FRAMES];
        let last = run.len() - 1;
        run[last] = Duration::from_millis(500);
        assert_eq!(
            percentile(&run, 0.99),
            Duration::from_millis(1),
            "one stalled frame in {FRAMES} reached the ninety-ninth percentile"
        );
    }

    #[test]
    fn a_shape_sits_off_the_pixel_grid() {
        // An axis-aligned rectangle at integer bounds has no edge to
        // antialias, which is the one case where the field has no advantage to
        // measure. Placing the grid on whole pixels would quietly flatter it.
        let first = shapes().next().expect("a shape");
        assert!(first.left.fract() != 0.0 && first.top.fract() != 0.0);
    }

    #[test]
    fn an_empty_run_says_so_rather_than_printing_a_bare_header() {
        assert!(text(&[]).contains("no device"));
    }

    /// A device is named when it is opened, so a run that dies measuring on it
    /// still says which one it was on.
    ///
    /// This is the whole reason [`Event`] is an enum. When a device's name
    /// arrived with its first result, a run that opened a device and finished
    /// no configuration on it printed nothing about that device at all --
    /// which is what a run on a Raspberry Pi 5 did, leaving no way to tell
    /// whether it had died opening the second device or measuring on it.
    #[test]
    fn a_device_that_measures_nothing_is_still_named() {
        let rendered = text(&[
            Event::Device("vulkan:0 a".into()),
            Event::Measured(a_timing(Path::Analytic.name())),
            Event::Measured(a_timing(Path::TessellatedMultisampled.name())),
            // Opened, and then the run ended before a configuration finished.
            Event::Device("vulkan:1 b".into()),
        ]);

        assert!(
            rendered.contains("vulkan:1 b"),
            "a device that measured nothing went unnamed: {rendered}"
        );
        assert_eq!(rendered.matches("vulkan:0 a").count(), 1, "{rendered}");
        assert_eq!(rendered.lines().filter(|l| l.starts_with("  ")).count(), 2);
    }

    /// A device is matched by any part of the name it gives itself.
    ///
    /// The name carries a driver version and a vector width, neither of which
    /// a caller can be expected to type, so matching the whole of it would
    /// make the option unusable and silently measure the device anyway.
    #[test]
    fn a_device_is_skipped_on_any_part_of_the_name_it_gives_itself() {
        let name = "vulkan:1 llvmpipe (LLVM 22.1.8, 256 bits)";
        assert!(skipped(name, &["llvmpipe".into()]));
        assert!(
            skipped(name, &["LLVMpipe".into()]),
            "case should not matter"
        );
        assert!(skipped(name, &["vulkan:1".into()]));
        assert!(!skipped(name, &["v3d".into()]));
        assert!(!skipped(name, &[]), "nothing named, nothing skipped");

        // And the hardware device beside it is not caught by the same pattern,
        // which is the whole point of asking for one.
        assert!(!skipped("vulkan:0 V3D 7.1.7.0", &["llvmpipe".into()]));
    }

    /// A device left unmeasured says so where its results would have been.
    #[test]
    fn a_skipped_device_is_named_rather_than_passed_over_quietly() {
        let rendered = text(&[
            Event::Device("vulkan:0 V3D 7.1.7.0".into()),
            Event::Measured(a_timing(Path::Analytic.name())),
            Event::Skipped("vulkan:1 llvmpipe".into()),
        ]);
        assert!(rendered.contains("vulkan:1 llvmpipe"), "{rendered}");
        assert!(rendered.contains("not measured"), "{rendered}");
    }

    /// The refusal names the command to run instead.
    ///
    /// A refusal that says only "no" costs the reader the twenty minutes it
    /// took to work out what to do about it, and this one is reached by
    /// someone who typed the documented invocation and got nothing.
    #[test]
    fn refusing_an_unoptimized_build_says_what_to_run_instead() {
        let said = why_not_debug();
        assert!(said.contains("--release"), "{said}");
        assert!(said.contains("cargo run"), "{said}");
        assert!(said.contains("bench"), "{said}");
        // And why, because a reader who does not believe it will run it anyway.
        assert!(said.contains("draws"), "{said}");
    }

    /// The frame reaches the machinery the three-route comparison never does.
    ///
    /// Without this the frame is a slower way to measure what is already
    /// measured. Each assertion names one thing the comparison cannot reach:
    /// a second pass is a layer, having its own target and a composite back;
    /// a tabulated ramp is a gradient past `MAX_STOPS`, which is an upload and
    /// a sampled texture rather than four colors riding inside a material.
    #[test]
    fn the_full_frame_reaches_a_layer_and_a_ramp() {
        let frame = frames::frame(frames::Frame::All);
        assert!(
            frame.passes.len() > 1,
            "no layer in the frame: {} pass(es)",
            frame.passes.len()
        );
        assert!(
            !frame.ramps.is_empty(),
            "no tabulated gradient, so the ramp is untimed"
        );
        // And it is a frame rather than one shape: ground, three cards, three
        // shadows and a highlight.
        assert!(frame.draw_count() >= 5, "{} draws", frame.draw_count());
    }

    /// A row's own tolerance is what gates it, not the default.
    ///
    /// The mutation that must break this: delete the `slack.unwrap_or` in
    /// `compare` and both halves fail at once -- the marked row starts failing
    /// on drift it is known to have, and the unmarked one stops failing on
    /// drift it does not.
    #[test]
    fn a_row_carrying_a_tolerance_is_held_to_that_one() {
        let baseline = Baseline::parse(
            "dev\tbimodal\t10.0\t5.0\n\
             dev\tsteady\t10.0\n",
        )
        .expect("should parse");

        // Three percent: inside the marked row's five, outside the default one.
        let found = compare(
            &baseline,
            &rows(&[("dev", "bimodal", 10.3), ("dev", "steady", 10.3)]),
            0.01,
        );
        assert_eq!(found.regressed, 1, "{}", found.lines.join("\n"));
        assert!(
            found
                .lines
                .iter()
                .any(|l| l.contains("steady") && l.contains("REGRESSED")),
            "the unmarked row is the one that should fail: {}",
            found.lines.join("\n")
        );

        // And the marked row is not exempt, only looser: six percent fails it.
        let found = compare(&baseline, &rows(&[("dev", "bimodal", 10.6)]), 0.01);
        assert_eq!(found.regressed, 1, "{}", found.lines.join("\n"));
    }

    /// A tolerance survives the re-record that overwrites the file.
    ///
    /// This is the failure the annotation would otherwise cause: `--record`
    /// writes fresh numbers over a file a person edited, and if the slack went
    /// with them the next check would gate the one row known to be
    /// unrepeatable at the tight default and fail for the reason the
    /// annotation existed to prevent.
    #[test]
    fn re_recording_keeps_the_tolerances_it_overwrites() {
        let previous =
            Baseline::parse("dev\tbimodal\t10.0\t5.0\ndev\tsteady\t10.0\n").expect("should parse");
        let text = Baseline::from_run(&rows(&[("dev", "bimodal", 11.0), ("dev", "steady", 11.0)]))
            .keeping_slack_from(&previous)
            .render();

        assert!(text.contains("bimodal\t11.000\t5.0"), "{text}");
        assert!(text.contains("steady\t11.000\n"), "{text}");

        // And it round-trips, so the next re-record keeps it too.
        let read = Baseline::parse(&text).expect("its own output should parse");
        let found = compare(&read, &rows(&[("dev", "bimodal", 11.3)]), 0.01);
        assert_eq!(found.regressed, 0, "{}", found.lines.join("\n"));
    }

    /// A fourth field that is not a tolerance is an error, not a shrug.
    #[test]
    fn a_tolerance_that_is_not_one_is_refused() {
        assert!(Baseline::parse("d\tw\t1.0\tloose").is_err());
        assert!(Baseline::parse("d\tw\t1.0\t-2.0").is_err());
        assert!(Baseline::parse("d\tw\t1.0\t2.0\textra").is_err());
        assert!(
            Baseline::parse("d\tw\t1.0\t0").is_ok(),
            "zero slack is a choice"
        );
    }

    fn rows(v: &[(&str, &str, f64)]) -> Vec<(String, String, f64)> {
        v.iter()
            .map(|(d, w, m)| (d.to_string(), w.to_string(), *m))
            .collect()
    }

    /// A baseline survives being written and read back.
    #[test]
    fn a_baseline_round_trips_through_its_own_format() {
        let original = rows(&[
            ("vulkan:0 V3D 7.1.7.0", "distance field, 1 sample", 13.376),
            ("gles V3D 7.1.7.0", "full frame, mixed content", 21.209),
        ]);
        let text = Baseline::from_run(&original).render();
        let read = Baseline::parse(&text).expect("its own output should parse");
        let found = compare(&read, &original, 0.0);
        assert_eq!(found.regressed, 0, "{:?}", found.lines);
        assert_eq!(found.unmatched, 0, "{:?}", found.lines);
    }

    /// A line that is neither three fields nor four stops the read rather
    /// than being skipped: a baseline half-understood gates on some
    /// configurations and silently ignores the others.
    #[test]
    fn a_malformed_baseline_is_refused_rather_than_partly_read() {
        assert!(Baseline::parse("device\tconfiguration").is_err());
        assert!(Baseline::parse("device\tconfiguration\tnot-a-number").is_err());
        // Comments and blank lines are not malformed.
        let ok = Baseline::parse("# a note\n\ndev\twhat\t1.5\n").expect("should parse");
        assert_eq!(ok.find("dev", "what").map(|r| r.ms), Some(1.5));
    }

    /// The device is part of the key, so a baseline from another machine does
    /// not quietly pass.
    #[test]
    fn a_baseline_from_another_device_matches_nothing() {
        let recorded = Baseline::from_run(&rows(&[("pi", "distance field", 13.0)]));
        let found = compare(
            &recorded,
            &rows(&[("workstation", "distance field", 0.9)]),
            0.05,
        );
        assert_eq!(found.regressed, 0, "a different device is not a regression");
        assert_eq!(
            found.unmatched, 2,
            "both rows are unmatched: {:?}",
            found.lines
        );
    }

    /// Slower past the tolerance fails; slower within it, and faster, do not.
    #[test]
    fn only_a_regression_past_the_tolerance_counts() {
        let recorded = Baseline::from_run(&rows(&[("d", "w", 10.0)]));
        let at = |now: f64| compare(&recorded, &rows(&[("d", "w", now)]), 0.05).regressed;
        assert_eq!(at(10.4), 0, "four percent slower is inside five");
        assert_eq!(at(10.6), 1, "six percent slower is not");
        assert_eq!(at(5.0), 0, "faster is never a regression");
        assert_eq!(at(10.5), 0, "exactly the tolerance is inside it");
    }

    #[test]
    fn a_wide_spread_is_reported_rather_than_hidden() {
        let timing = |fastest: u64, slowest: u64| Timing {
            what: "test",
            builds: false,
            draws: SHAPES,
            median: Duration::from_micros((fastest + slowest) / 2),
            p99: Duration::from_micros(slowest),
            fastest: Duration::from_micros(fastest),
            slowest: Duration::from_micros(slowest),
        };
        assert!(timing(100, 500).noisy());
        assert!(!timing(100, 150).noisy());
        assert!(text(&[Event::Measured(timing(100, 500))]).contains("noisy"));
    }
}

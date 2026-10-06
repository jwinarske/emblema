//! A long run, watching for what a short one cannot show.
//!
//! `docs/architecture.md`'s L5 row names the gap this fills. Everything that
//! rung covers is a short deterministic case: sixty frames through the scanout
//! ring, a thousand nested layers recorded once, twenty-five atlas unit tests
//! driving fullness and eviction. Each answers "does this work", and none can
//! answer "does this keep working" -- a resource leaked once per frame, or an
//! allocator fragmenting, shows up after thousands rather than after sixty.
//!
//! So this cycles the corpus until told to stop and samples what should be flat:
//! the process's resident size, and the frame time. A leak bends the first; a
//! structure growing behind it bends the second.
//!
//! # It reports rather than gates
//!
//! Like `bench`, and for the same reason the bench's own note gives: a figure
//! from a machine with a desktop on it is not a threshold anybody should hold a
//! merge to. What it *will* fail on is the one signature that is not a judgment
//! call -- resident size rising in every bucket of the run, with no bucket flat
//! or falling. A leak does that and noise does not.
//!
//! # Reading it
//!
//! The first bucket is warm-up and is excluded from the verdict: the first pass
//! over the corpus uploads the fixture sheet, the glyph atlas and every baked
//! ramp a scene asks for, and that growth is the renderer doing its job. What
//! the run is about is whether the buckets after it are flat.
//!
//! Resident size is read from `/proc/self/statm`, which is pages and needs no
//! dependency. It moves in page-sized steps, so a bucket differing by a few
//! kilobytes is quantization rather than growth -- which is why the verdict asks
//! for a rise in *every* bucket rather than a total above a threshold.

use emblema_hal::{Hal, HalContext, PixelFormat};
use emblema_hal_gles::{DisplayTarget, GlesContext, GlesHal};
use emblema_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
use emblema_testkit::{corpus, record_scene, Scene};
use std::time::{Duration, Instant};

/// How many buckets a run is divided into, whatever its length.
///
/// By time rather than by pass count, which the first version got wrong: two
/// passes over the corpus is nine milliseconds on a desktop GPU, so a
/// twenty-second run came back with seventeen hundred buckets -- unreadable, and
/// enough of them that "rose in every bucket" could never be true of noise or of
/// a leak. Twelve is few enough to read and enough to see a trend in.
const BUCKETS: usize = 12;

/// What one bucket of the run measured.
struct Bucket {
    frames: usize,
    resident_kib: usize,
    median_ms: f64,
}

/// The process's resident set size in KiB, from `/proc/self/statm`.
///
/// The second field is resident pages. Returns `None` where the file is not
/// there, which is every platform but Linux -- the soak then reports timing
/// alone rather than refusing to run.
fn resident_kib() -> Option<usize> {
    let text = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: usize = text.split_whitespace().nth(1)?.parse().ok()?;
    let page = 4096usize; // getpagesize, without a dependency to ask it
    Some(pages * page / 1024)
}

/// Render every scene in `scenes` once, returning each frame's duration.
fn one_pass<H: Hal>(
    ctx: &mut H::Context,
    scenes: &[(Scene, emblema_core::Recording)],
) -> Result<Vec<Duration>, String>
where
    H::Context: HalContext<Hal = H>,
{
    let mut times = Vec::with_capacity(scenes.len());
    for (scene, recording) in scenes {
        // The fixtures are uploaded and released per frame on purpose. A soak
        // that hoisted them would stop exercising the upload and release path,
        // which is where a texture leaked once a frame would come from.
        let fixtures = emblema_testkit::Fixtures::<H>::prepare(ctx, scene)
            .map_err(|e| format!("{}: fixtures: {e}", scene.name))?;
        let started = Instant::now();
        let drawn = emblema_core::render_offscreen_into::<H>(
            ctx,
            recording,
            &fixtures.bound(),
            PixelFormat::Rgba8Unorm,
        );
        times.push(started.elapsed());
        fixtures.destroy(ctx);
        // A scene this device declines is not a leak. It counts as a frame that
        // cost what it cost, because refusing is work too and a refusal that
        // leaked would be the interesting case.
        drop(drawn);
    }
    Ok(times)
}

fn median(mut times: Vec<Duration>) -> f64 {
    times.sort();
    if times.is_empty() {
        return 0.0;
    }
    times[times.len() / 2].as_secs_f64() * 1000.0
}

/// Soak one device for `seconds`, returning its buckets.
fn soak_device<H: Hal>(
    ctx: &mut H::Context,
    scenes: &[(Scene, emblema_core::Recording)],
    seconds: u64,
) -> Result<Vec<Bucket>, String>
where
    H::Context: HalContext<Hal = H>,
{
    let slice = Duration::from_secs_f64(seconds as f64 / BUCKETS as f64);
    let mut buckets = Vec::with_capacity(BUCKETS);
    for _ in 0..BUCKETS {
        let until = Instant::now() + slice;
        let mut times = Vec::new();
        // At least one pass, so a slice shorter than a pass still measures
        // something rather than closing an empty bucket.
        loop {
            times.extend(one_pass::<H>(ctx, scenes)?);
            if Instant::now() >= until {
                break;
            }
        }
        buckets.push(Bucket {
            frames: times.len(),
            resident_kib: resident_kib().unwrap_or(0),
            median_ms: median(times),
        });
    }
    Ok(buckets)
}

/// Whether resident size rose in every bucket after the warm-up.
///
/// The one signature that is not a judgment call. Asked of the buckets after
/// the first, because the first pass over the corpus uploads every fixture and
/// baked ramp the scenes ask for and that growth is intended.
fn leaks(buckets: &[Bucket]) -> bool {
    if buckets.len() < 4 {
        return false; // too short to say anything
    }
    // Every step after the warm-up, which `BUCKETS` keeps small enough that this
    // means something: twelve consecutive rises is a trend, and one of
    // seventeen hundred would be a coin landing the same way every time.

    buckets
        .windows(2)
        .skip(1)
        .all(|w| w[1].resident_kib > w[0].resident_kib)
}

fn describe(device: &str, buckets: &[Bucket]) -> String {
    let mut out = format!("\n{device}\n");
    if buckets.is_empty() {
        out.push_str("  nothing ran\n");
        return out;
    }
    out.push_str("  bucket  frames  resident  median frame\n");
    for (i, b) in buckets.iter().enumerate() {
        let note = if i == 0 { "  (warm-up)" } else { "" };
        out.push_str(&format!(
            "  {:>6}  {:>6}  {:>6} KiB  {:>8.3} ms{note}\n",
            i, b.frames, b.resident_kib, b.median_ms
        ));
    }
    let first = &buckets[1.min(buckets.len() - 1)];
    let last = buckets.last().expect("non-empty");
    out.push_str(&format!(
        "  resident {} KiB to {} ({:+} KiB over the whole run after warm-up)\n",
        first.resident_kib,
        last.resident_kib,
        last.resident_kib as i64 - first.resident_kib as i64,
    ));

    // The back half, not the whole run, because the front of a run settles. A
    // radeonsi GLES context measured here steps from 0.071 ms to 0.105 at the
    // third bucket and is flat at 0.105 for the nine after it -- comparing the
    // first post-warm-up bucket against the last calls that a 46 per cent drift,
    // which it is not. A clock settling is a step and then a plateau; a leak or a
    // growing structure is a slope that does not stop. The per-bucket rows above
    // are what distinguishes them, and this line is about the half that should be
    // flat.
    let back = &buckets[buckets.len() / 2..];
    let (lo, hi) = back.iter().fold((f64::MAX, 0.0f64), |(lo, hi), b| {
        (lo.min(b.median_ms), hi.max(b.median_ms))
    });
    let (bf, bl) = (back[0].median_ms, back[back.len() - 1].median_ms);
    out.push_str(&format!(
        "  median over the back half: {bf:.3} ms to {bl:.3} ({:+.1}%), spread {lo:.3} to {hi:.3}\n",
        if bf > 0.0 {
            (bl - bf) / bf * 100.0
        } else {
            0.0
        },
    ));
    // And the same for resident, for the same reason, which took measuring
    // twice to learn: a V3D Vulkan context here steps 18152 KiB to 18432 at the
    // third bucket and holds 18432 for the ten after it. Reported against the
    // first post-warm-up bucket that is "+280 KiB", which reads as a slow leak
    // and is a one-time step. The run's own verdict never thought it was one --
    // `leaks` wants a rise in *every* bucket -- but the summary line said
    // something the rows did not.
    let (rlo, rhi) = back.iter().fold((usize::MAX, 0usize), |(lo, hi), b| {
        (lo.min(b.resident_kib), hi.max(b.resident_kib))
    });
    out.push_str(&format!(
        "  resident over the back half: {} KiB to {} ({:+} KiB), spread {rlo} to {rhi}\n",
        back[0].resident_kib,
        back[back.len() - 1].resident_kib,
        back[back.len() - 1].resident_kib as i64 - back[0].resident_kib as i64,
    ));
    if leaks(buckets) {
        out.push_str(
            "  LEAK: resident size rose in every bucket after the warm-up, which \
             noise does not do\n",
        );
    }
    out
}

/// Run the soak on every device present. `Err` names the devices that leaked.
pub fn run(seconds: u64, skip: &[String]) -> Result<String, String> {
    let scenes: Vec<(Scene, emblema_core::Recording)> = corpus()
        .into_iter()
        .filter_map(|scene| record_scene(&scene).ok().map(|r| (scene, r)))
        .collect();
    let mut out = format!(
        "soaking {} corpus scenes in {BUCKETS} buckets, {seconds}s a device\n",
        scenes.len()
    );
    let mut leaked: Vec<String> = Vec::new();

    for index in 0.. {
        let Ok(mut ctx) = VulkanContext::new(DevicePreference::Index(index)) else {
            break;
        };
        let device = format!("vulkan:{index} {}", ctx.capabilities().device_name);
        if skip
            .iter()
            .any(|s| device.to_lowercase().contains(&s.to_lowercase()))
        {
            out.push_str(&format!("\n{device}\n  not soaked, as asked\n"));
            continue;
        }
        let buckets = soak_device::<VulkanHal>(&mut ctx, &scenes, seconds)?;
        if leaks(&buckets) {
            leaked.push(device.clone());
        }
        out.push_str(&describe(&device, &buckets));
    }

    match GlesContext::new(DisplayTarget::Surfaceless) {
        Ok(mut ctx) => {
            let device = format!("gles {}", ctx.capabilities().device_name);
            if skip
                .iter()
                .any(|s| device.to_lowercase().contains(&s.to_lowercase()))
            {
                out.push_str(&format!("\n{device}\n  not soaked, as asked\n"));
            } else {
                let buckets = soak_device::<GlesHal>(&mut ctx, &scenes, seconds)?;
                if leaks(&buckets) {
                    leaked.push(device.clone());
                }
                out.push_str(&describe(&device, &buckets));
            }
        }
        Err(e) => out.push_str(&format!("\ngles\n  no context ({e})\n")),
    }

    if leaked.is_empty() {
        Ok(out)
    } else {
        Err(format!(
            "{out}\nresident size grew in every bucket on: {}",
            leaked.join(", ")
        ))
    }
}

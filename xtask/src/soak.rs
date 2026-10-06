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

/// The scanout half: the ring driven for minutes rather than for sixty frames.
///
/// `a_long_run_neither_leaks_nor_loses_blanks` is the sixty-frame version, and
/// its comment says what it is watching -- the framebuffers the output keeps,
/// the descriptors the process keeps, the ring's depth and the CPU-wait count,
/// which are the resources the DRM path exchanges every frame. Sixty frames is
/// one second. This drives the same loop for as long as it is given and samples
/// those four per bucket, which is the difference between "does the ring work"
/// and "does the ring keep working".
///
/// It also reports **missed blanks per bucket**, which is the figure
/// `docs/architecture.md`'s L4 row says is printed and never bounded. A bound
/// still is not asserted here -- the row's reason stands, a figure worth having
/// comes from a release build on a quiet board and this command cannot know it
/// has one -- but a run that misses nothing for minutes is evidence a reader of
/// that row currently has nowhere to get.
pub mod scanout {
    use super::{leaks, resident_kib, Bucket, BUCKETS};
    use emblema_hal::{Batch, BlendMode, Material, PassDescriptor};
    use emblema_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal};
    use emblema_present::PresentTarget;
    use emblema_present_drm::{DrmScanoutTarget, KmsOutput};
    use std::time::{Duration, Instant};

    const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

    /// What the ring held at the end of a bucket, beside the bucket itself.
    struct Held {
        framebuffers: usize,
        descriptors: usize,
        ring_depth: usize,
        cpu_waits: u64,
        flips: u64,
        missed: u64,
    }

    /// Open descriptors, which is how the sixty-frame test spots a leaked fd.
    fn open_descriptors() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .map(|d| d.count())
            .unwrap_or(0)
    }

    /// The card to drive: the one named, or the first that opens.
    fn output() -> Option<KmsOutput> {
        if let Ok(path) = std::env::var("EMBLEMA_DRM_CARD") {
            return match KmsOutput::open(&path) {
                Ok(output) => Some(output),
                Err(e) => {
                    eprintln!("{path} was named and cannot be driven ({e})");
                    None
                }
            };
        }
        // Named rather than guessed is the better way round here, for the reason
        // the sixty-frame test gives: a Pi 5 lists the DSI controller first and
        // the HDMI one second, and "can this drive a display" is not the
        // question a board is worth running on to answer.
        for entry in std::fs::read_dir("/dev/dri").ok()?.flatten() {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with("card") {
                continue;
            }
            if let Ok(output) = KmsOutput::open(&entry.path().to_string_lossy()) {
                return Some(output);
            }
        }
        None
    }

    pub fn run(seconds: u64) -> Result<String, String> {
        let Some(output) = output() else {
            return Ok("scanout\n  no card this process can drive\n".to_string());
        };
        let mut ctx = VulkanContext::new(DevicePreference::Auto)
            .map_err(|e| format!("scanout: no Vulkan device ({e})"))?;
        let mut target = DrmScanoutTarget::<VulkanHal, _>::new(&mut ctx, output, 3)
            .map_err(|e| format!("scanout: building the target ({e})"))?;

        // Taken after the ring is built, so the buffers it imports on purpose
        // are not counted as growth.
        let base = Held {
            framebuffers: target.output().framebuffer_count(),
            descriptors: open_descriptors(),
            ring_depth: target.ring_depth(),
            cpu_waits: target.cpu_waits(),
            flips: target.output().pacing().flips(),
            missed: 0,
        };

        let slice = Duration::from_secs_f64(seconds as f64 / BUCKETS as f64);
        let mut buckets: Vec<(Bucket, Held)> = Vec::with_capacity(BUCKETS);
        let mut frame = 0u64;
        let mut last_flips = base.flips;
        let mut last_missed = 0u64;

        for _ in 0..BUCKETS {
            let until = Instant::now() + slice;
            let mut times = Vec::new();
            loop {
                let started = Instant::now();
                let image = target
                    .acquire(&mut ctx)
                    .map_err(|e| format!("frame {frame}: acquire ({e})"))?;
                let mut batch = Batch::new();
                let t = (frame % 8) as f32 / 8.0;
                batch
                    .push(
                        &FULL,
                        &QUAD,
                        Material::solid([t, 0.4, 1.0 - t, 1.0]),
                        BlendMode::Src,
                    )
                    .map_err(|e| format!("frame {frame}: push ({e})"))?;
                let fence = ctx
                    .submit_batch_deferred(image, &batch, PassDescriptor::clear([0.0; 4]))
                    .map_err(|e| format!("frame {frame}: submit ({e})"))?;
                target
                    .set_frame_fence(fence)
                    .map_err(|e| format!("frame {frame}: fence ({e})"))?;
                target
                    .present(&mut ctx)
                    .map_err(|e| format!("frame {frame}: present ({e})"))?;
                times.push(started.elapsed());
                frame += 1;
                if Instant::now() >= until {
                    break;
                }
            }
            let pacing = target.output().pacing();
            let missed = pacing.missed().unwrap_or(0);
            buckets.push((
                Bucket {
                    frames: times.len(),
                    resident_kib: resident_kib().unwrap_or(0),
                    median_ms: super::median(times),
                },
                Held {
                    framebuffers: target.output().framebuffer_count(),
                    descriptors: open_descriptors(),
                    ring_depth: target.ring_depth(),
                    cpu_waits: target.cpu_waits(),
                    flips: pacing.flips() - last_flips,
                    missed: missed - last_missed,
                },
            ));
            last_flips = pacing.flips();
            last_missed = missed;
        }

        let mut out = format!(
            "\nscanout {} ({} frames, ring {})\n",
            target.output().path(),
            frame,
            base.ring_depth
        );
        out.push_str("  bucket  frames   flips  missed  resident  fbs  fds  waits  median\n");
        for (i, (b, h)) in buckets.iter().enumerate() {
            let note = if i == 0 { "  (warm-up)" } else { "" };
            out.push_str(&format!(
                "  {:>6}  {:>6}  {:>6}  {:>6}  {:>6} KiB  {:>3}  {:>3}  {:>5}  {:>6.3} ms{note}\n",
                i,
                b.frames,
                h.flips,
                h.missed,
                b.resident_kib,
                h.framebuffers,
                h.descriptors,
                h.cpu_waits,
                b.median_ms
            ));
        }

        // The four the sixty-frame test holds constant, held over the whole run.
        let grew: Vec<String> = buckets
            .iter()
            .skip(1)
            .enumerate()
            .filter_map(|(i, (_, h))| {
                let mut why = Vec::new();
                if h.framebuffers != base.framebuffers {
                    why.push(format!(
                        "framebuffers {} against {}",
                        h.framebuffers, base.framebuffers
                    ));
                }
                if h.descriptors > base.descriptors {
                    why.push(format!(
                        "descriptors {} against {}",
                        h.descriptors, base.descriptors
                    ));
                }
                if h.ring_depth != base.ring_depth {
                    why.push(format!("ring {} against {}", h.ring_depth, base.ring_depth));
                }
                (!why.is_empty()).then(|| format!("bucket {}: {}", i + 1, why.join(", ")))
            })
            .collect();

        let total_missed: u64 = buckets.iter().skip(1).map(|(_, h)| h.missed).sum();
        out.push_str(&format!(
            "  missed {total_missed} blanks after the warm-up, over {} flips\n",
            buckets.iter().skip(1).map(|(_, h)| h.flips).sum::<u64>()
        ));

        let plain: Vec<Bucket> = buckets.into_iter().map(|(b, _)| b).collect();
        let mut problems = grew;
        if leaks(&plain) {
            problems.push("resident size rose in every bucket after the warm-up".to_string());
        }
        if problems.is_empty() {
            Ok(out)
        } else {
            Err(format!(
                "{out}\nthe ring did not hold steady: {}",
                problems.join("; ")
            ))
        }
    }
}

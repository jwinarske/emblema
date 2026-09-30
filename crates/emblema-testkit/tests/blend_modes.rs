//! Every blend mode, checked against the equation it claims to implement.
//!
//! The expectations are computed from the blend equation rather than recorded
//! from a run, so a mode wired to the wrong factors — or to the wrong blend op
//! — fails rather than being enshrined. The two families are checked the same
//! way but not with the same colors, because what distinguishes the modes
//! within a family differs: Porter-Duff modes differ in *where* each side
//! survives, so they need two partly transparent sides, while the separable
//! modes differ in *how* the two mix per channel, so they need channels that
//! are all distinct and none of them zero.

use emblema_hal::{
    blend::blend_advanced, Batch, BlendFactor, BlendMode, Capability, Error, Extent2D, Hal,
    HalContext, Material, PassDescriptor, PixelFormat, TextureDescriptor,
};
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesHal};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use googletest::prelude::*;

const SIZE: Extent2D = Extent2D {
    width: 16,
    height: 16,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Straight-alpha source, as a caller writes it.
///
/// Both sides are deliberately partly transparent, and with different alphas.
/// Porter-Duff modes are distinguished by *where* each side survives, so an
/// opaque destination collapses most of them onto each other — source-in
/// becomes source, destination-over becomes destination, and so on. Alphas of
/// 0.4 and 0.75 also keep each factor distinct from its complement, which an
/// alpha of 0.5 would not.
const SRC: [f32; 4] = [1.0, 0.0, 0.0, 0.4];
/// Premultiplied source, as the shader emits it and the equation sees it.
const SRC_PREMULTIPLIED: [f32; 4] = [0.4, 0.0, 0.0, 0.4];
/// The destination, cleared before the draw. Already premultiplied.
const DST: [f32; 4] = [0.0, 0.0, 0.75, 0.75];

/// Straight-alpha source for the separable modes.
///
/// A red source over a blue destination would send multiply, darken and several
/// others to the same near-black result, because a separable function of two
/// channels where one is zero is usually zero. Every channel here is distinct
/// and away from both ends, which is also what keeps dodge and burn off their
/// saturating branches where they would agree with screen and multiply.
/// Both sides are near-opaque rather than half transparent, and that is about
/// sensitivity rather than realism. The composite scales a blend function's
/// contribution by the product of the two alphas, so at a half each the
/// difference between a right formula and a wrong one arrives at the target
/// attenuated to about a third — small enough that a two-percent error in a
/// luminosity landed inside the one-unit tolerance and passed. Near one, almost
/// all of it survives, and the premultiplied round trip is still exercised
/// because neither side is actually opaque.
/// The channels also stay clear of where the formulas turn. Dodge and burn
/// clamp at a ratio of one, hard-light and overlay switch branches at a half,
/// and soft-light switches at a quarter; a channel sitting on one of those is a
/// knife edge where the hardware's un-premultiply and this one's need only
/// disagree in their last bit to fall on opposite sides. The first destination
/// tried here put burn exactly on its clamp in two channels and disagreed by
/// four units for that reason alone.
const ADV_SRC: [f32; 4] = [0.9, 0.4, 0.15, 0.92];
/// The destination for the advanced modes, premultiplied at alpha 0.88.
const ADV_DST: [f32; 4] = [0.264, 0.396, 0.616, 0.88];

/// Evaluate a factor the way the hardware does.
fn factor_value(factor: BlendFactor, src: [f32; 4], dst: [f32; 4], channel: usize) -> f32 {
    match factor {
        BlendFactor::Zero => 0.0,
        BlendFactor::One => 1.0,
        BlendFactor::SrcAlpha => src[3],
        BlendFactor::OneMinusSrcAlpha => 1.0 - src[3],
        BlendFactor::DstAlpha => dst[3],
        BlendFactor::OneMinusDstAlpha => 1.0 - dst[3],
        BlendFactor::DstColor => dst[channel],
    }
}

fn quantize(value: [f32; 4]) -> [u8; 4] {
    let mut out = [0u8; 4];
    for channel in 0..4 {
        out[channel] = (value[channel].clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out
}

/// The source and destination a mode is exercised with, premultiplied.
///
/// The straight-alpha source is what gets handed to the material; the
/// premultiplied one is what the shader emits and the equation sees.
fn colors(mode: BlendMode) -> ([f32; 4], [f32; 4], [f32; 4]) {
    if mode.is_advanced() {
        let a = ADV_SRC[3];
        let premultiplied = [ADV_SRC[0] * a, ADV_SRC[1] * a, ADV_SRC[2] * a, a];
        (ADV_SRC, premultiplied, ADV_DST)
    } else {
        (SRC, SRC_PREMULTIPLIED, DST)
    }
}

/// What the blend equation says the result should be.
fn expected(mode: BlendMode) -> [u8; 4] {
    let (_, src, dst) = colors(mode);
    match mode.factors() {
        Some(factors) => {
            let mut out = [0.0f32; 4];
            for (channel, slot) in out.iter_mut().enumerate() {
                *slot = src[channel] * factor_value(factors.src, src, dst, channel)
                    + dst[channel] * factor_value(factors.dst, src, dst, channel);
            }
            quantize(out)
        }
        // The separable formula, from the specification, evaluated on the same
        // premultiplied inputs the hardware is given.
        None => quantize(blend_advanced(mode, src, dst).expect("advanced formula")),
    }
}

/// Draw the source over a cleared destination with one mode, and read a pixel.
fn render<H: Hal>(ctx: &mut H::Context, mode: BlendMode) -> Result<[u8; 4], Error>
where
    H::Context: HalContext<Hal = H>,
{
    let (straight, _, dst) = colors(mode);
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, Material::solid(straight), mode)
        .expect("push");

    let mut target =
        ctx.create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))?;
    let submitted = ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(dst));
    let result = submitted.and_then(|()| ctx.read_texture(&mut target));
    ctx.destroy_texture(target);
    let pixels = result?;

    let middle = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    Ok([
        pixels[middle],
        pixels[middle + 1],
        pixels[middle + 2],
        pixels[middle + 3],
    ])
}

/// One eighth-bit unit, for every mode in both families.
///
/// A blended result is converted to fixed point and either of the two nearest
/// values is permitted, which is the whole of the allowance. The separable
/// modes were expected to need more, since they un-premultiply before blending
/// and re-premultiply afterwards and soft-light adds a square root on top; they
/// measure at one unit on both physical devices here, so they get one unit. A
/// looser bound would have to be justified by a device that actually needs it,
/// and until then it would only hide a wrong formula: a three-percent error in
/// darken passes at two units and fails at one.
const TOLERANCE: i32 = 1;

fn near(got: [u8; 4], want: [u8; 4]) -> bool {
    got.iter()
        .zip(&want)
        .all(|(a, b)| (*a as i32 - *b as i32).abs() <= TOLERANCE)
}

/// Check a backend against the equation, for whichever modes it supports.
fn check_against_equation<H: Hal>(ctx: &mut H::Context, backend: &str)
where
    H::Context: HalContext<Hal = H>,
{
    let advanced = ctx.capabilities().advanced_blend;
    let mut checked = 0;
    for mode in BlendMode::ALL {
        if mode.is_advanced() && !advanced {
            continue;
        }
        match render::<H>(ctx, *mode) {
            Ok(got) => {
                checked += 1;
                let want = expected(*mode);
                // Every mode reported at once: a wrong factor table usually
                // breaks several, and seeing which ones is what identifies the
                // mistake. A fatal assertion here would name one and stop.
                expect_true!(
                    near(got, want),
                    "{backend} {mode}: got {got:?}, equation says {want:?}"
                );
            }
            Err(e) => {
                expect_true!(
                    false,
                    "{backend} {mode}: refused despite being supported: {e}"
                )
            }
        }
    }
    // In the shape `cargo xtask verify` carries up, and naming the backend so
    // that three devices checking three different numbers come out as three
    // lines. The assertion below is a floor; this is what was actually reached,
    // and on a device without the advanced extension the two differ by half the
    // table while the suite passes either way.
    eprintln!(
        "compared {checked} of {} blend modes against the equations on {backend}",
        BlendMode::ALL.len()
    );
    assert!(
        checked >= BlendMode::PORTER_DUFF.len(),
        "{backend} checked only {checked} modes; every device must do all of Porter-Duff"
    );
}

#[gtest]
fn every_mode_matches_its_equation_on_vulkan() {
    // Both devices, because they do not offer the same modes. Advanced blending
    // is an extension one physical device can have and another can lack on the
    // same machine, and testing only whichever one `Auto` prefers means the
    // advanced modes go unchecked wherever the preferred device is the one
    // without it. Which device supplies the coverage is not asserted: that is a
    // property of the machine, not of the renderer.
    let mut ran = false;
    for (preference, label) in [
        (DevicePreference::Auto, "vulkan/auto"),
        (DevicePreference::Software, "vulkan/software"),
    ] {
        let Ok(mut ctx) = Validated::new(preference) else {
            continue;
        };
        check_against_equation::<VulkanHal>(&mut ctx, label);
        ran = true;
    }
    if !ran {
        eprintln!("skipping: no Vulkan device");
    }
}

#[gtest]
fn every_mode_matches_its_equation_on_gles() {
    let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    check_against_equation::<GlesHal>(&mut ctx, "gles");
}

#[test]
fn a_mode_the_device_cannot_do_is_refused_rather_than_approximated() {
    // The failure this guards against is not an error but a picture: a backend
    // that reports no advanced blending and then draws plain source-over
    // anyway looks almost right, and nobody finds that by looking.
    //
    // This used to run only where a backend happened to lack the capability, and
    // said so: "on a machine where both have it there is nothing here to check".
    // It now withholds the capability instead, so both backends are checked
    // everywhere and the only skip left is for a backend that will not start.
    //
    // Each half asserts three things rather than one, because "refused" on its
    // own is what the old shape could not tell from "this machine never had it":
    // the restricted context refuses every advanced mode, the *unrestricted* one
    // accepts them where the device can, and an ordinary mode draws the same
    // pixels either way -- which is what says the withholding is narrow rather
    // than a context that has stopped working.
    let mut ran_on = Vec::new();

    if let Ok(mut restricted) =
        Validated::without(DevicePreference::Auto, Capability::AdvancedBlend)
    {
        for mode in BlendMode::ADVANCED {
            assert!(
                matches!(
                    render::<VulkanHal>(&mut restricted, *mode),
                    Err(Error::Unsupported(_))
                ),
                "vulkan accepted {mode} with advanced blending withheld"
            );
        }
        let restricted_plain =
            render::<VulkanHal>(&mut restricted, BlendMode::SrcOver).expect("an ordinary mode");

        let mut ordinary = Validated::new(DevicePreference::Auto).expect("the same device again");
        assert_eq!(
            restricted_plain,
            render::<VulkanHal>(&mut ordinary, BlendMode::SrcOver).expect("an ordinary mode"),
            "withholding advanced blending moved a source-over draw"
        );
        if ordinary.capabilities().advanced_blend {
            assert!(
                render::<VulkanHal>(&mut ordinary, BlendMode::Multiply).is_ok(),
                "vulkan refused an advanced mode it reports it has, so the \
                 refusal above says nothing about the withholding"
            );
        }
        ran_on.push("vulkan");
    }

    if let Ok(mut restricted) =
        GlesValidated::without(DisplayTarget::Surfaceless, Capability::AdvancedBlend)
    {
        for mode in BlendMode::ADVANCED {
            assert!(
                matches!(
                    render::<GlesHal>(&mut restricted, *mode),
                    Err(Error::Unsupported(_))
                ),
                "gles accepted {mode} with advanced blending withheld"
            );
        }
        let restricted_plain =
            render::<GlesHal>(&mut restricted, BlendMode::SrcOver).expect("an ordinary mode");
        drop(restricted);

        let mut ordinary =
            GlesValidated::new(DisplayTarget::Surfaceless).expect("the same context again");
        assert_eq!(
            restricted_plain,
            render::<GlesHal>(&mut ordinary, BlendMode::SrcOver).expect("an ordinary mode"),
            "withholding advanced blending moved a source-over draw"
        );
        if ordinary.capabilities().advanced_blend {
            assert!(
                render::<GlesHal>(&mut ordinary, BlendMode::Multiply).is_ok(),
                "gles refused an advanced mode it reports it has, so the refusal \
                 above says nothing about the withholding"
            );
        }
        ran_on.push("gles");
    }

    assert!(
        !ran_on.is_empty(),
        "neither backend would start, so nothing was checked"
    );
}

#[gtest]
fn the_porter_duff_modes_agree_between_the_backends() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        return;
    };

    // Only the modes both backends have. The advanced modes -- separable and
    // non-separable alike, since one extension gates both -- are Vulkan-only
    // here, and comparing them across backends is what the corpus does through
    // an explicit capability gate rather than what this test papers over.
    for mode in BlendMode::PORTER_DUFF {
        let a = render::<VulkanHal>(&mut vulkan, *mode).expect("vulkan");
        let b = render::<GlesHal>(&mut gles, *mode).expect("gles");
        expect_true!(near(a, b), "{mode}: vulkan {a:?}, gles {b:?}");
    }
}

#[test]
fn the_modes_are_actually_distinct() {
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        let Ok(ctx) = Validated::new(preference) else {
            continue;
        };
        check_distinctness(ctx);
    }
}

fn check_distinctness(mut ctx: Validated) {
    // A table where two entries collapsed onto the same factors — or two modes
    // onto the same blend op — would pass every equation check above, because
    // the equation would then be wrong in the same way as the implementation.
    // This compares how many distinct results were rendered against how many
    // the equation predicts, so it needs no threshold and does not care which
    // modes happen to coincide for these particular colors.
    //
    // The two families are counted separately because they are exercised with
    // different colors, and a result from one carries no information about
    // whether a result from the other is distinct.
    let advanced = ctx.capabilities().advanced_blend;
    let families: &[(&str, &[BlendMode])] = if advanced {
        &[
            ("porter-duff", BlendMode::PORTER_DUFF),
            ("advanced", BlendMode::ADVANCED),
        ]
    } else {
        &[("porter-duff", BlendMode::PORTER_DUFF)]
    };

    for (name, modes) in families {
        let mut rendered: Vec<[u8; 4]> = modes
            .iter()
            .map(|mode| render::<VulkanHal>(&mut ctx, *mode).expect("render"))
            .collect();
        let mut predicted: Vec<[u8; 4]> = modes.iter().map(|mode| expected(*mode)).collect();
        for list in [&mut rendered, &mut predicted] {
            list.sort_unstable();
            list.dedup();
        }
        assert_eq!(
            rendered.len(),
            predicted.len(),
            "the {name} modes render {} distinct results where the equation predicts {}",
            rendered.len(),
            predicted.len()
        );
    }
}

/// Two overlapping draws, as one batch and as two submissions.
///
/// `PassDescriptor::preserve` between submissions is a hard ordering: the
/// second draw cannot start before the first has landed. Within one batch there
/// is no such barrier, and the hardware is only obliged to produce the same
/// answer because the device reported coherent advanced blending. This renders
/// both ways and requires them to agree, which is what turns that reported
/// feature into something checked rather than believed — a driver blending the
/// second shape against the original backdrop instead of against the first
/// shape's result would differ exactly in the overlap.
fn overlapping_draws_two_ways<H: Hal>(ctx: &mut H::Context, mode: BlendMode) -> (Vec<u8>, Vec<u8>)
where
    H::Context: HalContext<Hal = H>,
{
    // Two rectangles sharing a middle band, so part of each lies over bare
    // backdrop and part over the other.
    const LEFT: [[f32; 2]; 4] = [[-0.9, -0.6], [0.3, -0.6], [0.3, 0.6], [-0.9, 0.6]];
    const RIGHT: [[f32; 2]; 4] = [[-0.3, -0.6], [0.9, -0.6], [0.9, 0.6], [-0.3, 0.6]];
    let first = [0.85, 0.45, 0.2, 0.75];
    let second = [0.3, 0.65, 0.9, 0.75];

    let render_with = |ctx: &mut H::Context, batched: bool| {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
            .expect("texture");
        if batched {
            let mut batch = Batch::new();
            batch
                .push(&LEFT, &QUAD, Material::solid(first), mode)
                .expect("push");
            batch
                .push(&RIGHT, &QUAD, Material::solid(second), mode)
                .expect("push");
            ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(ADV_DST))
                .expect("one batch");
        } else {
            for (geometry, color, pass) in [
                (&LEFT, first, PassDescriptor::clear(ADV_DST)),
                (&RIGHT, second, PassDescriptor::preserve()),
            ] {
                let mut batch = Batch::new();
                batch
                    .push(geometry, &QUAD, Material::solid(color), mode)
                    .expect("push");
                ctx.submit_batch(&mut target, &batch, pass)
                    .expect("separate batch");
            }
        }
        let pixels = ctx.read_texture(&mut target).expect("readback");
        ctx.destroy_texture(target);
        pixels
    };

    let batched = render_with(ctx, true);
    let separate = render_with(ctx, false);
    (batched, separate)
}

/// Every advanced mode, batched once and submitted twice, must agree.
///
/// The two renderings differ only if the second draw blended against a
/// destination the first had not finished writing, so agreement is the whole
/// of what coherency means here.
fn check_coherence<H: Hal>(ctx: &mut H::Context, backend: &str) -> usize
where
    H::Context: HalContext<Hal = H>,
{
    let mut checked = 0;
    for mode in BlendMode::ADVANCED {
        let (batched, separate) = overlapping_draws_two_ways::<H>(ctx, *mode);
        let differing = batched
            .chunks_exact(4)
            .zip(separate.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            differing, 0,
            "{backend} {mode}: {differing} pixel(s) differ between one batch and \
             two submissions, so blending is not coherent within a batch"
        );
        checked += 1;
    }
    checked
}

#[test]
fn advanced_blending_is_coherent_within_a_batch() {
    let mut ran = false;
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        let Ok(mut ctx) = Validated::new(preference) else {
            continue;
        };
        if !ctx.capabilities().advanced_blend {
            continue;
        }
        check_coherence::<VulkanHal>(&mut ctx, "vulkan");
        ran = true;
    }
    if !ran {
        eprintln!("skipping: no device reports advanced blending");
    }
}

#[test]
fn advanced_blending_is_coherent_within_a_batch_on_gles() {
    // Separate from the Vulkan test above because the two get coherency from
    // different places, and only one of them gets it for free. Vulkan asks for
    // `advancedBlendCoherentOperations` and refuses the device without it;
    // GLES has a coherent variant of the extension that this machine's driver
    // does not offer, so the backend calls `glBlendBarrierKHR` before every
    // advanced draw instead.
    //
    // What this does *not* check is that barrier, and the distinction is worth
    // writing down because the first draft of this comment claimed the
    // opposite. Removing the barrier leaves both halves of the comparison
    // equally wrong -- the driver returns the source unblended either way --
    // so a test that compares them cannot see it. What sees it is
    // `every_mode_matches_its_equation_on_gles`, which compares against an
    // answer rather than against another rendering. This checks the property
    // its name says instead: that how a picture is submitted does not change
    // it.
    let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    if !ctx.capabilities().advanced_blend {
        eprintln!("skipping: this GLES context has no advanced-blend extension");
        return;
    }
    let checked = check_coherence::<GlesHal>(&mut ctx, "gles");
    assert_eq!(
        checked,
        BlendMode::ADVANCED.len(),
        "every advanced mode should have been checked"
    );
}

#[test]
fn overlapping_advanced_draws_actually_blend_with_each_other() {
    // The test above compares two renderings that would both be wrong if the
    // second shape ignored the first. This checks the overlap is genuinely a
    // three-way result: it must match neither shape drawn alone over the
    // backdrop, or the scene proves nothing about ordering.
    let Some(mut ctx) = [DevicePreference::Auto, DevicePreference::Software]
        .into_iter()
        .filter_map(|p| Validated::new(p).ok())
        .find(|ctx| ctx.capabilities().advanced_blend)
    else {
        eprintln!("skipping: no device reports advanced blending");
        return;
    };

    let (batched, _) = overlapping_draws_two_ways::<VulkanHal>(&mut ctx, BlendMode::Multiply);
    let row = (SIZE.height / 2) as usize * SIZE.width as usize * 4;
    // A column inside the shared band, and one inside the left shape only.
    let overlap = &batched[row + 8 * 4..row + 8 * 4 + 4];
    let left_only = &batched[row + 2 * 4..row + 2 * 4 + 4];
    assert_ne!(
        overlap, left_only,
        "the overlap matches the region covered by only one shape"
    );
}

/// The layer alphas a group is composited at, and why two.
///
/// One is the composite with nothing scaling it, which is the mode on its own.
/// A half is the case a renderer can get right and still have the order wrong:
/// a group's alpha scales what it holds *before* the mode combines it with the
/// frame, and applying it afterwards agrees at one and disagrees everywhere
/// else.
const GROUP_ALPHAS: [f32; 2] = [1.0, 0.5];

/// A group composited with one mode, and the pixel it left.
///
/// Through `Canvas` rather than through a batch, because there is no other way
/// to say "a group": the batch-level test above pushes one draw, and a group is
/// a target rendered into and composited out of, which only the recorder
/// builds.
fn composite_group<H: Hal>(
    ctx: &mut H::Context,
    mode: BlendMode,
    alpha: f32,
) -> Result<[u8; 4], Error>
where
    H::Context: HalContext<Hal = H>,
{
    use emblema_core::{Canvas, Color, Layer, Paint, Rect};

    let mut canvas = Canvas::new(SIZE);
    // What a clear writes is what it is given: these are the numbers that end
    // up in the target, so the equation below takes them as they are rather
    // than premultiplying them again.
    canvas.clear(Color::srgb(ADV_DST[0], ADV_DST[1], ADV_DST[2], ADV_DST[3]));
    canvas.save_layer(Layer::opacity(alpha).with_blend(mode));
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, SIZE.width as f32, SIZE.height as f32),
            &Paint::fill(Color::srgb(ADV_SRC[0], ADV_SRC[1], ADV_SRC[2], ADV_SRC[3])),
        )
        .map_err(|_| Error::Unsupported("the group's own draw"))?;
    canvas.restore();

    let pixels = emblema_core::render_offscreen_into::<H>(
        ctx,
        &canvas.finish(),
        &[],
        PixelFormat::Rgba8Unorm,
    )?;
    let middle = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    Ok([
        pixels[middle],
        pixels[middle + 1],
        pixels[middle + 2],
        pixels[middle + 3],
    ])
}

/// Every advanced mode composites a *group* by its equation too.
///
/// The test above pushes one draw and checks the mode against the formula. A
/// group is the other half of what `dart:ui` offers and nothing was checking
/// it: what reaches the blend is not a shape's color but a finished target,
/// scaled by the group's own alpha, and that is a different path through the
/// renderer with the same specification behind it.
///
/// The gap was worth closing on its own and was found sideways. Two versions of
/// one software rasterizer disagree with each other by up to ninety levels on
/// exactly this arrangement -- `docs/on-a-board.md` has it -- and there was no
/// way to say which of them was right, because nothing here computed the
/// answer. Now something does, and both agree with it.
///
/// One tolerance unit, the same as the draw-level check, and the same reasoning:
/// a looser bound would only hide a wrong order of operations.
#[gtest]
fn every_advanced_mode_composites_a_group_by_its_equation() {
    let mut ran = false;
    let check = |label: &str, run: &mut dyn FnMut(BlendMode, f32) -> Result<[u8; 4], Error>| {
        let mut checked = 0;
        for mode in BlendMode::ALL.iter().filter(|m| m.is_advanced()) {
            for alpha in GROUP_ALPHAS {
                match run(*mode, alpha) {
                    Ok(got) => {
                        checked += 1;
                        // The group's contents premultiplied by its alpha
                        // against the destination as the clear left it.
                        let a = ADV_SRC[3] * alpha;
                        let source = [ADV_SRC[0] * a, ADV_SRC[1] * a, ADV_SRC[2] * a, a];
                        let want = quantize(
                            blend_advanced(*mode, source, ADV_DST).expect("advanced formula"),
                        );
                        expect_true!(
                            near(got, want),
                            "{label} {mode} at group alpha {alpha}: got {got:?}, \
                             equation says {want:?}"
                        );
                    }
                    Err(e) => expect_true!(false, "{label} {mode} at {alpha}: refused: {e}"),
                }
            }
        }
        if checked > 0 {
            eprintln!("compared {checked} group composites against the equations on {label}");
        }
        checked
    };

    for (preference, label) in [
        (DevicePreference::Auto, "vulkan/auto"),
        (DevicePreference::Software, "vulkan/software"),
    ] {
        let Ok(mut ctx) = Validated::new(preference) else {
            continue;
        };
        if !ctx.capabilities().advanced_blend {
            continue;
        }
        ran |= check(label, &mut |mode, alpha| {
            composite_group::<VulkanHal>(&mut ctx, mode, alpha)
        }) > 0;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        if ctx.capabilities().advanced_blend {
            ran |= check("gles", &mut |mode, alpha| {
                composite_group::<GlesHal>(&mut ctx, mode, alpha)
            }) > 0;
        }
    }
    if !ran {
        eprintln!("skipping: no device here composites a group with an advanced mode");
    }
}

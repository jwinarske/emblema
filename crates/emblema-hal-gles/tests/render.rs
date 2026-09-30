//! Offscreen rendering on the GLES backend.
//!
//! The assertions here are deliberately the same shape as the Vulkan ones —
//! exact pixel positions, exact channel values, computed blend results — so a
//! divergence between the backends shows up as a specific failing property
//! rather than as two suites that merely happen to pass.

use emblema_hal::{
    Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat, TextureDescriptor,
};
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesTexture};

const SIZE: u32 = 32;
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn context() -> Option<GlesValidated> {
    match GlesValidated::new(DisplayTarget::Surfaceless) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable GLES context ({e})");
            None
        }
    }
}

fn target(ctx: &mut GlesValidated) -> GlesTexture {
    ctx.create_texture(&TextureDescriptor::offscreen(
        Extent2D::new(SIZE, SIZE),
        PixelFormat::Rgba8Unorm,
    ))
    .expect("texture")
}

const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A quad covering a horizontal band of clip space.
fn band(y0: f32, y1: f32) -> [[f32; 2]; 4] {
    [[-1.0, y0], [1.0, y0], [1.0, y1], [-1.0, y1]]
}

fn render(ctx: &mut GlesValidated, batch: &Batch) -> Vec<u8> {
    let mut tex = target(ctx);
    ctx.submit_batch(&mut tex, batch, PassDescriptor::clear(BLACK))
        .expect("submit");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    pixels
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn a_full_quad_covers_the_target_in_the_paint_color() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    assert!(
        pixels.chunks_exact(4).all(|p| p == [255, 0, 0, 255]),
        "expected a solid red target"
    );
}

#[test]
fn each_channel_of_the_paint_arrives_independently() {
    let Some(mut ctx) = context() else { return };
    // The paint reaches the shader through a lowered uniform rather than a push
    // constant, so this checks the lowering as much as the draw.
    for (color, expected) in [
        ([1.0, 0.0, 0.0, 1.0], [255, 0, 0, 255]),
        ([0.0, 1.0, 0.0, 1.0], [0, 255, 0, 255]),
        ([0.0, 0.0, 1.0, 1.0], [0, 0, 255, 255]),
    ] {
        let mut batch = Batch::new();
        batch
            .push(&FULL, &QUAD, Material::solid(color), BlendMode::Src)
            .expect("push");
        let pixels = render(&mut ctx, &batch);
        assert_eq!(pixel(&pixels, 4, 4), expected, "for {color:?}");
    }
}

#[test]
fn clip_space_follows_the_wgsl_convention_with_y_up() {
    let Some(mut ctx) = context() else { return };
    // Clip Y in [-1, 0] is the lower half under the WGSL convention, so it must
    // land at the bottom of the image, which is exactly what the Vulkan backend
    // asserts. Framebuffer origins differ between the two APIs and the shader
    // translator cancels the difference, so no correction is applied on
    // readback; adding one would mirror the image, which this catches.
    let mut batch = Batch::new();
    batch
        .push(
            &band(-1.0, 0.0),
            &QUAD,
            Material::solid([1.0, 1.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    assert_eq!(pixel(&pixels, SIZE / 2, SIZE - 2)[0], 255, "bottom covered");
    assert_eq!(pixel(&pixels, SIZE / 2, 2)[0], 0, "top untouched");
}

#[test]
fn source_over_blends_against_what_is_already_there() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([0.0, 0.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 0.5]),
            BlendMode::SrcOver,
        )
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    // The same arithmetic the Vulkan backend is held to: premultiplied source
    // over opaque blue gives half red, half blue, opaque.
    let got = pixel(&pixels, SIZE / 2, SIZE / 2);
    for (channel, want) in got.iter().zip([128u8, 0, 128, 255]) {
        assert!(
            (*channel as i32 - want as i32).abs() <= 1,
            "got {got:?}, expected about [128, 0, 128, 255]"
        );
    }
}

#[test]
fn draws_within_a_batch_keep_their_order() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    batch
        .push(
            &band(-1.0, 0.0),
            &QUAD,
            Material::solid([0.0, 1.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    // Second draw wins where they overlap, first survives where they do not.
    assert_eq!(pixel(&pixels, SIZE / 2, SIZE - 2), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, SIZE / 2, 2), [255, 0, 0, 255]);
}

#[test]
fn index_offsets_address_each_draw_correctly() {
    let Some(mut ctx) = context() else { return };
    // Every draw after the first is offset into shared buffers, and GL takes
    // that offset in bytes rather than in indices. Getting the unit wrong draws
    // the first shape repeatedly, which two differently placed bands catch.
    let mut batch = Batch::new();
    batch
        .push(
            &band(-1.0, -0.5),
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    batch
        .push(
            &band(0.5, 1.0),
            &QUAD,
            Material::solid([0.0, 1.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    let pixels = render(&mut ctx, &batch);

    assert_eq!(
        pixel(&pixels, SIZE / 2, SIZE - 2),
        [255, 0, 0, 255],
        "first"
    );
    assert_eq!(pixel(&pixels, SIZE / 2, 1), [0, 255, 0, 255], "second");
    // And the middle stays as cleared, or the two draws overlapped.
    assert_eq!(pixel(&pixels, SIZE / 2, SIZE / 2), [0, 0, 0, 255], "middle");
}

#[test]
fn an_empty_batch_still_clears() {
    let Some(mut ctx) = context() else { return };
    let batch = Batch::new();
    let mut tex = target(&mut ctx);
    ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 1.0, 1.0]),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    assert_eq!(pixel(&pixels, 0, 0), [0, 0, 255, 255]);
}

/// A triangle with a shallow edge, so partial coverage is measurable.
///
/// Not 45 degrees: sample patterns are symmetric about the diagonal, so an
/// exactly diagonal edge yields only zero, half, or full coverage no matter how
/// many samples are taken.
const SHALLOW: [[f32; 2]; 3] = [[-1.0, -1.0], [1.0, -1.0], [-1.0, 0.35]];

fn render_at(ctx: &mut GlesValidated, samples: u32) -> Vec<u8> {
    let mut batch = Batch::new();
    batch
        .push(
            &SHALLOW,
            &[0, 1, 2],
            Material::solid([1.0, 1.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    let mut tex = target(ctx);
    ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear(BLACK).with_samples(samples),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    pixels
}

fn partial_coverage(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|p| p[0] > 0 && p[0] < 255)
        .count()
}

#[test]
fn multisampling_produces_partial_coverage_along_the_edge() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    assert_eq!(partial_coverage(&render_at(&mut ctx, 1)), 0, "aliased");
    let partial = partial_coverage(&render_at(&mut ctx, 4));
    assert!(
        partial >= SIZE as usize / 2,
        "expected partial coverage along the edge, got {partial} pixels"
    );
}

#[test]
fn every_advertised_sample_count_is_one_the_driver_honors() {
    let Some(mut ctx) = context() else { return };
    // `RenderbufferStorageMultisample` rounds an unsupported request up to the
    // next count the format supports and reports no error, so a capability mask
    // that guesses is not caught by anything a rendered pixel could show: the
    // image is correct and costs twice what was budgeted.
    //
    // The mask used to be built by taking every power of two up to
    // `MAX_SAMPLES`. Both drivers available here contradict that -- llvmpipe
    // has no 2x and radeonsi no 1x multisample -- so this asks the backend to
    // actually allocate at every count it advertises. It fails on a mask that
    // claims a count the driver would silently substitute.
    let advertised: Vec<u32> = [1u32, 2, 4, 8, 16]
        .into_iter()
        .filter(|n| ctx.capabilities().sample_counts.supports(*n))
        .collect();
    assert!(
        advertised.contains(&1),
        "single-sampled should always be available, got {advertised:?}"
    );
    for samples in advertised {
        let pixels = render_at(&mut ctx, samples);
        assert_eq!(
            pixels.len(),
            (SIZE * SIZE * 4) as usize,
            "{samples}x did not render a full target"
        );
    }
}

#[test]
fn the_requested_sample_count_is_actually_used() {
    let Some(mut ctx) = context() else { return };
    // Resolving N samples yields at most N+1 distinct levels, so rendering at a
    // lower count than requested shows up as too few levels. The resolve here
    // is a blit rather than a render-pass attachment, so this checks a
    // genuinely different mechanism from the Vulkan side.
    //
    // The upper bound holds only because the backend refuses a pass whose
    // realized sample count differs from the requested one. Without that it is
    // not a property GLES offers: a driver may allocate more samples than it
    // was asked for, and llvmpipe does, which made this fail there.
    let mut seen = Vec::new();
    for samples in [1u32, 2, 4] {
        if !ctx.capabilities().sample_counts.supports(samples) {
            continue;
        }
        let pixels = render_at(&mut ctx, samples);
        let levels = pixels
            .chunks_exact(4)
            .map(|p| p[0])
            .collect::<std::collections::BTreeSet<u8>>()
            .len();
        assert!(
            levels <= samples as usize + 1,
            "{samples}x produced {levels} levels, more than resolving {samples} samples allows"
        );
        seen.push((samples, levels));
    }
    let best = seen.last().copied().expect("at least one count");
    assert!(best.1 > 2, "{}x produced only {} levels", best.0, best.1);
}

#[test]
fn a_multisampled_pass_that_would_preserve_is_refused() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        return;
    }
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
        .expect("push");
    let mut tex = target(&mut ctx);
    // Blitting single-sample into multisample is not legal, so there is no way
    // to seed the buffer with what the target held. The Vulkan backend refuses
    // for the same reason, which makes this a property of the technique rather
    // than of one backend.
    let result = ctx.submit_batch(&mut tex, &batch, PassDescriptor::preserve().with_samples(4));
    assert!(result.is_err());
    ctx.destroy_texture(tex);
}

#[test]
fn an_unsupported_sample_count_is_refused() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, Material::solid([1.0; 4]), BlendMode::Src)
        .expect("push");
    let mut tex = target(&mut ctx);
    for samples in [3u32, 128] {
        let result = ctx.submit_batch(
            &mut tex,
            &batch,
            PassDescriptor::clear(BLACK).with_samples(samples),
        );
        assert!(result.is_err(), "{samples}x should be refused");
    }
    ctx.destroy_texture(tex);
}

#[test]
fn textures_can_be_created_and_destroyed_repeatedly() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([0.0, 1.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    for i in 0..16 {
        let pixels = render(&mut ctx, &batch);
        assert_eq!(pixel(&pixels, 0, 0), [0, 255, 255, 255], "iteration {i}");
    }
}

/// A multisampled pass that preserves is refused, and the refusal is useful.
///
/// Both backends decline this. Vulkan's suite has said so for as long as the
/// refusal has existed; this backend's did not, and neither checked what the
/// refusal actually said -- which is how the message came to describe only the
/// operation it could not perform. A caller meets it through the front door,
/// antialiasing being on by default, a fresh canvas having no background and a
/// stroked line having only triangles, so what the message names is the whole
/// of what they have to go on.
///
/// The restriction belongs to the technique rather than to either backend, so
/// the Vulkan suite asserts the same property in the same shape.
#[test]
fn a_multisampled_pass_that_preserves_is_refused_with_both_remedies_named() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    let mut tex = target(&mut ctx);
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::solid([1.0, 1.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");

    let outcome = ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor {
            clear: None,
            samples: 4,
            viewport: None,
        },
    );
    let Err(emblema_hal::Error::Unsupported(message)) = outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    assert!(
        message.contains("clear color") && message.contains("one sample"),
        "the refusal names neither remedy: {message}"
    );

    // Each remedy on its own, so neither is merely mentioned.
    ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor {
            clear: Some(BLACK),
            samples: 4,
            viewport: None,
        },
    )
    .expect("clearing makes it drawable");
    ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor {
            clear: None,
            samples: 1,
            viewport: None,
        },
    )
    .expect("one sample makes it drawable");
    ctx.destroy_texture(tex);
}

//! Multisampled rendering.
//!
//! The observable difference is coverage: without multisampling a pixel is
//! either inside a triangle or outside it, so a diagonal edge is a staircase of
//! fully-on and fully-off pixels. With it, edge pixels take intermediate values
//! proportional to how much of the pixel the triangle covers. Counting those
//! intermediate pixels is a direct measurement rather than an eyeball test.

use emblema_hal::{
    Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat, TextureDescriptor,
};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

const SIZE: u32 = 64;

fn context() -> Option<VulkanContext> {
    match VulkanContext::with_config(ContextConfig {
        device: DevicePreference::Auto,
        validation: true,
        ..Default::default()
    }) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            eprintln!("skipping: no usable Vulkan device ({e})");
            None
        }
    }
}

/// A triangle whose hypotenuse crosses the target at a shallow angle.
///
/// Deliberately not 45 degrees. Standard sample patterns are symmetric about
/// the diagonal, so an exactly diagonal edge yields only 0, 50 or 100 percent
/// coverage no matter how many samples are taken — measured here, a 45 degree
/// edge produced three distinct levels at both 4x and 8x, and none at all at
/// 2x. A shallow edge makes the sample count observable.
const DIAGONAL: [[f32; 2]; 3] = [[-1.0, -1.0], [1.0, -1.0], [-1.0, 0.35]];

/// Fraction of the target the triangle covers: half of a 2 by 1.35 area within
/// a clip volume of area 4.
const COVERAGE: f32 = 0.3375;

fn render(ctx: &mut VulkanContext, samples: u32) -> Vec<u8> {
    let mut batch = Batch::new();
    batch
        .push(
            &DIAGONAL,
            &[0, 1, 2],
            Material::solid([1.0, 1.0, 1.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");

    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(SIZE, SIZE),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]).with_samples(samples),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
    pixels
}

/// Pixels that are neither fully covered nor fully uncovered.
fn partial_coverage(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|p| p[0] > 0 && p[0] < 255)
        .count()
}

/// How many distinct coverage values appear.
///
/// Resolving N samples can produce at most N+1 levels, so this measures
/// whether the requested sample count was actually used.
fn coverage_levels(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .map(|p| p[0])
        .collect::<std::collections::BTreeSet<u8>>()
        .len()
}

fn covered(pixels: &[u8]) -> usize {
    pixels.chunks_exact(4).filter(|p| p[0] > 127).count()
}

#[test]
fn single_sampled_edges_are_all_or_nothing() {
    let Some(mut ctx) = context() else { return };
    let pixels = render(&mut ctx, 1);
    // Every pixel is either inside the triangle or outside it. Any partial
    // value here would mean something other than coverage produced it.
    assert_eq!(
        partial_coverage(&pixels),
        0,
        "a single-sampled edge should have no intermediate values"
    );
}

#[test]
fn multisampling_produces_partial_coverage_along_the_edge() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    let pixels = render(&mut ctx, 4);
    let partial = partial_coverage(&pixels);

    // The edge crosses tens of pixels and each should be partially covered.
    // Requiring a substantial count rather than merely non-zero catches a
    // resolve that ran but effectively sampled one location.
    assert!(
        partial >= SIZE as usize / 2,
        "expected partial coverage along the edge, got {partial} pixels"
    );
}

#[test]
fn the_requested_sample_count_is_actually_used() {
    let Some(mut ctx) = context() else { return };
    // Resolving N samples yields at most N+1 distinct levels, so a backend that
    // silently rendered at a lower count would show up as too few levels here.
    // This is the sharpest available check that the sample count reached the
    // pipeline and the render pass rather than only the descriptor.
    let mut seen = Vec::new();
    for samples in [1u32, 2, 4, 8] {
        if !ctx.capabilities().sample_counts.supports(samples) {
            continue;
        }
        let levels = coverage_levels(&render(&mut ctx, samples));
        assert!(
            levels <= samples as usize + 1,
            "{samples}x produced {levels} levels, more than resolving {samples} samples allows"
        );
        seen.push((samples, levels));
    }

    // And more samples must actually buy more levels, or the count is being
    // ignored somewhere below the descriptor.
    let best = seen.last().copied().expect("at least one sample count");
    assert!(
        best.1 > 2,
        "the highest supported count ({}x) produced only {} levels",
        best.0,
        best.1
    );
    for pair in seen.windows(2) {
        assert!(
            pair[1].1 >= pair[0].1,
            "{}x gave {} levels but {}x gave {}",
            pair[0].0,
            pair[0].1,
            pair[1].0,
            pair[1].1
        );
    }
}

#[test]
fn more_samples_never_reduce_the_number_of_intermediate_values() {
    let Some(mut ctx) = context() else { return };
    let mut previous = 0;
    for samples in [1u32, 2, 4, 8] {
        if !ctx.capabilities().sample_counts.supports(samples) {
            continue;
        }
        let partial = partial_coverage(&render(&mut ctx, samples));
        assert!(
            partial >= previous,
            "{samples}x produced {partial} intermediate pixels, fewer than the previous {previous}"
        );
        previous = partial;
    }
    assert!(previous > 0, "no sample count above one was supported");
}

#[test]
fn multisampling_does_not_move_the_shape() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    let aliased = covered(&render(&mut ctx, 1));
    let smooth = covered(&render(&mut ctx, 4));

    // Antialiasing redistributes coverage at the boundary; it must not shift or
    // rescale the shape.
    let expected = (SIZE * SIZE) as f32 * COVERAGE;
    for (what, count) in [("aliased", aliased), ("multisampled", smooth)] {
        let ratio = count as f32 / expected;
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "{what} covered {count}, expected about {expected}"
        );
    }
}

#[test]
fn interior_pixels_are_untouched_by_multisampling() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        eprintln!("skipping: 4x not supported");
        return;
    }
    let pixels = render(&mut ctx, 4);
    let at = |x: u32, y: u32| pixels[((y * SIZE + x) * 4) as usize];

    // Clip Y runs up, so the triangle occupies the lower-left of the image and
    // its hypotenuse sweeps up to the left edge. Sampling near the corners of
    // that edge would land on partially covered pixels, which is why these
    // points are well away from it.
    assert_eq!(at(2, SIZE - 3), 255, "deep interior");
    assert_eq!(at(SIZE - 3, 2), 0, "deep exterior");
}

#[test]
fn an_unsupported_sample_count_is_refused() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(
            &DIAGONAL,
            &[0, 1, 2],
            Material::solid([1.0; 4]),
            BlendMode::Src,
        )
        .expect("push");
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(16, 16),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");

    // Not a power of two, so not a valid count on any device.
    let result = ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0; 4]).with_samples(3),
    );
    assert!(result.is_err(), "3x should be refused");

    // Beyond what any current device offers, and refused against reported
    // capabilities rather than left for the driver to reject.
    let result = ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0; 4]).with_samples(128),
    );
    assert!(result.is_err(), "128x should be refused");

    ctx.destroy_texture(tex);
}

#[test]
fn a_multisampled_pass_that_would_preserve_is_refused() {
    let Some(mut ctx) = context() else { return };
    if !ctx.capabilities().sample_counts.supports(4) {
        return;
    }
    let mut batch = Batch::new();
    batch
        .push(
            &DIAGONAL,
            &[0, 1, 2],
            Material::solid([1.0; 4]),
            BlendMode::Src,
        )
        .expect("push");
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(16, 16),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");

    // Preserving would need the target's contents seeded into the multisample
    // buffer, and there is no reverse of a resolve to do it with. Refusing
    // beats silently discarding what the target held.
    let result = ctx.submit_batch(&mut tex, &batch, PassDescriptor::preserve().with_samples(4));
    let Err(emblema_hal::Error::Unsupported(message)) = result else {
        panic!("expected a refusal, got {result:?}");
    };

    // What the refusal says, and not merely that there was one. This is the
    // first thing a caller drawing an antialiased line into an uncleared
    // canvas sees, and naming the operation that could not be performed left
    // them with nothing to do about it. Both remedies are real and either one
    // alone is enough, which the two draws below hold to.
    assert!(
        message.contains("clear color") && message.contains("one sample"),
        "the refusal names neither remedy: {message}"
    );
    ctx.submit_batch(
        &mut tex,
        &batch,
        PassDescriptor::clear([0.0; 4]).with_samples(4),
    )
    .expect("clearing makes it drawable");
    ctx.submit_batch(&mut tex, &batch, PassDescriptor::preserve())
        .expect("one sample makes it drawable");

    ctx.destroy_texture(tex);
}

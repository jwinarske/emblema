//! Clipping at the HAL boundary, by scissor and by stencil, across both
//! backends.
//!
//! The whole risk in this feature is orientation, in both directions. OpenGL
//! measures its scissor box upward from the bottom where Vulkan measures
//! downward from the top, so converting looks obligatory — and here it is not,
//! because the shader translator already negates Y for GLSL and cancels the
//! difference. Converting anyway mirrors the clip; that is what these caught
//! when the conversion was first written in.
//!
//! Either mistake is invisible in a target symmetric about its horizontal
//! center line, so every clip here sits deliberately off-center, with distinct
//! distances from all four edges. The stencil tests further down inherit that
//! discipline: clip space runs upward where an image's rows run downward, and
//! getting the negation wrong shifts a clip rather than obviously breaking it.

use emblema_hal::{
    Batch, BlendMode, ClipState, Extent2D, Hal, HalContext, Material, PassDescriptor, PixelFormat,
    Scissor, TextureDescriptor,
};
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesHal};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};

const SIZE: Extent2D = Extent2D {
    width: 32,
    height: 32,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const FOREGROUND: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Cover the whole target with one draw, confined to `clip`.
fn render<H: Hal>(ctx: &mut H::Context, clip: Option<Scissor>) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    let mut batch = Batch::new();
    batch
        .push_clipped(
            &FULL,
            &QUAD,
            Material::solid(FOREGROUND),
            BlendMode::Src,
            clip,
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("texture");
    ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(BACKGROUND))
        .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    pixels
}

/// Whether a pixel holds the foreground rather than the background.
fn covered(pixels: &[u8], x: u32, y: u32) -> bool {
    let i = ((y * SIZE.width + x) * 4) as usize;
    pixels[i] > 128
}

/// Every pixel the draw actually reached, as a rectangle.
///
/// Derived from the image rather than asserted pixel by pixel, so a failure
/// reports the rectangle that was drawn and not merely that some pixel was
/// wrong. A clip flipped vertically shows up as a different origin, which is
/// exactly what a per-pixel assertion would report least clearly.
fn covered_bounds(pixels: &[u8]) -> Option<Scissor> {
    let (mut min_x, mut min_y) = (u32::MAX, u32::MAX);
    let (mut max_x, mut max_y) = (0u32, 0u32);
    let mut any = false;
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            if covered(pixels, x, y) {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    any.then(|| Scissor::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
}

/// A clip touching none of the four edges and no center line.
///
/// Distinct distances from every edge, so a flip, a transpose, or an off-by-one
/// in either axis each produce a different rectangle.
const OFF_CENTER: Scissor = Scissor::new(5, 3, 11, 20);

fn check_backend<H: Hal>(ctx: &mut H::Context, backend: &str)
where
    H::Context: HalContext<Hal = H>,
{
    let pixels = render::<H>(ctx, Some(OFF_CENTER));
    assert_eq!(
        covered_bounds(&pixels),
        Some(OFF_CENTER),
        "{backend} drew outside or short of the clip"
    );

    // And the draw really was confined rather than merely starting there: every
    // pixel within the rectangle is covered and every pixel outside it is not.
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            let inside = x >= OFF_CENTER.x
                && x < OFF_CENTER.right()
                && y >= OFF_CENTER.y
                && y < OFF_CENTER.bottom();
            assert_eq!(
                covered(&pixels, x, y),
                inside,
                "{backend}: pixel ({x}, {y}) is {}",
                if inside {
                    "missing"
                } else {
                    "outside the clip"
                }
            );
        }
    }
}

#[test]
fn a_clip_confines_a_draw_on_vulkan() {
    let mut ctx = match Validated::new(DevicePreference::Auto) {
        Ok(ctx) => ctx,
        // With the reason. A board with a driver the loader can list and not
        // open reports one thing here and something quite different from the
        // error, and "no Vulkan device" sent one such run looking for a device
        // that was plugged in and working.
        Err(e) => {
            eprintln!("skipping: no Vulkan device ({e})");
            return;
        }
    };
    check_backend::<VulkanHal>(&mut ctx, "vulkan");
}

#[test]
fn a_clip_confines_a_draw_on_gles() {
    let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };
    check_backend::<GlesHal>(&mut ctx, "gles");
}

#[test]
fn the_backends_place_a_clip_in_the_same_half_of_the_target() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        return;
    };

    // A band across the top only. The two APIs disagree about which edge `y`
    // is measured from, so this is the comparison that fails outright if the
    // conversion is missing rather than merely being a pixel off.
    let top = Scissor::new(0, 0, SIZE.width, 8);
    let a = render::<VulkanHal>(&mut vulkan, Some(top));
    let b = render::<GlesHal>(&mut gles, Some(top));
    assert_eq!(
        covered_bounds(&a),
        Some(top),
        "vulkan put the band elsewhere"
    );
    assert_eq!(covered_bounds(&b), Some(top), "gles put the band elsewhere");
    assert_eq!(a, b, "the backends disagree about where the clip is");
}

#[test]
fn an_unclipped_draw_covers_the_whole_target() {
    for name in ["vulkan", "gles"] {
        let pixels = match name {
            "vulkan" => match Validated::new(DevicePreference::Auto) {
                Ok(mut ctx) => render::<VulkanHal>(&mut ctx, None),
                Err(_) => continue,
            },
            _ => match GlesValidated::new(DisplayTarget::Surfaceless) {
                Ok(mut ctx) => render::<GlesHal>(&mut ctx, None),
                Err(_) => continue,
            },
        };
        assert_eq!(
            covered_bounds(&pixels),
            Some(Scissor::covering(SIZE)),
            "{name} clipped a draw that asked for no clip"
        );
    }
}

#[test]
fn a_clip_does_not_restrict_the_clear() {
    // The clear fills the target; the clip confines only what is drawn over it.
    // This is a real difference between the two APIs: a GL clear is subject to
    // the scissor test and a Vulkan load-op clear is not, so a backend leaving
    // the test enabled across the clear would leave most of the target holding
    // whatever its memory happened to contain.
    let mut checked = 0;
    let corner = Scissor::new(20, 24, 6, 5);

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let pixels = render::<VulkanHal>(&mut ctx, Some(corner));
        assert_background_outside(&pixels, corner, "vulkan");
        checked += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let pixels = render::<GlesHal>(&mut ctx, Some(corner));
        assert_background_outside(&pixels, corner, "gles");
        checked += 1;
    }
    assert!(checked > 0, "no backend available");
}

fn assert_background_outside(pixels: &[u8], clip: Scissor, backend: &str) {
    let background = [0u8, 0, 0, 255];
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            if x >= clip.x && x < clip.right() && y >= clip.y && y < clip.bottom() {
                continue;
            }
            let i = ((y * SIZE.width + x) * 4) as usize;
            assert_eq!(
                &pixels[i..i + 4],
                &background,
                "{backend}: ({x}, {y}) outside the clip did not receive the clear"
            );
        }
    }
}

#[test]
fn clips_change_between_draws_within_one_batch() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    // Two draws with different clips, and one with none between them. Scissor
    // is state that persists until it is set again, so a backend that sets it
    // for the first draw and never restores it would confine the rest of the
    // batch to that first rectangle.
    let left = Scissor::new(0, 0, 8, SIZE.height);
    let right = Scissor::new(24, 0, 8, SIZE.height);
    let mut batch = Batch::new();
    for clip in [Some(left), None, Some(right)] {
        batch
            .push_clipped(
                &FULL,
                &QUAD,
                Material::solid(FOREGROUND),
                BlendMode::Src,
                clip,
            )
            .expect("push");
    }

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("texture");
    ctx.submit_batch(&mut target, &batch, PassDescriptor::clear(BACKGROUND))
        .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);

    // The unclipped draw in the middle covers everything, so the whole target
    // ends up painted. What this catches is the opposite: a stale scissor.
    assert_eq!(
        covered_bounds(&pixels),
        Some(Scissor::covering(SIZE)),
        "a clip leaked into a later draw that asked for none"
    );
}

#[test]
fn an_empty_clip_draws_nothing() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    // A clip narrowed to nothing is a normal state for a subtree scrolled out
    // of view, not an error. The draw is dropped rather than recorded, and the
    // target keeps its clear.
    let pixels = render::<VulkanHal>(&mut ctx, Some(Scissor::EMPTY));
    assert_eq!(
        covered_bounds(&pixels),
        None,
        "an empty clip drew something"
    );

    let mut batch = Batch::new();
    batch
        .push_clipped(
            &FULL,
            &QUAD,
            Material::solid(FOREGROUND),
            BlendMode::Src,
            Some(Scissor::EMPTY),
        )
        .expect("push");
    assert_eq!(
        batch.draw_count(),
        0,
        "a draw that can write no pixel was still recorded"
    );
}

// --- Stencil clipping, which is what a clip that is not a rectangle needs. ---

/// A rectangle in clip space, from one given in pixels with a top-left origin.
///
/// Y is negated because clip space follows the WGSL convention and runs upward,
/// so the image's top row sits at positive one.
fn clip_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> [[f32; 2]; 4] {
    let to_clip = |x: f32, y: f32| {
        [
            x / SIZE.width as f32 * 2.0 - 1.0,
            1.0 - y / SIZE.height as f32 * 2.0,
        ]
    };
    [
        to_clip(x0, y0),
        to_clip(x1, y0),
        to_clip(x1, y1),
        to_clip(x0, y1),
    ]
}

/// Build a clip from a rectangle, then fill the target through it.
fn stencil_clipped_batch(depth_two: bool) -> Batch {
    let mut batch = Batch::new();
    batch
        .push_with(
            &clip_rect(5.0, 3.0, 21.0, 26.0),
            &QUAD,
            Material::solid(FOREGROUND),
            emblema_hal::ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::narrow(0),
        )
        .expect("outer");
    if depth_two {
        batch
            .push_with(
                &clip_rect(11.0, 9.0, 30.0, 30.0),
                &QUAD,
                Material::solid(FOREGROUND),
                emblema_hal::ColorFilter::None,
                BlendMode::Src,
                None,
                ClipState::narrow(1),
            )
            .expect("inner");
    }
    batch
        .push_with(
            &FULL,
            &QUAD,
            Material::solid(FOREGROUND),
            emblema_hal::ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::content(if depth_two { 2 } else { 1 }),
        )
        .expect("content");
    batch
}

fn render_batch<H: Hal>(ctx: &mut H::Context, batch: &Batch, samples: u32) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("texture");
    ctx.submit_batch(
        &mut target,
        batch,
        PassDescriptor::clear(BACKGROUND).with_samples(samples),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    pixels
}

#[test]
fn a_stencil_clip_confines_a_draw_on_both_backends() {
    let batch = stencil_clipped_batch(false);
    let want = Some(Scissor::new(5, 3, 16, 23));
    let mut ran = 0;

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let pixels = render_batch::<VulkanHal>(&mut ctx, &batch, 1);
        assert_eq!(covered_bounds(&pixels), want, "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let pixels = render_batch::<GlesHal>(&mut ctx, &batch, 1);
        assert_eq!(covered_bounds(&pixels), want, "gles");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn the_backends_agree_pixel_for_pixel_on_a_nested_stencil_clip() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        return;
    };
    // A stencil clip has no per-fragment arithmetic in it: a pixel is either
    // admitted or not. So unlike a gradient or a blend, the two backends have
    // to agree exactly, and any tolerance here would only hide a defect.
    let batch = stencil_clipped_batch(true);
    let a = render_batch::<VulkanHal>(&mut vulkan, &batch, 1);
    let b = render_batch::<GlesHal>(&mut gles, &batch, 1);
    assert_eq!(
        covered_bounds(&a),
        Some(Scissor::new(11, 9, 10, 17)),
        "vulkan did not intersect the two clips"
    );
    assert_eq!(a, b, "the backends disagree about a stencil clip");
}

#[test]
fn a_stencil_clip_leaves_no_trace_on_a_later_pass() {
    // The GLES backend attaches its stencil buffer to the target's own
    // framebuffer, which outlives the pass, and drives global state a later
    // pass inherits. A color mask or a stencil test left on would make the
    // second pass here render nothing at all, which no test of the first pass
    // could catch.
    let mut ran = 0;
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let _ = render_batch::<GlesHal>(&mut ctx, &stencil_clipped_batch(true), 1);
        let mut plain = Batch::new();
        plain
            .push(&FULL, &QUAD, Material::solid(FOREGROUND), BlendMode::Src)
            .expect("push");
        let pixels = render_batch::<GlesHal>(&mut ctx, &plain, 1);
        assert_eq!(
            covered_bounds(&pixels),
            Some(Scissor::covering(SIZE)),
            "gles carried clip state into a later pass"
        );
        ran += 1;
    }
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let _ = render_batch::<VulkanHal>(&mut ctx, &stencil_clipped_batch(true), 1);
        let mut plain = Batch::new();
        plain
            .push(&FULL, &QUAD, Material::solid(FOREGROUND), BlendMode::Src)
            .expect("push");
        let pixels = render_batch::<VulkanHal>(&mut ctx, &plain, 1);
        assert_eq!(
            covered_bounds(&pixels),
            Some(Scissor::covering(SIZE)),
            "vulkan carried clip state into a later pass"
        );
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_multisampled_stencil_clip_agrees_between_the_backends() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        return;
    };
    if !(vulkan.capabilities().sample_counts.supports(4)
        && gles.capabilities().sample_counts.supports(4))
    {
        eprintln!("skipping: 4x not supported on both backends");
        return;
    }
    // The stencil attachment has to carry the color attachment's sample count
    // on both, and the two allocate it in quite different ways -- an image
    // beside the multisample color image on one, a renderbuffer on the other.
    let batch = stencil_clipped_batch(true);
    let a = render_batch::<VulkanHal>(&mut vulkan, &batch, 4);
    let b = render_batch::<GlesHal>(&mut gles, &batch, 4);
    assert_eq!(
        covered_bounds(&a),
        Some(Scissor::new(11, 9, 10, 17)),
        "the multisampled clip landed somewhere else"
    );
    assert_eq!(a, b, "the backends disagree about a multisampled clip");
}

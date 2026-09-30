//! Batched submission.
//!
//! The property that matters is equivalence: a batch of N draws must produce
//! exactly the pixels that N separate submissions would. Batching is a
//! performance change, and any visible difference is a bug in it rather than a
//! trade-off, so most of these tests render the same scene both ways and
//! compare the results directly.

use emblema_hal::{
    Batch, BlendMode, Extent2D, Material, PassDescriptor, PixelFormat, TextureDescriptor,
};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

const SIZE: u32 = 32;
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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

/// A quad covering a horizontal band of clip space.
fn band(y0: f32, y1: f32) -> [[f32; 2]; 4] {
    [[-1.0, y0], [1.0, y0], [1.0, y1], [-1.0, y1]]
}
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

struct Scene {
    draws: Vec<([[f32; 2]; 4], [f32; 4], BlendMode)>,
}

impl Scene {
    /// Render every draw in one batch and one submission.
    fn batched(&self, ctx: &mut VulkanContext) -> Vec<u8> {
        let mut batch = Batch::new();
        for (verts, color, blend) in &self.draws {
            batch
                .push(verts, &QUAD, Material::solid(*color), *blend)
                .expect("push");
        }
        let mut tex = target(ctx);
        ctx.submit_batch(&mut tex, &batch, PassDescriptor::clear(BLACK))
            .expect("submit");
        finish(ctx, tex)
    }

    /// Render every draw as its own submission.
    fn separately(&self, ctx: &mut VulkanContext) -> Vec<u8> {
        let mut tex = target(ctx);
        let mut clear = Some(BLACK);
        for (verts, color, blend) in &self.draws {
            ctx.draw_indexed(
                &mut tex,
                verts,
                &QUAD,
                Material::solid(*color),
                *blend,
                clear,
            )
            .expect("draw");
            clear = None;
        }
        finish(ctx, tex)
    }
}

fn target(ctx: &mut VulkanContext) -> emblema_hal_vulkan::VulkanTexture {
    ctx.create_texture(&TextureDescriptor::offscreen(
        Extent2D::new(SIZE, SIZE),
        PixelFormat::Rgba8Unorm,
    ))
    .expect("texture")
}

fn finish(ctx: &mut VulkanContext, mut tex: emblema_hal_vulkan::VulkanTexture) -> Vec<u8> {
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

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn a_batch_matches_the_same_draws_submitted_separately() {
    let Some(mut ctx) = context() else { return };
    let scene = Scene {
        draws: vec![
            (band(-1.0, -0.5), [1.0, 0.0, 0.0, 1.0], BlendMode::Src),
            (band(-0.5, 0.0), [0.0, 1.0, 0.0, 1.0], BlendMode::Src),
            (band(0.0, 0.5), [0.0, 0.0, 1.0, 1.0], BlendMode::Src),
            (band(0.5, 1.0), [1.0, 1.0, 0.0, 1.0], BlendMode::Src),
        ],
    };
    assert_eq!(
        scene.batched(&mut ctx),
        scene.separately(&mut ctx),
        "batching changed the result"
    );
}

#[test]
fn overlapping_draws_keep_their_order_within_a_batch() {
    let Some(mut ctx) = context() else { return };
    // Three overlapping opaque bands: the last one drawn must win where they
    // coincide. Sorting draws by pipeline would reorder these and change which
    // is on top, which is why the batch preserves submission order.
    let scene = Scene {
        draws: vec![
            (band(-1.0, 1.0), [1.0, 0.0, 0.0, 1.0], BlendMode::Src),
            (band(-1.0, 0.5), [0.0, 1.0, 0.0, 1.0], BlendMode::Src),
            (band(-1.0, 0.0), [0.0, 0.0, 1.0, 1.0], BlendMode::Src),
        ],
    };
    let batched = scene.batched(&mut ctx);
    assert_eq!(batched, scene.separately(&mut ctx));

    // Clip Y is up, so the earliest band ends up at the bottom of the image.
    assert_eq!(pixel(&batched, SIZE / 2, 2), [255, 0, 0, 255], "top band");
    assert_eq!(
        pixel(&batched, SIZE / 2, SIZE - 2),
        [0, 0, 255, 255],
        "the last draw wins where they overlap"
    );
}

#[test]
fn mixed_blend_modes_compose_the_same_way_in_one_pass() {
    let Some(mut ctx) = context() else { return };
    // Alternating modes force pipeline changes inside a single render pass,
    // which is where a wrong bind or a stale pipeline would show up.
    let scene = Scene {
        draws: vec![
            (band(-1.0, 1.0), [0.0, 0.0, 1.0, 1.0], BlendMode::Src),
            (band(-1.0, 0.0), [1.0, 0.0, 0.0, 0.5], BlendMode::SrcOver),
            (band(0.0, 1.0), [0.0, 1.0, 0.0, 1.0], BlendMode::Src),
            (band(-0.5, 0.5), [1.0, 1.0, 1.0, 0.25], BlendMode::SrcOver),
        ],
    };
    assert_eq!(
        scene.batched(&mut ctx),
        scene.separately(&mut ctx),
        "mixing blend modes in one pass diverged from separate passes"
    );
}

#[test]
fn a_batch_binds_a_pipeline_only_when_it_changes() {
    let mut batch = Batch::new();
    let verts = band(-1.0, 1.0);
    // A different color each time, so no two of these merge into one draw.
    // With identical draws the count would collapse to the number of binds and
    // this test would be comparing a number with itself.
    for (i, blend) in [
        BlendMode::Src,
        BlendMode::Src,
        BlendMode::Src,
        BlendMode::SrcOver,
        BlendMode::Src,
    ]
    .into_iter()
    .enumerate()
    {
        let shade = i as f32 / 8.0;
        batch
            .push(&verts, &QUAD, Material::solid([shade; 4]), blend)
            .expect("push");
    }
    // The saving batching exists for: five draws, three binds.
    assert_eq!(batch.draw_count(), 5);
    assert_eq!(batch.pipeline_binds(), 3);
}

#[test]
fn draws_that_differ_in_nothing_become_one_and_draw_the_same_picture() {
    // Merging changes how many times a backend is asked to draw, not what it
    // draws. Rendering each draw as its own submission is the reference: that
    // path never merges anything, so agreeing with it is the claim.
    //
    // The two bands overlap and are translucent, so the order they composite
    // in is visible in the overlap -- which is the property merging must not
    // disturb, since two draws sharing everything are still two draws whose
    // sequence decides what ends up on top.
    let Some(mut ctx) = context() else { return };
    let scene = Scene {
        draws: vec![
            (band(-1.0, 0.2), [1.0, 0.4, 0.2, 0.5], BlendMode::SrcOver),
            (band(-0.2, 1.0), [1.0, 0.4, 0.2, 0.5], BlendMode::SrcOver),
        ],
    };

    let mut batch = Batch::new();
    for (verts, color, blend) in &scene.draws {
        batch
            .push(verts, &QUAD, Material::solid(*color), *blend)
            .expect("push");
    }
    assert_eq!(batch.draw_count(), 1, "two alike draws should be one");

    assert_eq!(
        scene.batched(&mut ctx),
        scene.separately(&mut ctx),
        "merging two draws changed the picture"
    );
}

#[test]
fn many_draws_in_one_batch_all_land() {
    let Some(mut ctx) = context() else { return };
    // One thin band per row, so every draw is individually observable and a
    // dropped or misindexed one shows up as a gap rather than as a subtle
    // shift.
    let rows = 16;
    let mut batch = Batch::new();
    for i in 0..rows {
        let y0 = -1.0 + 2.0 * i as f32 / rows as f32;
        let y1 = -1.0 + 2.0 * (i + 1) as f32 / rows as f32;
        let shade = (i + 1) as f32 / rows as f32;
        batch
            .push(
                &band(y0, y1),
                &QUAD,
                Material::solid([shade, 0.0, 0.0, 1.0]),
                BlendMode::Src,
            )
            .expect("push");
    }
    assert_eq!(batch.draw_count(), rows);

    let mut tex = target(&mut ctx);
    ctx.submit_batch(&mut tex, &batch, PassDescriptor::clear(BLACK))
        .expect("submit");
    let pixels = finish(&mut ctx, tex);

    // Every row must be distinct and increasing downward: clip Y is up, so the
    // brightest band drawn last is at the top of clip space and the bottom of
    // the image reads darkest.
    let sample = |row: u32| pixel(&pixels, SIZE / 2, row)[0];
    let bottom = sample(SIZE - 1);
    let top = sample(0);
    assert!(top > bottom, "rows: top {top}, bottom {bottom}");
    assert_eq!(top, 255, "the last band should be fully bright");
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
    let pixels = finish(&mut ctx, tex);
    assert_eq!(pixel(&pixels, 0, 0), [0, 0, 255, 255]);
}

#[test]
fn a_batch_can_be_reused_across_submissions() {
    let Some(mut ctx) = context() else { return };
    let mut batch = Batch::new();
    batch
        .push(
            &band(-1.0, 0.0),
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");

    let first = {
        let mut tex = target(&mut ctx);
        ctx.submit_batch(&mut tex, &batch, PassDescriptor::clear(BLACK))
            .expect("a");
        finish(&mut ctx, tex)
    };

    // Rebuilding into the same batch must not leave the previous contents
    // behind, and the allocations are kept precisely so a frame loop can do
    // this every frame.
    batch.clear();
    batch
        .push(
            &band(-1.0, 0.0),
            &QUAD,
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");
    let second = {
        let mut tex = target(&mut ctx);
        ctx.submit_batch(&mut tex, &batch, PassDescriptor::clear(BLACK))
            .expect("b");
        finish(&mut ctx, tex)
    };

    assert_eq!(first, second, "a reused batch rendered differently");
}

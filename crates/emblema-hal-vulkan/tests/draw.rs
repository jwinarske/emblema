//! Drawing geometry and checking where it landed.
//!
//! A triangle that renders is easy to confirm by eye and easy to get subtly
//! wrong in ways a glance misses: mirrored, transposed, or off by a half
//! pixel. These tests assert on specific pixel coordinates so orientation
//! errors cannot pass.

use emblema_hal::{BlendMode, Extent2D, Material, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};

const SIZE: u32 = 64;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn context() -> Option<VulkanContext> {
    // Validation on: a draw touches render passes, framebuffers, pipelines and
    // layouts, which is where the layer earns the most.
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

struct Image {
    pixels: Vec<u8>,
    width: u32,
}

impl Image {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    fn is_red(&self, x: u32, y: u32) -> bool {
        self.at(x, y) == [255, 0, 0, 255]
    }

    fn is_clear(&self, x: u32, y: u32) -> bool {
        self.at(x, y) == [0, 0, 0, 255]
    }

    fn red_count(&self) -> usize {
        self.pixels.chunks_exact(4).filter(|p| p[0] > 128).count()
    }
}

fn draw(ctx: &mut VulkanContext, vertices: &[[f32; 2]], indices: &[u32]) -> Image {
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(SIZE, SIZE),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    ctx.draw_indexed(
        &mut tex,
        vertices,
        indices,
        Material::solid(RED),
        BlendMode::Src,
        Some(CLEAR),
    )
    .expect("draw");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);
    Image {
        pixels,
        width: SIZE,
    }
}

fn assert_validation_clean(ctx: &VulkanContext) {
    if !ctx.validation_active() {
        return;
    }
    let errors = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .map(|m| format!("[{}] {}", m.id, m.message))
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "validation errors:\n{}",
        errors.join("\n")
    );
}

#[test]
fn a_full_target_quad_covers_every_pixel() {
    let Some(mut ctx) = context() else { return };
    // Two triangles spanning the whole clip volume.
    let verts = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    let img = draw(&mut ctx, &verts, &[0, 1, 2, 0, 2, 3]);

    assert_eq!(
        img.red_count(),
        (SIZE * SIZE) as usize,
        "a full-clip quad must leave no pixel unwritten"
    );
    assert_validation_clean(&ctx);
}

#[test]
fn clip_space_follows_the_wgsl_convention_with_y_up() {
    let Some(mut ctx) = context() else { return };
    // A band over clip Y in [-1, 0]. In the WGSL convention that is the lower
    // half, so it must land at the bottom of the image. Under Vulkan's raw
    // Y-down convention it would land at the top.
    //
    // This pins the coordinate normalization naga performs. Turning that off
    // would mirror every shader vertically on Vulkan while leaving WebGPU
    // unchanged, which is the exact cross-backend divergence a single source
    // tree exists to prevent.
    let verts = [[-1.0, -1.0], [1.0, -1.0], [1.0, 0.0], [-1.0, 0.0]];
    let img = draw(&mut ctx, &verts, &[0, 1, 2, 0, 2, 3]);

    assert!(
        img.is_red(SIZE / 2, SIZE - 4),
        "clip Y of -1 should be the bottom of the image under the WGSL convention"
    );
    assert!(
        img.is_clear(SIZE / 2, 4),
        "top of the image should be untouched; coordinate space is not being adjusted"
    );
    assert_validation_clean(&ctx);
}

#[test]
fn geometry_is_not_mirrored_horizontally() {
    let Some(mut ctx) = context() else { return };
    // A band down the left half of clip space.
    let verts = [[-1.0, -1.0], [0.0, -1.0], [0.0, 1.0], [-1.0, 1.0]];
    let img = draw(&mut ctx, &verts, &[0, 1, 2, 0, 2, 3]);

    assert!(
        img.is_red(4, SIZE / 2),
        "left of the image should be covered"
    );
    assert!(
        img.is_clear(SIZE - 4, SIZE / 2),
        "right of the image should be untouched; geometry is flipped in X"
    );
    assert_validation_clean(&ctx);
}

#[test]
fn a_triangle_covers_about_half_the_quad_it_is_cut_from() {
    let Some(mut ctx) = context() else { return };
    let verts = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    let quad = draw(&mut ctx, &verts, &[0, 1, 2, 0, 2, 3]).red_count();
    let tri = draw(&mut ctx, &verts, &[0, 1, 2]).red_count();

    // Half the quad, within a few percent for the diagonal's rasterization.
    let ratio = tri as f32 / quad as f32;
    assert!(
        (ratio - 0.5).abs() < 0.05,
        "one triangle of the pair covered {ratio} of the quad"
    );
    assert_validation_clean(&ctx);
}

#[test]
fn triangle_winding_does_not_affect_coverage() {
    let Some(mut ctx) = context() else { return };
    let verts = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0]];
    let ccw = draw(&mut ctx, &verts, &[0, 1, 2]).red_count();
    let cw = draw(&mut ctx, &verts, &[0, 2, 1]).red_count();

    // Tessellation emits whatever winding the input produced, so culling by
    // winding would silently drop half of a real scene's triangles.
    assert_eq!(ccw, cw, "reversing winding changed coverage");
    assert!(ccw > 0);
    assert_validation_clean(&ctx);
}

#[test]
fn an_empty_index_buffer_clears_without_drawing() {
    let Some(mut ctx) = context() else { return };
    let img = draw(&mut ctx, &[[0.0, 0.0]], &[]);
    assert_eq!(img.red_count(), 0);
    assert!(img.is_clear(0, 0));
    assert!(img.is_clear(SIZE - 1, SIZE - 1));
    assert_validation_clean(&ctx);
}

#[test]
fn an_out_of_range_index_is_refused_before_reaching_the_gpu() {
    let Some(mut ctx) = context() else { return };
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(16, 16),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");

    // Out-of-range indices are an out-of-bounds read on the GPU, which is
    // undefined rather than merely wrong, so it must be caught on the CPU.
    let result = ctx.draw_indexed(
        &mut tex,
        &[[0.0, 0.0]],
        &[0, 1, 2],
        Material::solid(RED),
        BlendMode::Src,
        Some(CLEAR),
    );
    assert!(result.is_err());

    // A partial triangle would leave the draw reading past the buffer.
    let result = ctx.draw_indexed(
        &mut tex,
        &[[0.0, 0.0]],
        &[0, 0],
        Material::solid(RED),
        BlendMode::Src,
        Some(CLEAR),
    );
    assert!(result.is_err());

    ctx.destroy_texture(tex);
    assert_validation_clean(&ctx);
}

#[test]
fn repeated_draws_reuse_the_cached_pipeline() {
    let Some(mut ctx) = context() else { return };
    let verts = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0]];
    // Pipeline creation costs milliseconds, so it is cached per target format.
    // Repeating the draw exercises reuse and would surface a double-create or
    // a use-after-destroy.
    let first = draw(&mut ctx, &verts, &[0, 1, 2]).red_count();
    for _ in 0..5 {
        assert_eq!(draw(&mut ctx, &verts, &[0, 1, 2]).red_count(), first);
    }
    assert_validation_clean(&ctx);
}

#[test]
fn the_software_reference_rasterizes_identically() {
    let Some(mut default_ctx) = context() else {
        return;
    };
    let Ok(mut sw) = Validated::new(DevicePreference::Software) else {
        eprintln!("skipping: no software rasterizer");
        return;
    };

    let verts = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0]];
    let a = draw(&mut default_ctx, &verts, &[0, 1, 2]);
    let b = draw(&mut sw, &verts, &[0, 1, 2]);

    // Rasterization rules are specified exactly, so two conformant
    // implementations must agree on which pixels a triangle covers. This is
    // the property that makes a software device usable as the oracle.
    assert_eq!(
        a.pixels, b.pixels,
        "hardware and software rasterization disagree on triangle coverage"
    );
}

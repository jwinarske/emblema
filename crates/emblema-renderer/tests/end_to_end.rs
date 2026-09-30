//! From a path in user coordinates to the pixels it covers.
//!
//! Every earlier test checked one link: tessellation produced sensible
//! triangles, or a backend drew triangles where it was told. Neither catches a
//! wrong coordinate mapping between them, because each half is self-consistent
//! about a convention the other does not share. These tests assert on device
//! pixels given user-space input, which is the only place that error shows up
//! and the shape every golden scene will take.

use emblema_geometry::stroke::{LineCap, StrokeStyle};
use emblema_geometry::{Path, PathBuilder};
use emblema_hal::{BlendMode, Extent2D, Material, PixelFormat, TextureDescriptor};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext};
use emblema_renderer::{Renderer, TOLERANCE};
use glam::{Affine2, Vec2};

const SIZE: u32 = 64;
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const TARGET: Extent2D = Extent2D {
    width: SIZE,
    height: SIZE,
};

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

struct Image {
    pixels: Vec<u8>,
    extent: Extent2D,
}

impl Image {
    fn covered(&self, x: u32, y: u32) -> bool {
        self.pixels[((y * self.extent.width + x) * 4) as usize] > 128
    }

    fn count(&self) -> usize {
        self.pixels.chunks_exact(4).filter(|p| p[0] > 128).count()
    }

    /// Inclusive bounds of covered pixels, or None if nothing was drawn.
    fn bounds(&self) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        let mut any = false;
        for y in 0..self.extent.height {
            for x in 0..self.extent.width {
                if self.covered(x, y) {
                    any = true;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        any.then_some((x0, y0, x1, y1))
    }
}

fn render(ctx: &mut VulkanContext, positions: &[[f32; 3]], indices: &[u32]) -> Image {
    render_sized(ctx, positions, indices, TARGET)
}

/// Homogeneous positions, because that is what the renderer now produces.
///
/// Built into a batch here rather than through `draw_indexed`, which is the
/// convenience for hand-written quads whose divisor is one. Carrying the
/// divisor through means these tests exercise the path a real frame takes
/// rather than a narrowed copy of it.
fn render_sized(
    ctx: &mut VulkanContext,
    positions: &[[f32; 3]],
    indices: &[u32],
    extent: Extent2D,
) -> Image {
    let mut tex = ctx
        .create_texture(&TextureDescriptor::offscreen(
            extent,
            PixelFormat::Rgba8Unorm,
        ))
        .expect("texture");
    let vertices: Vec<emblema_hal::Vertex> = positions
        .iter()
        .map(|p| emblema_hal::Vertex::at_projected(*p))
        .collect();
    let mut batch = emblema_hal::Batch::new();
    batch
        .push_mesh(
            &vertices,
            indices,
            Material::solid(RED),
            emblema_hal::ColorFilter::None,
            BlendMode::Src,
            None,
            emblema_hal::ClipState::UNCLIPPED,
        )
        .expect("push");
    ctx.submit_batch(
        &mut tex,
        &batch,
        emblema_hal::PassDescriptor {
            clear: Some([0.0, 0.0, 0.0, 1.0]),
            samples: 1,
            viewport: None,
        },
    )
    .expect("draw");
    let pixels = ctx.read_texture(&mut tex).expect("readback");
    ctx.destroy_texture(tex);

    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");

    Image { pixels, extent }
}

fn fill(ctx: &mut VulkanContext, path: &Path, transform: Affine2) -> Image {
    let mut r = Renderer::new();
    r.begin_frame(TARGET, TOLERANCE);
    let geo = r.fill_path(path, transform);
    let positions = geo.positions();
    let indices = geo.indices.to_vec();
    render(ctx, &positions, &indices)
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Path {
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(x0, y0))
        .line_to(Vec2::new(x1, y0))
        .line_to(Vec2::new(x1, y1))
        .line_to(Vec2::new(x0, y1))
        .close();
    b.build()
}

#[test]
fn a_rect_covers_exactly_the_pixels_it_names() {
    let Some(mut ctx) = context() else { return };
    let img = fill(&mut ctx, &rect(16.0, 16.0, 48.0, 48.0), Affine2::IDENTITY);

    // Pixel centers sit at x+0.5, so a rect spanning [16, 48) covers columns 16
    // through 47 inclusive: 32 columns, not 33. An off-by-one in the mapping
    // shows up here as a shifted or fattened bound.
    assert_eq!(img.bounds(), Some((16, 16, 47, 47)));
    assert_eq!(img.count(), 32 * 32);
}

#[test]
fn user_space_y_runs_downward() {
    let Some(mut ctx) = context() else { return };
    // A band across the top of user space must appear at the top of the image.
    // This is the assertion that fails if the clip-space Y flip is dropped, and
    // neither half of the stack can catch it alone.
    let img = fill(&mut ctx, &rect(0.0, 0.0, 64.0, 16.0), Affine2::IDENTITY);

    assert_eq!(img.bounds(), Some((0, 0, 63, 15)));
    assert!(img.covered(32, 2), "top of the image should be covered");
    assert!(!img.covered(32, 60), "bottom should be untouched");
}

#[test]
fn user_space_x_runs_rightward() {
    let Some(mut ctx) = context() else { return };
    let img = fill(&mut ctx, &rect(0.0, 0.0, 16.0, 64.0), Affine2::IDENTITY);
    assert_eq!(img.bounds(), Some((0, 0, 15, 63)));
}

#[test]
fn translation_moves_geometry_by_the_pixels_requested() {
    let Some(mut ctx) = context() else { return };
    let path = rect(0.0, 0.0, 16.0, 16.0);
    let moved = fill(
        &mut ctx,
        &path,
        Affine2::from_translation(Vec2::new(20.0, 30.0)),
    );
    // Translating by (20, 30) device pixels must move the bounds by exactly
    // that, in the same direction user space runs.
    assert_eq!(moved.bounds(), Some((20, 30, 35, 45)));
}

#[test]
fn scaling_grows_geometry_about_the_origin() {
    let Some(mut ctx) = context() else { return };
    let path = rect(0.0, 0.0, 8.0, 8.0);
    let plain = fill(&mut ctx, &path, Affine2::IDENTITY);
    let scaled = fill(&mut ctx, &path, Affine2::from_scale(Vec2::splat(4.0)));

    assert_eq!(plain.bounds(), Some((0, 0, 7, 7)));
    assert_eq!(scaled.bounds(), Some((0, 0, 31, 31)));
    // Four times the extent is sixteen times the area.
    assert_eq!(scaled.count(), plain.count() * 16);
}

#[test]
fn a_non_square_target_does_not_distort_geometry() {
    let Some(mut ctx) = context() else { return };
    // Deliberately not the square target the other tests use: the projection
    // scales each axis by that axis' size, and a single shared scale would
    // only show up as stretching when the two differ.
    let extent = Extent2D::new(96, 48);
    let mut r = Renderer::new();
    r.begin_frame(extent, TOLERANCE);
    let geo = r.fill_path(&rect(8.0, 8.0, 40.0, 40.0), Affine2::IDENTITY);
    let positions = geo.positions();
    let indices = geo.indices.to_vec();
    let img = render_sized(&mut ctx, &positions, &indices, extent);

    let (x0, y0, x1, y1) = img.bounds().expect("geometry");
    assert_eq!(
        x1 - x0,
        y1 - y0,
        "a square in user space rendered as a rectangle on a {}x{} target",
        extent.width,
        extent.height
    );
    assert_eq!((x0, y0, x1, y1), (8, 8, 39, 39));
}

#[test]
fn a_stroked_line_covers_a_band_of_the_requested_width() {
    let Some(mut ctx) = context() else { return };
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(0.0, 32.0))
        .line_to(Vec2::new(64.0, 32.0));
    let path = b.build();

    let mut r = Renderer::new();
    r.begin_frame(TARGET, TOLERANCE);
    let style = StrokeStyle::new(8.0).with_cap(LineCap::Butt);
    let geo = r.stroke_path(&path, &style, None, Affine2::IDENTITY);
    let positions = geo.positions();
    let indices = geo.indices.to_vec();
    let img = render(&mut ctx, &positions, &indices);

    // Width 8 centered on y=32 spans [28, 36), so rows 28 through 35.
    assert_eq!(img.bounds(), Some((0, 28, 63, 35)));
    assert_eq!(img.count(), 64 * 8);
}

#[test]
fn a_curved_path_is_smooth_rather_than_faceted_after_scaling() {
    let Some(mut ctx) = context() else { return };
    // A quarter disc authored small and scaled up. If tolerance were applied
    // in path space without accounting for the transform, the arc would be
    // visibly faceted; the coverage count is what changes when it is.
    let mut b = PathBuilder::new();
    b.move_to(Vec2::new(0.0, 0.0))
        .line_to(Vec2::new(8.0, 0.0))
        .cubic_to(
            Vec2::new(8.0, 4.4),
            Vec2::new(4.4, 8.0),
            Vec2::new(0.0, 8.0),
        )
        .close();
    let path = b.build();

    let scaled = fill(&mut ctx, &path, Affine2::from_scale(Vec2::splat(8.0)));
    let area = scaled.count() as f32;
    // A quarter disc of radius 64 is about pi/4 * 64^2.
    let expected = std::f32::consts::FRAC_PI_4 * 64.0 * 64.0;
    assert!(
        (area - expected).abs() / expected < 0.02,
        "covered {area}, expected about {expected}"
    );
}

#[test]
fn geometry_entirely_outside_the_target_draws_nothing() {
    let Some(mut ctx) = context() else { return };
    let img = fill(
        &mut ctx,
        &rect(0.0, 0.0, 16.0, 16.0),
        Affine2::from_translation(Vec2::new(500.0, 500.0)),
    );
    // Clipping is the rasterizer's job here, but a mapping that wrapped
    // coordinates would smear geometry back into view.
    assert_eq!(img.count(), 0);
}

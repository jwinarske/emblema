//! Texture upload and sampling, across both backends.
//!
//! The image paint is the first thing here that reads a resource rather than
//! only writing one, and the two backends bind it in quite different ways — a
//! descriptor set against a separate image and sampler on one, a texture unit
//! and a sampler uniform on the other. What that risks is a picture that is
//! plausible on each and different between them, so most of these compare the
//! two rather than only checking each against an expectation.

use emblema_hal::{
    Batch, BlendMode, Capability, ClipState, Extent2D, Hal, HalContext, Material, PassDescriptor,
    PixelFormat, Sampling, TextureDescriptor, TileMode, Vertex,
};
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesHal};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};

/// Clip space to a texture spanning the whole target, as the shader reads it.
///
/// `scale` is half the target's size over the texture's: one where a texture
/// covers the target at its own size, a half where it covers twice that. The
/// paint's origin is inside the matrix rather than packed beside it, so this is
/// the whole mapping.
fn across_the_target(scale: f32) -> [f32; 12] {
    [
        scale, 0.0, 0.0, 0.0, //
        0.0, -scale, 0.0, 0.0, //
        scale, scale, 1.0, 0.0,
    ]
}

const SIZE: Extent2D = Extent2D {
    width: 32,
    height: 32,
};
const SOURCE: Extent2D = Extent2D {
    width: 4,
    height: 4,
};
const FULL: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// A four-by-four image with a different color in each corner.
///
/// Asymmetric in both axes and in all four quadrants, so a flip, a transpose,
/// or a mirrored sampling coordinate each produce a distinguishable result.
/// Fully opaque, so a comparison is about the mapping rather than about alpha.
fn source_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; (SOURCE.area() * 4) as usize];
    for y in 0..SOURCE.height {
        for x in 0..SOURCE.width {
            let i = ((y * SOURCE.width + x) * 4) as usize;
            let left = x < SOURCE.width / 2;
            let top = y < SOURCE.height / 2;
            let color: [u8; 4] = match (left, top) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 0, 255],
            };
            pixels[i..i + 4].copy_from_slice(&color);
        }
    }
    pixels
}

/// Map the whole target to the whole image.
///
/// Clip space runs from minus one to one with Y upward, and texture
/// coordinates from zero to one with V downward, so the V axis is negated. The
/// origin is the top-left corner of the target in clip space, which is
/// `(-1, 1)`.
fn full_target_mapping() -> Material {
    Material::Image {
        to_local: across_the_target(0.5),
        slot: 0,
        alpha: 1.0,
        tile: TileMode::Clamp,
        sampling: emblema_hal::Sampling::Linear,
        source: [0.0, 0.0, 1.0, 1.0],
        tint: [1.0, 1.0, 1.0, 1.0],
    }
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Upload the source image, then draw it over the whole target.
fn render<H: Hal>(ctx: &mut H::Context, material: Material) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    let mut source = ctx
        .create_texture(&TextureDescriptor::offscreen(
            SOURCE,
            PixelFormat::Rgba8Unorm,
        ))
        .expect("source texture");
    ctx.write_texture(&mut source, &source_pixels())
        .expect("upload");

    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, material, BlendMode::Src)
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        &[&source],
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    ctx.destroy_texture(source);
    pixels
}

#[test]
fn an_upload_survives_a_round_trip_through_readback() {
    // The inverse of readback, in the same layout, so this is the identity.
    // Everything below rests on it: a sampled image that came back wrong would
    // otherwise be as likely an upload fault as a sampling one.
    let want = source_pixels();
    let mut ran = 0;

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba8Unorm,
            ))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want, "vulkan changed the pixels in transit");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba8Unorm,
            ))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want, "gles changed the pixels in transit");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

/// Check the four quadrants land where the source put them.
/// Texture coordinates for [`FULL`], in the same order.
///
/// Chosen to reproduce exactly what the image material's mapping produces, so
/// a mesh and an image drawing the same picture is the assertion: the two take
/// different routes to a coordinate -- one computed from the fragment's
/// position, one interpolated from the vertices -- and a disagreement between
/// them is the mistake worth catching.
const FULL_UV: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

fn render_mesh<H: Hal>(ctx: &mut H::Context, material: Material) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    let mut source = ctx
        .create_texture(&TextureDescriptor::offscreen(
            SOURCE,
            PixelFormat::Rgba8Unorm,
        ))
        .expect("source texture");
    ctx.write_texture(&mut source, &source_pixels())
        .expect("upload");

    let vertices: Vec<Vertex> = FULL
        .iter()
        .zip(FULL_UV.iter())
        .map(|(p, uv)| Vertex::new(*p, *uv))
        .collect();
    let mut batch = Batch::new();
    batch
        .push_mesh(
            &vertices,
            &QUAD,
            material,
            emblema_hal::ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::default(),
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        &[&source],
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    ctx.destroy_texture(source);
    pixels
}

#[test]
fn a_mesh_reads_its_own_coordinates_the_same_way_on_both_backends() {
    // The mesh material is the one path where a texture coordinate reaches the
    // shader as a vertex attribute rather than as a computation, so it is the
    // one that can differ between a backend that interpolates it through a
    // descriptor-bound sampler and one that interpolates it through a texture
    // unit. Both must land the image the same way up, and the same way up as
    // the image material does -- which is what the shared assertion says.
    let mesh = || Material::Mesh {
        slot: 0,
        alpha: 1.0,
        tint: [1.0, 1.0, 1.0, 1.0],
        tile: TileMode::Clamp,
        sampling: emblema_hal::Sampling::Linear,
    };
    let mut ran = 0;
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let pixels = render_mesh::<VulkanHal>(&mut ctx, mesh());
        assert_quadrants(&pixels, "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let pixels = render_mesh::<GlesHal>(&mut ctx, mesh());
        assert_quadrants(&pixels, "gles");
        ran += 1;
    }
    if ran == 0 {
        eprintln!("skipping: no backend available");
    }
}

#[test]
fn a_mesh_tint_scales_the_texel_it_read() {
    // The mesh material carries a tint of its own rather than borrowing the
    // image material's, so it needs its own check that the tint reaches the
    // texel: a mesh drawing a sheet at half alpha is how a sprite batch fades.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let plain = render_mesh::<VulkanHal>(
        &mut ctx,
        Material::Mesh {
            slot: 0,
            alpha: 1.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            tile: TileMode::Clamp,
            sampling: emblema_hal::Sampling::Linear,
        },
    );
    let halved = render_mesh::<VulkanHal>(
        &mut ctx,
        Material::Mesh {
            slot: 0,
            alpha: 0.5,
            tint: [1.0, 1.0, 1.0, 1.0],
            tile: TileMode::Clamp,
            sampling: emblema_hal::Sampling::Linear,
        },
    );
    // Premultiplied, so alpha scales every channel and not only the fourth.
    // Sampled where the source is opaque red, which makes both the scaled
    // channel and the untouched one visible in the same pixel.
    let full = pixel(&plain, 4, 4);
    let half = pixel(&halved, 4, 4);
    assert_eq!(full, [255, 0, 0, 255], "the plain draw is not the source");
    for channel in 0..4 {
        let want = (full[channel] as f32 * 0.5).round() as i32;
        assert!(
            (half[channel] as i32 - want).abs() <= 1,
            "channel {channel} was not scaled by the alpha: {half:?} against {full:?}"
        );
    }
}

/// A corner-colored quad drawn with a plain white material.
///
/// No texture at all, which is the point: the vertex color is a fourth
/// attribute traveling through a vertex layout both backends declare
/// separately -- a descriptor-driven pipeline on one, `glVertexAttribPointer`
/// on the other -- so an offset or a stride wrong on one side shows up here
/// and nowhere else.
fn render_colored<H: Hal>(ctx: &mut H::Context) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    // Red, green, blue, and white, in the same corner order as `FULL`.
    const CORNERS: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
    ];
    let vertices: Vec<Vertex> = FULL
        .iter()
        .zip(CORNERS.iter())
        .map(|(p, c)| Vertex::at(*p).with_color(*c))
        .collect();
    let mut batch = Batch::new();
    batch
        .push_mesh(
            &vertices,
            &QUAD,
            Material::solid([1.0, 1.0, 1.0, 1.0]),
            emblema_hal::ColorFilter::None,
            BlendMode::Src,
            None,
            ClipState::default(),
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    pixels
}

#[test]
fn a_vertex_color_reaches_the_shader_the_same_way_on_both_backends() {
    let check = |pixels: &[u8], backend: &str| {
        // Well inside each corner, where its own color dominates. The corner
        // order is `FULL`'s, and the target's rows run the other way from clip
        // space, which is why the reds and greens are at the bottom here.
        for (x, y, channel, corner) in [
            (4u32, 27u32, 0usize, "first"),
            (27, 27, 1, "second"),
            (27, 4, 2, "third"),
        ] {
            let got = pixel(pixels, x, y);
            assert!(
                got[channel] > 150,
                "{backend}: the {corner} corner should be dominated by its own \
                 channel, got {got:?}"
            );
        }
    };

    let mut ran = 0;
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        check(&render_colored::<VulkanHal>(&mut ctx), "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        check(&render_colored::<GlesHal>(&mut ctx), "gles");
        ran += 1;
    }
    if ran == 0 {
        eprintln!("skipping: no backend available");
    }
}

#[test]
fn nearest_sampling_steps_between_texels_on_both_backends() {
    // Nearest is a coordinate snapped to a texel center rather than a second
    // sampler, and the snap needs the texture's size -- which the two
    // translations reach differently. So this is a place the two can disagree
    // while each looks plausible on its own, which is what this file is for.
    let quadrant = |sampling| Material::Image {
        to_local: across_the_target(1.0),
        slot: 0,
        alpha: 1.0,
        tile: TileMode::Clamp,
        sampling,
        source: [0.0, 0.0, 1.0, 1.0],
        tint: [1.0, 1.0, 1.0, 1.0],
    };

    let mut ran = 0;
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let linear = render::<VulkanHal>(&mut ctx, quadrant(Sampling::Linear));
        let nearest = render::<VulkanHal>(&mut ctx, quadrant(Sampling::Nearest));
        assert_steps(&linear, &nearest, "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let linear = render::<GlesHal>(&mut ctx, quadrant(Sampling::Linear));
        let nearest = render::<GlesHal>(&mut ctx, quadrant(Sampling::Nearest));
        assert_steps(&linear, &nearest, "gles");
        ran += 1;
    }
    if ran == 0 {
        eprintln!("skipping: no backend available");
    }
}

/// The two modes agree away from a boundary and differ across one.
fn assert_steps(linear: &[u8], nearest: &[u8], backend: &str) {
    // Well inside a quadrant, where there is nothing between texels to choose
    // between.
    for (x, y) in [(4u32, 4u32), (27, 27)] {
        assert_eq!(
            pixel(linear, x, y),
            pixel(nearest, x, y),
            "{backend}: the two modes should agree away from a boundary at ({x}, {y})"
        );
    }

    // What nearest means, stated exactly rather than by picking a boundary and
    // arguing about where the mapping puts it: every pixel is a texel of the
    // source, so the whole target is made of the source's own four colors and
    // nothing between them. Linear must produce something between them
    // somewhere, or it would not be blending at all.
    const PALETTE: [[u8; 4]; 4] = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
    ];
    let between = |pixels: &[u8]| {
        (0..SIZE.height)
            .flat_map(|y| (0..SIZE.width).map(move |x| (x, y)))
            .filter(|(x, y)| !PALETTE.contains(&pixel(pixels, *x, *y)))
            .count()
    };
    assert_eq!(
        between(nearest),
        0,
        "{backend}: nearest produced a color the source does not contain"
    );
    assert!(
        between(linear) > 0,
        "{backend}: linear blended nothing anywhere, so the two modes are the same"
    );
}

fn assert_quadrants(pixels: &[u8], backend: &str) {
    // Sampled well inside each quadrant, so a filter kernel at the boundary
    // does not blend two of them together.
    for (x, y, want, corner) in [
        (4u32, 4u32, [255u8, 0, 0, 255], "top-left"),
        (27, 4, [0, 255, 0, 255], "top-right"),
        (4, 27, [0, 0, 255, 255], "bottom-left"),
        (27, 27, [255, 255, 0, 255], "bottom-right"),
    ] {
        assert_eq!(
            pixel(pixels, x, y),
            want,
            "{backend}: the {corner} quadrant sampled the wrong part of the image"
        );
    }
}

#[test]
fn an_image_lands_the_right_way_up_on_both_backends() {
    let mut ran = 0;
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let pixels = render::<VulkanHal>(&mut ctx, full_target_mapping());
        assert_quadrants(&pixels, "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let pixels = render::<GlesHal>(&mut ctx, full_target_mapping());
        assert_quadrants(&pixels, "gles");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn the_backends_agree_on_a_sampled_image() {
    let Ok(mut vulkan) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        return;
    };
    let a = render::<VulkanHal>(&mut vulkan, full_target_mapping());
    let b = render::<GlesHal>(&mut gles, full_target_mapping());

    // One unit, because a filtered sample is arithmetic and the two need not
    // round it identically. The quadrant centers above are exact regardless,
    // being far from any boundary the filter would blend across.
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (*x as i32 - *y as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(worst <= 1, "the backends differ by up to {worst}");
}

#[test]
fn alpha_scales_the_sampled_color() {
    let mut ran = 0;
    for backend in ["vulkan", "gles"] {
        let mut material = full_target_mapping();
        if let Material::Image { alpha, .. } = &mut material {
            *alpha = 0.5;
        }
        let pixels = match backend {
            "vulkan" => match Validated::new(DevicePreference::Auto) {
                Ok(mut ctx) => render::<VulkanHal>(&mut ctx, material),
                Err(_) => continue,
            },
            _ => match GlesValidated::new(DisplayTarget::Surfaceless) {
                Ok(mut ctx) => render::<GlesHal>(&mut ctx, material),
                Err(_) => continue,
            },
        };
        // Premultiplied on the way out, so a half-alpha red is half red and
        // half alpha rather than full red with a reduced alpha.
        let got = pixel(&pixels, 4, 4);
        for (channel, want) in got.iter().zip([128u8, 0, 0, 128]) {
            assert!(
                (*channel as i32 - want as i32).abs() <= 1,
                "{backend}: half-alpha red came back {got:?}"
            );
        }
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_source_rectangle_draws_only_that_part_of_the_image() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    // The source image is four colored quadrants, so selecting one and drawing
    // it across the whole target should give one flat color -- and a different
    // one per quadrant, which is what says the rectangle is being read rather
    // than ignored.
    let quadrant = |source: [f32; 4]| Material::Image {
        to_local: across_the_target(1.0),
        slot: 0,
        alpha: 1.0,
        tile: TileMode::Clamp,
        sampling: emblema_hal::Sampling::Linear,
        source,
        tint: [1.0, 1.0, 1.0, 1.0],
    };

    for (name, source, want) in [
        ("top left", [0.0, 0.0, 0.5, 0.5], [255u8, 0, 0, 255]),
        ("top right", [0.5, 0.0, 1.0, 0.5], [0, 255, 0, 255]),
        ("bottom left", [0.0, 0.5, 0.5, 1.0], [0, 0, 255, 255]),
        ("bottom right", [0.5, 0.5, 1.0, 1.0], [255, 255, 0, 255]),
    ] {
        let pixels = render::<VulkanHal>(&mut ctx, quadrant(source));
        // Away from the edges, where a linear filter would blend a neighbor.
        for (x, y) in [(8, 8), (24, 8), (8, 24), (24, 24)] {
            assert_eq!(
                pixel(&pixels, x, y),
                want,
                "{name} at ({x}, {y}) is not the quadrant that was asked for"
            );
        }
    }

    // And the whole image is still the default, so nothing that never mentions
    // a source rectangle changed.
    let whole = render::<VulkanHal>(&mut ctx, quadrant([0.0, 0.0, 1.0, 1.0]));
    assert_quadrants(&whole, "vulkan");
}

#[test]
fn a_repeated_source_rectangle_tiles_the_piece_rather_than_the_sheet() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    // The interaction worth pinning. Tiling happens in the destination's own
    // space and the source rectangle maps what comes out of it, so a repeat
    // repeats the selected piece. Mapping first and tiling afterwards would
    // wrap across the whole sheet and draw the neighbors, which is the obvious
    // way to write this and the wrong one.
    //
    // The image is mapped to a quarter of the target, so there are four
    // repetitions across it, and the source is the top-left quadrant alone.
    let pixels = render::<VulkanHal>(
        &mut ctx,
        Material::Image {
            to_local: across_the_target(1.0),
            slot: 0,
            alpha: 1.0,
            tile: TileMode::Repeat,
            sampling: emblema_hal::Sampling::Linear,
            source: [0.0, 0.0, 0.5, 0.5],
            tint: [1.0, 1.0, 1.0, 1.0],
        },
    );
    // Every repetition is that one quadrant, so the whole target is its color.
    for (x, y) in [(4, 4), (20, 4), (4, 20), (20, 20), (28, 28)] {
        assert_eq!(
            pixel(&pixels, x, y),
            [255, 0, 0, 255],
            "({x}, {y}) is not the repeated quadrant, so the sheet was tiled"
        );
    }
}

#[test]
fn the_tile_modes_differ_outside_the_image() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    // Map the image to the top-left quarter of the target, so three quarters of
    // it lie outside and the modes have somewhere to disagree.
    let quarter = |tile| Material::Image {
        to_local: across_the_target(1.0),
        slot: 0,
        alpha: 1.0,
        tile,
        sampling: emblema_hal::Sampling::Linear,
        source: [0.0, 0.0, 1.0, 1.0],
        tint: [1.0, 1.0, 1.0, 1.0],
    };

    let clamp = render::<VulkanHal>(&mut ctx, quarter(TileMode::Clamp));
    let repeat = render::<VulkanHal>(&mut ctx, quarter(TileMode::Repeat));
    let decal = render::<VulkanHal>(&mut ctx, quarter(TileMode::Decal));

    // Inside the image, all three agree: the modes describe what happens
    // outside it, and a mode that changed the interior would be wrong.
    for mode in [&repeat, &decal] {
        assert_eq!(
            pixel(mode, 2, 2),
            pixel(&clamp, 2, 2),
            "a tile mode changed the image's own interior"
        );
    }

    // Outside it they must not. Clamping holds the edge, repeating starts the
    // image again, and decal contributes nothing.
    //
    // "Nothing" here is transparent black rather than the cleared color,
    // because these draw with `Src`, which replaces whatever it covers. Under
    // the source-over a caller would normally use, the same nothing leaves the
    // backdrop untouched, which is what decal is for.
    let outside = (24, 24);
    assert_eq!(
        pixel(&decal, outside.0, outside.1),
        [0, 0, 0, 0],
        "decal drew something outside the image"
    );
    assert_ne!(
        pixel(&clamp, outside.0, outside.1),
        pixel(&decal, outside.0, outside.1),
        "clamp and decal agree outside the image"
    );
    // Repeating puts the image's top-left quadrant back at the start of each
    // tile, where clamping holds the bottom-right one.
    assert_eq!(
        pixel(&repeat, 18, 18),
        [255, 0, 0, 255],
        "repeat did not start the image again"
    );
    assert_eq!(
        pixel(&clamp, 18, 18),
        [255, 255, 0, 255],
        "clamp did not hold the corner texel"
    );

    // Mirroring reflects each alternate copy instead of restarting it. The
    // image covers sixteen target pixels, so the fold is about sixteen, and a
    // fragment samples at its center: 17.5 reflects onto 14.5. That column is
    // inside the image, where every mode agrees, so it is read from `clamp`.
    let mirror = render::<VulkanHal>(&mut ctx, quarter(TileMode::Mirror));
    assert_eq!(
        pixel(&mirror, 2, 2),
        pixel(&clamp, 2, 2),
        "mirror changed the image's own interior"
    );
    assert_eq!(
        pixel(&mirror, 17, 2),
        pixel(&clamp, 14, 2),
        "mirror did not reflect the copy across the edge"
    );
    // And it is a reflection rather than a repeat: the same column under
    // repeating restarts at the image's left edge instead.
    assert_ne!(
        pixel(&mirror, 17, 2),
        pixel(&repeat, 17, 2),
        "mirror and repeat agree where they must differ"
    );
}

#[test]
fn a_batch_naming_a_texture_nobody_supplied_is_refused() {
    let mut ran = 0;
    let mut batch = Batch::new();
    batch
        .push(&FULL, &QUAD, full_target_mapping(), BlendMode::Src)
        .expect("push");

    // Reading whatever happens to be bound would draw a plausible picture from
    // a previous frame's texture, which is worse than failing.
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
            .expect("target");
        let result = ctx.submit_batch(&mut target, &batch, PassDescriptor::preserve());
        ctx.destroy_texture(target);
        assert!(
            result.is_err(),
            "vulkan accepted an unsupplied texture slot"
        );
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut target = ctx
            .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
            .expect("target");
        let result = ctx.submit_batch(&mut target, &batch, PassDescriptor::preserve());
        ctx.destroy_texture(target);
        assert!(result.is_err(), "gles accepted an unsupplied texture slot");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_rendered_target_can_be_sampled_by_a_later_pass() {
    // The case a save layer is built from: draw into a texture, then draw that
    // texture somewhere else. It exercises the layout transition a sampled
    // render target needs, which an uploaded texture does not, since the two
    // arrive in different layouts.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        return;
    };
    let mut layer = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("layer");
    let mut fill = Batch::new();
    fill.push(
        &FULL,
        &QUAD,
        Material::solid([0.0, 1.0, 0.0, 1.0]),
        BlendMode::Src,
    )
    .expect("push");
    ctx.submit_batch(
        &mut layer,
        &fill,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
    )
    .expect("draw into the layer");

    let mut composite = Batch::new();
    composite
        .push(
            &FULL,
            &QUAD,
            Material::Image {
                to_local: across_the_target(0.5),
                slot: 0,
                alpha: 1.0,
                tile: TileMode::Clamp,
                sampling: emblema_hal::Sampling::Linear,
                source: [0.0, 0.0, 1.0, 1.0],
                tint: [1.0, 1.0, 1.0, 1.0],
            },
            BlendMode::Src,
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &composite,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        &[&layer],
    )
    .expect("composite");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    ctx.destroy_texture(layer);

    assert_eq!(
        pixel(&pixels, 16, 16),
        [0, 255, 0, 255],
        "the layer did not survive being sampled"
    );
}

/// A four-by-four single-channel image with a distinct value per texel.
fn coverage_pixels() -> Vec<u8> {
    (0..16u8).map(|i| i * 16 + 8).collect()
}

#[test]
fn a_single_channel_texture_round_trips_on_both_backends() {
    // Coverage is one byte per texel, and storing it four times over costs four
    // times the memory and four times the bandwidth to sample. The transfer
    // paths had the four wired in as a literal, so this is what says they read
    // the format instead: a one-byte format with a four-byte assumption
    // anywhere reads three texels past the end of every row.
    let want = coverage_pixels();
    let mut ran = 0;

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(SOURCE, PixelFormat::R8Unorm))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(
            got, want,
            "vulkan changed a single-channel image in transit"
        );
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::offscreen(SOURCE, PixelFormat::R8Unorm))
            .expect("texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want, "gles changed a single-channel image in transit");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_single_channel_texture_samples_as_coverage() {
    // The shader reads the red channel, which is where a one-channel texture
    // puts its only value and where a four-channel one repeats it. Sampling one
    // as coverage gives that value alone, which is what the glyph material
    // scales a solid by.
    //
    // Left half fully covered and right half half-covered, so a swapped axis or
    // a wrong row stride is a visibly different picture rather than a uniform
    // one.
    let mut texels = vec![0u8; SOURCE.area() as usize];
    for y in 0..SOURCE.height {
        for x in 0..SOURCE.width {
            texels[(y * SOURCE.width + x) as usize] = if x < SOURCE.width / 2 { 255 } else { 128 };
        }
    }

    let mut ran = 0;
    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        assert_coverage(&render_coverage::<VulkanHal>(&mut ctx, &texels), "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        assert_coverage(&render_coverage::<GlesHal>(&mut ctx, &texels), "gles");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

/// Full coverage on the left, half on the right, both tinted white.
fn assert_coverage(pixels: &[u8], backend: &str) {
    assert_eq!(
        pixel(pixels, 4, 16),
        [255, 255, 255, 255],
        "{backend}: left"
    );
    let right = pixel(pixels, 27, 16);
    assert!(
        (right[0] as i32 - 128).abs() <= 2 && right[3] == right[0],
        "{backend}: half coverage came back {right:?}"
    );
}

/// Upload single-channel coverage and draw it through the glyph material.
fn render_coverage<H: Hal>(ctx: &mut H::Context, texels: &[u8]) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    use emblema_hal::Vertex;

    let mut source = ctx
        .create_texture(&TextureDescriptor::offscreen(SOURCE, PixelFormat::R8Unorm))
        .expect("source texture");
    ctx.write_texture(&mut source, texels).expect("upload");

    // The glyph material takes its coordinates from the vertices, so the quad
    // carries them rather than the paint.
    let corners = [
        ([-1.0f32, -1.0], [0.0f32, 1.0]),
        ([1.0, -1.0], [1.0, 1.0]),
        ([1.0, 1.0], [1.0, 0.0]),
        ([-1.0, 1.0], [0.0, 0.0]),
    ];
    let vertices: Vec<Vertex> = corners.iter().map(|(p, uv)| Vertex::new(*p, *uv)).collect();

    let mut batch = Batch::new();
    batch
        .push_mesh(
            &vertices,
            &QUAD,
            Material::Glyph {
                color: [1.0, 1.0, 1.0, 1.0],
                slot: 0,
            },
            emblema_hal::ColorFilter::None,
            BlendMode::Src,
            None,
            emblema_hal::ClipState::UNCLIPPED,
        )
        .expect("push");

    let mut target = ctx
        .create_texture(&TextureDescriptor::offscreen(SIZE, PixelFormat::Rgba8Unorm))
        .expect("target");
    ctx.submit_batch_textured(
        &mut target,
        &batch,
        PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]),
        &[&source],
    )
    .expect("submit");
    let pixels = ctx.read_texture(&mut target).expect("readback");
    ctx.destroy_texture(target);
    ctx.destroy_texture(source);
    pixels
}

#[test]
fn a_texture_that_is_only_sampled_needs_no_attachment_and_still_reads_back() {
    // A ramp and an uploaded image are written once and sampled many times, and
    // never drawn into. Saying so used to change nothing, because one backend
    // built a color attachment for every texture it made whatever the caller
    // asked for -- which is invisible until a format is filterable and not
    // renderable, and then it turns a texture that would have worked into a
    // creation failure for a target nothing wanted.
    //
    // Reading back is a separate permission from being drawn into, and both
    // backends have to agree about that: the usage here asks for transfer and
    // not for a render target, so the readback must still work.
    let want: Vec<u8> = (0..16u8).map(|i| i * 16 + 8).collect();
    let mut ran = 0;

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::sampled(SOURCE, PixelFormat::R8Unorm))
            .expect("vulkan refused a sampled-only texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("vulkan readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want);
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::sampled(SOURCE, PixelFormat::R8Unorm))
            .expect("gles refused a sampled-only texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("gles readback");
        ctx.destroy_texture(texture);
        assert_eq!(got, want);
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_half_float_texture_round_trips_on_both_backends() {
    // The other half of a bug this tree already fixed once. The transfer paths
    // named the channel layout from the format and then wrote the component
    // *type* in as a literal `UNSIGNED_BYTE`, which is invisible while every
    // format is four unsigned bytes and wrong for the two that are not.
    //
    // Half-float is also the first format here that can carry a component
    // outside zero to one, so the values below include some -- a color outside
    // the sRGB primaries is exactly a color with a negative component, and a
    // path that clamps or truncates has nowhere to hide it.
    let values: Vec<f32> = (0..16)
        .flat_map(|i| {
            let v = i as f32 / 8.0;
            [v, -v * 0.25, 1.0 + v, 1.0]
        })
        .collect();
    let want: Vec<u8> = values
        .iter()
        .flat_map(|v| half::f16::from_f32(*v).to_le_bytes())
        .collect();
    let mut ran = 0;

    let check = |got: Vec<u8>, backend: &str| {
        assert_eq!(got.len(), want.len(), "{backend} returned the wrong width");
        for (i, (a, b)) in got.chunks_exact(2).zip(want.chunks_exact(2)).enumerate() {
            let got = half::f16::from_le_bytes([a[0], a[1]]).to_f32();
            let expected = half::f16::from_le_bytes([b[0], b[1]]).to_f32();
            assert!(
                (got - expected).abs() < 1e-3,
                "{backend} changed component {i}: {got} against {expected}"
            );
        }
    };

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::sampled(
                SOURCE,
                PixelFormat::Rgba16Float,
            ))
            .expect("vulkan half-float texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        check(got, "vulkan");
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let mut texture = ctx
            .create_texture(&TextureDescriptor::sampled(
                SOURCE,
                PixelFormat::Rgba16Float,
            ))
            .expect("gles half-float texture");
        ctx.write_texture(&mut texture, &want).expect("upload");
        let got = ctx.read_texture(&mut texture).expect("readback");
        ctx.destroy_texture(texture);
        check(got, "gles");
        ran += 1;
    }
    assert!(ran > 0, "no backend available");
}

#[test]
fn a_float_render_target_follows_what_the_device_reports() {
    // The capability exists so that everything above the HAL can ask, rather
    // than discovering the answer as a framebuffer-incomplete number on one
    // backend and a driver error on the other. This is what says the two agree:
    // where a device reports it can render into half-float, creating such a
    // target must work, and where it does not, the refusal must name the reason
    // rather than arriving from somewhere further down.
    let mut ran = 0;
    let mut check = |offered: bool, made: Result<(), emblema_hal::Error>, backend: &str| {
        ran += 1;
        match (offered, made) {
            (true, Ok(())) | (false, Err(_)) => {}
            (true, Err(e)) => panic!("{backend} reports float targets and refused one: {e}"),
            (false, Ok(())) => panic!("{backend} reports no float targets and made one"),
        }
    };

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let offered = ctx.capabilities().float_render_targets;
        let made = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba16Float,
            ))
            .map(|t| ctx.destroy_texture(t));
        check(offered, made, "vulkan");
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let offered = ctx.capabilities().float_render_targets;
        let made = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba16Float,
            ))
            .map(|t| ctx.destroy_texture(t));
        check(offered, made, "gles");
    }

    // And the same pair on a device built without the capability, so the
    // `(false, Err)` arm above runs everywhere rather than only on hardware that
    // cannot render into half-float. Both real devices here offer it, so before
    // this the refusal half of a both-directions check was never taken.
    //
    // Kept beside the arms above rather than replacing them: a withheld
    // capability proves the refusal follows the capability, and only the real
    // device proves the capability follows the driver.
    if let Ok(mut ctx) = Validated::without(DevicePreference::Auto, Capability::FloatRenderTargets)
    {
        let offered = ctx.capabilities().float_render_targets;
        assert!(!offered, "the capability survived being withheld");
        let made = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba16Float,
            ))
            .map(|t| ctx.destroy_texture(t));
        check(offered, made, "vulkan without float targets");
    }
    if let Ok(mut ctx) =
        GlesValidated::without(DisplayTarget::Surfaceless, Capability::FloatRenderTargets)
    {
        let offered = ctx.capabilities().float_render_targets;
        assert!(!offered, "the capability survived being withheld");
        let made = ctx
            .create_texture(&TextureDescriptor::offscreen(
                SOURCE,
                PixelFormat::Rgba16Float,
            ))
            .map(|t| ctx.destroy_texture(t));
        check(offered, made, "gles without float targets");
    }

    assert!(ran > 0, "no backend available");
}

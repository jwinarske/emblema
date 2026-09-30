//! Draw one frame and write it out.
//!
//! The thing to run first, and laid out as an interface rather than as a
//! gallery of shapes: rounded cards over their own blurred shadows, circles,
//! gradients clipped to rounded tracks, an image panel, a blend mode, and a run
//! of glyphs. All through the public API, naming no backend and reaching for
//! nothing below it.
//!
//! Arranged that way because the features are more convincing where they meet
//! than in isolation. A drop shadow is a blurred layer holding the shape it
//! belongs to, and it is only right if the blur reaches outside the bounds the
//! layer was given — which is a thing to look at rather than assert.
//!
//! ```sh
//! cargo run -p emblema --example frame -- frame.ppm
//! ```
//!
//! The output is a binary PPM, which every image viewer reads and which needs
//! no encoder. Image encoding is out of scope for this project, and taking a
//! dependency on one so that an example could write a nicer file would be a
//! poor trade — `magick frame.ppm frame.png` converts it, and so does anything
//! else.

use emblema::{
    Atlas, BackendPreference, BlendMode, Canvas, Color, Context, Coverage, Extent2D, GlyphKey,
    GradientStop, Layer, Paint, PixelFormat, PositionedGlyph, Rect, Vec2,
};

const SIZE: Extent2D = Extent2D {
    width: 512,
    height: 512,
};

/// The surface format, which is what makes the written file look right.
///
/// Colors are sRGB-encoded inside the renderer, and a file is read as sRGB by
/// every viewer, so the bytes the renderer produces are already the bytes a
/// viewer expects. A plain surface writes them through unchanged.
///
/// An sRGB surface encodes on write, which is right when the values arriving
/// are light and wrong when they are not. This example rendered into a linear
/// surface once and produced a picture far too dark; it then rendered into an
/// sRGB one, which was right until the pipeline stopped carrying light, and
/// would now be far too bright. The format has to match what the pipeline
/// holds, and the pipeline says so in one place: `Color`.
const FORMAT: PixelFormat = PixelFormat::Rgba8Unorm;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "frame.ppm".into());

    // The backend is chosen at run time. One binary serves a board with a
    // working Vulkan driver and one where only GLES is usable, which is the
    // point of the HAL being a trait rather than a build-time choice.
    let mut ctx = match Context::new(BackendPreference::Auto) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("no usable rendering backend: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "{:?} on {}",
        ctx.backend(),
        ctx.capabilities().device_name.clone()
    );

    let (atlas, glyphs) = build_atlas();
    let mut image = ctx
        .create_image(
            Extent2D::new(atlas.size(), atlas.size()),
            PixelFormat::R8Unorm,
        )
        .expect("atlas image");
    ctx.write_image(&mut image, atlas.texels()).expect("upload");

    // sRGB, because these bytes were picked by eye and that is the space an eye
    // picks in -- as is every image file. The format is what decodes them on
    // sample; read as linear they come out pale, which is what this did.
    let mut swatch = ctx
        .create_image(Extent2D::new(2, 2), PixelFormat::Rgba8Unorm)
        .expect("swatch image");
    // Premultiplied, which is what a texture holds. These are opaque, so the
    // two conventions agree — the distinction only shows once alpha is
    // involved, and it is stated here because that is where it gets missed.
    ctx.write_image(
        &mut swatch,
        &[
            220, 90, 60, 255, 60, 140, 220, 255, 60, 200, 140, 255, 230, 200, 70, 255,
        ],
    )
    .expect("upload");

    let recording = compose(&atlas, &glyphs);
    let mut surface = ctx.create_surface(SIZE, FORMAT).expect("surface");
    ctx.draw_with_images(&mut surface, &recording, &[&image, &swatch])
        .expect("draw");
    let pixels = ctx.read(&mut surface).expect("read");

    ctx.destroy_surface(surface);
    ctx.destroy_image(swatch);
    ctx.destroy_image(image);

    match write_ppm(&path, SIZE, &pixels) {
        Ok(()) => eprintln!("wrote {path}"),
        Err(e) => {
            eprintln!("writing {path}: {e}");
            std::process::exit(1);
        }
    }
}

/// Everything the frame draws.
///
/// Laid out as an interface rather than as a gallery of shapes, because that is
/// what the renderer is for and because the features are more convincing where
/// they meet: a card is a rounded rectangle over its own blurred shadow, and
/// the shadow is only right if the blur reaches past the layer that holds it.
fn compose(atlas: &Atlas, glyphs: &[PositionedGlyph]) -> emblema::Recording {
    let mut canvas = Canvas::new(SIZE);
    canvas.clear(Color::srgb(0.93, 0.94, 0.96, 1.0));

    // The header: a gradient behind everything, whose endpoints travel with the
    // transform rather than staying pinned to the screen.
    canvas
        .draw_rect(
            Rect::new(0.0, 0.0, 512.0, 96.0),
            &Paint::linear_gradient(
                Vec2::new(0.0, 0.0),
                Vec2::new(512.0, 96.0),
                vec![
                    GradientStop::new(Color::srgb(0.18, 0.22, 0.55, 1.0), 0.0),
                    GradientStop::new(Color::srgb(0.55, 0.20, 0.45, 1.0), 1.0),
                ],
            ),
        )
        .expect("header");

    // Two cards, each a rounded rectangle over its own shadow. The shadow is a
    // blurred layer holding the same shape in black, offset down -- which is
    // what a drop shadow is, and needs the blur to reach outside the bounds the
    // layer was given or it would end in a straight line.
    for (index, top) in [128.0f32, 248.0].into_iter().enumerate() {
        let card = Rect::new(32.0, top, 480.0, top + 96.0);
        let shadow = Rect::new(card.left, card.top + 8.0, card.right, card.bottom + 8.0);

        canvas.save_layer_bounds(Layer::opacity(0.30).with_blur(9.0), shadow);
        canvas
            .draw_rrect(shadow, 18.0, &Paint::fill(Color::srgb(0.0, 0.0, 0.0, 1.0)))
            .expect("shadow");
        canvas.restore();

        canvas
            .draw_rrect(card, 18.0, &Paint::fill(Color::srgb(1.0, 1.0, 1.0, 1.0)))
            .expect("card");

        // An avatar: a circle, which takes the same distance field the card
        // does and so is a curve rather than a polygon approximating one.
        canvas
            .draw_circle(
                Vec2::new(card.left + 48.0, card.top + 48.0),
                26.0,
                &Paint::fill(if index == 0 {
                    Color::srgb(0.95, 0.55, 0.25, 1.0)
                } else {
                    Color::srgb(0.30, 0.75, 0.60, 1.0)
                }),
            )
            .expect("avatar");

        // A progress bar clipped to its own rounded ends, with the fill drawn
        // past them so the clip is what shapes it.
        let track = Rect::new(
            card.left + 92.0,
            card.top + 60.0,
            card.right - 24.0,
            card.top + 72.0,
        );
        canvas
            .draw_rrect(track, 6.0, &Paint::fill(Color::srgb(0.89, 0.90, 0.93, 1.0)))
            .expect("track");
        canvas.save();
        canvas
            .clip_path(&track.to_rounded_path(6.0))
            .expect("clip to the track");
        let fraction = if index == 0 { 0.7 } else { 0.35 };
        canvas
            .draw_rect(
                Rect::new(
                    track.left,
                    track.top,
                    track.left + track.width() * fraction,
                    track.bottom,
                ),
                &Paint::linear_gradient(
                    Vec2::new(track.left, 0.0),
                    Vec2::new(track.right, 0.0),
                    vec![
                        GradientStop::new(Color::srgb(0.35, 0.65, 1.0, 1.0), 0.0),
                        GradientStop::new(Color::srgb(0.55, 0.35, 0.95, 1.0), 1.0),
                    ],
                ),
            )
            .expect("progress");
        canvas.restore();
    }

    // A panel showing the image paint, clipped to rounded corners. Clipping to
    // a rounded rectangle goes through the stencil rather than the distance
    // field: a clip is not a shape being filled, and the two do not share a
    // path yet.
    let panel = Rect::new(32.0, 368.0, 236.0, 464.0);
    canvas.save();
    canvas
        .clip_path(&panel.to_rounded_path(18.0))
        .expect("clip to the panel");
    canvas
        .draw_rect(panel, &Paint::image(1, panel))
        .expect("image");
    canvas.restore();

    // A blend over the top. Plus accumulates rather than replacing, which is
    // what makes the overlap brighten.
    canvas
        .draw_circle(
            Vec2::new(428.0, 60.0),
            40.0,
            &Paint::fill(Color::srgb(0.25, 0.18, 0.45, 1.0)).with_blend(BlendMode::Plus),
        )
        .expect("glow");

    // A run of glyphs, which is one draw however many there are.
    canvas
        .draw_glyphs(
            glyphs,
            atlas,
            0,
            &Paint::fill(Color::srgb(0.25, 0.27, 0.33, 1.0)),
        )
        .expect("glyphs");

    canvas.finish()
}

/// An atlas of blocks standing in for rasterized glyphs, and a run using them.
///
/// Rasterizing an outline needs a font parser, which is out of scope — bring
/// `swash` or `ttf-parser` and hand the coverage over. What this shows is the
/// shape of that handover and that a run of any length is a single draw.
fn build_atlas() -> (Atlas, Vec<PositionedGlyph>) {
    let mut atlas = Atlas::new(64);
    let mut run = Vec::new();
    for i in 0..8u16 {
        let key = GlyphKey {
            font: 1,
            glyph: i,
            size: 24,
        };
        // A wedge per glyph, so the run is visibly a sequence of different
        // shapes rather than one repeated block.
        let (w, h) = (10u32, 18u32);
        let mut texels = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let lit = x as f32 / w as f32 + y as f32 / h as f32 > i as f32 / 8.0;
                texels[(y * w + x) as usize] = if lit { 255 } else { 40 };
            }
        }
        let rect = atlas
            .insert(
                key,
                &Coverage {
                    width: w,
                    height: h,
                    texels,
                },
            )
            .expect("atlas insert");
        run.push(PositionedGlyph::new(
            key,
            [364.0 + i as f32 * 14.0, 476.0],
            rect,
        ));
    }
    (atlas, run)
}

/// Write tightly packed RGBA as a binary PPM.
///
/// Alpha is dropped, which is what the format holds. The surface was cleared
/// opaque, so nothing here depends on how a viewer would have composited it.
fn write_ppm(path: &str, extent: Extent2D, pixels: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(out, "P6\n{} {}\n255\n", extent.width, extent.height)?;
    let mut rgb = Vec::with_capacity(pixels.len() / 4 * 3);
    for texel in pixels.chunks_exact(4) {
        rgb.extend_from_slice(&texel[..3]);
    }
    out.write_all(&rgb)?;
    out.flush()
}

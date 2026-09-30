//! The one texture a scene can sample.
//!
//! A scene is data that can be written down without a device, which is what
//! lets the same list serve a window, a headless comparison and a board at the
//! end of a cable. A texture handle is the opposite of that, so a scene does
//! not carry one: it says it samples an image, and the executor uploads this
//! and binds it as the only slot.
//!
//! One fixture rather than a table of them, because the alternative is a scene
//! format that has to name images and a set of images that has to be shipped
//! and kept in step. What a picture of a texture has to show is where each
//! texel landed, and one sheet chosen to make that legible does that for every
//! scene that needs it.

use emblema_hal::Extent2D;

/// The slot the executor binds the fixture to.
pub const SLOT: u32 = 0;

/// The fixture's size in texels.
pub const SIZE: Extent2D = Extent2D {
    width: 8,
    height: 8,
};

/// An eight-by-eight sheet, asymmetric in both axes and in every quadrant.
///
/// Deliberately blocky and few: what a texture scene has to show is which
/// texel a coordinate read, and a photograph would hide a transpose that this
/// makes obvious. The quadrants differ in hue and the diagonal differs from
/// its mirror, so a flip, a transpose and a rotation each produce a picture
/// that cannot be mistaken for the original.
pub fn pixels() -> Vec<u8> {
    let mut out = vec![0u8; (SIZE.area() * 4) as usize];
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            let i = ((y * SIZE.width + x) * 4) as usize;
            let left = x < SIZE.width / 2;
            let top = y < SIZE.height / 2;
            // A checker within each quadrant, so a scene that magnifies the
            // sheet shows texel boundaries and one that tiles it shows where
            // the repetitions meet.
            let dark = (x + y) % 2 == 0;
            let base: [u8; 3] = match (left, top) {
                (true, true) => [220, 40, 40],
                (false, true) => [40, 200, 90],
                (true, false) => [50, 90, 230],
                (false, false) => [230, 200, 40],
            };
            let scale = if dark { 160u32 } else { 255 };
            for c in 0..3 {
                out[i + c] = (base[c] as u32 * scale / 255) as u8;
            }
            out[i + 3] = 255;
        }
    }
    out
}

/// The one fragment program a scene can name.
///
/// The same reasoning as the sheet above. A scene is data that can be written
/// down without a device, so it cannot hold a program any more than it can
/// hold a texture: it says it uses the effect, and the executor registers this
/// and names it. One rather than a table, because what a picture of an effect
/// has to show is that a caller's program ran and produced the caller's
/// picture, and one program that draws something no material here draws shows
/// that as well as twenty would.
///
/// It reads two colors from the first two stop slots and a threshold from the
/// first geometry slot, and splits the frame vertically between them.
pub fn effect() -> emblema_hal::RuntimeProgram {
    emblema_hal::RuntimeProgram {
        spirv: emblema_shaders::EFFECT_SPV.to_vec(),
        glsl_es: emblema_shaders::EFFECT_FS_GLSL.to_string(),
    }
}

/// A program that samples two textures and shows their difference.
///
/// Registered second, so a scene naming it names program one. The difference is
/// the operation that cannot be mistaken for either texture alone: where the
/// two agree it is black whatever they hold, so a plate drawn with one texture
/// bound twice, or with the second binding left at the placeholder, is not the
/// plate this makes.
pub fn two_image_effect() -> emblema_hal::RuntimeProgram {
    emblema_hal::RuntimeProgram {
        spirv: emblema_shaders::EFFECT_TWO_IMAGES_SPV.to_vec(),
        glsl_es: emblema_shaders::EFFECT_TWO_IMAGES_FS_GLSL.to_string(),
    }
}

/// A program that samples one texture and multiplies it by a color.
///
/// Registered third, so a scene naming it names program two. It exists for the
/// scenes where a program is an *image filter* rather than a fill: its input is
/// then the layer the draw landed in, and multiplying is the operation that
/// cannot be mistaken for either operand -- a plate whose program read the
/// placeholder instead of the layer is flat where this one carries whatever the
/// draw put there.
pub fn image_effect() -> emblema_hal::RuntimeProgram {
    emblema_hal::RuntimeProgram {
        spirv: emblema_shaders::EFFECT_IMAGE_SPV.to_vec(),
        glsl_es: emblema_shaders::EFFECT_IMAGE_FS_GLSL.to_string(),
    }
}

/// A program that ramps along the coordinate the vertices state.
///
/// Registered fourth, so a scene naming it names program three. The sibling of
/// [`effect`] and deliberately so: it reads `in.uv` where that one reads the
/// clip position, which is the evidence that a per-vertex coordinate reaches a
/// caller's program at all -- something `docs/non-parity.md` section 14 had
/// recorded as impossible until the two were put side by side.
///
/// It ramps rather than splitting, because a ramp is what four quads can be
/// wrong about together: each stating its own share of one range, they draw a
/// single continuous ramp if the coordinate arrived and four copies of the
/// whole ramp if it did not.
pub fn mesh_uv_effect() -> emblema_hal::RuntimeProgram {
    emblema_hal::RuntimeProgram {
        spirv: emblema_shaders::EFFECT_MESH_UV_SPV.to_vec(),
        glsl_es: emblema_shaders::EFFECT_MESH_UV_FS_GLSL.to_string(),
    }
}

/// The color [`image_effect`] multiplies its input by.
pub fn tint_uniforms(tint: [f32; 4]) -> Vec<f32> {
    let mut out = vec![0.0; emblema_hal::RUNTIME_FLOATS];
    out[0..4].copy_from_slice(&tint);
    out
}

/// Lay two colors and a threshold out where [`effect`] reads them.
pub fn effect_uniforms(left: [f32; 4], right: [f32; 4], threshold: f32) -> Vec<f32> {
    let mut out = vec![0.0; emblema_hal::RUNTIME_FLOATS];
    out[0..4].copy_from_slice(&left);
    out[4..8].copy_from_slice(&right);
    out[emblema_hal::material::layout::GEOMETRY] = threshold;
    out
}

/// The slot a scene's glyph atlas arrives at.
///
/// One past the sheet, so a scene drawing both text and an image needs no
/// negotiation about which is which. A scene that draws only text still leaves
/// the sheet at zero rather than shifting down: the number a plate reads is
/// then the same whatever else the plate does.
pub const GLYPH_SLOT: u32 = 1;

/// How wide and tall each fixture glyph is.
pub const GLYPH_SIZE: u32 = 10;

/// The glyphs a scene can name, by index.
///
/// Synthetic rather than rasterized from a font, for the reason the whole
/// fixture is synthetic: a scene has to describe a picture without a device,
/// and a font file is a device of its own -- one whose version decides what the
/// picture is. These are coverage in the shape text has, an outline and a
/// weight, without being letters.
///
/// Four of them, which is enough for a run to be a run: two solid, one at half
/// coverage to check that an atlas is read as coverage rather than as color,
/// and one hollow so a glyph with a hole in it goes through the same path.
pub fn glyph_coverage(index: u32) -> Vec<u8> {
    let size = GLYPH_SIZE;
    let mut out = vec![0u8; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let edge = x == 0 || y == 0 || x == size - 1 || y == size - 1;
            let value = match index % 4 {
                // A filled block, the simplest thing an atlas can hold.
                0 => 255,
                // Half coverage everywhere, which comes out as the paint's
                // color at half alpha and not as a lighter color.
                1 => 128,
                // A ring, so a glyph with a hole in it is drawn as one.
                2 => {
                    if edge {
                        255
                    } else {
                        0
                    }
                }
                // A diagonal wedge, which has partial coverage along an edge
                // that is neither horizontal nor vertical.
                _ => {
                    if x + y >= size {
                        255
                    } else {
                        0
                    }
                }
            };
            out[(y * size + x) as usize] = value;
        }
    }
    out
}

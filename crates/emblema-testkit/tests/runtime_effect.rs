//! A caller's fragment program, on both backends.
//!
//! The picture is the point but not the reason. A runtime effect is a pipeline
//! built at run time from a module this renderer did not know about, and the
//! two backends build one by quite different means: a Vulkan pipeline from
//! SPIR-V, keyed alongside the blend mode and sample count, against a GL
//! program linked from source and bound like any other state. Both have to
//! reach the same picture from the same effect, which is what says the
//! interface is the interface rather than one backend's habit.

use emblema_hal::{
    Batch, BlendMode, Extent2D, Hal, HalContext, Material, PassDescriptor, PixelFormat,
    RuntimeProgram, TextureDescriptor,
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

fn program() -> RuntimeProgram {
    RuntimeProgram {
        spirv: emblema_shaders::EFFECT_SPV.to_vec(),
        glsl_es: emblema_shaders::EFFECT_FS_GLSL.to_string(),
    }
}

fn uniforms(threshold: f32) -> Vec<f32> {
    let mut out = vec![0.0; emblema_hal::RUNTIME_FLOATS];
    out[0..4].copy_from_slice(&[1.0, 0.0, 0.0, 1.0]);
    out[4..8].copy_from_slice(&[0.0, 0.0, 1.0, 1.0]);
    out[20] = threshold;
    out
}

fn render<H: Hal>(ctx: &mut H::Context, id: u32) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    let mut batch = Batch::new();
    batch
        .push(
            &FULL,
            &QUAD,
            Material::Runtime {
                program: id,
                uniforms: uniforms(0.25),
                textures: [None; emblema_hal::MAX_EFFECT_TEXTURES],
            },
            BlendMode::Src,
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

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn the_same_effect_draws_the_same_picture_on_both_backends() {
    let mut ran = 0;
    let mut pictures: Vec<(&str, Vec<u8>)> = Vec::new();

    if let Ok(mut ctx) = Validated::new(DevicePreference::Auto) {
        let id = ctx
            .register_program(&program())
            .expect("register on vulkan");
        pictures.push(("vulkan", render::<VulkanHal>(&mut ctx, id)));
        ran += 1;
    }
    if let Ok(mut ctx) = GlesValidated::new(DisplayTarget::Surfaceless) {
        let id = ctx.register_program(&program()).expect("register on gles");
        pictures.push(("gles", render::<GlesHal>(&mut ctx, id)));
        ran += 1;
    }
    if ran == 0 {
        eprintln!("skipping: no backend available");
        return;
    }

    for (backend, pixels) in &pictures {
        // A threshold of a quarter falls at pixel twenty of thirty-two.
        assert_eq!(
            pixel(pixels, 8, 16),
            [255, 0, 0, 255],
            "{backend}: left of the caller's split"
        );
        assert_eq!(
            pixel(pixels, 28, 16),
            [0, 0, 255, 255],
            "{backend}: right of it"
        );
    }

    if let [(_, a), (_, b)] = pictures.as_slice() {
        assert_eq!(a, b, "the two backends drew the same effect differently");
    }
}

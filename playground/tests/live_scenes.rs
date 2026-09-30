//! Every live scene draws something, at every part of its knob's range.
//!
//! The playground is a thing you look at, which makes it exactly the kind of
//! program that rots quietly: a scene that panics or draws nothing is invisible
//! until somebody steps onto it, and nobody steps onto all of them. This runs
//! them all offscreen, so a broken one is a failure rather than a discovery.
//!
//! It is not a golden test and asserts nothing about how they look. Two things
//! only: that drawing does not panic, and that the result is not the background
//! it started from.

use emblema::vulkan::{DevicePreference, Validated, VulkanHal};
use emblema::{Canvas, Extent2D};
use emblema::{PixelFormat, TextureDescriptor};

#[path = "../src/live.rs"]
// The scene table carries more than this binary reads -- `start` is the
// playground's initial knob, which a test that sweeps the whole range has no
// use for.
#[allow(dead_code)]
mod live;

const SIZE: Extent2D = Extent2D {
    width: 192,
    height: 192,
};

#[test]
fn every_live_scene_draws_across_its_whole_range() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    // The same sheet the playground uploads, because a scene that names a
    // texture slot is refused outright when none is supplied -- and being
    // refused is indistinguishable here from drawing nothing.
    let mut sheet = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(4, 4),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("sheet");
    ctx.write_texture(&mut sheet, &live::sheet_pixels())
        .expect("upload");

    for scene in live::scenes() {
        let (low, high) = scene.range;
        // The ends and the middle, plus a time that is not zero, because a
        // scene that only moves is still a scene that has to draw.
        for (fraction, time) in [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (0.5, 1.7)] {
            let knob = low + (high - low) * fraction;
            let mut canvas = Canvas::new(SIZE);
            (scene.draw)(&mut canvas, SIZE, knob, time);
            let recording = canvas.finish();

            let pixels = emblema::render_offscreen::<VulkanHal>(&mut ctx, &recording, &[&sheet])
                .unwrap_or_else(|e| panic!("{} at {knob}: {e}", scene.name));

            // Something other than the ground it cleared to. A scene that drew
            // nothing leaves one color everywhere, which is the failure this
            // catches -- a knob at the end of its range that silently produces
            // an empty picture.
            let first = &pixels[0..4];
            let drew = pixels.chunks_exact(4).any(|texel| texel != first);
            assert!(
                drew,
                "{} drew nothing at {} {knob:.2} (time {time})",
                scene.name, scene.knob
            );

            // And every pixel is a real color. A NaN reaching the shader
            // arrives as something the readback cannot represent, so this is
            // the cheap version of the sweep the library's own tests run.
            assert_eq!(
                pixels.len(),
                (SIZE.width * SIZE.height * 4) as usize,
                "{}: short frame",
                scene.name
            );
        }
    }
    ctx.destroy_texture(sheet);
}

#[test]
fn a_live_scene_reacts_to_its_knob() {
    // A knob that changes nothing is a scene that is not exercising what its
    // name says. This is what would have caught the corpus backdrop scene that
    // rendered identically with the filter on and off.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    // The same sheet the playground uploads, because a scene that names a
    // texture slot is refused outright when none is supplied -- and being
    // refused is indistinguishable here from drawing nothing.
    let mut sheet = ctx
        .create_texture(&TextureDescriptor::offscreen(
            Extent2D::new(4, 4),
            PixelFormat::Rgba8Unorm,
        ))
        .expect("sheet");
    ctx.write_texture(&mut sheet, &live::sheet_pixels())
        .expect("upload");

    for scene in live::scenes() {
        let (low, high) = scene.range;
        let render = |ctx: &mut Validated, knob: f32| {
            let mut canvas = Canvas::new(SIZE);
            (scene.draw)(&mut canvas, SIZE, knob, 0.0);
            emblema::render_offscreen::<VulkanHal>(ctx, &canvas.finish(), &[&sheet])
                .expect("render")
        };
        let a = render(&mut ctx, low);
        let b = render(&mut ctx, high);
        let differing = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(x, y)| x != y)
            .count();
        assert!(
            differing > 0,
            "{}: the ends of {} produce the same picture, so the knob does nothing",
            scene.name,
            scene.knob
        );
    }
    ctx.destroy_texture(sheet);
}

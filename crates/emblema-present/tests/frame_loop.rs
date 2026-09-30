//! Driving a frame loop through the presentation trait.
//!
//! The point of separating the two axes is that the loop is identical whatever
//! the target: acquire, render, present. This runs that loop over the offscreen
//! target and checks the results match what rendering directly to a texture
//! produces, which is the property every other target will be held to when it
//! arrives.

use emblema_hal::{BlendMode, Extent2D, Hal, HalContext, Material, PassDescriptor, PixelFormat};
use emblema_hal_vulkan::{ContextConfig, DevicePreference, VulkanContext, VulkanHal};
use emblema_present::{OffscreenTarget, PresentTarget};
use emblema_testkit::{corpus, render_scene, Scene};

const SIZE: Extent2D = Extent2D {
    width: 128,
    height: 128,
};

fn context() -> Option<VulkanContext> {
    // Validation on, because these drive the deferred submission path and the
    // fences that gate it, and that is where a resource freed while the GPU is
    // still reading it shows up. Nothing about such a bug reaches the pixels:
    // the frame renders correctly right up until the memory is reused.
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

/// Fail if the validation layer reported anything.
fn assert_validation_clean(ctx: &VulkanContext) {
    if !ctx.validation_active() {
        return;
    }
    let errors: Vec<_> = ctx
        .validation_messages()
        .into_iter()
        .filter(|m| m.severity == emblema_hal_vulkan::ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "validation errors: {errors:?}");
}

/// Render one scene through a presentation target, as a frame loop would.
fn present_scene<H: Hal, T: PresentTarget<H>>(
    ctx: &mut H::Context,
    target: &mut T,
    scene: &Scene,
) -> Vec<u8>
where
    H::Context: HalContext<Hal = H>,
{
    use emblema_testkit::record_scene;

    // Recording goes through the testkit rather than being repeated here.
    // Turning scene data into drawing has real content — a gradient's endpoints
    // travel through two transforms, a layer becomes a pass of its own — and a
    // second copy of it silently stopped matching the first as soon as the
    // scene format grew.
    let recording = record_scene(scene).expect("record");

    // The loop the architecture promises is the same everywhere. Presenting a
    // recording rather than a batch is what lets a scene with a layer through:
    // the layer passes render into targets of their own and the root lands in
    // the acquired image, which is the same shape of work an offscreen render
    // does and the reason this comparison is worth making.
    // The textures the scene names, through the testkit for the same reason the
    // recording is: a second copy of that table stopped being right the moment
    // the corpus first held a scene that reads one, and the failure read as a
    // fault in presenting rather than in this line.
    let fixtures = emblema_testkit::Fixtures::<H>::prepare(ctx, scene).expect("fixtures");
    let image = target.acquire(ctx).expect("acquire");
    emblema_core::execute::<H>(ctx, image, &recording, &fixtures.bound()).expect("submit");
    fixtures.destroy(ctx);
    target.present(ctx).expect("present");

    let image = target.acquire(ctx).expect("re-acquire for readback");
    ctx.read_texture(image).expect("readback")
}

#[test]
fn a_frame_loop_over_a_target_matches_rendering_to_a_texture() {
    let Some(mut ctx) = context() else { return };
    let mut failures = Vec::new();

    for scene in corpus() {
        // Both halves of the comparison render on this device, so a scene it
        // cannot render has nothing to say about whether presenting changes the
        // result. Skipping is right here in a way it would not be in the
        // conformance corpus: this test is about the presentation path, and the
        // capability being absent is not a gap in that path's coverage.
        if !scene.supported_by(HalContext::capabilities(&ctx)) {
            continue;
        }
        let mut target =
            OffscreenTarget::<VulkanHal>::new(&mut ctx, scene.size, PixelFormat::Rgba8Unorm)
                .expect("target");
        let through_target = present_scene::<VulkanHal, _>(&mut ctx, &mut target, &scene);
        target.destroy(&mut ctx);

        let direct = render_scene::<VulkanHal>(&mut ctx, &scene).expect("direct");
        if through_target != direct.pixels {
            failures.push(scene.name);
        }
    }

    // Going through a presentation target must change nothing about what is
    // rendered. Any difference means the target is doing something to the image
    // rather than only deciding where it goes.
    assert!(
        failures.is_empty(),
        "presenting changed the result for: {failures:?}"
    );
    assert_validation_clean(&ctx);
}

#[test]
fn presenting_counts_frames() {
    let Some(mut ctx) = context() else { return };
    let mut target =
        OffscreenTarget::<VulkanHal>::new(&mut ctx, SIZE, PixelFormat::Rgba8Unorm).expect("target");

    assert_eq!(target.presented_frames(), 0);
    for expected in 1..=5u64 {
        let _ = target.acquire(&mut ctx).expect("acquire");
        target.present(&mut ctx).expect("present");
        assert_eq!(target.presented_frames(), expected);
    }
    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn a_target_keeps_its_contents_between_acquisitions() {
    let Some(mut ctx) = context() else { return };
    let mut target =
        OffscreenTarget::<VulkanHal>::new(&mut ctx, SIZE, PixelFormat::Rgba8Unorm).expect("target");

    // Draw once, then acquire again without drawing. An offscreen target has
    // one image rather than a rotating set, so the second acquisition must show
    // the first frame's work. A target that rotated buffers would not, and that
    // difference is exactly what a frame loop has to be written against.
    let mut batch = emblema_hal::Batch::new();
    batch
        .push(
            &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            &[0, 1, 2, 0, 2, 3],
            Material::solid([1.0, 0.0, 0.0, 1.0]),
            BlendMode::Src,
        )
        .expect("push");

    let image = target.acquire(&mut ctx).expect("acquire");
    ctx.submit_batch(image, &batch, PassDescriptor::clear([0.0, 0.0, 0.0, 1.0]))
        .expect("submit");
    target.present(&mut ctx).expect("present");

    let pixels = target.read(&mut ctx).expect("read");
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn reconfiguring_changes_the_extent_and_keeps_the_target_usable() {
    let Some(mut ctx) = context() else { return };
    let mut target =
        OffscreenTarget::<VulkanHal>::new(&mut ctx, SIZE, PixelFormat::Rgba8Unorm).expect("target");
    assert_eq!(target.extent(), SIZE);

    let bigger = Extent2D::new(192, 96);
    target.reconfigure(&mut ctx, bigger).expect("reconfigure");
    assert_eq!(target.extent(), bigger);

    // Usable afterwards, and at the new size: a reconfigure that left a stale
    // image would read back the wrong number of pixels.
    let image = target.acquire(&mut ctx).expect("acquire");
    assert_eq!(HalTextureExt::extent_of(image), bigger);
    let pixels = target.read(&mut ctx).expect("read");
    assert_eq!(pixels.len(), (bigger.area() * 4) as usize);

    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

#[test]
fn reconfiguring_to_the_same_extent_is_a_no_op() {
    let Some(mut ctx) = context() else { return };
    let mut target =
        OffscreenTarget::<VulkanHal>::new(&mut ctx, SIZE, PixelFormat::Rgba8Unorm).expect("target");

    // A resize event that reports the size it already is arrives routinely, and
    // reallocating on each would churn memory for nothing.
    for _ in 0..8 {
        target.reconfigure(&mut ctx, SIZE).expect("reconfigure");
    }
    assert_eq!(target.extent(), SIZE);
    target.destroy(&mut ctx);
    assert_validation_clean(&ctx);
}

/// Reach a texture's extent through the HAL trait rather than the backend type.
trait HalTextureExt {
    fn extent_of(&self) -> Extent2D;
}

impl<T: emblema_hal::HalTexture> HalTextureExt for T {
    fn extent_of(&self) -> Extent2D {
        self.extent()
    }
}

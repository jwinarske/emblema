//! What the validation layer says while a context is being destroyed.
//!
//! Every other Vulkan assertion in this workspace reads the validation log
//! through the context that owns it, which cannot cover the context's own
//! teardown: a child object that outlives its device is reported at
//! `vkDestroyDevice`, and that call is inside the drop. So the reports made
//! there were not merely unchecked, they were unreachable -- and a descriptor
//! set layout leaked on every device for as long as the material set has
//! existed, with the whole suite green.

use emblema_hal_vulkan::{DevicePreference, Validated, VulkanHal};
use emblema_testkit::{catalog, render_scene};

#[test]
fn destroying_a_context_leaves_the_validation_layer_with_nothing_to_say() {
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
    if !ctx.validation_active() {
        eprintln!("skipping: the validation layer is not installed");
        return;
    }
    // Held across the drop below. This is the whole point: the context is
    // about to destroy the object every other accessor reads through.
    let log = ctx.validation_log();

    // Something has to be drawn first, because the objects worth checking are
    // the ones a context builds lazily: the layout that was leaking belongs to
    // the material set and is created on the first draw that needs one, so a
    // context that never drew would have passed this while the fault was live.
    //
    // The basic and effect plates between them exercise a solid, an image, a
    // gradient and a caller's own program, which is every route to a material
    // and every route to a texture. Rendering the whole catalog would say no
    // more and would put a minute on the suite.
    let mut drawn = 0;
    for scene in catalog() {
        let topic = scene.name.split('/').next();
        if !matches!(topic, Some("basic" | "effect")) || !scene.supported_by(ctx.capabilities()) {
            continue;
        }
        if render_scene::<VulkanHal>(&mut ctx, &scene).is_ok() {
            drawn += 1;
        }
    }
    assert!(
        drawn > 10,
        "only {drawn} scenes drew, which is too few to have built much to leak"
    );
    assert!(
        ctx.validation_clean(),
        "the layer complained before teardown, so this test cannot say anything \
         about teardown: {:?}",
        ctx.validation_messages()
    );

    drop(ctx);

    let errors = log.errors();
    assert!(
        errors.is_empty(),
        "destroying the context produced {} validation error(s). An object \
         outliving its device is undefined behavior, not a leak that ends with \
         the process: {errors:#?}",
        errors.len()
    );
}

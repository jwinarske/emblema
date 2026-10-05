//! The catalog rendered, and rendered the same way twice.
//!
//! These scenes mirror Impeller's own playground, which is a mode of its test
//! suite rather than a separate program: a test renders a frame, and with a
//! flag set it opens a window instead of running headless. Ours are data, so
//! the window and the headless run read the same list -- but that only helps
//! if the list is exercised, and a picture nobody renders is a picture nobody
//! knows is broken.
//!
//! Two things are checked, and deliberately not a third. Every scene draws
//! something on each backend, and the two backends agree. What is not checked
//! is whether a scene looks like the one it is named after: that is a judgment
//! about a picture, made by looking at it, which is what the playground is
//! for. The corpus is where per-scene tolerances and a software reference
//! live; this collection is broad rather than exact.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it. A failed assumption in a test should stop the
// run; the workspace denies these because a *library* must not.
#![allow(clippy::panic)]

use emblema_hal::{BlendMode, Hal, HalContext};
use emblema_hal_gles::Validated as GlesValidated;
use emblema_hal_gles::{DisplayTarget, GlesHal};
use emblema_hal_vulkan::Validated;
use emblema_hal_vulkan::{DevicePreference, VulkanHal};
use emblema_testkit::scene::LayerSpec;
use emblema_testkit::{
    accepts, catalog, compare, render_scene, Image, Item, Node, Scene, Shape, Tolerance, Transform,
};

/// What the two backends are allowed to disagree by across the whole catalog.
///
/// One budget for the collection rather than one argued about per picture,
/// which is the difference between this and the corpus. It is looser than any
/// corpus scene's, and deliberately: a plate here draws several shapes and
/// long strokes, so it has an order of magnitude more edge pixels than a
/// corpus scene does, and an edge pixel is exactly where two rasterizers may
/// legitimately assign coverage to a different number of samples. At four
/// samples one of them is a quarter, which is sixty-four in eight bits.
///
/// So this bounds *how much of the picture* may differ, not by how much. Three
/// per cent covers the edges of the busiest plate with room to spare, and
/// nothing that is actually wrong stays inside it: a shape missing on one
/// backend, a stroke placed differently, a color computed differently, each
/// cover far more of the frame than their own outlines.
const CATALOG: Tolerance = Tolerance::new(1, 0.03);

/// The budget above is the wrong shape for a scene that blends additively, and
/// the two are worth keeping apart rather than widening one to cover both.
///
/// It bounds how much of a picture may differ, on the reasoning that what
/// differs is edges. An additive scene's disagreement is not on its edges: every
/// overlapping draw contributes its own rounding and they add rather than
/// replace, so what differs is the whole overlapping *area* and what bounds it
/// is the magnitude. Judging that by area asks the wrong question, and the
/// additive atlas plate answers it at 3.57 per cent -- just past a budget sized
/// for outlines, by a mechanism that has nothing to do with them.
///
/// So an additive scene is judged on magnitude instead. Found on a Raspberry
/// Pi 5, where the two backends read two levels apart on that plate while
/// sharing a GPU and its fixed-function blending -- so the difference is the
/// two shader compilers, and the blend adding it up.
///
/// This is not the loosening it reads as, and the direction is worth stating
/// because it is the opposite of what swapping in a wider-sounding tolerance
/// suggests. The budget above admits three per cent of the frame differing by
/// *any* amount, up to and including a shape drawn on one backend and missing
/// on the other, as long as it is small enough. The one below admits none: no
/// pixel may differ by more than three levels, anywhere. Four plates take this
/// branch and all four hold to it on a Pi 5, where the two backends are v3d and
/// llvmpipe and share nothing but the frame -- so on the scenes that take it,
/// this is the stricter of the two.
fn tolerance_for(scene: &Scene) -> Tolerance {
    match scene.blends_additively() {
        true => Tolerance::ACCUMULATED,
        false => CATALOG,
    }
}

fn render<H: Hal>(ctx: &mut H::Context, scene: &Scene) -> Image
where
    H::Context: HalContext<Hal = H>,
{
    render_scene::<H>(ctx, scene).unwrap_or_else(|e| panic!("{}: {e}", scene.name))
}

#[test]
fn every_catalog_scene_draws_something() {
    // Every Vulkan device on this machine rather than the preferred one, and
    // the difference is not small. Advanced blending is an extension a discrete
    // or integrated GPU can lack while the software rasterizer beside it has
    // it, and nineteen plates of this catalog need it -- so asking only the
    // preferred device skipped nineteen scenes here, silently, and reported a
    // pass. The count in `docs/playground-parity.md` said what the catalog
    // holds and nothing said what had been looked at.
    let mut devices: Vec<Validated> = Vec::new();
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        if let Ok(ctx) = Validated::new(preference) {
            let name = ctx.capabilities().device_name.clone();
            if devices.iter().any(|d| d.capabilities().device_name == name) {
                continue;
            }
            devices.push(ctx);
        }
    }
    if devices.is_empty() {
        eprintln!("skipping: no Vulkan device");
        return;
    }

    let mut blank = Vec::new();
    let mut orphans = Vec::new();
    let mut drawn = 0usize;
    for scene in catalog() {
        let Some(index) = devices
            .iter()
            .position(|ctx| scene.supported_by(ctx.capabilities()))
        else {
            // Reported by name rather than asserted, the way the corpus reports
            // its own: a scene nothing here can render is not a failure of the
            // renderer, and naming it is what keeps it from reading as coverage.
            orphans.push(scene.name);
            continue;
        };
        let ctx = &mut devices[index];
        drawn += 1;
        let image = render::<VulkanHal>(ctx, &scene);
        // Something other than the ground it cleared to. A scene whose
        // geometry landed offscreen, or whose color matched the background,
        // is one nobody would notice was wrong by scrolling past it.
        //
        // Against the background rather than against the first pixel, which is
        // not the same question and gets one plate wrong: a scene that covers
        // the frame twice and blends the second over the first is *correctly*
        // one color everywhere, and reads as blank to a check that only asks
        // whether the picture is uniform. It went unnoticed because the plate
        // in question needs advanced blending, which the device here does not
        // have -- so the flaw only surfaced on a software device that does.
        let ground: [u8; 4] = scene
            .background
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let pixels = image.pixels.as_slice();
        if pixels.chunks_exact(4).all(|texel| texel == ground) {
            blank.push(scene.name);
        }
    }
    eprintln!(
        "drew {drawn} of {} catalog scenes across {} device(s)",
        catalog().len(),
        devices.len()
    );
    if !orphans.is_empty() {
        eprintln!(
            "{} scene(s) no available device can render: {}",
            orphans.len(),
            orphans.join(", ")
        );
    }
    assert!(
        blank.is_empty(),
        "these catalog scenes drew nothing at all: {blank:?}"
    );
}

/// Plates that diverge between backends on a device whose advanced blending is
/// already shown unsound by the probe below.
///
/// Named rather than derived, because these are three scenes in two classes and
/// a predicate covering both would skip far more than it has to. Measured on
/// lavapipe at Mesa 26.2.3 against GLES on the same machine:
///
/// | plate | worst | differing |
/// |---|---|---|
/// | `blend/draw-advanced-blend-partly-offscreen` | 211 | 41.3% |
/// | `blend/framebuffer-advanced-blend-coverage` | 135 | 16.4% |
/// | `atlas/sprites-multiplied-into-what-is-behind` | 14 | 22.0% |
///
/// The first reads its destination where a clip cuts it; the other two blend a
/// *sampled* source. Reduced: an image drawn under `Multiply` over a flat
/// ground differs by 69 between the two backends, where the same color as a
/// solid fill agrees to one. So this is the same family as the group composite
/// -- an advanced blend over something other than a plain solid -- and it is
/// skipped only where that probe has already found the device wanting.
///
/// A device whose advanced blending is sound compares all four hundred and
/// thirty-eight, this list included.
const ADVANCED_BLEND_DIVERGENCES: &[&str] = &[
    "blend/draw-advanced-blend-partly-offscreen",
    "blend/framebuffer-advanced-blend-coverage",
    "atlas/sprites-multiplied-into-what-is-behind",
];

/// Whether this device composites a group under an advanced blend at all.
///
/// Probed rather than read from a version string, the way `golden.rs` probes
/// for the scissor defect, and for the same reason: a version is a guess about
/// behavior and this is the behavior.
///
/// A single opaque circle inside a `Multiply` group over a flat ground. The
/// group's content is opaque and its alpha is one, so a device that composites
/// it has to produce the same pixel a direct `Multiply` draw of that circle
/// would -- and a device that drops the composite leaves the ground showing.
/// Measured on lavapipe at Mesa 26.2.3: the ground, unchanged, for every one of
/// the fifteen advanced modes. GLES through `GL_KHR_blend_equation_advanced`
/// composites it correctly on the same machine, and matches
/// `emblema_hal::blend::blend_advanced` to a level -- Multiply, Screen and
/// Difference against a destination of `[130, 119, 87]` give `[123, 80, 52]`,
/// `[186, 143, 104]` and `[114, 74, 61]` where the formula says `[124, 80, 52]`,
/// `[186, 143, 104]` and `[115, 74, 62]`.
///
/// **Whether the fault is this renderer's or lavapipe's is undetermined**, and
/// no device here can settle it. Vulkan advanced blending is reported by exactly
/// one of them, and it is the one that is wrong:
///
/// | device | Vulkan advanced blend |
/// |---|---|
/// | the workstation's GPU, which `Auto` picks | no |
/// | lavapipe, Mesa 26.2.3 | **yes** |
/// | lavapipe, Mesa 25.2.8, which CI has | no |
/// | Adreno 640 on the SA8155P | no |
/// | Vivante GC7000UL on the i.MX8MP | no |
///
/// So a second opinion needs a bare-API Vulkan reproduction rather than another
/// board, which `docs/architecture.md`'s rule asks for before a driver is named
/// and which has not been written. What is certain is the asymmetry: a solid
/// source under an advanced blend agrees between the backends to a level, and a
/// non-solid one does not -- a group's image writes nothing, an image or atlas
/// source writes something else. `blend_modes.rs` compares every one of the
/// twenty-nine modes against its equation on this same Vulkan device and passes,
/// so what fails is not the arithmetic of a mode.
fn composites_a_group_under_an_advanced_blend(ctx: &mut Validated) -> bool {
    let ground = |items: Vec<Node>| {
        let mut all = vec![Node::Draw(Box::new(Item::fill(
            Shape::Rect {
                min: [0.0, 0.0],
                max: [64.0, 64.0],
            },
            [0.5, 0.6, 0.7, 1.0],
        )))];
        all.extend(items);
        Scene::tree("advanced-group-probe", all).with_samples(1)
    };
    let circle = Shape::Circle {
        center: [32.0, 32.0],
        radius: 20.0,
    };
    let direct = ground(vec![Node::Draw(Box::new(
        Item::fill(circle.clone(), [0.9, 0.35, 0.2, 1.0]).with_blend(BlendMode::Multiply),
    ))]);
    let grouped = ground(vec![Node::Layer {
        layer: Box::new(LayerSpec {
            alpha: 1.0,
            blend: BlendMode::Multiply,
            ..LayerSpec::default()
        }),
        bounds: None,
        transform: Transform::default(),
        children: vec![Node::Draw(Box::new(Item::fill(
            circle,
            [0.9, 0.35, 0.2, 1.0],
        )))],
    }]);

    let (Ok(direct), Ok(grouped)) = (
        render_scene::<VulkanHal>(ctx, &direct),
        render_scene::<VulkanHal>(ctx, &grouped),
    ) else {
        // Unanswerable rather than answered: a device that cannot draw the
        // probe is held to the comparison as usual.
        return true;
    };
    // At the circle's center the two have to agree, coverage being one and the
    // group's alpha being one.
    let at = |image: &emblema_testkit::Image| {
        let i = ((32 * image.width + 32) * 4) as usize;
        [image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]]
    };
    at(&direct) == at(&grouped)
}

/// Whether a scene composites a group under an advanced blend.
fn groups_under_an_advanced_blend(scene: &Scene) -> bool {
    fn walk(node: &Node) -> bool {
        match node {
            Node::Layer {
                layer, children, ..
            } => layer.blend.is_advanced() || children.iter().any(walk),
            _ => false,
        }
    }
    scene.items.iter().any(walk)
}

#[test]
fn the_catalog_matches_across_backends() {
    let mut vulkan = match Validated::new(DevicePreference::Auto) {
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
    let Ok(mut gles) = GlesValidated::new(DisplayTarget::Surfaceless) else {
        eprintln!("skipping: no GLES context");
        return;
    };

    let mut failures = Vec::new();
    let mut gaps = Vec::new();
    let mut compared = 0usize;
    // Whether this Vulkan device composites a group under an advanced blend at
    // all. See `composites_a_group_under_an_advanced_blend`: lavapipe at Mesa
    // 26.2.3 writes nothing, and a comparison cannot be held against a device
    // that drops the draw.
    let composites_groups = composites_a_group_under_an_advanced_blend(&mut vulkan);

    for scene in catalog() {
        // A scene needing a capability a device does not have is coverage that
        // was not got, not a difference between backends. The scene derives
        // that need from what it contains, so a plate using an advanced blend
        // on a device without the extension is reported here rather than
        // failing as a regression.
        if !scene.supported_by(vulkan.capabilities()) || !scene.supported_by(gles.capabilities()) {
            gaps.push(scene.name);
            continue;
        }
        if !composites_groups
            && (groups_under_an_advanced_blend(&scene)
                || ADVANCED_BLEND_DIVERGENCES.contains(&scene.name))
        {
            gaps.push(scene.name);
            continue;
        }
        let a = render::<VulkanHal>(&mut vulkan, &scene);
        let b = render::<GlesHal>(&mut gles, &scene);
        compared += 1;
        let difference = compare(&a, &b).expect("same size");
        if !accepts(&difference, tolerance_for(&scene)) {
            failures.push(format!(
                "{}: {}",
                scene.name,
                difference.describe(tolerance_for(&scene))
            ));
        }
    }
    // Said out loud rather than left implicit. A comparison that quietly
    // compared nothing passes, and the number is the only thing that
    // distinguishes that from a comparison that found no differences.
    eprintln!(
        "compared {compared} of {} catalog scenes across backends",
        catalog().len()
    );
    if !gaps.is_empty() {
        eprintln!(
            "{} scene(s) not compared, by declared capability: {}",
            gaps.len(),
            gaps.join(", ")
        );
    }
    assert!(
        failures.is_empty(),
        "catalog scenes differ between backends:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn a_scene_that_needs_a_fixture_renders_on_its_own() {
    // A catalog hides an ordering bug by construction: every scene shares one
    // context, so whatever the first scene set up is there for the rest. The
    // fixture programs were registered by a derivation that decided which
    // scenes wanted them, that derivation missed a case, and the plates it
    // missed drew correctly anyway -- another scene having registered them
    // first. Alone they failed every time.
    //
    // So these render one scene per context. Not all of them, which would cost
    // two hundred and forty-nine devices: one for each way a scene can depend
    // on something the executor sets up, which is a program named by a fill, by
    // an image filter, by a group's backdrop, and a texture read from the
    // fixture sheet.
    let wanted = [
        "effect/can-render-runtime-effect",
        "effect/can-render-runtime-effect-filter",
        "effect/clipped-backdrop-filter-with-shader",
        "atlas/draw-atlas-no-color",
    ];
    let mut checked = 0usize;
    for scene in catalog() {
        if !wanted.contains(&scene.name) {
            continue;
        }
        let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
            eprintln!("skipping: no Vulkan device");
            return;
        };
        if !scene.supported_by(ctx.capabilities()) {
            continue;
        }
        render_scene::<VulkanHal>(&mut ctx, &scene)
            .unwrap_or_else(|e| panic!("{} does not render on its own: {e}", scene.name));
        checked += 1;
    }
    assert_eq!(
        checked,
        wanted.len(),
        "{} of the {} scenes named here were not found or not runnable, so this \
         checked less than it says",
        wanted.len() - checked,
        wanted.len()
    );
}

#[test]
fn catalog_names_say_which_file_they_came_from() {
    // The name is the only link back to the test each scene mirrors, so it
    // carries the topic its file is named for. A scene that does not is one
    // whose original nobody can find.
    const TOPICS: [&str; 14] = [
        "basic/",
        // `aiks_dl_primitive_shape_unittests.cc`, whose whole subject is a
        // hairline holding its weight under a transform.
        "primitive/",
        "path/",
        "gradient/",
        "clip/",
        "opacity/",
        "blend/",
        "vertices/",
        "atlas/",
        "blur/",
        "shadow/",
        "dl/",
        "effect/",
        // The text file's subject is text rendering, of which only shaping is
        // out of scope here. These plates mirror what a run does on its way to
        // the screen using synthetic coverage, which is the part of that file
        // this renderer has anything to say about.
        "text/",
    ];
    let stray: Vec<&str> = catalog()
        .iter()
        .map(|scene| scene.name)
        .filter(|name| !TOPICS.iter().any(|topic| name.starts_with(topic)))
        .collect();
    assert!(stray.is_empty(), "catalog scenes with no topic: {stray:?}");

    // A plate showing something this renderer has and Impeller's playground
    // does not would break the one property the naming carries: that a name
    // leads back to the test it mirrors. There was such a plate briefly --
    // nearest sampling, which has no counterpart there -- and it was removed
    // rather than named around, because a catalog that is a mirror plus some
    // extras is not a mirror. Anything of ours worth looking at belongs in the
    // corpus or in a live scene.
    assert!(
        !catalog().iter().any(|s| s.name.ends_with("-nearest")),
        "a catalog plate is showing something the original has no test for"
    );

    // And no two scenes share a name, which would make one of them
    // unreachable from the playground and hide it from the sweep above.
    let mut names: Vec<&str> = catalog().iter().map(|s| s.name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(count, names.len(), "two catalog scenes share a name");
}

/// Strip every backdrop key from a scene's groups, in place.
fn forget_backdrop_ids(nodes: &mut [emblema_testkit::Node]) {
    for node in nodes {
        if let emblema_testkit::Node::Layer {
            layer, children, ..
        } = node
        {
            layer.backdrop_id = None;
            forget_backdrop_ids(children);
        }
    }
}

#[test]
fn a_backdrop_key_plate_would_notice_if_the_key_stopped_working() {
    // The failure this is here for has happened once already, in the plates
    // right beside these: two backdrop-blur scenes rendered identically to
    // themselves with the blur removed, so they asked for the filter and could
    // not show it. A plate that cannot tell whether the thing it is named for
    // happened is worse than no plate, because the catalog reports it as
    // covered.
    //
    // Nothing else in the catalog can catch this. Cross-backend comparison
    // says the two backends agree, and they would agree just as well on the
    // wrong picture. So each keyed plate is rendered again with its keys taken
    // away, and has to come out different: with them, every panel filters the
    // capture taken before any of them drew; without, each captures afresh and
    // the later ones filter what the earlier ones left behind.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    // The single-panel plate is excluded by name rather than by accident. One
    // layer naming a key has nothing to share with, so it is *supposed* to
    // render the same either way -- and that is the thing it is in the catalog
    // to show.
    let alone = "blur/backdrop-blur-with-single-backdrop-id";
    let mut checked = 0usize;
    for scene in catalog() {
        if !scene.name.contains("backdrop-id") || scene.name == alone {
            continue;
        }
        if !scene.supported_by(ctx.capabilities()) {
            eprintln!("skipping: {} is not supported here", scene.name);
            continue;
        }
        let keyed = render::<VulkanHal>(&mut ctx, &scene);

        let mut without = scene.clone();
        forget_backdrop_ids(&mut without.items);
        assert_ne!(
            scene.items, without.items,
            "{} was picked up by name but holds no backdrop key",
            scene.name
        );
        let fresh = render::<VulkanHal>(&mut ctx, &without);

        // The same budget the catalog's own comparison uses, read the other
        // way round: a difference this small is what two rasterizers may
        // legitimately disagree by, so anything at or under it is not evidence
        // that the key did something.
        assert!(
            !accepts(
                &compare(&keyed, &fresh).expect("the two renders are the same size"),
                CATALOG
            ),
            "{} renders the same with its backdrop keys as without them, so it \
             cannot show what it is named for",
            scene.name
        );
        checked += 1;
    }
    assert_eq!(
        checked, 3,
        "three keyed plates share a capture between panels; this checked {checked}"
    );
}

/// A mask blur over a stroke keeps the fill that stroke was given.
///
/// The five `gradient-oval-stroke-mask-blur` plates exist to say that a mask
/// blur acts on coverage and the fill colors whatever survives. They said it
/// for weeks while drawing no color at all: the route for a fill that varies
/// floods the held region with the caller's paint and then masks it by the
/// blurred coverage, and it was carrying the caller's *stroke* style into the
/// flood -- so the flood was the outline of a rectangle the mask then cut away
/// -- while dropping that style from the coverage, which made the mask the
/// region the outline encloses rather than the band it covers.
///
/// Nothing caught it. Both backends agreed, being wrong the same way, and the
/// sweep that asks whether a plate can show its blur only asks whether taking
/// the blur away changes the picture, which it did: from a gradient to
/// nothing. So the claim is checked here instead, on the thing the plates are
/// named for -- that the gradient reaches the blurred stroke.
#[test]
fn a_mask_blur_over_a_stroke_keeps_its_gradient() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    for name in [
        "blur/gradient-oval-stroke-mask-blur",
        "blur/gradient-oval-stroke-mask-blur-translated-and-rotated",
    ] {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .expect("the plate is in the catalog");
        let img = render::<VulkanHal>(&mut ctx, &scene);
        // The gradient runs red to blue, so a pixel it reached has channels
        // that differ. The guide lines and the ground are gray and do not.
        let colored = img
            .pixels
            .chunks(4)
            .filter(|p| p[0].abs_diff(p[2]) > 12)
            .count();
        assert!(
            colored > 500,
            "{name} left {colored} pixels carrying the gradient, which is as \
             good as none: the blurred stroke is not being filled with what \
             the caller asked for"
        );

        // And the mask has to be the band the outline covers rather than the
        // region it encloses, which is the other half of the same route. A
        // coverage drawn without the caller's style makes the two the same
        // picture, so the same scene with its stroke taken off is the control.
        let mut filled = scene.clone();
        for node in filled.items.iter_mut() {
            if let Node::Draw(item) = node {
                if item.stroke.is_some() {
                    item.stroke = None;
                }
            }
        }
        let difference = compare(&img, &render::<VulkanHal>(&mut ctx, &filled)).expect("same size");
        assert!(
            !difference.is_identical(),
            "{name} drew the same picture with its stroke removed, so the \
             blurred mask is the shape rather than its outline"
        );
    }
}

/// The blue an unoverlapped `BLUE_HALF` stroke leaves on the catalog ground.
///
/// Read off a render rather than derived, and the two doubled values are
/// nowhere near it: a second cover reads 226 and a sixth reads 251, so a
/// threshold between them does not need to be exact to mean what it says.
const SINGLE_COVER_BLUE: u8 = 140;

#[test]
fn a_wide_stroke_through_its_own_call_covers_each_pixel_once() {
    // Upstream keeps two forms of this scene, one drawing the rectangle
    // directly and one handing over its path, and both are named for the
    // stroke not overlapping itself. The half alpha is what makes the claim
    // legible: an outline covering a pixel twice is invisible at full opacity
    // and darker at half.
    //
    // The two routes here do not agree, and that is recorded rather than
    // asserted -- `docs/non-parity.md` section 13, with these numbers. What is
    // asserted is the half that holds: where the stroke goes through the call
    // the public API offers for a rectangle, the middle of a rectangle whose
    // stroke is twice its width carries exactly one cover. That is a real
    // property and nothing else in the suite protects it.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    let find = |name: &str| {
        catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"))
    };
    let direct = find("basic/can-render-wide-stroked-rect-without-overlap");
    let as_path = find("basic/can-render-wide-stroked-rect-path-without-overlap");
    assert_ne!(
        direct.items, as_path.items,
        "the pair is supposed to differ in how it says the rectangle"
    );

    // The round-join and mitered rectangles of the lower row, which are the two
    // this renderer draws analytically. A bevel cuts its corner off, which is
    // neither the arc an offset gives nor the point a miter does, so that
    // column still goes to the tessellator and still doubles -- which is what
    // is left of `docs/non-parity.md` section 13.
    let img = render::<VulkanHal>(&mut ctx, &direct);
    for (join, left) in [("round", 49u32), ("miter", 92)] {
        let mut covered = 0usize;
        let mut worst = (0u8, (0u32, 0u32));
        for y in 68..98u32 {
            for x in left..left + 30 {
                let blue = img.pixel(x, y)[2];
                if blue > worst.0 {
                    worst = (blue, (x, y));
                }
                if blue.abs_diff(SINGLE_COVER_BLUE) <= 2 {
                    covered += 1;
                }
            }
        }
        assert!(
            worst.0 <= SINGLE_COVER_BLUE + 20,
            "the {join} column: a pixel at {:?} reads {} where one cover is \
             {SINGLE_COVER_BLUE}, so the outline is blending over itself",
            worst.1,
            worst.0
        );
        assert!(
            covered > 700,
            "the {join} column: only {covered} of 900 pixels carry a full cover, \
             so this window is edge rather than stroke and the bound above \
             proved nothing"
        );
    }
}

fn forget_mask_blurs(nodes: &mut [emblema_testkit::Node]) -> usize {
    let mut dropped = 0;
    for node in nodes {
        match node {
            emblema_testkit::Node::Draw(item) => {
                if item.mask_blur > 0.0 {
                    item.mask_blur = 0.0;
                    dropped += 1;
                }
            }
            emblema_testkit::Node::Layer { children, .. } => {
                dropped += forget_mask_blurs(children);
            }
            emblema_testkit::Node::Picture(spec) => {
                dropped += forget_mask_blurs(&mut spec.children);
            }
            _ => {}
        }
    }
    dropped
}

#[test]
fn every_plate_that_asks_for_a_mask_blur_can_show_one() {
    // The failure this is here for has happened in this catalog before, to the
    // backdrop-blur plates: two scenes rendered identically to themselves with
    // the filter taken away, so they asked for it and could not show it. A
    // plate that cannot tell whether the thing it is named for happened is
    // worse than no plate, because the inventory counts it as covered.
    //
    // Nothing else catches it. Cross-backend comparison says the two backends
    // agree, and they agree just as well on the wrong picture. So every plate
    // holding a mask blur is rendered again with the sigma set to zero and has
    // to come out different.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    let mut checked = 0usize;
    for scene in catalog() {
        let mut without = scene.clone();
        if forget_mask_blurs(&mut without.items) == 0 {
            continue;
        }
        if !scene.supported_by(ctx.capabilities()) {
            eprintln!("skipping: {} is not supported here", scene.name);
            continue;
        }
        let blurred = render::<VulkanHal>(&mut ctx, &scene);
        let sharp = render::<VulkanHal>(&mut ctx, &without);
        assert!(
            !accepts(
                &compare(&blurred, &sharp).expect("the two renders are the same size"),
                CATALOG
            ),
            "{} renders the same with its mask blur as without it, so it cannot \
             show what it asked for",
            scene.name
        );
        checked += 1;
    }
    assert!(
        checked >= 25,
        "only {checked} plates were found to hold a mask blur, which is fewer \
         than the catalog has and means the walk missed some"
    );
}

#[test]
fn a_bounded_backdrop_blur_moves_with_the_transform_it_was_opened_under() {
    // The plate's own subject, which its comment could only state. Both of its
    // blurred bands are bounded by layers whose bounds travel with the layer's
    // transform; bounds left in the frame's axes would put the first band in
    // the wrong place and leave the second exactly where it was however it was
    // turned. So each subcase is checked by changing its transform alone and
    // requiring the picture to change with it.
    //
    // A change, not an amount. How many pixels a turn moves depends on the
    // device's rasterization of the band's edge, and a count recorded on one
    // device is a claim no other device can check.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "blur/can-render-bounded-blur-with-translation")
        .expect("the plate is in the catalog");
    let base = render::<VulkanHal>(&mut ctx, &scene);

    let varied = |ctx: &mut Validated, index: usize, change: &dyn Fn(&mut Transform)| {
        let mut changed = scene.clone();
        match &mut changed.items[index] {
            // The first band is a layer whose bounds travel with its own
            // transform; the second states its turn as a clip around one, the
            // way upstream does, so the transform to change is the clip's.
            Node::Layer { transform, .. } | Node::Clip { transform, .. } => change(transform),
            other => panic!("item {index} should carry a transform, and is {other:?}"),
        }
        render::<VulkanHal>(ctx, &changed)
    };
    let moved = varied(&mut ctx, 1, &|t| t.translate[0] += 16.0);
    let turned = varied(&mut ctx, 2, &|t| t.rotate = 0.0);

    for (what, other) in [
        ("moving the first band", &moved),
        ("unturning the second", &turned),
    ] {
        let difference = compare(&base, other).expect("same size");
        assert!(
            !difference.is_identical(),
            "{what} left the picture unchanged, so its bounds are not following \
             the layer's transform"
        );
    }
}

#[test]
fn a_layers_blur_widens_with_the_transform_it_was_opened_under() {
    // A filter on a save layer is stated in the space of the caller, not in
    // device pixels, so the same layer opened under a threefold scale blurs
    // three times as wide on screen. Upstream draws its scene twice at two
    // scales to say so, and this repository's architecture document records
    // taking the same position deliberately -- the sigma is local and the
    // conversion happens where the layer opens.
    //
    // A renderer that treated the sigma as already-device would draw the two
    // panels with the same soft edge and differ only in size, which is a
    // difference nobody comparing them by eye would necessarily notice. The
    // edge is measured instead.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "basic/save-layer-filters-scale-with-transform")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);

    // How many pixels the right edge of each panel takes to fall from covered
    // to ground, counted along one row well clear of the corners.
    let ramp = |y: u32, xs: std::ops::Range<u32>| {
        xs.filter(|x| (20..250).contains(&img.pixel(*x, y)[0]))
            .count()
    };
    let small = ramp(16, 14..40);
    let large = ramp(74, 90..128);
    assert!(
        small >= 3 && large >= 3,
        "neither panel should have a hard edge; they measured {small} and {large}"
    );
    let ratio = large as f32 / small as f32;
    assert!(
        (2.2..3.8).contains(&ratio),
        "the panel drawn at three times the scale spread its blur over {large} \
         pixels against the other's {small}, a ratio of {ratio:.2}; a sigma \
         carried in device pixels would put that at one"
    );
}

#[test]
fn an_empty_layer_composites_nothing_at_all() {
    // A layer with no contents still allocates a target and still composites
    // it, so what this asks is whether that target was cleared: a layer
    // composited over a buffer nobody wrote would show as a rectangle of
    // whatever the allocation held, exactly where the layer's bounds are.
    //
    // Upstream's scene paints the frame red, opens a layer with a blue paint
    // and closes it at once, and the frame stays red. Here the frame is one
    // color or the plate has failed, which is a thing a test can say exactly.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "basic/empty-save-layer-ignores-paint")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);
    let ground = img.pixel(0, 0);
    for y in 0..128 {
        for x in 0..128 {
            assert_eq!(
                img.pixel(x, y),
                ground,
                "({x}, {y}) is not the color the rest of the frame is, so the \
                 empty layer left something behind"
            );
        }
    }
    assert_eq!(
        ground,
        [255, 0, 0, 255],
        "the frame should be the paint's red"
    );
}

#[test]
fn the_same_thin_line_said_four_ways_says_what_it_should() {
    // Upstream draws its thin-line grid four times -- as a line, as a stroked
    // path, as a filled rectangle and as a filled rounded one -- because a
    // renderer may specialize any of them and a specialization that disagrees
    // with the general route is a bug nobody sees until the two are side by
    // side. Two claims here, and neither is the same as "they all agree".
    //
    // The line and the path used to be the same picture to the byte, because
    // `Canvas::draw_line` built a two-point path and handed it to `draw_path`.
    // That was pinned here rather than assumed, with the note that if the line
    // ever gained a route of its own this is what would say so. It has, and this
    // did.
    //
    // The route is the hairline snap: at a width of exactly zero, under a
    // transform that is a translation and a scale, an axis-aligned line is
    // carried into device space and its constant coordinate rounded to a
    // pixel's middle, so the one pixel it covers is covered fully instead of
    // split half and half across two. Upstream does it in `LineGeometry` and
    // nowhere else, so a two-point *path* does not get it there either -- which
    // is exactly why this family of four plates exists, and the difference is
    // upstream's rather than a disagreement to fix.
    //
    // So the claim tightens instead of going away: they differ, and only in the
    // column where the snap can apply. The grid's three columns are widths of
    // zero, three tenths and one, left to right.
    //
    // The filled forms are not the same picture, and should not be. A stroke
    // narrower than a device pixel is widened to one and dimmed to match,
    // which is upstream's rule and is copied constant for constant; a filled
    // rectangle a third of a pixel tall gets a third of a pixel of coverage
    // and nothing else. So the middle column of the grid is where the two
    // families part, and this asserts that they do -- a renderer that had
    // dropped the thin-stroke rule would make all four agree and would look,
    // from any other test here, entirely well.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut get = |n: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == n)
            .unwrap_or_else(|| panic!("{n} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };

    let line = get("path/draw-lines-with-draw-line");
    let path = get("path/draw-lines-with-path");
    let diff = compare(&line, &path).expect("the two renders are the same size");
    assert!(
        diff.differing > 0,
        "draw_line snaps a hairline to a pixel and a two-point stroked path does \
         not, so the two plates cannot be identical: {diff:?}"
    );
    // And the snap reaches no further than the hairline column. A third of the
    // plate, less a pixel of margin for the widened line's own edge.
    let column = (128.0 / 3.0) as u32 + 1;
    for y in 0..128u32 {
        for x in column..128u32 {
            assert_eq!(
                line.pixel(x, y),
                path.pixel(x, y),
                "({x}, {y}) is outside the hairline column and differs, so the \
                 snap is reaching widths the caller stated"
            );
        }
    }

    for name in [
        "path/draw-lines-with-filled-rects",
        "path/draw-lines-with-filled-round-rects",
    ] {
        let filled = get(name);
        let diff = compare(&line, &filled).expect("the two renders are the same size");
        assert!(
            !accepts(&diff, CATALOG),
            "{name} renders the same as the stroked line, so the widening a \
             sub-pixel stroke gets is not being applied: {diff:?}"
        );
    }
}

#[test]
fn a_tile_mode_family_draws_four_different_pictures() {
    // Eight plates say the same seven-stop ramp under the four tile modes,
    // linear and swept, and the whole of what they are for is the difference
    // between them: the ramp covers a third of the shape, so what fills the
    // rest is the tile mode and nothing else. A renderer that quietly treated
    // one mode as another -- decal as clamp is the easy mistake, since both
    // leave the ramp's end color at the boundary -- would draw two of these
    // identically and pass every other test here.
    //
    // Pairwise rather than against a reference, because there is no reference:
    // no mode is the "right" one to compare the others to, and what is being
    // asserted is that four distinct things happened.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    for family in ["linear-gradient-many-colors", "sweep-gradient-many-colors"] {
        let imgs: Vec<(&str, Image)> = ["clamp", "repeat", "mirror", "decal"]
            .into_iter()
            .map(|tile| {
                let name = format!("gradient/can-render-{family}-{tile}");
                let scene = catalog()
                    .into_iter()
                    .find(|s| s.name == name)
                    .unwrap_or_else(|| panic!("{name} is not in the catalog"));
                (tile, render::<VulkanHal>(&mut ctx, &scene))
            })
            .collect();
        for i in 0..imgs.len() {
            for j in i + 1..imgs.len() {
                let diff = compare(&imgs[i].1, &imgs[j].1).expect("same size");
                assert!(
                    !accepts(&diff, CATALOG),
                    "{family}: {} and {} are the same picture, so one of the two \
                     tile modes is not being applied: {diff:?}",
                    imgs[i].0,
                    imgs[j].0
                );
            }
        }
    }
}

#[test]
fn reversing_a_gradient_reverses_the_picture() {
    // Upstream keeps each axis-aligned gradient in both directions, and its
    // `VerifyNonOptimizedGradient` beside them with the endpoints pulled inside
    // the shape so that whatever fast route the others may take, that one must
    // not. The stops are bunched at one end -- nought, a tenth, and one -- so
    // the picture is lopsided and a reversal is not a symmetry.
    //
    // What this catches is a fast path keyed on the axis that forgot the
    // direction, which would draw the forward and reversed plates the same and
    // is exactly the kind of thing a per-draw optimization gets wrong.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut shot = |name: String| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };
    for axis in ["horizontal", "vertical"] {
        let forward = shot(format!("gradient/fast-gradient-test-{axis}"));
        let back = shot(format!("gradient/fast-gradient-test-{axis}-reversed"));
        let diff = compare(&forward, &back).expect("same size");
        assert!(
            !accepts(&diff, CATALOG),
            "the {axis} gradient draws the same forward and reversed, so the \
             direction is being dropped: {diff:?}"
        );
    }
    // And the one that must not take the fast route differs from the one that
    // may, which is what says the condition was tested rather than assumed.
    let plain = shot("gradient/fast-gradient-test-vertical".to_string());
    let inset = shot("gradient/verify-non-optimized-gradient".to_string());
    let diff = compare(&plain, &inset).expect("same size");
    assert!(
        !accepts(&diff, CATALOG),
        "a gradient inset within its shape and repeating draws the same as one \
         spanning it and clamping: {diff:?}"
    );
}

#[test]
fn a_stroke_wider_than_its_arc_stops_exactly_at_the_frontier() {
    // Upstream draws a line at the rectangle's right side plus half the stroke
    // width and leaves a reader to check that the fat white arc reaches it and
    // does not pass it. The plate keeps the line, and this does the checking.
    //
    // It is worth checking rather than looking at because the stroke is wider
    // than the shape's radius -- half of forty-eight against twenty -- so the
    // inner offset has crossed the center and the outline self-intersects.
    // That is where a stroker either clamps, and falls short, or runs away, and
    // the frontier is the only place the difference is a number.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    // Without the marker, which would otherwise cover the edge it marks.
    let mut scene = catalog()
        .into_iter()
        .find(|s| s.name == "path/fat-stroke-arc")
        .expect("the plate is in the catalog");
    let marker = scene.items.pop();
    assert!(
        marker.is_some(),
        "the plate should end with its frontier line"
    );
    let img = render::<VulkanHal>(&mut ctx, &scene);

    let white_in = |x: u32| (0..128u32).map(|y| img.pixel(x, y)[1]).max().unwrap_or(0);
    // The frontier is at eighty-four, so the last pixel the arc may touch is
    // the one spanning eighty-three to eighty-four, and it is touched
    // partially. Eighty-two is inside and has to be covered outright.
    assert_eq!(
        white_in(82),
        255,
        "the arc should reach a full cover one pixel inside the frontier"
    );
    assert!(
        white_in(83) > 0x11,
        "the arc should reach into the pixel the frontier passes through"
    );
    for x in 85..128 {
        assert_eq!(
            white_in(x),
            0x11,
            "the arc put white at x={x}, past a frontier at eighty-four"
        );
    }
}

#[test]
fn a_shape_clip_cuts_a_circle_through_its_center() {
    // The clip's corner is the circle's center, so what survives is one
    // quarter: two straight edges meeting where a curve used to be. A clip
    // that had been applied as the shape's bounding box would leave the whole
    // circle, and one applied a pixel out would show along two straight edges
    // forty pixels long, which is the easiest kind of difference to measure.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "path/can-render-clips")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);
    let ground = img.pixel(0, 0);
    let mut drawn = 0usize;
    for y in 0..128u32 {
        for x in 0..128u32 {
            if img.pixel(x, y) != ground {
                assert!(
                    x < 64 && y < 64,
                    "({x}, {y}) is outside a clip that ends at sixty-four"
                );
                drawn += 1;
            }
        }
    }
    // A quarter of a disc of radius forty-four is about fifteen hundred.
    assert!(
        (1400..1700).contains(&drawn),
        "a quarter of the circle should survive the clip; {drawn} pixels did"
    );
}

#[test]
fn an_identity_matrix_filter_changes_nothing_it_passes_through() {
    // Upstream draws the same filtered image twice, once with an identity
    // matrix image filter on it, and keeps them side by side for a reader to
    // see that they agree. It puts the filter there to take the draw off its
    // atlas fast path; there is no such path here, so what the pair says
    // instead is the claim that survives the translation: an identity matrix
    // filter routes the draw through an offscreen and resamples it on the way
    // back, and must come out the same anyway.
    //
    // That is not free of ways to fail. A half-texel offset in the resample, a
    // target sized to the wrong bounds, a color filter applied on the way in
    // rather than on the way out -- each would show here and in nothing else.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    for name in [
        "atlas/draw-image-rect-with-blend-color-filter",
        "atlas/draw-image-rect-with-matrix-color-filter",
    ] {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        let img = render::<VulkanHal>(&mut ctx, &scene);
        // The two panels are fifty-six wide, one at four and one at
        // sixty-eight, so the same offset into each is sixty-four apart.
        let mut differing = 0usize;
        let mut worst = 0u8;
        for y in 36..92u32 {
            for x in 4..60u32 {
                let (a, b) = (img.pixel(x, y), img.pixel(x + 64, y));
                let apart = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
                worst = worst.max(apart);
                if apart > 2 {
                    differing += 1;
                }
            }
        }
        assert!(
            differing * 100 < 56 * 56,
            "{name}: {differing} of {} pixels differ between the filtered draw \
             and the plain one, worst by {worst}",
            56 * 56
        );
    }
}

#[test]
fn a_blur_along_one_axis_leaves_the_other_alone() {
    // `dart:ui`'s `ImageFilter.blur` states `sigmaX` and `sigmaY` separately,
    // and a blur here is two separable passes already -- so the second
    // deviation is a field rather than a mechanism. What has to be checked is
    // that it is *used*: a renderer that took one sigma and applied it both
    // ways would draw a square halo where these draw a band, and a renderer
    // that skipped the wrong pass would draw the band the wrong way round.
    //
    // The pass whose deviation is zero is skipped rather than run as an
    // identity, which matters beyond the pass it saves: the reduction that
    // makes a wide blur affordable shrinks the image, so a zero-sigma pass
    // that still resampled would soften the axis it was supposed to leave
    // alone. That is checked here as the sharp axis staying the shape's own
    // width.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut extent = |name: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        let img = render::<VulkanHal>(&mut ctx, &scene);
        let lit: Vec<(u32, u32)> = (0..128u32)
            .flat_map(|y| (0..128u32).map(move |x| (x, y)))
            .filter(|(x, y)| img.pixel(*x, *y)[0] > 20)
            .collect();
        assert!(!lit.is_empty(), "{name} drew nothing");
        let x = lit.iter().map(|p| p.0).max().unwrap() - lit.iter().map(|p| p.0).min().unwrap();
        let y = lit.iter().map(|p| p.1).max().unwrap() - lit.iter().map(|p| p.1).min().unwrap();
        (x, y)
    };
    let across = extent("blur/a-blur-along-one-axis");
    let down = extent("blur/a-blur-along-the-other-axis");

    // The square is thirty-two on a side, so an unblurred axis measures about
    // thirty-one between its first and last lit pixel and a blurred one runs
    // well past it.
    assert!(
        across.1 <= 33,
        "the axis with no deviation spread to {} pixels from a square of \
         thirty-two, so the pass that should have been skipped ran",
        across.1
    );
    assert!(
        across.0 > across.1 + 12,
        "the blurred axis measured {} against the sharp one's {}, which is \
         not a band",
        across.0,
        across.1
    );
    // And the other plate is the same thing turned, which is what says the two
    // deviations reached the two passes rather than one reaching both.
    assert_eq!(
        (down.1, down.0),
        (across.0, across.1),
        "the two plates should be each other transposed"
    );
}

#[test]
fn composing_a_channel_swap_with_a_blur_gives_the_same_picture_either_way() {
    // Upstream keeps both orders, and they are the same picture on purpose: a
    // channel swap is a permutation matrix, a blur is a weighted sum, and two
    // linear operators commute. So the plates agree, and the agreement is the
    // assertion rather than a redundancy.
    //
    // What it catches is a composition that applied only the outer filter.
    // That failure keeps both plates valid-looking -- one would be a recolored
    // sharp circle and the other a blurred green one -- and it makes them
    // differ, which nothing else here would notice. Dropping the *inner* one
    // instead is caught by the rule that a scene carrying a filter has to
    // render differently without it.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut shot = |name: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };
    let inner = shot("blur/compose-paint-blur-inner");
    let outer = shot("blur/compose-paint-blur-outer");
    let diff = compare(&inner, &outer).expect("the two renders are the same size");
    // Within a level, not bit for bit, and the gap between those two is a
    // device rather than a renderer. The operators commute exactly in real
    // arithmetic; what does not commute is where the rounding falls, since one
    // order quantizes the swapped color before the weighted sum and the other
    // quantizes the sum before the swap. lavapipe rounds both the same way and
    // these were byte for byte identical for as long as that was the only
    // device asked. On v3d they differ by one level on a hundred and
    // twenty-seven pixels of sixteen thousand, which this used to report as a
    // dropped filter.
    //
    // A level is still far inside what the test is for, and the margin is
    // measured rather than described: rendering these plates with one of the two
    // filters deleted puts them 178 levels apart over 43 per cent of the frame
    // with the swap gone, and 84 over 32 per cent with the blur gone. So the
    // failure this guards against clears a bound of one by two orders of
    // magnitude, and `max_delta` is the right thing to bound because that
    // failure is large everywhere rather than small somewhere.
    assert!(
        diff.max_delta <= 1,
        "a permutation and a blur commute, so composing them either way is one \
         operator; these differ by more than a rounding, which means one of the \
         two was dropped: {diff:?}"
    );
    // And the swap ran at all: green in, red out.
    let middle = inner.pixel(64, 64);
    assert!(
        middle[0] > middle[1],
        "the swap should take the circle's green to red; the middle reads {middle:?}"
    );
}

#[test]
fn a_clip_cuts_a_blurred_shape_after_the_blur_rather_than_before() {
    // The halo has to stop dead at the clip and the shape's own edge has to
    // stay soft. Blurring what the clip left would soften the cut as well, and
    // the two are told apart by looking at one edge of each in the same frame.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "blur/can-render-clipped-blur")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);

    // The clip's right edge is at a hundred and twelve, and the circle reaches
    // past it, so the transition there is one pixel wide.
    let green = |x: u32, y: u32| img.pixel(x, y)[1];
    assert!(
        green(110, 76) > 120 && green(114, 76) < 40,
        "the clip's edge should be a cut, and reads {} then {}",
        green(110, 76),
        green(114, 76)
    );
    // The circle's own top edge is nowhere near the clip and has to fade.
    let column: Vec<u8> = (28..48u32).map(|y| green(76, y)).collect();
    let partial = column.iter().filter(|g| (40..120).contains(*g)).count();
    assert!(
        partial >= 4,
        "the circle's unclipped edge should fade over several pixels; {partial} \
         of the column carried a partial value, so the blur is being cut off"
    );
}

#[test]
fn a_blurred_layer_survives_a_mirrored_transform() {
    // A mirrored transform has a negative determinant, so a renderer deriving
    // a layer's extent by transforming its corners and subtracting gets a
    // negative width -- a target of no size, which draws nothing at all. That
    // is the failure upstream keeps its flipped scene for, and it is loud:
    // either the layer is there or it is not.
    //
    // The content sits left of center in the layer's own space, so it has to
    // land right of center on the frame. Symmetric content would have made the
    // mirror invisible, and a renderer that dropped the flip would place the
    // rectangle on the wrong side while drawing an otherwise plausible
    // picture.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "blur/gaussian-blur-flipped")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);

    let red: Vec<(u32, u32)> = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let p = img.pixel(*x, *y);
            p[0] > 100 && p[1] < 100
        })
        .collect();
    assert!(
        red.len() > 1000,
        "the mirrored layer drew {} red pixels, which is not a layer",
        red.len()
    );
    let left = red.iter().map(|p| p.0).min().unwrap_or(0);
    let right = red.iter().map(|p| p.0).max().unwrap_or(0);
    assert!(
        left > 64,
        "content left of center in the layer's space belongs right of center \
         on the frame; it runs from {left} to {right}"
    );
    assert!(
        right <= 112,
        "and it must still stop at the layer's bounds, but reaches {right}"
    );
}

#[test]
fn a_clipped_blur_fills_its_window_and_turns_with_its_content() {
    // The clip is stated outside the transform, so the window stays put on the
    // frame while what is drawn into it moves: the blur's target is decided by
    // one space and its contents by another. Upstream keeps the scaled version
    // and the scaled-and-turned one side by side for that reason.
    //
    // Two things have to hold and they pull in opposite directions. The window
    // is filled edge to edge in both, so nothing about the transform may leave
    // a gap at the clip -- and the two are different pictures, so the rotation
    // has to reach the content. A renderer that clipped in the wrong space
    // would fail the first; one that dropped the rotation would fail the
    // second, while still drawing a plausible blurred window.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut shot = |name: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        render::<VulkanHal>(&mut ctx, &scene)
    };
    let flat = shot("blur/gaussian-blur-scaled-and-clipped");
    let turned = shot("blur/gaussian-blur-rotated-and-clipped");

    // The window is thirty-four to ninety-four and forty-five to eighty-three,
    // and every pixel of it has to differ from the ground outside.
    let ground = flat.pixel(0, 0);
    for img in [&flat, &turned] {
        let mut inside = 0usize;
        for y in 45..83u32 {
            for x in 34..94u32 {
                if img.pixel(x, y) != ground {
                    inside += 1;
                }
            }
        }
        assert_eq!(
            inside,
            60 * 38,
            "the clip's window should be filled edge to edge; {inside} of \
             {} pixels were",
            60 * 38
        );
        assert_eq!(img.pixel(33, 64), ground, "and nothing may fall outside it");
    }

    let diff = compare(&flat, &turned).expect("the two renders are the same size");
    assert!(
        !accepts(&diff, CATALOG),
        "the turned plate renders the same as the flat one, so the rotation is \
         not reaching the content: {diff:?}"
    );
}

#[test]
fn a_blur_turns_with_the_transform_it_was_stated_under() {
    // `dart:ui` states a deviation per axis in the space the caller was drawing
    // in. If that space is turned relative to the target, the blur has to turn
    // with it -- otherwise a horizontal smear stays horizontal on screen while
    // the thing it is smearing rotates underneath, which is what this renderer
    // did until the passes took their directions from the transform.
    //
    // A quarter turn is the case that can be checked exactly rather than
    // approximately. The content is a square, so it is unchanged by the
    // rotation; the blur is along one axis only, so turning it a quarter turn
    // has to give precisely the transpose of the upright picture. Nothing about
    // that depends on a threshold or a tolerance chosen by hand.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut smear = |rotate: f32| {
        let scene = Scene::tree(
            "smear",
            vec![emblema_testkit::Node::Layer {
                layer: Box::new(emblema_testkit::LayerSpec {
                    filter: emblema_core::ImageFilter::blur_xy(10.0, 0.0),
                    ..emblema_testkit::LayerSpec::default()
                }),
                bounds: None,
                transform: emblema_testkit::Transform {
                    rotate,
                    translate: [64.0, 64.0],
                    ..emblema_testkit::Transform::default()
                },
                children: vec![emblema_testkit::Node::Draw(Box::new(
                    emblema_testkit::Item::fill(
                        emblema_testkit::Shape::Rect {
                            min: [-8.0, -8.0],
                            max: [8.0, 8.0],
                        },
                        [1.0, 1.0, 1.0, 1.0],
                    ),
                ))],
            }],
        );
        render::<VulkanHal>(&mut ctx, &scene)
    };
    let upright = smear(0.0);
    let quarter = smear(std::f32::consts::FRAC_PI_2);

    // The upright smear is wide and sharp: a deviation of ten reaches
    // seventeen, and the axis with no deviation stays the square's own size.
    let extent = |img: &Image| {
        let lit: Vec<(u32, u32)> = (0..128u32)
            .flat_map(|y| (0..128u32).map(move |x| (x, y)))
            .filter(|(x, y)| img.pixel(*x, *y)[0] > 20)
            .collect();
        assert!(!lit.is_empty(), "the smear drew nothing");
        (
            lit.iter().map(|p| p.0).max().unwrap() - lit.iter().map(|p| p.0).min().unwrap(),
            lit.iter().map(|p| p.1).max().unwrap() - lit.iter().map(|p| p.1).min().unwrap(),
        )
    };
    let (wide, tall) = extent(&upright);
    assert!(
        wide > tall + 12,
        "upright, the smear should be much wider than it is tall: {wide} by {tall}"
    );
    assert_eq!(
        extent(&quarter),
        (tall, wide),
        "a quarter turn should transpose the smear's extent"
    );

    // And the whole picture, not only its extent. The square is unchanged by a
    // quarter turn, so the two frames have to be each other's transpose.
    let mut differing = 0usize;
    for y in 0..128u32 {
        for x in 0..128u32 {
            if upright.pixel(x, y)[0].abs_diff(quarter.pixel(y, x)[0]) > 2 {
                differing += 1;
            }
        }
    }
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between the upright smear and the transpose \
         of the turned one, so the blur did not turn with the layer"
    );
}

#[test]
fn an_emulated_advanced_blend_leaves_the_clip_in_force() {
    // A mode the hardware cannot do directly is emulated with a pass of its
    // own, and the failure upstream keeps this scene for is that pass leaving
    // the clip behind. The blue rectangle sits entirely outside the clip: if it
    // appears, the clip did not survive the draw before it.
    //
    // Checked by color rather than by area, because the thing that would go
    // wrong shows as a specific twenty-six pixel square in a specific corner
    // and nothing else in the plate is blue.
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "blend/emulated-advanced-blend-restore")
        .expect("the plate is in the catalog");
    // The preferred device is not necessarily the one that can draw this.
    // Advanced blending is an extension a discrete or integrated GPU can lack
    // while the software rasterizer beside it has it, which is the trap
    // `every_catalog_scene_draws_something` records having fallen into.
    let Some(mut ctx) = [DevicePreference::Auto, DevicePreference::Software]
        .into_iter()
        .filter_map(|preference| Validated::new(preference).ok())
        .find(|ctx| scene.supported_by(ctx.capabilities()))
    else {
        eprintln!("skipping: no device here has advanced blending");
        return;
    };
    let img = render::<VulkanHal>(&mut ctx, &scene);

    let blue = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let p = img.pixel(*x, *y);
            p[2] > 150 && p[0] < 100 && p[1] < 120
        })
        .count();
    assert_eq!(
        blue, 0,
        "{blue} pixels are the blue that sits outside the clip, so the clip was \
         lost across the emulated blend"
    );
    // And the advanced draw did land, or the check above passes on an empty
    // plate: the difference against white inside the clip is not white.
    assert_ne!(
        img.pixel(64, 50),
        [255, 255, 255, 255],
        "the difference blend should have darkened the middle of the clip"
    );
}

/// Drop every draw carrying an advanced blend, and say how many went.
fn without_advanced_blends(nodes: &mut Vec<emblema_testkit::Node>) -> usize {
    let mut removed = 0;
    nodes.retain(|node| {
        let keep = match node {
            emblema_testkit::Node::Draw(item) => !item.blend.is_advanced(),
            // A mode on a group is the other half of what `dart:ui` offers, and
            // the whole group goes rather than the mode alone -- for the reason
            // a draw is deleted rather than given a plain mode: substituting
            // one draws the shape, so a dropped advanced composite and a
            // working one both differ from it and the comparison says nothing.
            emblema_testkit::Node::Layer { layer, .. } => !layer.blend.is_advanced(),
            _ => true,
        };
        removed += usize::from(!keep);
        keep
    });
    for node in nodes.iter_mut() {
        if let emblema_testkit::Node::Layer { children, .. } = node {
            removed += without_advanced_blends(children);
        }
    }
    removed
}

#[test]
fn every_plate_that_asks_for_an_advanced_blend_can_show_one() {
    // The failure this exists for was found by hand and was wide: seventeen
    // plates rendered identically to themselves with the blended draw deleted,
    // while the catalog counted every one of them as coverage of a mode.
    //
    // Two separate causes, which is why the check is against *deletion* rather
    // than against substituting a plain mode. Substituting source-over draws
    // the shape, so a dropped advanced draw and a working one both differ from
    // it and the comparison says nothing; deleting the draw is the only version
    // that can tell "this mode did something" from "this mode did nothing".
    //
    // The first cause was the driver. On this machine's Vulkan software
    // rasterizer an advanced blend under multisampling produces no output at
    // all -- correct at one sample, and correct at both on GLES, which is the
    // same Mesa through a different extension. The plates are single-sampled
    // now, which costs them nothing: their subject is what a blend computes,
    // not where an edge falls.
    //
    // The second was two plates that could not have shown their mode on any
    // device. Hue and saturation over a gray backdrop collapse to the backdrop,
    // gray having no saturation to exchange, so the family's gradient is
    // colored now.
    let mut devices: Vec<Validated> = Vec::new();
    for preference in [DevicePreference::Auto, DevicePreference::Software] {
        if let Ok(ctx) = Validated::new(preference) {
            let name = ctx.capabilities().device_name.clone();
            if devices.iter().all(|d| d.capabilities().device_name != name) {
                devices.push(ctx);
            }
        }
    }
    let mut gles = GlesValidated::new(DisplayTarget::Surfaceless).ok();
    let mut checked = 0usize;
    let mut unasked: Vec<&str> = Vec::new();
    for scene in catalog() {
        let mut without = scene.clone();
        if without_advanced_blends(&mut without.items) == 0 {
            continue;
        }
        // Any device here that can show it, rather than the first that claims
        // to support it. A mode on a *group* is drawn as an image quad
        // composited with that mode, and on this machine's Vulkan software
        // rasterizer that draw produces nothing -- while GLES, the same Mesa
        // through a different extension, is correct. It is the second time a
        // driver has answered an advanced blend with an empty frame, and the
        // claim the catalog can honestly make is that some device here shows
        // it. `docs/on-a-board.md` has both.
        let mut shown = false;
        // Counted, because a plate nothing could render and a plate everything
        // rendered identically are the same `false` and are not the same thing.
        // This test asserted on the pair of them and so failed on a Raspberry
        // Pi 5, where *no* device has advanced blending: v3d has not got the
        // extension, the GLES context is the same v3d, and that board's llvmpipe
        // reports it absent too -- 19.1.7, where the one on the machine this was
        // written on is 22.1.8 and has it. Nothing was asked and the message said
        // the mode was not reaching the picture.
        //
        // The skip below was written for exactly that machine and could never
        // run, because the assertion reached it first. So the count is what
        // decides which of the two is true.
        let mut asked = 0usize;
        for ctx in devices.iter_mut() {
            if !scene.supported_by(ctx.capabilities()) {
                continue;
            }
            asked += 1;
            let with = render::<VulkanHal>(ctx, &scene);
            let plain = render::<VulkanHal>(ctx, &without);
            shown |= compare(&with, &plain).expect("same size").differing != 0;
        }
        if let Some(gles) = gles.as_mut() {
            if scene.supported_by(gles.capabilities()) {
                asked += 1;
                let with = render::<GlesHal>(gles, &scene);
                let plain = render::<GlesHal>(gles, &without);
                shown |= compare(&with, &plain).expect("same size").differing != 0;
            }
        }
        if asked == 0 {
            unasked.push(scene.name);
            continue;
        }
        assert!(
            shown,
            "{} renders the same with its advanced blends deleted on every \
             device that can render it, so whatever mode it is named for is not \
             reaching the picture",
            scene.name
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!(
            "skipping: no device here has advanced blending, so none of the {} \
             plates carrying one could be checked",
            unasked.len()
        );
        return;
    }
    // Said rather than passed over, for the machine that has one capable device
    // and not another: the plates it could not ask about are unchecked, and a
    // pass here is a claim about the rest.
    if !unasked.is_empty() {
        eprintln!(
            "skipping: {} plate(s) carrying an advanced blend that no device \
             here can render",
            unasked.len()
        );
    }
    assert!(
        checked + unasked.len() >= 36,
        "only {} plates were found to carry an advanced blend, which is fewer \
         than the catalog has and means the walk missed some",
        checked + unasked.len()
    );
}

#[test]
fn a_points_size_is_its_width_times_the_scale_at_either_extreme() {
    // Upstream keeps the pair and the pair is the point. One plate is a point a
    // ten-thousandth of a unit wide under a millionfold scale; the other is a
    // hundred million units wide under a millionth of one. The product is the
    // same hundred device pixels both times, so the two have to draw the same
    // disc -- and the two ways of getting it wrong are opposite. Sizing the
    // point in device pixels from the width alone draws nothing in the first
    // and fills the frame in the second.
    //
    // Which makes this a test about where the multiplication happens rather
    // than about points: any single plate could be passed by a renderer that
    // had the arithmetic backwards.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let mut disc = |name: &str| {
        let scene = catalog()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"));
        let img = render::<VulkanHal>(&mut ctx, &scene);
        let ground = img.pixel(0, 0);
        (0..128u32)
            .flat_map(|y| (0..128u32).map(move |x| (x, y)))
            .filter(|(x, y)| img.pixel(*x, *y) != ground)
            .count()
    };
    let large = disc("dl/can-draw-scaled-points-large-scale-small-radius");
    let small = disc("dl/can-draw-scaled-points-small-scale-large-radius");

    // A disc of radius fifty is about seven thousand nine hundred pixels, and
    // the frame is sixteen thousand -- so neither "nothing" nor "everything"
    // is anywhere near it.
    for (label, count) in [("large scale", large), ("small scale", small)] {
        assert!(
            (7000..9000).contains(&count),
            "{label} should draw a disc of radius fifty, about seven thousand \
             nine hundred pixels; it drew {count}"
        );
    }
    assert!(
        large.abs_diff(small) * 50 < large,
        "the two extremes are the same width times the same scale and should \
         draw the same disc: {large} against {small}"
    );
}

#[test]
fn a_multi_draw_point_mode_is_clipped_all_at_once() {
    // `Lines` mode makes several draws out of one call, and upstream keeps this
    // scene because they all have to carry the same depth. A clip is tested
    // against depth, so draws given different ones would be cut differently --
    // some segments surviving the circle and some not, out of a single call.
    //
    // Stated as "no blue outside the circle" rather than as a pixel count,
    // because that is the shape the failure would take: a segment that escaped
    // the clip runs off the frame, and one that was over-cut leaves a gap. Both
    // are counted.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "dl/depth-values-for-line-mode")
        .expect("the plate is in the catalog");
    let img = render::<VulkanHal>(&mut ctx, &scene);

    let (mut inside, mut outside) = (0usize, 0usize);
    for y in 0..128u32 {
        for x in 0..128u32 {
            let p = img.pixel(x, y);
            if !(p[2] > 150 && p[0] < 120) {
                continue;
            }
            let (dx, dy) = (x as f32 - 64.0, y as f32 - 64.0);
            // A pixel past the circle's own edge plus the stroke's half width,
            // so the red outline itself is not counted either way.
            if dx.hypot(dy) > 54.0 {
                outside += 1;
            } else {
                inside += 1;
            }
        }
    }
    assert_eq!(
        outside, 0,
        "{outside} pixels of the line run escaped the clip, so the draws that \
         one call produced were not given the same depth"
    );
    // And the run is there at all: five segments six wide across a circle of
    // radius fifty-two cover a good part of it.
    assert!(
        inside > 1500,
        "only {inside} pixels of the line run survived, which is less than five \
         segments across a circle that size"
    );
}

/// A shadow must not depend on which way its caster is wound.
///
/// This is what upstream's shadow file is mostly made of and the half of it a
/// catalog can carry. Upstream draws each caster twice, once each way round,
/// and asserts that the mesh its convex-shadow optimization produced matches
/// the one the general path would have. That assertion needs the optimization
/// and a way to count vertices in a shadow, and this renderer has neither -- so
/// what is left is the property the pairs exist to protect, which is that the
/// two pictures are the same picture.
///
/// Identical rather than within a tolerance, and that is the point of it. Two
/// windings of one polygon are one region, so a renderer that decides coverage
/// from the region cannot tell them apart at all. Anything that reads the
/// direction -- a hull walk that assumes a turn is a left turn, a fan that
/// seeds from the first vertex, a fill rule applied signed -- differs, and
/// differs by whole triangles rather than by an edge sample. So a budget here
/// would only hide the failure it is looking for.
///
/// Paired by name: every `-counter-clockwise-` plate must have a
/// `-clockwise-` sibling, and the count is asserted so that renaming one out
/// of the pattern shows up as this test covering less rather than as nothing.
#[test]
fn a_shadow_does_not_care_which_way_its_caster_is_wound() {
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    let mut pairs = 0usize;
    for scene in catalog() {
        let Some(sibling) = scene.name.strip_prefix("shadow/") else {
            continue;
        };
        let Some(rest) = sibling.split_once("-counter-clockwise-") else {
            continue;
        };
        let clockwise = format!("shadow/{}-clockwise-{}", rest.0, rest.1);
        let other = catalog()
            .into_iter()
            .find(|s| s.name == clockwise)
            .unwrap_or_else(|| panic!("{} has no {clockwise} to pair with", scene.name));

        let a = render::<VulkanHal>(&mut ctx, &scene);
        let b = render::<VulkanHal>(&mut ctx, &other);
        let difference = compare(&a, &b).unwrap_or_else(|e| panic!("{}: {e}", scene.name));
        assert!(
            difference.is_identical(),
            "{} and {clockwise} are the same caster wound the other way and \
             drew different pictures: {}",
            scene.name,
            difference.describe(Tolerance::new(0, 0.0))
        );
        pairs += 1;
    }
    assert_eq!(
        pairs, 4,
        "the shadow chapter should hold four winding pairs; a plate renamed out \
         of the pattern would leave this test quietly covering less"
    );
}

/// A per-vertex coordinate reaches a caller's program.
///
/// This was written down as a thing that could not happen. `draw_vertices`
/// states a coordinate per vertex; every built-in shader can be told to measure
/// from it, and a caller's program was refused, on the reasoning that a program
/// replaces this renderer's fragment shader outright and so leaves the
/// attribute nowhere to arrive.
///
/// It arrives. A program replaces the *fragment* stage, and the vertex stage it
/// runs behind is this renderer's own whatever the fragment does -- so a
/// program declaring the same `VertexOutput`, which it must in order to be a
/// pipeline here at all, receives `uv` beside `clip`.
///
/// Asserted by mirroring rather than by naming a color. The same mesh is drawn
/// twice, once with its coordinates running along its positions and once with
/// them reversed, and what has to hold is that the second is the first turned
/// over. A program reading the clip position would draw the same picture both
/// times; one reading nothing would draw a flat one. This fails differently for
/// each of the ways it could be wrong, which is what
/// `vertices/draw-vertices-texture-coordinates-with-fragment-shader` cannot say
/// on its own -- a plate is one picture, and this is a claim about two.
#[test]
fn a_program_reads_the_coordinate_the_vertices_state() {
    use emblema_core::{ImageFilter, VertexMode};
    use emblema_testkit::scene::{MeshSpec, Node, Transform};
    use emblema_testkit::Fill;

    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };

    let corners = [[8.0, 8.0], [120.0, 8.0], [120.0, 120.0], [8.0, 120.0]];
    let shot = |ctx: &mut Validated, reversed: bool| -> Image {
        let coords = corners
            .iter()
            .map(|p| {
                let along = (p[0] - 8.0) / 112.0;
                [if reversed { 1.0 - along } else { along }, 0.0]
            })
            .collect();
        let scene = Scene::tree(
            "probe",
            vec![Node::Mesh(Box::new(MeshSpec {
                mode: VertexMode::Triangles,
                positions: corners.to_vec(),
                colors: Vec::new(),
                texture_coords: coords,
                indices: vec![0, 1, 2, 0, 2, 3],
                fill: Fill::RuntimeEffect {
                    program: 3,
                    uniforms: emblema_testkit::fixture::effect_uniforms(
                        [1.0, 0.0, 0.0, 1.0],
                        [0.0, 0.0, 1.0, 1.0],
                        0.0,
                    ),
                    images: Vec::new(),
                },
                tint_blend: emblema_hal::BlendMode::Modulate,
                blend: emblema_hal::BlendMode::SrcOver,
                transform: Transform::default(),
                image_filter: ImageFilter::None,
                mask_blur: 0.0,
            }))],
        )
        .with_samples(1);
        render::<VulkanHal>(ctx, &scene)
    };

    let along = shot(&mut ctx, false);
    let reversed = shot(&mut ctx, true);

    let left = along.pixel(20, 64);
    let right = along.pixel(108, 64);
    assert!(
        left[0] > 200 && left[2] < 60,
        "the ramp should start at the first color, got {left:?}"
    );
    assert!(
        right[2] > 200 && right[0] < 60,
        "and end at the second, got {right:?}"
    );

    // Sampled at the same two columns, so a program that ignored the
    // coordinates fails here and nowhere else.
    let mirrored_left = reversed.pixel(20, 64);
    let mirrored_right = reversed.pixel(108, 64);
    assert!(
        mirrored_left[2] > 200 && mirrored_left[0] < 60,
        "reversing the coordinates should put the second color on the left, got \
         {mirrored_left:?}"
    );
    assert!(
        mirrored_right[0] > 200 && mirrored_right[2] < 60,
        "and the first on the right, got {mirrored_right:?}"
    );
}

#[test]
fn a_clip_popped_before_a_backdrop_cuts_does_not_come_back() {
    // What `basic/backdrop-filter-over-unclosed-clip` is for, stated as a
    // number the plate itself cannot.
    //
    // A backdrop filter cannot sample the attachment it is writing, so the pass
    // stops and the narrowings are made again in the one that follows. Which
    // ones is the question: the outer clip is still in force where the layer
    // opens and the two around the corner are not. A rebuild that replayed the
    // batch rather than the stack would put them back, and the blur would then
    // read only the corner -- so the corner's color would stop dead at the
    // clip's edge instead of spreading past it.
    //
    // Sampled to the left of that edge, well inside the outer clip, where the
    // plate draws nothing at all: white if the blur was confined, colored if it
    // reached over the region the outer clip admits.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "basic/backdrop-filter-over-unclosed-clip")
        .expect("the plate is in the catalog");
    let image = render::<VulkanHal>(&mut ctx, &scene);

    // Six pixels left of the corner clip's edge. The plate's sigma of eight
    // reaches thirteen, and the corner arrives here twenty levels down from
    // white -- measured, because how far a blur shows in eight bits is not the
    // radius its kernel is truncated at.
    let bled = image.pixel(90, 112);
    assert!(
        bled[1] < 250 || bled[2] < 250,
        "the blur should have carried the corner past the clip that is no \
         longer in force, and (90, 112) came back {bled:?}"
    );
    // And it is the corner's color arriving rather than the plate being dark
    // everywhere: a point the same distance the other side of the frame's
    // middle has nothing to bleed from and stays white.
    let clean = image.pixel(40, 64);
    assert_eq!(
        clean,
        [255, 255, 255, 255],
        "nothing should reach the middle of the plate"
    );
}

#[test]
fn a_projected_clip_narrows_across_the_plate() {
    // `basic/perspective-rectangle` states its clip after its projection, so the
    // clip's own edges go through the divide and the region it admits is a
    // trapezoid rather than the rectangle written down. That is the whole
    // subject, and a renderer that took the clip in frame axes -- a scissor,
    // say -- would draw the rectangle instead and agree with itself perfectly.
    //
    // So the check is the shape of the region: its lit span has to be wider
    // near the top than near the bottom, by more than a rasterizer's edge.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "basic/perspective-rectangle")
        .expect("the plate is in the catalog");
    let image = render::<VulkanHal>(&mut ctx, &scene);

    // The ground is nearly black and everything inside the clip is flooded with
    // half white, so a lit pixel is any that is not the ground.
    let span = |y: u32| {
        (0..128u32)
            .filter(|x| {
                let p = image.pixel(*x, y);
                p[0] > 60 || p[1] > 60 || p[2] > 100
            })
            .count()
    };
    let (high, low) = (span(16), span(60));
    assert!(
        high > low + 4,
        "the projected clip should narrow down the plate, and spans {high} at \
         y=16 against {low} at y=60"
    );
    // And it is a region rather than nothing: a clip that admitted no pixel
    // would satisfy the inequality above with two zeros.
    assert!(low > 8, "the lower span is {low}, which is not a region");
}

#[test]
fn a_clip_drawn_before_an_image_does_not_bound_it() {
    // `basic/can-draw-perspective-transform-with-clips` puts a clip on each side
    // of a projected image, which is what upstream's own comments say it is for:
    // one is drawn and restored so its geometry sits behind the image, the other
    // scopes the image and sits in front of it.
    //
    // Both are checkable from the picture. The difference clip cut the middle
    // out of the surround and was popped before the image drew, so the image has
    // to be there in the middle -- if that clip reached the image, the image
    // would be missing exactly where the cut was. And the oval is a stencil in
    // frame axes while the image's corners are where the divide put them, so a
    // point inside the image's projected quad and outside the oval has to come
    // back as ground.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let scene = catalog()
        .into_iter()
        .find(|s| s.name == "basic/can-draw-perspective-transform-with-clips")
        .expect("the plate is in the catalog");
    let image = render::<VulkanHal>(&mut ctx, &scene);

    // The two grounds, each under the translucent disc: the square the
    // difference clip spared, and the surround it flooded.
    let square = [0u8, 153, 102, 255];
    let surround = [0u8, 0, 102, 255];
    let is_ground = |p: [u8; 4]| p == square || p == surround;

    assert!(
        !is_ground(image.pixel(64, 64)),
        "the middle should hold the image, and holds {:?} -- the difference clip \
         was popped before the image drew and must not bound it",
        image.pixel(64, 64)
    );
    assert!(
        is_ground(image.pixel(34, 34)),
        "a corner inside the image's quad and outside the oval should be ground, \
         and is {:?}",
        image.pixel(34, 34)
    );
    // And the square is still there, so the difference clip did cut it: without
    // it the surround would be flooded over the lot and there would be no
    // second ground to tell apart.
    assert_eq!(
        image.pixel(30, 64),
        square,
        "the difference clip should have spared the middle of the surround"
    );
}

#[test]
fn a_hairline_holds_its_weight_under_a_transform() {
    // What `aiks_dl_primitive_shape_unittests.cc` exists for, in its own words:
    // "the effects of scaling, rotation and skew transforms on the consistency
    // of a stroke (particularly hairlines)". Its three scenes all stroke at a
    // width of zero, so each is a hairline over a filled shape, and the claim is
    // that the hairline is there and is thin however the shape is stretched.
    //
    // Both halves are checkable without a reference image. The outline has to
    // exist -- a renderer reading zero as no stroke draws none of it, which is
    // what this renderer did until the rule was taken -- and it has to be about
    // one pixel wide, which a scan across the plate counts directly.
    let Ok(mut ctx) = Validated::new(DevicePreference::Auto) else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    for scene in catalog()
        .into_iter()
        .filter(|s| s.name.starts_with("primitive/"))
    {
        let image = render::<VulkanHal>(&mut ctx, &scene);
        // The outline is white over a blue fill and a darkened middle, so it is
        // the only thing in the plate with every channel high.
        let white = |x: u32, y: u32| {
            let p = image.pixel(x, y);
            p[0] > 150 && p[1] > 150 && p[2] > 150
        };
        let lit: Vec<(u32, u32)> = (0..128u32)
            .flat_map(|y| (0..128u32).map(move |x| (x, y)))
            .filter(|(x, y)| white(*x, *y))
            .collect();
        assert!(
            lit.len() > 60,
            "{}: the hairline should trace the inner shape and lit {} pixels",
            scene.name,
            lit.len()
        );

        // And it is thin. Taken as the widest unbroken white run on any row,
        // which for a one-pixel outline is small even where the outline runs
        // along the scan -- upstream's shapes are curves or turned rectangles,
        // so no edge here is exactly horizontal.
        let widest = (0..128u32)
            .map(|y| {
                let mut best = 0u32;
                let mut run = 0u32;
                for x in 0..128u32 {
                    run = match white(x, y) {
                        true => run + 1,
                        false => 0,
                    };
                    best = best.max(run);
                }
                best
            })
            .max()
            .unwrap_or(0);
        assert!(
            widest <= 12,
            "{}: the widest white run is {widest} pixels, which is not a hairline",
            scene.name
        );
    }
}

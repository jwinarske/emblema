//! The C reproducer still describes this renderer's frame.
//!
//! `docs/mesa-blend-instability.c` is the artifact a Mesa bug report carries:
//! the frame loop in EGL and GLES with no Rust in it, reading the two generated
//! shaders from files. Because it is not compiled by anything here, nothing
//! would notice if the paint block it fills in by hand stopped matching the
//! block the shader declares -- it would keep building, keep running, and
//! quietly stop reproducing.
//!
//! So the numbers it hardcodes are checked against the ones this tree defines.
//! Not the whole file: the geometry and the colors are a shrunk case's and mean
//! nothing outside it. What is checked is the contract between a paint block
//! and the shader that reads it, which is the thing that would silently rot.

use emblema_hal::material::{kind, layout};
use emblema_hal::BlendMode;

const REPRODUCER: &str = include_str!("../../../docs/mesa-blend-instability.c");

/// Every `#define` the reproducer gives an offset to, against `layout`.
#[test]
fn the_reproducers_paint_block_matches_the_one_the_shader_declares() {
    let offsets = [
        ("STOPS", layout::STOPS),
        ("GEOMETRY", layout::GEOMETRY),
        ("TO_LOCAL", layout::TO_LOCAL),
        ("PARAMS", layout::PARAMS),
        ("FILTER_PARAMS", layout::FILTER_PARAMS),
        ("MATERIAL_FLOATS", emblema_hal::MATERIAL_FLOATS),
    ];
    for (name, value) in offsets {
        let expected = format!("#define {name} {value}\n");
        assert!(
            REPRODUCER.contains(&expected),
            "the reproducer does not define {name} as {value}; it would fill \
             the paint block at the wrong offset and reproduce nothing"
        );
    }
}

/// The three material kinds it packs, and the tint blend every draw carries.
#[test]
fn the_reproducers_material_kinds_match() {
    let kinds = [
        ("KIND_SOLID", kind::SOLID),
        ("KIND_IMAGE", kind::IMAGE),
        ("KIND_BLUR", kind::BLUR),
        ("TINT_MODULATE", BlendMode::Modulate.code()),
    ];
    for (name, value) in kinds {
        let expected = format!("#define {name} {value:.1}f\n");
        assert!(
            REPRODUCER.contains(&expected),
            "the reproducer does not define {name} as {value}; the shader \
             would dispatch to a different material"
        );
    }
}

/// The two generated names it looks up.
///
/// Both are the translator's rather than the WGSL's -- the block is named after
/// the struct it carries and the sampler after the texture's group and binding
/// -- so both are part of the contract and neither is an implementation
/// detail. The same two names are pinned on the shader crate's side.
#[test]
fn the_reproducer_looks_up_the_generated_names() {
    for name in ["Paint_block_0Fragment", "_group_0_binding_0_fs"] {
        assert!(
            REPRODUCER.contains(name),
            "the reproducer does not name {name}, which it has to look up"
        );
        assert!(
            emblema_shaders::SOLID_FS_GLSL.contains(name),
            "the generated shader no longer declares {name}, so the \
             reproducer's lookup of it is stale"
        );
    }
}

/// And the shader files it reads are the ones the probe prints.
///
/// The reproducer takes the fragment source *without* the blend qualifier and
/// inserts the two lines itself, because it builds both forms of the program
/// the way the renderer does. A generated source that already carried them
/// would declare them twice and fail to compile, so this pins which end does
/// the inserting.
#[test]
fn the_generated_fragment_source_carries_no_blend_qualifier() {
    assert!(
        !emblema_shaders::SOLID_FS_GLSL.contains("blend_support_all_equations"),
        "the generated fragment source now declares the blend qualifier, so \
         the reproducer and `render::with_advanced_blend` would both insert it"
    );
    assert!(
        REPRODUCER.contains("layout(blend_support_all_equations) out;"),
        "the reproducer no longer inserts the qualifier, so an advanced \
         equation would not apply to its composite at all"
    );
}

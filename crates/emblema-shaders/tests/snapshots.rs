//! The translated shaders, pinned.
//!
//! One WGSL source produces the code both backends actually run, and naga
//! produces it. A naga upgrade that changes its output changes what every
//! fragment executes, and nothing else here would say so: the pixels are
//! compared against another implementation of the same translator, so a
//! codegen change that is wrong in the same way on both targets passes every
//! comparison in this workspace. That is the case a snapshot is for.
//!
//! It is not a golden image and makes no claim about correctness. It says the
//! generated code is what it was, so a change to it is something somebody
//! decided rather than something that arrived with a dependency bump.
//!
//! Set `UPDATE_SHADER_SNAPSHOTS=1` to rewrite them, which is the reviewed
//! event: the diff is the thing to look at. Set `EMBLEMA_SHADER_SNAPSHOTS`
//! to say where they are, which a cross-built binary on a board has to.

// The comparison below runs from a helper rather than from the test body, so
// clippy's test-code exemption does not reach it. A snapshot that does not
// match is a failure and a panic is how a test reports one.
#![allow(clippy::panic)]

use std::path::PathBuf;

/// Where the snapshots live, which is outside this crate on purpose.
///
/// A workspace-level directory, because the question they answer is about the
/// dependency the whole workspace shares rather than about this crate.
///
/// `EMBLEMA_SHADER_SNAPSHOTS` overrides it, and the reason is cross
/// compilation. The default is built from `CARGO_MANIFEST_DIR`, which is
/// baked in at compile time and names a path on the machine that did the
/// compiling -- so a binary cross-built here and copied to a board looks for
/// its snapshots under the host's source tree and finds nothing. Every other
/// test in this workspace runs from a bare binary on a board; without this,
/// these two are the only ones that cannot, and a device run can never come
/// out clean.
fn directory() -> PathBuf {
    if let Some(path) = std::env::var_os("EMBLEMA_SHADER_SNAPSHOTS") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/shader-snapshots")
}

/// SPIR-V as words is not reviewable, so it is summarized rather than stored.
///
/// A hash and a length: enough to notice a change, and honest about not being
/// a diff anybody can read. The GLSL beside it is text and is stored whole,
/// which is where an actual review of a codegen change would happen.
fn digest(words: &[u32]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for word in words {
        for byte in word.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{} words, fnv1a {hash:016x}\n", words.len())
}

fn check(name: &str, contents: &str) {
    let path = directory().join(name);
    if std::env::var_os("UPDATE_SHADER_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(directory()).expect("snapshot directory");
        std::fs::write(&path, contents).expect("write snapshot");
        return;
    }
    let Ok(expected) = std::fs::read_to_string(&path) else {
        panic!(
            "{} has no snapshot. Run with UPDATE_SHADER_SNAPSHOTS=1 to write one, \
             and review what it contains.",
            path.display()
        );
    };
    if expected != contents {
        // The first differing line rather than the whole file, since a codegen
        // change moves everything after the first insertion and printing all of
        // it buries the part that matters.
        let at = expected
            .lines()
            .zip(contents.lines())
            .position(|(a, b)| a != b);
        let detail = match at {
            Some(line) => format!(
                "first difference at line {}:\n  was: {}\n  now: {}",
                line + 1,
                expected.lines().nth(line).unwrap_or(""),
                contents.lines().nth(line).unwrap_or("")
            ),
            None => format!(
                "the same up to line {}, then one is longer ({} against {} lines)",
                expected.lines().count().min(contents.lines().count()),
                expected.lines().count(),
                contents.lines().count()
            ),
        };
        panic!(
            "{} changed.\n{detail}\n\nIf this is a naga upgrade or a shader edit, \
             rerun with UPDATE_SHADER_SNAPSHOTS=1 and review the diff.",
            path.display()
        );
    }
}

#[test]
fn the_translated_glsl_is_what_it_was() {
    check("solid.vert.glsl", emblema_shaders::SOLID_VS_GLSL);
    check("solid.frag.glsl", emblema_shaders::SOLID_FS_GLSL);
    // The stand-in for a caller's own program is snapshotted for a different
    // reason from the renderer's shader. Nothing ships it, but every test of
    // the runtime-effect path rests on what it translates to -- so a
    // translator change that altered it would move the ground under those
    // tests rather than under anything a user sees.
    check("effect.frag.glsl", emblema_shaders::EFFECT_FS_GLSL);
    check(
        "effect_image.frag.glsl",
        emblema_shaders::EFFECT_IMAGE_FS_GLSL,
    );
    check(
        "effect_two_images.frag.glsl",
        emblema_shaders::EFFECT_TWO_IMAGES_FS_GLSL,
    );
    check(
        "effect_mesh_uv.frag.glsl",
        emblema_shaders::EFFECT_MESH_UV_FS_GLSL,
    );
    // The list above is by hand, because the constants it names are separate
    // identifiers and nothing can iterate them. This is what stops a shader
    // being added and quietly going unsnapshotted: the count comes from the
    // directory walk `build.rs` does, so a new `.wgsl` fails here until
    // somebody names it.
    assert_eq!(
        snapshotted_shaders(),
        5,
        "the shader directory holds {} sources and this test names five of them: \
         `solid` and the four stand-ins for a caller's program. A shader added \
         to the tree goes unsnapshotted until it is named above, and this is \
         what says so.",
        snapshotted_shaders()
    );
}

/// The WGSL sources the build found, from the same walk that compiled them.
mod sources {
    include!(concat!(env!("OUT_DIR"), "/sources.rs"));
}

fn snapshotted_shaders() -> usize {
    sources::SHADER_SOURCES.len()
}

#[test]
fn the_translated_spirv_is_what_it_was() {
    check("solid.spv.txt", &digest(emblema_shaders::SOLID_SPV));
    check("effect.spv.txt", &digest(emblema_shaders::EFFECT_SPV));
    check(
        "effect_mesh_uv.spv.txt",
        &digest(emblema_shaders::EFFECT_MESH_UV_SPV),
    );
    check(
        "effect_image.spv.txt",
        &digest(emblema_shaders::EFFECT_IMAGE_SPV),
    );
    check(
        "effect_two_images.spv.txt",
        &digest(emblema_shaders::EFFECT_TWO_IMAGES_SPV),
    );
}

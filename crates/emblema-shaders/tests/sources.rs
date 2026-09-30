//! What the WGSL sources say about themselves, before naga sees them.
//!
//! Distinct from the snapshots beside this, which pin what naga *produced*.
//! This asks a question about the input, and it exists because the one thing
//! that would have answered it does not: naga emits an uncalled function into
//! the GLSL rather than dropping it, so a helper nothing calls is code every
//! GLES driver compiles for the life of the program.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it.
#![allow(clippy::panic)]

include!(concat!(env!("OUT_DIR"), "/sources.rs"));

/// Every WGSL source, by file name.
///
/// From the build rather than from the directory, and the reason is a board: a
/// test binary is cross-built and copied to a device with no source tree, where
/// `CARGO_MANIFEST_DIR` names a path on the machine that compiled it. `build.rs`
/// walks the directory once for both jobs, so a shader added to it is covered
/// here without being added anywhere.
fn sources() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = SHADER_SOURCES
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect();
    assert!(!out.is_empty(), "the build emitted no shader sources");
    out.sort();
    out
}

/// Comments name functions, so they have to go before anything is counted.
///
/// This file's own convention makes that necessary rather than tidy: a
/// function's doc comment usually names the function under it, and half the
/// prose in `solid.wgsl` names one function while explaining another.
fn without_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every function is called, or is a stage the pipeline enters at.
///
/// A shader is not a program with a linker that drops what nothing reaches.
/// Measured here: `outline_if_asked` in `solid.wgsl` was written with no
/// caller and stayed that way from the commit that added it, and naga put it
/// in the GLSL every time -- two hundred and seven bytes of source and
/// seventy-eight words of SPIR-V that every driver parsed and no fragment ever
/// ran. `docs/on-a-board.md` records that this shader's cost is a step
/// function of its size, which is what makes that worth a test rather than a
/// tidy-up.
///
/// A chain of dead functions is caught one link at a time: a helper called
/// only from an uncalled function still reads as called here. That is the
/// direction to be wrong in -- removing the root makes the next run name the
/// next one -- and it is the whole of what this does not catch.
#[test]
fn every_function_in_a_shader_is_either_called_or_a_stage() {
    let mut dead = Vec::new();
    for (name, source) in sources() {
        let body = without_comments(&source);
        for (index, line) in body.lines().enumerate() {
            let Some(rest) = line.strip_prefix("fn ") else {
                continue;
            };
            let Some(function) = rest.split('(').next() else {
                continue;
            };
            // A stage is entered by the pipeline rather than called, and is
            // marked as one on the line above.
            let entry = index
                .checked_sub(1)
                .and_then(|previous| body.lines().nth(previous))
                .is_some_and(|previous| {
                    previous.contains("@vertex") || previous.contains("@fragment")
                });
            if entry {
                continue;
            }
            let calls = body.matches(&format!("{function}(")).count() - 1;
            if calls == 0 {
                dead.push(format!("{name}: {function}"));
            }
        }
    }
    assert!(
        dead.is_empty(),
        "these shader functions are never called, and naga emits them anyway:\n  {}",
        dead.join("\n  ")
    );
}

/// Every material kind the renderer can ask for is one the shader answers, and
/// the other way round.
///
/// Two lists that have to agree and nothing making them: `Material::kind` in
/// `emblema-hal` names fourteen codes, and `shade` in `solid.wgsl` decides
/// what to do with a number. A code with no arm draws whatever the fall-through
/// leaves, which is a wrong picture rather than an error. An arm with no code
/// is a branch no fragment reaches, which the test above would not catch --
/// dead code inside a live function rather than a function nothing calls.
///
/// The dispatch is read for the numbers it tests against rather than for its
/// shape, because it has two shapes and has had three. A `switch` names its
/// arms as `case 4:`; the gradients ahead of it are an `if` chain comparing
/// against half-integers, because they share a tail the switch cannot; and the
/// solid case is a bare early return. What is common to all of them is that the
/// number appears, so that is what is looked for.
#[test]
fn the_shader_answers_for_every_material_kind_and_no_others() {
    use emblema_hal::material::kind;

    // Written out rather than derived, because there is nothing to derive
    // from: these are `pub const f32` in a module, and a module is not
    // enumerable. The count is asserted below so that a constant added there
    // and not here is caught rather than silently uncovered.
    let named: [(&str, f32); 14] = [
        ("SOLID", kind::SOLID),
        ("LINEAR", kind::LINEAR),
        ("RADIAL", kind::RADIAL),
        ("SWEEP", kind::SWEEP),
        ("IMAGE", kind::IMAGE),
        ("GLYPH", kind::GLYPH),
        ("BLUR", kind::BLUR),
        ("ROUNDED_RECT", kind::ROUNDED_RECT),
        ("ELLIPSE", kind::ELLIPSE),
        ("CONICAL", kind::CONICAL),
        ("MESH", kind::MESH),
        ("MORPHOLOGY", kind::MORPHOLOGY),
        ("ROUNDED_RECT_BLUR", kind::ROUNDED_RECT_BLUR),
        ("POINT_FIELD", kind::POINT_FIELD),
    ];

    let solid = sources()
        .into_iter()
        .find(|(name, _)| name == "solid.wgsl")
        .expect("solid.wgsl");
    let body = without_comments(&solid.1);
    let dispatch = body
        .split_once("fn shade(")
        .expect("solid.wgsl has a `shade`")
        .1;

    // A kind is answered if the dispatch tests against it: as `case 7:` in the
    // switch, or as the half-integers either side of it in the chain before.
    let answered = |code: f32| {
        let n = code as i32;
        dispatch.contains(&format!("case {n}:"))
            || (dispatch.contains(&format!("kind > {}.5", n - 1))
                && dispatch.contains(&format!("kind < {n}.5")))
            || (n == 0 && dispatch.contains("kind < 0.5"))
    };

    let unanswered: Vec<&str> = named
        .iter()
        .filter(|(_, code)| !answered(*code))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        unanswered.is_empty(),
        "the shader has no arm for {unanswered:?}, so a draw asking for one \
         gets whatever the fall-through leaves"
    );

    // And the other way: every `case` in the switch is a code something can
    // ask for. The chain before it is not read for this, since its bounds are
    // a range rather than a value and a spurious one would have to be written
    // deliberately.
    let mut orphans = Vec::new();
    for (index, _) in dispatch.match_indices("case ") {
        let rest = &dispatch[index + "case ".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(code) = digits.parse::<i32>() else {
            continue;
        };
        if !named.iter().any(|(_, c)| *c as i32 == code) {
            orphans.push(code);
        }
    }
    assert!(
        orphans.is_empty(),
        "the shader has arms for {orphans:?}, which no material kind names -- \
         a branch every driver compiles and no fragment reaches"
    );

    // And the list above is the whole of `kind` rather than a sample of it,
    // which is the assertion that keeps the two before it honest. Read from the
    // module's own source, because a list checked against itself cannot fail:
    // the first version of this compared the highest code it had been given
    // against the length of the list it had been given, and adding a constant
    // to `emblema-hal` passed it.
    //
    // `include_str!` rather than a read, for the reason `sources` gives.
    const MATERIAL: &str = include_str!("../../emblema-hal/src/material.rs");
    let module = MATERIAL
        .split_once("pub mod kind {")
        .expect("a `kind` module")
        .1
        .split_once("\n}")
        .expect("the end of it")
        .0;
    let declared: Vec<&str> = module
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub const "))
        .filter_map(|rest| rest.split(':').next())
        .collect();
    let missing: Vec<&str> = declared
        .iter()
        .filter(|name| !named.iter().any(|(known, _)| known == *name))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "`emblema_hal::material::kind` declares {missing:?}, which the list in \
         this test does not name -- so nothing here asks whether the shader \
         answers for them"
    );
}

/// The paint block, as four shaders declare it and as the renderer packs it.
///
/// This is the most consequential pair of lists in the tree and had nothing
/// holding it together. `emblema_hal::material::layout` names the float offset
/// each part of a material is written at; the `Paint` block in the WGSL names
/// what the shader reads there. A member inserted, reordered or resized on one
/// side and not the other does not fail to build and does not fail to render --
/// every draw simply reads the wrong floats, and what that looks like depends
/// on which kinds a scene happens to use.
///
/// Every source declares it, not two. A caller's fragment program replaces this
/// renderer's shader outright and reads the same uniform block, so the fixture
/// effects declare it too and say in their own comments that they are declaring
/// the same one. Nothing was checking that they still were.
///
/// What is compared is the members and their widths rather than the prose
/// around them, since the comments differ between the four on purpose -- the
/// renderer's block explains the arrangement and a caller's does not need to.
#[test]
fn every_shader_declares_the_paint_block_the_renderer_packs() {
    use emblema_hal::material::{layout, MATERIAL_FLOATS};

    // A member is `name: vec4<f32>,` or `name: array<vec4<f32>, N>,`. Both are
    // four floats a slot, which is the arrangement the block is built on and
    // the reason it can be copied in without writing padding.
    fn members(block: &str) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            let Some((name, rest)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let floats = if rest.contains("array<vec4<f32>") {
                let count: usize = rest
                    .rsplit(',')
                    .nth(1)
                    .and_then(|n| n.trim().trim_end_matches('>').trim().parse().ok())
                    .unwrap_or_else(|| panic!("cannot read the length of `{line}`"));
                4 * count
            } else if rest.contains("vec4<f32>") {
                4
            } else {
                panic!("`{line}` is not a four-component slot, which this block is made of");
            };
            out.push((name.to_string(), floats));
        }
        out
    }

    let mut declarations: Vec<(String, Vec<(String, usize)>)> = Vec::new();
    for (name, source) in sources() {
        let body = without_comments(&source);
        let Some((_, rest)) = body.split_once("struct Paint {") else {
            continue;
        };
        let block = rest.split_once('}').expect("the end of the block").0;
        declarations.push((name, members(block)));
    }
    // Every source declares it, which is the count that matters rather than a
    // number: the renderer's own shader and every stand-in for a caller's,
    // because a program is a pipeline here only if it declares the block the
    // renderer packs.
    assert_eq!(
        declarations.len(),
        sources().len(),
        "every shader declares the paint block; these do not: {:?}",
        sources()
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| !declarations.iter().any(|(n, _)| n == name))
            .collect::<Vec<_>>()
    );

    // `solid.wgsl` is the reference rather than whichever sorted first: it is
    // the renderer's own block, and the other three are a caller's copy of it.
    let reference = &declarations
        .iter()
        .find(|(name, _)| name == "solid.wgsl")
        .expect("solid.wgsl declares the paint block")
        .1;
    for (name, other) in &declarations {
        assert_eq!(
            other, reference,
            "{name} declares the paint block differently from solid.wgsl, so a \
             draw through one reads floats the other did not write"
        );
    }

    // The offsets, cumulative from zero, against what the renderer packs to.
    // The correspondence is by hand because the two vocabularies differ -- the
    // block's `recolor` is the renderer's `FILTER` -- and it is checked for
    // completeness below rather than trusted.
    let expected: [(&str, &str, usize); 8] = [
        ("stops", "STOPS", layout::STOPS),
        ("offsets", "OFFSETS", layout::OFFSETS),
        ("geometry", "GEOMETRY", layout::GEOMETRY),
        ("to_local", "TO_LOCAL", layout::TO_LOCAL),
        ("params", "PARAMS", layout::PARAMS),
        ("recolor", "FILTER", layout::FILTER),
        ("filter_offset", "FILTER_OFFSET", layout::FILTER_OFFSET),
        ("filter_params", "FILTER_PARAMS", layout::FILTER_PARAMS),
    ];
    let mut at = 0usize;
    for (index, (member, floats)) in reference.iter().enumerate() {
        let (declared, constant, offset) = expected
            .get(index)
            .unwrap_or_else(|| panic!("the block has a member `{member}` this test does not name"));
        assert_eq!(member, declared, "member {index} of the paint block");
        assert_eq!(
            at, *offset,
            "`{member}` sits at float {at} in the shader and `layout::{constant}` \
             says {offset}"
        );
        at += floats;
    }
    assert_eq!(
        reference.len(),
        expected.len(),
        "the block declares {} members and this test names {}",
        reference.len(),
        expected.len()
    );
    assert_eq!(
        at, MATERIAL_FLOATS,
        "the block is {at} floats and `MATERIAL_FLOATS` is {MATERIAL_FLOATS}"
    );

    // And every constant in `layout` is accounted for, so one added there is
    // classified rather than quietly uncovered. `DITHER` is the one that names
    // no member of its own: it is two floats inside `filter_params`, which the
    // constant's own documentation says, so what is asserted is that it lands
    // inside that member rather than beside it.
    const MATERIAL: &str = include_str!("../../emblema-hal/src/material.rs");
    let module = MATERIAL
        .split_once("pub mod layout {")
        .expect("a `layout` module")
        .1
        .split_once("\n}")
        .expect("the end of it")
        .0;
    for line in module.lines() {
        let Some(rest) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        let Some(name) = rest.split(':').next() else {
            continue;
        };
        // `DITHER` names no member of its own -- it is two floats inside
        // `filter_params` -- so there is nothing here to match it against. Its
        // place is asserted where both numbers are, beside the constant itself,
        // and at compile time: two constants compared in a test is an
        // assertion that cannot fail.
        if name == "DITHER" {
            continue;
        }
        assert!(
            expected.iter().any(|(_, constant, _)| *constant == name),
            "`layout::{name}` is not matched to a member of the paint block here, \
             so nothing asks whether the shader reads it where the renderer wrote it"
        );
    }
}

/// Every color-filter code the renderer can write is one the shader tells
/// apart from its neighbors.
///
/// A different hazard from the material kinds beside it, and one this module
/// invites. `filtered` dispatches by threshold rather than by arm -- `kind >
/// 2.5` and not `case 3:` -- because most of the codes above a line share a
/// route and the line is what matters. That makes adding a code silent in a way
/// a missing `case` is not: a sixth filter would fall into the fifth's branch
/// and be applied as a blend, with nothing failing anywhere.
///
/// So what is checked is that consecutive codes are separated. For every code
/// but the first there must be a threshold at the half-integer below it, which
/// is the only thing that can tell it from the code beneath.
///
/// And `is_straight` is checked against the shader rather than described as
/// agreeing with it. Its own documentation says the boundary lives there "where
/// both can cite it", which is the right arrangement and was still two
/// expressions with nothing comparing them.
#[test]
fn the_shader_tells_every_color_filter_from_its_neighbors() {
    use emblema_hal::material::filter;

    const MATERIAL: &str = include_str!("../../emblema-hal/src/material.rs");
    let module = MATERIAL
        .split_once("pub mod filter {")
        .expect("a `filter` module")
        .1
        .split_once("\n}")
        .expect("the end of it")
        .0;
    let codes: Vec<(String, f32)> = module
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub const "))
        .filter_map(|rest| {
            let (name, value) = rest.split_once(": f32 = ")?;
            Some((name.to_string(), value.trim_end_matches(';').parse().ok()?))
        })
        .collect();
    assert!(
        codes.len() >= 2,
        "expected the filter codes to be read from the module, got {codes:?}"
    );

    let solid = sources()
        .into_iter()
        .find(|(name, _)| name == "solid.wgsl")
        .expect("solid.wgsl");
    let body = without_comments(&solid.1);
    let dispatch = body
        .split_once("fn filtered(")
        .expect("solid.wgsl has a `filtered`")
        .1
        .split_once("\nfn ")
        .expect("the end of it")
        .0;

    for (name, code) in &codes {
        if *code == 0.0 {
            continue;
        }
        let separator = format!("{}.5", *code as i32 - 1);
        assert!(
            dispatch.contains(&separator),
            "nothing in `filtered` tests against {separator}, so `filter::{name}` \
             is indistinguishable from the code below it and takes its branch"
        );
    }

    // The straight/premultiplied boundary, stated in two places and now
    // compared. The shader reads straight color where it says `kind > 1.5` and
    // has not already returned for the blend above; `is_straight` has to say
    // the same of every code.
    for (name, code) in &codes {
        let shader_says = *code > 1.5 && *code < filter::BLEND;
        assert_eq!(
            filter::is_straight(*code),
            shader_says,
            "`is_straight` and the shader disagree about `filter::{name}`"
        );
    }
}

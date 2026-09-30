//! The parity table, checked against the thing it describes.
//!
//! `docs/parity.md` makes two kinds of claim that can be wrong quietly. It
//! cites corpus scenes as evidence that a row exists, and those citations were
//! verified once, by hand, on the day the table was written; a scene renamed
//! afterwards leaves a claim pointing at nothing. And it counts its own rows in
//! prose, which is a summary of a table sitting directly above it and therefore
//! a summary that can disagree with it.
//!
//! Both were checked by hand once. Doing it by hand once is how a document
//! starts out true and stops being true, and this table is the answer given to
//! anyone asking what the renderer can do -- so a stale citation is worse than
//! no citation, since it reads as evidence.
//!
//! What this does not check is whether a cited scene actually exercises the row
//! it is cited under. That is a judgment about what a picture demonstrates, and
//! no test makes it. The mechanical part is still worth having: it is the part
//! that rots on its own, without anybody touching the document.

// Reached from a helper rather than from a test body, so clippy's test-code
// exemption does not see it. A failed assumption in a test should stop the
// run; the workspace denies these because a *library* must not.
#![allow(clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The workspace root, which is where every path in this file is relative to.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .to_path_buf()
}

fn doc(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("docs")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn parity() -> String {
    doc("parity.md")
}

/// The table rows, as `(feature, status, citations)`.
///
/// A row is a line of four cells. Header and separator lines have that shape
/// too, so they are dropped by their contents rather than by position -- the
/// file holds two tables, and counting lines from the top would only work for
/// the first one.
fn rows(doc: &str) -> Vec<(String, String, Vec<String>)> {
    doc.lines()
        .filter(|line| line.starts_with('|'))
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() != 6 {
                return None;
            }
            let status = cells[2];
            if status == "Status" || status.starts_with("---") {
                return None;
            }
            let cited = cells[4]
                .split(',')
                .filter_map(|c| c.trim().strip_prefix('`')?.strip_suffix('`'))
                .map(str::to_owned)
                .collect();
            Some((cells[1].to_owned(), status.to_owned(), cited))
        })
        .collect()
}

/// Every scene name a citation could refer to.
fn scene_names() -> BTreeSet<String> {
    emblema_testkit::corpus()
        .into_iter()
        .map(|scene| scene.name.to_owned())
        .collect()
}

/// Every Rust source file under `crates`.
fn sources() -> Vec<String> {
    fn walk(dir: &std::path::Path, into: &mut Vec<String>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|e| e == "rs") {
                into.push(std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .to_path_buf();
    let mut files = Vec::new();
    // `xtask` as well as `crates`, because the tests in here are cited by the
    // documents like any other and a citation that resolves to nothing reads
    // the same whichever directory the test lives in.
    for root in [workspace.join("crates"), workspace.join("xtask")] {
        assert!(root.is_dir(), "no directory at {}", root.display());
        walk(&root, &mut files);
    }
    assert!(!files.is_empty(), "no sources found in the workspace");
    files
}

#[test]
fn every_scene_the_parity_table_cites_as_evidence_exists() {
    let scenes = scene_names();
    let sources = sources();
    let mut dangling = Vec::new();

    for (feature, _, cited) in rows(&parity()) {
        for citation in cited {
            // A citation ending in `*` stands for a family -- `gradient-*` is
            // every gradient scene -- and is satisfied by any one of them.
            let found = match citation.strip_suffix('*') {
                Some(prefix) => scenes.iter().any(|name| name.starts_with(prefix)),
                // A row whose evidence is an assertion rather than a picture
                // cites the test by name instead, so a citation is satisfied by
                // either. Matching a definition and not merely the text means a
                // local variable inside an unrelated test does not qualify --
                // which is what three rows here were citing, and how a reader
                // following the citation ended up somewhere that proved
                // something else.
                None => {
                    scenes.contains(&citation)
                        || sources
                            .iter()
                            .any(|src| src.contains(&format!("fn {citation}(")))
                }
            };
            if !found {
                dangling.push(format!("{feature} cites {citation}"));
            }
        }
    }

    assert!(
        dangling.is_empty(),
        "docs/parity.md cites evidence that does not exist:\n  {}\n\
         A citation names a corpus scene or a test function. Either it was \
         renamed and the citation should follow it, or the evidence for that row \
         is gone and the row's status is now a claim with nothing behind it.",
        dangling.join("\n  ")
    );
}

/// English for a number, for numbers a table of features can reach.
fn spell(n: usize) -> String {
    const ONES: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 10] = [
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    match n {
        0..=19 => ONES[n].to_owned(),
        20..=99 if n % 10 == 0 => TENS[n / 10].to_owned(),
        20..=99 => format!("{}-{}", TENS[n / 10], ONES[n % 10]),
        // Hundreds arrived when the catalog passed a hundred plates, which the
        // parity table alone would never have done. The panic below was worded
        // for that table and was reached by the other document -- a reminder
        // that a helper stops being about the thing it was written for the
        // moment a second caller uses it.
        100..=999 if n % 100 == 0 => format!("{} hundred", ONES[n / 100]),
        100..=999 => format!("{} hundred and {}", ONES[n / 100], spell(n % 100)),
        _ => panic!("no document here counts past nine hundred and ninety-nine"),
    }
}

#[test]
fn the_parity_tables_prose_summary_counts_its_own_rows_correctly() {
    let doc = parity();
    let rows = rows(&doc);
    let count = |status: &str| rows.iter().filter(|(_, s, _)| s == status).count();

    // The document is prose and has to read as prose, so a count of one takes a
    // singular verb. Written with `are` throughout, this test would demand a
    // sentence saying "one are partial" -- and the way that gets resolved under
    // time pressure is by writing the ungrammatical sentence, since the test is
    // the thing that has to pass.
    let agreeing = |n: usize, verb: &str| {
        let (singular, plural) = match verb {
            "exist" => ("exists", "exist"),
            _ => ("is", "are"),
        };
        format!("{} {}", spell(n), if n == 1 { singular } else { plural })
    };
    // A category with nothing in it is left out of the sentence rather than
    // counted at zero. Prose does not say "zero are partial"; it stops
    // mentioning the thing that is not there. Demanding otherwise would put
    // this test in the position of requiring a sentence nobody would write,
    // which is how a document ends up serving its checker instead of a reader.
    let mut clauses = Vec::new();
    if count("yes") > 0 {
        clauses.push(agreeing(count("yes"), "exist"));
    }
    for (n, noun) in [
        (count("partial"), "partial"),
        (count("via"), "expressible by a caller who assembles them"),
        (count("no"), "absent"),
        (count("out of scope"), "out of scope"),
    ] {
        if n > 0 {
            clauses.push(format!("{} {noun}", agreeing(n, "be")));
        }
    }
    let listed = match clauses.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{}, and {last}", rest.join(", ")),
        _ => clauses.join(""),
    };
    let claim = format!(
        "Of {} rows across `Canvas` and `Paint`: {listed}.",
        spell(rows.len()),
    );

    // The sentence is wrapped in the source, so compare against the document
    // with its line breaks flattened rather than requiring a particular wrap.
    let flattened = doc.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flattened.contains(&claim),
        "docs/parity.md summarizes its own table wrongly. The table says:\n  {claim}\n\
         Adding or changing a row means changing that sentence too -- it is the \
         only part of the document a reader takes on trust."
    );
}

#[test]
fn a_row_claiming_the_operation_exists_names_the_evidence() {
    // The document states this rule about itself: "A row claiming *yes* with no
    // evidence is a bug in this table." Two rows broke it -- both accessors,
    // both genuinely tested, neither citing the test that covered it -- so the
    // rule was true as a policy and false as a description.
    let uncited: Vec<String> = rows(&parity())
        .into_iter()
        .filter(|(_, status, cited)| status == "yes" && cited.is_empty())
        .map(|(feature, _, _)| feature)
        .collect();

    assert!(
        uncited.is_empty(),
        "docs/parity.md claims these exist without naming what renders or asserts \
         them:\n  {}\n\
         An uncited yes is the failure mode the table was written to avoid: it \
         reads as verified and is only asserted.",
        uncited.join("\n  ")
    );
}

#[test]
fn the_architecture_states_the_material_size_the_code_enforces() {
    // The document said a hundred and twelve bytes and the assertion in the
    // code said a hundred and twenty-eight. Both numbers were written
    // deliberately; one of them stopped being true when the layout grew, and
    // nothing anywhere connected the two.
    //
    // This is the number the whole push-constant argument rests on -- the
    // reason the stop count is four, the reason an image material was a
    // question, the reason a conical gradient had to find a spare float. A
    // reader checking that argument against the wrong figure would conclude
    // there was room to spare.
    let bytes = emblema_hal::MATERIAL_FLOATS * 4;
    let stated = format!("packed into {bytes} bytes");
    let flattened = doc("architecture.md")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        flattened.contains(&stated),
        "docs/architecture.md does not say the material is {bytes} bytes, which is \
         what MATERIAL_FLOATS makes it. Changing the layout means changing the \
         sentence that explains why the layout is the size it is."
    );
}

#[test]
fn the_playground_inventory_counts_the_catalog_correctly() {
    // The other numbers in that document describe a source tree this
    // repository does not contain, and it says so. This one describes the
    // collection right here, which makes it the one number a reader would be
    // entitled to trust -- so it is the one that is checked.
    let total = emblema_testkit::catalog().len();
    let flattened = doc("playground-parity.md")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Anchored on "holds N scenes" rather than the whole sentence it used to
    // sit in. That sentence read "of roughly four hundred", which stopped being
    // true once the catalog passed the column it was being compared against,
    // and a check that pins prose around the number outlives its own wording.
    let stated = format!("holds {} scenes", spell(total));
    assert!(
        flattened.contains(&stated),
        "docs/playground-parity.md does not say the catalog holds {total} scenes. \
         Adding one means saying so, or the inventory stops being an inventory."
    );
}

#[test]
fn the_playground_inventory_counts_each_file_correctly() {
    // The total alone is too forgiving a check: a per-file number drifted by
    // twelve while the total stayed right, because the two were edited
    // separately and only one of them was under test. Every number that
    // describes this repository is checked here, so the table cannot be
    // partially true.
    let by_topic = |topic: &str| {
        emblema_testkit::catalog()
            .iter()
            .filter(|scene| {
                scene
                    .name
                    .split_once('/')
                    .is_some_and(|(prefix, _)| prefix == topic)
            })
            .count()
    };
    // The C++ file each topic mirrors. Primitive shapes used to be absent, on
    // the grounds that the catalog held nothing under that topic and there would
    // be nothing to check -- which is also why the row's count went unchecked
    // and was wrong from the day it was written. A topic with no scenes is worth
    // naming here for exactly that reason, so it is named now that it has three.
    let mirrors = [
        ("aiks_dl_basic_unittests.cc", "basic"),
        ("aiks_dl_primitive_shape_unittests.cc", "primitive"),
        ("aiks_dl_path_unittests.cc", "path"),
        ("aiks_dl_gradient_unittests.cc", "gradient"),
        ("aiks_dl_clip_unittests.cc", "clip"),
        ("aiks_dl_opacity_unittests.cc", "opacity"),
        ("aiks_dl_blend_unittests.cc", "blend"),
        ("aiks_dl_blur_unittests.cc", "blur"),
        ("aiks_dl_vertices_unittests.cc", "vertices"),
        ("aiks_dl_atlas_unittests.cc", "atlas"),
        ("aiks_dl_shadow_unittests.cc", "shadow"),
        ("aiks_dl_unittests.cc", "dl"),
        ("aiks_dl_runtime_effect_unittests.cc", "effect"),
        ("aiks_dl_text_unittests.cc", "text"),
    ];
    let doc = doc("playground-parity.md");
    let mut counted = 0;
    for (file, topic) in mirrors {
        let row = doc
            .lines()
            .find(|line| line.starts_with(&format!("| `{file}` |")))
            .unwrap_or_else(|| panic!("docs/playground-parity.md has no row for {file}"));
        let stated: usize = row
            .split('|')
            .nth(3)
            .and_then(|cell| cell.trim().parse().ok())
            .unwrap_or_else(|| panic!("the {file} row does not state a count: {row}"));
        let actual = by_topic(topic);
        assert_eq!(
            stated, actual,
            "docs/playground-parity.md says {stated} scenes mirror {file}, \
             but the catalog holds {actual} under {topic}/"
        );
        counted += actual;
    }
    let total = emblema_testkit::catalog().len();
    assert_eq!(
        counted, total,
        "the catalog holds {total} scenes but only {counted} fall under a topic \
         the inventory names, so some scene is uncounted by the table"
    );
}

/// A skip has to say "skipping", because that is the word the census counts.
///
/// `cargo xtask verify` exists because a skipped test passes and its reason is
/// discarded, and it finds the reasons by looking for that word. A test that
/// announces itself some other way is therefore counted as having run -- which
/// is the exact condition `verify` was written to end, arrived at from the
/// other side.
///
/// Four sites said "no backend available" and one called itself a note. All
/// five were invisible to the census, and one of them had been since it was
/// written.
///
/// The vocabulary below is small on purpose. It is not trying to recognize
/// every sentence a skip might be written in; it recognizes the ones this
/// workspace has actually used, so a sixth site copied from any existing one
/// is caught.
#[test]
fn a_skip_says_the_word_the_census_counts() {
    const SKIP_LIKE: [&str; 6] = [
        "no backend available",
        "unavailable",
        "not installed",
        "not supported",
        "not built",
        "no device",
    ];
    // Its own walk, because `sources` hands back contents and not paths -- and
    // the first draft of this took them for paths, found no file to open, and
    // passed having read nothing. A check that cannot fail is worse than none,
    // so this one counts what it looked at and says so if that is zero.
    fn test_files(dir: &std::path::Path, into: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                test_files(&path, into);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.display().to_string();
                if name.contains("/tests/") {
                    into.push((name, std::fs::read_to_string(&path).unwrap_or_default()));
                }
            }
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("crates");
    let mut files = Vec::new();
    test_files(&root, &mut files);
    assert!(
        files.len() > 10,
        "found {} test files to scan, which cannot be right",
        files.len()
    );

    let mut bare = Vec::new();
    for (path, text) in files {
        for (number, line) in text.lines().enumerate() {
            let Some(rest) = line.split_once("eprintln!(\"") else {
                continue;
            };
            let Some((message, _)) = rest.1.split_once('"') else {
                continue;
            };
            let lower = message.to_lowercase();
            if lower.contains("skipping") {
                continue;
            }
            if SKIP_LIKE.iter().any(|word| lower.contains(word)) {
                bare.push(format!("{path}:{}: {message}", number + 1));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "these read as skips and do not say \"skipping\", so `cargo xtask \
         verify` counts the tests as having run:\n  {}",
        bare.join("\n  ")
    );
}

#[test]
fn the_tree_is_written_in_american_english() {
    // A stated convention for this project, and one that drifts silently:
    // nothing about "colour" beside "color" fails to compile, and a message a
    // caller reads is as much a part of the interface as the name it belongs
    // to. Eighty-odd of these accumulated before anyone looked, in comments, in
    // test names, and in the text of two errors that sat next to their
    // American twins.
    //
    // Checked by spelling rather than by a dictionary, because the list of
    // words this project actually uses is short and a dictionary would need one
    // anyway. None of these is a substring of an American word, which is what
    // makes an unanchored search the right one -- "recolours" has to be found
    // as surely as "colours".
    //
    // Stems rather than whole words, and that is a fix rather than a tidy-up.
    // The list held "recognise" and "normalise", which are not substrings of
    // "recognising" or "normalisation" -- so every inflected form walked
    // straight past a check named for catching them, and eight did: a
    // "recognising", a "normalising", a "quantise", a "serialise", an
    // "optimisation", a "Honouring", and "grey" in two spellings of the same
    // identifier. Each was in the tree while this test was green.
    //
    // "analyse" stays a whole word on purpose: "analys" is a substring of
    // "analysis", which is spelled that way on both sides of the Atlantic. The
    // same reading turned up "licence", which is the noun in British English
    // and "license" in American for both parts of speech, and four test names
    // spelled in the wrong one.
    //
    // "travell" and "signall" are stems on the same grounds as the rest: the
    // doubled consonant is the difference, so neither is a substring of the
    // American spelling. "judgement" and "analogue" are whole words, since
    // "judgment" and "analog" do not contain them.
    const BRITISH: [&str; 28] = [
        "colour",
        "neighbour",
        "centre",
        "behaviour",
        "recognis",
        "normalis",
        "optimis",
        "organis",
        "serialis",
        "visualis",
        "initialis",
        "prioritis",
        "summaris",
        "quantis",
        "analyse",
        "favour",
        "honour",
        "grey",
        "whilst",
        "licence",
        "catalogue",
        "modelled",
        "labelled",
        "cancelled",
        "travell",
        "signall",
        "judgement",
        "analogue",
    ];
    // Everything tracked that a reader or a caller sees. The two untracked
    // files are excluded by not being here.
    let roots = ["crates", "xtask", "docs", "playground", "tests", ".github"];
    // The walk starts below the repository root, so nothing at the root was
    // read. Named rather than reached by walking the root, which would descend
    // into `.git`.
    let root_files = ["README.md", "CHANGELOG.md", "Cargo.toml", "clippy.toml"];
    let mut found = Vec::new();
    for name in root_files {
        let path = repo_root().join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lower = text.to_lowercase();
        for word in BRITISH {
            if lower.contains(word) {
                found.push(format!("{}: {word}", path.display()));
            }
        }
    }
    for root in roots {
        let mut stack = vec![repo_root().join(root)];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }
                let is_text = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "rs" | "md" | "wgsl" | "toml" | "yml"));
                if !is_text {
                    continue;
                }
                // This file names every word it forbids, so it answers to all
                // of them. A check that has to spell out what it is looking for
                // cannot also be looked in.
                if path.file_name().is_some_and(|n| n == "documentation.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let lower = text.to_lowercase();
                for word in BRITISH {
                    if lower.contains(word) {
                        found.push(format!("{}: {word}", path.display()));
                    }
                }
            }
        }
    }
    found.sort();
    found.dedup();
    assert!(
        found.is_empty(),
        "this project is written in American English, and these are not:\n  {}",
        found.join("\n  ")
    );
}

/// The scanout seam has to stay free of the library behind it.
///
/// `ScanoutOutput` exists so the KMS library can be replaced without this
/// renderer reimplementing KMS logic, and the architecture says as much: the
/// surface is stated as a trait "so the two projects can be sequenced against
/// each other rather than discovering a mismatch at integration". That is only
/// true while the trait, the types in its signatures, and the frame loop that
/// drives it name no type from whichever library currently implements it.
///
/// It is true today and was measured rather than assumed: every mention of
/// `drm::` in the crate is in the two files implementing the trait. This test
/// is here because that is an invariant a single convenient import would end,
/// silently, in a crate where importing it is otherwise ordinary -- and a
/// second implementation is planned, which is exactly when it would be found
/// out the expensive way.
#[test]
fn the_scanout_trait_names_nothing_from_the_library_behind_it() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("crates/emblema-present-drm/src");

    // Where the binding may be named: the implementation of the trait, and the
    // device handling underneath it. Everything else is the seam.
    let implementation = ["kms.rs", "device.rs"];

    let entries = std::fs::read_dir(&crate_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", crate_dir.display()));
    let mut checked = 0;
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("file name")
            .to_string();
        if implementation.contains(&name.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        // Prose may discuss the library; code may not name it. The comment
        // marker is enough to tell them apart here, every mention in this crate
        // being either an import or a path in an expression.
        for (number, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("*") {
                continue;
            }
            assert!(
                !line.contains("drm::"),
                "{name}:{} names the KMS binding, which the seam may not: {}\n\
                 A type from it in the trait, in a type the trait mentions, or \
                 in the frame loop is what would stop a second implementation \
                 from being a drop-in.",
                number + 1,
                line.trim()
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 3,
        "expected the trait, the frame loop and the crate root at least, \
         checked {checked} files"
    );
}

/// The one program outside the workspace has to reach everything it needs
/// through the facade.
///
/// It is the only consumer this repository has, so it is the only evidence that
/// what a caller can reach is enough to write something real with. That
/// evidence is worth nothing if it is allowed to reach past the facade to the
/// crates underneath, which it did for as long as `present-wsi` was a feature
/// that enabled nothing: it named six of them, and the one it could not do
/// without was the swapchain the facade had no route to.
///
/// So the rule is that it names the facade and nothing else from here, with one
/// exception. `emblema-testkit` is the scene corpus this tool exists to look
/// at, not part of the renderer's surface, and a consumer who is not a test
/// harness would never want it.
#[test]
fn the_playground_reaches_everything_through_the_facade() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("playground/Cargo.toml");
    let source = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest.display()));

    let allowed = ["emblema", "emblema-testkit"];
    let mut named = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.starts_with("emblema") {
            named.push(name.to_string());
        }
    }
    assert!(
        !named.is_empty(),
        "the playground names no crate from here, which cannot be right"
    );
    for name in &named {
        assert!(
            allowed.contains(&name.as_str()),
            "the playground depends on {name}, reaching past the facade.\n\
             Whatever it needed from there is missing from `emblema`, and \
             adding the dependency hides that instead of fixing it."
        );
    }
}

/// Every divergence recorded says what it costs.
///
/// `docs/non-parity.md` exists so that a difference from upstream is a decision
/// somebody made and can find. A section that says what differs and why, and
/// stops there, leaves the reader to work out whether it matters -- which is
/// the part they came for and the part hardest to reconstruct later. So each
/// numbered entry has to state an impact, including the ones whose impact is
/// nothing.
#[test]
fn every_non_parity_entry_states_its_impact() {
    let text = doc("non-parity.md");
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if !heading.is_empty() {
                sections.push((heading.clone(), std::mem::take(&mut body)));
            }
            heading = rest.to_string();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !heading.is_empty() {
        sections.push((heading, body));
    }

    let numbered: Vec<_> = sections
        .iter()
        .filter(|(heading, _)| heading.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .collect();
    assert!(
        numbered.len() >= 6,
        "docs/non-parity.md has {} numbered entries, which is too few to be the \
         list it claims to be -- did the headings change shape?",
        numbered.len()
    );
    let silent: Vec<&str> = numbered
        .iter()
        .filter(|(_, body)| !body.contains("Impact"))
        .map(|(heading, _)| heading.as_str())
        .collect();
    assert!(
        silent.is_empty(),
        "docs/non-parity.md records a difference without saying what it costs:\n  {}",
        silent.join("\n  ")
    );

    // And the numbers are a sequence, which nothing checked. Two entries were
    // both numbered thirteen for as long as the second one existed, because an
    // entry is written by copying its neighbor and the heading is the part most
    // easily left behind. The numbers are not decoration: `architecture.md`
    // cites them by number, so a duplicate makes a reference ambiguous and a gap
    // makes one dangle.
    let stated: Vec<usize> = numbered
        .iter()
        .filter_map(|(heading, _)| heading.split('.').next()?.trim().parse().ok())
        .collect();
    assert_eq!(
        stated.len(),
        numbered.len(),
        "a numbered entry in docs/non-parity.md does not start with a number this \
         could read: {:?}",
        numbered.iter().map(|(h, _)| h).collect::<Vec<_>>()
    );
    let expected: Vec<usize> = (1..=stated.len()).collect();
    assert_eq!(
        stated,
        expected,
        "docs/non-parity.md's entries are numbered {stated:?}, which is not one \
         through {}. A duplicate makes every citation of that number ambiguous.",
        stated.len()
    );
}

/// The changelog says of itself that `docs/parity.md` describes what is built
/// "far better than a list could -- both are checked by tests, so neither can
/// drift from the code without failing the build." Four lines below that it
/// kept its own list of two operations it called absent, and went on calling
/// them absent after they were written, because nothing read it.
///
/// So this reads it. Not the whole duplication -- a changelog is allowed to say
/// what an operation's signature used to be, and one paragraph does -- but the
/// one shape that can only ever be wrong: an operation the parity table has
/// verified as built, described here as missing.
#[test]
fn the_changelog_does_not_call_a_built_operation_absent() {
    const ABSENCE: [&str; 5] = ["absent", "absence", "not built", "missing", "unimplemented"];

    let path = repo_root().join("CHANGELOG.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));

    // A cell can name more than one operation, so the names are taken one at a
    // time and with their backticks: `transform` is both a parity row and an
    // ordinary English word, and only the quoted form means the operation.
    let built: Vec<String> = rows(&parity())
        .into_iter()
        .filter(|(_, status, _)| status == "yes")
        .flat_map(|(feature, _, _)| {
            feature
                .split(',')
                .map(|name| name.trim().to_owned())
                .filter(|name| name.starts_with('`') && name.ends_with('`'))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        built.len() >= 40,
        "the parity table yielded {} built rows, which is too few to be the table \
         it was parsed from -- did the columns change shape?",
        built.len()
    );

    let mut stale: Vec<String> = Vec::new();
    for paragraph in text.split("\n\n") {
        let flattened = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
        let lowered = flattened.to_lowercase();
        let Some(word) = ABSENCE.iter().find(|w| lowered.contains(**w)) else {
            continue;
        };
        for feature in &built {
            if flattened.contains(feature.as_str()) {
                stale.push(format!("{feature} -- said to be {word}"));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "CHANGELOG.md describes as unbuilt what docs/parity.md has verified is \
         built:\n  {}\n\
         The parity table owns that claim and a test holds it to the code. A \
         second copy here has nothing holding it to anything, which is how the \
         first one went stale.",
        stale.join("\n  ")
    );
}

/// Two documents count how many of these crates go to a registry, and the
/// manifests decide it.
///
/// This is the shape of claim that goes stale here without anybody touching the
/// sentence: adding a crate moves both numbers, and a crate changing its mind
/// moves one of them and a list of names besides. The README carried "Nothing is
/// released" for as long as that was true and for a while after it was not,
/// which is the same failure one step further along.
///
/// What is checkable offline is the arithmetic and the names, not whether
/// anything reached crates.io -- no test here goes to the network, so the
/// registry's state stays a thing a reader is told rather than shown.
#[test]
fn the_documents_count_the_publishing_crates_correctly() {
    let root = repo_root();
    let mut refusing = Vec::new();
    let mut total = 0usize;
    let mut members: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("reading crates/")
        .map(|entry| entry.expect("a directory entry under crates/").path())
        .collect();
    members.push(root.join("xtask"));
    members.sort();

    for member in members {
        let manifest = member.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        total += 1;
        // `publish = false` is spelled out per crate; everything else inherits
        // `publish = true` from the workspace, which is the direction the
        // workspace comment says it chose deliberately.
        if text.lines().any(|line| line.trim() == "publish = false") {
            let name = member
                .file_name()
                .expect("a member directory has a name")
                .to_string_lossy()
                .into_owned();
            refusing.push(format!("`{name}`"));
        }
    }
    assert!(
        total >= 10 && !refusing.is_empty(),
        "found {total} members and {} refusing, which is not the shape of this \
         workspace -- did the layout change?",
        refusing.len()
    );

    let listed = match refusing.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => refusing.join(""),
    };
    let publishing = total - refusing.len();
    let claim = format!(
        "{} of this workspace's {} crates publish; {listed} refuse",
        spell(publishing),
        spell(total),
    );
    // Lowercased on both sides, because `spell` has no way to know the clause
    // opens a sentence in one document and not the other. Every name in it is
    // a crate's, so nothing case-bearing is lost.
    let flattened = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    assert!(
        flattened(&doc("../README.md")).contains(&claim),
        "README.md counts the publishing crates wrongly. The manifests say:\n  {claim}\n\
         Adding a crate, or one changing its mind about publishing, means \
         changing that sentence too."
    );

    // The changelog says it in its own words for the version it shipped with,
    // so only the two numbers are held in common. A past version's sentence is
    // a record and stays as written; this is here because `0.1.0` was the
    // whole workspace and its sentence is therefore about today's manifests.
    let counted = format!(
        "{} of the workspace's {} crates",
        spell(publishing),
        spell(total)
    );
    assert!(
        flattened(&doc("../CHANGELOG.md")).contains(&counted),
        "CHANGELOG.md counts the publishing crates wrongly. The manifests say:\n  {counted}"
    );
}

/// The README's Rust snippet is the crate's doctest, character for character.
///
/// Nothing compiles a README. That one does not merely risk going stale, it
/// went stale in the way that matters: it created a surface and never destroyed
/// it, while the doctest beside it did. Nothing in this workspace frees a GPU
/// object on drop, so the snippet a reader copies first left a `VkImage` alive
/// at `vkDestroyDevice` -- `VUID-vkDestroyDevice-device-05137`, an error from
/// the validation layer rather than a leak nobody notices.
///
/// So the two are held equal and the doctest is the one that compiles. Exact
/// equality rather than something cleverer about which lines matter: the
/// difference that mattered here was one line present in one copy, which any
/// comparison forgiving enough to ignore formatting would also have ignored.
#[test]
fn the_readme_snippet_is_the_crate_doctest() {
    /// The fenced block's body, given the line that opens it.
    ///
    /// The closing fence is derived from the opener rather than assumed: in a
    /// doc comment every line carries `//!`, the closing one included, so a
    /// search for a fence at the start of a line finds nothing.
    fn fenced(text: &str, opener: &str) -> String {
        let close = match opener.split_once("```") {
            Some((prefix, _)) => format!("\n{prefix}```"),
            None => panic!("`{opener}` is not a fence"),
        };
        let body = text
            .split_once(&format!("{opener}\n"))
            .unwrap_or_else(|| panic!("no block opening with `{opener}`"))
            .1;
        let end = body
            .find(&close)
            .unwrap_or_else(|| panic!("`{opener}` is never closed"));
        body[..end].to_owned()
    }

    let readme = fenced(&doc("../README.md"), "```rust");
    let lib = std::fs::read_to_string(repo_root().join("crates/emblema/src/lib.rs"))
        .expect("reading the facade crate");
    // Stripped back to what rustdoc compiles: the `//!` that makes it a doc
    // comment, and the `#` lines it hides from a reader. Those hidden lines are
    // why this is not a plain string comparison -- the doctest needs an `Ok` to
    // close over its `?`, and a README showing one would be teaching noise.
    let raw = fenced(&lib, "//! ```no_run");
    let doctest: Vec<&str> = raw
        .lines()
        .map(|line| line.strip_prefix("//!").unwrap_or(line))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .filter(|line| !line.starts_with("# "))
        .collect();

    assert!(
        readme.contains("destroy_surface"),
        "the README's snippet creates a surface and never destroys it, which is \
         the defect this test exists for: nothing here frees a GPU object on \
         drop, so the surface outlives its device and the validation layer says \
         so."
    );
    assert_eq!(
        readme,
        doctest.join("\n"),
        "README.md's Rust block and the doctest in crates/emblema/src/lib.rs \
         have drifted apart. They are one snippet in two places and only the \
         doctest is compiled, so the README is the copy that can be wrong. \
         Change both, or neither."
    );
}

/// A run of spaces inside a string literal is alignment or it is a mistake.
///
/// Rust's line continuation swallows the newline *and* the indentation after
/// it, which is why a long message can be written across several lines and
/// still read as one sentence. Drop the backslash and nothing complains: the
/// literal keeps the indentation, and the message reaches a caller with
/// eighteen spaces in the middle of it. One did, in the refusal for asking to
/// draw into an sRGB target, and it read as one sentence in the source the
/// whole time.
///
/// The rule below is what separates the two cases. Padding inside a literal is
/// legitimate when it lines up columns, and something with columns in it has
/// lines: `xtask report` writes rows like `"  dma-buf           import {}"`. A
/// run of spaces in a literal with no newline in it is aligning nothing.
///
/// That is narrower than the mistake, deliberately. A backslash deleted where
/// it stands leaves a real newline in the literal followed by the source's
/// indentation, and this does not catch that: a newline followed by spaces is
/// how several messages here lay out a list, and the shortest indent a dropped
/// backslash leaves is not far above the longest one that is meant. Both
/// instances found so far were the flattened form, which has no such ambiguity
/// -- so this catches the shape it can be sure about rather than guessing at
/// the other, and a reader should not take a pass here as saying no message is
/// wrapped oddly.
#[test]
fn no_message_carries_a_run_of_spaces_where_a_line_continuation_was_dropped() {
    let mut flattened: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for source in sources() {
        for literal in string_literals(&source) {
            seen += 1;
            if !literal.contains('\n') && literal.trim().contains("   ") {
                flattened.push(literal);
            }
        }
    }
    // A scanner that found nothing would pass this and say nothing, which is
    // the shape of check this file exists to distrust. Both instances the rule
    // caught were found by running it, so the floor is set where a scanner that
    // still walks the tree stays above it and one that stopped does not.
    assert!(
        seen > 3_000,
        "the scanner found {seen} string literals across the workspace, which is \
         too few to have read it -- the check would pass whatever the sources say"
    );
    assert!(
        flattened.is_empty(),
        "these string literals carry a run of spaces and have no line to align \
         to, which is what a dropped `\\` leaves behind:\n  {}",
        flattened.join("\n  ")
    );
}

/// Every ordinary string literal in a source file, as Rust will have it.
///
/// Escapes are resolved far enough for the caller above and no further: `\n`
/// becomes a newline so a literal with lines in it can be recognized, a line
/// continuation eats the newline and the indentation after it the way the
/// compiler does, and every other escape yields the character it escaped so
/// that `\"` cannot end a literal early.
///
/// Comments are skipped, both kinds, because a quotation mark inside one would
/// otherwise open a literal that runs to the next unrelated quote and swallow
/// the source between them. Raw strings are skipped rather than returned: the
/// tree's are shader source and SQL-shaped fixtures, none of which is a message
/// anyone reads, and taking them properly means matching hash counts.
fn string_literals(source: &str) -> Vec<String> {
    let bytes: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            '/' if bytes.get(i + 1) == Some(&'/') => {
                while i < bytes.len() && bytes[i] != '\n' {
                    i += 1;
                }
            }
            '/' if bytes.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == '*' && bytes.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            'r' if matches!(bytes.get(i + 1), Some('"') | Some('#')) => {
                let mut hashes = 0;
                let mut j = i + 1;
                while bytes.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if bytes.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let rest: String = bytes[j + 1..].iter().collect();
                i = match rest.find(&close) {
                    Some(at) => j + 1 + rest[..at].chars().count() + close.chars().count(),
                    None => bytes.len(),
                };
            }
            // A lifetime or a character literal, not the start of a string.
            '\'' => {
                i += if bytes.get(i + 1) == Some(&'\\') {
                    4
                } else {
                    3
                }
            }
            '"' => {
                i += 1;
                let mut literal = String::new();
                while i < bytes.len() && bytes[i] != '"' {
                    if bytes[i] != '\\' {
                        literal.push(bytes[i]);
                        i += 1;
                        continue;
                    }
                    i += 1;
                    match bytes.get(i) {
                        Some('n') => literal.push('\n'),
                        // The continuation: the newline goes, and so does the
                        // indentation that follows it. This is the whole point.
                        Some('\n') => {
                            i += 1;
                            while bytes.get(i).is_some_and(|c| c.is_whitespace()) {
                                i += 1;
                            }
                            continue;
                        }
                        Some(other) => literal.push(*other),
                        None => break,
                    }
                    i += 1;
                }
                out.push(literal);
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

#[test]
fn the_timing_baseline_records_the_commit_it_was_checked_against() {
    // `cargo xtask gate` counts the commits touching what the bench times since
    // that line, and says so beside the skip census. It is the cheapest thing
    // that would have caught the failure it exists for: sixteen renderer
    // commits once went by between two board runs, and the ten and a half per
    // cent they cost was invisible from every diff and every green gate.
    //
    // Checked here because the line is a comment in a data file, which nothing
    // else would notice the loss of. A baseline that has stopped saying when it
    // was checked reports "current" forever.
    // Every baseline in the directory, not the one that happened to exist when
    // this was written. A second board arrived and its file was held to nothing:
    // the convention is the directory's, so the test walks it.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/bench-baselines");
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&dir)
        .expect("the baseline directory")
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "txt") {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let text = std::fs::read_to_string(&path).expect("a baseline");
        let sha = text
            .lines()
            .find_map(|line| line.strip_prefix("# Last checked against the board: "))
            .unwrap_or_else(|| {
                panic!(
                    "{name} should record the commit a board run last passed against; \
                     see `baseline_drift` in xtask"
                )
            })
            .trim();
        assert!(
            sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "{name}'s checked-at line should hold a commit hash, and holds {sha:?}"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "no baselines found in {}, so this read nothing and would pass on anything",
        dir.display()
    );
}

/// A document making claims about upstream says when it last read them.
///
/// `docs/non-parity.md` argues this at length in its own voice -- a parity
/// decision is worth exactly as much as the source it was read from, and "it
/// was checked" is not a fact a later reader can act on without a date. Then
/// nothing checked that the line was there, which is the same gap the timing
/// baseline had before the test above it.
///
/// It is worth more than tidiness. The audit that added the line to
/// `docs/parity.md` found two of its three upstream claims wrong, both in prose
/// that was read as a checklist and never re-derived. A date is what turns
/// "someone should check these" into a thing a reader can see is overdue.
///
/// The date is not compared against anything. Staleness is a judgment about how
/// fast upstream moves, not a threshold, and a test that failed on a date would
/// only teach people to bump it.
#[test]
fn a_document_claiming_something_about_upstream_says_when_it_last_read_it() {
    for name in ["parity.md", "non-parity.md"] {
        let text = doc(name);
        let date = text
            .lines()
            .find_map(|line| {
                let line = line.trim_start_matches("**");
                line.strip_prefix("Last re-read: ")
            })
            .unwrap_or_else(|| {
                panic!(
                    "docs/{name} makes claims about upstream and should carry a \
                     `**Last re-read: YYYY-MM-DD**` line saying when they were \
                     last read at tip"
                )
            });
        let date: String = date.chars().take(10).collect();
        let parts: Vec<&str> = date.split('-').collect();
        assert!(
            parts.len() == 3
                && parts[0].len() == 4
                && parts[1].len() == 2
                && parts[2].len() == 2
                && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())),
            "docs/{name}'s re-read line should hold a YYYY-MM-DD date, and holds {date:?}"
        );
    }
}

/// The ratios `docs/architecture.md` reasons from are the ones the board
/// recorded.
///
/// That document's distance-field section reaches a design conclusion -- that
/// the analytic field costs more than the triangles on a tiler, against what
/// the same passage used to say -- and it reaches it from quotients of the
/// timing baseline. A quotient in prose is exactly the kind of number that goes
/// stale silently: re-recording the baseline is a reviewed event with its own
/// diff, and nothing about that diff points at a paragraph three files away
/// that was reading it.
///
/// So the quotients are recomputed here from
/// `tests/bench-baselines/raspberry-pi-5-v3d.txt` and matched against what the
/// prose says, to the two decimals it states them at. The arrow form in the
/// table -- `0.98× → 2.40×` -- is read for its right-hand side: the left is
/// what the board said before the fragment shader stopped doing work nobody
/// asked for, and is history rather than a claim about now.
#[test]
fn the_ratios_here_are_the_ones_the_baseline_records() {
    let baseline = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/bench-baselines/raspberry-pi-5-v3d.txt"),
    )
    .expect("the board baseline");

    let median = |device: &str, configuration: &str| -> f32 {
        baseline
            .lines()
            .filter(|line| !line.starts_with('#'))
            .find_map(|line| {
                let mut fields = line.split('\t');
                let named = fields.next()?;
                let config = fields.next()?;
                if !named.starts_with(device) || config != configuration {
                    return None;
                }
                fields.next()?.parse().ok()
            })
            .unwrap_or_else(|| panic!("no {device} row for {configuration}"))
    };

    let text = doc("architecture.md");
    // Named as the document names them, so a failure says which row of which
    // table to look at rather than which quotient of which pair.
    let expected = [
        (
            "vulkan",
            "Vulkan field over tessellated at one sample",
            "distance field, 1 sample",
            "tessellated, 1 sample",
        ),
        (
            "gles",
            "GLES field over tessellated at one sample",
            "distance field, 1 sample",
            "tessellated, 1 sample",
        ),
        (
            "vulkan",
            "Vulkan cost of four samples",
            "tessellated, 4 samples",
            "tessellated, 1 sample",
        ),
        (
            "gles",
            "GLES cost of four samples",
            "tessellated, 4 samples",
            "tessellated, 1 sample",
        ),
        (
            "vulkan",
            "Vulkan field over tessellated at four samples",
            "distance field, 1 sample",
            "tessellated, 4 samples",
        ),
        (
            "gles",
            "GLES field over tessellated at four samples",
            "distance field, 1 sample",
            "tessellated, 4 samples",
        ),
    ];
    for (device, what, over, under) in expected {
        let ratio = median(device, over) / median(device, under);
        let stated = format!("{ratio:.2}×");
        assert!(
            text.contains(&stated),
            "the baseline puts {what} at {stated}, and docs/architecture.md does \
             not say that anywhere -- a row moved and the reasoning above it did not"
        );
    }
}

/// A test named in prose is a test that exists.
///
/// `docs/parity.md`'s evidence column has been checked for a while, because a
/// row whose evidence has been renamed is a row with nothing behind it. The
/// documents cite tests everywhere else too -- the architecture cites the one
/// that caught a convention change, the playground inventory cites the ones
/// that make a plate mean something -- and none of those was checked. One had
/// already gone stale: the architecture named
/// `a_mask_blurs_deviation_is_in_device_pixels`, which was renamed in `3e505eb`
/// and left a reader following the citation nowhere.
///
/// A citation is recognized by this tree's own naming convention rather than by
/// where it appears: a backticked identifier with four or more underscores is a
/// test here, and a field, a method or a lint is not. That is a heuristic and
/// the two exceptions it needs are listed below rather than hidden in the
/// pattern -- both are symbols in somebody else's library, which is exactly the
/// case the convention cannot distinguish.
#[test]
fn every_test_the_documents_name_exists() {
    // Names from outside this tree that happen to look like tests here. Kept
    // short on purpose: a growing list is a sign the rule below has stopped
    // fitting, rather than a reason to keep adding to it.
    const FOREIGN: [&str; 3] = [
        // A lint, in `Cargo.toml`.
        "unsafe_op_in_unsafe_fn",
        // libgbm, named where the DRM presentation path explains what it asks
        // that library for.
        "gbm_bo_create_with_modifiers",
        // The Vulkan loader, named where the architecture explains what it
        // does before a driver is reached.
        "loader_get_icd_and_device",
    ];

    let sources = sources().join("\n");
    let mut dangling = Vec::new();
    let mut checked = 0usize;
    let mut documents = vec![
        ("README.md", doc("../README.md")),
        ("CHANGELOG.md", doc("../CHANGELOG.md")),
    ];
    for name in [
        "architecture.md",
        "parity.md",
        "non-parity.md",
        "playground-parity.md",
        "on-a-board.md",
    ] {
        documents.push((name, doc(name)));
    }

    for (name, text) in &documents {
        let mut rest = text.as_str();
        while let Some(open) = rest.find('`') {
            rest = &rest[open + 1..];
            let Some(close) = rest.find('`') else { break };
            let cited = &rest[..close];
            rest = &rest[close + 1..];
            let looks_like_a_test = cited.len() > 8
                && cited.matches('_').count() >= 4
                && cited
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && !cited.starts_with('_')
                && !cited.ends_with('_');
            if !looks_like_a_test || FOREIGN.contains(&cited) {
                continue;
            }
            checked += 1;
            if !sources.contains(&format!("fn {cited}(")) {
                dangling.push(format!("{name} names {cited}"));
            }
        }
    }

    assert!(
        checked >= 40,
        "only {checked} test citations were found across the documents, which is \
         fewer than there are and means the walk missed some"
    );
    assert!(
        dangling.is_empty(),
        "these documents name tests that do not exist:\n  {}\n\
         Either the test was renamed and the prose should follow it, or what the \
         prose says was checked is no longer checked by anything.",
        dangling.join("\n  ")
    );
}

/// A path the documents name is a path that is there.
///
/// The same question as the citations above, asked of files instead of tests,
/// and it found the same kind of answer: three references written relative to
/// somewhere other than the repository root. Two of them named `tests/cost.rs`
/// and `tests/cost-baseline.txt`, which is the worst version of the mistake --
/// there *is* a `tests/` directory at the root, holding the shader snapshots
/// and the board baselines, so a reader following those went somewhere real and
/// found the file absent.
///
/// Only extensions this repository actually uses are checked. The documents
/// cite upstream's files constantly -- `gaussian_blur_filter_contents.cc`,
/// `dl_dispatcher.cc`, a `.frag` here and there -- and those live in a tree
/// nobody has locally, which is the whole reason `docs/non-parity.md` says to
/// read them at tip through `gh`. Extension is what tells the two apart without
/// a list of exceptions to maintain.
#[test]
fn every_path_the_documents_name_is_there() {
    const OURS: [&str; 5] = [".rs", ".md", ".wgsl", ".toml", ".txt"];

    let root = repo_root();
    let mut dangling = Vec::new();
    let mut checked = 0usize;
    let mut documents = vec![
        ("README.md", doc("../README.md")),
        ("CHANGELOG.md", doc("../CHANGELOG.md")),
    ];
    for name in [
        "architecture.md",
        "parity.md",
        "non-parity.md",
        "playground-parity.md",
        "on-a-board.md",
    ] {
        documents.push((name, doc(name)));
    }

    for (name, text) in &documents {
        let mut rest = text.as_str();
        while let Some(open) = rest.find('`') {
            rest = &rest[open + 1..];
            let Some(close) = rest.find('`') else { break };
            let cited = &rest[..close];
            rest = &rest[close + 1..];
            // A path rather than a name: it has to say where, or there is
            // nothing here to resolve. A bare `canvas.rs` is prose.
            if !cited.contains('/') || !OURS.iter().any(|e| cited.ends_with(e)) {
                continue;
            }
            if cited.contains(' ') || cited.starts_with('/') {
                continue;
            }
            checked += 1;
            if !root.join(cited).exists() {
                dangling.push(format!("{name} names {cited}"));
            }
        }
    }

    assert!(
        checked >= 40,
        "only {checked} paths were found across the documents, which is fewer \
         than there are and means the walk missed some"
    );
    assert!(
        dangling.is_empty(),
        "these documents name files that are not there:\n  {}\n\
         Either the file moved and the prose should follow it, or the path was \
         written relative to somewhere other than the repository root.",
        dangling.join("\n  ")
    );
}

/// The documents count the DRM crate's tests correctly.
///
/// Two documents state how many tests `emblema-present-drm` has, split three
/// ways, because the split is the claim: the ones taking DRM master are the ones
/// that need a card, and a reader deciding whether a board run was complete
/// counts them. Nothing checked those numbers, so they went stale in the
/// understating direction -- `five` after a sixth master test landed, and
/// `eleven unit` after eight more did. An understated count is the worse
/// direction, since it reads as a gap someone might set out to fill.
#[test]
fn the_documents_count_the_drm_tests_correctly() {
    /// Lines that are exactly the attribute, so a mention inside prose or a
    /// string does not count as a test.
    fn tests_in(path: &std::path::Path) -> usize {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.trim() == "#[test]")
            .count()
    }

    let crate_root = repo_root().join("crates/emblema-present-drm");
    let master = tests_in(&crate_root.join("tests/kms.rs"));
    let scanout = tests_in(&crate_root.join("tests/scanout.rs"));
    let unit = {
        let mut total = 0;
        let mut stack = vec![crate_root.join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    total += tests_in(&path);
                }
            }
        }
        total
    };

    // The lesson `a_skip_says_the_word_the_census_counts` records: a check that
    // read nothing would pass every assertion below.
    assert!(
        master > 0 && scanout > 0 && unit > 0,
        "counted {master} master, {scanout} scanout and {unit} unit tests, \
         so this read the wrong paths and would pass on anything"
    );
    let total = master + scanout + unit;

    let flat = |name: &str| {
        doc(name)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let architecture = flat("architecture.md");
    let board = flat("on-a-board.md");

    for (name, text, stated) in [
        (
            "docs/architecture.md",
            &architecture,
            format!("{} tests drive a real display controller", spell(master)),
        ),
        (
            "docs/architecture.md",
            &architecture,
            format!("{} tests in `emblema-present-drm`", spell(master)),
        ),
        (
            "docs/architecture.md",
            &architecture,
            format!(
                "all {} tests pass, along with the {} scanout and unit tests",
                spell(master),
                spell(scanout + unit)
            ),
        ),
        (
            "docs/on-a-board.md",
            &board,
            format!(
                "{} tests in `emblema-present-drm` pass on the pi 5",
                spell(total)
            ),
        ),
        (
            "docs/on-a-board.md",
            &board,
            format!("{} unit,", spell(unit)),
        ),
        (
            "docs/on-a-board.md",
            &board,
            format!("{} scanout,", spell(scanout)),
        ),
        (
            "docs/on-a-board.md",
            &board,
            format!("the {} that take drm master", spell(master)),
        ),
    ] {
        assert!(
            text.contains(&stated),
            "{name} does not say \"{stated}\". The crate has {master} tests \
             taking DRM master, {scanout} scanout and {unit} unit, {total} in \
             all. Adding one means saying so."
        );
    }
}

/// The VisionFive 2's frame decomposition says what the baseline recorded.
///
/// Nothing read that baseline before this. Every figure quoted from that board
/// -- and the section this guards derives four more from them by subtraction --
/// was prose that no test could contradict, which is where the mistakes in this
/// tree live. The stage table is the worst case of it: three of its four numbers
/// exist only to be subtracted from each other, so a single stale row makes the
/// attribution wrong while every individual figure still looks plausible.
#[test]
fn the_board_frame_stages_say_what_the_baseline_recorded() {
    let baseline = |name: &str| {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/bench-baselines")
                .join(name),
        )
        .unwrap_or_else(|e| panic!("reading {name}: {e}"))
    };

    let median = |text: &str, device: &str, configuration: &str| -> f64 {
        text.lines()
            .filter(|line| !line.starts_with('#'))
            .find_map(|line| {
                let mut fields = line.split('\t');
                let named = fields.next()?;
                if !named.starts_with(device) || fields.next()? != configuration {
                    return None;
                }
                fields.next()?.parse().ok()
            })
            .unwrap_or_else(|| panic!("no {device} row for {configuration}"))
    };

    let vf2 = baseline("visionfive-2-powervr.txt");
    let ground = median(&vf2, "vulkan:0 PowerVR", "frame, gradient ground");
    let cards = median(&vf2, "vulkan:0 PowerVR", "frame, plus cards");
    let shadows = median(&vf2, "vulkan:0 PowerVR", "frame, plus shadows");
    let whole = median(&vf2, "vulkan:0 PowerVR", "full frame, mixed content");

    let text = doc("on-a-board.md");
    let says = |what: &str, needle: String| {
        assert!(
            text.contains(&needle),
            "on-a-board.md no longer says {what} is {needle:?}; the baseline now \
             records ground {ground}, cards {cards}, shadows {shadows}, whole \
             {whole}"
        );
    };

    // The table's own column, and then the differences it is there to support.
    says("the ground", format!("| {ground:.3} ms | 1 |"));
    says("the cards row", format!("| plus cards | {cards:.3} | 4 |"));
    says(
        "the shadows row",
        format!("| plus shadows | {shadows:.3} | 7 |"),
    );
    says("the whole frame", format!("| {whole:.3} | 12 |"));
    says("the ground's share", format!("**{ground:.3}**"));
    says("what the cards cost", format!("| {:.3} |", cards - ground));
    says(
        "what the shadows cost",
        format!("| {:.3} |", shadows - cards),
    );
    says(
        "what the layer costs",
        format!("| {:.3} |", whole - shadows),
    );

    // The prose restates three of those as rounded figures, which is where a
    // stale number hides best: it reads fine and contradicts the table above it.
    says(
        "the ground's percentage",
        format!("is {:.0} per cent of the frame", ground / whole * 100.0),
    );
    says(
        "the cards and shadows together",
        format!("together are {:.1} ms", shadows - ground),
    );
    says(
        "the layer in prose",
        format!("over the top is {:.1}", whole - shadows),
    );

    // And the cross-board ratio, which spans two files.
    let pi = baseline("raspberry-pi-5-v3d.txt");
    let v3d = median(&pi, "vulkan:0 V3D", "full frame, mixed content");
    says(
        "the ratio against V3D",
        format!("{:.1} times V3D", whole / v3d),
    );

    // The Pi 5's own stage rows, recorded 2026-09-30, and the shares the prose
    // derives from them. Added because the claim the section now rests on is a
    // *cross-board* one -- that a full-screen gradient dominates the frame on two
    // unrelated tile architectures -- and half of it was unchecked while only the
    // VisionFive 2's half was pinned.
    for (device, label) in [("vulkan:0 V3D", "Pi 5 Vulkan"), ("gles V3D", "Pi 5 GLES")] {
        let ground = median(&pi, device, "frame, gradient ground");
        let cards = median(&pi, device, "frame, plus cards");
        let shadows = median(&pi, device, "frame, plus shadows");
        let frame = median(&pi, device, "full frame, mixed content");
        let row = format!(
            "| {label} | {ground:.3}{} | {cards:.3} | {shadows:.3} | {frame:.3} | **{:.1}%** |",
            if device.starts_with("vulkan") {
                " ms"
            } else {
                ""
            },
            ground / frame * 100.0,
        );
        assert!(
            text.contains(&row),
            "on-a-board.md no longer carries the {label} stage row the \
             baseline records:\n  want {row}"
        );
        // The shapes' share, which the prose states to one decimal for each board.
        let shapes = (shadows - ground) / frame * 100.0;
        assert!(
            text.contains(&format!("{shapes:.1}")),
            "on-a-board.md does not state that the cards and shadows are \
             {shapes:.1} per cent of the {label} frame"
        );
    }
}

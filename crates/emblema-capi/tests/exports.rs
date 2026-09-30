//! What `libimpeller` actually exports.
//!
//! The crate documents how much of the upstream C API it implements, and a
//! sentence like that rots the moment somebody adds an entry point without
//! reading it. This makes the claim checkable: the exported set is compared
//! against a list written down here, so growing the surface means saying so.
//!
//! It is not the parity check the crate wants and does not pretend to be. That
//! one compares this library against a pinned copy of upstream's header and
//! fails on entry points upstream has and this does not; it needs the header,
//! which is not vendored. This one only fixes the surface against itself, which
//! catches an accidental export and a status line drifting from reality —
//! and nothing about whether the surface is the right one.

/// Every symbol this library is meant to export, in the header's spelling.
///
/// Add to this only alongside the entry point itself, and only when it is
/// implemented. An exported symbol that returns nothing useful is worse than an
/// absent one: a consumer links successfully and fails at run time with no
/// diagnostic, where a missing symbol fails at link time and names itself.
const EXPECTED: &[&str] = &["ImpellerGetVersion"];

/// An empty list would make the comparison below pass against a library that
/// exports nothing, so it is ruled out where it can be: at compile time.
const _: () = assert!(!EXPECTED.is_empty());

/// The built shared library, next to the test binary's directory.
///
/// Test binaries live in `<profile>/deps`, so the artifact is one level up.
/// Locating it this way rather than by a hard-coded profile means the test
/// works the same under a release run.
fn library() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile = exe.parent()?.parent()?;
    for name in ["libimpeller.so", "libimpeller.dylib", "impeller.dll"] {
        let path = profile.join(name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Exported text symbols, or `None` where they cannot be read.
fn exported(path: &std::path::Path) -> Option<Vec<String>> {
    let output = std::process::Command::new("nm")
        .args(["-D", "--defined-only"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let kind = fields.nth(1)?;
                // `T` is a defined function in the text section, which is what
                // an entry point is. Data symbols and the linker's own are not
                // part of the API surface.
                (kind == "T").then(|| fields.next().map(str::to_string))?
            })
            // Everything the platform's own runtime contributes is not this
            // library's API, and filtering by the prefix the header uses is
            // both what identifies ours and what a consumer would look for.
            .filter(|name| name.starts_with("Impeller"))
            .collect(),
    )
}

#[test]
fn the_exported_surface_is_exactly_what_is_documented() {
    let Some(path) = library() else {
        eprintln!("skipping: the shared library was not built beside this test");
        return;
    };
    let Some(mut found) = exported(&path) else {
        eprintln!("skipping: nm is not available to read {}", path.display());
        return;
    };
    found.sort();
    let mut expected: Vec<String> = EXPECTED.iter().map(|s| s.to_string()).collect();
    expected.sort();

    let missing: Vec<&String> = expected.iter().filter(|s| !found.contains(s)).collect();
    let extra: Vec<&String> = found.iter().filter(|s| !expected.contains(s)).collect();
    assert!(
        missing.is_empty(),
        "documented but not exported: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "exported but not documented: {extra:?} — add them to EXPECTED alongside \
         the entry point, and say so in the crate's status"
    );
}

#[test]
fn the_library_exports_something_at_all() {
    // A build that produced no entry points would satisfy the comparison above
    // against an empty list, and would be a library nobody could use. The list
    // is kept non-empty at compile time; this is the other half.
    let Some(path) = library() else {
        return;
    };
    let Some(found) = exported(&path) else {
        return;
    };
    assert!(
        !found.is_empty(),
        "{} exports no Impeller entry point at all",
        path.display()
    );
}

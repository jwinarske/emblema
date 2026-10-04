//! What a release needs before it is pushed to the registry.
//!
//! Fourteen crates publish from this workspace and a crate cannot be published
//! before the ones it depends on, so a release is an ordered sequence and
//! nothing recorded the order. Working it out by hand is where a release goes
//! wrong, and the failure is public: a version on the registry can be yanked
//! but never removed.
//!
//! What this prints is the order and the state of each crate against the
//! registry's own names. What the tests below enforce is the part that can
//! break silently between releases, which is not the order -- a cycle is a
//! compile error -- but the two ways a manifest can make a crate unpublishable
//! while the workspace still builds:
//!
//! - **A dependency on a crate that refuses to publish.** `emblema-testkit`,
//!   `emblema-capi` and `xtask` do. Depending on one normally would mean
//!   publishing something that names a crate the registry does not have.
//! - **A *dev*-dependency on one, carrying a version.** This is the trap, and
//!   it is a trap because the tidy spelling is the broken one. Every other
//!   dependency in this workspace is `{ workspace = true }`, and the workspace
//!   table gives `emblema-testkit` a version -- so writing it that way asks the
//!   registry for `emblema-testkit 0.1.0`, which does not exist and will not.
//!   `emblema-present` spells it `{ path = "../emblema-testkit" }` instead,
//!   with no version, and cargo strips a dev-dependency like that when it
//!   packages. Measured: adding the version form to `emblema-geometry` fails
//!   `cargo package` with "failed to select a version for the requirement
//!   `emblema-testkit = ^0.1.0`"; the path form packages 19 files.
//!
//! # What this does not do
//!
//! It does not publish, and it does not run `cargo package`. Packaging a crate
//! whose workspace dependencies are not yet on the registry at this version
//! fails by construction -- the names are reserved at 0.0.1 and the tree is at
//! 0.1.0 -- so the check would report a problem that publishing in order is
//! what fixes. Verify the leaf, publish in order, and let each step make the
//! next one resolvable.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A workspace member, as its manifest describes it.
pub struct Member {
    pub name: String,
    pub publishes: bool,
    /// Names of workspace crates this depends on, by section.
    pub normal: BTreeSet<String>,
    pub dev: BTreeSet<String>,
    /// Dev-dependencies on workspace crates that carry a version requirement,
    /// whether written out or inherited from the workspace table.
    pub versioned_dev: BTreeSet<String>,
}

/// The repository root, from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits one level below the root")
        .to_path_buf()
}

/// Every member under `crates/`, plus `xtask`.
///
/// Read as text rather than through `cargo metadata`, for the reason
/// `dependencies.rs` gives about itself: this crate has no JSON parser, and
/// taking a serialization dependency to check the release policy would be a
/// dependency added to check dependencies.
pub fn members(root: &Path) -> Vec<Member> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("reading crates/")
        .map(|entry| entry.expect("a directory entry under crates/").path())
        .collect();
    paths.push(root.join("xtask"));
    paths.sort();

    let names: BTreeSet<String> = paths
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();

    paths
        .iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(path.join("Cargo.toml")).ok()?;
            Some(parse(path, &text, &names))
        })
        .collect()
}

/// Which section of a manifest a line is in, for the three that name crates.
#[derive(PartialEq, Clone, Copy)]
enum Section {
    Normal,
    Dev,
    Other,
}

fn parse(path: &Path, text: &str, names: &BTreeSet<String>) -> Member {
    let name = path
        .file_name()
        .expect("a member directory has a name")
        .to_string_lossy()
        .into_owned();
    let mut member = Member {
        name,
        // `publish = false` is spelled out per crate; everything else inherits
        // `publish = true` from the workspace.
        publishes: !text.lines().any(|line| line.trim() == "publish = false"),
        normal: BTreeSet::new(),
        dev: BTreeSet::new(),
        versioned_dev: BTreeSet::new(),
    };

    let mut section = Section::Other;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // Target-specific tables carry the kind in their own suffix:
            // `[target.'cfg(unix)'.dev-dependencies]`.
            section = if trimmed.contains("dev-dependencies") {
                Section::Dev
            } else if trimmed.contains("dependencies") {
                // Build dependencies count as normal: a published crate's
                // build script needs them from the registry too.
                Section::Normal
            } else {
                Section::Other
            };
            continue;
        }
        if section == Section::Other || trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let Some((key, rest)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !names.contains(key) {
            continue;
        }
        match section {
            Section::Normal => {
                member.normal.insert(key.to_owned());
            }
            Section::Dev => {
                member.dev.insert(key.to_owned());
                // `workspace = true` inherits the workspace table's entry,
                // which carries a version for every crate in this repository.
                if rest.contains("workspace = true") || rest.contains("version") {
                    member.versioned_dev.insert(key.to_owned());
                }
            }
            Section::Other => {}
        }
    }
    member
}

/// The order to publish in: every crate after the ones it depends on.
///
/// `Err` names the crates left over, which means a cycle. Cargo would refuse to
/// build one, so this is a shape the workspace cannot reach -- reported rather
/// than asserted because a function that returns an order should say when it
/// has none.
pub fn order(members: &[Member]) -> Result<Vec<String>, Vec<String>> {
    let publishing: BTreeMap<&str, &Member> = members
        .iter()
        .filter(|m| m.publishes)
        .map(|m| (m.name.as_str(), m))
        .collect();

    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::with_capacity(publishing.len());
    while out.len() < publishing.len() {
        let ready: Vec<&str> = publishing
            .iter()
            .filter(|(name, m)| {
                !done.contains(**name)
                    && m.normal
                        .iter()
                        .filter(|d| publishing.contains_key(d.as_str()))
                        .all(|d| done.contains(d))
            })
            .map(|(name, _)| *name)
            .collect();
        if ready.is_empty() {
            return Err(publishing
                .keys()
                .filter(|n| !done.contains(**n))
                .map(|n| (*n).to_owned())
                .collect());
        }
        for name in ready {
            done.insert(name.to_owned());
            out.push(name.to_owned());
        }
    }
    Ok(out)
}

/// Print the order and what each step needs, for whoever runs the release.
pub fn report() -> String {
    let root = repo_root();
    let members = members(&root);
    let mut out = String::new();

    let refusing: Vec<&str> = members
        .iter()
        .filter(|m| !m.publishes)
        .map(|m| m.name.as_str())
        .collect();
    let publishing = members.iter().filter(|m| m.publishes).count();
    out.push_str(&format!(
        "{publishing} crates publish; {} refuse: {}\n\n",
        refusing.len(),
        refusing.join(", ")
    ));

    match order(&members) {
        Ok(order) => {
            out.push_str("Publish in this order, waiting for each to appear on the\n");
            out.push_str("registry before the next -- a crate cannot be published\n");
            out.push_str("before the ones it depends on:\n\n");
            for (i, name) in order.iter().enumerate() {
                out.push_str(&format!("  {:2}. cargo publish -p {name}\n", i + 1));
            }
        }
        Err(left) => {
            out.push_str(&format!(
                "No order: {} crates depend on each other in a cycle: {}\n",
                left.len(),
                left.join(", ")
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape this module assumes, so a layout change fails here rather
    /// than producing an empty report that passes.
    #[test]
    fn the_workspace_still_looks_like_itself() {
        let members = members(&repo_root());
        assert!(
            members.len() >= 15,
            "found {} members, which is not this workspace",
            members.len()
        );
        let publishing = members.iter().filter(|m| m.publishes).count();
        assert!(
            publishing >= 10 && publishing < members.len(),
            "{publishing} of {} publish, which is not this workspace's shape",
            members.len()
        );
    }

    /// A release has an order.
    #[test]
    fn the_publishable_crates_can_be_ordered() {
        let members = members(&repo_root());
        let order = order(&members).expect("no cycle among publishable crates");
        assert_eq!(
            order.len(),
            members.iter().filter(|m| m.publishes).count(),
            "the order should name every publishing crate once"
        );
        // The leaf first: it is the one that can be published against the
        // registry as it stands, and the check a release starts from.
        assert_eq!(
            order.first().map(String::as_str),
            Some("emblema-geometry"),
            "the order should start at a crate with no workspace dependencies"
        );
    }

    /// Nothing that publishes depends on something that does not.
    #[test]
    fn no_published_crate_needs_an_unpublished_one() {
        let members = members(&repo_root());
        let refusing: BTreeSet<&str> = members
            .iter()
            .filter(|m| !m.publishes)
            .map(|m| m.name.as_str())
            .collect();
        let mut wrong = Vec::new();
        for member in members.iter().filter(|m| m.publishes) {
            for dep in &member.normal {
                if refusing.contains(dep.as_str()) {
                    wrong.push(format!("{} depends on {dep}", member.name));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "a crate that publishes cannot name one that does not; the registry \
             has no such crate:\n  {}",
            wrong.join("\n  ")
        );
    }

    /// And a dev-dependency on one carries no version.
    ///
    /// The trap this file exists for. Cargo strips a path-only dev-dependency
    /// when it packages, so `{ path = "../emblema-testkit" }` publishes;
    /// `{ workspace = true }` asks the registry for a version that does not
    /// exist and fails. The second spelling is the one every other dependency
    /// in this workspace uses, which is what makes it easy to write.
    #[test]
    fn a_dev_dependency_on_an_unpublished_crate_names_no_version() {
        let members = members(&repo_root());
        let refusing: BTreeSet<&str> = members
            .iter()
            .filter(|m| !m.publishes)
            .map(|m| m.name.as_str())
            .collect();
        let mut wrong = Vec::new();
        for member in members.iter().filter(|m| m.publishes) {
            for dep in &member.versioned_dev {
                if refusing.contains(dep.as_str()) {
                    wrong.push(format!(
                        "{}'s dev-dependency on {dep} carries a version",
                        member.name
                    ));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "a dev-dependency on a crate that does not publish has to be path-only, \
             or `cargo package` asks the registry for a version nothing will ever \
             have:\n  {}\nWrite it as `{{ path = \"../<crate>\" }}`.",
            wrong.join("\n  ")
        );
    }

    /// The parser reads the sections it claims to.
    ///
    /// Against text rather than the tree, so a manifest that happens to be
    /// right today cannot make this pass.
    #[test]
    fn the_parser_separates_the_sections() {
        let names: BTreeSet<String> = ["emblema-hal", "emblema-testkit", "emblema-geometry"]
            .into_iter()
            .map(String::from)
            .collect();
        let member = parse(
            Path::new("/tmp/emblema-present"),
            r#"
[package]
name = "emblema-present"

[dependencies]
emblema-hal = { workspace = true }
# emblema-geometry = { workspace = true }

[dev-dependencies]
emblema-testkit = { path = "../emblema-testkit" }

[target.'cfg(unix)'.dev-dependencies]
emblema-geometry = { workspace = true }
"#,
            &names,
        );
        assert!(member.publishes);
        assert_eq!(
            member.normal.iter().map(String::as_str).collect::<Vec<_>>(),
            ["emblema-hal"],
            "a commented-out dependency is not one"
        );
        assert_eq!(
            member.dev.iter().map(String::as_str).collect::<Vec<_>>(),
            ["emblema-geometry", "emblema-testkit"],
            "a target-specific dev table is a dev table"
        );
        assert_eq!(
            member
                .versioned_dev
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["emblema-geometry"],
            "only the workspace-inherited one carries a version"
        );
    }

    /// `publish = false` is what makes a crate refuse.
    #[test]
    fn a_crate_refusing_to_publish_is_read_as_refusing() {
        let names = BTreeSet::new();
        let refusing = parse(
            Path::new("/tmp/emblema-testkit"),
            "[package]\nname = \"emblema-testkit\"\npublish = false\n",
            &names,
        );
        assert!(!refusing.publishes);
        let publishing = parse(
            Path::new("/tmp/emblema-hal"),
            "[package]\nname = \"emblema-hal\"\npublish.workspace = true\n",
            &names,
        );
        assert!(publishing.publishes);
    }
}

//! Whether this machine can run the direct-scanout lane, and what it would use.
//!
//! The scanout path needs a KMS device it can become master of. Whether one
//! exists, and whether anything else already holds it, is the difference
//! between a suite that runs and a suite that is skipped — and it is a question
//! about the machine rather than about the code, which is why it belongs in a
//! tool rather than in a test that would silently pass by skipping.
//!
//! Everything read here is readable without privilege. Acquiring master is not
//! attempted: doing so on a card a compositor is driving would be a rude thing
//! for a reporting command to do, and failing to acquire it proves only that
//! something else had it a moment ago.

use std::fmt::Write as _;
use std::path::Path;

/// A DRM node and what the kernel says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The device file, as it would be opened.
    pub path: String,
    /// Whether this node can modeset, or only render.
    ///
    /// A render node has no connectors and cannot scan out, which is exactly
    /// the distinction that decides whether the lane can use it.
    pub modeset: bool,
    pub driver: Option<String>,
    /// Connector name and status, as the kernel reports them.
    pub connectors: Vec<(String, String)>,
}

impl Node {
    /// Whether this node has something plugged in to scan out to.
    pub fn has_connected_output(&self) -> bool {
        self.connectors
            .iter()
            .any(|(_, status)| status == "connected")
    }
}

/// What the machine offers the scanout lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Survey {
    pub nodes: Vec<Node>,
    /// Whether the virtual KMS driver is loaded.
    pub vkms_loaded: bool,
    /// Whether it is present to be loaded.
    pub vkms_available: bool,
}

/// Read what the kernel exposes, under the given roots.
///
/// The roots are parameters rather than constants so the reading can be
/// exercised against a directory laid out by a test. What this returns on a
/// machine is not something a test can assert about; what it returns for a
/// given tree is.
pub fn survey(dev_dri: &Path, sys_drm: &Path, modules: &Path, module_dir: &Path) -> Survey {
    let mut nodes = Vec::new();

    let mut names: Vec<String> = std::fs::read_dir(dev_dri)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with("card") || name.starts_with("renderD"))
        .collect();
    names.sort();

    for name in names {
        let modeset = name.starts_with("card");
        let sys = sys_drm.join(&name);
        nodes.push(Node {
            path: dev_dri.join(&name).to_string_lossy().into_owned(),
            modeset,
            // The symlink's last component is the driver's name, which is what
            // identifies the hardware more usefully than the node number does.
            driver: std::fs::read_link(sys.join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
            connectors: if modeset {
                connectors_of(sys_drm, &name)
            } else {
                Vec::new()
            },
        });
    }

    Survey {
        nodes,
        // Read from the module list rather than by running a command, so this
        // needs nothing on the path and cannot be confused by one that is
        // shadowed.
        vkms_loaded: std::fs::read_to_string(modules)
            .map(|s| s.lines().any(|line| line.starts_with("vkms ")))
            .unwrap_or(false),
        vkms_available: module_dir.join("kernel/drivers/gpu/drm/vkms").exists(),
    }
}

/// Connectors belonging to one card, as `cardN-NAME` directories beside it.
fn connectors_of(sys_drm: &Path, card: &str) -> Vec<(String, String)> {
    let prefix = format!("{card}-");
    let mut found: Vec<(String, String)> = std::fs::read_dir(sys_drm)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let connector = name.strip_prefix(&prefix)?.to_string();
            let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
            Some((connector, status.trim().to_string()))
        })
        .collect();
    found.sort();
    found
}

/// What a card's primary plane says it can scan out.
///
/// Read from the device rather than from sysfs, because this is the half of
/// format negotiation the display supplies and nothing else exposes it. A
/// failure is reported inline rather than propagated: the rest of the report is
/// still worth printing, and why one card would not answer is itself the
/// finding.
fn scanout_formats(path: &str) -> String {
    use emblema_present_drm::device::DrmDevice;

    let formats = match DrmDevice::open(path).and_then(|d| d.scanout_formats()) {
        Ok(formats) => formats,
        Err(e) => return format!("  scanout formats  unavailable: {e}\n"),
    };
    if formats.is_empty() {
        return "  scanout formats  none advertised\n".to_string();
    }
    let mut out = String::new();
    for set in &formats {
        // The fourcc as its four characters, which is how anyone reading a
        // modifier table or a driver source will recognize it.
        let code = set.fourcc.0.to_le_bytes();
        let name: String = code
            .iter()
            .map(|b| {
                if b.is_ascii_graphic() {
                    *b as char
                } else {
                    '?'
                }
            })
            .collect();
        let _ = writeln!(
            out,
            "  {name:<16} {} modifier(s){}",
            set.modifiers.len(),
            if set.modifiers.contains(&emblema_hal::Modifier::LINEAR) {
                ", linear among them"
            } else {
                ""
            }
        );
    }
    out
}

/// A human-readable report, and what it means for the lane.
pub fn text(survey: &Survey) -> String {
    let mut out = String::new();

    if survey.nodes.is_empty() {
        out.push_str("no DRM nodes; this machine has no graphics device the kernel exposes\n\n");
    }
    for node in &survey.nodes {
        let kind = if node.modeset { "modeset" } else { "render" };
        let driver = node.driver.as_deref().unwrap_or("unknown driver");
        let _ = writeln!(out, "{} ({kind}, {driver})", node.path);
        for (connector, status) in &node.connectors {
            let _ = writeln!(out, "  {connector:<16} {status}");
        }
        if node.modeset {
            out.push_str(&scanout_formats(&node.path));
        }
    }

    let _ = writeln!(
        out,
        "\nvkms              {}",
        match (survey.vkms_loaded, survey.vkms_available) {
            (true, _) => "loaded",
            (false, true) => "available, not loaded",
            (false, false) => "not present in this kernel",
        }
    );

    // The conclusion, stated rather than left to be inferred from the lines
    // above. This is the question the command was run to answer.
    out.push('\n');
    let scanout: Vec<&Node> = survey
        .nodes
        .iter()
        .filter(|n| n.modeset && n.has_connected_output())
        .collect();
    if survey.vkms_loaded {
        out.push_str(
            "The scanout lane can run against vkms, which needs no display and \n\
             takes no card away from anything using one.\n",
        );
    } else if scanout.is_empty() {
        out.push_str(
            "No modeset node has a connected output. The scanout lane needs one, \n\
             or vkms.\n",
        );
    } else {
        let _ = writeln!(
            out,
            "The scanout lane would need master on one of: {}.\n\
             A running compositor holds it, so this generally means a bare VT, or\n\
             loading vkms instead.",
            scanout
                .iter()
                .map(|n| n.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !survey.vkms_loaded && survey.vkms_available {
        out.push_str("\n  sudo modprobe vkms\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a directory tree standing in for what the kernel exposes.
    fn fixture(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("emblema-xtask-drm-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("dev")).expect("fixture");
        fs::create_dir_all(root.join("sys")).expect("fixture");
        root
    }

    fn add_card(root: &Path, name: &str, driver: &str, connectors: &[(&str, &str)]) {
        fs::write(root.join("dev").join(name), b"").expect("node");
        let sys = root.join("sys").join(name);
        fs::create_dir_all(sys.join("device")).expect("sys");
        // The driver is a symlink whose target's last component is its name,
        // which is how the kernel presents it.
        let target = root.join("sys").join("drivers").join(driver);
        fs::create_dir_all(&target).expect("driver");
        std::os::unix::fs::symlink(&target, sys.join("device/driver")).expect("symlink");
        for (connector, status) in connectors {
            let dir = root.join("sys").join(format!("{name}-{connector}"));
            fs::create_dir_all(&dir).expect("connector");
            fs::write(dir.join("status"), status).expect("status");
        }
    }

    fn read(root: &Path, modules: &Path, module_dir: &Path) -> Survey {
        survey(&root.join("dev"), &root.join("sys"), modules, module_dir)
    }

    #[test]
    fn a_render_node_is_distinguished_from_a_modeset_one() {
        // The distinction decides whether the lane can use the node at all: a
        // render node has no connectors and cannot scan out.
        let root = fixture("kinds");
        add_card(&root, "card0", "amdgpu", &[("DP-1", "connected")]);
        fs::write(root.join("dev/renderD128"), b"").expect("node");

        let survey = read(&root, Path::new("/nonexistent"), Path::new("/nonexistent"));
        assert_eq!(survey.nodes.len(), 2);
        let card = &survey.nodes[0];
        let render = &survey.nodes[1];
        assert!(card.modeset && card.path.ends_with("card0"));
        assert!(!render.modeset && render.path.ends_with("renderD128"));
        assert!(render.connectors.is_empty());
        assert_eq!(card.driver.as_deref(), Some("amdgpu"));
        assert!(card.has_connected_output());
    }

    #[test]
    fn a_card_with_nothing_plugged_in_is_not_a_scanout_candidate() {
        let root = fixture("disconnected");
        add_card(
            &root,
            "card0",
            "i915",
            &[("HDMI-A-1", "disconnected"), ("Writeback-1", "unknown")],
        );
        let survey = read(&root, Path::new("/nonexistent"), Path::new("/nonexistent"));
        assert!(!survey.nodes[0].has_connected_output());

        let text = text(&survey);
        assert!(
            text.contains("No modeset node has a connected output"),
            "{text}"
        );
    }

    #[test]
    fn a_loaded_vkms_is_reported_as_the_way_to_run_the_lane() {
        // The point of vkms is that it needs no display and takes no card away
        // from whatever is using one, so where it is loaded it is the answer
        // regardless of what else the machine has.
        let root = fixture("vkms-loaded");
        add_card(&root, "card0", "amdgpu", &[("DP-1", "connected")]);
        let modules = root.join("proc-modules");
        fs::write(
            &modules,
            "vkms 40960 0 - Live 0x0\nsomething 1 0 - Live 0x0\n",
        )
        .expect("modules");

        let survey = read(&root, &modules, Path::new("/nonexistent"));
        assert!(survey.vkms_loaded);
        let text = text(&survey);
        assert!(text.contains("can run against vkms"), "{text}");
        assert!(
            !text.contains("modprobe"),
            "it suggested loading what is loaded"
        );
    }

    #[test]
    fn a_module_whose_name_merely_starts_with_vkms_is_not_vkms() {
        // `vkms_helper` would match a substring search and is a different
        // module. The list is columns, so the name ends at the first space.
        let root = fixture("vkms-lookalike");
        let modules = root.join("proc-modules");
        fs::write(&modules, "vkms_helper 4096 0 - Live 0x0\n").expect("modules");
        assert!(!read(&root, &modules, Path::new("/nonexistent")).vkms_loaded);
    }

    #[test]
    fn an_available_but_unloaded_vkms_comes_with_the_command_to_load_it() {
        let root = fixture("vkms-available");
        add_card(&root, "card0", "amdgpu", &[("DP-1", "connected")]);
        let module_dir = root.join("modules");
        fs::create_dir_all(module_dir.join("kernel/drivers/gpu/drm/vkms")).expect("module dir");

        let survey = read(&root, Path::new("/nonexistent"), &module_dir);
        assert!(survey.vkms_available && !survey.vkms_loaded);
        let text = text(&survey);
        assert!(text.contains("available, not loaded"), "{text}");
        assert!(text.contains("modprobe vkms"), "{text}");
        // And it says what the alternative costs, rather than only offering
        // the module: a compositor holds master on a card that has a display.
        assert!(text.contains("master"), "{text}");
    }

    #[test]
    fn a_machine_with_no_graphics_device_says_so() {
        let root = fixture("empty");
        let survey = read(&root, Path::new("/nonexistent"), Path::new("/nonexistent"));
        assert!(survey.nodes.is_empty());
        let text = text(&survey);
        assert!(text.contains("no DRM nodes"), "{text}");
    }
}

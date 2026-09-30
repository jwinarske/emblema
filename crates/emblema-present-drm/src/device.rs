//! Reading what a KMS device advertises.
//!
//! This is the half of the scanout path that needs no privilege. Enumerating
//! connectors, modes and planes, and reading the format and modifier lists a
//! plane advertises, all work on a card another process is driving — which is
//! what makes them checkable on an ordinary desktop, where becoming master is
//! not something a test may do.
//!
//! It is deliberately **not** a [`ScanoutOutput`]. That trait also imports
//! buffers, commits, and reads events, none of which work without master, and a
//! type that implemented half of it would be a thing whose other half fails at
//! run time. What this provides is the data the negotiating half of that trait
//! would return, available now and usable by whatever implements the rest.
//!
//! [`ScanoutOutput`]: crate::output::ScanoutOutput

use emblema_hal::{Error, Fourcc, Modifier, Result};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt as _;

/// An open KMS device.
pub struct DrmDevice {
    fd: OwnedFd,
    path: String,
}

impl AsFd for DrmDevice {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl drm::Device for DrmDevice {}
impl drm::control::Device for DrmDevice {}

/// A connector and what it reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    pub name: String,
    pub connected: bool,
    /// Width, height and refresh in hertz, in the order the kernel lists them,
    /// which puts the preferred mode first.
    pub modes: Vec<(u16, u16, u32)>,
}

/// A plane, and the layouts it can scan out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    pub id: u32,
    /// Whether this is the plane a full-screen frame would go to.
    ///
    /// A cursor or overlay plane advertises formats too, and negotiating
    /// against one of those would agree a layout the primary plane may not
    /// accept.
    pub primary: bool,
    pub formats: Vec<emblema_hal::FormatModifierSet>,
}

impl DrmDevice {
    /// Open a card node.
    ///
    /// Read-write, because that is what the enumeration ioctls want, and
    /// opening a card another process is master of is allowed — it is becoming
    /// master that is not.
    ///
    /// Non-blocking, which matters for one caller and not at all for the rest:
    /// reading events from a blocking DRM fd waits until one arrives, so a
    /// `poll_events` that is documented not to block would block forever the
    /// first time nothing had happened yet. Nothing else here reads the fd.
    pub fn open(path: &str) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .open(path)
            .map_err(|e| Error::Backend {
                backend: "drm",
                detail: format!("opening {path}: {e}"),
            })?;
        let device = Self {
            fd: OwnedFd::from(file),
            path: path.to_string(),
        };

        // Without this, `plane_handles` returns only overlay planes: the
        // primary and cursor planes are hidden from clients that have not asked
        // to see them, for the sake of drivers written before they existed. The
        // symptom is a device that appears to have no primary plane at all,
        // which is what this looked like before the capability was set.
        //
        // Setting a client capability needs no master, which is what keeps this
        // usable on a card something else is driving.
        use drm::Device as _;
        device
            .set_client_capability(drm::ClientCapability::UniversalPlanes, true)
            .map_err(|e| backend_err("enabling universal planes", e))?;
        Ok(device)
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Every connector, whether anything is plugged into it or not.
    pub fn connectors(&self) -> Result<Vec<Connector>> {
        use drm::control::Device as _;

        let resources = self
            .resource_handles()
            .map_err(|e| backend_err("resource_handles", e))?;
        let mut out = Vec::new();
        for handle in resources.connectors() {
            // `false` asks for what the kernel already knows rather than
            // forcing a probe. A probe on a card someone else is driving would
            // be both slow and rude, and the cached answer is what that
            // process last established.
            let Ok(connector) = self.get_connector(*handle, false) else {
                continue;
            };
            out.push(Connector {
                name: format!(
                    "{}-{}",
                    connector.interface().as_str(),
                    connector.interface_id()
                ),
                connected: connector.state() == drm::control::connector::State::Connected,
                modes: connector
                    .modes()
                    .iter()
                    .map(|m| (m.size().0, m.size().1, m.vrefresh()))
                    .collect(),
            });
        }
        Ok(out)
    }

    /// Every plane, and the formats and modifiers each advertises.
    pub fn planes(&self) -> Result<Vec<Plane>> {
        use drm::control::Device as _;

        let handles = self
            .plane_handles()
            .map_err(|e| backend_err("plane_handles", e))?;
        let mut out = Vec::new();
        for handle in handles {
            let Ok(properties) = self.get_properties(handle) else {
                continue;
            };
            let mut primary = false;
            let mut formats = Vec::new();
            for (id, value) in properties.iter() {
                let Ok(info) = self.get_property(*id) else {
                    continue;
                };
                match info.name().to_str() {
                    // An enum property, whose values the kernel fixes as
                    // overlay, primary, cursor in that order.
                    Ok("type") => primary = *value == 1,
                    Ok("IN_FORMATS") => {
                        if let Ok(blob) = self.get_property_blob(*value) {
                            formats = parse_in_formats(&blob);
                        }
                    }
                    _ => {}
                }
            }
            out.push(Plane {
                id: u32::from(handle),
                primary,
                formats,
            });
        }
        Ok(out)
    }

    /// The formats and modifiers a full-screen frame could use.
    ///
    /// The primary planes' lists, which is what the render side negotiates
    /// against. A device with no primary plane returns nothing rather than
    /// falling back to an overlay's list, since agreeing a layout the primary
    /// plane may not accept is worse than agreeing none.
    pub fn scanout_formats(&self) -> Result<Vec<emblema_hal::FormatModifierSet>> {
        Ok(self
            .planes()?
            .into_iter()
            .filter(|plane| plane.primary)
            .flat_map(|plane| plane.formats)
            .collect())
    }
}

fn backend_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Backend {
        backend: "drm",
        detail: format!("{what}: {e}"),
    }
}

/// Parse an `IN_FORMATS` property blob.
///
/// The kernel's layout, and one worth stating because nothing in the crate
/// graph parses it: a header, an array of fourcc codes, and an array of
/// modifier entries. Each modifier entry carries a sixty-four bit mask saying
/// which formats it applies to, and an offset saying which format the mask's
/// lowest bit refers to — so a device advertising more than sixty-four formats
/// repeats the modifier with a higher offset rather than widening the mask.
///
/// Malformed input yields nothing rather than an error. This is a property read
/// from a kernel that may be newer than this code, and the useful response to a
/// version it does not recognize is to negotiate as though the device
/// advertised nothing — which falls back to linear — rather than to fail the
/// frame loop.
pub fn parse_in_formats(blob: &[u8]) -> Vec<emblema_hal::FormatModifierSet> {
    const HEADER: usize = 24;
    const MODIFIER_ENTRY: usize = 24;
    if blob.len() < HEADER {
        return Vec::new();
    }
    // The slice is exactly the width of the array, which is what makes the
    // conversion total; the length check above is what makes the slice exist.
    let u32_at = |at: usize| {
        u32::from_ne_bytes(
            blob[at..at + 4]
                .try_into()
                .expect("four bytes is four bytes"),
        ) as usize
    };
    let u64_at =
        |at: usize| u64::from_ne_bytes(blob[at..at + 8].try_into().expect("eight bytes is eight"));

    if u32_at(0) != 1 {
        // Version one is the only one that has existed. A different one may
        // rearrange what follows, and reading it as though it had not is how a
        // parser invents modifiers.
        return Vec::new();
    }
    let count_formats = u32_at(8);
    let formats_offset = u32_at(12);
    let count_modifiers = u32_at(16);
    let modifiers_offset = u32_at(20);

    let formats_end = formats_offset.saturating_add(count_formats.saturating_mul(4));
    let modifiers_end =
        modifiers_offset.saturating_add(count_modifiers.saturating_mul(MODIFIER_ENTRY));
    if formats_end > blob.len() || modifiers_end > blob.len() {
        return Vec::new();
    }

    let formats: Vec<Fourcc> = (0..count_formats)
        .map(|i| Fourcc(u32_at(formats_offset + i * 4) as u32))
        .collect();

    // Built as a list per format rather than per modifier entry, because that
    // is the shape negotiation wants: what can this fourcc be laid out as.
    let mut modifiers: Vec<Vec<Modifier>> = vec![Vec::new(); formats.len()];
    for i in 0..count_modifiers {
        let at = modifiers_offset + i * MODIFIER_ENTRY;
        let mask = u64_at(at);
        let offset = u32_at(at + 8);
        let modifier = Modifier(u64_at(at + 16));
        for bit in 0..64usize {
            if mask & (1u64 << bit) == 0 {
                continue;
            }
            if let Some(slot) = modifiers.get_mut(offset + bit) {
                if !slot.contains(&modifier) {
                    slot.push(modifier);
                }
            }
        }
    }

    formats
        .into_iter()
        .zip(modifiers)
        // A format no modifier entry claimed can only be laid out linearly, and
        // reporting it with an empty list would say it cannot be used at all.
        .filter(|(_, modifiers)| !modifiers.is_empty())
        .map(|(fourcc, modifiers)| emblema_hal::FormatModifierSet::new(fourcc, modifiers))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `IN_FORMATS` blob the way the kernel lays one out.
    fn blob(formats: &[u32], entries: &[(u64, u32, u64)]) -> Vec<u8> {
        let header = 24usize;
        let formats_offset = header;
        let modifiers_offset = formats_offset + formats.len() * 4;
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_ne_bytes()); // version
        out.extend_from_slice(&0u32.to_ne_bytes()); // flags
        out.extend_from_slice(&(formats.len() as u32).to_ne_bytes());
        out.extend_from_slice(&(formats_offset as u32).to_ne_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_ne_bytes());
        out.extend_from_slice(&(modifiers_offset as u32).to_ne_bytes());
        for f in formats {
            out.extend_from_slice(&f.to_ne_bytes());
        }
        for (mask, offset, modifier) in entries {
            out.extend_from_slice(&mask.to_ne_bytes());
            out.extend_from_slice(&offset.to_ne_bytes());
            out.extend_from_slice(&0u32.to_ne_bytes()); // pad
            out.extend_from_slice(&modifier.to_ne_bytes());
        }
        out
    }

    #[test]
    fn a_modifier_applies_to_the_formats_its_mask_names() {
        // Bit zero of the mask is the format at the entry's offset, not the
        // first format in the array. Reading it as the latter attributes a
        // modifier to the wrong format, which negotiation then agrees to.
        //   mask 0b101 at offset 0 -> formats 0 and 2
        //   mask 0b1   at offset 2 -> format 2
        // so format 1 is claimed by nothing and drops out, and format 2 has
        // both modifiers. Reading bit zero as "the first format in the array"
        // rather than "the format at this entry's offset" gives format 0 the
        // second modifier, which is a layout the plane never advertised.
        let parsed = parse_in_formats(&blob(
            &[0x11111111, 0x22222222, 0x33333333],
            &[(0b101, 0, 0), (0b1, 2, 7)],
        ));
        assert_eq!(parsed.len(), 2, "{parsed:?}");
        assert_eq!(parsed[0].fourcc, Fourcc(0x11111111));
        assert_eq!(parsed[0].modifiers, vec![Modifier(0)]);
        assert_eq!(parsed[1].fourcc, Fourcc(0x33333333));
        assert_eq!(parsed[1].modifiers, vec![Modifier(0), Modifier(7)]);
    }

    #[test]
    fn an_offset_past_the_format_array_is_ignored_rather_than_panicking() {
        // A kernel advertising more than sixty-four formats repeats a modifier
        // at a higher offset, and a blob whose last entry runs off the end is
        // the same shape as one that is simply malformed.
        let parsed = parse_in_formats(&blob(&[1, 2], &[(0b11, 8, 5)]));
        assert!(parsed.is_empty(), "{parsed:?}");
    }

    #[test]
    fn a_format_no_modifier_claims_is_left_out() {
        // Reporting it with an empty modifier list would say the format cannot
        // be used at all, which is the opposite of what an absent entry means.
        let parsed = parse_in_formats(&blob(&[1, 2], &[(0b01, 0, 0)]));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].fourcc, Fourcc(1));
    }

    #[test]
    fn a_repeated_modifier_is_listed_once() {
        // Two entries may name the same modifier for the same format, and a
        // negotiation reading duplicates would rank a layout by how many times
        // the kernel happened to mention it.
        let parsed = parse_in_formats(&blob(&[1], &[(0b1, 0, 4), (0b1, 0, 4)]));
        assert_eq!(parsed[0].modifiers, vec![Modifier(4)]);
    }

    #[test]
    fn a_blob_this_code_does_not_understand_advertises_nothing() {
        // Not an error: negotiation against nothing falls back to linear, which
        // is correct and slow, where failing would take down a frame loop over
        // a kernel newer than this parser.
        let mut future = blob(&[1], &[(0b1, 0, 0)]);
        future[0] = 2; // a version that does not exist
        assert!(parse_in_formats(&future).is_empty());

        assert!(parse_in_formats(&[]).is_empty());
        assert!(parse_in_formats(&[0u8; 12]).is_empty());
        // A header whose arrays claim more than the blob holds.
        let mut truncated = blob(&[1, 2, 3], &[(0b111, 0, 0)]);
        truncated.truncate(30);
        assert!(parse_in_formats(&truncated).is_empty());
    }
}

#[cfg(test)]
mod machine_tests {
    use super::*;

    /// The first card node on this machine, if there is one.
    fn card() -> Option<DrmDevice> {
        for entry in std::fs::read_dir("/dev/dri").ok()?.flatten() {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with("card") {
                continue;
            }
            match DrmDevice::open(&entry.path().to_string_lossy()) {
                Ok(device) => return Some(device),
                Err(e) => eprintln!("skipping {name}: {e}"),
            }
        }
        None
    }

    #[test]
    fn a_card_can_be_enumerated_without_becoming_master() {
        // The claim this rests on, and the reason this half of the scanout path
        // is checkable at all on an ordinary desktop: a compositor holds master
        // on the card driving the display, and reading what it advertises is
        // allowed anyway. If that stops being true, everything here starts
        // skipping and this is the test that says so.
        let Some(device) = card() else {
            eprintln!("skipping: no card node on this machine");
            return;
        };

        let connectors = device.connectors().expect("connectors");
        assert!(
            !connectors.is_empty(),
            "{} reported no connectors at all",
            device.path()
        );
        // A connected connector has modes; that pairing is what a scanout path
        // reads to pick one, so a connector claiming to be connected with
        // nothing to display would be worse than an absent one.
        for connector in connectors.iter().filter(|c| c.connected) {
            assert!(
                !connector.modes.is_empty(),
                "{} is connected and offers no mode",
                connector.name
            );
            let (w, h, _) = connector.modes[0];
            assert!(
                w > 0 && h > 0,
                "{} offers a zero-sized mode",
                connector.name
            );
        }
    }

    #[test]
    fn a_primary_plane_advertises_something_to_scan_out() {
        let Some(device) = card() else {
            return;
        };
        let planes = device.planes().expect("planes");
        assert!(!planes.is_empty(), "no planes at all");
        assert!(
            planes.iter().any(|p| p.primary),
            "no plane identified itself as primary; the type property was not read"
        );

        // The point of reading IN_FORMATS rather than assuming: this is the
        // half of negotiation the display supplies, and a device advertising
        // nothing leaves the render side with linear and no way to know better.
        let formats = device.scanout_formats().expect("scanout formats");
        assert!(
            !formats.is_empty(),
            "the primary plane advertised no format at all"
        );
        for set in &formats {
            assert!(
                !set.modifiers.is_empty(),
                "{:?} was reported with no modifier, which says it cannot be used",
                set.fourcc
            );
        }
        // Linear is required of every plane, so its absence means the blob was
        // misread rather than that the hardware lacks it.
        assert!(
            formats
                .iter()
                .any(|s| s.modifiers.contains(&Modifier::LINEAR)),
            "no advertised format accepts a linear layout"
        );
    }
}

impl DrmDevice {
    /// Become the device's modesetting master, and enable atomic commits.
    ///
    /// Everything that changes what is on screen needs this, and only one
    /// process may hold it at a time — a compositor holds it on any card
    /// driving a display. Failing is therefore a normal outcome and not an
    /// error in this code: the answer is another device, or a bare VT.
    ///
    /// Atomic is asked for at the same time because the two travel together:
    /// the commit path here is atomic-only, and a device that cannot do atomic
    /// is one this cannot drive at all.
    pub fn become_master(&self) -> Result<()> {
        use drm::Device as _;

        self.acquire_master_lock()
            .map_err(|e| backend_err("acquiring the modesetting master lock", e))?;
        self.set_client_capability(drm::ClientCapability::Atomic, true)
            .map_err(|e| backend_err("enabling atomic modesetting", e))?;
        Ok(())
    }

    /// Give the master lock back.
    pub fn release_master(&self) -> Result<()> {
        use drm::Device as _;

        self.release_master_lock()
            .map_err(|e| backend_err("releasing the modesetting master lock", e))
    }
}

//! Choosing a format and memory layout both sides can live with.
//!
//! Negotiation runs between the two axes: a presentation target advertises what
//! it can scan out, the rendering context advertises what it can render into
//! and export, and the intersection decides what gets allocated.
//!
//! The failure mode this guards against is not a crash. A renderer that quietly
//! falls back to a linear layout when a vendor tiled or compressed one was
//! available still produces correct pixels, and on an embedded panel it can
//! halve effective memory bandwidth while looking entirely healthy. So the
//! outcome is always reported, an empty intersection is an error carrying both
//! sides rather than a silent substitution, and a caller can require that the
//! result not be linear.

use emblema_hal::{Error, FormatModifierSet, Fourcc, Modifier, Result};

/// What negotiation settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    pub fourcc: Fourcc,
    pub modifier: Modifier,
}

impl Negotiated {
    /// Whether the chosen layout is the universally supported, universally slow
    /// one.
    pub fn is_linear(&self) -> bool {
        self.modifier.is_linear()
    }
}

impl std::fmt::Display for Negotiated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} with modifier {}", self.fourcc, self.modifier)
    }
}

/// Formats to try, best first.
///
/// Ten-bit first because a pipeline that can drive it should, then the ordinary
/// eight-bit orders. Opaque variants sit alongside their alpha counterparts
/// because a scanout plane frequently offers only one of the pair.
pub const PREFERRED_FORMATS: &[Fourcc] = &[
    Fourcc::XRGB2101010,
    Fourcc::ARGB2101010,
    Fourcc::ARGB8888,
    Fourcc::XRGB8888,
    Fourcc::ABGR8888,
    Fourcc::XBGR8888,
];

/// Pick a format and modifier both sides accept.
///
/// `preferred` is consulted in order; a format neither side offers is skipped.
/// Within a format, [`FormatModifierSet::intersect`] orders shared modifiers so
/// linear sorts last, so the first entry is the best layout available.
pub fn negotiate(
    render: &[FormatModifierSet],
    scanout: &[FormatModifierSet],
    preferred: &[Fourcc],
) -> Result<Negotiated> {
    for fourcc in preferred {
        let (Some(a), Some(b)) = (find(render, *fourcc), find(scanout, *fourcc)) else {
            continue;
        };
        let Some(common) = a.intersect(b) else {
            continue;
        };
        let modifier = *common
            .modifiers
            .first()
            .expect("intersect returns non-empty");
        let chosen = Negotiated {
            fourcc: *fourcc,
            modifier,
        };

        // Always logged, never only on failure. Which layout was chosen is the
        // first thing anyone investigating a bandwidth problem needs, and it is
        // invisible from the rendered output.
        if chosen.is_linear() {
            log::warn!("negotiated {chosen}; no shared tiled or compressed layout was available");
        } else {
            log::info!("negotiated {chosen}");
        }
        return Ok(chosen);
    }

    Err(Error::Backend {
        backend: "present",
        detail: format!(
            "no shared format and modifier.\n  render side: {}\n  scanout side: {}",
            describe(render),
            describe(scanout)
        ),
    })
}

/// Negotiate, and refuse a linear result.
///
/// For a target whose hardware is known to support a better layout, a linear
/// fallback is a defect rather than a degraded mode: it looks correct and costs
/// bandwidth silently. Callers that know what their panel can do use this so
/// the fallback fails loudly instead.
pub fn negotiate_non_linear(
    render: &[FormatModifierSet],
    scanout: &[FormatModifierSet],
    preferred: &[Fourcc],
) -> Result<Negotiated> {
    let chosen = negotiate(render, scanout, preferred)?;
    if chosen.is_linear() {
        return Err(Error::Backend {
            backend: "present",
            detail: format!(
                "negotiation fell back to a linear layout for {}, but a non-linear one was required",
                chosen.fourcc
            ),
        });
    }
    Ok(chosen)
}

fn find(sets: &[FormatModifierSet], fourcc: Fourcc) -> Option<&FormatModifierSet> {
    sets.iter().find(|s| s.fourcc == fourcc)
}

fn describe(sets: &[FormatModifierSet]) -> String {
    if sets.is_empty() {
        return "(nothing advertised)".into();
    }
    sets.iter()
        .map(|s| {
            let modifiers: Vec<String> = s.modifiers.iter().map(|m| m.to_string()).collect();
            format!("{} [{}]", s.fourcc, modifiers.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VENDOR_TILED: Modifier = Modifier(0x0100_0000_0000_0001);
    const VENDOR_COMPRESSED: Modifier = Modifier(0x0100_0000_0000_0002);

    fn set(fourcc: Fourcc, modifiers: &[Modifier]) -> FormatModifierSet {
        FormatModifierSet::new(fourcc, modifiers.to_vec())
    }

    #[test]
    fn a_shared_tiled_layout_beats_linear() {
        let render = [set(Fourcc::ARGB8888, &[Modifier::LINEAR, VENDOR_TILED])];
        let scanout = [set(Fourcc::ARGB8888, &[VENDOR_TILED, Modifier::LINEAR])];

        let chosen = negotiate(&render, &scanout, PREFERRED_FORMATS).expect("negotiated");
        assert_eq!(chosen.modifier, VENDOR_TILED);
        assert!(!chosen.is_linear());
    }

    #[test]
    fn linear_is_chosen_only_when_it_is_all_that_is_shared() {
        // Each side supports a different tiled layout, so linear is the only
        // common ground. That is a legitimate outcome, and it is reported.
        let render = [set(Fourcc::ARGB8888, &[VENDOR_TILED, Modifier::LINEAR])];
        let scanout = [set(
            Fourcc::ARGB8888,
            &[VENDOR_COMPRESSED, Modifier::LINEAR],
        )];

        let chosen = negotiate(&render, &scanout, PREFERRED_FORMATS).expect("negotiated");
        assert!(chosen.is_linear());
    }

    #[test]
    fn a_caller_can_require_a_non_linear_layout() {
        let render = [set(Fourcc::ARGB8888, &[Modifier::LINEAR])];
        let scanout = [set(Fourcc::ARGB8888, &[Modifier::LINEAR])];

        // Plain negotiation succeeds; the strict form refuses, because on
        // hardware known to do better a linear result is a defect that looks
        // exactly like success.
        assert!(negotiate(&render, &scanout, PREFERRED_FORMATS).is_ok());
        assert!(negotiate_non_linear(&render, &scanout, PREFERRED_FORMATS).is_err());
    }

    #[test]
    fn a_higher_bit_depth_is_preferred_when_both_sides_offer_it() {
        let render = [
            set(Fourcc::ARGB8888, &[VENDOR_TILED]),
            set(Fourcc::XRGB2101010, &[VENDOR_TILED]),
        ];
        let scanout = [
            set(Fourcc::XRGB2101010, &[VENDOR_TILED]),
            set(Fourcc::ARGB8888, &[VENDOR_TILED]),
        ];

        let chosen = negotiate(&render, &scanout, PREFERRED_FORMATS).expect("negotiated");
        assert_eq!(chosen.fourcc, Fourcc::XRGB2101010);
    }

    #[test]
    fn a_format_only_one_side_offers_is_skipped_rather_than_chosen() {
        let render = [
            set(Fourcc::XRGB2101010, &[VENDOR_TILED]),
            set(Fourcc::ARGB8888, &[VENDOR_TILED]),
        ];
        // The scanout plane cannot do ten-bit, which is the common case.
        let scanout = [set(Fourcc::ARGB8888, &[VENDOR_TILED])];

        let chosen = negotiate(&render, &scanout, PREFERRED_FORMATS).expect("negotiated");
        assert_eq!(chosen.fourcc, Fourcc::ARGB8888);
    }

    #[test]
    fn a_format_shared_in_name_but_not_in_layout_is_skipped() {
        // Both sides claim the format, but share no modifier for it. Falling
        // through to the next format is right; picking a modifier one side
        // cannot use would produce a buffer the display controller rejects.
        let render = [
            set(Fourcc::ARGB8888, &[VENDOR_TILED]),
            set(Fourcc::XRGB8888, &[Modifier::LINEAR]),
        ];
        let scanout = [
            set(Fourcc::ARGB8888, &[VENDOR_COMPRESSED]),
            set(Fourcc::XRGB8888, &[Modifier::LINEAR]),
        ];

        let chosen = negotiate(&render, &scanout, PREFERRED_FORMATS).expect("negotiated");
        assert_eq!(chosen.fourcc, Fourcc::XRGB8888);
    }

    #[test]
    fn failure_names_what_each_side_offered() {
        let render = [set(Fourcc::ARGB8888, &[VENDOR_TILED])];
        let scanout = [set(Fourcc::XBGR8888, &[Modifier::LINEAR])];

        let error = negotiate(&render, &scanout, PREFERRED_FORMATS).expect_err("nothing in common");
        let text = error.to_string();
        // Diagnosing this from a bare failure is nearly impossible, so both
        // sides travel with the error.
        assert!(text.contains("AR24"), "{text}");
        assert!(text.contains("XB24"), "{text}");
        assert!(text.contains("LINEAR"), "{text}");
    }

    #[test]
    fn an_empty_advertisement_fails_clearly_rather_than_panicking() {
        let render = [set(Fourcc::ARGB8888, &[Modifier::LINEAR])];
        let error = negotiate(&render, &[], PREFERRED_FORMATS).expect_err("nothing shared");
        assert!(error.to_string().contains("nothing advertised"));
    }
}

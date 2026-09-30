//! Generated format and modifier sets, put through negotiation.
//!
//! Negotiation is where two independently-reported lists of what hardware can do
//! meet, and agreeing a layout neither side advertised is the failure with
//! consequences: the renderer allocates one thing, the display scans out
//! another, and what reaches a panel is skewed or nothing. The table of cases in
//! `negotiate.rs` covers the shapes someone thought of, including both refusal
//! paths. These cover the shapes nobody did.
//!
//! `proptest` rather than a fuzzer, for the reason `docs/architecture.md` gives
//! where it rejects `cargo-fuzz`: no nightly and no C toolchain, and a property
//! that runs in the gate beats one that runs somewhere else.
//!
//! # The alphabet is small on purpose
//!
//! Generating fourccs and modifiers from the whole of `u32` and `u64` would make
//! two sides sharing anything vanishingly rare, so every case would take the
//! refusal path and the properties about a successful agreement would never run
//! -- passing while testing nothing, which is the trap this repository has
//! written down twice. So both sides draw from a handful of values, which makes
//! collisions the common case. `agreement_and_refusal_both_happen` is what keeps
//! that claim honest rather than assumed.
//!
//! # One guard these cannot reach
//!
//! `FormatModifierSet::intersect` refuses two sets whose fourccs differ, and
//! removing that refusal leaves every property here passing. That is correct
//! rather than a hole: `negotiate` only ever intersects sets it has already
//! selected by the same fourcc, so the guard is unreachable through this API and
//! is pinned where it is reachable, by `intersection_fails_loudly_rather_than_falling_back`
//! in `emblema-hal`. Worth stating, because a reader who found that mutation
//! surviving would otherwise conclude these properties were weaker than they are.

use emblema_hal::{FormatModifierSet, Fourcc, Modifier};
use emblema_present::{negotiate, negotiate_non_linear};
use proptest::prelude::*;

/// A vendor-ish modifier, and two more, against `LINEAR`.
///
/// Four values, because what matters is whether a shared one exists and whether
/// it is linear, and four reaches every combination of those.
const MODIFIERS: [Modifier; 4] = [
    Modifier::LINEAR,
    Modifier(0x0100_0000_0000_0001),
    Modifier(0x0100_0000_0000_0002),
    Modifier(7),
];

fn fourcc() -> impl Strategy<Value = Fourcc> {
    prop_oneof![
        Just(Fourcc::ARGB8888),
        Just(Fourcc::XRGB8888),
        Just(Fourcc::XRGB2101010),
    ]
}

/// One side's advertised layouts.
///
/// An empty modifier list is representable -- `FormatModifierSet` permits one, so
/// a driver naming a format it cannot lay out has to be survivable -- but it is
/// weighted down rather than drawn uniformly. The first version of this drew
/// lengths from `0..4` over sets of `0..5`, and `agreement_and_refusal_both_happen`
/// caught the result: thirty-six of five hundred and twelve cases agreed, so the
/// property about a successful agreement was running on seven per cent of them.
fn sets() -> impl Strategy<Value = Vec<FormatModifierSet>> {
    prop::collection::vec(
        (
            fourcc(),
            prop_oneof![
                7 => prop::collection::vec(prop::sample::select(MODIFIERS.as_slice()), 1..4),
                1 => Just(Vec::new()),
            ],
        ),
        1..4,
    )
    .prop_map(|pairs| {
        pairs
            .into_iter()
            .map(|(fourcc, modifiers)| FormatModifierSet::new(fourcc, modifiers))
            .collect()
    })
}

fn preferred() -> impl Strategy<Value = Vec<Fourcc>> {
    prop::collection::vec(fourcc(), 1..4)
}

/// What one side advertises for a fourcc, taking the first set as `find` does.
fn advertised(sets: &[FormatModifierSet], fourcc: Fourcc) -> Option<&FormatModifierSet> {
    sets.iter().find(|s| s.fourcc == fourcc)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// An agreed layout is one both sides advertised, and one that was asked for.
    ///
    /// Three claims in one, because they fail together: the fourcc has to be in
    /// the preferred list, and the modifier has to appear in both sides' lists
    /// for that fourcc. A layout only one side named is the defect that puts a
    /// skewed picture on a panel.
    #[test]
    fn an_agreed_layout_is_one_both_sides_advertised(
        render in sets(),
        scanout in sets(),
        preferred in preferred(),
    ) {
        let Ok(chosen) = negotiate(&render, &scanout, &preferred) else {
            return Ok(());
        };
        prop_assert!(
            preferred.contains(&chosen.fourcc),
            "agreed {:?}, which was not among the {} formats asked for",
            chosen.fourcc,
            preferred.len()
        );
        for (side, sets) in [("render", &render), ("scanout", &scanout)] {
            let set = advertised(sets, chosen.fourcc);
            let Some(set) = set else {
                prop_assert!(false, "the {side} side never advertised {:?}", chosen.fourcc);
                unreachable!()
            };
            prop_assert!(
                set.modifiers.contains(&chosen.modifier),
                "agreed {:?} for {:?}, which the {side} side does not list",
                chosen.modifier,
                chosen.fourcc
            );
        }
    }

    /// A refusal means nothing asked for was shared.
    ///
    /// The direction the property above cannot check. A negotiation that gave up
    /// while a workable layout was on the table would send a caller to a linear
    /// fallback, or fail a frame loop outright, over a layout both sides had.
    #[test]
    fn a_refusal_means_nothing_asked_for_was_shared(
        render in sets(),
        scanout in sets(),
        preferred in preferred(),
    ) {
        if negotiate(&render, &scanout, &preferred).is_ok() {
            return Ok(());
        }
        for fourcc in &preferred {
            let (Some(a), Some(b)) = (advertised(&render, *fourcc), advertised(&scanout, *fourcc))
            else {
                continue;
            };
            let shared: Vec<Modifier> = a
                .modifiers
                .iter()
                .filter(|m| b.modifiers.contains(m))
                .copied()
                .collect();
            prop_assert!(
                shared.is_empty(),
                "refused, but both sides list {shared:?} for {fourcc:?}"
            );
        }
    }

    /// A shared non-linear layout is preferred over a shared linear one.
    ///
    /// `intersect` sorts linear last so the caller taking the first entry gets
    /// the bandwidth-preserving layout. That ordering is the whole reason the
    /// function sorts at all, and it is invisible in the rendered output -- a
    /// negotiation quietly choosing linear costs memory bandwidth on every frame
    /// and looks identical.
    #[test]
    fn a_shared_tiled_layout_beats_a_shared_linear_one(
        render in sets(),
        scanout in sets(),
        preferred in preferred(),
    ) {
        let Ok(chosen) = negotiate(&render, &scanout, &preferred) else {
            return Ok(());
        };
        if !chosen.is_linear() {
            return Ok(());
        }
        // It chose linear, so no non-linear layout may have been available for
        // the format it settled on.
        let (Some(a), Some(b)) = (
            advertised(&render, chosen.fourcc),
            advertised(&scanout, chosen.fourcc),
        ) else {
            return Ok(());
        };
        let tiled: Vec<Modifier> = a
            .modifiers
            .iter()
            .filter(|m| b.modifiers.contains(m) && !m.is_linear())
            .copied()
            .collect();
        prop_assert!(
            tiled.is_empty(),
            "chose a linear layout for {:?} while both sides shared {tiled:?}",
            chosen.fourcc
        );
    }

    /// Requiring a non-linear layout never yields a linear one.
    ///
    /// The whole purpose of the stricter call, and the one a caller relies on
    /// when linear would not meet a bandwidth budget. It must refuse rather than
    /// quietly hand back the thing it was told not to.
    #[test]
    fn requiring_a_tiled_layout_never_agrees_a_linear_one(
        render in sets(),
        scanout in sets(),
        preferred in preferred(),
    ) {
        let Ok(chosen) = negotiate_non_linear(&render, &scanout, &preferred) else {
            return Ok(());
        };
        prop_assert!(
            !chosen.is_linear(),
            "a negotiation that required a non-linear layout agreed {:?}",
            chosen.modifier
        );
    }
}

/// Both outcomes happen often enough for the properties to mean anything.
///
/// Every property above returns early on the outcome it is not about, so a
/// generator that only ever produced refusals would make
/// `an_agreed_layout_is_one_both_sides_advertised` vacuous, and one that only
/// ever agreed would do the same to the refusal property. Neither would fail.
/// This is the check that says the alphabet above is small enough.
#[test]
fn agreement_and_refusal_both_happen() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = (sets(), sets(), preferred());
    let mut agreed = 0usize;
    let mut refused = 0usize;
    let mut linear = 0usize;
    const TRIES: usize = 512;
    for _ in 0..TRIES {
        let (render, scanout, preferred) = strategy
            .new_tree(&mut runner)
            .expect("the generators above are infallible")
            .current();
        match negotiate(&render, &scanout, &preferred) {
            Ok(chosen) => {
                agreed += 1;
                if chosen.is_linear() {
                    linear += 1;
                }
            }
            Err(_) => refused += 1,
        }
    }
    assert!(
        agreed * 10 >= TRIES && refused * 10 >= TRIES,
        "of {TRIES} generated negotiations {agreed} agreed and {refused} were \
         refused, so the properties over one of the two outcomes are vacuous"
    );
    // Counted separately, because it is the case
    // `requiring_a_tiled_layout_never_agrees_a_linear_one` is entirely about: a
    // linear agreement is the only way the stricter call has anything to refuse.
    // Disabling that call's guard was caught only because this happens; a
    // generator where both sides always shared something tiled would leave the
    // property passing over an implementation that had stopped checking.
    assert!(
        linear > 0,
        "none of {agreed} agreements were linear, so nothing exercised the \
         refusal that negotiate_non_linear exists for"
    );
}

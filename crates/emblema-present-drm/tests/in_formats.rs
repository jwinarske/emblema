//! Generated `IN_FORMATS` blobs, which is the one place here that reads the
//! kernel's bytes at offsets it computes itself.
//!
//! The table of cases beside `parse_in_formats` pins what a correct blob means:
//! bit zero of a mask is the format at that entry's offset, a format nothing
//! claims drops out, a repeated modifier is listed once. Those say the parser
//! reads a *well-formed* blob correctly. What they cannot say is that no
//! malformed one gets it to index outside what it was given, because a table
//! only holds the malformations someone thought of.
//!
//! So this generates them. `proptest` rather than a fuzzer for the reason
//! `docs/architecture.md` gives: this workspace compiles with no C toolchain and
//! no nightly, and a property runs in the gate on every commit instead of
//! somewhere else on a schedule.
//!
//! # Why these are not fed random bytes
//!
//! Almost every byte string fails at the version word -- a `u32` that has to
//! equal one -- so a generator over `Vec<u8>` would spend its whole run
//! returning at the second guard, and pass having exercised nothing past it.
//! That is the trap `a_skip_says_the_word_the_census_counts` exists for in
//! another form: a check that reads nothing passes everything. These build a
//! structurally plausible blob and then corrupt it, so the arithmetic under test
//! is actually reached. `a_generated_blob_is_usually_read_rather_than_refused`
//! is what holds that honest.
//!
//! # What is asserted, and what is deliberately not
//!
//! The properties here are *containment* claims: nothing comes out that did not
//! go in. They do not recompute which format a mask ought to claim, because a
//! reference implementation of that would be the same arithmetic twice and would
//! agree with the parser about any mistake in it. Attribution is pinned by the
//! hand-written table instead. The division is deliberate: the table catches the
//! right modifier on the wrong format, and these catch a value from nowhere.

use emblema_hal::{Fourcc, Modifier};
use emblema_present_drm::device::parse_in_formats;
use proptest::prelude::*;

const HEADER: usize = 24;

/// A modifier entry as the kernel lays one out: which formats, from where, and
/// what layout.
type Entry = (u64, u32, u64);

/// Build a blob the way the kernel does, and the way the unit tests beside the
/// parser do.
fn blob(formats: &[u32], entries: &[Entry]) -> Vec<u8> {
    let formats_offset = HEADER;
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

/// Formats, generated past the sixty-four a single mask can name.
///
/// Sixty-four is where a mask's bits run out and the kernel starts repeating an
/// entry at a higher offset, so a list that never crosses it never reaches the
/// case the offset arithmetic exists for.
fn formats() -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(
        prop_oneof![
            // Real fourccs, so a generated blob sometimes looks like a card's.
            Just(0x3432_5258u32),
            Just(0x3234_5241u32),
            any::<u32>(),
        ],
        0..70,
    )
}

/// Entries whose offsets land in range about as often as they do not.
fn entries() -> impl Strategy<Value = Vec<Entry>> {
    prop::collection::vec(
        (
            prop_oneof![
                Just(0u64),
                Just(u64::MAX),
                Just(1u64),
                Just(0b101u64),
                any::<u64>(),
            ],
            prop_oneof![0u32..80, Just(u32::MAX), Just(u32::MAX - 63), any::<u32>(),],
            prop_oneof![Just(0u64), Just(u64::MAX), any::<u64>()],
        ),
        0..12,
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// Nothing comes out of a blob that did not go into it.
    ///
    /// The parser's own comment names the failure this is about -- "reading it as
    /// though it had not is how a parser invents modifiers". A fourcc or a
    /// modifier in the output that is absent from the input is a read that went
    /// somewhere it was not given, whatever the offsets said.
    #[test]
    fn a_blob_of_any_shape_advertises_only_what_it_contains(
        formats in formats(),
        entries in entries(),
    ) {
        let parsed = parse_in_formats(&blob(&formats, &entries));

        // Order-preserving subsequence of the formats given. One walk catches
        // three things at once: an invented fourcc, a reordering, and a format
        // reported more times than it was advertised.
        let mut next = 0usize;
        for set in &parsed {
            let found = formats[next..]
                .iter()
                .position(|f| Fourcc(*f) == set.fourcc)
                .map(|at| next + at);
            let Some(at) = found else {
                prop_assert!(
                    false,
                    "advertised {:?}, which is not in the {} formats the blob \
                     carries at or after index {next}",
                    set.fourcc,
                    formats.len()
                );
                unreachable!()
            };
            next = at + 1;
        }

        let offered: Vec<Modifier> = entries.iter().map(|(_, _, m)| Modifier(*m)).collect();
        for set in &parsed {
            prop_assert!(
                !set.modifiers.is_empty(),
                "{:?} came back with no modifiers, which says the format cannot \
                 be used at all rather than that nothing claimed it",
                set.fourcc
            );
            for modifier in &set.modifiers {
                prop_assert!(
                    offered.contains(modifier),
                    "advertised {modifier:?} for {:?}, which no entry in the blob \
                     names",
                    set.fourcc
                );
            }
            let mut seen = set.modifiers.clone();
            seen.sort_by_key(|m| m.0);
            let before = seen.len();
            seen.dedup();
            prop_assert_eq!(
                before,
                seen.len(),
                "{:?} listed a modifier twice, so negotiation would rank a layout \
                 by how often the kernel mentioned it",
                set.fourcc
            );
        }
    }

    /// Cutting a blob short anywhere leaves it readable or refused, never wrong.
    ///
    /// A truncated blob is the shape a short read produces and the shape a
    /// malformed one produces, and the header still claims the arrays the whole
    /// blob had -- so this is the case where the length checks are all that
    /// stands between the parser and an index past the end.
    #[test]
    fn a_blob_cut_short_is_refused_rather_than_read_past(
        formats in formats(),
        entries in entries(),
        cut in 0usize..400,
    ) {
        let mut whole = blob(&formats, &entries);
        let at = cut.min(whole.len());
        whole.truncate(at);
        let parsed = parse_in_formats(&whole);

        // Whatever survives must still only name formats the bytes still hold.
        for set in &parsed {
            prop_assert!(
                formats.iter().any(|f| Fourcc(*f) == set.fourcc),
                "a blob cut to {at} bytes advertised {:?}, which is not among the \
                 formats it was built from",
                set.fourcc
            );
        }
    }

    /// Only version one is read, whatever follows it.
    ///
    /// The parser refuses an unknown version rather than erroring, because a
    /// kernel newer than this code must not take down a frame loop -- and
    /// negotiation against nothing falls back to linear, which is correct and
    /// slow. What this pins is that the refusal does not depend on the rest of
    /// the blob being implausible.
    #[test]
    fn a_version_this_code_does_not_know_advertises_nothing(
        formats in formats(),
        entries in entries(),
        version in prop_oneof![Just(0u32), Just(2u32), 2u32..u32::MAX],
    ) {
        let mut wrong = blob(&formats, &entries);
        wrong[0..4].copy_from_slice(&version.to_ne_bytes());
        prop_assert!(
            parse_in_formats(&wrong).is_empty(),
            "version {version} was read as though it were version one"
        );
    }

    /// A blob whose header points anywhere at all still only reports what it holds.
    ///
    /// The offsets and counts are the header's own claims, and nothing checks
    /// them against how the blob was actually built. This overwrites them with
    /// values a correct kernel would never write, which is the case the
    /// saturating arithmetic and the two end checks exist for.
    #[test]
    fn a_header_claiming_arrays_the_blob_does_not_hold_is_refused(
        formats in formats(),
        entries in entries(),
        count_formats in prop_oneof![0u32..80, Just(u32::MAX), any::<u32>()],
        formats_offset in prop_oneof![0u32..80, Just(u32::MAX), any::<u32>()],
        count_modifiers in prop_oneof![0u32..20, Just(u32::MAX), any::<u32>()],
        modifiers_offset in prop_oneof![0u32..200, Just(u32::MAX), any::<u32>()],
    ) {
        let mut lying = blob(&formats, &entries);
        lying[8..12].copy_from_slice(&count_formats.to_ne_bytes());
        lying[12..16].copy_from_slice(&formats_offset.to_ne_bytes());
        lying[16..20].copy_from_slice(&count_modifiers.to_ne_bytes());
        lying[20..24].copy_from_slice(&modifiers_offset.to_ne_bytes());

        // It returns, which is the whole of the claim. A header pointing into
        // the middle of the blob addresses real bytes, so what comes back may
        // legitimately be formats and modifiers that were never written as
        // such -- there is nothing to compare it against, and asserting there
        // were would be asserting the parser re-derives the header it was given.
        let _ = parse_in_formats(&lying);
    }
}

/// The generator reaches past the guards often enough to be worth running.
///
/// Every property above would pass on a parser that returned an empty list for
/// every input, and a generator that only ever produced blobs the guards refuse
/// would make that indistinguishable from a working one. So this measures it:
/// build blobs the same way and count how many come back with something in them.
///
/// Not a proptest, because it is a claim about the generators rather than about
/// the parser, and it needs to see the whole run to make it.
#[test]
fn a_generated_blob_is_usually_read_rather_than_refused() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = (formats(), entries());
    let mut read = 0usize;
    const TRIES: usize = 512;
    for _ in 0..TRIES {
        let (formats, entries) = strategy
            .new_tree(&mut runner)
            .expect("the generators above are infallible")
            .current();
        if !parse_in_formats(&blob(&formats, &entries)).is_empty() {
            read += 1;
        }
    }
    assert!(
        read * 4 >= TRIES,
        "only {read} of {TRIES} generated blobs advertised anything, so the \
         properties over them are mostly testing the guards rather than the \
         arithmetic behind them"
    );
}

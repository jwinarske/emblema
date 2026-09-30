//! Impeller C API: an ABI-compatible implementation of upstream's
//! `impeller.h`.
//!
//! **One entry point exists so far.** The goal is that a consumer linking
//! against the upstream C API can link against this instead without
//! recompiling, and the goal governs how everything here is built — binary
//! compatibility with the header rather than a Rust-flavored approximation of
//! it. Nothing about that goal is reached yet, and the surface a consumer
//! would need is almost entirely absent.
//!
//! # What this is not
//!
//! It is **not** a drop-in for Impeller inside the Flutter Engine build. The
//! engine does not consume Impeller across this boundary — it compiles the C++
//! sources directly, against internal C++ classes with templates, STL types,
//! and virtual inheritance that no Rust library can present. This crate serves
//! the embedder-facing C API only.
//!
//! # Conventions the header imposes
//!
//! - **Reference counting.** Every handle type has `Retain` and `Release`
//!   entry points, and both are no-ops when passed NULL.
//! - **No error channel.** Creation functions return NULL on failure and
//!   fallible operations return `bool`. There are no error codes and no way to
//!   report a reason, so anything diagnostic has to go to the log.
//! - **Version negotiation.** The caller passes its compiled-in version to
//!   context creation, and a mismatch fails the call. See [`IMPELLER_VERSION`].
//!
//! # Verifying parity
//!
//! Two checks are intended, and neither exists. Symbol-level parity is to be
//! checked against a pinned copy of the upstream header rather than maintained
//! by hand, so that an upstream addition shows up as a failure naming the
//! missing entry points; the header is not vendored, so that check cannot be
//! written yet. Semantic parity — blend mode values, fill rules, color
//! handling, stroke geometry — is to be checked by running the upstream C API
//! samples against this library and comparing output through the usual golden
//! comparators.
//!
//! What does exist is narrower and worth not confusing with either: a test
//! that the exported symbols are exactly the ones written down beside it. That
//! catches an accidental export and keeps the status below from drifting away
//! from the code. It says nothing about whether the surface matches upstream's,
//! which is the question the two checks above are for.
//!
//! # Status
//!
//! Only version negotiation is implemented, and `ImpellerGetVersion` is the
//! only exported symbol. The rest of the surface waits on a vendored header:
//! guessing an enum value or a struct layout would produce a library that
//! links and then corrupts memory, which is worse than one that does not link.
//! Stubbing entry points to return NULL is worse than their absence for the
//! same shape of reason — a consumer links successfully and fails at run time
//! with no diagnostic, where a missing symbol fails at link time and names
//! itself.

#![allow(non_snake_case)]

/// Packs a version the way the header's `IMPELLER_MAKE_VERSION` does.
pub const fn make_version(variant: u32, major: u32, minor: u32, patch: u32) -> u32 {
    (variant << 29) | (major << 22) | (minor << 12) | patch
}

pub const fn version_variant(v: u32) -> u32 {
    v >> 29
}

pub const fn version_major(v: u32) -> u32 {
    (v >> 22) & 0x7f
}

pub const fn version_minor(v: u32) -> u32 {
    (v >> 12) & 0x3ff
}

pub const fn version_patch(v: u32) -> u32 {
    v & 0xfff
}

/// The API version this library implements.
///
/// Must track the pinned upstream header exactly. A consumer compiled against
/// a different version is refused at context creation rather than allowed to
/// run against a surface it may not match.
///
/// # Provenance
///
/// Read from `engine/src/flutter/impeller/toolkit/interop/impeller.h` on the
/// `flutter/flutter` master branch, 2026-08-15. The older `flutter/engine`
/// repository carries a different minor version and is not the source of
/// truth; the engine moved into the monorepo.
///
/// A bare version constant with no recorded origin is a trap, because master
/// moves and negotiation failures surface as an opaque NULL from context
/// creation. When the pinned header is vendored for the symbol-parity check,
/// this constant is derived from it rather than restated here.
pub const IMPELLER_VERSION: u32 = make_version(1, 1, 4, 0);

/// `ImpellerGetVersion` — the version this library implements.
///
/// Callers use this to check compatibility before creating a context.
#[no_mangle]
pub extern "C" fn ImpellerGetVersion() -> u32 {
    IMPELLER_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_packing_round_trips() {
        let v = make_version(1, 1, 4, 0);
        assert_eq!(version_variant(v), 1);
        assert_eq!(version_major(v), 1);
        assert_eq!(version_minor(v), 4);
        assert_eq!(version_patch(v), 0);
    }

    #[test]
    fn version_fields_do_not_bleed_into_each_other() {
        // Maximum values per field, to catch a shift or mask that overlaps a
        // neighbor. A packing bug here would make version negotiation accept
        // or reject the wrong callers.
        let v = make_version(0b111, 0x7f, 0x3ff, 0xfff);
        assert_eq!(version_variant(v), 0b111);
        assert_eq!(version_major(v), 0x7f);
        assert_eq!(version_minor(v), 0x3ff);
        assert_eq!(version_patch(v), 0xfff);
    }

    #[test]
    fn exported_version_matches_the_constant() {
        assert_eq!(ImpellerGetVersion(), IMPELLER_VERSION);
    }
}

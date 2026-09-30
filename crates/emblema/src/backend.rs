//! Choosing a rendering backend at runtime.
//!
//! The backend is selected when a context is created rather than when the
//! binary is built, because one binary frequently has to run on machines with
//! different graphics stacks — an embedded image that boots on a board with a
//! working Vulkan driver and on one where only GLES is usable should not be two
//! builds.
//!
//! That selection is an enumeration rather than a trait object. The HAL trait
//! has associated types, which is what keeps it zero-cost and lets each backend
//! name its own texture and fence, and a trait with associated types cannot be
//! made into an object. Enumerating the compiled-in backends costs one branch
//! per call and keeps the types intact.

use emblema_hal::Error;

/// Which backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendPreference {
    /// Try each compiled-in backend in order of preference.
    ///
    /// Vulkan first: it is the reference implementation, the backend features
    /// land on first, and the one every other is diffed against.
    #[default]
    Auto,
    /// Vulkan, or fail.
    Vulkan,
    /// GLES, or fail.
    Gles,
}

/// Which backend a context ended up using.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Vulkan,
    Gles,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
            Self::Gles => "gles",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The error reported when nothing usable was found.
///
/// Carries why each candidate was rejected. A bare "no backend available" is
/// nearly impossible to act on, and the usual cause — a driver present but
/// missing an extension, or a display server holding a device — is only visible
/// in the individual failures.
pub(crate) fn no_backend(attempts: Vec<(Backend, Error)>) -> Error {
    if attempts.is_empty() {
        return Error::Unsupported("no rendering backend was compiled in");
    }
    let detail = attempts
        .iter()
        .map(|(backend, error)| format!("  {backend}: {error}"))
        .collect::<Vec<_>>()
        .join("\n");
    Error::Backend {
        backend: "emblema",
        detail: format!("no usable rendering backend:\n{detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_names_every_candidate_and_its_reason() {
        let error = no_backend(vec![
            (Backend::Vulkan, Error::Unsupported("no device")),
            (
                Backend::Gles,
                Error::Backend {
                    backend: "gles",
                    detail: "libEGL missing".into(),
                },
            ),
        ]);
        let text = error.to_string();
        // Acting on this requires knowing which backend failed and why; a bare
        // "nothing available" leaves a user with nowhere to start.
        assert!(text.contains("vulkan"), "{text}");
        assert!(text.contains("no device"), "{text}");
        assert!(text.contains("gles"), "{text}");
        assert!(text.contains("libEGL missing"), "{text}");
    }

    #[test]
    fn a_build_with_no_backends_says_so_rather_than_blaming_the_machine() {
        let error = no_backend(Vec::new());
        assert!(error.to_string().contains("compiled in"));
    }

    #[test]
    fn auto_is_the_default_preference() {
        assert_eq!(BackendPreference::default(), BackendPreference::Auto);
    }
}

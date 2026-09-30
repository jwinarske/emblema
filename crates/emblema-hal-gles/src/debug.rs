//! Capturing driver diagnostics so tests can assert on them.
//!
//! The counterpart of the Vulkan backend's validation log, and here for the
//! same reason: a diagnostic that only reaches stderr is noticed when somebody
//! happens to look, and code that misuses the API can pass an entire suite
//! while appearing to work.
//!
//! What is available differs from Vulkan in kind. There is no layer to load —
//! `GL_KHR_debug` is the driver reporting on itself, which every GLES 3.2
//! implementation offers and Mesa implements well. It reports API errors,
//! undefined behavior, and portability problems, which is narrower than a
//! validation layer: it does not track object lifetimes or synchronization.
//! What it does cover is exactly what this backend is most likely to get
//! wrong, since GLES is a state machine and most mistakes here are a call made
//! with the wrong state bound.
//!
//! Messages are requested synchronously, so one arrives inside the call that
//! caused it rather than at some later flush. That costs time, which is why it
//! is off unless asked for, and it is what makes a captured message point at
//! the code responsible.

use std::sync::{Arc, Mutex};

/// One diagnostic from the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugMessage {
    pub severity: DebugSeverity,
    /// The driver's message id, which identifies the diagnostic across runs.
    pub id: u32,
    pub message: String,
}

/// How serious the driver considers a message.
///
/// The same three levels the Vulkan side reports, mapped from `GL_DEBUG_SEVERITY_*`
/// so that a test asserting "no errors" means the same thing on both backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DebugSeverity {
    Info,
    Warning,
    Error,
}

impl DebugSeverity {
    /// Classify a `GL_DEBUG_SEVERITY_*` value.
    ///
    /// High is an error: the enumeration reserves it for API errors,
    /// undefined behavior, and other cases where the program is wrong rather
    /// than merely slow. Medium is a warning, which covers deprecation and
    /// portability. Low and notification are information.
    pub fn from_gl(severity: u32) -> Self {
        match severity {
            glow::DEBUG_SEVERITY_HIGH => Self::Error,
            glow::DEBUG_SEVERITY_MEDIUM => Self::Warning,
            _ => Self::Info,
        }
    }
}

/// Messages collected from the driver for the lifetime of a context.
#[derive(Debug, Default)]
pub struct DebugLog {
    messages: Mutex<Vec<DebugMessage>>,
}

impl DebugLog {
    pub fn record(&self, message: DebugMessage) {
        // A poisoned lock means a previous callback panicked. Dropping the
        // message beats panicking again inside a driver callback, where
        // unwinding across the boundary is undefined.
        if let Ok(mut guard) = self.messages.lock() {
            guard.push(message);
        }
    }

    pub fn messages(&self) -> Vec<DebugMessage> {
        self.messages.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Messages at error severity, which are the ones that fail a test.
    pub fn errors(&self) -> Vec<DebugMessage> {
        self.messages()
            .into_iter()
            .filter(|m| m.severity == DebugSeverity::Error)
            .collect()
    }

    pub fn is_clean(&self) -> bool {
        self.errors().is_empty()
    }
}

/// Turn on synchronous debug output and route it into `log`.
///
/// Returns false where the driver offers no debug output, which is not an
/// error: the context works, and what a caller loses is the checking rather
/// than the rendering.
///
/// # Safety
///
/// `gl` must be current on this thread.
pub unsafe fn install(gl: &mut glow::Context, log: Arc<DebugLog>) -> bool {
    // SAFETY: the caller guarantees what the `# Safety` note on this
    // function requires; every operation below needs exactly that and
    // nothing more.
    unsafe {
        use glow::HasContext as _;

        // Synchronous, so the callback runs inside the call that caused the
        // message. Without it the driver may batch, and a message that arrives
        // later names no useful place.
        gl.enable(glow::DEBUG_OUTPUT);
        gl.enable(glow::DEBUG_OUTPUT_SYNCHRONOUS);
        // Everything, then let the log filter. Asking the driver to drop
        // categories here would mean deciding what matters before seeing it.
        gl.debug_message_control(glow::DONT_CARE, glow::DONT_CARE, glow::DONT_CARE, &[], true);
        gl.debug_message_callback(move |_source, _kind, id, severity, text| {
            log.record(DebugMessage {
                severity: DebugSeverity::from_gl(severity),
                id,
                message: text.to_string(),
            });
        });
        true
    }
}

/// A context that asserts the driver reported nothing when it goes out of scope.
///
/// The counterpart of the Vulkan backend's guard, and the same argument: a
/// context that turns diagnostics on and never reads them is indistinguishable
/// from one that left them off, and a check written out in every test is one
/// some test will be missing.
///
/// Derefs to the context, so code that takes a [`GlesContext`] is unchanged.
///
/// [`GlesContext`]: crate::GlesContext
pub struct Validated(std::mem::ManuallyDrop<crate::GlesContext>);

impl Validated {
    /// A context with debug output requested, or whatever went wrong instead.
    pub fn new(target: crate::DisplayTarget) -> emblema_hal::Result<Self> {
        crate::GlesContext::with_config(crate::GlesConfig {
            target,
            debug: true,
            ..Default::default()
        })
        .map(|ctx| Self(std::mem::ManuallyDrop::new(ctx)))
    }

    /// The same, built without the capabilities named.
    ///
    /// The Vulkan backend's constructor of this name says what it is for. Here
    /// too the `Result` shape matches [`Self::new`], so a caller's skip line does
    /// not change.
    pub fn without(
        target: crate::DisplayTarget,
        withheld: impl Into<emblema_hal::Withheld>,
    ) -> emblema_hal::Result<Self> {
        crate::GlesContext::with_config(crate::GlesConfig {
            target,
            debug: true,
            withheld: withheld.into(),
        })
        .map(|ctx| Self(std::mem::ManuallyDrop::new(ctx)))
    }
}

impl std::ops::Deref for Validated {
    type Target = crate::GlesContext;

    fn deref(&self) -> &crate::GlesContext {
        &self.0
    }
}

impl std::ops::DerefMut for Validated {
    fn deref_mut(&mut self) -> &mut crate::GlesContext {
        &mut self.0
    }
}

impl Drop for Validated {
    fn drop(&mut self) {
        // Dropped here rather than after this body, matching the other backend
        // and for a reason that holds on this one too. A context deletes its
        // program, its buffers and its placeholder while it is still current,
        // so an error raised by any of those reaches the debug callback -- and
        // a check written in the ordinary arrangement runs before all of it.
        //
        // What does not carry over is the fault that motivated it there. GL has
        // no object tracking, so nothing reports an object outliving its
        // context; destroying the EGL context releases what it owns and there
        // is no undefined behavior to report. This covers the deletions
        // themselves, which is less, and is what there is.
        let panicking = std::thread::panicking();
        let active = self.0.debug_active();
        let log = self.0.debug_log();
        // SAFETY: this is the only place the context is dropped, `Drop::drop`
        // runs once, and nothing reads the field afterward.
        unsafe { std::mem::ManuallyDrop::drop(&mut self.0) };

        // A test that is already failing keeps its own message: panicking here
        // while the first panic unwinds aborts the process and loses it.
        if panicking {
            return;
        }
        if !active {
            eprintln!("skipping: this driver reports no diagnostics, so API use went unchecked");
            return;
        }
        let errors: Vec<_> = log
            .messages()
            .into_iter()
            .filter(|message| message.severity == DebugSeverity::Error)
            .collect();
        assert!(errors.is_empty(), "driver reported errors: {errors:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_maps_the_way_the_vulkan_side_does() {
        // The two backends' logs are read by the same kind of assertion, so a
        // level that meant something different on each would make "no errors"
        // two different claims.
        assert_eq!(
            DebugSeverity::from_gl(glow::DEBUG_SEVERITY_HIGH),
            DebugSeverity::Error
        );
        assert_eq!(
            DebugSeverity::from_gl(glow::DEBUG_SEVERITY_MEDIUM),
            DebugSeverity::Warning
        );
        assert_eq!(
            DebugSeverity::from_gl(glow::DEBUG_SEVERITY_LOW),
            DebugSeverity::Info
        );
        assert_eq!(
            DebugSeverity::from_gl(glow::DEBUG_SEVERITY_NOTIFICATION),
            DebugSeverity::Info
        );
    }

    #[test]
    fn a_log_reports_only_errors_as_unclean() {
        let log = DebugLog::default();
        log.record(DebugMessage {
            severity: DebugSeverity::Warning,
            id: 1,
            message: "deprecated".into(),
        });
        assert!(log.is_clean(), "a warning is not a failure");
        log.record(DebugMessage {
            severity: DebugSeverity::Error,
            id: 2,
            message: "invalid operation".into(),
        });
        assert!(!log.is_clean());
        assert_eq!(log.errors().len(), 1);
        assert_eq!(log.messages().len(), 2);
    }
}

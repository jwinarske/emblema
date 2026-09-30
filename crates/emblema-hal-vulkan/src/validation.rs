//! Capturing validation-layer output so tests can assert on it.
//!
//! Validation errors that only reach stderr are worth very little: they are
//! noticed when someone happens to look, and a device created invalid can pass
//! an entire suite while appearing to work. Routing the messenger into a log
//! the context owns turns "validation-clean" from something a developer
//! remembers to check into something a test asserts.

use ash::vk;
use std::ffi::{c_void, CStr};
use std::sync::Mutex;

/// The layer name, requested at instance creation when validation is on.
pub const VALIDATION_LAYER: &str = "VK_LAYER_KHRONOS_validation";

/// One diagnostic from the validation layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationMessage {
    pub severity: ValidationSeverity,
    /// The VUID or message identifier, where the layer supplies one.
    pub id: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValidationSeverity {
    Info,
    Warning,
    Error,
}

/// Messages collected from the layer for the lifetime of a context.
#[derive(Debug, Default)]
pub struct ValidationLog {
    messages: Mutex<Vec<ValidationMessage>>,
}

impl ValidationLog {
    pub fn record(&self, message: ValidationMessage) {
        // A poisoned lock means a previous callback panicked. Dropping the
        // message is better than panicking again inside a driver callback,
        // where unwinding across the FFI boundary is undefined.
        if let Ok(mut guard) = self.messages.lock() {
            guard.push(message);
        }
    }

    pub fn messages(&self) -> Vec<ValidationMessage> {
        self.messages.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Messages at error severity, which are the ones that fail a test.
    pub fn errors(&self) -> Vec<ValidationMessage> {
        self.messages()
            .into_iter()
            .filter(|m| m.severity == ValidationSeverity::Error)
            .collect()
    }

    pub fn is_clean(&self) -> bool {
        self.errors().is_empty()
    }
}

/// The messenger callback.
///
/// # Safety
///
/// Invoked by the loader with a valid callback-data pointer, and with
/// `user_data` set to the [`ValidationLog`] pointer supplied at messenger
/// creation. The log outlives the messenger because the context destroys the
/// messenger before releasing the log.
pub(crate) unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    // Never unwind out of here: this frame is owned by the driver.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if data.is_null() || user_data.is_null() {
            return;
        }
        let data = unsafe { &*data };
        let log = unsafe { &*(user_data as *const ValidationLog) };

        let severity = if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
            ValidationSeverity::Error
        } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
            ValidationSeverity::Warning
        } else {
            ValidationSeverity::Info
        };

        let cstr_or_empty = |p: *const std::ffi::c_char| {
            if p.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
            }
        };

        log.record(ValidationMessage {
            severity,
            id: cstr_or_empty(data.p_message_id_name),
            message: cstr_or_empty(data.p_message),
        });
    }));
    let _ = result;

    // False means "do not abort the call that triggered this". The layer is a
    // reporting mechanism here, not a policy one; tests decide what is fatal.
    vk::FALSE
}

/// Severities and types worth subscribing to.
pub(crate) fn messenger_create_info<'a>() -> vk::DebugUtilsMessengerCreateInfoEXT<'a> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback))
}

/// A context that asserts the layer reported nothing when it goes out of scope.
///
/// The other half of what this module is for. Capturing the messenger makes
/// validation assertable; this makes it asserted, which is not the same thing:
/// a test that turns the layer on and never reads what it said is
/// indistinguishable from one that left it off, and a check written out in
/// every test is one some test will be missing. That is not hypothetical --
/// the layer was on for this crate's own tests and off for the whole shared
/// harness, which is the largest body of rendering in the suite.
///
/// Dropping is the one moment that happens exactly once per context however
/// the test around it is written, so that is where the check goes.
///
/// Derefs to the context, so code that takes a [`crate::VulkanContext`] is unchanged.
pub struct Validated(std::mem::ManuallyDrop<crate::VulkanContext>);

impl Validated {
    /// A context with the layer requested, or whatever went wrong instead.
    pub fn new(device: crate::DevicePreference) -> emblema_hal::Result<Self> {
        crate::VulkanContext::with_config(crate::ContextConfig {
            device,
            validation: true,
            ..Default::default()
        })
        .map(|ctx| Self(std::mem::ManuallyDrop::new(ctx)))
    }

    /// The same, built without the capabilities named.
    ///
    /// What a test reaches for to exercise a refusal on a machine whose device
    /// has the thing. Takes one capability or a set, so the ordinary case reads
    /// `Validated::without(DevicePreference::Auto, Capability::AdvancedBlend)`.
    ///
    /// Withholding only; `emblema_hal::Withheld` says why there is no other
    /// direction. The same `Result` shape as [`Self::new`] on purpose, so the
    /// `let Ok(ctx) = .. else { skip }` every test here opens with is unchanged.
    ///
    /// A restricted device is still checked by the layer, which matters more here
    /// than for an ordinary context: leaving an extension out is a configuration
    /// nothing else in this suite creates, and a finding would be worth seeing
    /// rather than guessing at.
    pub fn without(
        device: crate::DevicePreference,
        withheld: impl Into<emblema_hal::Withheld>,
    ) -> emblema_hal::Result<Self> {
        crate::VulkanContext::with_config(crate::ContextConfig {
            device,
            validation: true,
            withheld: withheld.into(),
        })
        .map(|ctx| Self(std::mem::ManuallyDrop::new(ctx)))
    }
}

impl std::ops::Deref for Validated {
    type Target = crate::VulkanContext;

    fn deref(&self) -> &crate::VulkanContext {
        &self.0
    }
}

impl std::ops::DerefMut for Validated {
    fn deref_mut(&mut self) -> &mut crate::VulkanContext {
        &mut self.0
    }
}

impl Drop for Validated {
    fn drop(&mut self) {
        // The context is dropped here rather than after this body, which is
        // what the `ManuallyDrop` is for and the whole reason this is written
        // the awkward way. A field's drop runs after its owner's, so checking
        // the log from here in the ordinary arrangement checks everything the
        // context did *except* its own teardown -- and a Vulkan object
        // outliving its device is reported at `vkDestroyDevice`, inside that
        // teardown. That gap is not hypothetical: a descriptor set layout
        // leaked on every device in this workspace for as long as the material
        // set has existed, with every validated test green.
        //
        // The log is behind an `Arc`, so a handle taken before the context goes
        // is still readable after it.
        let panicking = std::thread::panicking();
        let active = self.0.validation_active();
        let log = self.0.validation_log();
        // SAFETY: this is the only place the context is dropped, `Drop::drop`
        // runs once, and nothing reads the field afterward.
        unsafe { std::mem::ManuallyDrop::drop(&mut self.0) };

        // A test that is already failing keeps its own message: panicking here
        // while the first panic unwinds aborts the process and loses it.
        if panicking {
            return;
        }
        if !active {
            // Said rather than passed over. The layer being absent means the
            // work ran with nothing checking its API use, which is a gap in
            // what the run covered even where every pixel matched.
            eprintln!("skipping: the validation layer is unavailable, so API use went unchecked");
            return;
        }
        let errors = log.errors();
        assert!(errors.is_empty(), "validation errors: {errors:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_with_only_warnings_is_still_clean() {
        let log = ValidationLog::default();
        log.record(ValidationMessage {
            severity: ValidationSeverity::Warning,
            id: "test".into(),
            message: "a warning".into(),
        });
        // Warnings are recorded for inspection but do not fail a run: drivers
        // emit performance advice that is not a correctness problem.
        assert!(log.is_clean());
        assert_eq!(log.messages().len(), 1);
        assert!(log.errors().is_empty());
    }

    #[test]
    fn errors_are_separated_from_the_rest() {
        let log = ValidationLog::default();
        for severity in [
            ValidationSeverity::Info,
            ValidationSeverity::Warning,
            ValidationSeverity::Error,
        ] {
            log.record(ValidationMessage {
                severity,
                id: "id".into(),
                message: format!("{severity:?}"),
            });
        }
        assert!(!log.is_clean());
        assert_eq!(log.errors().len(), 1);
        assert_eq!(log.errors()[0].severity, ValidationSeverity::Error);
        assert_eq!(log.messages().len(), 3);
    }
}

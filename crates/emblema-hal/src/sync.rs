//! GPU completion, and handing it to something outside this process.

use crate::error::Result;
use std::time::Duration;

/// A signal that GPU work has completed.
///
/// This is the second of the two requirements that exist in the HAL from day
/// one for the sake of the DRM path. Explicit sync is the design center of the
/// frame loop: on the DRM path the render-done primitive is exported as a
/// `sync_file` fd and attached to the atomic commit as `IN_FENCE_FD`, so the
/// kernel latches the flip when the fence signals. Neither `glFinish` nor
/// `vkDeviceWaitIdle` belongs in the frame loop on any path.
///
/// Backing primitives differ per backend — Vulkan fences, `GLsync` objects,
/// sync_file fds — and the fence waiter thread retires all of them uniformly.
pub trait HalFence: Send + Sync + 'static {
    /// Whether the fence has signaled, without blocking.
    fn is_signaled(&self) -> Result<bool>;

    /// Block until the fence signals or the timeout elapses.
    ///
    /// Returns `Ok(true)` on signal and `Ok(false)` on timeout, so an expected
    /// timeout is not an error. A caller that treats timeout as fatal checks
    /// the returned value.
    fn wait(&self, timeout: Duration) -> Result<bool>;

    /// Export as a `sync_file` fd for a consumer outside this process.
    ///
    /// Returns [`crate::Error::Unsupported`] where the driver cannot export.
    /// Callers check [`crate::SyncSupport::export_sync_file`] first and fall
    /// back to a CPU-side wait before commit — correct, slower, and worth
    /// logging loudly, since on Tier-1 hardware it is a driver bug to chase
    /// rather than a state to settle into.
    #[cfg(unix)]
    fn export_sync_file(&self) -> Result<std::os::fd::OwnedFd> {
        Err(crate::Error::Unsupported("sync_file export"))
    }
}

/// A fence for a backend that cannot submit without waiting.
///
/// Uninhabited, so every method is unreachable: no value can exist to call one
/// on. That is a stronger statement than a type whose methods return errors,
/// which would be constructible and could reach a caller expecting a real
/// fence.
impl HalFence for std::convert::Infallible {
    fn is_signaled(&self) -> Result<bool> {
        match *self {}
    }

    fn wait(&self, _timeout: Duration) -> Result<bool> {
        match *self {}
    }
}

/// How long a frame-loop wait should tolerate before it is considered a hang.
///
/// Generous relative to any real frame: a wait reaching this has hit a lost
/// device or a fence that will never signal, not a slow frame.
pub const FRAME_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestFence {
        signaled: AtomicBool,
    }

    impl HalFence for TestFence {
        fn is_signaled(&self) -> Result<bool> {
            Ok(self.signaled.load(Ordering::Acquire))
        }

        fn wait(&self, _timeout: Duration) -> Result<bool> {
            self.is_signaled()
        }
    }

    #[test]
    fn fence_export_defaults_to_unsupported_rather_than_panicking() {
        let fence = TestFence {
            signaled: AtomicBool::new(false),
        };
        // A backend that has not implemented export must degrade to the
        // CPU-wait fallback, so the default has to be a recoverable error.
        #[cfg(unix)]
        assert!(matches!(
            fence.export_sync_file(),
            Err(Error::Unsupported(_))
        ));
        assert!(!fence.is_signaled().unwrap());
    }

    #[test]
    fn timeout_is_reported_as_a_value_not_an_error() {
        let fence = TestFence {
            signaled: AtomicBool::new(false),
        };
        // Waiting out a frame slot that is still in flight is ordinary, so it
        // must not surface as Err.
        assert!(!fence.wait(Duration::from_millis(0)).unwrap());
        fence.signaled.store(true, Ordering::Release);
        assert!(fence.wait(Duration::from_millis(0)).unwrap());
    }
}

//! A GPU fence for the GLES backend, built on `EGL_ANDROID_native_fence_sync`.
//!
//! This backend had no fence at all until now: `Hal::Fence` was
//! `std::convert::Infallible`, so one could not be constructed, and
//! `SyncSupport` reported false whatever EGL advertised. That was honest but
//! it closed two doors at once — a caller could not wait on GPU completion,
//! and a display path could not obtain the `IN_FENCE_FD` an atomic commit
//! wants, so `execute_deferred` was unavailable on this backend entirely.
//!
//! What makes it a *native* fence rather than an EGL-internal one is the
//! attribute at creation: with `EGL_SYNC_NATIVE_FENCE_ANDROID` the object can
//! be dup'd out as a `sync_file` descriptor and handed to something outside
//! this process, which is the whole reason a scanout ring needs it. An
//! `EGL_KHR_fence_sync` object cannot leave, so it is not enough here even
//! though it would serve the two waits.

use crate::context::Egl;
use emblema_hal::{Error, HalFence, Result};
use std::sync::Arc;
use std::time::Duration;

/// `EGL_SYNC_NATIVE_FENCE_ANDROID`, the sync type that can be exported.
pub(crate) const SYNC_NATIVE_FENCE_ANDROID: khronos_egl::Enum = 0x3144;

/// `eglDupNativeFenceFDANDROID`, which `khronos-egl` does not wrap.
type DupNativeFenceFd =
    unsafe extern "system" fn(khronos_egl::EGLDisplay, khronos_egl::EGLSync) -> khronos_egl::Int;

/// `EGL_NO_NATIVE_FENCE_FD_ANDROID`, returned when there is nothing to dup.
const NO_NATIVE_FENCE_FD: khronos_egl::Int = -1;

/// A submission's completion, as an EGL sync object.
pub struct GlesFence {
    egl: Arc<Egl>,
    display: khronos_egl::Display,
    sync: khronos_egl::Sync,
    /// Whether this one can be exported, which is a property of how it was
    /// created rather than of the driver: a fence made without the native
    /// attribute serves the waits and cannot leave the process.
    exportable: bool,
}

// SAFETY: an EGLSync belongs to an EGLDisplay rather than to a thread or a
// context, and the specification allows any thread to wait on one. The two
// pointers this holds are the display and the sync, both of which the EGL
// implementation owns for as long as this value lives -- `Drop` is what ends
// that, and it runs once. `Egl` is behind an `Arc` so the dispatch table
// outlives every fence made through it.
unsafe impl Send for GlesFence {}
unsafe impl Sync for GlesFence {}

impl GlesFence {
    /// Create a fence for work already submitted on the current context.
    ///
    /// The caller must have issued the work first; this marks the point in the
    /// command stream that the fence signals at. A flush is required and is
    /// the caller's — `eglCreateSync` places the fence in the stream but does
    /// not guarantee the stream reaches the driver, and a native fence that is
    /// never flushed never signals, which reads as a hang rather than an
    /// error.
    pub(crate) fn create(
        egl: Arc<Egl>,
        display: khronos_egl::Display,
        exportable: bool,
    ) -> Result<Self> {
        let kind: khronos_egl::Enum = if exportable {
            SYNC_NATIVE_FENCE_ANDROID
        } else {
            khronos_egl::SYNC_FENCE as khronos_egl::Enum
        };
        // SAFETY: `display` is a live display this instance initialized, and
        // the attribute list is terminated.
        let sync = unsafe { egl.create_sync(display, kind, &[khronos_egl::ATTRIB_NONE]) }.map_err(
            |e| Error::Backend {
                backend: "gles",
                detail: format!("eglCreateSync: {e}"),
            },
        )?;
        Ok(Self {
            egl,
            display,
            sync,
            exportable,
        })
    }

    fn client_wait(&self, timeout_ns: u64) -> Result<bool> {
        // SAFETY: the sync belongs to this display and is live until `Drop`.
        let status = unsafe {
            self.egl
                .client_wait_sync(self.display, self.sync, 0, timeout_ns)
        }
        .map_err(|e| Error::Backend {
            backend: "gles",
            detail: format!("eglClientWaitSync: {e}"),
        })?;
        Ok(status == khronos_egl::CONDITION_SATISFIED)
    }
}

impl HalFence for GlesFence {
    fn is_signaled(&self) -> Result<bool> {
        // A zero timeout is the nonblocking query: signaled comes back as
        // satisfied, and anything else as expired.
        self.client_wait(0)
    }

    fn wait(&self, timeout: Duration) -> Result<bool> {
        // Saturating, because EGL takes nanoseconds in a u64 and a caller may
        // pass a Duration that does not fit. Clamping to FOREVER is what a
        // caller asking for three hundred years meant.
        let ns = u64::try_from(timeout.as_nanos()).unwrap_or(khronos_egl::FOREVER);
        self.client_wait(ns)
    }

    #[cfg(unix)]
    fn export_sync_file(&self) -> Result<std::os::fd::OwnedFd> {
        use std::os::fd::FromRawFd;

        if !self.exportable {
            return Err(Error::Unsupported(
                "this fence was not created as a native fence and cannot be exported",
            ));
        }
        let Some(dup) = self.egl.get_proc_address("eglDupNativeFenceFDANDROID") else {
            return Err(Error::Unsupported("eglDupNativeFenceFDANDROID"));
        };
        // SAFETY: the name resolves only where the extension is present, and
        // the signature is the one the extension defines.
        let dup: DupNativeFenceFd = unsafe { std::mem::transmute(dup) };
        let fd = unsafe { dup(self.display.as_ptr(), self.sync.as_ptr()) };
        if fd == NO_NATIVE_FENCE_FD {
            return Err(Error::Backend {
                backend: "gles",
                detail: "eglDupNativeFenceFDANDROID returned no descriptor".into(),
            });
        }
        // SAFETY: the call returns a descriptor this process now owns, and
        // dup'ing is what makes it independent of the sync object's lifetime.
        Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
    }
}

impl Drop for GlesFence {
    fn drop(&mut self) {
        // Destroying a sync object does not wait for it, and does not cancel
        // the work: an exported descriptor keeps the underlying fence alive on
        // its own, which is what lets a commit outlive the fence it was given.
        // SAFETY: destroyed exactly once, here, and not used afterward.
        let _ = unsafe { self.egl.destroy_sync(self.display, self.sync) };
    }
}

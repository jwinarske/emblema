//! The display-side surface this crate requires.
//!
//! Everything to do with KMS — connector and plane discovery, mode selection,
//! atomic commit construction, page-flip events, framebuffer import — belongs
//! to the KMS binding. This crate reimplements none of it, and the dependency
//! runs one way: the binding knows nothing about rendering.
//!
//! That surface is expressed here as a trait rather than consumed directly, for
//! two reasons. It states what a binding must provide, so the two projects can
//! be sequenced against each other rather than discovering the mismatch at
//! integration. And it makes the frame loop testable without a display: the
//! parts most worth testing are the ring accounting and the fence plumbing,
//! neither of which needs real hardware to get wrong.
//!
//! "A binding" rather than the one in use, because a second is expected and the
//! first has no special claim. drm-rs implements this today, in `kms.rs` and
//! `device.rs`, and those two files are the only ones in the crate permitted to
//! name it — this file, the frame loop, and the types below name nothing from
//! it, which is what makes replacing it a matter of writing another
//! implementation rather than editing this one. A test enforces that, because
//! a single convenient import would end it silently.

use emblema_hal::{Extent2D, FormatModifierSet, Fourcc, Modifier, Result};

/// A display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub extent: Extent2D,
    /// Refresh rate in millihertz, which is how KMS reports it.
    pub refresh_mhz: u32,
}

impl Mode {
    /// Nanoseconds between vertical blanks, for pacing budgets.
    pub fn frame_nanos(&self) -> u64 {
        if self.refresh_mhz == 0 {
            return 0;
        }
        1_000_000_000_000u64 / self.refresh_mhz as u64
    }
}

/// A framebuffer the display side has built from an imported buffer.
///
/// Opaque here: what it identifies is drm-rs's business, and this crate only
/// needs to name one in a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FbHandle(pub u64);

/// A buffer being handed to the display side.
#[derive(Debug)]
pub struct DmaBufPlanes {
    #[cfg(unix)]
    pub planes: Vec<emblema_hal::DmaBufPlane>,
    pub fourcc: Fourcc,
    pub modifier: Modifier,
    pub extent: Extent2D,
}

/// One atomic commit.
#[derive(Debug)]
pub struct CommitRequest {
    pub fb: FbHandle,
    /// The render-done signal, attached so the kernel latches the flip when
    /// rendering completes rather than the caller blocking first.
    ///
    /// `None` means the caller has already waited on the CPU, which is correct
    /// but costs a frame of latency. That fallback exists for drivers that
    /// cannot export a sync_file and is reported rather than assumed.
    #[cfg(unix)]
    pub in_fence_fd: Option<std::os::fd::OwnedFd>,
    /// Whether this commit may change the mode, which is far more expensive
    /// than a flip and is only wanted on the first frame or after a hotplug.
    pub allow_modeset: bool,
}

/// Something that happened on the display side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputEvent {
    /// A previously committed framebuffer is now on screen, which means the one
    /// it replaced is free to reuse.
    FlipComplete { fb: FbHandle },
    /// The mode or the connector changed; everything must be rebuilt.
    Reconfigured,
}

/// The subset of drm-rs this crate consumes.
pub trait ScanoutOutput {
    fn mode(&self) -> Mode;

    /// Modes this output can be driven at, preferred first.
    ///
    /// What a caller may resize *to*. A display's geometry is the display's to
    /// offer, so a size that is not in here is not a size this output has.
    ///
    /// The default is the one mode it is at, which is the truth for an output
    /// that cannot switch.
    fn modes(&self) -> Vec<Mode> {
        vec![self.mode()]
    }

    /// Switch to one of [`Self::modes`].
    ///
    /// The commit after this has to set the mode; a flip alone would put the
    /// new buffers on a pipeline still configured for the old size, which the
    /// kernel refuses outright.
    ///
    /// The default accepts the mode it is already at and refuses every other,
    /// which is what an output with one mode can honestly say.
    fn set_mode(&mut self, mode: Mode) -> Result<()> {
        if mode == self.mode() {
            return Ok(());
        }
        Err(emblema_hal::Error::Unsupported(
            "this output cannot change mode",
        ))
    }

    /// Formats and layouts the plane can scan out.
    ///
    /// One half of negotiation. A plane that advertised nothing would leave the
    /// render side guessing, and guessing means linear.
    fn supported_formats(&self) -> &[FormatModifierSet];

    /// Turn an exported buffer into a framebuffer.
    fn import_dmabuf(&mut self, buffer: DmaBufPlanes) -> Result<FbHandle>;

    /// Release a framebuffer.
    fn release_framebuffer(&mut self, fb: FbHandle);

    /// Submit an atomic commit. Does not block.
    fn commit(&mut self, request: CommitRequest) -> Result<()>;

    /// Take any events that have arrived, without blocking.
    fn poll_events(&mut self) -> Vec<OutputEvent>;

    /// Block until at least one event arrives, or the timeout elapses.
    ///
    /// This is what paces the frame loop: a target waits here rather than
    /// spinning, and returns as soon as the display says a slot is free.
    fn wait_for_event(&mut self, timeout_nanos: u64) -> Result<Vec<OutputEvent>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_reports_its_frame_budget() {
        let sixty = Mode {
            extent: Extent2D::new(1920, 1080),
            refresh_mhz: 60_000,
        };
        // About 16.67 ms, which is the budget every pacing assertion is
        // measured against.
        assert!((sixty.frame_nanos() as i64 - 16_666_666).abs() < 100);
    }

    #[test]
    fn a_mode_with_no_refresh_reports_no_budget_rather_than_dividing_by_zero() {
        let unknown = Mode {
            extent: Extent2D::new(640, 480),
            refresh_mhz: 0,
        };
        assert_eq!(unknown.frame_nanos(), 0);
    }
}

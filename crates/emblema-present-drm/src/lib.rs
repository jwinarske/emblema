//! DRM/KMS direct scanout: rendering straight to a display with no compositor.
//!
//! This is the configuration embedded, automotive, kiosk, and industrial
//! products actually ship, and the one most renderers leave as an exercise. It
//! is a presentation target, not a rendering backend: the renderer draws through
//! a HAL as always, and this decides how the result reaches a panel.
//!
//! Everything to do with KMS belongs to drm-rs and is consumed through
//! [`output::ScanoutOutput`], which states exactly what this crate requires.
//! Expressing it as a trait lets the two projects be sequenced against each
//! other, and makes the parts most worth testing — ring accounting and fence
//! plumbing — testable without a display attached.
//!
//! # What it does on real boards
//!
//! The whole path is: render into a buffer, export it as a dma-buf, hand the fd
//! to the display controller, scan it out. Two Raspberry Pis say that works on
//! one of the three display controllers between them, and the differences are
//! worth having written down, because CI has neither the hardware nor anything
//! that would fail the way it fails.
//!
//! | board | controller | outcome |
//! |---|---|---|
//! | Pi 4 | `vc4` | the import is refused |
//! | Pi 5 | `vc4`, HDMI | **every test passes**, since `possible_crtcs` is honored |
//! | Pi 5 | `rp1-dsi`, DSI | **every test passes** |
//!
//! So direct scanout does work on a board, end to end: over DSI at 800x1280 and
//! over HDMI at 1280x1440, both on a Pi 5. That is the first time anything here
//! has been shown to reach a panel rather than a virtual display controller.
//!
//! And it keeps up with them, with room to spare. Ten seconds on each controller,
//! three runs apiece, measured 2026-09-22: six hundred frames at sixty a second with
//! **no vertical blank missed** and one CPU wait -- the modesetting commit -- in every
//! run. The blanks come from the sequence the kernel reports with each flip rather
//! than from a clock; `pacing` says why, and `docs/on-a-board.md` has the numbers and
//! the preconditions.
//!
//! The room to spare was measured separately, because a three-deep ring can hold a
//! finished buffer back and so absorb a frame that overran. Repeating both controllers
//! at `DEPTH=2`, where it cannot, missed nothing either -- and the same scene grown
//! four times over misses 294 blanks at depth two against 111 at depth three, which is
//! what says the depth was doing something. The scene still fits at four cards and
//! does not at five, so the margin is over one card's work and under two.
//!
//! The HDMI half of that took a fix rather than a discovery. It refused every
//! commit until `primary_plane_for` began honoring the kernel's
//! `possible_crtcs` mask, which the comment there had said would be the precise
//! answer and had not been consulted -- see that function for what the mask
//! says on a Pi 5 and why the first CRTC is the wrong one to pair with the
//! first plane.
//!
//! On both boards the renderer and the display are separate DRM devices --
//! `v3d` renders, something else scans out -- and `v3d` has no display role at
//! all: it carries no connectors and refuses `create_dumb_buffer` with
//! `ENOSYS`. There is no second display device to fall back to.
//!
//! ## The Pi 4: `vc4` will not take the memory
//!
//! ```text
//! driving /dev/dri/card1 at 1280x1440
//! agreed on Fourcc(875713089) with Modifier(0)
//! prime_fd_to_buffer: Invalid argument (os error 22)
//! ```
//!
//! Everything up to the handoff works, which is what makes it precise: master
//! is taken, the connector and mode are read, and negotiation agrees on `XR24`
//! with the linear modifier -- a format and layout the controller itself
//! advertised. It is the buffer that is declined, and that was shown rather
//! than inferred: a dumb buffer allocated on the same card exports and imports
//! straight back through the same call. `vc4` imports dma-bufs; it declines
//! *this* one, which leaves the memory as the only difference.
//!
//! The reason is the board rather than the driver. A Pi 4 has no IOMMU, so its
//! display controller can address only physically contiguous memory, and a
//! render device with an MMU has no reason to allocate that way. There is
//! nothing to arrange differently on this side of the handoff.
//!
//! Worth separating from a claim it is easily confused with: Vulkan itself
//! works on a Pi 4. The corpus comparison renders every scene through `v3d` and
//! agrees with the GLES backend on all of them, and the color anchor holds
//! there too. What a Pi 4 cannot do is *scan out* what Vulkan allocated.
//!
//! ## The Pi 5's `vc4`: it was the plane, not the memory
//!
//! This refused every `atomic_commit` with `EINVAL` and was recorded here as
//! undiagnosed. It was a plane committed to a CRTC that cannot drive it: four
//! CRTCs, forty-eight planes, and a `possible_crtcs` mask of `1110` that
//! excludes exactly the CRTC a single connected output is otherwise given.
//! Honoring the mask fixed it.
//!
//! Worth noticing how it hid. The kernel logs nothing for this, the error names
//! the commit rather than the plane, and every one of the three other
//! controllers tried has a single CRTC, where taking the first plane and the
//! first CRTC is always right.
//!
//! ## What this suggests, and what it does not
//!
//! One of the three controllers refuses and two are fine, and the one that
//! refuses does so because the board it is on has no IOMMU -- which is a fact
//! about the hardware and not a shape this code can be bent into. It is not an
//! ARM limitation, and not a Raspberry Pi one: the Pi 5 does both outputs.
//!
//! The Pi 4 has not been re-tested since the plane fix -- it left the network
//! first -- and the fix is not expected to change it: that failure is at the
//! import, which happens after a plane is chosen and does not depend on which.
//!
//! Where the buffer comes from is the part worth reconsidering, and it is a
//! design question rather than a defect to patch. A board with no IOMMU wants
//! the buffer allocated on the *display* side, where it will be contiguous, and
//! imported into the renderer -- the opposite direction to this, and a
//! different shape for [`target::DrmScanoutTarget`] rather than a fix to its
//! import call. Whether that is worth building depends on whether boards
//! without an IOMMU are a target, which is a question for whoever is shipping
//! rather than for this file.
//!
//! The tests take `EMBLEMA_DRM_CARD` for this reason: a board with more than
//! one display controller otherwise gets whichever `/dev/dri` lists first,
//! which on a Pi 5 is the one that works.

pub mod device;
pub mod kms;
pub mod output;
pub mod pacing;
pub mod target;

pub use output::{CommitRequest, DmaBufPlanes, FbHandle, Mode, OutputEvent, ScanoutOutput};
pub use target::{DrmScanoutTarget, DEFAULT_RING_DEPTH};

pub use kms::KmsOutput;

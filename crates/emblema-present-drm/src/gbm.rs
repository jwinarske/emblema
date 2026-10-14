// SPDX-License-Identifier: Apache-2.0
//! Allocating scanout buffers through GBM.
//!
//! The path this crate has named since it was written. [`DrmScanoutTarget`]
//! refuses with "this device cannot allocate its own scanout buffers; use the
//! GBM path" where [`DmaBufSupport::can_allocate_scanout`] is false, and this
//! is that path: GBM allocates a buffer on the *display's* device, and the
//! renderer imports it through [`HalContext::import_texture`] rather than
//! making its own.
//!
//! Why the display's device and not the GPU's. On a split render/display SoC
//! the two are different devices with different ideas about layout, and a
//! buffer the GPU chose for itself is one the display controller may not
//! accept. Allocating on the display side and importing into the renderer puts
//! the choice where the stricter constraint is.
//!
//! libgbm is opened at run time by `drmkit-gbm`, so this adds no build
//! dependency: a machine with no libgbm reports it as a capability gap on the
//! first allocation rather than failing to build or to start.
//!
//! [`DrmScanoutTarget`]: crate::DrmScanoutTarget
//! [`DmaBufSupport::can_allocate_scanout`]: emblema_hal::DmaBufSupport::can_allocate_scanout
//! [`HalContext::import_texture`]: emblema_hal::HalContext::import_texture

use drmkit_gbm::{GbmBuffer, GbmDevice};
use emblema_hal::{DmaBufPlane, Error, Extent2D, ExternalImageDesc, Fourcc, Modifier, Result};

/// A GBM device opened on a display's descriptor.
///
/// Held by a caller that allocates more than one buffer, because opening it
/// per buffer would dlopen libgbm and re-probe the backend each time.
pub struct GbmAllocator {
    device: GbmDevice,
}

impl GbmAllocator {
    /// Open GBM on the descriptor an output reports.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] where the output has no descriptor — a stand-in
    /// rather than a card — and [`Error::Backend`] where libgbm is missing or
    /// refuses the device, which is the same answer in practice and a
    /// different one in cause, so they are not merged.
    pub fn open(fd: std::os::fd::BorrowedFd<'_>) -> Result<Self> {
        let device = GbmDevice::from_fd(fd).map_err(|e| Error::Backend {
            backend: "gbm",
            detail: format!("opening a GBM device: {e}"),
        })?;
        Ok(Self { device })
    }

    /// Allocate one scanout buffer and describe it for import.
    ///
    /// `modifiers` are the layouts the display accepts, in preference order.
    /// The returned descriptor carries **two** things a caller needs to keep
    /// together: the buffer, which owns the allocation, and the description,
    /// whose descriptors are the renderer's and the display's to take.
    ///
    /// The modifier is read back rather than assumed. A driver may answer a
    /// constrained request with a layout outside the list — measured on two
    /// boards, and `drmkit-gbm` falls back to an unconstrained allocation when
    /// a driver refuses — so what came out is what the display must be told,
    /// not what was asked for.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] where the allocation or the export fails, and
    /// [`Error::Unsupported`] for a buffer of more than one plane, which this
    /// path refuses rather than describing partly.
    pub fn allocate(
        &self,
        extent: Extent2D,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<Allocated> {
        let raw: Vec<u64> = modifiers.iter().map(|m| m.0).collect();
        let buffer = GbmBuffer::create_with_modifiers(
            &self.device,
            extent.width,
            extent.height,
            fourcc.0,
            &raw,
        )
        .map_err(|e| Error::Backend {
            backend: "gbm",
            detail: format!(
                "allocating {}x{} as {fourcc}: {e}",
                extent.width, extent.height
            ),
        })?;

        if buffer.plane_count() != 1 {
            // Refused rather than described partly, for the reason the Vulkan
            // import gives: a picture built from plane zero of several looks
            // nearly right, which is worse than an error.
            return Err(Error::Unsupported("a GBM buffer of more than one plane"));
        }
        Ok(Allocated {
            fourcc: Fourcc(buffer.fourcc()),
            modifier: Modifier(buffer.modifier()),
            stride: buffer.stride(),
            buffer,
        })
    }
}

/// A GBM buffer and what it takes to hand it to a renderer and a display.
///
/// The buffer is kept because it owns the allocation: dropping it while a
/// framebuffer or a texture still refers to the memory is the one way to use
/// this wrongly.
pub struct Allocated {
    buffer: GbmBuffer,
    fourcc: Fourcc,
    modifier: Modifier,
    stride: u32,
}

impl Allocated {
    /// The layout the driver actually chose, which may not be one that was asked for.
    pub fn modifier(&self) -> Modifier {
        self.modifier
    }

    /// The format the buffer came back as.
    pub fn fourcc(&self) -> Fourcc {
        self.fourcc
    }

    /// Describe the buffer for one importer.
    ///
    /// Called once per importer rather than once per buffer, because the
    /// renderer and the display each need a descriptor of their own and
    /// ownership of a descriptor transfers to whoever imports it. `gbm_bo_get_fd`
    /// returns a new one on every call, so two importers get two descriptors
    /// onto the same allocation.
    ///
    /// # Errors
    ///
    /// [`Error::Backend`] where the export fails.
    pub fn describe(&self) -> Result<ExternalImageDesc> {
        let fd = self.buffer.export().map_err(|e| Error::Backend {
            backend: "gbm",
            detail: format!("exporting a GBM buffer as a dma-buf: {e}"),
        })?;
        Ok(ExternalImageDesc {
            planes: vec![DmaBufPlane {
                fd,
                offset: 0,
                stride: self.stride,
            }],
            fourcc: self.fourcc,
            modifier: self.modifier,
        })
    }
}

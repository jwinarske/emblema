//! Rendering HAL trait: the backend-agnostic contract every rendering backend
//! implements.
//!
//! The HAL answers how draw commands become pixels in a GPU image.
//! Presentation — how a finished image reaches a display, and how the frame
//! loop is paced — is a separate, orthogonal axis and lives in
//! `emblema-present`.
//!
//! # Shape of the trait
//!
//! Deliberately Vulkan-leaning. Recording commands and replaying them as GL
//! calls works; extracting Vulkan-grade explicitness from an immediate-mode
//! abstraction does not. GLES pays a small CPU cost for command recording, and
//! that is the accepted trade. The trait is never reduced to a lowest common
//! denominator to accommodate a weaker backend: backends implement, emulate,
//! or capability-gate.
//!
//! # Two requirements present from day one
//!
//! Both exist for the sake of the DRM presentation path, and both are here
//! before any DRM code, because retrofitting either is a breaking redesign:
//!
//! 1. **External render targets** — [`ExternalImageDesc`] lets the renderer
//!    draw into images it did not allocate, carrying dma-buf fd, DRM fourcc,
//!    and format modifier.
//! 2. **Exportable sync** — [`HalFence::export_sync_file`] converts GPU
//!    completion into a `sync_file` fd that can ride an atomic commit as
//!    `IN_FENCE_FD`.
//!
//! # Branch on capabilities, never on backend identity
//!
//! [`Capabilities`] is the only thing layers above the HAL are allowed to
//! condition on. A Mali GPU lacking fence export and a desktop Vulkan driver
//! lacking `VK_KHR_external_fence_fd` are the same problem, and code asking
//! "is this GLES?" instead of "can this export a fence?" gets both wrong.

pub mod batch;
pub mod blend;
pub mod capabilities;
pub mod error;
pub mod format;
pub mod material;
pub mod resource;
pub mod scissor;
pub mod sync;

pub use batch::{Batch, BatchDraw, ClipRole, ClipState, Vertex};
pub use blend::{BlendFactor, BlendFactors, BlendMode};
pub use capabilities::{
    Capabilities, Capability, DmaBufSupport, SampleCounts, SyncSupport, Withheld,
};
pub use error::{Error, Result};
pub use format::{Extent2D, FormatModifierSet, Fourcc, Modifier, PixelFormat};
/// A caller's fragment program, in the forms each backend can take one.
///
/// Both are given because a program is registered once and drawn on whichever
/// backend the context happens to be, and neither payload can be derived from
/// the other here: nothing in this workspace translates shaders at run time,
/// by the decision recorded in the architecture document. A caller's build
/// produces both, which is what the reference implementation's toolchain does
/// and what this project's own build does.
#[derive(Debug, Clone)]
pub struct RuntimeProgram {
    /// A SPIR-V fragment module, for Vulkan.
    pub spirv: Vec<u32>,
    /// A GLSL ES 300 fragment source, for GLES.
    pub glsl_es: String,
}

pub use material::{
    ColorFilter, ColorForm, Gamma, Material, MaterialVariant, Sampling, Stop, TileMode,
    MATERIAL_FLOATS, MAX_EFFECT_TEXTURES, MAX_STOPS, MORPHOLOGY_TAPS, RUNTIME_FLOATS,
};
pub use resource::{
    mip_levels_for, BufferDescriptor, BufferUsage, TextureDescriptor, TextureUsage,
};
pub use scissor::Scissor;
pub use sync::{HalFence, FRAME_WAIT_TIMEOUT};

#[cfg(unix)]
pub use resource::{DmaBufPlane, ExternalImageDesc};

/// A rendering backend, as a family of associated types.
///
/// Implementors are zero-sized markers; the types they name carry the work.
pub trait Hal: 'static {
    type Context: HalContext<Hal = Self>;
    type Texture: HalTexture;
    /// GPU completion, for callers that submit without waiting.
    ///
    /// Named here because the DRM presentation path consumes one: it needs
    /// something to hand the kernel and something to decide when a frame slot
    /// is free again.
    type Fence: HalFence;

    /// Backend name, for logs and report fingerprints.
    const NAME: &'static str;
}

/// What a target must be able to tell the layers above it.
pub trait HalTexture {
    fn extent(&self) -> Extent2D;
    fn format(&self) -> PixelFormat;
}

/// A device, and the resources created from it.
///
/// # Why a batch rather than a command buffer
///
/// An earlier shape of this trait had callers record incrementally — begin a
/// pass, bind a pipeline, draw, finish — mirroring how Vulkan itself works.
/// Building a real backend showed that to be the wrong seam. What a backend
/// actually wants is the whole [`Batch`] at once, because the useful decisions
/// are all global to it: which draws can share a pipeline binding, how to lay
/// out one shared vertex buffer, what to upload in a single copy. Handing over
/// a stream of calls forces each backend to reconstruct that shape, and a
/// record-and-replay backend would have to buffer the stream anyway just to see
/// what it was given.
///
/// This stays explicit in the sense that matters — nothing is discovered at
/// draw time and the caller states its whole intent up front — while leaving
/// each backend free to realize it natively.
///
/// # Threading
///
/// These take `&mut self`, so a context is not yet usable for resource
/// creation from several threads at once. The intended design is thread-safe
/// creation behind `&self`, which needs interior mutability around the
/// allocator. That is deferred rather than decided against: nothing creates
/// resources off the recording thread yet, and adding the synchronization
/// before there is a caller to shape it around would be guesswork.
pub trait HalContext {
    type Hal: Hal;

    /// What this device can do. The only thing callers branch on.
    fn capabilities(&self) -> &Capabilities;

    /// Allocate a texture, or import an external image when the descriptor
    /// carries one.
    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<<Self::Hal as Hal>::Texture>;

    /// Release a texture and its memory.
    fn destroy_texture(&mut self, texture: <Self::Hal as Hal>::Texture);

    /// Register a caller's fragment program and return the name for it.
    ///
    /// Registering the same payload twice gives the same name back rather than
    /// a second entry. That is not a convenience: a program leads to pipelines
    /// keyed by it, so registering one repeatedly would multiply the pipeline
    /// cache by the number of times a caller happened to ask -- and a caller
    /// with no place to cache an index, which is every caller that renders a
    /// list of scenes, would do exactly that.
    fn register_program(&mut self, program: &RuntimeProgram) -> Result<u32>;

    /// Draw a batch into a target.
    ///
    /// A descriptor that preserves rather than clears composes several batches
    /// onto one target. A multisampled descriptor renders to a transient
    /// multisample buffer and resolves into the target, so the target stays
    /// single-sampled and readable either way.
    fn submit_batch(
        &mut self,
        target: &mut <Self::Hal as Hal>::Texture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<()> {
        self.submit_batch_textured(target, batch, pass, &[])
    }

    /// Draw a batch whose materials sample textures.
    ///
    /// `textures` is the table a [`Material::Image`] slot indexes. It is passed
    /// alongside the batch rather than held inside it because a batch is a
    /// description a recorder produces without touching the device, and a
    /// backend texture handle is not something it can name.
    ///
    /// The target is borrowed mutably and the table immutably, so a batch
    /// cannot sample the target it draws into. That restriction is real rather
    /// than incidental — reading an attachment being written in the same pass
    /// needs machinery this does not have — and having the borrow checker state
    /// it is better than discovering it as a driver-dependent picture.
    ///
    /// A slot with no entry is an error rather than a fallback: a paint
    /// silently drawn as something else is the failure nobody debugs from.
    fn submit_batch_textured(
        &mut self,
        target: &mut <Self::Hal as Hal>::Texture,
        batch: &Batch,
        pass: PassDescriptor,
        textures: &[&<Self::Hal as Hal>::Texture],
    ) -> Result<()>;

    /// Copy a target back to host memory, tightly packed.
    ///
    /// Part of the trait rather than a backend extra because the offscreen
    /// target is a first-class citizen: the entire golden and conformance
    /// apparatus is built on rendering to one and reading it back.
    fn read_texture(&mut self, texture: &mut <Self::Hal as Hal>::Texture) -> Result<Vec<u8>>;

    /// Fill a texture from host memory, tightly packed and top row first.
    ///
    /// The exact inverse of [`Self::read_texture`], stated in the same layout,
    /// so a round trip through the pair is the identity on every backend. That
    /// is what lets it be checked without a decoder: write known bytes, read
    /// them back, compare.
    ///
    /// Color must be **premultiplied**, matching what a render target holds
    /// and what a paint sampling this expects. A decoder usually produces
    /// straight alpha, so converting is the caller's job. The distinction is
    /// invisible for an opaque image, which is what makes it worth stating
    /// here rather than leaving to be discovered.
    ///
    /// Decoding images is out of scope for this project; getting already
    /// decoded pixels onto the device is not. Without this, an image shader
    /// could sample nothing but what the renderer itself had drawn.
    fn write_texture(
        &mut self,
        texture: &mut <Self::Hal as Hal>::Texture,
        pixels: &[u8],
    ) -> Result<()>;

    /// Allocate a target that can be shared with a display controller.
    ///
    /// `modifiers` are the layouts the other side accepts, in preference order.
    /// The default reports the capability as absent, which is the honest answer
    /// for a backend that cannot do it: callers check
    /// [`DmaBufSupport::can_allocate_scanout`] and take the GBM-allocated path
    /// instead. This is capability gating rather than a stub — a backend that
    /// answered every method this way would be useless, but one that answers
    /// only the optional ones is correctly describing itself.
    fn create_exportable_texture(
        &mut self,
        _extent: Extent2D,
        _format: PixelFormat,
        _modifiers: &[Modifier],
    ) -> Result<<Self::Hal as Hal>::Texture> {
        Err(Error::Unsupported("allocating exportable images"))
    }

    /// Export a texture as a dma-buf for scanout or cross-device sharing.
    ///
    /// Returns [`Error::Unsupported`] where [`DmaBufSupport::export`] is false;
    /// the DRM path then allocates through GBM and imports instead.
    #[cfg(unix)]
    fn export_texture(
        &mut self,
        _texture: &<Self::Hal as Hal>::Texture,
    ) -> Result<ExternalImageDesc> {
        Err(Error::Unsupported("dma-buf export"))
    }

    /// Submit a batch without waiting, returning something that signals when
    /// the GPU has finished.
    ///
    /// A frame loop needs this rather than the waiting form: the returned
    /// fence is what gets handed to a display commit, and what decides when a
    /// frame slot may be reused.
    ///
    /// Ordering between two of these is the caller's, and nothing here checks
    /// it. Submitting twice into one target without waiting for the first is a
    /// write-after-write hazard: both calls are well formed, both succeed, and
    /// what lands is whichever the device finished last. A frame loop avoids it
    /// by construction, since a ring hands out a different slot each frame and
    /// will not reuse one until its fence has retired.
    fn submit_batch_deferred(
        &mut self,
        target: &mut <Self::Hal as Hal>::Texture,
        batch: &Batch,
        pass: PassDescriptor,
    ) -> Result<<Self::Hal as Hal>::Fence> {
        self.submit_batch_deferred_textured(target, batch, pass, &[])
    }

    /// The same, for a batch whose materials sample textures.
    ///
    /// What a frame with layers needs. The pass that lands in the image being
    /// presented is the one that composites the layers, so it samples the
    /// targets they were rendered into — and a deferred submission that could
    /// not sample anything meant such a frame could be rendered offscreen and
    /// never displayed.
    ///
    /// The textures must outlive the submission, which the caller arranges:
    /// they cannot travel with the fence, because a fence is handed to a
    /// display commit and has to stay sendable while a texture tracks mutable
    /// state of its own.
    fn submit_batch_deferred_textured(
        &mut self,
        _target: &mut <Self::Hal as Hal>::Texture,
        _batch: &Batch,
        _pass: PassDescriptor,
        _textures: &[&<Self::Hal as Hal>::Texture],
    ) -> Result<<Self::Hal as Hal>::Fence> {
        Err(Error::Unsupported("deferred submission"))
    }

    /// Release a deferred submission once its work has completed.
    fn retire_fence(&mut self, _fence: <Self::Hal as Hal>::Fence) {}
}

/// How a pass is configured, beyond the draws themselves.
///
/// Grouped rather than passed as loose parameters so later pass-level state —
/// stencil, depth, damage regions — extends this without changing every
/// backend's signature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassDescriptor {
    /// Clear to this color first, or preserve the target's contents.
    pub clear: Option<[f32; 4]>,
    /// MSAA sample count. 1 disables multisampling.
    ///
    /// Rendering is multisampled and resolved into the target, so the target
    /// itself stays single-sampled and directly readable.
    pub samples: u32,
    /// Where this pass's clip space lands in the target, if not the whole of it.
    ///
    /// `None` maps clip space onto the target exactly, which is what every
    /// pass wants whose geometry was recorded against the target it renders
    /// into. A pass whose geometry was recorded against a *larger* space sets
    /// this to crop instead of scale — see [`PassViewport`].
    pub viewport: Option<PassViewport>,
}

/// A clip space larger than the target it lands in, and where it lands.
///
/// A layer records its draws before anything is known about how much of the
/// target they will cover. The geometry and every material go into clip space
/// as they are recorded — `docs/architecture.md` states that a recording is
/// tessellated geometry rather than a command list — so by the time the extent
/// could be narrowed, narrowing it would mean rewriting all of that.
///
/// This narrows the *target* instead and leaves the recording alone. The
/// viewport keeps the extent the geometry was recorded against, and `offset`
/// slides it so that the wanted sub-rectangle lands on the target. Clip space
/// maps onto the viewport, not onto the target, so the picture is cropped
/// rather than scaled and every recorded coordinate still means what it meant.
///
/// `offset` is normally negative: a target holding the region starting at
/// device (x, y) of the recorded space offsets by (-x, -y).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassViewport {
    /// Where the recorded space's origin sits, in target pixels.
    pub offset: [f32; 2],
    /// The extent the geometry was recorded against.
    pub extent: Extent2D,
}

impl Default for PassDescriptor {
    fn default() -> Self {
        Self {
            clear: None,
            samples: 1,
            viewport: None,
        }
    }
}

impl PassDescriptor {
    pub fn clear(color: [f32; 4]) -> Self {
        Self {
            clear: Some(color),
            samples: 1,
            viewport: None,
        }
    }

    /// Land this pass's clip space on part of the target rather than all of it.
    pub fn with_viewport(mut self, viewport: PassViewport) -> Self {
        self.viewport = Some(viewport);
        self
    }

    pub fn preserve() -> Self {
        Self::default()
    }

    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = samples;
        self
    }

    pub fn is_multisampled(&self) -> bool {
        self.samples > 1
    }
}

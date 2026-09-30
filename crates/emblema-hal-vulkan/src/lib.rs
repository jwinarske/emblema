//! Vulkan backend via ash. First-class: the reference implementation, the
//! conformance oracle other backends are diffed against, and where new
//! features land first.
//!
//! Vulkan 1.1 is the floor; later features are used when present rather than
//! required. Features avoided for MoltenVK compatibility — geometry shaders,
//! tessellation shaders, sparse residency, multi-draw-indirect-with-count —
//! are none of them needed for 2D.
//!
//! Implements the `Hal` and `HalContext` traits, so the renderer and the
//! conformance harness can drive this backend without naming it. The concrete
//! API stays available alongside for bring-up and for tests that need Vulkan
//! specifics.

pub mod device;
pub mod external;
pub mod fence;
pub mod hal;
pub(crate) mod materials;
pub mod render;
pub mod resource;
pub mod sampling;
pub mod stencil;
pub mod validation;

pub use device::{ContextConfig, DevicePreference, FrameSync, VulkanContext};
pub use fence::VulkanFence;
pub use hal::VulkanHal;
pub use resource::VulkanTexture;
pub use validation::{Validated, ValidationLog, ValidationMessage, ValidationSeverity};

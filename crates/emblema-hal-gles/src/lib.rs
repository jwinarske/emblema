//! GLES 3.0 backend via glow, with contexts from EGL.
//!
//! Commands are recorded and replayed as GL calls rather than issued as they
//! arrive, which is why the HAL hands a whole batch over: a record-and-replay
//! backend wants to see the entire scene before touching any GL state, so it
//! can order binds and avoid redundant ones.
//!
//! GLES 3.0 is the floor. GLES 2.0 is permanently out of scope; the feature gap
//! is too large to bridge.
//!
//! # Status
//!
//! Offscreen rendering of solid-color geometry, with blending and
//! multisampling. Rendering to a window or to a GBM surface for direct scanout
//! is not implemented yet.

pub mod context;
pub mod debug;
pub mod fence;
pub mod hal;
pub mod render;

pub use context::{DisplayTarget, Egl, GlesConfig, GlesContext};
pub use debug::{DebugMessage, DebugSeverity, Validated};
pub use hal::GlesHal;
pub use render::GlesTexture;

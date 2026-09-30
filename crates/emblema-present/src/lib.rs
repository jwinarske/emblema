//! Presentation: how a finished image reaches a display, and what paces the
//! frame loop.
//!
//! This is the axis orthogonal to the rendering HAL. The HAL answers how draw
//! commands become pixels in a GPU image; a presentation target answers how that
//! image is displayed and when the next frame may begin. Keeping them
//! independent is what lets every target work with every backend that can
//! produce compatible images.
//!
//! Format negotiation lives here rather than in either axis because it runs
//! *between* them: the target says what it can scan out, the context says what
//! it can render and export, and the intersection decides what is allocated.

pub mod negotiate;
pub mod target;

pub use negotiate::{negotiate, negotiate_non_linear, Negotiated, PREFERRED_FORMATS};
pub use target::{OffscreenTarget, PresentTarget};

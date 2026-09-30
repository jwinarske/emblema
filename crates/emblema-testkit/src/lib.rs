//! Shared test harness.
//!
//! One corpus, many executions. Scenes are data rather than Rust code, so a
//! single corpus drives golden comparison, cross-backend conformance,
//! performance runs, and on-device runs without being rewritten for each. A new
//! feature adds scenes once and every execution mode picks them up, which is
//! what keeps authoring cost flat while the test matrix multiplies.
//!
//! The pieces:
//!
//! - [`shape`] and [`scene`] describe what to draw, as plain data.
//! - [`executor`] renders a scene through any backend implementing the HAL.
//! - [`image`] compares results, with tolerances that state where the
//!   specification permits a difference rather than papering over one.

pub mod catalog;
pub mod executor;
pub mod fixture;
pub mod image;
pub mod scene;
pub mod shape;

pub use catalog::catalog;
pub use executor::{record_scene, render_corpus, render_scene, render_scene_into, Fixtures};
pub use image::{accepts, compare, Difference, Image, Tolerance};
pub use scene::{corpus, Fill, Item, LayerSpec, Node, Scene, Stop, StrokeSpec, Transform};
// The stroke settings a scene states, so a test can vary one without reaching
// past this crate for the type that names it.
pub use emblema_geometry::stroke::{LineCap, LineJoin};
pub use shape::Shape;

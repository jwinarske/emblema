//! Path types and tessellation.
//!
//! Every path this crate is given is tessellated. A convex path takes a fan
//! fill, which avoids both the sweep and the stencil pass, and everything else
//! goes through the general fill — so [`path::Path::convexity`] is what
//! chooses between them.
//!
//! Detection is conservative in one direction only: a wrong `Concave` answer
//! costs speed, and a wrong `Convex` answer renders incorrectly, because a fan
//! applies no fill rule and cannot express what filling a self-crossing path
//! means. That asymmetry is why the classifier measures how far a polygon
//! turns in total rather than only whether its turns agree.
//!
//! Not every shape comes here. An antialiased rounded rect, circle or ellipse,
//! and a blurred rounded rect, are drawn as one quad whose coverage the
//! fragment shader computes; `emblema-core`'s canvas makes that choice, and
//! those shapes never become a [`Path`] here. A filled plain rect is still
//! tessellated, deliberately: evaluating it measured about thirty per cent
//! slower on every board this targets (`docs/on-a-board.md`); a stroked one is
//! evaluated.

pub mod dash;
pub mod flatten;
pub mod path;
pub mod shadow;
pub mod stroke;
pub mod superellipse;
pub mod tessellate;
pub mod transform;

pub use flatten::{flatten, DEFAULT_TOLERANCE};
pub use path::{Convexity, FillRule, Path, PathBuilder, Rect, Verb, MAX_COORDINATE};
pub use shadow::{ambient_shadow_mesh, shadow_opacity, shadow_reach, ShadowMesh};
pub use stroke::{LineCap, LineJoin, StrokeStyle};
pub use tessellate::{Tessellator, VertexBuffers};
pub use transform::{
    invert_to_local, max_scale, to_local_columns, transform_points, unbounded, viewport_projection,
    Transform2D,
};

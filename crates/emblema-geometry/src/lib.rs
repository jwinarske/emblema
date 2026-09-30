//! Path types and tessellation.
//!
//! Everything is tessellated. A convex path takes a fan fill, which avoids
//! both the sweep and the stencil pass, and everything else goes through the
//! general fill — so [`path::Path::convexity`] is what chooses between them.
//!
//! Detection is conservative in one direction only: a wrong `Concave` answer
//! costs speed, and a wrong `Convex` answer renders incorrectly, because a fan
//! applies no fill rule and cannot express what filling a self-crossing path
//! means. That asymmetry is why the classifier measures how far a polygon
//! turns in total rather than only whether its turns agree.
//!
//! **Not implemented**, and named here because this header described it as
//! though it were: analytic coverage for rects, rounded rects, circles and
//! ellipses, computed in the fragment shader instead of tessellating. There is
//! no rounded rect or ellipse in this crate at all, and a circle is four
//! cubics that get flattened like any other curve. It is the intended shape --
//! it is where most of the draw calls in a real interface land -- rather than
//! something that exists.

pub mod dash;
pub mod flatten;
pub mod path;
pub mod stroke;
pub mod superellipse;
pub mod tessellate;
pub mod transform;

pub use flatten::{flatten, DEFAULT_TOLERANCE};
pub use path::{Convexity, FillRule, Path, PathBuilder, Rect, Verb, MAX_COORDINATE};
pub use stroke::{LineCap, LineJoin, StrokeStyle};
pub use tessellate::{Tessellator, VertexBuffers};
pub use transform::{
    invert_to_local, max_scale, to_local_columns, transform_points, unbounded, viewport_projection,
    Transform2D,
};

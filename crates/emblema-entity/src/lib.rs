//! Entity and Contents layer: backend-agnostic description of what to draw.
//! An Entity carries a transform, blend, clip depth, contents, and geometry.
//!
//! What is built here is coverage, and only coverage. The canvas above already
//! records into a batch directly and does not route through this layer, so
//! anything else would be shaped around a guess about a caller that does not
//! exist yet. Coverage is the exception because it is not a guess: the canvas
//! computes it today in four scattered places -- bounds expanded by a stroke,
//! a layer's outward reach, what a mask blur covers, and the device bounds a
//! save layer is sized to -- and every one of them is this same composition
//! done by hand. Writing it once, with the rules stated and tested, is
//! worthwhile whether or not the rest of the layer is ever built.
//!
//! The composition, in order: geometry bounds in the entity's own coordinates,
//! carried through the transform into device coordinates, grown by however far
//! the contents reach outward, and finally narrowed by the clip. The order
//! matters at both ends. A filter reaches in device pixels rather than in the
//! entity's units, so growing before the transform would scale the reach along
//! with the shape. And the clip narrows last because it is the only step that
//! can make coverage empty for a reason other than the shape being empty.

use emblema_geometry::Rect;
use emblema_hal::BlendMode;
use glam::{Affine2, Vec2};

/// Where an entity lands, or that it lands nowhere, or that it lands wherever
/// it is allowed to.
///
/// Three cases rather than a rectangle, because two of them are not rectangles.
/// Nothing drawn has no bounds to report and must not be confused with a
/// zero-area rectangle at the origin, which is a real place. And `drawPaint`
/// covers whatever the clip admits, which is unbounded until a clip says
/// otherwise -- representing that as a very large rectangle would be a number
/// chosen by whoever wrote it, and would be wrong at some scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Coverage {
    /// Nothing is drawn.
    Empty,
    /// Everything inside this rectangle, in device coordinates, may be.
    Bounded(Rect),
    /// Whatever the clip admits. Only a clip can bound this.
    Unbounded,
}

impl Coverage {
    /// The rectangle this covers, given what the clip admits.
    ///
    /// `None` for nothing drawn. An unbounded coverage resolves to the clip,
    /// which is what makes an unclipped `drawPaint` the one case that cannot
    /// answer -- the caller has to supply the target's bounds as the clip.
    pub fn resolve(self, clip: Option<Rect>) -> Option<Rect> {
        match self {
            Self::Empty => None,
            Self::Bounded(rect) => match clip {
                Some(clip) => intersect(rect, clip),
                None => Some(rect),
            },
            Self::Unbounded => clip,
        }
    }

    /// Narrow by what a clip admits.
    ///
    /// An unbounded coverage becomes the clip, which is the whole reason the
    /// unbounded case can be carried around at all rather than being resolved
    /// where it arises.
    pub fn clipped(self, clip: Rect) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Bounded(rect) => match intersect(rect, clip) {
                Some(rect) => Self::Bounded(rect),
                None => Self::Empty,
            },
            Self::Unbounded => Self::Bounded(clip),
        }
    }

    /// Grow outward by this much in each direction, in device pixels.
    ///
    /// Growing nothing leaves nothing: a filter that reaches outward from a
    /// shape that was never drawn still draws nothing, and a blur of an empty
    /// region is empty rather than a soft patch of nowhere.
    pub fn grown(self, reach: Vec2) -> Self {
        match self {
            Self::Bounded(rect) if reach.is_finite() => {
                Self::Bounded(Rect::new(rect.min - reach.abs(), rect.max + reach.abs()))
            }
            other => other,
        }
    }

    /// The smallest coverage containing both.
    pub fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unbounded, _) | (_, Self::Unbounded) => Self::Unbounded,
            (Self::Empty, x) | (x, Self::Empty) => x,
            (Self::Bounded(a), Self::Bounded(b)) => {
                Self::Bounded(Rect::new(a.min.min(b.min), a.max.max(b.max)))
            }
        }
    }
}

/// The region an entity's geometry occupies, in the entity's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Geometry {
    /// Nothing.
    Empty,
    /// A shape with these bounds. Already grown by a stroke if there is one:
    /// how far a stroke reaches past its path is the stroker's rule, not this
    /// layer's, and re-deriving it here would be a second copy to disagree.
    Bounded(Rect),
    /// Fills whatever the clip admits, which is what `drawPaint` means.
    Unbounded,
}

impl Geometry {
    /// Coverage in device coordinates.
    ///
    /// The bounding box of the four transformed corners, which is a superset of
    /// the transformed shape whenever the transform rotates or shears -- a
    /// square turned an eighth of a turn needs a box half again its area. A
    /// superset is the safe direction: coverage decides what may be skipped and
    /// how large an offscreen target is, and both are wrong in a way that shows
    /// only if the answer is too small.
    pub fn transformed(self, transform: Affine2) -> Coverage {
        match self {
            Self::Empty => Coverage::Empty,
            Self::Unbounded => Coverage::Unbounded,
            Self::Bounded(rect) if rect.is_empty() => Coverage::Empty,
            Self::Bounded(rect) => {
                let corners = [
                    Vec2::new(rect.min.x, rect.min.y),
                    Vec2::new(rect.max.x, rect.min.y),
                    Vec2::new(rect.max.x, rect.max.y),
                    Vec2::new(rect.min.x, rect.max.y),
                ]
                .map(|corner| transform.transform_point2(corner));
                // A transform carrying a corner to infinity or to no number at
                // all describes no region. Reporting the bounding box of those
                // corners would be a rectangle of infinities, which every later
                // intersection would silently accept.
                if !corners.iter().all(|corner| corner.is_finite()) {
                    return Coverage::Empty;
                }
                let mut bounds = Rect::empty();
                for corner in corners {
                    bounds.union_point(corner);
                }
                Coverage::Bounded(bounds)
            }
        }
    }
}

/// What an entity draws with, so far as coverage is concerned.
///
/// A material decides color and is invisible to this calculation; what matters
/// here is only how far the result lands from the geometry that produced it. A
/// blur carries color outward, a dilation does, an erosion does not, and a
/// solid color does not -- so this is a distance rather than a paint.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Contents {
    /// How far color lands outside the geometry, in device pixels, per axis.
    pub reach: Vec2,
}

impl Contents {
    /// Contents that stay inside their geometry, which is most of them.
    pub fn tight() -> Self {
        Self { reach: Vec2::ZERO }
    }

    /// Contents reaching this far outward, in device pixels.
    pub fn reaching(reach: Vec2) -> Self {
        Self { reach }
    }
}

/// One thing to draw, described without reference to a device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entity {
    /// Carries the geometry into device coordinates.
    pub transform: Affine2,
    pub blend: BlendMode,
    /// Which clip generation this is inside, matching the stencil the renderer
    /// keeps. Not used by coverage: a clip narrows what is drawn, and the
    /// rectangle that clip admits is passed to [`Self::coverage`] separately,
    /// because a depth is a name for a clip and not its shape.
    pub clip_depth: u32,
    pub contents: Contents,
    pub geometry: Geometry,
}

impl Default for Entity {
    fn default() -> Self {
        Self {
            transform: Affine2::IDENTITY,
            blend: BlendMode::SrcOver,
            clip_depth: 0,
            contents: Contents::tight(),
            geometry: Geometry::Empty,
        }
    }
}

impl Entity {
    /// Where this lands, given what the clip admits.
    ///
    /// `None` for a clip means unclipped, which leaves an unbounded entity
    /// unbounded -- see [`Coverage::resolve`] for why that is not answered
    /// with a large rectangle.
    pub fn coverage(&self, clip: Option<Rect>) -> Coverage {
        let covered = self
            .geometry
            .transformed(self.transform)
            .grown(self.contents.reach);
        match clip {
            Some(clip) => covered.clipped(clip),
            None => covered,
        }
    }
}

/// The overlap of two rectangles, or `None` where they do not meet.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let min = a.min.max(b.min);
    let max = a.max.min(b.max);
    if min.x > max.x || min.y > max.y {
        None
    } else {
        Some(Rect::new(min, max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::FRAC_PI_4;

    fn rect(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Rect {
        Rect::new(Vec2::new(min_x, min_y), Vec2::new(max_x, max_y))
    }

    fn bounded(geometry: Rect) -> Entity {
        Entity {
            geometry: Geometry::Bounded(geometry),
            ..Entity::default()
        }
    }

    #[test]
    fn a_transform_that_turns_a_shape_needs_a_larger_box_to_hold_it() {
        // The bounding box of the transformed corners rather than the
        // transformed bounding box, which are the same thing only while the
        // transform keeps the axes. A two-by-two square turned an eighth of a
        // turn stands on a corner and spans two root two, which is the number
        // that says the corners were transformed rather than the extents.
        let square = bounded(rect(-1.0, -1.0, 1.0, 1.0));
        let upright = square.coverage(None);
        assert_eq!(upright, Coverage::Bounded(rect(-1.0, -1.0, 1.0, 1.0)));

        let turned = Entity {
            transform: Affine2::from_angle(FRAC_PI_4),
            ..square
        }
        .coverage(None);
        let Coverage::Bounded(bounds) = turned else {
            panic!("a turned square still covers something, got {turned:?}");
        };
        let half = core::f32::consts::SQRT_2;
        assert!(
            (bounds.max.x - half).abs() < 1e-5 && (bounds.max.y - half).abs() < 1e-5,
            "an eighth turn should reach root two, got {bounds:?}"
        );
        // And it is a superset rather than the shape: the corners of this box
        // are outside the turned square, which is the direction coverage is
        // allowed to be wrong in.
        assert!(bounds.width() > 2.0, "the box grew, got {}", bounds.width());
    }

    #[test]
    fn a_filter_reaches_in_device_pixels_rather_than_in_the_shapes_own_units() {
        // The order the composition is written in, and the reason it is not
        // the other one. A filter's radius is a distance on the target, so the
        // growth has to happen after the transform; growing first would send
        // the reach through the transform along with the shape, and a shape
        // drawn at ten times the scale would blur ten times as far.
        let scaled = Entity {
            transform: Affine2::from_scale(Vec2::splat(10.0)),
            contents: Contents::reaching(Vec2::splat(4.0)),
            ..bounded(rect(0.0, 0.0, 1.0, 1.0))
        };
        assert_eq!(
            scaled.coverage(None),
            Coverage::Bounded(rect(-4.0, -4.0, 14.0, 14.0)),
            "the unit square scaled to ten, grown by four device pixels"
        );
    }

    #[test]
    fn the_clip_narrows_after_the_filter_has_reached() {
        // Also an order, and also not the other one. What a clip admits is
        // decided about the finished picture, so a blur reaches out from the
        // shape and the clip then cuts the result. Clipping the geometry first
        // and growing afterwards would let the blur back out past the clip --
        // the same coverage a caller uses to size an offscreen target, which
        // would then be too large by the reach on every side.
        let entity = Entity {
            contents: Contents::reaching(Vec2::splat(5.0)),
            ..bounded(rect(0.0, 0.0, 10.0, 10.0))
        };
        let clip = rect(0.0, 0.0, 8.0, 8.0);
        assert_eq!(
            entity.coverage(Some(clip)),
            Coverage::Bounded(rect(0.0, 0.0, 8.0, 8.0)),
            "grown to -5..15 and then cut to the clip"
        );
        // Growing after clipping would have reached to thirteen.
        let wrong = Coverage::Bounded(clip).grown(Vec2::splat(5.0));
        assert_ne!(
            entity.coverage(Some(clip)),
            wrong,
            "the two orders must not agree, or this test proves nothing"
        );
    }

    #[test]
    fn nothing_drawn_stays_nothing_however_it_is_filtered() {
        // A blur of an empty region is empty rather than a soft patch of
        // nowhere, and an empty rectangle is not a zero-area one at the origin
        // -- which is a real place and has to survive.
        let nothing = Entity::default();
        assert_eq!(nothing.coverage(None), Coverage::Empty);
        assert_eq!(
            nothing.coverage(Some(rect(0.0, 0.0, 10.0, 10.0))),
            Coverage::Empty
        );
        assert_eq!(
            Coverage::Empty.grown(Vec2::splat(9.0)),
            Coverage::Empty,
            "growing nothing leaves nothing"
        );

        // Empty geometry, spelled as an inverted rectangle, is the same answer.
        assert_eq!(
            bounded(rect(10.0, 10.0, 0.0, 0.0)).coverage(None),
            Coverage::Empty
        );

        // A transform that collapses the plane leaves a point, which is a place
        // rather than nothing: something is drawn there, and a caller culling
        // against an empty answer would skip a draw that marks the target.
        let collapsed = Entity {
            transform: Affine2::from_scale(Vec2::ZERO),
            ..bounded(rect(0.0, 0.0, 10.0, 10.0))
        };
        assert_eq!(
            collapsed.coverage(None),
            Coverage::Bounded(rect(0.0, 0.0, 0.0, 0.0)),
            "a collapsed shape is a point, not an absence"
        );
    }

    #[test]
    fn a_transform_to_nowhere_covers_nothing() {
        // A corner carried to infinity or to no number at all describes no
        // region. The bounding box of those corners is a rectangle of
        // infinities, which every later intersection accepts without
        // complaint -- so it is refused where it arises rather than passed on.
        for bad in [f32::INFINITY, f32::NAN] {
            let entity = Entity {
                transform: Affine2::from_scale(Vec2::splat(bad)),
                ..bounded(rect(1.0, 1.0, 2.0, 2.0))
            };
            assert_eq!(
                entity.coverage(None),
                Coverage::Empty,
                "a transform by {bad} covers nothing"
            );
        }
        // A reach that is not a length is ignored rather than propagated, on
        // the same grounds.
        assert_eq!(
            Coverage::Bounded(rect(0.0, 0.0, 1.0, 1.0)).grown(Vec2::splat(f32::NAN)),
            Coverage::Bounded(rect(0.0, 0.0, 1.0, 1.0))
        );
    }

    #[test]
    fn unbounded_coverage_is_only_ever_bounded_by_a_clip() {
        // `drawPaint` covers whatever the clip admits. Representing that as a
        // very large rectangle would be a number somebody chose, and wrong at
        // some scale, so it stays a case of its own until a clip resolves it.
        let paint = Entity {
            geometry: Geometry::Unbounded,
            ..Entity::default()
        };
        assert_eq!(paint.coverage(None), Coverage::Unbounded);
        assert_eq!(
            paint.coverage(Some(rect(2.0, 2.0, 6.0, 6.0))),
            Coverage::Bounded(rect(2.0, 2.0, 6.0, 6.0))
        );
        // A transform does not bound it either: everywhere transformed is
        // still everywhere.
        assert_eq!(
            Entity {
                transform: Affine2::from_scale(Vec2::splat(0.5)),
                ..paint
            }
            .coverage(None),
            Coverage::Unbounded
        );
        // Unclipped, it is the one coverage that cannot answer with a
        // rectangle, and says so rather than inventing one.
        assert_eq!(Coverage::Unbounded.resolve(None), None);
        assert_eq!(
            Coverage::Unbounded.resolve(Some(rect(0.0, 0.0, 3.0, 3.0))),
            Some(rect(0.0, 0.0, 3.0, 3.0))
        );
    }

    #[test]
    fn a_clip_that_misses_leaves_nothing() {
        let entity = bounded(rect(0.0, 0.0, 4.0, 4.0));
        assert_eq!(
            entity.coverage(Some(rect(9.0, 9.0, 12.0, 12.0))),
            Coverage::Empty
        );
        // Touching at a corner is a meeting rather than a miss: the shared
        // point is covered by both, and a zero-area overlap is still a place.
        assert_eq!(
            entity.coverage(Some(rect(4.0, 4.0, 8.0, 8.0))),
            Coverage::Bounded(rect(4.0, 4.0, 4.0, 4.0))
        );
    }

    #[test]
    fn union_takes_the_widest_claim() {
        let a = Coverage::Bounded(rect(0.0, 0.0, 2.0, 2.0));
        let b = Coverage::Bounded(rect(5.0, 1.0, 6.0, 9.0));
        assert_eq!(a.union(b), Coverage::Bounded(rect(0.0, 0.0, 6.0, 9.0)));
        // Empty is the identity, so a group of one draw covers what that draw
        // covers rather than that draw and the origin.
        assert_eq!(a.union(Coverage::Empty), a);
        assert_eq!(Coverage::Empty.union(a), a);
        // And unbounded absorbs, in either order.
        assert_eq!(a.union(Coverage::Unbounded), Coverage::Unbounded);
        assert_eq!(Coverage::Unbounded.union(a), Coverage::Unbounded);
    }

    #[test]
    fn a_reach_grows_outward_whichever_sign_it_is_given() {
        // A radius is a distance. A caller who computed one by subtraction and
        // got it backwards should get the same region, not an inside-out one
        // that then reads as empty.
        let covered = Coverage::Bounded(rect(0.0, 0.0, 4.0, 4.0));
        assert_eq!(
            covered.grown(Vec2::new(-2.0, -2.0)),
            covered.grown(Vec2::new(2.0, 2.0))
        );
        assert_eq!(
            covered.grown(Vec2::new(2.0, 0.0)),
            Coverage::Bounded(rect(-2.0, 0.0, 6.0, 4.0)),
            "per axis, so a one-dimensional blur grows one dimension"
        );
    }
}

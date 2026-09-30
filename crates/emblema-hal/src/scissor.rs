//! The rectangle a draw is confined to, in device pixels.
//!
//! # Why a rectangle and not a clip
//!
//! This is deliberately not called a clip. A clip in a drawing API is whatever
//! region a caller asked for — a rounded rectangle, an arbitrary path, several
//! of those intersected — and only some of those are rectangles. This type is
//! the part every device implements directly and exactly, at no cost, through
//! the same fixed-function unit in both APIs. Naming it for what it is keeps
//! the eventual stencil-backed clip from having to pretend to be one of these.
//!
//! It is also not a stepping stone that a stencil implementation would replace.
//! An axis-aligned clip should keep using this even once arbitrary clips work,
//! because a scissor is exact and free where a stencil pass is neither.
//!
//! # Orientation
//!
//! `y` is measured **downward from the top** of the target, matching the row
//! order that reading a texture back produces.
//!
//! Neither backend converts, which is worth stating because it is the opposite
//! of what the two APIs' documented conventions suggest. OpenGL numbers window
//! rows upward from the bottom, so a conversion looks obligatory. It is not:
//! the shader translator already negates Y when targeting GLSL, so geometry
//! lands in the GL framebuffer with its top row at GL's y of zero. That is the
//! same cancellation that makes reading a target back need no row flip, and a
//! scissor converted "correctly" mirrors the clip vertically for exactly the
//! same reason an added row flip mirrors the image.
//!
//! The convention has to be fixed here rather than left to each backend,
//! because a scissor is invisible in any target symmetric about its horizontal
//! center line: a backend that got this wrong would pass every test whose clip
//! did not sit deliberately off-center.

use crate::format::Extent2D;

/// A rectangle of the target that a draw may write to.
///
/// Stored as an origin and an extent rather than two corners so that an empty
/// rectangle has one representation rather than many, and so the type cannot
/// hold a rectangle whose corners are the wrong way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scissor {
    /// Leftmost column included.
    pub x: u32,
    /// Topmost row included, measured downward from the top of the target.
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Scissor {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The whole of a target.
    pub const fn covering(extent: Extent2D) -> Self {
        Self::new(0, 0, extent.width, extent.height)
    }

    /// A rectangle that admits nothing.
    pub const EMPTY: Self = Self::new(0, 0, 0, 0);

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// One past the last column included.
    pub const fn right(self) -> u32 {
        self.x + self.width
    }

    /// One past the last row included.
    pub const fn bottom(self) -> u32 {
        self.y + self.height
    }

    /// The overlap of two rectangles.
    ///
    /// Nested clips intersect rather than replace, so this is the operation a
    /// clip stack is built from. Disjoint rectangles give [`Self::EMPTY`]
    /// rather than something with a negative extent, which is what lets the
    /// result be pushed straight back onto the stack.
    pub fn intersect(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            return Self::EMPTY;
        }
        Self::new(x, y, right - x, bottom - y)
    }

    /// Whether this admits every pixel of a target.
    ///
    /// A draw whose scissor covers the whole target is unclipped, and saying so
    /// lets a backend skip the state change rather than setting a rectangle
    /// that changes nothing.
    pub fn covers(self, extent: Extent2D) -> bool {
        self.x == 0 && self.y == 0 && self.width >= extent.width && self.height >= extent.height
    }

    /// Clamp to a target, so nothing addresses outside it.
    pub fn clamped_to(self, extent: Extent2D) -> Self {
        self.intersect(Self::covering(extent))
    }

    /// The whole pixels of a device-space rectangle.
    ///
    /// A pixel belongs to the rectangle when its center does, which is the same
    /// rule the rasterizer applies to the shape being drawn. Rounding outward
    /// instead would admit pixels the caller excluded, and rounding inward
    /// would drop ones they kept; neither is exact for a boundary falling
    /// inside a pixel, but matching the rasterizer means a shape and a clip
    /// along the same edge agree about which pixels lie on it.
    ///
    /// Coordinates are plain arrays rather than a vector type so this stays put
    /// with the type it produces instead of pulling a geometry dependency into
    /// the HAL. Non-finite input collapses to nothing rather than saturating to
    /// the whole target, since a clip nobody can describe should not silently
    /// become the widest possible one.
    /// The same region, in a target whose origin moved by `(dx, dy)`.
    ///
    /// For a pass whose target was narrowed after its draws were recorded: the
    /// scissors are in the pixels of the space they were recorded against, and
    /// the target now starts `(dx, dy)` into that space. Shifting and
    /// re-clamping is exact -- a scissor is whole pixels either side of the
    /// move -- and a region that falls entirely outside the new target becomes
    /// empty rather than wrapping, which is what `saturating_sub` on the far
    /// edges gives.
    pub fn shifted(self, dx: u32, dy: u32, extent: Extent2D) -> Self {
        let left = self.x.saturating_sub(dx).min(extent.width);
        let top = self.y.saturating_sub(dy).min(extent.height);
        let right = (self.x + self.width).saturating_sub(dx).min(extent.width);
        let bottom = (self.y + self.height).saturating_sub(dy).min(extent.height);
        if right <= left || bottom <= top {
            return Self::EMPTY;
        }
        Self::new(left, top, right - left, bottom - top)
    }

    pub fn from_device_bounds(min: [f32; 2], max: [f32; 2], extent: Extent2D) -> Self {
        if !min.iter().chain(&max).all(|v| v.is_finite()) {
            return Self::EMPTY;
        }
        let whole = |v: f32, limit: u32| v.round().clamp(0.0, limit as f32) as u32;
        let left = whole(min[0], extent.width);
        let top = whole(min[1], extent.height);
        let right = whole(max[0], extent.width);
        let bottom = whole(max[1], extent.height);
        if right <= left || bottom <= top {
            return Self::EMPTY;
        }
        Self::new(left, top, right - left, bottom - top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: Extent2D = Extent2D::new(100, 80);

    #[test]
    fn covering_a_target_is_recognized_as_unclipped() {
        assert!(Scissor::covering(TARGET).covers(TARGET));
        assert!(!Scissor::new(1, 0, 99, 80).covers(TARGET));
        assert!(!Scissor::new(0, 0, 100, 79).covers(TARGET));
    }

    #[test]
    fn intersection_narrows_to_the_overlap() {
        let a = Scissor::new(10, 10, 50, 50);
        let b = Scissor::new(30, 20, 50, 50);
        assert_eq!(a.intersect(b), Scissor::new(30, 20, 30, 40));
        // And it does not matter which side is named first.
        assert_eq!(a.intersect(b), b.intersect(a));
    }

    #[test]
    fn disjoint_rectangles_intersect_to_nothing() {
        let a = Scissor::new(0, 0, 10, 10);
        let b = Scissor::new(20, 20, 10, 10);
        assert!(a.intersect(b).is_empty());
        // Touching along an edge shares no pixel either, since the right and
        // bottom edges are exclusive.
        assert!(a.intersect(Scissor::new(10, 0, 10, 10)).is_empty());
    }

    #[test]
    fn intersection_is_associative_so_a_clip_stack_may_fold_in_any_order() {
        let a = Scissor::new(5, 5, 60, 60);
        let b = Scissor::new(10, 0, 40, 70);
        let c = Scissor::new(0, 20, 100, 20);
        assert_eq!(a.intersect(b).intersect(c), a.intersect(b.intersect(c)));
    }

    #[test]
    fn device_bounds_take_the_pixels_whose_centers_are_inside() {
        // A rectangle from 2.4 to 5.6 covers the centers of pixels 2, 3, 4 and
        // 5 -- the ones at 2.5 through 5.5.
        let rect = Scissor::from_device_bounds([2.4, 0.0], [5.6, 4.0], TARGET);
        assert_eq!(rect, Scissor::new(2, 0, 4, 4));
        // Exactly on a boundary, the pixel to the right of it is the first one
        // included, since its center is the first past the edge.
        assert_eq!(
            Scissor::from_device_bounds([3.0, 0.0], [7.0, 1.0], TARGET),
            Scissor::new(3, 0, 4, 1)
        );
    }

    #[test]
    fn device_bounds_narrower_than_a_pixel_admit_nothing() {
        // Nothing rounds to a nonzero width, and admitting a whole pixel for a
        // sliver would be the outward rounding this deliberately avoids.
        assert!(Scissor::from_device_bounds([2.1, 0.0], [2.3, 4.0], TARGET).is_empty());
    }

    #[test]
    fn device_bounds_outside_the_target_do_not_wrap_or_saturate() {
        assert!(Scissor::from_device_bounds([-50.0, -50.0], [-10.0, -10.0], TARGET).is_empty());
        // A rectangle straddling the edge keeps only the part inside.
        assert_eq!(
            Scissor::from_device_bounds([-20.0, -20.0], [10.0, 10.0], TARGET),
            Scissor::new(0, 0, 10, 10)
        );
        // And a transform that produced non-finite coordinates gives nothing
        // rather than the whole target.
        assert!(Scissor::from_device_bounds([f32::NAN, 0.0], [10.0, 10.0], TARGET).is_empty());
        assert!(Scissor::from_device_bounds([0.0, 0.0], [f32::INFINITY, 10.0], TARGET).is_empty());
    }

    #[test]
    fn clamping_keeps_a_rectangle_inside_its_target() {
        let outside = Scissor::new(90, 70, 100, 100);
        let clamped = outside.clamped_to(TARGET);
        assert_eq!(clamped, Scissor::new(90, 70, 10, 10));
        assert!(clamped.right() <= TARGET.width && clamped.bottom() <= TARGET.height);
    }
}

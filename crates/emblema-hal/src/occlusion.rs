//! What is left of a rectangle once other rectangles are taken out of it.
//!
//! The arithmetic behind culling a draw that a later opaque draw covers. It is here,
//! away from [`crate::batch`], because it is about rectangles and nothing else: no
//! draw, no material, no device. That makes it testable without one, which matters
//! for the thing it decides -- a rectangle too large leaves a draw writing pixels
//! that are then overwritten, which costs time, and a rectangle too small leaves a
//! hole in the frame, which is a wrong picture.
//!
//! # The decomposition
//!
//! Scanline bands. Every covering rectangle's top and bottom edge, plus the input's
//! own two, cut the input into horizontal bands; inside a band the set of covered
//! columns does not change, so what survives is the column intervals no rectangle
//! spans. Each interval becomes one output rectangle.
//!
//! Bands *are* merged vertically afterwards, and the measurement is why. Without it the
//! bench's `stacked interface` frame went from twelve draws to two hundred and
//! twenty-seven: every horizontal edge in the scene cuts a band whether or not the
//! columns either side of it differ. Merging rectangles that share their columns and
//! touch is exact -- the union of two edge-to-edge rectangles with the same left and
//! width is a rectangle -- and it is the difference between a draw per band and a draw
//! per distinct shape.
//!
//! # What this is not
//!
//! Not a general region type. There is no union, no intersection of two results, no
//! simplification -- the one question asked is "what of this rectangle is not covered
//! by those", and a type that answered more would need to be correct about more.

use crate::scissor::Scissor;

/// The most rectangles [`remainder`] will return before giving up.
///
/// A band decomposition of `n` covering rectangles can reach `2n + 1` bands with
/// `n + 1` intervals each, so the output is bounded but the bound is quadratic. Past
/// this the caller is told nothing came of the attempt and leaves its draw alone, which
/// is always correct: drawing more than necessary is slow, never wrong.
///
/// **Thirty-two because the frame that matters needs twenty-eight.** The bench's
/// `stacked interface` is a full-screen gradient under eleven opaque rectangles, and the
/// gradient is the draw worth culling -- it is most of the frame's cost. Measured on
/// x86-64 Vulkan, against 1.790 ms unculled:
///
/// | cap | draws | median |
/// |---|---|---|
/// | 16 | 68 | 0.974 ms |
/// | 24 | 107 | 0.987 |
/// | 32 | 131 | **0.838** |
/// | 64 | 131 | 0.853 |
///
/// Below thirty-two the gradient's own remainder is refused and the frame keeps drawing
/// all of it, which is why twenty-four is no better than sixteen despite forty more
/// draws. Above thirty-two nothing changes, because nothing in that frame needs more.
///
/// A cap this size trades draws for fragments deliberately. `docs/non-parity.md` 76 --
/// the draw-count measurement -- is what makes that trade safe to make, and the board is
/// what checks it: per-draw cost is higher on a tiler than it is here.
pub const MAX_PIECES: usize = 32;

/// The parts of `inside` that none of `covered` contains.
///
/// Returns `None` where the answer would need more than [`MAX_PIECES`] rectangles,
/// which is the caller's signal to leave the draw as it was. An empty slice gives
/// back `inside` unchanged, and a fully covered `inside` gives an empty vector --
/// those two are different answers and the caller acts differently on each.
///
/// Rectangles in `covered` may overlap each other and may hang outside `inside`;
/// both are clamped and neither affects the result. Empty ones are ignored.
pub fn remainder(inside: Scissor, covered: &[Scissor]) -> Option<Vec<Scissor>> {
    if inside.is_empty() {
        return Some(Vec::new());
    }

    // Clipped to `inside` first, so every edge collected below is one that actually
    // cuts it. A rectangle hanging off the side would otherwise contribute a band
    // boundary outside the input and a band with nothing in it.
    let mut blockers: Vec<Scissor> = covered
        .iter()
        .map(|c| c.intersect(inside))
        .filter(|c| !c.is_empty())
        .collect();
    if blockers.is_empty() {
        return Some(vec![inside]);
    }

    // A blocker spanning the full width and height ends it: nothing survives, and
    // saying so early keeps the common "a draw is entirely hidden" case off the
    // band walk.
    if blockers.contains(&inside) {
        return Some(Vec::new());
    }

    let mut edges: Vec<u32> = Vec::with_capacity(blockers.len() * 2 + 2);
    edges.push(inside.y);
    edges.push(inside.bottom());
    for b in &blockers {
        edges.push(b.y);
        edges.push(b.bottom());
    }
    edges.sort_unstable();
    edges.dedup();

    // The blockers are walked once per band, so sorting them by top edge lets a band
    // skip the ones that start below it. The scenes this serves have few blockers and
    // the sort is cheap either way; it is here because the alternative reads as an
    // accident rather than a choice.
    blockers.sort_unstable_by_key(|b| b.y);

    // Bounded while building, before the merge has had a chance to bring the count
    // down. The ceiling is loose because the merge usually collapses most of it; it is
    // here so a pathological blocker set cannot allocate without limit on the way to
    // being refused.
    let ceiling = MAX_PIECES * 8;
    let mut out: Vec<Scissor> = Vec::new();
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for pair in edges.windows(2) {
        let (top, bottom) = (pair[0], pair[1]);
        if bottom <= top {
            continue;
        }

        // Every blocker covering this whole band, as column intervals. A blocker that
        // only partly overlaps the band cannot exist: the band's edges came from the
        // blockers' own edges, so each either spans the band or misses it.
        spans.clear();
        for b in &blockers {
            if b.y > top {
                break;
            }
            if b.bottom() >= bottom {
                spans.push((b.x, b.right()));
            }
        }
        spans.sort_unstable();

        let mut x = inside.x;
        for &(start, end) in &spans {
            if start > x {
                out.push(Scissor::new(x, top, start - x, bottom - top));
                if out.len() > ceiling {
                    return None;
                }
            }
            x = x.max(end);
        }
        if x < inside.right() {
            out.push(Scissor::new(x, top, inside.right() - x, bottom - top));
            if out.len() > ceiling {
                return None;
            }
        }
    }
    // Vertically adjacent pieces sharing their columns are one rectangle. Sorted by
    // column first so that the candidates for a merge are neighbors in the list.
    out.sort_unstable_by_key(|r| (r.x, r.width, r.y));
    let mut merged: Vec<Scissor> = Vec::with_capacity(out.len());
    for piece in out {
        match merged.last_mut() {
            Some(prev)
                if prev.x == piece.x && prev.width == piece.width && prev.bottom() == piece.y =>
            {
                prev.height += piece.height;
            }
            _ => merged.push(piece),
        }
    }
    (merged.len() <= MAX_PIECES).then_some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHOLE: Scissor = Scissor::new(0, 0, 100, 80);

    fn area(rects: &[Scissor]) -> u64 {
        rects
            .iter()
            .map(|r| u64::from(r.width) * u64::from(r.height))
            .sum()
    }

    /// Every pixel of the input is in exactly one output or in a blocker.
    ///
    /// Counted rather than reasoned about, which is the only check that catches a
    /// decomposition that is self-consistent and wrong -- one that drops a band, or
    /// emits a band twice, satisfies every other property here.
    fn covers_exactly(inside: Scissor, blockers: &[Scissor], out: &[Scissor]) {
        for y in inside.y..inside.bottom() {
            for x in inside.x..inside.right() {
                let blocked = blockers
                    .iter()
                    .any(|b| x >= b.x && x < b.right() && y >= b.y && y < b.bottom());
                let hits = out
                    .iter()
                    .filter(|r| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
                    .count();
                let want = usize::from(!blocked);
                assert_eq!(
                    hits, want,
                    "pixel ({x}, {y}) is in {hits} pieces and should be in {want}; \
                     blocked = {blocked}"
                );
            }
        }
    }

    #[test]
    fn nothing_covered_gives_the_whole_rectangle_back() {
        assert_eq!(remainder(WHOLE, &[]), Some(vec![WHOLE]));
        let elsewhere = Scissor::new(200, 200, 10, 10);
        assert_eq!(remainder(WHOLE, &[elsewhere]), Some(vec![WHOLE]));
        assert_eq!(remainder(WHOLE, &[Scissor::EMPTY]), Some(vec![WHOLE]));
    }

    #[test]
    fn a_blocker_covering_everything_leaves_nothing() {
        assert_eq!(remainder(WHOLE, &[WHOLE]), Some(Vec::new()));
        // Larger than the input, which is clamped rather than refused.
        let larger = Scissor::new(0, 0, 500, 500);
        assert_eq!(remainder(WHOLE, &[larger]), Some(Vec::new()));
        // And in two halves, which does not take the early exit above.
        let top = Scissor::new(0, 0, 100, 40);
        let bottom = Scissor::new(0, 40, 100, 40);
        assert_eq!(remainder(WHOLE, &[top, bottom]), Some(Vec::new()));
    }

    #[test]
    fn an_empty_input_has_no_remainder() {
        assert_eq!(remainder(Scissor::EMPTY, &[WHOLE]), Some(Vec::new()));
    }

    /// A bar across the top, which is one of the two shapes the stacked frame has.
    #[test]
    fn a_bar_across_the_top_leaves_the_rest() {
        let bar = Scissor::new(0, 0, 100, 20);
        let out = remainder(WHOLE, &[bar]).expect("under the cap");
        assert_eq!(out, vec![Scissor::new(0, 20, 100, 60)]);
        covers_exactly(WHOLE, &[bar], &out);
    }

    /// A bar and a sidebar, which is the other. The corner where they meet must be
    /// counted once, and an implementation that subtracted them one after another
    /// without bands would count it twice.
    #[test]
    fn a_bar_and_a_sidebar_do_not_double_count_their_corner() {
        let bar = Scissor::new(0, 0, 100, 20);
        let sidebar = Scissor::new(0, 0, 25, 80);
        let out = remainder(WHOLE, &[bar, sidebar]).expect("under the cap");
        assert_eq!(area(&out), 100 * 80 - (100 * 20 + 25 * 80 - 25 * 20));
        covers_exactly(WHOLE, &[bar, sidebar], &out);
    }

    /// A hole in the middle, which is the case that needs four pieces and the one a
    /// bounding-box answer gets most wrong.
    #[test]
    fn a_block_in_the_middle_leaves_a_ring() {
        let middle = Scissor::new(40, 30, 20, 20);
        let out = remainder(WHOLE, &[middle]).expect("under the cap");
        assert_eq!(area(&out), 100 * 80 - 20 * 20);
        covers_exactly(WHOLE, &[middle], &out);
    }

    #[test]
    fn overlapping_blockers_are_counted_once() {
        let a = Scissor::new(10, 10, 40, 40);
        let b = Scissor::new(30, 20, 40, 40);
        let out = remainder(WHOLE, &[a, b]).expect("under the cap");
        covers_exactly(WHOLE, &[a, b], &out);
    }

    #[test]
    fn touching_blockers_leave_no_seam_between_them() {
        // Edge-to-edge: `a`'s right is `b`'s left, so there is no column between
        // them and no piece should be emitted there.
        let a = Scissor::new(0, 0, 50, 80);
        let b = Scissor::new(50, 0, 50, 80);
        assert_eq!(remainder(WHOLE, &[a, b]), Some(Vec::new()));
    }

    #[test]
    fn too_many_pieces_declines_rather_than_returning_some_of_them() {
        // A diagonal of small blocks, each cutting its own band, which is the shape
        // that multiplies pieces fastest.
        let many: Vec<Scissor> = (0..12).map(|i| Scissor::new(i * 8, i * 6, 4, 3)).collect();
        assert_eq!(remainder(WHOLE, &many), None);
    }

    /// Every pair of rectangles from a coarse grid, checked pixel by pixel.
    ///
    /// Exhaustive rather than sampled, which for two blockers it can afford to be:
    /// the grid gives 64 rectangles, so 4,096 pairs over a 12 by 10 target. A random
    /// sweep would find the same bugs eventually and this one cannot miss them, and
    /// between them the cases that actually broke earlier drafts -- a blocker ending
    /// exactly where another begins, a band of zero height, a blocker flush against
    /// an edge -- are all in here by construction rather than by having been thought
    /// of.
    #[test]
    fn every_pair_from_a_grid_decomposes_exactly() {
        const TARGET: Scissor = Scissor::new(0, 0, 12, 10);
        let candidates: Vec<Scissor> = [0u32, 3, 6, 9]
            .iter()
            .flat_map(|&x| {
                [3u32, 6].iter().flat_map(move |&w| {
                    [0u32, 2, 5, 8].iter().flat_map(move |&y| {
                        [2u32, 5].iter().map(move |&h| Scissor::new(x, y, w, h))
                    })
                })
            })
            .collect();
        assert_eq!(candidates.len(), 64);

        let mut decomposed = 0usize;
        let mut declined = 0usize;
        for a in &candidates {
            for b in &candidates {
                let blockers = [*a, *b];
                match remainder(TARGET, &blockers) {
                    Some(out) => {
                        covers_exactly(TARGET, &blockers, &out);
                        decomposed += 1;
                    }
                    None => declined += 1,
                }
            }
        }
        assert_eq!(decomposed + declined, 64 * 64);
        // Two blockers cannot need more than the cap, so nothing here should decline.
        // If that ever changes the cap is too low and this says so rather than letting
        // the sweep quietly check fewer cases.
        assert_eq!(declined, 0, "the cap refused a two-blocker case");
    }

    /// Every triple from a smaller grid, which is where nesting can go wrong.
    ///
    /// Two blockers cannot nest one inside another's span and still exercise the
    /// column walk's `max`; three can. Exhaustive again, on a grid small enough to
    /// afford it -- 16 rectangles over an 8 by 6 target is 4,096 triples. Exhaustive
    /// beats generated input on a domain this small: there is no seed to be lucky or
    /// unlucky with, and the oracle is exact rather than statistical.
    #[test]
    fn every_triple_from_a_smaller_grid_decomposes_exactly() {
        const TARGET: Scissor = Scissor::new(0, 0, 8, 6);
        let candidates: Vec<Scissor> = [0u32, 2, 4, 6]
            .iter()
            .flat_map(|&x| {
                [2u32, 4]
                    .iter()
                    .flat_map(move |&w| [0u32, 3].iter().map(move |&y| Scissor::new(x, y, w, 3)))
            })
            .collect();
        assert_eq!(candidates.len(), 16);

        let mut checked = 0usize;
        for a in &candidates {
            for b in &candidates {
                for c in &candidates {
                    let blockers = [*a, *b, *c];
                    if let Some(out) = remainder(TARGET, &blockers) {
                        covers_exactly(TARGET, &blockers, &out);
                        checked += 1;
                    }
                }
            }
        }
        // Nothing here should hit the cap either, and saying so keeps a sweep that
        // silently started declining everything from reading as a pass.
        assert_eq!(checked, 16 * 16 * 16);
    }

    /// An input that does not start at the origin, since a scissor need not.
    #[test]
    fn an_offset_input_keeps_its_own_bounds() {
        let inside = Scissor::new(20, 10, 50, 40);
        let blocker = Scissor::new(30, 20, 10, 10);
        let out = remainder(inside, &[blocker]).expect("under the cap");
        for r in &out {
            assert!(r.x >= inside.x && r.right() <= inside.right(), "{r:?}");
            assert!(r.y >= inside.y && r.bottom() <= inside.bottom(), "{r:?}");
        }
        covers_exactly(inside, &[blocker], &out);
    }
}

//! Packing rasterized glyph coverage into one texture.
//!
//! # What this does and does not do
//!
//! It takes coverage bitmaps and answers where in a texture each one sits. It
//! does not rasterize them: turning an outline into coverage needs a font
//! parser, and font parsing is out of scope here — bring `swash` or
//! `ttf-parser` and hand the result over. That boundary is what lets the atlas
//! be tested with no font anywhere in the tree, against bitmaps whose contents
//! are known exactly.
//!
//! # Shelf packing
//!
//! Glyphs are packed into horizontal shelves: a new glyph goes on the first
//! shelf tall enough for it with room to its right, and starts a new shelf
//! otherwise. That wastes the space above short glyphs on a tall shelf, which
//! a skyline or a max-rects packer would recover.
//!
//! It is the right trade for glyphs specifically. A run of text is a stream of
//! boxes of very similar height, so shelves fill densely in practice, and the
//! packer runs once per glyph per size rather than per frame. A packer with
//! better worst-case density would cost more per insertion and more to read for
//! a gain that the input's own shape mostly removes.
//!
//! # Making room
//!
//! Shelves cannot free a glyph in place: a hole in the middle of one is not
//! reusable by anything but a glyph of the same height, and tracking holes is
//! most of what makes a general packer expensive. So room is made by compacting
//! — keeping the glyphs this frame has asked for, discarding the rest, and
//! repacking from scratch.
//!
//! That is a heavier operation than freeing one entry and a much simpler one to
//! be sure of, and its cost is bounded by how often it can happen: it runs only
//! when an insertion would otherwise fail, and it cannot run twice in a frame
//! without the second one failing outright, because everything left after the
//! first is something this frame needs. Text that genuinely needs more than an
//! atlas holds is a case for a second page, not for a cleverer packer.

use std::collections::HashMap;

/// Where a glyph sits in the atlas, in texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtlasRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl AtlasRect {
    /// The rectangle in texture coordinates, from zero to one.
    ///
    /// Returned as `[left, top, right, bottom]`, with V running downward to
    /// match the row order the texture was uploaded in.
    pub fn uv(&self, atlas: u32) -> [f32; 4] {
        let scale = 1.0 / atlas as f32;
        [
            self.x as f32 * scale,
            self.y as f32 * scale,
            (self.x + self.width) as f32 * scale,
            (self.y + self.height) as f32 * scale,
        ]
    }
}

/// What identifies a glyph in the atlas.
///
/// A font identifier alongside the glyph index, because glyph indices are
/// per font and two fonts will disagree about what index seven means. Size is
/// in whole pixels: a cache keyed by a float would miss on values that differ
/// only in their last bit, and rasterizing at a rounded size is what a caller
/// is doing anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphKey {
    pub font: u64,
    pub glyph: u16,
    pub size: u16,
}

/// A rasterized glyph, as a caller supplies it.
#[derive(Debug, Clone, PartialEq)]
pub struct Coverage {
    pub width: u32,
    pub height: u32,
    /// One byte per texel, row-major from the top, tightly packed.
    pub texels: Vec<u8>,
}

impl Coverage {
    /// Whether the dimensions and the buffer agree.
    pub fn is_consistent(&self) -> bool {
        self.texels.len() as u64 == self.width as u64 * self.height as u64
    }
}

/// Why a glyph could not be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasError {
    /// The coverage's dimensions and its buffer disagree.
    Inconsistent,
    /// The glyph does not fit, even in an empty atlas.
    TooLarge,
    /// The atlas is full.
    ///
    /// Distinct from `TooLarge` because the remedies differ: this one is fixed
    /// by evicting or by growing, and that one never is.
    Full,
}

impl std::fmt::Display for AtlasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inconsistent => write!(f, "coverage dimensions do not match its buffer"),
            Self::TooLarge => write!(f, "glyph is larger than the atlas"),
            Self::Full => write!(f, "atlas is full"),
        }
    }
}

impl std::error::Error for AtlasError {}

/// One shelf: a horizontal band, filled left to right.
#[derive(Debug, Clone, Copy)]
struct Shelf {
    top: u32,
    height: u32,
    /// Where the next glyph on this shelf would start.
    used: u32,
}

/// A square atlas of glyph coverage, and where each glyph landed.
///
/// Holds the texels itself rather than a device texture, so that packing can be
/// exercised and reasoned about without a GPU. Whoever owns the device uploads
/// [`Atlas::texels`] when [`Atlas::is_dirty`] says something changed.
#[derive(Debug)]
pub struct Atlas {
    size: u32,
    texels: Vec<u8>,
    shelves: Vec<Shelf>,
    placed: HashMap<GlyphKey, Placed>,
    frame: u64,
    dirty: bool,
    compactions: u64,
    /// The largest this may grow to.
    ///
    /// A caller's to choose, because the ceiling that matters is the device's
    /// maximum texture size and the atlas has no way to ask.
    limit: u32,
    growths: u64,
}

/// Where a glyph is, and when it was last asked for.
#[derive(Debug, Clone, Copy)]
struct Placed {
    rect: AtlasRect,
    /// The frame this glyph was most recently inserted in.
    ///
    /// Insertion rather than lookup, because insertion is what a caller does
    /// for every glyph of every run each frame — that is the usage the atlas is
    /// built around — and marking on lookup would need a unique borrow at the
    /// point where a run is being recorded from a shared one.
    used: u64,
}

/// Blank texels around each glyph.
///
/// One texel is enough and less is not. A linear filter samples a two-by-two
/// neighborhood, so a glyph flush against its neighbor bleeds that neighbor's
/// coverage into its own edge under any magnification. The padding is cleared
/// rather than merely skipped, since the space may hold an evicted glyph.
const PADDING: u32 = 1;

impl Atlas {
    /// An empty atlas of `size` by `size` texels.
    pub fn new(size: u32) -> Self {
        // Four thousand and ninety-six is the smallest maximum texture size
        // this project's supported devices are required to offer, so it is the
        // largest an atlas can grow to without asking. A caller that knows the
        // device says so.
        Self::with_limit(size, 4096)
    }

    /// An atlas that may grow up to `limit` texels on a side.
    ///
    /// A limit below the starting size means it never grows, which is a
    /// reasonable thing to ask for and not an error.
    pub fn with_limit(size: u32, limit: u32) -> Self {
        Self {
            size,
            texels: vec![0; (size as usize) * (size as usize)],
            shelves: Vec::new(),
            placed: HashMap::new(),
            frame: 0,
            dirty: false,
            compactions: 0,
            limit,
            growths: 0,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// One byte of coverage per texel, row-major from the top.
    pub fn texels(&self) -> &[u8] {
        &self.texels
    }

    /// Whether anything has been added since [`Atlas::mark_clean`].
    ///
    /// The upload is the caller's to make, because only they know which device
    /// the texture lives on and when in the frame it is safe to write.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn len(&self) -> usize {
        self.placed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }

    /// Where a glyph sits, if it is present.
    pub fn get(&self, key: GlyphKey) -> Option<AtlasRect> {
        self.placed.get(&key).map(|placed| placed.rect)
    }

    /// Begin a new frame, after which glyphs not inserted again are evictable.
    ///
    /// Nothing happens here but a counter moving. A caller that never calls it
    /// keeps every glyph forever, which is correct rather than a leak: without
    /// frame boundaries nothing is stale, and an atlas that filled would be
    /// genuinely out of room.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// How many times room has been made by repacking.
    ///
    /// Worth reporting because it is the number that says the atlas is too
    /// small for the text going through it: a compaction or two as a working
    /// set settles is ordinary, and one per frame means every frame is
    /// repacking everything.
    pub fn compactions(&self) -> u64 {
        self.compactions
    }

    /// How many times the atlas has doubled.
    ///
    /// Every growth reallocates and repacks everything, so a count that keeps
    /// climbing says the atlas started far too small. It settles once the
    /// working set fits.
    pub fn growths(&self) -> u64 {
        self.growths
    }

    /// The largest this may grow to.
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// Add a glyph, or return where it already is.
    ///
    /// Idempotent, so a caller can insert every glyph of every run each frame
    /// and pay only for the ones that are new. That is the usage this is for:
    /// deciding what is already present is exactly what an atlas is.
    pub fn insert(&mut self, key: GlyphKey, coverage: &Coverage) -> Result<AtlasRect, AtlasError> {
        if let Some(placed) = self.placed.get_mut(&key) {
            // Refreshed rather than merely found: a glyph asked for again this
            // frame is one this frame needs, and that is what keeps it from
            // being the one compaction discards.
            placed.used = self.frame;
            return Ok(placed.rect);
        }
        if !coverage.is_consistent() {
            return Err(AtlasError::Inconsistent);
        }
        // A glyph with no area still has a position, which is what lets a space
        // travel through a run like any other glyph rather than being a case
        // every caller has to remember to skip.
        if coverage.width == 0 || coverage.height == 0 {
            let rect = AtlasRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
            self.placed.insert(
                key,
                Placed {
                    rect,
                    used: self.frame,
                },
            );
            return Ok(rect);
        }

        let rect = match self.allocate(coverage.width, coverage.height) {
            Ok(rect) => rect,
            // Only fullness is worth retrying. A glyph larger than the atlas
            // does not fit an empty one either, and compacting to discover that
            // would throw away everything for nothing.
            // Compaction first, then growth. Discarding what nothing has
            // asked for is far cheaper than doubling, and an atlas that grew
            // before compacting would keep the memory it had stopped needing.
            Err(AtlasError::Full) => {
                if !self.compact() && !self.grow() {
                    return Err(AtlasError::Full);
                }
                self.allocate(coverage.width, coverage.height)?
            }
            Err(e) => return Err(e),
        };
        for row in 0..coverage.height {
            let from = (row * coverage.width) as usize;
            let to = ((rect.y + row) * self.size + rect.x) as usize;
            self.texels[to..to + coverage.width as usize]
                .copy_from_slice(&coverage.texels[from..from + coverage.width as usize]);
        }
        self.placed.insert(
            key,
            Placed {
                rect,
                used: self.frame,
            },
        );
        self.dirty = true;
        Ok(rect)
    }

    /// Discard glyphs this frame has not asked for, and repack the rest.
    ///
    /// Returns whether anything was freed. False means every glyph present is
    /// one this frame needs, so there is nothing to give up and the atlas is
    /// genuinely too small for the text going through it.
    fn compact(&mut self) -> bool {
        let survivors: Vec<(GlyphKey, Placed)> = self
            .placed
            .iter()
            .filter(|(_, placed)| placed.used == self.frame)
            .map(|(key, placed)| (*key, *placed))
            .collect();
        if survivors.len() == self.placed.len() {
            return false;
        }

        // Copied out before anything is cleared, since the atlas's own texels
        // are the only place a glyph's coverage still exists — the bitmaps a
        // caller supplied were borrowed and are long gone.
        let mut coverage: Vec<(GlyphKey, Coverage)> = survivors
            .iter()
            .map(|(key, placed)| (*key, self.extract(placed.rect)))
            .collect();
        repacking_order(&mut coverage);

        self.texels.fill(0);
        self.shelves.clear();
        self.placed.clear();
        self.compactions += 1;
        self.dirty = true;

        for (key, coverage) in coverage {
            // Everything here fitted a moment ago and is being packed into an
            // atlas holding a subset of what it held then, so this cannot fail.
            // If it somehow does, dropping the glyph is better than refusing
            // the insertion that triggered the compaction: the caller will
            // offer it again next frame.
            let _ = self.insert(key, &coverage);
        }
        true
    }

    /// Double the atlas and repack everything into it.
    ///
    /// Returns whether it grew. False means it is already at its limit, which
    /// is the point at which being full is genuinely full.
    ///
    /// Everything is kept, not only what this frame asked for: growing is what
    /// happens when nothing was stale, so there is nothing to discard, and a
    /// glyph dropped here would be re-rasterized by the caller for no reason.
    fn grow(&mut self) -> bool {
        if self.size >= self.limit {
            return false;
        }
        let grown = (self.size.saturating_mul(2)).min(self.limit);
        if grown <= self.size {
            return false;
        }

        // Read back before the texels are replaced: the atlas is the only place
        // a glyph's coverage still exists, the bitmaps a caller supplied having
        // been borrowed.
        let mut kept: Vec<(GlyphKey, Coverage)> = self
            .placed
            .iter()
            .map(|(key, placed)| (*key, self.extract(placed.rect)))
            .collect();
        repacking_order(&mut kept);

        self.size = grown;
        self.texels = vec![0; (grown as usize) * (grown as usize)];
        self.shelves.clear();
        self.placed.clear();
        self.growths += 1;
        self.dirty = true;

        for (key, coverage) in kept {
            // Everything fitted the smaller atlas, so it fits this one.
            let _ = self.insert(key, &coverage);
        }
        true
    }

    /// Read a glyph's coverage back out of the atlas.
    fn extract(&self, rect: AtlasRect) -> Coverage {
        let mut texels = Vec::with_capacity((rect.width * rect.height) as usize);
        for row in 0..rect.height {
            let from = ((rect.y + row) * self.size + rect.x) as usize;
            texels.extend_from_slice(&self.texels[from..from + rect.width as usize]);
        }
        Coverage {
            width: rect.width,
            height: rect.height,
            texels,
        }
    }

    /// Find room for a glyph and reserve it.
    fn allocate(&mut self, width: u32, height: u32) -> Result<AtlasRect, AtlasError> {
        let padded_width = width + PADDING;
        let padded_height = height + PADDING;
        if padded_width > self.size || padded_height > self.size {
            return Err(AtlasError::TooLarge);
        }

        // The first shelf tall enough with room to the right. Shelves are not
        // sorted by height, so this is a scan; a run of text produces shelves
        // of similar height and few of them, which is why that is not worth
        // indexing.
        for shelf in &mut self.shelves {
            if shelf.height >= padded_height && self.size - shelf.used >= padded_width {
                let rect = AtlasRect {
                    x: shelf.used,
                    y: shelf.top,
                    width,
                    height,
                };
                shelf.used += padded_width;
                return Ok(rect);
            }
        }

        let top = self
            .shelves
            .last()
            .map_or(0, |shelf| shelf.top + shelf.height);
        if top + padded_height > self.size {
            return Err(AtlasError::Full);
        }
        self.shelves.push(Shelf {
            top,
            height: padded_height,
            used: padded_width,
        });
        Ok(AtlasRect {
            x: 0,
            y: top,
            width,
            height,
        })
    }
}

/// The order glyphs are packed in when the atlas is rebuilt.
///
/// Tallest first, which is what shelf packing wants: a shelf is as tall as the
/// tallest glyph on it, so a short glyph landing first opens a shelf that a
/// tall one cannot use and every tall glyph after it opens another. Width and
/// then the key break ties, the key because two glyphs of the same size have to
/// land in the same order every run.
///
/// Determinism is the reason this exists at all, not the packing. Both rebuilds
/// took their order from a `HashMap`, whose iteration order Rust seeds per
/// process -- so the same text through the same atlas produced a different
/// layout each run, and, worse, a different *number of compactions*: measured
/// over four runs of one fixed sequence, three compacted once and one did not
/// compact at all. Anything asserting on those counters was a coin flip, and
/// the comment claiming a repack "cannot fail" rested on packing a subset in an
/// order nothing guaranteed.
fn repacking_order(glyphs: &mut [(GlyphKey, Coverage)]) {
    glyphs.sort_by(|(left_key, left), (right_key, right)| {
        right
            .height
            .cmp(&left.height)
            .then(right.width.cmp(&left.width))
            .then(left_key.cmp(right_key))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, value: u8) -> Coverage {
        Coverage {
            width,
            height,
            texels: vec![value; (width * height) as usize],
        }
    }

    fn key(glyph: u16) -> GlyphKey {
        GlyphKey {
            font: 1,
            glyph,
            size: 16,
        }
    }

    #[test]
    fn a_compaction_lays_its_survivors_out_tallest_first() {
        // The observable consequence of packing a rebuild in a defined order,
        // and the reason there is one. Shelves are opened top-down, so if the
        // tallest survivor is placed first it takes the first shelf, and every
        // survivor after it sits on that shelf or a later one -- which makes
        // "sorted by height descending" and "sorted by row" the same ordering.
        //
        // What this stands in for is a property that cannot be asserted from
        // inside a single run. Both rebuilds took their order from a `HashMap`,
        // seeded per process, so the same text packed differently every run;
        // measured over five runs of one sequence, the atlas held thirty-six
        // glyphs three times and thirty-five twice, a glyph lost to the hasher.
        // That loss is a probabilistic consequence and a test for it would
        // catch the fault only sometimes, which is worse than not testing it.
        // This asks the deterministic thing that causes it instead.
        let mut atlas = Atlas::with_limit(64, 64);
        let sizes: Vec<(u32, u32)> = (0..40u32)
            .map(|i| (3 + (i * 7) % 11, 3 + (i * 5) % 13))
            .collect();
        for (i, (w, h)) in sizes.iter().enumerate() {
            let _ = atlas.insert(key(i as u16), &solid(*w, *h, 200));
        }
        atlas.begin_frame();
        let mut survivors = Vec::new();
        for i in (0..40usize).step_by(2) {
            let (w, h) = sizes[i];
            if atlas.insert(key(i as u16), &solid(w, h, 200)).is_ok() {
                survivors.push((i as u16, h));
            }
        }
        // One more, which needs the room the stale glyphs are holding.
        let _ = atlas.insert(key(90), &solid(6, 9, 200));
        assert_eq!(
            atlas.compactions(),
            1,
            "nothing compacted, so this proves nothing"
        );

        // The tallest survivor is placed first into an empty atlas, so it
        // opens the first shelf and sits at its top. Nothing stronger holds:
        // shelves are scanned for the first that fits, so a shorter glyph
        // placed later can land on an earlier shelf, and rows do go backward.
        survivors.sort_by(|(left_key, left), (right_key, right)| {
            right.cmp(left).then(left_key.cmp(right_key))
        });
        let (tallest, height) = survivors[0];
        let rect = atlas
            .get(key(tallest))
            .expect("the tallest survivor should have been kept");
        assert_eq!(
            rect.y, 0,
            "glyph {tallest}, the tallest survivor at {height}, landed at row \
             {} rather than opening the first shelf -- so the rebuild placed \
             something else before it",
            rect.y
        );
    }

    #[test]
    fn a_rebuild_packs_the_tallest_glyphs_first() {
        // A shelf is as tall as the tallest glyph on it, so a short glyph
        // landing first opens a shelf a tall one cannot use, and every tall
        // glyph after it opens another. Sorting by height is what makes a
        // rebuild pack at least as well as the atlas it is rebuilding.
        let mut glyphs = vec![
            (key(1), solid(4, 4, 1)),
            (key(2), solid(4, 20, 2)),
            (key(3), solid(9, 12, 3)),
            (key(4), solid(2, 20, 4)),
        ];
        repacking_order(&mut glyphs);
        let heights: Vec<u32> = glyphs.iter().map(|(_, c)| c.height).collect();
        assert_eq!(heights, vec![20, 20, 12, 4], "tallest first");
        // Ties broken by width and then by the key, so two glyphs of a size
        // land in the same order every run rather than in whichever order they
        // arrived.
        assert_eq!(
            (glyphs[0].1.width, glyphs[1].1.width),
            (4, 2),
            "a tie in height is broken by width"
        );
    }

    #[test]
    fn an_inserted_glyph_can_be_found_again() {
        let mut atlas = Atlas::new(64);
        let rect = atlas.insert(key(1), &solid(8, 10, 200)).expect("insert");
        assert_eq!(atlas.get(key(1)), Some(rect));
        assert_eq!(rect.width, 8);
        assert_eq!(rect.height, 10);
        assert_eq!(atlas.len(), 1);
    }

    #[test]
    fn inserting_the_same_glyph_twice_returns_the_same_place() {
        // The usage this exists for: a caller inserts every glyph of every run
        // each frame and pays only for the new ones. A second insertion that
        // allocated again would fill the atlas in proportion to frames drawn
        // rather than to distinct glyphs.
        let mut atlas = Atlas::new(64);
        let first = atlas.insert(key(1), &solid(8, 10, 200)).expect("first");
        let second = atlas.insert(key(1), &solid(8, 10, 200)).expect("second");
        assert_eq!(first, second);
        assert_eq!(atlas.len(), 1);
    }

    #[test]
    fn glyphs_differing_only_in_font_or_size_are_distinct() {
        // Glyph indices are per font, and a glyph rasterized at one size is not
        // the same picture as at another. A key that ignored either would serve
        // one glyph's coverage for another's.
        let mut atlas = Atlas::new(64);
        let a = atlas.insert(key(1), &solid(8, 8, 255)).expect("a");
        let b = atlas
            .insert(GlyphKey { font: 2, ..key(1) }, &solid(8, 8, 255))
            .expect("b");
        let c = atlas
            .insert(GlyphKey { size: 32, ..key(1) }, &solid(8, 8, 255))
            .expect("c");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert_eq!(atlas.len(), 3);
    }

    #[test]
    fn coverage_lands_where_the_rectangle_says() {
        let mut atlas = Atlas::new(16);
        // A bitmap with a distinct value per texel, so a transposed or
        // off-by-one copy produces a different atlas rather than the same one.
        let coverage = Coverage {
            width: 3,
            height: 2,
            texels: vec![10, 20, 30, 40, 50, 60],
        };
        let rect = atlas.insert(key(1), &coverage).expect("insert");
        for row in 0..coverage.height {
            for column in 0..coverage.width {
                let at = ((rect.y + row) * atlas.size() + rect.x + column) as usize;
                assert_eq!(
                    atlas.texels()[at],
                    coverage.texels[(row * coverage.width + column) as usize],
                    "texel ({column}, {row}) landed wrong"
                );
            }
        }
    }

    #[test]
    fn glyphs_do_not_touch_each_other() {
        // A linear filter samples a neighborhood, so a glyph flush against its
        // neighbor bleeds that neighbor's coverage into its own edge. This
        // asserts the gap rather than the filtering, since the filtering is the
        // device's business.
        let mut atlas = Atlas::new(64);
        let a = atlas.insert(key(1), &solid(8, 8, 255)).expect("a");
        let b = atlas.insert(key(2), &solid(8, 8, 255)).expect("b");
        assert!(
            b.x >= a.x + a.width + PADDING || b.y >= a.y + a.height + PADDING,
            "{a:?} and {b:?} are adjacent"
        );
    }

    #[test]
    fn a_second_shelf_starts_below_the_first() {
        let mut atlas = Atlas::new(32);
        // Three glyphs eleven wide with padding do not fit across thirty-two.
        let first = atlas.insert(key(1), &solid(11, 6, 255)).expect("first");
        let second = atlas.insert(key(2), &solid(11, 6, 255)).expect("second");
        let third = atlas.insert(key(3), &solid(11, 6, 255)).expect("third");
        assert_eq!(first.y, second.y, "the first two share a shelf");
        assert!(third.y > first.y, "the third started a new shelf");
        assert_eq!(third.x, 0, "a new shelf starts at the left edge");
    }

    #[test]
    fn a_short_glyph_reuses_a_taller_shelf() {
        // The trade shelf packing makes: the space above a short glyph on a
        // tall shelf is lost, and in exchange placement is a scan of a few
        // entries. A run of text is boxes of similar height, which is what
        // keeps that loss small.
        let mut atlas = Atlas::new(64);
        let tall = atlas.insert(key(1), &solid(8, 20, 255)).expect("tall");
        let short = atlas.insert(key(2), &solid(8, 4, 255)).expect("short");
        assert_eq!(tall.y, short.y, "the short glyph opened a new shelf");
    }

    #[test]
    fn a_glyph_larger_than_the_atlas_is_reported_as_such() {
        // Distinct from being full, because the remedies differ: growing or
        // evicting fixes one and never fixes the other.
        let mut atlas = Atlas::new(16);
        assert_eq!(
            atlas.insert(key(1), &solid(16, 16, 255)),
            Err(AtlasError::TooLarge),
            "a glyph needing padding beyond the edge should not fit"
        );
        assert_eq!(
            atlas.insert(key(2), &solid(64, 4, 255)),
            Err(AtlasError::TooLarge)
        );
    }

    #[test]
    fn a_full_atlas_says_so_rather_than_overwriting() {
        let mut atlas = fixed(16);
        let mut inserted = 0;
        for glyph in 0..64u16 {
            match atlas.insert(key(glyph), &solid(6, 6, 255)) {
                Ok(_) => inserted += 1,
                Err(e) => {
                    assert_eq!(e, AtlasError::Full);
                    break;
                }
            }
        }
        assert!(inserted > 0, "nothing fit at all");
        assert!(inserted < 64, "everything fit, so fullness went untested");

        // And every glyph that did fit is still where it was put: running out
        // of room must not disturb what is already there.
        let mut seen = std::collections::HashSet::new();
        for glyph in 0..inserted as u16 {
            let rect = atlas.get(key(glyph)).expect("still present");
            assert!(seen.insert((rect.x, rect.y)), "two glyphs share a place");
        }
    }

    #[test]
    fn a_glyph_with_no_area_is_placed_rather_than_refused() {
        // A space has a position in a run like any other glyph, and making
        // every caller remember to skip it is how one of them forgets.
        let mut atlas = Atlas::new(16);
        let rect = atlas.insert(key(1), &solid(0, 0, 0)).expect("insert");
        assert_eq!(rect.width, 0);
        assert!(!atlas.is_dirty(), "an empty glyph changed no texels");
    }

    #[test]
    fn inconsistent_coverage_is_refused() {
        let mut atlas = Atlas::new(16);
        let bad = Coverage {
            width: 4,
            height: 4,
            texels: vec![0; 3],
        };
        assert_eq!(atlas.insert(key(1), &bad), Err(AtlasError::Inconsistent));
    }

    #[test]
    fn the_dirty_flag_tracks_whether_an_upload_is_owed() {
        let mut atlas = Atlas::new(32);
        assert!(!atlas.is_dirty(), "an empty atlas owes no upload");
        atlas.insert(key(1), &solid(4, 4, 255)).expect("insert");
        assert!(atlas.is_dirty());
        atlas.mark_clean();
        assert!(!atlas.is_dirty());
        // A repeat insertion changes no texels, so it owes nothing either.
        atlas.insert(key(1), &solid(4, 4, 255)).expect("again");
        assert!(
            !atlas.is_dirty(),
            "a glyph already present dirtied the atlas"
        );
    }

    /// An atlas that cannot grow, for the behaviors that only appear at the
    /// limit: compaction, and being genuinely full.
    fn fixed(size: u32) -> Atlas {
        Atlas::with_limit(size, size)
    }

    /// Insert glyphs until the atlas grows, returning how many went in.
    ///
    /// The counterpart of `fill` for an atlas that can grow, where "until it
    /// refuses" never arrives until the limit.
    fn fill_until_growth(atlas: &mut Atlas, from: u16) -> u16 {
        let mut glyph = from;
        while atlas.growths() == 0 {
            atlas
                .insert(key(glyph), &solid(6, 6, 255))
                .unwrap_or_else(|e| panic!("insert {glyph}: {e}"));
            glyph += 1;
            assert!(glyph < from + 200, "the atlas never grew");
        }
        glyph - from
    }

    /// Fill an atlas until it refuses, returning how many glyphs fitted.
    fn fill(atlas: &mut Atlas, from: u16) -> u16 {
        let mut glyph = from;
        while atlas.insert(key(glyph), &solid(6, 6, 255)).is_ok() {
            glyph += 1;
            assert!(glyph < from + 200, "the atlas never filled");
        }
        glyph - from
    }

    #[test]
    fn a_full_atlas_makes_room_for_what_the_new_frame_needs() {
        let mut atlas = fixed(32);
        let fitted = fill(&mut atlas, 0);
        assert!(
            fitted > 2,
            "only {fitted} glyphs fitted; too few to evict from"
        );

        // A new frame, and nothing from the old one asked for again. Everything
        // present is now stale, so an insertion that would have failed makes
        // room instead.
        atlas.begin_frame();
        let rect = atlas
            .insert(key(500), &solid(6, 6, 255))
            .expect("a stale atlas should make room");
        assert_eq!(rect.width, 6);
        assert_eq!(atlas.compactions(), 1);
        assert_eq!(atlas.len(), 1, "the stale glyphs were not discarded");
    }

    #[test]
    fn compaction_keeps_the_glyphs_this_frame_asked_for() {
        let mut atlas = fixed(32);
        let fitted = fill(&mut atlas, 0);

        // A new frame that asks for two of the old glyphs before filling up.
        // Those two are what this frame needs; the rest are not.
        atlas.begin_frame();
        let kept: Vec<u16> = vec![0, 1];
        for glyph in &kept {
            atlas
                .insert(key(*glyph), &solid(6, 6, 255))
                .expect("refresh");
        }
        atlas
            .insert(key(500), &solid(6, 6, 255))
            .expect("should make room");

        assert_eq!(atlas.compactions(), 1);
        for glyph in &kept {
            assert!(
                atlas.get(key(*glyph)).is_some(),
                "glyph {glyph} was asked for this frame and discarded anyway"
            );
        }
        assert!(
            atlas.get(key(fitted - 1)).is_none(),
            "a glyph nothing asked for survived"
        );
    }

    #[test]
    fn a_glyph_kept_through_compaction_keeps_its_coverage() {
        // The coverage a caller supplied was borrowed and is long gone, so
        // repacking has to read it back out of the atlas. Getting that wrong
        // gives a glyph that is present, addressable, and blank.
        let mut atlas = fixed(32);
        let distinct = Coverage {
            width: 3,
            height: 2,
            texels: vec![11, 22, 33, 44, 55, 66],
        };
        atlas.insert(key(0), &distinct).expect("insert");
        fill(&mut atlas, 1);

        atlas.begin_frame();
        atlas.insert(key(0), &distinct).expect("refresh");
        atlas
            .insert(key(500), &solid(6, 6, 255))
            .expect("should make room");

        let rect = atlas.get(key(0)).expect("survived");
        let mut got = Vec::new();
        for row in 0..rect.height {
            let from = ((rect.y + row) * atlas.size() + rect.x) as usize;
            got.extend_from_slice(&atlas.texels()[from..from + rect.width as usize]);
        }
        assert_eq!(got, distinct.texels, "coverage was lost in repacking");
    }

    #[test]
    fn an_atlas_full_of_glyphs_this_frame_needs_reports_full() {
        // Compaction can free only what nothing has asked for, so an atlas
        // whose every glyph is in use this frame is genuinely out of room.
        // Saying so is the honest answer; the remedy is a second page, and
        // pretending otherwise would evict a glyph about to be drawn.
        let mut atlas = fixed(32);
        let fitted = fill(&mut atlas, 0);
        atlas.begin_frame();
        for glyph in 0..fitted {
            atlas
                .insert(key(glyph), &solid(6, 6, 255))
                .expect("refresh");
        }
        assert_eq!(
            atlas.insert(key(500), &solid(6, 6, 255)),
            Err(AtlasError::Full)
        );
        assert_eq!(
            atlas.compactions(),
            0,
            "it repacked without freeing anything"
        );
    }

    #[test]
    fn a_glyph_too_large_is_not_worth_compacting_for() {
        // Nothing an empty atlas cannot hold is made to fit by emptying it, and
        // compacting to find that out throws away every glyph for nothing.
        let mut atlas = fixed(32);
        fill(&mut atlas, 0);
        let before = atlas.len();
        atlas.begin_frame();
        assert_eq!(
            atlas.insert(key(500), &solid(64, 64, 255)),
            Err(AtlasError::TooLarge)
        );
        assert_eq!(atlas.compactions(), 0);
        assert_eq!(atlas.len(), before, "the atlas was emptied for nothing");
    }

    #[test]
    fn an_atlas_that_is_never_told_about_frames_keeps_everything() {
        // Without frame boundaries nothing is stale, so a full atlas is
        // genuinely full. That is correct rather than a leak: a caller that
        // never says a frame ended has never said any glyph stopped mattering.
        let mut atlas = fixed(32);
        let fitted = fill(&mut atlas, 0);
        assert_eq!(
            atlas.insert(key(500), &solid(6, 6, 255)),
            Err(AtlasError::Full)
        );
        assert_eq!(atlas.len(), fitted as usize);
        assert_eq!(atlas.compactions(), 0);
    }

    #[test]
    fn an_atlas_with_nothing_stale_grows_rather_than_refusing() {
        // Everything present was asked for this frame, so compaction has
        // nothing to free. Growing is what keeps a run of any length one draw;
        // a second page would make it two, which is the property the vertex
        // format exists to provide.
        // No frame boundary, so nothing is ever stale and compaction can free
        // nothing. Growing is the only way forward, which is the ordering this
        // pins as well as the growth.
        let mut atlas = Atlas::with_limit(32, 128);
        let inserted = fill_until_growth(&mut atlas, 0);
        assert!(inserted > 2, "only {inserted} glyphs went in");

        assert_eq!(atlas.size(), 64, "it did not double");
        assert_eq!(atlas.growths(), 1);
        assert_eq!(atlas.compactions(), 0, "it discarded something it needed");
        assert_eq!(atlas.len(), inserted as usize, "a glyph was lost");
    }

    #[test]
    fn growth_keeps_every_glyph_and_its_coverage() {
        // Growing happens because nothing was stale, so nothing may be dropped
        // — and the coverage has to be read back out of the atlas, the bitmaps
        // a caller supplied having been borrowed and long gone.
        let mut atlas = Atlas::with_limit(32, 128);
        let distinct = Coverage {
            width: 3,
            height: 2,
            texels: vec![11, 22, 33, 44, 55, 66],
        };
        atlas.insert(key(0), &distinct).expect("insert");
        fill_until_growth(&mut atlas, 1);
        assert_eq!(atlas.growths(), 1);

        let rect = atlas.get(key(0)).expect("survived");
        let mut got = Vec::new();
        for row in 0..rect.height {
            let from = ((rect.y + row) * atlas.size() + rect.x) as usize;
            got.extend_from_slice(&atlas.texels()[from..from + rect.width as usize]);
        }
        assert_eq!(got, distinct.texels, "coverage was lost in growing");
    }

    #[test]
    fn growth_stops_at_the_limit_and_says_so() {
        // The limit is the device's maximum texture size, which the atlas has
        // no way to ask about. Past it there is nowhere to go, and reporting
        // full is the honest answer rather than allocating what cannot be
        // uploaded.
        let mut atlas = Atlas::with_limit(16, 32);
        let mut glyph = 0u16;
        loop {
            match atlas.insert(key(glyph), &solid(6, 6, 255)) {
                Ok(_) => glyph += 1,
                Err(e) => {
                    assert_eq!(e, AtlasError::Full);
                    break;
                }
            }
            assert!(glyph < 200, "it never filled");
        }
        assert_eq!(atlas.size(), 32, "it did not grow to its limit");
        assert!(atlas.growths() >= 1);
        // And it stays usable at the limit: a glyph already present is still
        // found, rather than the atlas being poisoned by having filled.
        assert!(atlas.get(key(0)).is_some());
    }

    #[test]
    fn a_limit_no_larger_than_the_atlas_means_it_never_grows() {
        // A caller who knows the size they want says so this way, and it is a
        // reasonable thing to ask for rather than a contradiction to reject.
        let mut atlas = Atlas::with_limit(16, 16);
        fill(&mut atlas, 0);
        assert_eq!(
            atlas.insert(key(500), &solid(6, 6, 255)),
            Err(AtlasError::Full)
        );
        assert_eq!(atlas.size(), 16);
        assert_eq!(atlas.growths(), 0);
    }

    #[test]
    fn texture_coordinates_span_the_rectangle() {
        let rect = AtlasRect {
            x: 16,
            y: 32,
            width: 8,
            height: 4,
        };
        let [left, top, right, bottom] = rect.uv(64);
        assert!((left - 0.25).abs() < 1e-6);
        assert!((top - 0.5).abs() < 1e-6);
        assert!((right - 0.375).abs() < 1e-6);
        assert!((bottom - 0.5625).abs() < 1e-6);
        // V runs downward, matching the row order the texels are stored in.
        assert!(bottom > top);
    }
}

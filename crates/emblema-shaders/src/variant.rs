// Included by `build.rs` as well as compiled into the library, so the build
// step and the lookup agree on what each variant is and where it sits without
// a second list to keep in step. That is why this file names nothing outside
// itself and carries no inner doc comment.

/// Which material arms of the built-in shader a variant carries, beyond the
/// core every variant has: a solid color, the gradients, and glyphs.
///
/// One at most, because a draw has one material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SolidKinds {
    /// The core alone.
    Core,
    /// An image.
    Images,
    /// A mesh sampling a texture at its own coordinates.
    Meshes,
    /// Rounded rectangles, ellipses, a rounded rectangle's blur, point fields
    /// and shadow meshes.
    Shapes,
    /// A blur or a morphology pass.
    Passes,
}

impl SolidKinds {
    /// Every value, in the order the variants are laid out.
    pub const ALL: [SolidKinds; 5] = [
        SolidKinds::Core,
        SolidKinds::Images,
        SolidKinds::Meshes,
        SolidKinds::Shapes,
        SolidKinds::Passes,
    ];
}

/// One translation of the built-in shader, with only the parts a draw needs.
///
/// Each part left out is code a driver does not compile. The whole shader
/// took seconds to compile on some drivers with their shader caches empty;
/// see the note above the overrides in `solid.wgsl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SolidVariant {
    pub kinds: SolidKinds,
    /// Applies a color filter.
    pub filters: bool,
    /// Blends the tint by a mode other than `Modulate`.
    pub tint_blends: bool,
}

impl SolidVariant {
    /// How many variants there are.
    pub const COUNT: usize = SolidKinds::ALL.len() * 4;

    /// The variant a draw with a solid color, a gradient or a glyph needs,
    /// unfiltered and tinted by `Modulate`: what most of a frame draws with.
    pub const CORE: SolidVariant = SolidVariant {
        kinds: SolidKinds::Core,
        filters: false,
        tint_blends: false,
    };

    /// Where this variant sits among [`SolidVariant::every`].
    pub const fn index(self) -> usize {
        let kinds = match self.kinds {
            SolidKinds::Core => 0,
            SolidKinds::Images => 1,
            SolidKinds::Meshes => 2,
            SolidKinds::Shapes => 3,
            SolidKinds::Passes => 4,
        };
        kinds * 4 + (self.filters as usize) * 2 + self.tint_blends as usize
    }

    /// Every variant, in index order.
    pub fn every() -> impl Iterator<Item = SolidVariant> {
        SolidKinds::ALL.into_iter().flat_map(|kinds| {
            [(false, false), (false, true), (true, false), (true, true)].map(
                move |(filters, tint_blends)| SolidVariant {
                    kinds,
                    filters,
                    tint_blends,
                },
            )
        })
    }

    /// The value of each of `solid.wgsl`'s overrides for this variant.
    pub fn overrides(self) -> [(&'static str, bool); 6] {
        [
            ("IMAGES", self.kinds == SolidKinds::Images),
            ("MESHES", self.kinds == SolidKinds::Meshes),
            ("SHAPES", self.kinds == SolidKinds::Shapes),
            ("PASSES", self.kinds == SolidKinds::Passes),
            ("FILTERS", self.filters),
            ("TINT_BLENDS", self.tint_blends),
        ]
    }
}

//! Geometry and pixel format types shared by the rendering and presentation
//! axes.
//!
//! DRM fourcc codes and format modifiers live here rather than in the
//! presentation layer because format negotiation runs *between* the two axes:
//! a presentation target advertises what it can scan out, the HAL context
//! advertises what it can render to and export, and the intersection is what
//! gets allocated.

/// A 2D extent in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Extent2D {
    pub width: u32,
    pub height: u32,
}

impl Extent2D {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Total pixel count, widened so large surfaces cannot overflow.
    pub const fn area(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Pixel formats the renderer can target.
///
/// Color is sRGB-encoded f32 internally; these describe storage at the API
/// boundary and at target write, and no transfer function sits between the
/// two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// 8-bit RGBA, unsigned normalized.
    Rgba8Unorm,
    /// 8-bit RGBA with sRGB transfer on write.
    Rgba8UnormSrgb,
    /// 8-bit BGRA, the common scanout order.
    Bgra8Unorm,
    /// 8-bit BGRA with sRGB transfer on write.
    Bgra8UnormSrgb,
    /// 10-bit color with 2-bit alpha, preferred when the pipeline is HDR-aware.
    Rgb10A2Unorm,
    /// 16-bit float per channel, for intermediate targets.
    Rgba16Float,
    /// One 8-bit channel, unsigned normalized.
    ///
    /// For data that is coverage rather than color — a glyph atlas is the
    /// case — where storing the same byte four times costs four times the
    /// memory and four times the bandwidth to sample it.
    R8Unorm,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8Unorm
            | Self::Rgba8UnormSrgb
            | Self::Bgra8Unorm
            | Self::Bgra8UnormSrgb
            | Self::Rgb10A2Unorm => 4,
            Self::Rgba16Float => 8,
            Self::R8Unorm => 1,
        }
    }

    /// The format an offscreen layer takes when the frame lands in this one.
    ///
    /// Following the root is what keeps the cost where the choice was made: a
    /// caller who asks for a floating-point surface gets layers that can hold
    /// what it holds, and a caller who does not pays nothing. A layer is an
    /// intermediate of *this* frame, so it should carry at least what the frame
    /// it composites into can carry.
    ///
    /// Stated as a match rather than as identity because two formats here would
    /// make bad layers. A single-channel one has nowhere to put color at all.
    /// And ten-bit color with two-bit alpha is worse for a layer than the
    /// eight-bit target it would replace, because a layer's alpha is group
    /// opacity -- a value that gets composited -- rather than a scanout channel
    /// nothing reads back.
    ///
    /// Never an sRGB format, whatever the root is. The pipeline carries encoded
    /// components, so a target that encodes on write would encode them a second
    /// time and a layer would come back paler than the same content drawn
    /// straight onto the frame.
    ///
    /// This followed the root for a while, and was right to: color was light
    /// then, eight bits of *linear* color band visibly in the darks, and a dark
    /// ramp resolving forty distinct tones drawn straight into an sRGB frame
    /// came back as six through a layer holding linear eight-bit color. Neither
    /// half of that can happen now -- a layer's eight bits are spaced by the
    /// transfer function because the values arriving already are.
    pub const fn intermediate(self) -> Self {
        match self {
            Self::Rgba16Float => Self::Rgba16Float,
            // Never the sRGB sibling of an eight-bit format, even when the
            // frame it composites into is one. The pipeline holds encoded
            // components, so a target that encodes on write would encode them a
            // second time -- a layer would come back washed out against the
            // same content drawn directly. This did follow the frame, back when
            // the values reaching it were light.
            _ => Self::Rgba8Unorm,
        }
    }

    /// How far apart two representable values are in this format's storage,
    /// or zero where the question does not apply.
    ///
    /// The distance a dither has to bridge. It is stated in storage units
    /// rather than in light, which for [`Self::Rgba8UnormSrgb`] and its sibling
    /// is not the same thing: the hardware encodes on write, so a step there is
    /// a step of the *encoded* value and the light it stands for varies across
    /// the range by a factor of about thirty. Anything acting on this number
    /// therefore has to know which space it is in, which is what
    /// [`Self::is_srgb`] answers.
    ///
    /// Zero for [`Self::Rgba16Float`], where there is no fixed quantum to
    /// bridge -- half's precision is relative, so a step near black is minute
    /// and a dither sized for one near white would swamp it. Zero for
    /// [`Self::R8Unorm`] as well, which holds coverage rather than color.
    pub const fn quantization_step(self) -> f32 {
        match self {
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb | Self::Bgra8Unorm | Self::Bgra8UnormSrgb => {
                1.0 / 255.0
            }
            Self::Rgb10A2Unorm => 1.0 / 1023.0,
            Self::Rgba16Float | Self::R8Unorm => 0.0,
        }
    }

    /// Whether the renderer can draw into this format.
    ///
    /// It cannot draw into one that encodes on write. The pipeline carries
    /// sRGB-encoded components from the API boundary onward, so a target
    /// applying the transfer function would apply it a second time and the
    /// picture would come back over a third too bright at mid gray.
    ///
    /// Stated once and consulted from three places, because it has been got
    /// wrong at two of them and neither showed up in a test that renders
    /// pixels. The swapchain preferred an sRGB surface format and DRM scanout
    /// rendered through an sRGB image view, both for the same reason -- they
    /// were written while the pipeline carried light, where encoding on write
    /// was exactly what was wanted. A rule that lives in one place can be
    /// re-read; three copies of an argument get updated one at a time.
    pub const fn is_drawable(self) -> bool {
        !self.is_srgb()
    }

    /// Whether writes to this format apply an sRGB transfer function.
    pub const fn is_srgb(self) -> bool {
        matches!(self, Self::Rgba8UnormSrgb | Self::Bgra8UnormSrgb)
    }

    /// The DRM fourcc this format scans out as, if any.
    ///
    /// Intermediate formats have no scanout representation and return `None`,
    /// which is what keeps them out of negotiation.
    pub const fn fourcc(self) -> Option<Fourcc> {
        match self {
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb => Some(Fourcc::ABGR8888),
            Self::Bgra8Unorm | Self::Bgra8UnormSrgb => Some(Fourcc::ARGB8888),
            Self::Rgb10A2Unorm => Some(Fourcc::XRGB2101010),
            // Neither is anything a display controller scans out: one is an
            // intermediate precision and the other is coverage rather than
            // color. Returning nothing is what keeps both out of negotiation.
            Self::Rgba16Float | Self::R8Unorm => None,
        }
    }
}

/// A DRM fourcc format code.
///
/// Stored as the packed little-endian character code the kernel uses, so it
/// can be handed to KMS unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fourcc(pub u32);

impl Fourcc {
    pub const fn new(code: [u8; 4]) -> Self {
        Self(u32::from_le_bytes(code))
    }

    pub const ARGB8888: Self = Self::new(*b"AR24");
    pub const XRGB8888: Self = Self::new(*b"XR24");
    pub const ABGR8888: Self = Self::new(*b"AB24");
    pub const XBGR8888: Self = Self::new(*b"XB24");
    pub const XRGB2101010: Self = Self::new(*b"XR30");
    pub const ARGB2101010: Self = Self::new(*b"AR30");

    /// The code as its four characters, for logging.
    pub const fn to_bytes(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

impl std::fmt::Display for Fourcc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let b = self.to_bytes();
        for c in b {
            write!(f, "{}", c as char)?;
        }
        Ok(())
    }
}

/// A DRM format modifier describing the memory layout of a buffer.
///
/// Getting this right is a bandwidth question, not a correctness one: falling
/// back to [`Modifier::LINEAR`] when a vendor tiled or compressed layout was
/// available can halve effective memory bandwidth on an embedded panel. The
/// chosen modifier is therefore always logged, and a negotiation that cannot
/// find a common non-linear layout is reported rather than silently accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Modifier(pub u64);

impl Modifier {
    /// Plain linear layout. Universally supported and universally slow.
    pub const LINEAR: Self = Self(0);

    /// The "driver picks" sentinel, `DRM_FORMAT_MOD_INVALID`.
    ///
    /// Valid only where no explicit modifier negotiation happened. It must
    /// never reach an atomic commit that was built from a negotiated set.
    pub const INVALID: Self = Self(0x00ff_ffff_ffff_ffff);

    pub const fn is_linear(self) -> bool {
        self.0 == Self::LINEAR.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != Self::INVALID.0
    }
}

impl std::fmt::Display for Modifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_linear() {
            write!(f, "LINEAR")
        } else if !self.is_valid() {
            write!(f, "INVALID")
        } else {
            write!(f, "{:#018x}", self.0)
        }
    }
}

/// A format paired with the layouts a device will accept for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatModifierSet {
    pub fourcc: Fourcc,
    pub modifiers: Vec<Modifier>,
}

impl FormatModifierSet {
    pub fn new(fourcc: Fourcc, modifiers: impl Into<Vec<Modifier>>) -> Self {
        Self {
            fourcc,
            modifiers: modifiers.into(),
        }
    }

    /// Modifiers common to both sides, preferring non-linear layouts.
    ///
    /// Order matters: the caller takes the first entry, so linear sorts last
    /// and is chosen only when nothing better is shared.
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        if self.fourcc != other.fourcc {
            return None;
        }
        let mut modifiers: Vec<Modifier> = self
            .modifiers
            .iter()
            .filter(|m| other.modifiers.contains(m))
            .copied()
            .collect();
        if modifiers.is_empty() {
            return None;
        }
        modifiers.sort_by_key(|m| (m.is_linear(), m.0));
        Some(Self {
            fourcc: self.fourcc,
            modifiers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_round_trips_through_characters() {
        assert_eq!(Fourcc::new(*b"AR24"), Fourcc::ARGB8888);
        assert_eq!(&Fourcc::ARGB8888.to_bytes(), b"AR24");
        assert_eq!(Fourcc::XRGB2101010.to_string(), "XR30");
    }

    #[test]
    fn scanout_formats_map_to_fourcc_and_intermediates_do_not() {
        assert_eq!(PixelFormat::Bgra8Unorm.fourcc(), Some(Fourcc::ARGB8888));
        assert_eq!(PixelFormat::Rgba8Unorm.fourcc(), Some(Fourcc::ABGR8888));
        // Rgba16Float is an intermediate target; it must stay out of
        // negotiation rather than be offered to KMS.
        assert_eq!(PixelFormat::Rgba16Float.fourcc(), None);
    }

    #[test]
    fn srgb_variants_are_distinguished_from_linear_ones() {
        assert!(PixelFormat::Bgra8UnormSrgb.is_srgb());
        assert!(!PixelFormat::Bgra8Unorm.is_srgb());
        // The sRGB transfer is a write-time property, not a storage one.
        assert_eq!(
            PixelFormat::Bgra8UnormSrgb.bytes_per_pixel(),
            PixelFormat::Bgra8Unorm.bytes_per_pixel()
        );
    }

    #[test]
    fn modifier_sentinels_are_distinct() {
        assert!(Modifier::LINEAR.is_linear());
        assert!(Modifier::LINEAR.is_valid());
        assert!(!Modifier::INVALID.is_valid());
        assert_eq!(Modifier::LINEAR.to_string(), "LINEAR");
        assert_eq!(Modifier::INVALID.to_string(), "INVALID");
    }

    #[test]
    fn intersection_prefers_non_linear_layouts() {
        let vendor = Modifier(0x0100_0000_0000_0001);
        let render = FormatModifierSet::new(Fourcc::ARGB8888, vec![Modifier::LINEAR, vendor]);
        let scanout = FormatModifierSet::new(Fourcc::ARGB8888, vec![vendor, Modifier::LINEAR]);

        let common = render.intersect(&scanout).expect("shared modifiers");
        // Linear sorts last so the caller taking the first entry gets the
        // bandwidth-preserving layout.
        assert_eq!(common.modifiers.first(), Some(&vendor));
        assert_eq!(common.modifiers.last(), Some(&Modifier::LINEAR));
    }

    #[test]
    fn intersection_fails_loudly_rather_than_falling_back() {
        let render = FormatModifierSet::new(Fourcc::ARGB8888, vec![Modifier(1)]);
        let scanout = FormatModifierSet::new(Fourcc::ARGB8888, vec![Modifier(2)]);
        // No shared layout is a negotiation failure the caller must report,
        // not an invitation to substitute LINEAR.
        assert_eq!(render.intersect(&scanout), None);

        let other_format = FormatModifierSet::new(Fourcc::XRGB2101010, vec![Modifier(1)]);
        assert_eq!(render.intersect(&other_format), None);
    }

    #[test]
    fn extent_area_does_not_overflow_at_large_sizes() {
        let huge = Extent2D::new(u32::MAX, 2);
        assert_eq!(huge.area(), u32::MAX as u64 * 2);
        assert!(Extent2D::new(0, 1080).is_empty());
    }
}

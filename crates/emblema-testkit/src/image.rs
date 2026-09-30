//! Captured images and how they are compared.
//!
//! Comparison is tolerance-based rather than exact because the specification
//! permits latitude in places: converting a blended result to normalized
//! fixed-point may take either of the two nearest representable values, so two
//! conformant drivers can differ by one unit. A tolerance is a statement about
//! where the specification allows a difference, not a way to make a failing
//! comparison pass.

/// An RGBA8 image read back from a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, premultiplied.
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    pub fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }
}

/// How much difference is acceptable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// Largest acceptable difference in any one channel.
    pub per_channel: u8,
    /// Fraction of pixels allowed to exceed `per_channel`.
    ///
    /// Zero for most comparisons. A budget above zero belongs to scene classes
    /// where a driver's rasterization or filtering is permitted to differ at
    /// edges, and raising one should be as deliberate as changing a golden.
    pub outlier_fraction: f32,
}

impl Tolerance {
    /// Bit-exact. Correct for clears, coverage, and anything else the
    /// specification pins down.
    /// The same profile with `extra` more levels allowed per channel.
    ///
    /// Added to whichever profile a scene already earned rather than replacing
    /// it, because the mechanisms are independent: what a multisampled edge
    /// permits and what a texture filter permits are different things happening
    /// in the same frame. A scene that does both needs both, which is how
    /// `nine-patch-stretched` came to fail on a board -- it earns the multisample
    /// profile, and a term applied only to the per-store one never reached it.
    ///
    /// Saturating, because a `u8` is what the field is and a profile at its
    /// ceiling already permits everything.
    pub const fn widened_by(self, extra: u8) -> Self {
        Self {
            per_channel: self.per_channel.saturating_add(extra),
            outlier_fraction: self.outlier_fraction,
        }
    }

    pub const EXACT: Self = Self {
        per_channel: 0,
        outlier_fraction: 0.0,
    };

    /// One unit per channel, no outliers. Correct where a blended result is
    /// converted to fixed-point.
    pub const ROUNDING: Self = Self {
        per_channel: 1,
        outlier_fraction: 0.0,
    };

    /// Rounding, plus a few pixels allowed to differ by more.
    ///
    /// For multisampling, where the extra latitude is a specific and bounded
    /// thing rather than general slack. Neither graphics specification says
    /// which samples an edge covers when it passes near a sample point, so two
    /// rasterizers may include a different one -- and the resolve then differs
    /// by one sample's share of the total, which at four samples is a quarter
    /// of full scale. That is far too much for the per-channel budget and is
    /// not a defect, so it is bounded by *how many* pixels rather than by how
    /// much.
    ///
    /// A thousandth of the image: sixteen pixels at the corpus's size, against
    /// the two to four that curved edges actually produce. Anything systematic
    /// -- a shape in the wrong place, a color computed differently, a missing
    /// draw -- moves far more of the image than that, so this stays able to
    /// tell a rasterization tie from a divergence.
    /// A few units everywhere, no outliers.
    ///
    /// For coverage computed from a distance field, where the width of the
    /// edge comes from a screen-space derivative. Both specifications leave
    /// those to the implementation -- they may be evaluated once per
    /// two-by-two quad, or by differencing neighbors, and the choice is not
    /// observable except through exactly this. A derivative differing by a
    /// percent moves coverage by a couple of units along the whole edge, which
    /// is small everywhere rather than large somewhere.
    ///
    /// Bounded by magnitude rather than by count, which is the opposite of the
    /// multisample budget and matches the opposite failure: a shape in the
    /// wrong place or the wrong size moves edge pixels by the whole range, so
    /// this stays able to tell arithmetic from a defect.
    ///
    /// Eight rather than four because an outline is a band, and a band's
    /// coverage is the difference of two edges' -- so it inherits both their
    /// errors. Measured on one pair of devices with the same three shapes: a
    /// single edge differs by three, a band around it by seven, and a band with
    /// gentle curvature by one. Four was derived from filled shapes alone and
    /// only ever fitted them.
    pub const ANALYTIC: Self = Self {
        per_channel: 8,
        outlier_fraction: 0.0,
    };

    pub const MULTISAMPLED: Self = Self {
        per_channel: 1,
        outlier_fraction: 0.001,
    };

    /// Rounding, once per draw that can land on the same pixel.
    ///
    /// For a scene whose items blend *additively*. Every other mode here
    /// replaces the destination or mixes toward it, so a fragment's rounding is
    /// the last one that happened and one level covers it. `Plus` adds to what
    /// is already there, so each overlapping draw contributes its own rounding
    /// and they accumulate rather than cancel -- two draws can be two levels
    /// apart between implementations that each round correctly.
    ///
    /// Three because that is what the corpus contains. The additive atlas scene
    /// places its sprites sixteen apart at a width of fifty-six and says so in
    /// its own comment: "the run has single, double and triple coverage in it".
    /// A scene stacking more would want more, and would fail here rather than
    /// pass quietly.
    ///
    /// Found on a Raspberry Pi 5, where the two backends read two levels apart
    /// on that scene. They share a GPU and its fixed-function blending, so the
    /// difference is the two shader compilers arriving at slightly different
    /// arithmetic -- the same thing that has three runtime-effect tests reading
    /// 179 against x86's 178 -- and then the blend adding it up.
    pub const ACCUMULATED: Self = Self {
        per_channel: 3,
        outlier_fraction: 0.0,
    };

    pub const fn new(per_channel: u8, outlier_fraction: f32) -> Self {
        Self {
            per_channel,
            outlier_fraction,
        }
    }
}

/// What a comparison found.
///
/// The per-pixel differences are retained rather than reduced to a count,
/// because how many pixels are "outliers" is not a property of the comparison:
/// it depends on the per-channel bound they are being counted against, and one
/// difference is judged against several. Counting at a fixed threshold here is
/// how [`Tolerance::outlier_fraction`] came to mean something other than what
/// it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    pub max_delta: u8,
    /// Pixels that are not identical, whatever the margin.
    ///
    /// Not the outlier count: a pixel a single level out is counted here and
    /// is within every tolerance but [`Tolerance::EXACT`]. For the count that
    /// a tolerance is judged on, see [`Difference::exceeding`].
    pub differing: usize,
    pub total: usize,
    /// Where the largest difference was, for pointing at the failure.
    pub worst_at: Option<(u32, u32)>,
    /// Largest single-channel difference at each pixel, in row-major order.
    deltas: Vec<u8>,
}

impl Difference {
    /// Fraction of the image that is not identical.
    ///
    /// Answers "how much of this picture moved", which is a question about the
    /// two images. It is not what a tolerance is checked against.
    pub fn fraction_differing(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.differing as f32 / self.total as f32
        }
    }

    /// Pixels differing by more than `per_channel` in some channel.
    ///
    /// The outlier count, and the one [`accepts`] uses: a tolerance says how
    /// many pixels may exceed its per-channel bound, so the bound has to be
    /// known before they can be counted.
    pub fn exceeding(&self, per_channel: u8) -> usize {
        self.deltas.iter().filter(|d| **d > per_channel).count()
    }

    /// [`Difference::exceeding`] as a fraction of the image.
    pub fn fraction_exceeding(&self, per_channel: u8) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.exceeding(per_channel) as f32 / self.total as f32
        }
    }

    /// Why a tolerance rejected this, in the terms the tolerance is written in.
    ///
    /// [`Difference`]'s own `Display` cannot say this: it does not know the
    /// bound, so it reports every pixel that moved, and a reader takes that
    /// percentage for the one that was judged. It is usually far larger --
    /// three per cent of a frame differing by a single level, against a
    /// hundredth of a per cent exceeding the bound -- so the message points at
    /// the wrong number and reads as though a wider budget were needed.
    pub fn describe(&self, tolerance: Tolerance) -> String {
        let over = self.exceeding(tolerance.per_channel);
        format!(
            "{self}; {over} exceed {} ({:.4}%, budget {:.4}%)",
            tolerance.per_channel,
            self.fraction_exceeding(tolerance.per_channel) * 100.0,
            tolerance.outlier_fraction * 100.0
        )
    }

    pub fn is_identical(&self) -> bool {
        self.max_delta == 0
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // "differing" rather than "outside tolerance": this counts every pixel
        // that is not identical, and whether that is acceptable is a separate
        // question the caller answers with a tolerance. Saying otherwise made
        // passing comparisons read like failures.
        write!(
            f,
            "max delta {}, {} of {} pixels differing ({:.4}%)",
            self.max_delta,
            self.differing,
            self.total,
            self.fraction_differing() * 100.0
        )?;
        if let Some((x, y)) = self.worst_at {
            write!(f, ", worst at ({x}, {y})")?;
        }
        Ok(())
    }
}

/// Compare two images.
///
/// Returns the difference either way; the caller decides whether it passes via
/// [`accepts`], so a passing comparison can still report how close it came.
pub fn compare(a: &Image, b: &Image) -> Result<Difference, String> {
    if a.width != b.width || a.height != b.height {
        return Err(format!(
            "size mismatch: {}x{} against {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    if a.pixels.len() != b.pixels.len() {
        return Err("pixel buffers differ in length".into());
    }

    let mut max_delta = 0u8;
    let mut worst_at = None;
    let mut differing = 0usize;
    // Recorded per comparison rather than passed in, so the same difference can
    // be judged against different tolerances without re-scanning. It is kept
    // rather than counted away, which is the point: a count taken here can only
    // be taken at a threshold this function does not know.
    let mut per_pixel_max = vec![0u8; a.pixel_count()];

    for (i, (pa, pb)) in a
        .pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .enumerate()
    {
        let delta = pa
            .iter()
            .zip(pb)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        per_pixel_max[i] = delta;
        if delta > max_delta {
            max_delta = delta;
            worst_at = Some((i as u32 % a.width, i as u32 / a.width));
        }
    }
    for delta in &per_pixel_max {
        if *delta > 0 {
            differing += 1;
        }
    }

    Ok(Difference {
        max_delta,
        differing,
        total: a.pixel_count(),
        worst_at,
        deltas: per_pixel_max,
    })
}

/// Whether a difference is within tolerance.
pub fn accepts(difference: &Difference, tolerance: Tolerance) -> bool {
    if difference.max_delta <= tolerance.per_channel {
        return true;
    }
    difference.fraction_exceeding(tolerance.per_channel) <= tolerance.outlier_fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Image {
        Image::new(
            width,
            height,
            color
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect(),
        )
    }

    #[test]
    fn identical_images_differ_by_nothing() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let d = compare(&a, &a).unwrap();
        assert!(d.is_identical());
        assert_eq!(d.differing, 0);
        assert!(accepts(&d, Tolerance::EXACT));
    }

    /// An image differing from a flat one at `count` pixels, by a whole
    /// sample's worth at four samples.
    fn with_outliers(width: u32, height: u32, count: usize) -> Image {
        let mut image = solid(width, height, [10, 20, 30, 255]);
        for i in 0..count {
            image.pixels[i * 4] = 10u8.wrapping_add(64);
        }
        image
    }

    #[test]
    fn the_multisample_budget_admits_a_tie_and_refuses_a_divergence() {
        // The budget exists for one thing: two rasterizers including different
        // samples where an edge passes near a sample point, which moves a few
        // pixels by a quarter of full scale at four samples. It has to admit
        // that and still refuse anything systematic, or it is general slack
        // rather than a specific allowance.
        let flat = solid(128, 128, [10, 20, 30, 255]);
        let total = 128 * 128;

        // What curved edges actually produce here: single figures.
        let tie = with_outliers(128, 128, 4);
        let difference = compare(&flat, &tie).unwrap();
        assert_eq!(difference.max_delta, 64, "a sample's worth at four samples");
        assert!(
            accepts(&difference, Tolerance::MULTISAMPLED),
            "a handful of edge pixels should be admitted"
        );
        assert!(
            !accepts(&difference, Tolerance::ROUNDING),
            "and should not be, without the multisample budget"
        );

        // One percent of the image, which is far more edge than a corpus scene
        // has and is what a real divergence looks like.
        let divergent = with_outliers(128, 128, total / 100);
        let difference = compare(&flat, &divergent).unwrap();
        assert!(
            !accepts(&difference, Tolerance::MULTISAMPLED),
            "a divergence over one percent of the image was admitted"
        );
    }

    #[test]
    fn a_one_unit_difference_fails_exact_and_passes_rounding() {
        let a = solid(4, 4, [10, 20, 30, 255]);
        let b = solid(4, 4, [10, 21, 30, 255]);
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.max_delta, 1);
        // The distinction the tolerance profiles exist to make.
        assert!(!accepts(&d, Tolerance::EXACT));
        assert!(accepts(&d, Tolerance::ROUNDING));
    }

    #[test]
    fn an_outlier_budget_admits_a_few_bad_pixels_but_not_many() {
        let a = solid(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        // Two pixels of a hundred, well outside per-channel tolerance.
        for i in 0..2 {
            b.pixels[i * 4] = 200;
        }
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.max_delta, 200);
        assert_eq!(d.differing, 2);

        assert!(accepts(&d, Tolerance::new(1, 0.05)));
        assert!(!accepts(&d, Tolerance::new(1, 0.01)));
    }

    /// The distinction the outlier budget is written in terms of, and which it
    /// did not make until this test existed.
    ///
    /// An outlier is a pixel exceeding the per-channel bound. Counting instead
    /// every pixel that differs at all folds ordinary rounding into the budget,
    /// and rounding is exactly what the per-channel bound is there to absorb --
    /// so a picture within tolerance on every pixel but a handful is rejected
    /// for the pixels that were never in question.
    ///
    /// The numbers are a real case: two devices rendering the corpus's
    /// antialiased circle, where four pixels of sixteen thousand land a sample
    /// apart at the edge and nine hundredths of the frame round differently.
    /// The budget is a thousandth, the four are a fortieth of it, and it failed
    /// on the nine hundredths.
    #[test]
    fn rounding_is_not_an_outlier() {
        let a = solid(100, 100, [40, 40, 40, 255]);
        let mut b = a.clone();
        // Nine hundred pixels a single level out: inside the per-channel bound,
        // and nothing a comparison is meant to care about.
        for i in 0..900 {
            b.pixels[i * 4] = 41;
        }
        // Four a whole sample's worth out, which is what the budget is for.
        for i in 900..904 {
            b.pixels[i * 4] = 104;
        }
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.max_delta, 64);
        assert_eq!(d.differing, 904, "every pixel that moved");
        assert_eq!(d.exceeding(1), 4, "only those past the per-channel bound");

        // Four in ten thousand is four ten-thousandths, inside a thousandth.
        assert!(
            accepts(&d, Tolerance::MULTISAMPLED),
            "four outliers against a budget of ten were rejected -- \
             counting the nine hundred rounded pixels as outliers is the only \
             way to reach that"
        );
        // And the budget still discriminates: eleven is past ten.
        for i in 904..911 {
            b.pixels[i * 4] = 104;
        }
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.exceeding(1), 11);
        assert!(
            !accepts(&d, Tolerance::MULTISAMPLED),
            "eleven outliers against a budget of ten were admitted"
        );
    }

    #[test]
    fn the_worst_pixel_is_located() {
        let a = solid(4, 4, [0, 0, 0, 255]);
        let mut b = a.clone();
        // Row 2, column 1.
        b.pixels[(2 * 4 + 1) * 4] = 99;
        let d = compare(&a, &b).unwrap();
        assert_eq!(d.worst_at, Some((1, 2)));
    }

    #[test]
    fn mismatched_sizes_are_an_error_rather_than_a_difference() {
        // Comparing images of different sizes means the harness is confused
        // about what it rendered, which is not a tolerance question.
        let a = solid(4, 4, [0; 4]);
        let b = solid(8, 4, [0; 4]);
        assert!(compare(&a, &b).is_err());
    }
}

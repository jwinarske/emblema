//! Baking a gradient's stops into a texture the shader can sample.
//!
//! # Why a texture at all
//!
//! The material carries a fixed, small number of stops in push constants, which
//! covers the overwhelming majority of real gradients and nothing beyond them.
//! A gradient with more used to be truncated, silently; then it was refused,
//! honestly. This is the third answer: evaluate the ramp once on the way in and
//! hand the shader an image of it, so the count stops mattering.
//!
//! # Why half-floats, and why straight alpha
//!
//! The table is stored the way the walk produced it: sRGB-encoded like
//! everything else the pipeline carries, unclamped, four half-floats a texel.
//!
//! Half rather than the eight bits upstream's gradient texture uses, and the
//! reason is range rather than precision. An eight-bit table cannot hold a
//! component outside the sRGB primaries at all, and a wide-gamut gradient has
//! them. Upstream's table cannot carry one either; it reaches a table so rarely
//! -- past two hundred and fifty-six stops -- that it has not had to.
//!
//! The color is stored straight rather than premultiplied, because the shader
//! premultiplies stops read from push constants at the very end. A
//! premultiplied table would fork the two paths at exactly the point this
//! design exists to join them.

use crate::paint::GradientStop;

/// How many texels a baked ramp gets.
///
/// A gradient is a one-dimensional function sampled with a linear filter, so
/// the question is how finely it has to be tabulated before interpolation
/// between neighbors is indistinguishable from evaluating it. Between two stops
/// the function is a lerp and the filter is a lerp, so the table is exact
/// there however wide it is; what the width actually bounds is how precisely a
/// stop lands that falls between two texel centers. At this width that is under
/// half a percent of the ramp's length, which is finer than the four-stop path
/// it has to agree with can be told apart from.
pub const RAMP_WIDTH: usize = 256;

/// A gradient's colors, tabulated and encoded for upload.
///
/// Owned by the recording rather than by the caller, because the recorder is
/// what knows the stops and nothing outside it should have to bake them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ramp {
    /// `RAMP_WIDTH` texels, RGBA, linear, four half-floats each.
    pub texels: Vec<u8>,
}

impl Ramp {
    /// Tabulate `stops` across the width.
    ///
    /// The stops are taken as given: in order, and covering whatever part of
    /// the range they cover. Before the first and after the last the ramp holds
    /// the end colors, which is not a tile mode — tiling happens to the
    /// parameter before it reaches the ramp, so by here the question is only
    /// what a parameter inside the table means.
    pub fn bake(stops: &[GradientStop]) -> Self {
        let mut texels = vec![0u8; RAMP_WIDTH * 8];
        for (i, texel) in texels.chunks_exact_mut(8).enumerate() {
            // Sampled at texel centers, because that is where a linear filter
            // reads them: treating the first texel as t=0 would shift the whole
            // ramp by half a texel against the four-stop path, which is exactly
            // the kind of difference that shows up as a seam between a gradient
            // and one drawn beside it with fewer stops.
            let t = (i as f32 + 0.5) / RAMP_WIDTH as f32;
            let color = sample_at(stops, t);
            // Linear, and stored as it is. Little-endian because the device
            // reads native order and every target here is little-endian.
            for (channel, out) in color.to_array().iter().zip(texel.chunks_exact_mut(2)) {
                out.copy_from_slice(&half::f16::from_f32(*channel).to_le_bytes());
            }
        }
        Self { texels }
    }
}

/// The color a gradient shows at `t`, from stops in order.
///
/// Deliberately the same walk the shader performs over its four stops, so that
/// a gradient stated within the material's four and the same gradient restated
/// with more do not differ. Sharing the rule is half of what makes that true;
/// the other half is that the table holds what the walk produced rather than a
/// rounded, clamped version of it, which is why it is linear half-floats.
///
/// It used to be neither. The table was encoded and quantized to eight bits,
/// so a component the sRGB primaries cannot hold was flattened on the way in --
/// and adding a stop that changed nothing about a gradient changed the picture
/// by twenty-four levels once a color filter brought the difference back inside
/// the range a target could show.
fn sample_at(stops: &[GradientStop], t: f32) -> crate::Color {
    let Some(first) = stops.first() else {
        return crate::Color::TRANSPARENT;
    };
    let mut result = first.color;
    for pair in stops.windows(2) {
        let (lower, upper) = (&pair[0], &pair[1]);
        let span = (upper.offset - lower.offset).max(1e-6);
        let local = ((t - lower.offset) / span).clamp(0.0, 1.0);
        if t >= lower.offset {
            result = mix(lower.color, upper.color, local);
        }
    }
    result
}

/// Interpolate two stops, in one space rather than component by component.
///
/// The two colors need not have been stated against the same primaries, and
/// mixing their components directly would then average numbers that do not
/// describe the same axes -- an answer that is subtly wrong rather than
/// obviously so, and one whose error grows with how far apart the two spaces
/// are. `to_array` puts both against the primaries the pipeline works in, which
/// is the same thing every other reader of a color does.
/// Blend two stops, in the space the shader blends them in.
///
/// `to_array` hands back encoded components and `Color::srgb` takes encoded
/// components, so the pair is a decompose and a recompose with nothing applied
/// between them. Rebuilding with `Color::linear` would encode a second time,
/// which is what it did for exactly as long as `to_array` returned light: the
/// walked and tabulated halves of a gradient then disagreed by fifty-one
/// levels, on the invariant the shared walk exists to provide.
fn mix(a: crate::Color, b: crate::Color, t: f32) -> crate::Color {
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, ba] = b.to_array();
    crate::Color::srgb(
        ar + (br - ar) * t,
        ag + (bg - ag) * t,
        ab + (bb - ab) * t,
        aa + (ba - aa) * t,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    fn stops(n: usize) -> Vec<GradientStop> {
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                GradientStop::new(Color::srgb(t, 0.0, 1.0 - t, 1.0), t)
            })
            .collect()
    }

    /// One texel, decoded back to the linear components it was baked from.
    fn texel(ramp: &Ramp, i: usize) -> [f32; 4] {
        let at = i * 8;
        let mut out = [0.0; 4];
        for (channel, slot) in out.iter_mut().enumerate() {
            let byte = at + channel * 2;
            *slot = half::f16::from_le_bytes([ramp.texels[byte], ramp.texels[byte + 1]]).to_f32();
        }
        out
    }

    #[test]
    fn a_ramp_is_the_full_width_and_fully_written() {
        let ramp = Ramp::bake(&stops(6));
        assert_eq!(ramp.texels.len(), RAMP_WIDTH * 8);
        // Every alpha is opaque here, so a texel nothing wrote would show as a
        // hole rather than blend in with its neighbors.
        assert!(
            (0..RAMP_WIDTH).all(|i| texel(&ramp, i)[3] == 1.0),
            "a texel was left unwritten"
        );
    }

    #[test]
    fn the_ends_hold_the_first_and_last_stop() {
        let ramp = Ramp::bake(&stops(6));
        let first = texel(&ramp, 0);
        let last = texel(&ramp, RAMP_WIDTH - 1);
        // Not exactly the stop colors: the first texel's center is half a texel
        // in, so it has already traveled that far along the ramp. Close, and
        // unmistakably at the right end of it.
        assert!(first[2] > 0.94 && first[0] < 0.06, "first texel {first:?}");
        assert!(last[0] > 0.94 && last[2] < 0.06, "last texel {last:?}");
    }

    #[test]
    fn every_stop_appears_where_it_was_placed() {
        // Three stops at a quarter, half and three quarters, each a channel of
        // its own, so a ramp that placed them evenly instead of where they were
        // asked for would put the wrong color at each probe.
        let stops = vec![
            GradientStop::new(Color::linear(1.0, 0.0, 0.0, 1.0), 0.25),
            GradientStop::new(Color::linear(0.0, 1.0, 0.0, 1.0), 0.5),
            GradientStop::new(Color::linear(0.0, 0.0, 1.0, 1.0), 0.75),
        ];
        let ramp = Ramp::bake(&stops);
        let at = |t: f32| {
            texel(
                &ramp,
                ((t * RAMP_WIDTH as f32) as usize).min(RAMP_WIDTH - 1),
            )
        };
        assert!(at(0.25)[0] > 0.78, "the red stop is not at a quarter");
        assert!(at(0.5)[1] > 0.78, "the green stop is not at the half");
        assert!(at(0.75)[2] > 0.78, "the blue stop is not at three quarters");
        // And before the first stop it holds that stop rather than fading in
        // from nothing, which is what clamping means inside the table.
        assert!(
            at(0.05)[0] > 0.78,
            "the ramp faded in before its first stop"
        );
    }

    #[test]
    fn the_ramp_is_monotonic_where_the_stops_are() {
        // Two stops from black to white. Every texel must be at least as bright
        // as the one before it: a ramp that wrapped, reversed, or sampled out
        // of order would break this without necessarily looking wrong at any
        // single probe.
        let ramp = Ramp::bake(&[
            GradientStop::new(Color::linear(0.0, 0.0, 0.0, 1.0), 0.0),
            GradientStop::new(Color::linear(1.0, 1.0, 1.0, 1.0), 1.0),
        ]);
        let mut previous = 0.0;
        for i in 0..RAMP_WIDTH {
            let value = texel(&ramp, i)[0];
            assert!(
                value >= previous,
                "texel {i} is {value} after {previous}, so the ramp goes backwards"
            );
            previous = value;
        }
        assert!(previous > 0.98, "the ramp never reached white");
    }

    #[test]
    fn the_table_holds_what_the_walk_produced() {
        // In and out unchanged, with no eight-bit step in between: the table
        // stores the color rather than a rounding of it. Both channels come
        // back at a half because neither was transformed -- which is what makes
        // the color one stated in the space the table holds, since a color
        // stated in light would be encoded on the way in and would not.
        let ramp = Ramp::bake(&[
            GradientStop::new(Color::srgb(0.5, 0.5, 0.5, 0.5), 0.0),
            GradientStop::new(Color::srgb(0.5, 0.5, 0.5, 0.5), 1.0),
        ]);
        let t = texel(&ramp, RAMP_WIDTH / 2);
        assert!((t[0] - 0.5).abs() < 1e-3, "color came back {}", t[0]);
        assert!((t[3] - 0.5).abs() < 1e-3, "alpha came back {}", t[3]);
    }

    /// The property the table could not hold when it was eight bits through a
    /// transfer function, and the reason a gradient of five stops disagreed
    /// with the same gradient stated in four.
    #[test]
    fn a_stop_outside_the_srgb_primaries_survives_being_tabulated() {
        let ramp = Ramp::bake(&[
            GradientStop::new(Color::srgb(1.2, -0.3, 0.0, 1.0), 0.0),
            GradientStop::new(Color::srgb(1.2, -0.3, 0.0, 1.0), 1.0),
        ]);
        let t = texel(&ramp, RAMP_WIDTH / 2);
        assert!((t[0] - 1.2).abs() < 1e-2, "above one became {}", t[0]);
        assert!((t[1] + 0.3).abs() < 1e-2, "below zero became {}", t[1]);
    }

    #[test]
    fn no_stops_bakes_to_nothing_rather_than_to_a_panic() {
        // Reachable only through a paint that carries no stops, which does not
        // draw -- but a bake that indexed into an empty slice would take the
        // process down on the way to finding that out.
        let ramp = Ramp::bake(&[]);
        assert_eq!(ramp.texels.len(), RAMP_WIDTH * 8);
        assert!(ramp.texels.iter().all(|b| *b == 0));
    }
}

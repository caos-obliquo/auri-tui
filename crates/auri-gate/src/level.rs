//! Quantized-section helpers and magnitude in dB, shared with the gate.
//!
//! Port of `src/dsp/level.mjs`: same arithmetic, same order of summations, so
//! every value matches the JavaScript reference bit for bit (both are IEEE-754
//! doubles evaluated in the same expression order).

use std::sync::OnceLock;

use auri_codec::{
    Band, Q23_ONE, Q27_ONE, SAMPLE_RATE, Section, design_section, encode_section, preamp_word,
};

/// The official 280-point log grid (20 Hz–20 kHz, rounded to 0.01 Hz) filtered
/// to 300–3000 Hz: 93 frequencies.
///
/// Computed once, bit-identical to `Math.round(20 * 1000 ** (i / 279) * 100) / 100`
/// in the JavaScript reference (`f64::round` agrees with `Math.round` here: the
/// values are positive and no grid point lands exactly on a half-cent).
#[must_use]
pub fn mean_frequencies() -> &'static [f64] {
    static POINTS: OnceLock<Vec<f64>> = OnceLock::new();
    POINTS.get_or_init(|| {
        (0..280)
            .map(|i| (20.0 * 1000_f64.powf(f64::from(i) / 279.0) * 100.0).round() / 100.0)
            .filter(|f| (300.0..=3000.0).contains(f))
            .collect()
    })
}

/// The Q27-quantized section of one band, as the chip sees it: the design
/// section quantized onto the five wire words.
///
/// # Errors
///
/// Whatever the codec refuses for this band (design or quantization).
pub fn quantized_section(band: Band) -> Result<Section, auri_codec::CodecError> {
    Ok(from_words(encode_section(design_section(band)?, 0)?))
}

/// The effective level (dB) of `bands` with `preamp_db`: the Q23 preamp plus
/// the mean, over [`mean_frequencies`], of the summed Q27 section gains.
///
/// This is the number the UI calls "predicted level 300 Hz–3 kHz". It is not
/// LUFS, ISO 226 or a loudness prediction — just the curve's cascade gain the
/// way the chip computes it. Same arithmetic and summation order as the
/// JavaScript `levelDb`, so parity tests compare at ~1e-9 relative.
///
/// # Errors
///
/// [`auri_codec::CodecError`] for bands or a preamp the codec cannot encode.
pub fn level_db(bands: &[Band], preamp_db: f64) -> Result<f64, auri_codec::CodecError> {
    let sections = bands
        .iter()
        .map(|band| quantized_section(*band))
        .collect::<Result<Vec<_>, _>>()?;
    let preamp = quantized_preamp_db(preamp_db)?;
    let frequencies = mean_frequencies();
    let mut total = 0.0;
    for &f in frequencies {
        let mut sum = 0.0;
        for section in &sections {
            sum += section_db(*section, f);
        }
        total += sum;
    }
    Ok(preamp + total / f64::from(u32::try_from(frequencies.len()).expect("93 fits in u32")))
}

/// Section coefficients as the chip sees them, from the 5 wire words
/// `[b0, b2, -a1, b1, -a2]` (Q27).
#[must_use]
pub fn from_words(words: [i32; 5]) -> Section {
    Section {
        b0: f64::from(words[0]) / Q27_ONE,
        b2: f64::from(words[1]) / Q27_ONE,
        a1: -f64::from(words[2]) / Q27_ONE,
        b1: f64::from(words[3]) / Q27_ONE,
        a2: -f64::from(words[4]) / Q27_ONE,
    }
}

/// True for the exact pass-through section (OFF, or any section whose
/// quantized numerator equals its denominator).
#[must_use]
pub fn is_identity_section(s: Section) -> bool {
    s.b0 == 1.0 && s.b1 == s.a1 && s.b2 == s.a2
}

/// Magnitude (dB) of one section at `f` Hz, evaluated around `z^-1 = 1`.
/// Exact identities return 0.
#[must_use]
pub fn section_db(s: Section, f: f64) -> f64 {
    if is_identity_section(s) {
        return 0.0;
    }
    let w = (2.0 * std::f64::consts::PI * f) / f64::from(SAMPLE_RATE);
    let re = 2.0 * (w / 2.0).sin().powi(2);
    let im = w.sin();
    section_db_from_terms(s, re, im, re * re - im * im, 2.0 * re * im)
}

/// [`section_db`] from precomputed frequency terms (`u = 1 - e^-jw = re + j·im`
/// and `u² = u2re + j·u2im`), for callers scanning a fixed grid many times.
/// Same arithmetic as [`section_db`], so results are bit-identical.
#[must_use]
pub fn section_db_from_terms(s: Section, re: f64, im: f64, u2re: f64, u2im: f64) -> f64 {
    if is_identity_section(s) {
        return 0.0;
    }
    let power = |p0: f64, p1: f64, p2: f64| {
        let r = p0 + p1 + p2 - (p1 + 2.0 * p2) * re + p2 * u2re;
        let i = -(p1 + 2.0 * p2) * im + p2 * u2im;
        r * r + i * i
    };
    10.0 * (power(s.b0, s.b1, s.b2) / power(1.0, s.a1, s.a2)).log10()
}

/// The preamp actually applied by the chip (Q23 word), in dB.
///
/// # Errors
///
/// [`auri_codec::CodecError::Preamp`] when the value is not finite or is
/// positive — the same refusal as the codec.
pub fn quantized_preamp_db(preamp_db: f64) -> Result<f64, auri_codec::CodecError> {
    let word = preamp_word(preamp_db)?;
    Ok(20.0 * (f64::from(word) / Q23_ONE).log10())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auri_codec::NEUTRAL_BANDS;

    #[test]
    fn mean_frequencies_are_the_93_official_points() {
        let points = mean_frequencies();
        assert_eq!(
            points.len(),
            93,
            "the 280-point grid has 93 in 300 Hz–3 kHz"
        );
        assert!(points.iter().all(|f| (300.0..=3000.0).contains(f)));
        assert!(points.windows(2).all(|w| w[1] > w[0]), "strictly rising");
        // The first and last points, pinned to the reference's rounding.
        assert_eq!(points[0], 304.67);
        assert_eq!(points[points.len() - 1], 2972.17);
    }

    #[test]
    fn the_neutral_curve_level_is_the_quantized_preamp() {
        // Every section is the identity, so every section_db is 0 and the
        // mean is 0: only the preamp remains.
        let level = level_db(&NEUTRAL_BANDS, -0.5).expect("neutral encodes");
        let preamp = quantized_preamp_db(-0.5).expect("preamp encodes");
        assert!((level - preamp).abs() < 1e-12, "{level} vs {preamp}");
        assert!(
            (level - (-0.5)).abs() < 0.01,
            "Q23 keeps it within 0.01 dB of the requested preamp: {level}"
        );
    }

    #[test]
    fn a_codec_refusal_propagates() {
        assert!(level_db(&NEUTRAL_BANDS, 1.0).is_err(), "positive preamp");
        let short = [NEUTRAL_BANDS[0]; 3];
        // Any band count is accepted by level_db itself (it is a measurement,
        // not a gate); an invalid band is what must fail.
        assert!(level_db(&short, -0.5).is_ok());
        let bad = [Band::Pk {
            freq_hz: 1_000.0,
            gain_db: 0.0,
            q_factor: 0.0,
        }];
        assert!(level_db(&bad, -0.5).is_err(), "qFactor 0 is refused");
    }

    #[test]
    fn a_cut_lowers_the_level_and_a_boost_raises_it() {
        let mut cut = NEUTRAL_BANDS;
        cut[0] = Band::Pk {
            freq_hz: 1_000.0,
            gain_db: -6.0,
            q_factor: 1.0,
        };
        let mut boost = NEUTRAL_BANDS;
        boost[0] = Band::Pk {
            freq_hz: 1_000.0,
            gain_db: 3.0,
            q_factor: 1.0,
        };
        let neutral = level_db(&NEUTRAL_BANDS, -0.5).expect("neutral");
        let cut_level = level_db(&cut, -0.5).expect("cut");
        let boost_level = level_db(&boost, -0.5).expect("boost");
        assert!(cut_level < neutral, "{cut_level} < {neutral}");
        assert!(boost_level > neutral, "{boost_level} > {neutral}");
    }
}

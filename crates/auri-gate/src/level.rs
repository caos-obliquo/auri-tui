//! Quantized-section helpers and magnitude in dB, shared with the gate.
//!
//! Port of `src/dsp/level.mjs`: same arithmetic, same order of summations, so
//! every value matches the JavaScript reference bit for bit (both are IEEE-754
//! doubles evaluated in the same expression order).

use auri_codec::{Q23_ONE, Q27_ONE, SAMPLE_RATE, Section, preamp_word};

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

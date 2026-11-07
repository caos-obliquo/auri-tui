//! Quantization of floating-point coefficients onto the wire's fixed-point
//! words, and the stability test that goes with it.

use crate::design::Section;
use crate::error::CodecError;

/// One in Q23 (the preamp's format).
pub const Q23_ONE: f64 = 8_388_608.0; // 2^23

/// One in Q27 (the sections' format).
pub const Q27_ONE: f64 = 134_217_728.0; // 2^27

const INT32_MIN: i64 = -2_147_483_648;
const INT32_MAX: i64 = 2_147_483_647;

/// `Math.round` semantics: closest value, ties toward `+infinity`.
///
/// `f64::round` breaks ties *away* from zero, which differs for negative
/// half-integers — and the wire bytes depend on it.
#[inline]
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// Jury criterion for `1 + a1 z^-1 + a2 z^-2`: both poles strictly inside the
/// unit circle.
#[must_use]
pub fn is_stable(a1: f64, a2: f64) -> bool {
    a2.abs() < 1.0 && a1.abs() < 1.0 + a2
}

/// Quantizes one section to its five wire words `[b0, b2, -a1, b1, -a2]` (Q27).
///
/// Stability is checked on the *quantized* feedback words, which is what the
/// chip will actually run.
///
/// # Errors
///
/// [`CodecError::CoefficientOutOfRange`] when a word does not fit `i32`, and
/// [`CodecError::Unstable`] when the quantized poles leave the unit circle.
pub fn encode_section(section: Section, index: usize) -> Result<[i32; 5], CodecError> {
    let ordered = [
        section.b0,
        section.b2,
        -section.a1,
        section.b1,
        -section.a2,
    ];
    let mut words = [0_i32; 5];
    for (slot, &coefficient) in ordered.iter().enumerate() {
        // `+ 0.0` turns -0.0 (from negating a zero a1/a2) into +0.0.
        let word = js_round(coefficient * Q27_ONE) + 0.0;
        if !word.is_finite() {
            return Err(CodecError::CoefficientOutOfRange { index });
        }
        let word = word as i64;
        if !(INT32_MIN..=INT32_MAX).contains(&word) {
            return Err(CodecError::CoefficientOutOfRange { index });
        }
        words[slot] = word as i32;
    }
    // words[2] is -a1, words[4] is -a2: negate them back for the criterion.
    if !is_stable(f64::from(-words[2]) / Q27_ONE, f64::from(-words[4]) / Q27_ONE) {
        return Err(CodecError::Unstable { index });
    }
    Ok(words)
}

/// Q23 linear-gain word for a preamp in dB.
///
/// The preamp runs before the bands, so a positive value clips full-scale
/// content: it is refused.
///
/// # Errors
///
/// [`CodecError::Preamp`] when the value is not finite or is positive.
pub fn preamp_word(preamp_db: f64) -> Result<i32, CodecError> {
    if !preamp_db.is_finite() {
        return Err(CodecError::Preamp {
            got: format!("a non-finite dB value ({preamp_db})"),
        });
    }
    if preamp_db > 0.0 {
        return Err(CodecError::Preamp {
            got: format!("{preamp_db}"),
        });
    }
    let word = js_round(10.0_f64.powf(preamp_db / 20.0) * Q23_ONE) + 0.0;
    let word = word as i64;
    if !(INT32_MIN..=INT32_MAX).contains(&word) {
        return Err(CodecError::Int32OutOfRange {
            got: format!("{word}"),
        });
    }
    Ok(word as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_ties_toward_positive_infinity() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-0.5), 0.0);
        assert_eq!(js_round(-1.5), -1.0);
        assert_eq!(js_round(-2.5), -2.0);
    }

    #[test]
    fn identity_is_stable() {
        assert!(is_stable(0.0, 0.0));
    }

    #[test]
    fn off_section_quantizes_to_identity_words() {
        // 1.0 in Q27 is 0x08000000, not 1: the wire stores fixed point.
        let words = encode_section(Section::identity(), 0).unwrap();
        assert_eq!(words, [1 << 27, 0, 0, 0, 0]);
    }

    #[test]
    fn zero_db_preamp_is_one_in_q23() {
        assert_eq!(preamp_word(0.0).unwrap(), 1 << 23);
        assert_eq!(preamp_word(-6.0206).unwrap(), 4_194_304);
    }

    #[test]
    fn positive_preamp_is_refused() {
        assert!(preamp_word(0.1).is_err());
        assert!(preamp_word(f64::NAN).is_err());
        assert!(preamp_word(f64::INFINITY).is_err());
    }
}

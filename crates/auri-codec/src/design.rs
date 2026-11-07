//! Biquad design.
//!
//! Every design function returns coefficients normalized by `a0`, with the
//! cookbook sign convention:
//!
//! ```text
//! H(z) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)
//! ```
//!
//! Sources:
//!
//! - Peaking, low-shelf and high-shelf are ported from AutoEq
//!   (`autoeq/peq.py`: `Peaking/LowShelf/HighShelf.biquad_coefficients`),
//!   Copyright (c) 2018-2022 Jaakko Pasanen, MIT. AutoEq returns the feedback
//!   coefficients *negated*; they are flipped once, here, and the arithmetic
//!   keeps AutoEq's order.
//! - Low and high pass follow Robert Bristow-Johnson's Audio EQ Cookbook
//!   directly (AutoEq has no such filters).

use crate::SAMPLE_RATE;
use crate::band::{BAND_COUNT, Band};
use crate::error::CodecError;

/// One normalized biquad section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    /// Numerator, constant term.
    pub b0: f64,
    /// Numerator, `z^-1` term.
    pub b1: f64,
    /// Numerator, `z^-2` term.
    pub b2: f64,
    /// Denominator, `z^-1` term (normalized by `a0`).
    pub a1: f64,
    /// Denominator, `z^-2` term (normalized by `a0`).
    pub a2: f64,
}

impl Section {
    /// The exact pass-through section behind [`crate::OFF`].
    pub const fn identity() -> Self {
        Section {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

/// Peaking EQ — AutoEq `Peaking.biquad_coefficients`.
#[must_use]
pub fn design_peaking(freq_hz: f64, gain_db: f64, q_factor: f64) -> Section {
    design_peaking_at(freq_hz, gain_db, q_factor, f64::from(SAMPLE_RATE))
}

/// Peaking EQ at an explicit sample rate.
#[must_use]
pub fn design_peaking_at(freq_hz: f64, gain_db: f64, q_factor: f64, sample_rate: f64) -> Section {
    let amp = 10.0_f64.powf(gain_db / 40.0);
    let w0 = (2.0 * std::f64::consts::PI * freq_hz) / sample_rate;
    let alpha = w0.sin() / (2.0 * q_factor);
    let cos_w0 = w0.cos();

    let a0 = 1.0 + alpha / amp;
    // AutoEq's sign convention, flipped once: negation is exact in IEEE 754.
    let a1_auto_eq = -(-2.0 * cos_w0) / a0;
    let a2_auto_eq = -(1.0 - alpha / amp) / a0;
    Section {
        b0: (1.0 + alpha * amp) / a0,
        b1: (-2.0 * cos_w0) / a0,
        b2: (1.0 - alpha * amp) / a0,
        a1: -a1_auto_eq,
        a2: -a2_auto_eq,
    }
}

/// Low shelf — AutoEq `LowShelf.biquad_coefficients`.
#[must_use]
pub fn design_low_shelf(freq_hz: f64, gain_db: f64, q_factor: f64) -> Section {
    design_low_shelf_at(freq_hz, gain_db, q_factor, f64::from(SAMPLE_RATE))
}

/// Low shelf at an explicit sample rate.
#[must_use]
pub fn design_low_shelf_at(freq_hz: f64, gain_db: f64, q_factor: f64, sample_rate: f64) -> Section {
    let amp = 10.0_f64.powf(gain_db / 40.0);
    let w0 = (2.0 * std::f64::consts::PI * freq_hz) / sample_rate;
    let alpha = w0.sin() / (2.0 * q_factor);
    let cos_w0 = w0.cos();
    let two_sqrt_amp_alpha = 2.0 * amp.sqrt() * alpha;

    let a0 = amp + 1.0 + (amp - 1.0) * cos_w0 + two_sqrt_amp_alpha;
    let a1_auto_eq = -(-2.0 * (amp - 1.0 + (amp + 1.0) * cos_w0)) / a0;
    let a2_auto_eq = -(amp + 1.0 + (amp - 1.0) * cos_w0 - two_sqrt_amp_alpha) / a0;
    Section {
        b0: (amp * (amp + 1.0 - (amp - 1.0) * cos_w0 + two_sqrt_amp_alpha)) / a0,
        b1: (2.0 * amp * (amp - 1.0 - (amp + 1.0) * cos_w0)) / a0,
        b2: (amp * (amp + 1.0 - (amp - 1.0) * cos_w0 - two_sqrt_amp_alpha)) / a0,
        a1: -a1_auto_eq,
        a2: -a2_auto_eq,
    }
}

/// High shelf — AutoEq `HighShelf.biquad_coefficients`.
#[must_use]
pub fn design_high_shelf(freq_hz: f64, gain_db: f64, q_factor: f64) -> Section {
    design_high_shelf_at(freq_hz, gain_db, q_factor, f64::from(SAMPLE_RATE))
}

/// High shelf at an explicit sample rate.
#[must_use]
pub fn design_high_shelf_at(
    freq_hz: f64,
    gain_db: f64,
    q_factor: f64,
    sample_rate: f64,
) -> Section {
    let amp = 10.0_f64.powf(gain_db / 40.0);
    let w0 = (2.0 * std::f64::consts::PI * freq_hz) / sample_rate;
    let alpha = w0.sin() / (2.0 * q_factor);
    let cos_w0 = w0.cos();
    let two_sqrt_amp_alpha = 2.0 * amp.sqrt() * alpha;

    let a0 = amp + 1.0 - (amp - 1.0) * cos_w0 + two_sqrt_amp_alpha;
    let a1_auto_eq = -(2.0 * (amp - 1.0 - (amp + 1.0) * cos_w0)) / a0;
    let a2_auto_eq = -(amp + 1.0 - (amp - 1.0) * cos_w0 - two_sqrt_amp_alpha) / a0;
    Section {
        b0: (amp * (amp + 1.0 + (amp - 1.0) * cos_w0 + two_sqrt_amp_alpha)) / a0,
        b1: (-2.0 * amp * (amp - 1.0 + (amp + 1.0) * cos_w0)) / a0,
        b2: (amp * (amp + 1.0 + (amp - 1.0) * cos_w0 - two_sqrt_amp_alpha)) / a0,
        a1: -a1_auto_eq,
        a2: -a2_auto_eq,
    }
}

/// 2nd-order low pass — RBJ Audio EQ Cookbook.
#[must_use]
pub fn design_low_pass(freq_hz: f64, q_factor: f64) -> Section {
    design_low_pass_at(freq_hz, q_factor, f64::from(SAMPLE_RATE))
}

/// Low pass at an explicit sample rate.
#[must_use]
pub fn design_low_pass_at(freq_hz: f64, q_factor: f64, sample_rate: f64) -> Section {
    let w0 = (2.0 * std::f64::consts::PI * freq_hz) / sample_rate;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q_factor);
    let a0 = 1.0 + alpha;
    Section {
        b0: (1.0 - cos_w0) / 2.0 / a0,
        b1: (1.0 - cos_w0) / a0,
        b2: (1.0 - cos_w0) / 2.0 / a0,
        a1: (-2.0 * cos_w0) / a0,
        a2: (1.0 - alpha) / a0,
    }
}

/// 2nd-order high pass — RBJ Audio EQ Cookbook.
#[must_use]
pub fn design_high_pass(freq_hz: f64, q_factor: f64) -> Section {
    design_high_pass_at(freq_hz, q_factor, f64::from(SAMPLE_RATE))
}

/// High pass at an explicit sample rate.
#[must_use]
pub fn design_high_pass_at(freq_hz: f64, q_factor: f64, sample_rate: f64) -> Section {
    let w0 = (2.0 * std::f64::consts::PI * freq_hz) / sample_rate;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q_factor);
    let a0 = 1.0 + alpha;
    Section {
        b0: (1.0 + cos_w0) / 2.0 / a0,
        b1: -(1.0 + cos_w0) / a0,
        b2: (1.0 + cos_w0) / 2.0 / a0,
        a1: (-2.0 * cos_w0) / a0,
        a2: (1.0 - alpha) / a0,
    }
}

/// Section coefficients for one band.
///
/// Unknown kinds are impossible in Rust (the enum is closed); codes such as
/// `LSQ`/`HSQ` must be mapped explicitly by the caller, never guessed.
///
/// # Errors
///
/// Never, for a valid [`Band`]. The signature exists so callers can treat all
/// design entry points uniformly.
pub fn design_section(band: Band) -> Result<Section, CodecError> {
    Ok(match band {
        Band::Pk {
            freq_hz,
            gain_db,
            q_factor,
        } => design_peaking(freq_hz, gain_db, q_factor),
        Band::Ls {
            freq_hz,
            gain_db,
            q_factor,
        } => design_low_shelf(freq_hz, gain_db, q_factor),
        Band::Hs {
            freq_hz,
            gain_db,
            q_factor,
        } => design_high_shelf(freq_hz, gain_db, q_factor),
        Band::Lp { freq_hz, q_factor } => design_low_pass(freq_hz, q_factor),
        Band::Hp { freq_hz, q_factor } => design_high_pass(freq_hz, q_factor),
        Band::Off => Section::identity(),
    })
}

/// Validates a band against the ranges the firmware can actually run.
///
/// `freq_hz` must be in `(0, SAMPLE_RATE/2)`, `q_factor` finite and `> 0`, and
/// `gain_db` finite for the shapes that take one. `LP`/`HP` take no gain.
///
/// # Errors
///
/// [`CodecError::Band`] describing the offending field.
pub fn check_band(band: Band, index: usize) -> Result<(), CodecError> {
    let range = |message: String| CodecError::Band { index, message };

    let freq_hz = match band.freq_hz() {
        Some(value) => value,
        None => return Ok(()), // OFF carries no fields.
    };
    let q_factor = match band.q_factor() {
        Some(value) => value,
        None => return Err(range("missing qFactor".to_string())),
    };

    let nyquist = f64::from(SAMPLE_RATE) / 2.0;
    // NaN fails both comparisons and +/-Infinity fails one.
    if !(freq_hz > 0.0 && freq_hz < nyquist) {
        return Err(range(format!(
            "freqHz must be in (0, {nyquist}), got {freq_hz}"
        )));
    }
    if !(q_factor.is_finite() && q_factor > 0.0) {
        return Err(range(format!("qFactor must be > 0, got {q_factor}")));
    }
    if band.kind().has_gain() && !band.gain_db().is_finite() {
        return Err(range(format!(
            "gainDb must be finite, got {}",
            band.gain_db()
        )));
    }
    Ok(())
}

/// Validates a whole curve: exactly [`BAND_COUNT`] bands, each one sound.
///
/// # Errors
///
/// [`CodecError::WrongBandCount`] or the first [`CodecError::Band`].
pub fn check_bands(bands: &[Band]) -> Result<(), CodecError> {
    if bands.len() != BAND_COUNT {
        return Err(CodecError::WrongBandCount {
            expected: BAND_COUNT,
        });
    }
    for (index, band) in bands.iter().enumerate() {
        check_band(*band, index)?;
    }
    Ok(())
}

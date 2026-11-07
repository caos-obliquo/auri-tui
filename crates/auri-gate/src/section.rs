//! Per-section analysis: the quantized words, the pole radius and the analytic
//! peaks of one biquad.
//!
//! Port of the analysis helpers of `src/safety/gate.mjs`.

use auri_codec::{Section, encode_section};

use crate::level::{from_words, section_db};
use crate::reason::{Reason, ReasonCode};
use crate::thresholds;

/// A peak of a magnitude response: where and how high.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// Gain at the peak (dB).
    pub db: f64,
    /// Frequency of the peak (Hz).
    pub frequency_hz: f64,
}

/// A prefix peak: the peak of the first `stage` sections plus the preamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrefixPeak {
    /// Number of sections in the prefix (0 = the preamp alone).
    pub stage: u32,
    /// Gain at the peak, preamp included (dB).
    pub db: f64,
    /// Frequency of the peak (Hz), `None` for the preamp alone.
    pub frequency_hz: Option<f64>,
}

/// The per-section checks (a, b, c) on the quantized section of a band, with
/// its analytic peaks.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionAnalysis {
    /// The 5 wire words `[b0, b2, -a1, b1, -a2]` (Q27).
    pub words: [i32; 5],
    /// The section as the chip computes it from those words.
    pub section: Section,
    /// Pole radius of `1 + a1 z^-1 + a2 z^-2`.
    pub pole_radius: f64,
    /// Highest gain over DC–48 kHz (dB).
    pub full_peak: Peak,
    /// Highest gain over 20 Hz–20 kHz (dB).
    pub audible_peak: Peak,
    /// The a/b/c refusals, without a stage (the caller adds it).
    pub reasons: Vec<Reason>,
}

/// Pole radius of `1 + a1 z^-1 + a2 z^-2`.
#[must_use]
pub fn pole_radius(s: Section) -> f64 {
    let d = s.a1 * s.a1 - 4.0 * s.a2;
    if d < 0.0 {
        s.a2.sqrt()
    } else {
        ((-s.a1 + d.sqrt()) / 2.0)
            .abs()
            .max(((-s.a1 - d.sqrt()) / 2.0).abs())
    }
}

/// `|p(e^jw)|^2` is quadratic in `t = sin²(w/2)`; the ratio's derivative is
/// quadratic too. Returns its three coefficients `[c0, c1, c2]`.
fn power_polynomial(p0: f64, p1: f64, p2: f64) -> [f64; 3] {
    let sum = p0 + p1 + p2;
    [
        sum * sum,
        4.0 * ((p0 - p2).powi(2) - sum * (p0 + p2)),
        16.0 * p0 * p2,
    ]
}

/// Frequencies (Hz, inside DC–48 kHz) where the magnitude of one section is
/// stationary: the real roots of the ratio's derivative in `t = sin²(w/2)`.
fn stationary_frequencies(s: Section) -> Vec<f64> {
    let n = power_polynomial(s.b0, s.b1, s.b2);
    let d = power_polynomial(1.0, s.a1, s.a2);
    let a = n[2] * d[1] - n[1] * d[2];
    let b = 2.0 * (n[2] * d[0] - n[0] * d[2]);
    let c = n[1] * d[0] - n[0] * d[1];

    let mut roots: Vec<f64> = Vec::new();
    if a == 0.0 {
        if b != 0.0 {
            roots.push(-c / b);
        }
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant >= 0.0 {
            let sign = if b >= 0.0 { 1.0 } else { -1.0 };
            let q = -0.5 * (b + sign * discriminant.sqrt());
            if q != 0.0 {
                roots.push(q / a);
                roots.push(c / q);
            } else {
                roots.push(-b / (2.0 * a));
            }
        }
    }

    roots
        .into_iter()
        .filter(|t| *t > 0.0 && *t < 1.0)
        .map(|t| (t.sqrt().asin() * f64::from(auri_codec::SAMPLE_RATE)) / std::f64::consts::PI)
        .collect()
}

/// The highest `section_db` among `frequencies` (the first one on ties).
fn peak_among(s: Section, frequencies: &[f64]) -> Peak {
    let mut best = Peak {
        db: section_db(s, frequencies[0]),
        frequency_hz: frequencies[0],
    };
    for &f in &frequencies[1..] {
        let db = section_db(s, f);
        if db > best.db {
            best = Peak {
                db,
                frequency_hz: f,
            };
        }
    }
    best
}

/// Analytic peak of one section over a scan domain: DC–48 kHz, 20 Hz–20 kHz or
/// 20 Hz–48 kHz.
#[must_use]
pub fn single_section_peak(s: Section, min_hz: f64, max_hz: f64) -> Peak {
    if min_hz == 0.0 {
        return section_peaks(s).0;
    }
    if max_hz == crate::policy::AUDIBLE_MAX_HZ {
        return section_peaks(s).1;
    }
    section_extended_peak(s)
}

/// Exact (analytic) peaks of one section: `(full)` over DC–48 kHz and
/// `(audible)` over 20 Hz–20 kHz.
#[must_use]
pub fn section_peaks(s: Section) -> (Peak, Peak) {
    let stationary_hz = stationary_frequencies(s);
    let mut full_candidates = vec![0.0, 48000.0];
    full_candidates.extend_from_slice(&stationary_hz);
    let full = peak_among(s, &full_candidates);

    let mut audible_candidates = vec![20.0, 20000.0];
    audible_candidates.extend(
        stationary_hz
            .iter()
            .copied()
            .filter(|f| *f >= 20.0 && *f <= 20000.0),
    );
    let audible = peak_among(s, &audible_candidates);
    (full, audible)
}

/// Exact (analytic) peak of one section over the extended domain, 20 Hz–48 kHz.
#[must_use]
pub fn section_extended_peak(s: Section) -> Peak {
    let stationary_hz = stationary_frequencies(s);
    let candidates: Vec<f64> = std::iter::once(20.0)
        .chain(std::iter::once(48000.0))
        .chain(
            stationary_hz
                .iter()
                .copied()
                .filter(|f| *f >= 20.0 && *f <= 48000.0),
        )
        .collect();
    peak_among(s, &candidates)
}

/// Golden-section refinement of a local maximum in `[a, b]`.
pub(crate) fn refine(f: impl Fn(f64) -> f64, a: f64, b: f64) -> Peak {
    let r = (5.0_f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (a, b);
    let mut c = b - r * (b - a);
    let mut d = a + r * (b - a);
    let mut fc = f(c);
    let mut fd = f(d);
    let mut i = 0;
    while i < 70 && b - a > 1e-8 {
        if fc > fd {
            b = d;
            d = c;
            fd = fc;
            c = b - r * (b - a);
            fc = f(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + r * (b - a);
            fd = f(d);
        }
        i += 1;
    }
    if fc > fd {
        Peak {
            db: fc,
            frequency_hz: c,
        }
    } else {
        Peak {
            db: fd,
            frequency_hz: d,
        }
    }
}

/// The per-section checks (a, b, c) on the quantized section of `band`, with
/// its analytic peaks. Reasons carry `value` and `limit` (limits without
/// tolerance).
///
/// # Errors
///
/// [`auri_codec::CodecError`] like the codec, on bands it cannot encode.
pub fn section_analysis(band: auri_codec::Band) -> Result<SectionAnalysis, auri_codec::CodecError> {
    let words = encode_section(auri_codec::design_section(band)?, 0)?;
    let section = from_words(words);
    let radius = pole_radius(section);
    let (full_peak, audible_peak) = section_peaks(section);

    let mut reasons = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let limit = thresholds::COEFFICIENT_ABS_WORDS[i] as f64;
        let magnitude = f64::from(*word).abs();
        if !(magnitude <= limit) {
            reasons.push(Reason::measured(
                coefficient_reason(i),
                None,
                magnitude,
                limit,
            ));
        }
    }
    if !(radius <= thresholds::POLE_RADIUS + thresholds::RADIUS_NUMERICAL_TOLERANCE) {
        reasons.push(Reason::measured(
            ReasonCode::PoleRadius,
            None,
            radius,
            thresholds::POLE_RADIUS,
        ));
    }
    if !(full_peak.db <= thresholds::SECTION_PEAK_DB + thresholds::DB_NUMERICAL_TOLERANCE) {
        reasons.push(Reason::measured(
            ReasonCode::SectionPeak,
            None,
            full_peak.db,
            thresholds::SECTION_PEAK_DB,
        ));
    }

    Ok(SectionAnalysis {
        words,
        section,
        pole_radius: radius,
        full_peak,
        audible_peak,
        reasons,
    })
}

/// The reason code of wire position `i` (0 = `b0`, 4 = `-a2`).
#[must_use]
pub const fn coefficient_reason(i: usize) -> ReasonCode {
    match i {
        0 => ReasonCode::CoefficientB0,
        1 => ReasonCode::CoefficientB2,
        2 => ReasonCode::CoefficientMinusA1,
        3 => ReasonCode::CoefficientB1,
        _ => ReasonCode::CoefficientMinusA2,
    }
}

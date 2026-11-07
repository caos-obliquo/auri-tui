//! Gate v2 ("observed envelope") thresholds: the largest values the 11
//! official presets already make the chip compute, on the real Q27/Q23 words.
//!
//! A similarity policy, **not** a hardware safety proof — see the crate docs.
//!
//! Port of `src/safety/thresholds.mjs`, which `tools/gen-thresholds.mjs`
//! generates from `design/analysis/envelope.json`. The provenance hash is kept
//! here so a reader can tell which envelope these numbers came from.

/// Wire order of the section words.
pub const COEFFICIENT_POSITIONS: [&str; 5] = ["b0", "b2", "-a1", "b1", "-a2"];

/// Largest `|Q27 word|` the presets reach, per wire position.
pub const COEFFICIENT_ABS_WORDS: [i64; 5] = [176145720, 133967223, 268076748, 268076748, 133859379];

/// The same limits as normalized coefficients.
pub const COEFFICIENT_ABS: [f64; 5] = [
    1.3123878836631775,
    0.9981335923075676,
    1.9973274171352386,
    1.9973274171352386,
    0.9973300918936729,
];

/// Largest quantized pole radius.
pub const POLE_RADIUS: f64 = 0.99986846753918;
/// Largest single-section peak, DC–48 kHz (dB).
pub const SECTION_PEAK_DB: f64 = 9.200000050722664;
/// Largest prefix peak with preamp, 20 Hz–20 kHz (dB).
pub const INTERNAL_AUDIBLE_DB: f64 = -0.20368875272918074;
/// Largest output peak with preamp, 20 Hz–20 kHz (dB).
pub const OUTPUT_AUDIBLE_DB: f64 = -0.47087539615209006;
/// Largest output peak with preamp, DC–48 kHz (dB).
pub const OUTPUT_FULL_DB: f64 = 2.5000638697363913;
/// Highest nominal preamp (dB).
pub const PREAMP_MAX_DB: f64 = -0.5;
/// Lowest nominal preamp (dB).
pub const PREAMP_MIN_DB: f64 = -25.5;
/// Computation tolerance on dB limits.
pub const DB_NUMERICAL_TOLERANCE: f64 = 1e-8;
/// Computation tolerance on the pole radius.
pub const RADIUS_NUMERICAL_TOLERANCE: f64 = 2e-15;

/// SHA-256 of `design/analysis/envelope.json` (LF line endings) these numbers
/// were generated from.
pub const PROVENANCE_SHA256: &str =
    "45c1976cc4e520e42af3a053905e1a38b6fb3e74cbad6c9c21457361d81f38a3";

// YOLO gate profile only, so `THRESHOLDS` (and the gate version) do not change
// when these do.

/// `d:prefix-peak-full` limit: largest prefix peak with preamp, DC–48 kHz (dB).
pub const YOLO_INTERNAL_FULL_DB: f64 = 2.5000638697363913;
/// `d:prefix-peak-extended` limit: largest prefix peak with preamp, 20 Hz–48 kHz (dB).
pub const YOLO_INTERNAL_EXTENDED_DB: f64 = -0.20368875272917986;
/// `e:output-extended` limit: largest output peak with preamp, 20 Hz–48 kHz (dB).
pub const YOLO_OUTPUT_EXTENDED_DB: f64 = -0.4708753961520857;

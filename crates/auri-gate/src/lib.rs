//! Gate v2: the safety verdict every Kuba Auri curve must pass before it is
//! written to the cable.
//!
//! The limits are the **observed envelope of the 11 official presets** — the
//! largest values they already make the chip compute, on the real Q27/Q23 words
//! that go on the wire. This is a similarity policy, *not* a hardware safety
//! proof: it does not bound internal states, impulse-response norms, limit
//! cycles, transients or unknown firmware arithmetic.
//!
//! The gate is a faithful port of `src/safety/gate.mjs` from the open-auri
//! repository (AGPL-3.0), which is **not** vendored here: only the algorithms
//! are reimplemented, and [`tests/parity.rs`](https://docs.rs/) compares this
//! port's verdicts against the JavaScript oracle when a sibling checkout (or
//! `AURI_GATE_VERDICTS`) is available.
//!
//! # Example
//!
//! ```
//! use auri_gate::{gate, GateOptions};
//! use auri_codec::NEUTRAL_BANDS;
//!
//! let verdict = gate(&NEUTRAL_BANDS, -0.5, GateOptions::default());
//! assert!(verdict.ok);
//! println!("CRC {}", verdict.metrics.payload_crc.unwrap());
//! ```
//!
//! # Invariants
//!
//! - Nothing is ever written from here: this crate only judges.
//! - [`gate_for_write`] is the entry point a transport must use (always the
//!   `full` grid).
//! - Every comparison is written so that NaN fails (`!(x <= limit)`).

// The gate's comparisons are deliberately negated: `!(x <= limit)` is true for
// `x > limit`, for `+inf` **and for `NaN`**, which is exactly how the
// JavaScript oracle rejects a measurement it could not make. `x > limit` alone
// would silently accept `NaN`. Rewriting these as `partial_cmp` matches would
// obscure that, so the lint stays off here on purpose.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

mod gate;
mod level;
mod policy;
mod reason;
mod scan;
mod section;
mod thresholds;

pub use gate::{
    GateMetrics, GateOptions, GateResult, Grid, HARD_REASONS, Profile, YOLO_WAIVABLE, gate,
    gate_for_write, is_waivable,
};
pub use level::{
    from_words, is_identity_section, quantized_preamp_db, section_db, section_db_from_terms,
};
pub use policy::{
    AUDIBLE_MAX_HZ, BOOST_Q_MAX, GAIN_MAX_DB, GAIN_MIN_DB, PK_BOOST_MAX_HZ, ULTRA_CUT_Q_MAX,
    ULTRA_MAX_HZ, freq_max_for, is_kind_allowed, is_non_amplifying, is_ultrasonic, policy_reasons,
};
pub use reason::{Reason, ReasonCode, ReasonValue};
pub use section::{
    Peak, PrefixPeak, SectionAnalysis, coefficient_reason, pole_radius, section_analysis,
    section_extended_peak, section_peaks, single_section_peak,
};
pub use thresholds::{
    COEFFICIENT_ABS, COEFFICIENT_ABS_WORDS, COEFFICIENT_POSITIONS, DB_NUMERICAL_TOLERANCE,
    INTERNAL_AUDIBLE_DB, OUTPUT_AUDIBLE_DB, OUTPUT_FULL_DB, POLE_RADIUS, PREAMP_MAX_DB,
    PREAMP_MIN_DB, PROVENANCE_SHA256, RADIUS_NUMERICAL_TOLERANCE, SECTION_PEAK_DB,
    YOLO_INTERNAL_EXTENDED_DB, YOLO_INTERNAL_FULL_DB, YOLO_OUTPUT_EXTENDED_DB,
};

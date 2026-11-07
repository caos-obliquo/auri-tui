//! The few UI policy rules the gate enforces: filter kinds, the D7 boost rules
//! and the rule above 20 kHz.
//!
//! Port of `src/safety/policy.mjs`. Every comparison is written so that NaN
//! fails (`!(x <= limit)`), which matters for the JavaScript oracle even though
//! a Rust `Band` always carries numbers.

use auri_codec::{Band, BandKind};

/// Highest Q of an LS/HS cut and of an LP/HP placed above 20 kHz: up to it the
/// section has no resonant bump above 0 dB.
pub const ULTRA_CUT_Q_MAX: f64 = 0.71;

/// Top of the audible band (Hz): the highest frequency of Ouvir and Ajustar.
pub const AUDIBLE_MAX_HZ: f64 = 20000.0;

/// Highest frequency of a section above the audible band (Hz).
pub const ULTRA_MAX_HZ: f64 = 32000.0;

/// Allowed gain range of the editor (dB).
pub const GAIN_MIN_DB: f64 = -24.0;
/// Allowed gain range of the editor (dB).
pub const GAIN_MAX_DB: f64 = 9.2;

/// Highest Q of a boosting section.
pub const BOOST_Q_MAX: f64 = 4.0;
/// Highest frequency of a boosting peaking section (Hz).
pub const PK_BOOST_MAX_HZ: f64 = 16000.0;

/// Kinds allowed in each UI layer. Unknown layers allow nothing.
const LAYER_KINDS: [(&str, &[BandKind]); 3] = [
    ("ouvir", &[BandKind::Pk, BandKind::Ls, BandKind::Off]),
    ("ajustar", &[BandKind::Pk, BandKind::Ls, BandKind::Off]),
    (
        "engenharia",
        &[
            BandKind::Pk,
            BandKind::Ls,
            BandKind::Off,
            BandKind::Hs,
            BandKind::Lp,
            BandKind::Hp,
        ],
    ),
];

/// Whether `kind` may be used in `layer`. Unknown layers allow nothing.
#[must_use]
pub fn is_kind_allowed(kind: BandKind, layer: &str) -> bool {
    LAYER_KINDS
        .iter()
        .find(|(name, _)| *name == layer)
        .is_some_and(|(_, kinds)| kinds.contains(&kind))
}

/// Whether `band` is a filter section above the audible band: `freq_hz > 20 kHz`.
#[must_use]
pub fn is_ultrasonic(band: Band) -> bool {
    let freq_hz = match band.freq_hz() {
        Some(value) => value,
        None => return false, // OFF
    };
    if !matches!(
        band.kind(),
        BandKind::Pk | BandKind::Ls | BandKind::Hs | BandKind::Lp | BandKind::Hp
    ) {
        return false;
    }
    !(freq_hz <= AUDIBLE_MAX_HZ)
}

/// Whether `band` is a section that never amplifies, wherever it sits: a PK
/// with gain <= 0 (any Q), an LS or HS with gain <= 0 and Q <= 0.71, an LP or HP
/// with Q <= 0.71.
#[must_use]
pub fn is_non_amplifying(band: Band) -> bool {
    let q = band.q_factor();
    match band.kind() {
        BandKind::Pk => band.gain_db() <= 0.0,
        BandKind::Ls | BandKind::Hs => {
            band.gain_db() <= 0.0 && q.is_some_and(|q| q <= ULTRA_CUT_Q_MAX)
        }
        BandKind::Lp | BandKind::Hp => q.is_some_and(|q| q <= ULTRA_CUT_Q_MAX),
        BandKind::Off => false,
    }
}

/// Highest frequency (Hz) `band` may take in `layer`.
#[must_use]
pub fn freq_max_for(band: Band, layer: &str, yolo: bool) -> f64 {
    if layer != "engenharia" {
        return AUDIBLE_MAX_HZ;
    }
    if yolo {
        return ULTRA_MAX_HZ;
    }
    if is_non_amplifying(band) {
        ULTRA_MAX_HZ
    } else {
        AUDIBLE_MAX_HZ
    }
}

/// The policy reasons (kinds, D7 and the rule above 20 kHz) for each band, in
/// stage order: every `k:` first, then every `d7:` — the JavaScript order.
pub fn policy_reasons(bands: &[Band], experimental: bool) -> Vec<crate::reason::Reason> {
    use crate::reason::{Reason, ReasonCode};

    let layer = if experimental {
        "engenharia"
    } else {
        "ajustar"
    };
    let mut kinds: Vec<Reason> = Vec::new();
    let mut d7: Vec<Reason> = Vec::new();

    for (index, band) in bands.iter().enumerate() {
        let stage = u32::try_from(index + 1).expect("stage fits in u32");
        let kind = band.kind();

        if !is_kind_allowed(kind, layer) {
            kinds.push(Reason {
                code: ReasonCode::KindNotAllowed,
                stage: Some(stage),
                value: crate::reason::ReasonValue::Text(kind.as_str().to_string()),
                limit: None,
            });
        }

        if matches!(kind, BandKind::Pk | BandKind::Ls | BandKind::Hs) && !(band.gain_db() <= 0.0) {
            if kind == BandKind::Pk && !(band.freq_hz().unwrap_or(f64::NAN) <= PK_BOOST_MAX_HZ) {
                d7.push(Reason::measured(
                    ReasonCode::PkBoostAbove16k,
                    Some(stage),
                    band.freq_hz().unwrap_or(f64::NAN),
                    PK_BOOST_MAX_HZ,
                ));
            }
            if !(band.q_factor().unwrap_or(f64::NAN) <= BOOST_Q_MAX) {
                d7.push(Reason::measured(
                    ReasonCode::BoostQ,
                    Some(stage),
                    band.q_factor().unwrap_or(f64::NAN),
                    BOOST_Q_MAX,
                ));
            }
        }

        // Above 20 kHz only what Engenharia allows there (sections that never
        // amplify). A NaN frequency counts as above.
        if is_ultrasonic(*band) && !(freq_max_for(*band, "engenharia", false) > AUDIBLE_MAX_HZ) {
            d7.push(Reason::measured(
                ReasonCode::BoostAbove20k,
                Some(stage),
                band.freq_hz().unwrap_or(f64::NAN),
                AUDIBLE_MAX_HZ,
            ));
        }
    }

    kinds.extend(d7);
    kinds
}

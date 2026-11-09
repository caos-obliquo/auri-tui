//! Gate v2: the verdict every curve must pass before it is written.
//!
//! Port of `src/safety/gate.mjs` (the current version), with the same math and
//! the same order of summations.
//!
//! One read per field: [`gate`] copies the bands once on entry and evaluates
//! only that copy (policy, codec, analysis). The copy and the exact payload
//! bytes are returned in [`GateMetrics::bands`] and [`GateMetrics::payload`],
//! so a transport can send exactly what was gated.
//!
//! # Profiles
//!
//! The **standard** profile (default) blocks on every reason. The **YOLO**
//! profile splits them into two classes:
//!
//! - class 1, hard in every profile: `f:codec`, `a:-a1`, `a:-a2`,
//!   `b:pole-radius`, `d:prefix-peak`, `d:prefix-peak-full`,
//!   `d:prefix-peak-extended`, `e:output-audible`, `e:output-extended`,
//!   `e:output-full`;
//! - class 2, waived ([`YOLO_WAIVABLE`]): `k:kind-not-allowed`, `d7:*`,
//!   `g:preamp`, `a:b0`, `a:b2`, `a:b1`, `c:section-peak`.
//!
//! The YOLO-only checks (`d:prefix-peak-full`, `d:prefix-peak-extended`,
//! `e:output-extended`) apply only when the verdict relies on a waiver, so YOLO
//! is never stricter than the standard profile.
//!
//! # This is not a hardware safety proof
//!
//! The limits are the observed envelope of the 11 official presets, on the real
//! Q27/Q23 words that go to the cable. They do not bound internal states,
//! impulse-response norms, limit cycles, transients or unknown firmware
//! arithmetic.

use auri_codec::{BAND_COUNT, Band, SAMPLE_RATE, encode_payload};

use crate::level::{is_identity_section, quantized_preamp_db};
use crate::policy::policy_reasons;
use crate::reason::{Reason, ReasonCode};
use crate::scan::CascadeScan;
use crate::section::{Peak, PrefixPeak, SectionAnalysis, section_analysis};
use crate::thresholds;

/// Log intervals of the multi-section scans per grid mode.
///
/// Both modes scan the grid, refine every interior local maximum by
/// golden-section search, and also always refine the first and the last
/// interval — which the JavaScript reference does not, so a peak can only be
/// higher (stricter) here, never lower. Writes must use [`gate_for_write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grid {
    /// 2048 intervals: advisory, for interactive feedback.
    Drag,
    /// 16384 intervals: the reference, and what a write uses.
    #[default]
    Full,
}

impl Grid {
    /// Number of log intervals this grid scans.
    #[must_use]
    pub const fn intervals(self) -> usize {
        match self {
            Grid::Drag => 2048,
            Grid::Full => 16384,
        }
    }

    /// The JavaScript spelling of this grid.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Grid::Drag => "drag",
            Grid::Full => "full",
        }
    }
}

/// Which set of limits a verdict is judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// The default: blocks on every reason.
    #[default]
    Standard,
    /// Waives the class-2 reasons and adds the out-of-band checks.
    Yolo,
}

impl Profile {
    /// The JavaScript spelling of this profile.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Profile::Standard => "standard",
            Profile::Yolo => "yolo",
        }
    }
}

/// Reason codes the YOLO profile waives (class 2): engineering and similarity
/// limits of the editor, not limits of the chip's arithmetic. Every other code
/// blocks in every profile.
pub const YOLO_WAIVABLE: [ReasonCode; 9] = [
    ReasonCode::KindNotAllowed,
    ReasonCode::PkBoostAbove16k,
    ReasonCode::BoostQ,
    ReasonCode::BoostAbove20k,
    ReasonCode::Preamp,
    ReasonCode::CoefficientB0,
    ReasonCode::CoefficientB2,
    ReasonCode::CoefficientB1,
    ReasonCode::SectionPeak,
];

/// Reason codes that block in every profile (class 1), in reason order.
pub const HARD_REASONS: [ReasonCode; 10] = [
    ReasonCode::Codec,
    ReasonCode::CoefficientMinusA1,
    ReasonCode::CoefficientMinusA2,
    ReasonCode::PoleRadius,
    ReasonCode::PrefixPeak,
    ReasonCode::PrefixPeakFull,
    ReasonCode::PrefixPeakExtended,
    ReasonCode::OutputAudible,
    ReasonCode::OutputExtended,
    ReasonCode::OutputFull,
];

/// Whether the YOLO profile waives `code` (class 2).
#[must_use]
pub fn is_waivable(code: ReasonCode) -> bool {
    YOLO_WAIVABLE.contains(&code)
}

/// Scan domains, as `design/analysis/envelope.mjs` defines them.
const AUDIBLE_MIN_HZ: f64 = 20.0;
const AUDIBLE_MAX_HZ: f64 = 20000.0;
const FULL_MAX_HZ: f64 = SAMPLE_RATE as f64 / 2.0;
const EXTENDED_MIN_HZ: f64 = AUDIBLE_MIN_HZ;
const EXTENDED_MAX_HZ: f64 = FULL_MAX_HZ;

/// What a verdict carries besides its reasons.
#[derive(Debug, Clone, PartialEq)]
pub struct GateMetrics {
    /// The grid the multi-section scans used.
    pub grid: Grid,
    /// CRC-16 of the payload, as `0x` + the two little-endian bytes in hex.
    pub payload_crc: Option<String>,
    /// The exact 220 bytes that would go on the wire.
    pub payload: Option<Vec<u8>>,
    /// The frozen copy the verdict was computed from.
    pub bands: Option<Vec<Band>>,
    /// The requested preamp (dB).
    pub preamp_db: Option<f64>,
    /// The preamp the chip will actually apply, after Q23 quantization (dB).
    pub quantized_preamp_db: Option<f64>,
    /// Per-section analysis (words, radius, analytic peaks, a/b/c reasons).
    pub sections: Option<Vec<SectionAnalysis>>,
    /// Peak of each prefix of sections plus the preamp, 20 Hz–20 kHz.
    pub prefix_audible: Option<Vec<PrefixPeak>>,
    /// Highest prefix peak, 20 Hz–20 kHz.
    pub internal_audible: Option<PrefixPeak>,
    /// Peak of the whole curve plus the preamp, 20 Hz–20 kHz.
    pub output_audible: Option<PrefixPeak>,
    /// Peak of the whole curve plus the preamp, DC–48 kHz.
    pub output_full: Option<Peak>,
    /// Peak of each prefix, DC–48 kHz (YOLO with a waiver only).
    pub prefix_full: Option<Vec<PrefixPeak>>,
    /// Highest prefix peak, DC–48 kHz (YOLO with a waiver only).
    pub internal_full: Option<PrefixPeak>,
    /// Peak of each prefix, 20 Hz–48 kHz (YOLO with a waiver only).
    pub prefix_extended: Option<Vec<PrefixPeak>>,
    /// Highest prefix peak, 20 Hz–48 kHz (YOLO with a waiver only).
    pub internal_extended: Option<PrefixPeak>,
    /// Peak of the whole curve plus the preamp, 20 Hz–48 kHz (YOLO with a waiver only).
    pub output_extended: Option<PrefixPeak>,
}

impl Default for GateMetrics {
    fn default() -> Self {
        GateMetrics {
            grid: Grid::Full,
            payload_crc: None,
            payload: None,
            bands: None,
            preamp_db: None,
            quantized_preamp_db: None,
            sections: None,
            prefix_audible: None,
            internal_audible: None,
            output_audible: None,
            output_full: None,
            prefix_full: None,
            internal_full: None,
            prefix_extended: None,
            internal_extended: None,
            output_extended: None,
        }
    }
}

/// The verdict for a curve.
///
/// `reasons` are the blocking reasons (all of them in the standard profile).
/// YOLO results only: [`GateResult::waived`] lists, once each in order of first
/// appearance, the codes the profile waived, and [`GateResult::waived_reasons`]
/// the waived reasons themselves. A standard result has none of them.
#[derive(Debug, Clone, PartialEq)]
pub struct GateResult {
    /// Whether the curve may be written.
    pub ok: bool,
    /// The blocking reasons.
    pub reasons: Vec<Reason>,
    /// The waived codes (YOLO only).
    pub waived: Option<Vec<ReasonCode>>,
    /// The waived reasons themselves (YOLO only).
    pub waived_reasons: Option<Vec<Reason>>,
    /// Always [`Profile::Yolo`] on a YOLO result, `None` otherwise.
    pub profile: Option<Profile>,
    /// Everything the verdict measured.
    pub metrics: GateMetrics,
}

/// Options of a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateOptions {
    /// Allow the engineering kinds (HS, LP, HP).
    pub experimental: bool,
    /// Which grid the multi-section scans use.
    pub grid: Grid,
    /// Which set of limits to judge against.
    pub profile: Profile,
}

impl Default for GateOptions {
    fn default() -> Self {
        GateOptions {
            experimental: false,
            grid: Grid::Full,
            profile: Profile::Standard,
        }
    }
}

impl GateOptions {
    /// Options for a write: always the `full` grid.
    #[must_use]
    pub fn for_write(experimental: bool, profile: Profile) -> Self {
        GateOptions {
            experimental,
            grid: Grid::Full,
            profile,
        }
    }
}

/// The verdict for `bands` (wire order) and `preamp_db`.
///
/// `grid: Drag` scans multi-section responses on 2048 log intervals for
/// interactive feedback; `full` (default) uses the reference 16384. Single
/// sections always use the analytic peak. Only `full` decides a write: call
/// [`gate_for_write`]. HS/LP/HP need `experimental: true`.
pub fn gate(bands: &[Band], preamp_db: f64, options: GateOptions) -> GateResult {
    let profile = options.profile;
    let yolo = profile == Profile::Yolo;
    let intervals = options.grid.intervals();
    let mut metrics = GateMetrics {
        grid: options.grid,
        ..GateMetrics::default()
    };

    // The copy step: `BAND_COUNT` or `f:codec` and nothing else.
    if bands.len() != BAND_COUNT {
        return verdict(
            vec![Reason::codec(format!("expected {BAND_COUNT} bands"))],
            metrics,
            profile,
        );
    }
    let snapshot: Vec<Band> = bands.to_vec();

    let mut reasons = policy_reasons(&snapshot, options.experimental);
    let payload = match encode_payload(&snapshot, preamp_db) {
        Ok(payload) => payload,
        Err(error) => {
            reasons.push(Reason::codec(error.to_string()));
            return verdict(reasons, metrics, profile);
        }
    };

    metrics.payload_crc = Some(payload_crc(&payload));
    metrics.payload = Some(payload);
    metrics.bands = Some(snapshot.clone());

    if !(thresholds::PREAMP_MIN_DB..=thresholds::PREAMP_MAX_DB).contains(&preamp_db) {
        let limit = if preamp_db < thresholds::PREAMP_MIN_DB {
            thresholds::PREAMP_MIN_DB
        } else {
            thresholds::PREAMP_MAX_DB
        };
        reasons.push(Reason::measured(ReasonCode::Preamp, None, preamp_db, limit));
    }

    let preamp =
        quantized_preamp_db(preamp_db).expect("encode_payload already accepted this preamp");
    metrics.preamp_db = Some(preamp_db);
    metrics.quantized_preamp_db = Some(preamp);

    let mut sections: Vec<SectionAnalysis> = Vec::with_capacity(BAND_COUNT);
    for band in &snapshot {
        let analysis = match section_analysis(*band) {
            Ok(analysis) => analysis,
            Err(error) => {
                // Unreachable: `encode_payload` encoded these very words.
                reasons.push(Reason::codec(error.to_string()));
                return verdict(reasons, metrics, profile);
            }
        };
        for reason in &analysis.reasons {
            let stage = u32::try_from(sections.len() + 1).expect("stage fits in u32");
            reasons.push(Reason {
                code: reason.code,
                stage: Some(stage),
                value: reason.value.clone(),
                limit: reason.limit,
            });
        }
        sections.push(analysis);
    }
    metrics.sections = Some(sections.clone());

    // Whether the verdict relies on a waiver. Every waivable reason is already
    // in `reasons` here (the d and e reasons below never are), so this is the
    // final answer.
    let waiver = yolo && reasons.iter().any(|r| is_waivable(r.code));

    // Identity sections (OFF) are skipped, as in the reference. The scan grids
    // are only built when more than one section is active.
    let active: Vec<_> = sections
        .iter()
        .map(|s| s.section)
        .filter(|s| !is_identity_section(*s))
        .collect();
    let scan = active.len() > 1;

    let mut audible = CascadeScan::new(intervals, AUDIBLE_MIN_HZ, AUDIBLE_MAX_HZ, scan);
    let mut prefix_audible = vec![PrefixPeak {
        stage: 0,
        db: preamp,
        frequency_hz: None,
    }];
    let mut peak: Option<Peak> = None;
    for (i, analysis) in sections.iter().enumerate() {
        if !is_identity_section(analysis.section) {
            audible.add(analysis.section);
            peak = None;
        }
        // An unchanged prefix has the same (deterministic) peak: reuse it.
        if peak.is_none() {
            peak = Some(audible.peak());
        }
        let peak = peak.expect("peak set above");
        prefix_audible.push(PrefixPeak {
            stage: u32::try_from(i + 1).expect("stage fits in u32"),
            db: peak.db + preamp,
            frequency_hz: Some(peak.frequency_hz),
        });
    }
    metrics.prefix_audible = Some(prefix_audible.clone());
    let internal_audible = highest(&prefix_audible);
    let output_audible = *prefix_audible
        .last()
        .expect("the preamp prefix is always present");
    metrics.internal_audible = Some(internal_audible);
    metrics.output_audible = Some(output_audible);

    let mut full_scan = CascadeScan::new(intervals, 0.0, FULL_MAX_HZ, scan);
    let mut prefix_full: Option<Vec<PrefixPeak>> = None;
    if waiver {
        // YOLO with a waiver only: every prefix over DC–48 kHz. The sections are
        // added in the same order as below, so the last prefix's scan is the
        // very scan `output_full` reads.
        let mut built = vec![PrefixPeak {
            stage: 0,
            db: preamp,
            frequency_hz: None,
        }];
        let mut full_peak: Option<Peak> = None;
        for (i, analysis) in sections.iter().enumerate() {
            if !is_identity_section(analysis.section) {
                full_scan.add(analysis.section);
                full_peak = None;
            }
            if full_peak.is_none() {
                full_peak = Some(full_scan.peak());
            }
            let full_peak = full_peak.expect("peak set above");
            built.push(PrefixPeak {
                stage: u32::try_from(i + 1).expect("stage fits in u32"),
                db: full_peak.db + preamp,
                frequency_hz: Some(full_peak.frequency_hz),
            });
        }
        prefix_full = Some(built);
    } else {
        for section in &active {
            full_scan.add(*section);
        }
    }
    let full = full_scan.peak();
    let output_full = Peak {
        db: full.db + preamp,
        frequency_hz: full.frequency_hz,
    };
    metrics.output_full = Some(output_full);

    let mut internal_full: Option<PrefixPeak> = None;
    if let Some(prefix) = &prefix_full {
        internal_full = Some(highest(prefix));
        metrics.prefix_full = Some(prefix.clone());
        metrics.internal_full = internal_full;
    }

    let mut prefix_extended: Option<Vec<PrefixPeak>> = None;
    if waiver {
        // YOLO with a waiver only: every prefix over 20 Hz–48 kHz, whatever the
        // waiver and the sections' frequencies, so a boost at 20000 Hz that
        // needs a waiver meets the limits of one at 20001 Hz (no cliff).
        let mut extended_scan = CascadeScan::new(intervals, EXTENDED_MIN_HZ, EXTENDED_MAX_HZ, scan);
        let mut built = vec![PrefixPeak {
            stage: 0,
            db: preamp,
            frequency_hz: None,
        }];
        let mut extended_peak: Option<Peak> = None;
        for (i, analysis) in sections.iter().enumerate() {
            if !is_identity_section(analysis.section) {
                extended_scan.add(analysis.section);
                extended_peak = None;
            }
            if extended_peak.is_none() {
                extended_peak = Some(extended_scan.peak());
            }
            let extended_peak = extended_peak.expect("peak set above");
            built.push(PrefixPeak {
                stage: u32::try_from(i + 1).expect("stage fits in u32"),
                db: extended_peak.db + preamp,
                frequency_hz: Some(extended_peak.frequency_hz),
            });
        }
        metrics.internal_extended = Some(highest(&built));
        metrics.output_extended = built.last().copied();
        prefix_extended = Some(built);
    }
    if let Some(prefix) = &prefix_extended {
        metrics.prefix_extended = Some(prefix.clone());
    }

    let tolerance = thresholds::DB_NUMERICAL_TOLERANCE;

    if !(internal_audible.db <= thresholds::INTERNAL_AUDIBLE_DB + tolerance) {
        // Where it first happens: the earliest prefix over the limit.
        let first = prefix_audible
            .iter()
            .find(|p| !(p.db <= thresholds::INTERNAL_AUDIBLE_DB + tolerance));
        reasons.push(Reason::measured(
            ReasonCode::PrefixPeak,
            Some(first.map_or(internal_audible.stage, |p| p.stage)),
            internal_audible.db,
            thresholds::INTERNAL_AUDIBLE_DB,
        ));
    }
    if let (Some(prefix), Some(internal)) = (&prefix_full, internal_full) {
        let limit = thresholds::YOLO_INTERNAL_FULL_DB;
        if !(internal.db <= limit + tolerance) {
            let first = prefix.iter().find(|p| !(p.db <= limit + tolerance));
            reasons.push(Reason::measured(
                ReasonCode::PrefixPeakFull,
                Some(first.map_or(internal.stage, |p| p.stage)),
                internal.db,
                limit,
            ));
        }
    }
    if let (Some(prefix), Some(internal)) = (&prefix_extended, metrics.internal_extended) {
        let limit = thresholds::YOLO_INTERNAL_EXTENDED_DB;
        if !(internal.db <= limit + tolerance) {
            let first = prefix.iter().find(|p| !(p.db <= limit + tolerance));
            reasons.push(Reason::measured(
                ReasonCode::PrefixPeakExtended,
                Some(first.map_or(internal.stage, |p| p.stage)),
                internal.db,
                limit,
            ));
        }
    }
    if !(output_audible.db <= thresholds::OUTPUT_AUDIBLE_DB + tolerance) {
        reasons.push(Reason::measured(
            ReasonCode::OutputAudible,
            None,
            output_audible.db,
            thresholds::OUTPUT_AUDIBLE_DB,
        ));
    }
    if let Some(output_extended) = metrics.output_extended {
        let limit = thresholds::YOLO_OUTPUT_EXTENDED_DB;
        if !(output_extended.db <= limit + tolerance) {
            reasons.push(Reason::measured(
                ReasonCode::OutputExtended,
                None,
                output_extended.db,
                limit,
            ));
        }
    }
    if !(output_full.db <= thresholds::OUTPUT_FULL_DB + tolerance) {
        reasons.push(Reason::measured(
            ReasonCode::OutputFull,
            None,
            output_full.db,
            thresholds::OUTPUT_FULL_DB,
        ));
    }

    verdict(reasons, metrics, profile)
}

/// The gate transport MUST call before writing: always the `full` grid,
/// whatever the options say.
#[must_use]
pub fn gate_for_write(bands: &[Band], preamp_db: f64, options: GateOptions) -> GateResult {
    gate(
        bands,
        preamp_db,
        GateOptions {
            grid: Grid::Full,
            ..options
        },
    )
}

/// The highest prefix of a peak list (the first one on ties).
fn highest(prefix: &[PrefixPeak]) -> PrefixPeak {
    let mut best = prefix[0];
    for peak in &prefix[1..] {
        if peak.db > best.db {
            best = *peak;
        }
    }
    best
}

/// CRC-16 of the payload, as `0x` plus the two little-endian bytes in hex.
fn payload_crc(payload: &[u8]) -> String {
    let hex = |byte: u8| format!("{byte:02x}");
    let last = payload[payload.len() - 1];
    let second_last = payload[payload.len() - 2];
    format!("0x{}{}", hex(last), hex(second_last))
}

/// The verdict for `reasons` under `profile`: standard blocks on all of them;
/// YOLO moves the waivable ones (class 2) to `waived`/`waived_reasons`.
fn verdict(reasons: Vec<Reason>, metrics: GateMetrics, profile: Profile) -> GateResult {
    if profile != Profile::Yolo {
        let ok = reasons.is_empty();
        return GateResult {
            ok,
            reasons,
            waived: None,
            waived_reasons: None,
            profile: None,
            metrics,
        };
    }
    let mut blocking = Vec::new();
    let mut waived_reasons = Vec::new();
    for reason in reasons {
        if is_waivable(reason.code) {
            waived_reasons.push(reason);
        } else {
            blocking.push(reason);
        }
    }
    let mut waived: Vec<ReasonCode> = Vec::new();
    for reason in &waived_reasons {
        if !waived.contains(&reason.code) {
            waived.push(reason.code);
        }
    }
    let ok = blocking.is_empty();
    GateResult {
        ok,
        reasons: blocking,
        waived: Some(waived),
        waived_reasons: Some(waived_reasons),
        profile: Some(Profile::Yolo),
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auri_codec::{Band, NEUTRAL_BANDS};

    fn code(result: &GateResult, index: usize) -> ReasonCode {
        result.reasons[index].code
    }

    #[test]
    fn neutral_curve_passes_the_standard_profile() {
        let result = gate(&NEUTRAL_BANDS, -0.5, GateOptions::default());
        assert!(result.ok, "{:?}", result.reasons);
        assert!(result.reasons.is_empty());
        assert!(result.profile.is_none());
        assert!(result.waived.is_none());
        let payload = encode_payload(&NEUTRAL_BANDS, -0.5).expect("neutral payload");
        let expected = format!("0x{:02x}{:02x}", payload[219], payload[218]);
        assert_eq!(
            result.metrics.payload_crc.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(result.metrics.payload.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn a_preamp_above_the_range_is_refused() {
        let result = gate(&NEUTRAL_BANDS, -0.2, GateOptions::default());
        assert!(!result.ok);
        // `g:preamp` comes before the d/e reasons it also causes.
        assert_eq!(code(&result, 0), ReasonCode::Preamp);
        assert_eq!(result.reasons[0].limit, Some(thresholds::PREAMP_MAX_DB));
        assert!(
            result
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::PrefixPeak)
        );
    }

    #[test]
    fn a_preamp_below_the_range_names_the_floor() {
        let result = gate(&NEUTRAL_BANDS, -30.0, GateOptions::default());
        assert!(!result.ok);
        assert_eq!(code(&result, 0), ReasonCode::Preamp);
        assert_eq!(result.reasons[0].limit, Some(thresholds::PREAMP_MIN_DB));
    }

    #[test]
    fn a_positive_preamp_never_encodes() {
        let result = gate(&NEUTRAL_BANDS, 1.0, GateOptions::default());
        assert!(!result.ok);
        assert_eq!(code(&result, 0), ReasonCode::Codec);
    }

    #[test]
    fn a_zero_q_is_a_codec_refusal() {
        let mut bands = NEUTRAL_BANDS;
        bands[0] = Band::Pk {
            freq_hz: 1000.0,
            gain_db: 0.0,
            q_factor: 0.0,
        };
        let result = gate(&bands, -0.5, GateOptions::default());
        assert!(!result.ok);
        assert_eq!(code(&result, 0), ReasonCode::Codec);
        assert!(result.reasons[0].value.as_text().is_some());
    }

    #[test]
    fn the_wrong_band_count_is_a_codec_refusal() {
        let bands = [Band::Off; BAND_COUNT - 1];
        let result = gate(&bands, -0.5, GateOptions::default());
        assert!(!result.ok);
        assert_eq!(result.reasons.len(), 1);
        assert_eq!(code(&result, 0), ReasonCode::Codec);
        assert_eq!(result.reasons[0].value.as_text(), Some("expected 10 bands"));
        assert!(result.metrics.payload.is_none());
    }

    #[test]
    fn a_high_shelf_needs_the_engineering_layer() {
        let mut bands = NEUTRAL_BANDS;
        bands[0] = Band::Hs {
            freq_hz: 8000.0,
            gain_db: -3.0,
            q_factor: 0.7,
        };
        let standard = gate(&bands, -0.5, GateOptions::default());
        assert!(!standard.ok);
        assert!(
            standard
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::KindNotAllowed)
        );

        let experimental = gate(
            &bands,
            -0.5,
            GateOptions {
                experimental: true,
                ..GateOptions::default()
            },
        );
        assert!(
            !experimental
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::KindNotAllowed),
            "{:?}",
            experimental.reasons
        );
    }

    #[test]
    fn an_ultrasonic_boost_is_refused_above_20k() {
        let mut bands = NEUTRAL_BANDS;
        // A shelf that amplifies above 20 kHz: Engenharia would not allow it.
        bands[0] = Band::Ls {
            freq_hz: 24000.0,
            gain_db: 6.0,
            q_factor: 0.5,
        };
        let result = gate(&bands, -0.5, GateOptions::default());
        assert!(!result.ok);
        assert!(
            result
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::BoostAbove20k)
        );
        // ...while a cut up there is allowed.
        let mut cut = NEUTRAL_BANDS;
        cut[0] = Band::Ls {
            freq_hz: 24000.0,
            gain_db: -6.0,
            q_factor: 0.5,
        };
        let allowed = gate(&cut, -0.5, GateOptions::default());
        assert!(
            !allowed
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::BoostAbove20k),
            "{:?}",
            allowed.reasons
        );
    }

    #[test]
    fn yolo_waives_the_preamp_and_standard_does_not() {
        // −26 dB is below the storable floor: `g:preamp` and nothing else.
        let standard = gate(&NEUTRAL_BANDS, -26.0, GateOptions::default());
        assert!(!standard.ok);
        assert_eq!(standard.reasons.len(), 1);
        assert_eq!(code(&standard, 0), ReasonCode::Preamp);
        assert!(standard.profile.is_none());

        let yolo = gate(
            &NEUTRAL_BANDS,
            -26.0,
            GateOptions {
                profile: Profile::Yolo,
                ..GateOptions::default()
            },
        );
        assert!(yolo.ok, "{:?}", yolo.reasons);
        assert!(yolo.reasons.is_empty());
        assert_eq!(yolo.profile, Some(Profile::Yolo));
        assert_eq!(
            yolo.waived.as_deref(),
            Some([ReasonCode::Preamp].as_slice())
        );
        assert_eq!(
            yolo.waived_reasons.as_ref().map(Vec::len),
            Some(1),
            "the waived reason is kept with its value and limit"
        );
    }

    #[test]
    fn yolo_still_blocks_on_a_hard_reason() {
        let result = gate(
            &NEUTRAL_BANDS,
            1.0,
            GateOptions {
                profile: Profile::Yolo,
                ..GateOptions::default()
            },
        );
        assert!(!result.ok);
        assert_eq!(code(&result, 0), ReasonCode::Codec);
        assert_eq!(result.waived.as_deref(), Some(&[][..]));
    }
}

//! The "before writing" checklist: lamps a–g + k with their criterion values,
//! the tickable confirmations, spacing, counters and the loudness warning.
//!
//! One evaluation reads **one** gate verdict — never a second gate — and every
//! screen derives from the same [`ChecklistResult`]. Behaviour follows
//! open-auri's `web/src/lib/cable/checklist.ts` and `criteria.ts`
//! (AGPL-3.0); the algorithms are re-derived, the prose is re-authored.

use auri_codec::{Band, BandKind};
use auri_gate::{
    COEFFICIENT_ABS_WORDS, GateOptions, GateResult, INTERNAL_AUDIBLE_DB, OUTPUT_AUDIBLE_DB,
    POLE_RADIUS, PREAMP_MAX_DB, PREAMP_MIN_DB, Profile, SECTION_PEAK_DB, Reason, gate_for_write,
    is_ultrasonic, level_db,
};

use crate::flash::{Counters, FlashState, Spacing, can_write_now, counters, daily_notice};
use crate::loudness::{Loudness, LoudnessInput, loudness_warning};

/// The confirmation sentence for curves with kinds (HS, LP, HP) or sections
/// above 20 kHz the official page never wrote — re-authored.
pub const EXPERIMENTAL_TEXT: &str =
    "This curve uses filter types the official page never wrote. Their coefficients are inside the observed envelope, but the firmware has never received these formats.";

/// Appended when the curve also has a section above 20 kHz — re-authored.
pub const ULTRASONIC_TEXT: &str =
    "Filters above 20 kHz: confirm this experimental write.";

/// Shown while any required item is still unsatisfied — re-authored.
pub const LOCKED_TEXT: &str = "Complete every item to enable writing.";

/// The eight lamps of a verdict.
pub const LAMPS: [LampId; 8] = [
    LampId::A,
    LampId::B,
    LampId::C,
    LampId::D,
    LampId::E,
    LampId::F,
    LampId::G,
    LampId::K,
];

/// One lamp of the checklist (`a`–`g` plus `k`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LampId {
    /// Coefficient words within the envelope.
    A,
    /// Pole radius within the envelope.
    B,
    /// Single-section peak within the envelope.
    C,
    /// Internal (prefix) peak within the envelope.
    D,
    /// Output peak within the envelope.
    E,
    /// The codec accepted the curve (and the CRC shows).
    F,
    /// Preamp within the envelope.
    G,
    /// Filter kinds and the D7 editing limits.
    K,
}

impl LampId {
    /// The lamp's letter, as the reasons spell its prefix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LampId::A => "a",
            LampId::B => "b",
            LampId::C => "c",
            LampId::D => "d",
            LampId::E => "e",
            LampId::F => "f",
            LampId::G => "g",
            LampId::K => "k",
        }
    }

    /// Position in [`LAMPS`], for array-backed storage.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            LampId::A => 0,
            LampId::B => 1,
            LampId::C => 2,
            LampId::D => 3,
            LampId::E => 4,
            LampId::F => 5,
            LampId::G => 6,
            LampId::K => 7,
        }
    }
}

/// What one lamp shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LampState {
    /// Nothing blocks on this lamp.
    Ok,
    /// Blocking reasons carry this lamp.
    Fail,
    /// Only reasons the YOLO profile waived (class 1 never becomes this).
    Waived,
    /// The verdict stopped before this lamp could be measured (an early
    /// `f:codec` refusal) — except `f` itself, which still has an answer.
    Unknown,
}

/// One lamp with the reasons that decided it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lamp {
    /// Which lamp.
    pub id: LampId,
    /// How it lights.
    pub state: LampState,
    /// The blocking, then the waived, reasons carrying this lamp.
    pub reasons: Vec<Reason>,
}

/// The lamp of a reason code: `d7:*` and `k:*` share `k`, everything else
/// follows its prefix, and an unknown prefix falls back to `f` (codec).
#[must_use]
pub fn lamp_of(code: &str) -> LampId {
    let prefix = code.split(':').next().unwrap_or("");
    match prefix {
        "d7" | "k" => LampId::K,
        "a" => LampId::A,
        "b" => LampId::B,
        "c" => LampId::C,
        "d" => LampId::D,
        "e" => LampId::E,
        "f" => LampId::F,
        "g" => LampId::G,
        _ => LampId::F,
    }
}

/// The eight lamps of `verdict`, in [`LAMPS`] order.
#[must_use]
pub fn lamps_from_gate(verdict: &GateResult) -> Vec<Lamp> {
    let waived: &[Reason] = if verdict.profile == Some(Profile::Yolo) {
        verdict.waived_reasons.as_deref().unwrap_or(&[])
    } else {
        &[]
    };
    let analysed = verdict.metrics.sections.is_some();

    LAMPS
        .iter()
        .map(|&id| {
            let mut reasons: Vec<Reason> = verdict
                .reasons
                .iter()
                .filter(|reason| lamp_of(reason.code.as_str()) == id)
                .cloned()
                .collect();
            let own_waived: Vec<Reason> = waived
                .iter()
                .filter(|reason| lamp_of(reason.code.as_str()) == id)
                .cloned()
                .collect();
            let state = if !reasons.is_empty() {
                LampState::Fail
            } else if !own_waived.is_empty() {
                LampState::Waived
            } else if analysed || id == LampId::F {
                LampState::Ok
            } else {
                LampState::Unknown
            };
            reasons.extend(own_waived);
            Lamp {
                id,
                state,
                reasons,
            }
        })
        .collect()
}

/// The displayed value of one criterion: `None` is "not measured", never an
/// invented number.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CriterionValue {
    /// The curve's measured value, formatted.
    pub value: Option<String>,
    /// The envelope limit it is compared with, formatted.
    pub limit: Option<String>,
}

/// The criterion display values of one verdict, indexed by
/// [`LampId::index`]. Presentation only: the same verdict the lamps read,
/// never a second gate.
#[must_use]
pub fn criteria_values(verdict: Option<&GateResult>) -> [CriterionValue; 8] {
    let mut values = std::array::from_fn(|_| CriterionValue::default());
    let Some(verdict) = verdict else {
        return values;
    };
    let metrics = &verdict.metrics;

    // a: largest share of the coefficient limit used by any section word.
    if let Some(sections) = &metrics.sections {
        let mut worst: Option<f64> = None;
        for section in sections {
            for (index, &word) in section.words.iter().enumerate() {
                let Some(&limit) = COEFFICIENT_ABS_WORDS.get(index) else {
                    continue;
                };
                let limit = limit as f64;
                if limit <= 0.0 {
                    continue;
                }
                let share = (f64::from(word).abs() / limit) * 100.0;
                if share.is_finite() {
                    worst = Some(worst.map_or(share, |current: f64| current.max(share)));
                }
            }
        }
        if let Some(share) = worst {
            values[LampId::A.index()] = CriterionValue {
                value: Some(format!("{share:.1} %")),
                limit: Some("100 %".to_string()),
            };
        }

        // b: largest pole radius.
        let radius = largest(sections.iter().map(|section| section.pole_radius));
        if let Some(radius) = radius {
            values[LampId::B.index()] = CriterionValue {
                value: Some(format!("r {radius:.6}")),
                limit: Some(format!("{POLE_RADIUS:.6}")),
            };
        }

        // c: largest single-section full peak.
        let peak = largest(sections.iter().map(|section| section.full_peak.db));
        if let Some(peak) = peak {
            values[LampId::C.index()] = CriterionValue {
                value: Some(format!("{peak:.2} dB")),
                limit: Some(format!("{SECTION_PEAK_DB:.1} dB")),
            };
        }
    }

    // d / e: the measured output peaks, when the gate got that far.
    if let Some(internal) = metrics.internal_audible
        && internal.db.is_finite()
    {
        values[LampId::D.index()] = CriterionValue {
            value: Some(format!("{:.2} dB", internal.db)),
            limit: Some(format!("{INTERNAL_AUDIBLE_DB:.2} dB")),
        };
    }
    if let Some(output) = metrics.output_audible
        && output.db.is_finite()
    {
        values[LampId::E.index()] = CriterionValue {
            value: Some(format!("{:.2} dB", output.db)),
            limit: Some(format!("{OUTPUT_AUDIBLE_DB:.2} dB")),
        };
    }

    // f: the CRC of the exact bytes.
    if let Some(crc) = &metrics.payload_crc {
        values[LampId::F.index()] = CriterionValue {
            value: Some(format!("CRC {crc}")),
            limit: None,
        };
    }

    // g: the requested preamp against its range.
    if let Some(preamp) = metrics.preamp_db
        && preamp.is_finite()
    {
        values[LampId::G.index()] = CriterionValue {
            value: Some(format!("{preamp:.1} dB")),
            limit: Some(format!("{PREAMP_MIN_DB:.1} … {PREAMP_MAX_DB:.1} dB")),
        };
    }

    // k has no single number to show.
    values
}

fn largest(values: impl Iterator<Item = f64>) -> Option<f64> {
    values
        .filter(|value| value.is_finite())
        .fold(None, |worst: Option<f64>, value| {
            Some(worst.map_or(value, |current| current.max(value)))
        })
}

/// Whether the curve uses kinds the official page never wrote (HS, LP, HP).
#[must_use]
pub fn has_experimental_kinds(bands: &[Band]) -> bool {
    bands.iter().any(|band| {
        matches!(
            band.kind(),
            BandKind::Hs | BandKind::Lp | BandKind::Hp
        )
    })
}

/// Whether the curve has a section above 20 kHz: the official page never
/// wrote one either, so a write asks for the same confirmation.
#[must_use]
pub fn has_ultrasonic_bands(bands: &[Band]) -> bool {
    bands.iter().any(|band| is_ultrasonic(*band))
}

/// The confirmation text, extended when the curve also reaches above 20 kHz.
#[must_use]
pub fn experimental_text(ultrasonic: bool) -> String {
    if ultrasonic {
        format!("{EXPERIMENTAL_TEXT} {ULTRASONIC_TEXT}")
    } else {
        EXPERIMENTAL_TEXT.to_string()
    }
}

/// The tickable confirmations. Every field is a user's word, never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Confirmations {
    /// The volume was lowered before writing.
    pub volume_lowered: bool,
    /// The cable will not be disconnected during the write.
    pub keep_connected: bool,
    /// The experimental filter types were accepted.
    pub experimental: bool,
    /// The YOLO session phrase was confirmed (YOLO sessions only).
    pub yolo_session: bool,
    /// The per-write YOLO confirmation (YOLO only, and every write resets it).
    pub yolo_write: bool,
}

/// One line of the checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemId {
    /// The gate approved the curve.
    Gate,
    /// Volume is down.
    Volume,
    /// The cable stays connected.
    Connection,
    /// Experimental types accepted (only required when the curve has them).
    Experimental,
    /// The 2 s flash spacing elapsed.
    Spacing,
    /// YOLO session confirmed (YOLO only).
    YoloSession,
    /// This write confirmed under YOLO (YOLO only).
    YoloWrite,
}

impl ItemId {
    /// The item's stable id, as the UI labels it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ItemId::Gate => "gate",
            ItemId::Volume => "volume",
            ItemId::Connection => "connection",
            ItemId::Experimental => "experimental",
            ItemId::Spacing => "spacing",
            ItemId::YoloSession => "yolo-session",
            ItemId::YoloWrite => "yolo-write",
        }
    }
}

/// One checklist line: whether it must be satisfied, and whether it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChecklistItem {
    /// Which line.
    pub id: ItemId,
    /// Whether `can_write` demands it.
    pub required: bool,
    /// Whether it is satisfied right now.
    pub satisfied: bool,
}

/// What the checklist needs from the world: the flash state, the clock, and
/// the journal's memory of the last write.
#[derive(Debug, Clone, Copy)]
pub struct ChecklistContext<'a> {
    /// The persisted (or fresh) flash counters and cooldown anchor.
    pub flash: &'a FlashState,
    /// The injected "now", epoch ms.
    pub now_ms: i64,
    /// `level_db` of the newest journaled `Ok` write, if any.
    pub last_written_level_db: Option<f64>,
    /// Status of the newest journal entry, if any.
    pub last_journal_status: Option<crate::journal::JournalStatus>,
}

/// What is being judged and what the user confirmed.
#[derive(Debug, Clone, Copy)]
pub struct ChecklistInput<'a> {
    /// The curve under edit.
    pub bands: &'a [Band],
    /// Its preamp (dB).
    pub preamp_db: f64,
    /// Which limits judge it.
    pub profile: Profile,
    /// Whether the gate ran with the engineering layer (kind-based option of
    /// the *gate*; distinct from the `Confirmations::experimental` tick).
    pub experimental: bool,
    /// The user's ticks.
    pub confirmations: Confirmations,
}

/// Everything every screen shows about writing.
#[derive(Debug, Clone, PartialEq)]
pub struct ChecklistResult {
    /// The profile the verdict was judged under.
    pub profile: Profile,
    /// `verdict.ok`, for convenience.
    pub gate_ok: bool,
    /// The eight lamps, in [`LAMPS`] order.
    pub lamps: Vec<Lamp>,
    /// The eight criterion values, indexed by [`LampId::index`].
    pub criteria: [CriterionValue; 8],
    /// The payload CRC (`0x` + hex) when the codec accepted the curve.
    pub crc: Option<String>,
    /// "Predicted level 300 Hz–3 kHz" of this curve.
    pub new_level_db: Option<f64>,
    /// The same measurement of the newest journaled `Ok` write.
    pub last_written_level_db: Option<f64>,
    /// The jump/unknown-state warning.
    pub loudness: Loudness,
    /// Whether the experimental confirmation is on the list.
    pub experimental_required: bool,
    /// The 2 s flash spacing decision.
    pub spacing: Spacing,
    /// Session / today / in total.
    pub counters: Counters,
    /// Whether today reached the endurance notice threshold.
    pub daily_notice: bool,
    /// The checklist lines, in display order.
    pub items: Vec<ChecklistItem>,
    /// Every required item satisfied — the only gate for arming a write.
    pub can_write: bool,
}

impl ChecklistResult {
    /// The item with this id, when it is on the list.
    #[must_use]
    pub fn item(&self, id: ItemId) -> Option<&ChecklistItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Whether a required item is still unsatisfied (the arm is locked).
    pub fn missing(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.items
            .iter()
            .filter(|item| item.required && !item.satisfied)
            .map(|item| item.id)
    }
}

/// Evaluates the checklist for one curve.
///
/// `verdict` may be passed to reuse a verdict computed for the **same
/// input** (display only — a write always re-evaluates); a verdict of another
/// profile is ignored and [`gate_for_write`] runs again, because a YOLO
/// verdict never approves a standard write nor stands in for one.
#[must_use]
pub fn evaluate_checklist(
    input: &ChecklistInput<'_>,
    context: &ChecklistContext<'_>,
    verdict: Option<&GateResult>,
) -> ChecklistResult {
    let yolo = input.profile == Profile::Yolo;
    let fresh;
    let verdict: &GateResult = match verdict {
        Some(existing) if (existing.profile == Some(Profile::Yolo)) == yolo => existing,
        _ => {
            fresh = gate_for_write(
                input.bands,
                input.preamp_db,
                GateOptions::for_write(input.experimental, input.profile),
            );
            &fresh
        }
    };

    let lamps = lamps_from_gate(verdict);
    let criteria = criteria_values(Some(verdict));
    let crc = verdict.metrics.payload_crc.clone();

    // The level is measured on the frozen copy the verdict judged, so the
    // number belongs to exactly the curve the gate approved.
    let new_level_db = verdict
        .metrics
        .bands
        .as_deref()
        .and_then(|bands| level_db(bands, input.preamp_db).ok())
        .filter(|level| level.is_finite());

    let loudness = loudness_warning(LoudnessInput {
        new_level_db,
        last_written_level_db: context.last_written_level_db,
        session_write_count: context.flash.session,
        last_journal_status: context.last_journal_status,
    });

    // The confirmation covers HS/LP/HP and any section above 20 kHz; the
    // gate's own experimental option stays kind-based.
    let snapshot: &[Band] = verdict.metrics.bands.as_deref().unwrap_or(input.bands);
    let experimental_required =
        has_experimental_kinds(snapshot) || has_ultrasonic_bands(snapshot);
    let spacing = can_write_now(context.flash, context.now_ms);
    let confirmations = input.confirmations;

    let items = vec![
        ChecklistItem {
            id: ItemId::Gate,
            required: true,
            satisfied: verdict.ok,
        },
        ChecklistItem {
            id: ItemId::Volume,
            required: true,
            satisfied: confirmations.volume_lowered,
        },
        ChecklistItem {
            id: ItemId::Connection,
            required: true,
            satisfied: confirmations.keep_connected,
        },
        ChecklistItem {
            id: ItemId::Experimental,
            required: experimental_required,
            satisfied: !experimental_required || confirmations.experimental,
        },
        ChecklistItem {
            id: ItemId::Spacing,
            required: true,
            satisfied: spacing.ok,
        },
        ChecklistItem {
            id: ItemId::YoloSession,
            required: yolo,
            satisfied: !yolo || confirmations.yolo_session,
        },
        ChecklistItem {
            id: ItemId::YoloWrite,
            required: yolo,
            satisfied: !yolo || confirmations.yolo_write,
        },
    ];
    let can_write = items
        .iter()
        .all(|item| !item.required || item.satisfied);

    ChecklistResult {
        profile: input.profile,
        gate_ok: verdict.ok,
        lamps,
        criteria,
        crc,
        new_level_db,
        last_written_level_db: context.last_written_level_db,
        loudness,
        experimental_required,
        spacing,
        counters: counters(context.flash, context.now_ms),
        daily_notice: daily_notice(context.flash, context.now_ms),
        items,
        can_write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auri_codec::NEUTRAL_BANDS;
    use auri_gate::gate;

    fn context<'a>(flash: &'a FlashState, now_ms: i64) -> ChecklistContext<'a> {
        ChecklistContext {
            flash,
            now_ms,
            last_written_level_db: None,
            last_journal_status: None,
        }
    }

    fn input<'a>(bands: &'a [Band], confirmations: Confirmations) -> ChecklistInput<'a> {
        ChecklistInput {
            bands,
            preamp_db: -0.5,
            profile: Profile::Standard,
            experimental: false,
            confirmations,
        }
    }

    fn spaced(now: i64) -> FlashState {
        crate::flash::observe_flash_state(&FlashState::default(), now)
    }

    #[test]
    fn lamp_of_maps_every_reason_code() {
        use auri_gate::ReasonCode;
        let all = [
            ReasonCode::Codec,
            ReasonCode::KindNotAllowed,
            ReasonCode::PkBoostAbove16k,
            ReasonCode::BoostQ,
            ReasonCode::BoostAbove20k,
            ReasonCode::Preamp,
            ReasonCode::CoefficientB0,
            ReasonCode::CoefficientB2,
            ReasonCode::CoefficientMinusA1,
            ReasonCode::CoefficientB1,
            ReasonCode::CoefficientMinusA2,
            ReasonCode::PoleRadius,
            ReasonCode::SectionPeak,
            ReasonCode::PrefixPeak,
            ReasonCode::PrefixPeakFull,
            ReasonCode::PrefixPeakExtended,
            ReasonCode::OutputAudible,
            ReasonCode::OutputExtended,
            ReasonCode::OutputFull,
        ];
        assert_eq!(all.len(), 19, "the gate has 19 codes");
        for code in all {
            let expected = match code.as_str().split(':').next().unwrap_or("") {
                "d7" | "k" => LampId::K,
                "a" => LampId::A,
                "b" => LampId::B,
                "c" => LampId::C,
                "d" => LampId::D,
                "e" => LampId::E,
                "f" => LampId::F,
                "g" => LampId::G,
                _ => LampId::F,
            };
            assert_eq!(lamp_of(code.as_str()), expected, "{}", code.as_str());
        }
        assert_eq!(lamp_of("d7:boost-q"), LampId::K, "d7 shares k");
        assert_eq!(lamp_of("k:kind-not-allowed"), LampId::K);
        assert_eq!(lamp_of("future:whatever"), LampId::F, "unknown → f");
        assert_eq!(lamp_of(""), LampId::F);
    }

    #[test]
    fn the_neutral_curve_lights_every_lamp_ok() {
        let verdict = gate(&NEUTRAL_BANDS, -0.5, GateOptions::default());
        let lamps = lamps_from_gate(&verdict);
        assert_eq!(lamps.len(), 8);
        assert!(
            lamps.iter().all(|lamp| lamp.state == LampState::Ok),
            "neutral is green everywhere: {lamps:?}"
        );
        assert!(lamps.iter().all(|lamp| lamp.reasons.is_empty()));
    }

    #[test]
    fn a_failed_lamp_carries_its_reason_and_an_unanalysed_one_is_unknown() {
        // Preamp out of range: g fails, the analysis lamps were still made.
        let verdict = gate(&NEUTRAL_BANDS, -0.2, GateOptions::default());
        assert!(!verdict.ok);
        let lamps = lamps_from_gate(&verdict);
        let g = &lamps[LampId::G.index()];
        assert_eq!(g.state, LampState::Fail);
        assert!(
            g.reasons.iter().any(|r| r.code.as_str() == "g:preamp"),
            "{:?}",
            g.reasons
        );
        assert_eq!(lamps[LampId::A.index()].state, LampState::Ok);
        assert_eq!(lamps[LampId::F.index()].state, LampState::Ok, "f always answers");

        // A wrong band count stops before any analysis: every lamp but f is
        // unknown, and f fails with the codec refusal it carries.
        let short = &NEUTRAL_BANDS[..9];
        let early = gate(short, -0.5, GateOptions::default());
        assert!(!early.ok);
        assert!(early.metrics.sections.is_none());
        let lamps = lamps_from_gate(&early);
        for lamp in &lamps {
            if lamp.id == LampId::F {
                assert_eq!(lamp.state, LampState::Fail, "f carries f:codec");
                assert!(
                    lamp.reasons.iter().any(|r| r.code.as_str() == "f:codec"),
                    "{:?}",
                    lamp.reasons
                );
            } else {
                assert_eq!(lamp.state, LampState::Unknown, "{:?}", lamp.id);
            }
        }
    }

    #[test]
    fn yolo_marks_waived_lamps_amber() {
        let verdict = gate(
            &NEUTRAL_BANDS,
            -26.0,
            GateOptions {
                profile: Profile::Yolo,
                ..GateOptions::default()
            },
        );
        assert!(verdict.ok, "{:?}", verdict.reasons);
        let lamps = lamps_from_gate(&verdict);
        let g = &lamps[LampId::G.index()];
        assert_eq!(g.state, LampState::Waived, "g was waived, not passed");
        assert!(!g.reasons.is_empty());
        assert!(lamps
            .iter()
            .filter(|lamp| lamp.id != LampId::G)
            .all(|lamp| lamp.state == LampState::Ok));

        // The same curve under the standard profile fails g outright.
        let standard = gate(&NEUTRAL_BANDS, -26.0, GateOptions::default());
        let lamps = lamps_from_gate(&standard);
        assert_eq!(lamps[LampId::G.index()].state, LampState::Fail, "class 1 never waives");
    }

    #[test]
    fn criteria_of_the_neutral_curve_read_the_same_verdict() {
        let verdict = gate(&NEUTRAL_BANDS, -0.5, GateOptions::default());
        let values = criteria_values(Some(&verdict));

        // a: the identity word over its limit, one decimal percent.
        let a = &values[LampId::A.index()];
        let expected_share =
            ((1 << 27) as f64 / COEFFICIENT_ABS_WORDS[0] as f64) * 100.0;
        let shown = a
            .value
            .as_deref()
            .and_then(|text| text.strip_suffix(" %"))
            .and_then(|text| text.parse::<f64>().ok())
            .expect("a shows a percentage");
        assert!(
            (shown - expected_share).abs() < 0.05,
            "{shown} vs {expected_share}"
        );
        assert_eq!(a.limit.as_deref(), Some("100 %"));

        // b: the neutral sections have radius 0.
        let b = &values[LampId::B.index()];
        assert_eq!(b.value.as_deref(), Some("r 0.000000"), "{b:?}");
        let radius_limit = format!("{POLE_RADIUS:.6}");
        assert_eq!(b.limit.as_deref(), Some(radius_limit.as_str()));

        // f: the CRC, exactly as the gate spells it.
        let f = &values[LampId::F.index()];
        assert_eq!(f.value.as_deref(), Some("CRC 0x4fff"), "{f:?}");
        assert_eq!(f.limit, None);

        // g: the preamp and its range.
        let g = &values[LampId::G.index()];
        assert_eq!(g.value.as_deref(), Some("-0.5 dB"));
        assert_eq!(g.limit.as_deref(), Some("-25.5 … -0.5 dB"));

        // k has no number.
        assert_eq!(values[LampId::K.index()], CriterionValue::default());
    }

    #[test]
    fn no_verdict_means_no_numbers() {
        let values = criteria_values(None);
        assert!(
            values.iter().all(|value| value.value.is_none() && value.limit.is_none()),
            "a missing metric is shown as nothing, never invented"
        );
    }

    #[test]
    fn an_early_codec_refusal_leaves_measurements_empty() {
        let verdict = gate(&NEUTRAL_BANDS[..9], -0.5, GateOptions::default());
        let values = criteria_values(Some(&verdict));
        assert_eq!(values[LampId::A.index()].value, None, "no sections were analysed");
        assert_eq!(values[LampId::B.index()].value, None);
        assert_eq!(values[LampId::C.index()].value, None);
        assert_eq!(values[LampId::D.index()].value, None);
        assert_eq!(
            values[LampId::F.index()].value,
            None,
            "a refusal before encoding has no CRC to show"
        );
    }

    #[test]
    fn nothing_is_writable_until_every_required_item_is_satisfied() {
        let now = 1_000_000;
        let flash = spaced(now);
        let empty = Confirmations::default();
        let result = evaluate_checklist(
            &input(&NEUTRAL_BANDS, empty),
            &context(&flash, now + 2000),
            None,
        );
        assert!(result.gate_ok, "the neutral curve passes the gate");
        assert!(!result.can_write, "confirmations are still unticked");
        assert_eq!(
            result.missing().collect::<Vec<_>>(),
            vec![ItemId::Volume, ItemId::Connection],
            "gate and spacing are already satisfied"
        );

        let ticked = Confirmations {
            volume_lowered: true,
            keep_connected: true,
            ..Confirmations::default()
        };
        let ready = evaluate_checklist(
            &input(&NEUTRAL_BANDS, ticked),
            &context(&flash, now + 2000),
            None,
        );
        assert!(ready.can_write, "every required item is satisfied");
        assert!(ready.missing().next().is_none());
    }

    #[test]
    fn spacing_is_required_and_unknown_until_the_interval_passes() {
        let now = 1_000_000;
        let flash = spaced(now);
        let ticked = Confirmations {
            volume_lowered: true,
            keep_connected: true,
            ..Confirmations::default()
        };
        let early = evaluate_checklist(
            &input(&NEUTRAL_BANDS, ticked),
            &context(&flash, now),
            None,
        );
        assert!(!early.can_write, "the first write waits the full interval");
        assert_eq!(early.spacing.wait_ms, 2000);
        assert!(early.item(ItemId::Spacing).is_some_and(|i| !i.satisfied));

        let later = evaluate_checklist(
            &input(&NEUTRAL_BANDS, ticked),
            &context(&flash, now + 2000),
            None,
        );
        assert!(later.can_write);
        assert!(later.item(ItemId::Spacing).is_some_and(|i| i.satisfied));
    }

    #[test]
    fn experimental_curves_need_the_extra_confirmation() {
        let now = 2_000_000;
        let flash = spaced(now - 2000);
        let ticked = Confirmations {
            volume_lowered: true,
            keep_connected: true,
            ..Confirmations::default()
        };
        let mut bands = NEUTRAL_BANDS.to_vec();
        bands[0] = Band::Hs {
            freq_hz: 8_000.0,
            gain_db: -3.0,
            q_factor: 0.7,
        };

        let mut input = input(&bands, ticked);
        input.experimental = true; // the gate's own layer, so the verdict stays green
        let result = evaluate_checklist(&input, &context(&flash, now), None);
        assert!(result.experimental_required);
        assert!(!result.can_write, "the experimental tick is still missing");
        assert_eq!(
            result.missing().collect::<Vec<_>>(),
            vec![ItemId::Experimental]
        );

        input.confirmations = Confirmations {
            experimental: true,
            ..ticked
        };
        let confirmed = evaluate_checklist(&input, &context(&flash, now), None);
        assert!(confirmed.can_write, "{:?}", confirmed.missing().collect::<Vec<_>>());

        // An ultrasonic section also demands it, even as a plain LS.
        let mut ultrasonic = NEUTRAL_BANDS.to_vec();
        ultrasonic[0] = Band::Ls {
            freq_hz: 24_000.0,
            gain_db: -6.0,
            q_factor: 0.5,
        };
        assert!(has_ultrasonic_bands(&ultrasonic));
        assert!(has_experimental_kinds(&bands));
        assert!(!has_experimental_kinds(&NEUTRAL_BANDS));
        assert!(!has_ultrasonic_bands(&NEUTRAL_BANDS));
    }

    #[test]
    fn yolo_adds_two_items_and_only_yolo_does() {
        let now = 3_000_000;
        let flash = spaced(now - 2000);
        let ticked = Confirmations {
            volume_lowered: true,
            keep_connected: true,
            ..Confirmations::default()
        };

        let standard_input = ChecklistInput {
            bands: &NEUTRAL_BANDS,
            preamp_db: -26.0, // refused by standard
            profile: Profile::Standard,
            experimental: false,
            confirmations: ticked,
        };
        let standard = evaluate_checklist(&standard_input, &context(&flash, now), None);
        assert!(!standard.gate_ok);
        let yolo_lines = [ItemId::YoloSession, ItemId::YoloWrite];
        for id in yolo_lines {
            assert!(
                standard
                    .item(id)
                    .is_some_and(|item| !item.required && item.satisfied),
                "standard checklists carry {id:?} as not-required, trivially satisfied"
            );
        }
        assert!(
            standard.missing().collect::<Vec<_>>() == vec![ItemId::Gate],
            "the refused gate is the only miss: {:?}",
            standard.missing().collect::<Vec<_>>()
        );

        let yolo_input = ChecklistInput {
            profile: Profile::Yolo,
            ..standard_input
        };
        let yolo = evaluate_checklist(&yolo_input, &context(&flash, now), None);
        assert!(yolo.gate_ok, "{:?}", yolo.lamps);
        assert!(!yolo.can_write, "neither yolo confirmation is ticked");
        assert_eq!(
            yolo.missing().collect::<Vec<_>>(),
            vec![ItemId::YoloSession, ItemId::YoloWrite]
        );

        let ready = Confirmations {
            yolo_session: true,
            yolo_write: true,
            ..ticked
        };
        let yolo_input = ChecklistInput {
            confirmations: ready,
            ..yolo_input
        };
        let yolo = evaluate_checklist(&yolo_input, &context(&flash, now), None);
        assert!(yolo.can_write, "{:?}", yolo.missing().collect::<Vec<_>>());
    }

    #[test]
    fn a_reused_verdict_must_have_the_same_profile() {
        let now = 4_000_000;
        let flash = spaced(now - 2000);
        // A YOLO verdict for a standard request must not be trusted: the
        // checklist runs the gate again and reports the standard refusal.
        let yolo_verdict = gate(
            &NEUTRAL_BANDS,
            -26.0,
            GateOptions {
                profile: Profile::Yolo,
                ..GateOptions::default()
            },
        );
        assert!(yolo_verdict.ok);
        let standard_input = ChecklistInput {
            bands: &NEUTRAL_BANDS,
            preamp_db: -26.0,
            profile: Profile::Standard,
            experimental: false,
            confirmations: Confirmations::default(),
        };
        let result = evaluate_checklist(&standard_input, &context(&flash, now), Some(&yolo_verdict));
        assert!(
            !result.gate_ok,
            "a yolo verdict never approves a standard write"
        );
        assert_eq!(result.profile, Profile::Standard);
    }

    #[test]
    fn counters_and_daily_notice_flow_from_the_flash_state() {
        use crate::flash::record_write_on;
        let now = 5_000_000;
        let today = crate::flash::local_date_key(now).expect("a valid clock has a date");
        let mut flash = FlashState::default();
        for _ in 0..49 {
            flash = record_write_on(&flash, now, &today).expect("real date");
        }
        let before = evaluate_checklist(
            &input(&NEUTRAL_BANDS, Confirmations::default()),
            &context(&flash, now),
            None,
        );
        assert_eq!(before.counters.day, 49);
        assert_eq!(before.counters.session, 49);
        assert_eq!(before.counters.total, 49);
        assert!(!before.daily_notice, "49 is under the threshold");

        flash = record_write_on(&flash, now, &today).expect("real date");
        let after = evaluate_checklist(
            &input(&NEUTRAL_BANDS, Confirmations::default()),
            &context(&flash, now),
            None,
        );
        assert_eq!(after.counters.day, 50);
        assert_eq!(after.counters.total, 50);
        assert!(after.daily_notice, "the 50th write of the day fires the notice");
    }

    #[test]
    fn loudness_is_part_of_the_result() {
        let now = 6_000_000;
        let flash = spaced(now - 2000);
        // No journal history at all → unknown state, warned once.
        let result = evaluate_checklist(
            &input(&NEUTRAL_BANDS, Confirmations::default()),
            &context(&flash, now),
            None,
        );
        assert!(result.loudness.warn);
        assert_eq!(
            result.loudness.reasons,
            vec![crate::loudness::LoudnessReason::UnknownState]
        );
        assert_eq!(result.last_written_level_db, None);
        let level = result.new_level_db.expect("the neutral curve has a level");
        assert!((level - (-0.5)).abs() < 0.01, "{level}");
    }
}

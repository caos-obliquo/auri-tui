//! Reasons: why a curve is refused, in the gate's deterministic order.

use serde::{Deserialize, Serialize};

/// Why a curve is refused.
///
/// The spelling is the JavaScript gate's, so verdicts diff cleanly against the
/// oracle. Reason order (deterministic, see the crate docs):
///
/// 0. `f:codec` alone when the bands cannot even be copied;
/// 1. `k:kind-not-allowed`, per stage in stage order;
/// 2. `d7:pk-boost-above-16k`, `d7:boost-q`, `d7:boost-above-20k`;
/// 3. `f:codec` (evaluation stops here);
/// 4. `g:preamp`;
/// 5. per stage: `a:<position>` in wire order, then `b:pole-radius`, then `c:section-peak`;
/// 6. `d:prefix-peak`, `d:prefix-peak-full`, `d:prefix-peak-extended`,
///    `e:output-audible`, `e:output-extended`, `e:output-full`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReasonCode {
    /// The codec refused the curve (or the bands could not be read).
    #[serde(rename = "f:codec")]
    Codec,
    /// The filter kind is not allowed in the active layer.
    #[serde(rename = "k:kind-not-allowed")]
    KindNotAllowed,
    /// Peaking boost above 16 kHz.
    #[serde(rename = "d7:pk-boost-above-16k")]
    PkBoostAbove16k,
    /// Boost with `Q` above the allowed maximum.
    #[serde(rename = "d7:boost-q")]
    BoostQ,
    /// A section above 20 kHz Engenharia would not allow.
    #[serde(rename = "d7:boost-above-20k")]
    BoostAbove20k,
    /// Preamp outside `[−25.5, −0.5]` dB.
    #[serde(rename = "g:preamp")]
    Preamp,
    /// Feedforward `b0` word beyond the observed envelope.
    #[serde(rename = "a:b0")]
    CoefficientB0,
    /// Feedforward `b2` word beyond the observed envelope.
    #[serde(rename = "a:b2")]
    CoefficientB2,
    /// Feedback `-a1` word beyond the observed envelope (hard).
    #[serde(rename = "a:-a1")]
    CoefficientMinusA1,
    /// Feedforward `b1` word beyond the observed envelope.
    #[serde(rename = "a:b1")]
    CoefficientB1,
    /// Feedback `-a2` word beyond the observed envelope (hard).
    #[serde(rename = "a:-a2")]
    CoefficientMinusA2,
    /// Quantized pole radius beyond the observed envelope (hard).
    #[serde(rename = "b:pole-radius")]
    PoleRadius,
    /// Single-section peak beyond the observed envelope.
    #[serde(rename = "c:section-peak")]
    SectionPeak,
    /// Cumulative prefix peak, 20 Hz–20 kHz (hard).
    #[serde(rename = "d:prefix-peak")]
    PrefixPeak,
    /// Cumulative prefix peak, DC–48 kHz (YOLO, hard).
    #[serde(rename = "d:prefix-peak-full")]
    PrefixPeakFull,
    /// Cumulative prefix peak, 20 Hz–48 kHz (YOLO, hard).
    #[serde(rename = "d:prefix-peak-extended")]
    PrefixPeakExtended,
    /// Output peak, 20 Hz–20 kHz (hard).
    #[serde(rename = "e:output-audible")]
    OutputAudible,
    /// Output peak, 20 Hz–48 kHz (YOLO, hard).
    #[serde(rename = "e:output-extended")]
    OutputExtended,
    /// Output peak, DC–48 kHz (hard).
    #[serde(rename = "e:output-full")]
    OutputFull,
}

impl ReasonCode {
    /// The JavaScript spelling of this code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonCode::Codec => "f:codec",
            ReasonCode::KindNotAllowed => "k:kind-not-allowed",
            ReasonCode::PkBoostAbove16k => "d7:pk-boost-above-16k",
            ReasonCode::BoostQ => "d7:boost-q",
            ReasonCode::BoostAbove20k => "d7:boost-above-20k",
            ReasonCode::Preamp => "g:preamp",
            ReasonCode::CoefficientB0 => "a:b0",
            ReasonCode::CoefficientB2 => "a:b2",
            ReasonCode::CoefficientMinusA1 => "a:-a1",
            ReasonCode::CoefficientB1 => "a:b1",
            ReasonCode::CoefficientMinusA2 => "a:-a2",
            ReasonCode::PoleRadius => "b:pole-radius",
            ReasonCode::SectionPeak => "c:section-peak",
            ReasonCode::PrefixPeak => "d:prefix-peak",
            ReasonCode::PrefixPeakFull => "d:prefix-peak-full",
            ReasonCode::PrefixPeakExtended => "d:prefix-peak-extended",
            ReasonCode::OutputAudible => "e:output-audible",
            ReasonCode::OutputExtended => "e:output-extended",
            ReasonCode::OutputFull => "e:output-full",
        }
    }
}

/// A measured or requested value carried by a [`Reason`].
///
/// Untagged, like the JavaScript `value`: a number, a string, or null (the
/// JavaScript `undefined`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReasonValue {
    /// A measured number (dB, Hz, Q or a Q27 word magnitude).
    Number(f64),
    /// A message or a kind name.
    Text(String),
    /// Absent (`undefined` in JavaScript).
    Null(()),
}

impl ReasonValue {
    /// The number this value carries, if any.
    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            ReasonValue::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// The text this value carries, if any.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ReasonValue::Text(value) => Some(value),
            _ => None,
        }
    }
}

/// One refusal: a code, the stage it belongs to (1..=10), its value and limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    /// What failed.
    pub code: ReasonCode,
    /// The band the failure belongs to (1-based); absent for curve-wide checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<u32>,
    /// The measured value.
    pub value: ReasonValue,
    /// The limit it was compared against; absent for checks without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
}

impl Reason {
    /// A reason with a numeric value.
    #[must_use]
    pub fn number(code: ReasonCode, value: f64) -> Self {
        Reason {
            code,
            stage: None,
            value: ReasonValue::Number(value),
            limit: None,
        }
    }

    /// A reason with a numeric value at a stage.
    #[must_use]
    pub fn number_at(code: ReasonCode, stage: u32, value: f64) -> Self {
        Reason {
            code,
            stage: Some(stage),
            value: ReasonValue::Number(value),
            limit: None,
        }
    }

    /// A reason with a numeric value and a limit, optionally at a stage.
    #[must_use]
    pub fn measured(code: ReasonCode, stage: Option<u32>, value: f64, limit: f64) -> Self {
        Reason {
            code,
            stage,
            value: ReasonValue::Number(value),
            limit: Some(limit),
        }
    }

    /// A `f:codec` reason carrying the codec's message.
    #[must_use]
    pub fn codec(message: impl Into<String>) -> Self {
        Reason {
            code: ReasonCode::Codec,
            stage: None,
            value: ReasonValue::Text(message.into()),
            limit: None,
        }
    }
}

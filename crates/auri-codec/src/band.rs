//! The ten filter sections a curve is made of.

/// Number of biquad sections the firmware stores.
pub const BAND_COUNT: usize = 10;

/// Supported filter kinds.
///
/// `OFF` is an exact pass-through section (wire words `[1.0, 0, 0, 0, 0]`,
/// which in Q27 are `[0x08000000, 0, 0, 0, 0]`); prefer it to a 0 dB peaking
/// band, which still runs poles close to the unit circle.
pub const FILTER_KINDS: [BandKind; 6] = [
    BandKind::Pk,
    BandKind::Ls,
    BandKind::Hs,
    BandKind::Lp,
    BandKind::Hp,
    BandKind::Off,
];

/// An exact pass-through section.
pub const OFF: Band = Band::Off;

/// Ten exact pass-through sections: the known-good neutral preset.
pub const NEUTRAL_BANDS: [Band; BAND_COUNT] = [OFF; BAND_COUNT];

/// The filter shape of a [`Band`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BandKind {
    /// Peaking EQ (gain).
    Pk,
    /// Low shelf (gain).
    Ls,
    /// High shelf (gain).
    Hs,
    /// Second-order low pass.
    Lp,
    /// Second-order high pass.
    Hp,
    /// Exact pass-through.
    Off,
}

impl BandKind {
    /// Whether this shape takes a `gainDb` field.
    ///
    /// `PK`/`LS`/`HS` do; `LP`/`HP` do not; `OFF` carries no fields at all.
    pub const fn has_gain(self) -> bool {
        matches!(self, BandKind::Pk | BandKind::Ls | BandKind::Hs)
    }

    /// The wire spelling of the kind, as the JavaScript codec spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            BandKind::Pk => "PK",
            BandKind::Ls => "LS",
            BandKind::Hs => "HS",
            BandKind::Lp => "LP",
            BandKind::Hp => "HP",
            BandKind::Off => "OFF",
        }
    }
}

/// One biquad section of the curve.
///
/// `freq_hz` is in Hz at [`crate::SAMPLE_RATE`], `gain_db` in dB, `q_factor`
/// unitless. `LP`/`HP` carry no gain.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "UPPERCASE", rename_all_fields = "camelCase")]
pub enum Band {
    /// Peaking EQ.
    Pk {
        /// Centre frequency in Hz.
        freq_hz: f64,
        /// Gain in dB (may be negative).
        gain_db: f64,
        /// Quality factor, must be > 0.
        q_factor: f64,
    },
    /// Low shelf.
    Ls {
        /// Corner frequency in Hz.
        freq_hz: f64,
        /// Gain in dB.
        gain_db: f64,
        /// Quality factor, must be > 0.
        q_factor: f64,
    },
    /// High shelf.
    Hs {
        /// Corner frequency in Hz.
        freq_hz: f64,
        /// Gain in dB.
        gain_db: f64,
        /// Quality factor, must be > 0.
        q_factor: f64,
    },
    /// Second-order low pass.
    Lp {
        /// Cutoff frequency in Hz.
        freq_hz: f64,
        /// Quality factor, must be > 0.
        q_factor: f64,
    },
    /// Second-order high pass.
    Hp {
        /// Cutoff frequency in Hz.
        freq_hz: f64,
        /// Quality factor, must be > 0.
        q_factor: f64,
    },
    /// Exact pass-through section.
    Off,
}

impl Band {
    /// The kind of this band.
    pub const fn kind(self) -> BandKind {
        match self {
            Band::Pk { .. } => BandKind::Pk,
            Band::Ls { .. } => BandKind::Ls,
            Band::Hs { .. } => BandKind::Hs,
            Band::Lp { .. } => BandKind::Lp,
            Band::Hp { .. } => BandKind::Hp,
            Band::Off => BandKind::Off,
        }
    }

    /// Frequency in Hz, or `None` for [`Band::Off`].
    pub const fn freq_hz(self) -> Option<f64> {
        match self {
            Band::Pk { freq_hz, .. }
            | Band::Ls { freq_hz, .. }
            | Band::Hs { freq_hz, .. }
            | Band::Lp { freq_hz, .. }
            | Band::Hp { freq_hz, .. } => Some(freq_hz),
            Band::Off => None,
        }
    }

    /// Gain in dB. `LP`/`HP`/`OFF` have none and report `0.0`.
    pub const fn gain_db(self) -> f64 {
        match self {
            Band::Pk { gain_db, .. } | Band::Ls { gain_db, .. } | Band::Hs { gain_db, .. } => {
                gain_db
            }
            Band::Lp { .. } | Band::Hp { .. } | Band::Off => 0.0,
        }
    }

    /// Quality factor, or `None` for [`Band::Off`].
    pub const fn q_factor(self) -> Option<f64> {
        match self {
            Band::Pk { q_factor, .. }
            | Band::Ls { q_factor, .. }
            | Band::Hs { q_factor, .. }
            | Band::Lp { q_factor, .. }
            | Band::Hp { q_factor, .. } => Some(q_factor),
            Band::Off => None,
        }
    }
}

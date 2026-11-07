//! Loading a curve from disk and turning it into wire reports.

use std::path::Path;

use auri_codec::{BAND_COUNT, Band, encode_reports};
use serde::Deserialize;

/// A curve in memory: ten validated bands plus the preamp that precedes them.
#[derive(Debug, Clone)]
pub struct CurveFile {
    /// Preamp in dB, finite and `<= 0`.
    pub preamp_db: f64,
    /// Exactly ten bands, in the order they must reach the wire.
    pub eq_bands: Vec<Band>,
}

/// `preampDb` / `freqHz` / `gainDb` / `qFactor` on disk, Rust snake_case in
/// memory.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCurve {
    #[serde(default)]
    preamp_db: f64,
    eq_bands: Vec<Band>,
}

impl CurveFile {
    /// The neutral curve: ten pass-through sections, no preamp.
    #[must_use]
    pub fn neutral() -> Self {
        CurveFile {
            preamp_db: 0.0,
            eq_bands: auri_codec::NEUTRAL_BANDS.to_vec(),
        }
    }

    /// Reads and parses a curve file.
    ///
    /// # Errors
    ///
    /// I/O or JSON problems, a band count other than ten, or any band the
    /// codec refuses.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let wire: WireCurve = serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        let curve = CurveFile {
            preamp_db: wire.preamp_db,
            eq_bands: wire.eq_bands,
        };
        curve.validate()?;
        Ok(curve)
    }

    /// Validates shape and ranges without touching the wire.
    pub fn validate(&self) -> Result<(), String> {
        if self.eq_bands.len() != BAND_COUNT {
            return Err(format!(
                "expected {BAND_COUNT} bands, got {}",
                self.eq_bands.len()
            ));
        }
        for (index, band) in self.eq_bands.iter().enumerate() {
            auri_codec::check_band(*band, index).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// The nine reports this curve encodes to.
    ///
    /// # Errors
    ///
    /// Whatever the codec refuses.
    pub fn encode(&self) -> Result<Vec<[u8; auri_codec::REPORT_SIZE]>, String> {
        encode_reports(&self.eq_bands, self.preamp_db).map_err(|e| e.to_string())
    }
}

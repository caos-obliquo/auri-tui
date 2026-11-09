//! Exporting a curve for other tools: Equalizer APO and PipeWire.
//!
//! Format writing, not sound design: the numbers are the curve's, the
//! spelling is each host's. Off bands stay in the output so band *N* means
//! the same band in every tool. Offline by design — no HID, no state files.

use auri_codec::Band;

use crate::curve::CurveFile;

/// Whether the export targets Equalizer APO or PipeWire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    /// Equalizer APO's `Preamp:` / `Filter:` text.
    Apo,
    /// A `key = value` listing per band.
    Pipewire,
}

/// Renders the curve in the requested host format.
#[must_use]
pub fn render(curve: &CurveFile, format: ExportFormat) -> String {
    match format {
        ExportFormat::Apo => apo(curve),
        ExportFormat::Pipewire => pipewire(curve),
    }
}

/// Equalizer APO config text: `Preamp:` then one `Filter:` line per band.
#[must_use]
fn apo(curve: &CurveFile) -> String {
    let mut text = format!("Preamp: {:.1} dB\n", curve.preamp_db);
    for (index, band) in curve.eq_bands.iter().enumerate() {
        text.push_str(&apo_filter(index + 1, band));
        text.push('\n');
    }
    text
}

#[must_use]
fn apo_filter(number: usize, band: &Band) -> String {
    let kind = band.kind();
    if kind.as_str() == "OFF" {
        return format!("Filter {number}: OFF");
    }
    let mut line = format!(
        "Filter {number}: ON {} Fc {:.0} Hz",
        kind.as_str(),
        band.freq_hz().unwrap_or(0.0)
    );
    if kind.has_gain() {
        line.push_str(&format!(" Gain {:.1} dB", band.gain_db()));
    }
    line.push_str(&format!(" Q {:.2}", band.q_factor().unwrap_or(0.0)));
    line
}

/// A PipeWire-style key/value listing, one block per band.
#[must_use]
fn pipewire(curve: &CurveFile) -> String {
    let mut text = format!("preamp = {:.1}\n", curve.preamp_db);
    for (index, band) in curve.eq_bands.iter().enumerate() {
        let n = index + 1;
        text.push_str(&format!(
            "band.{n}.type = {}\n",
            band.kind().as_str().to_lowercase()
        ));
        if let Some(freq) = band.freq_hz() {
            text.push_str(&format!("band.{n}.frequency = {freq:.0}\n"));
        }
        if band.kind().has_gain() {
            text.push_str(&format!("band.{n}.gain = {:.1}\n", band.gain_db()));
        }
        if let Some(q) = band.q_factor() {
            text.push_str(&format!("band.{n}.q = {q:.2}\n"));
        }
        text.push_str(&format!(
            "band.{n}.enabled = {}\n",
            band.kind().as_str() != "OFF"
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveFile;

    #[test]
    fn apo_output_has_a_preamp_and_one_filter_per_band() {
        let curve = CurveFile::neutral();
        let text = render(&curve, ExportFormat::Apo);
        assert!(text.starts_with("Preamp: -0.5 dB"), "preamp line: {text}");
        assert_eq!(text.matches("Filter ").count(), 10, "{text}");
        assert!(
            text.contains("Filter 1: OFF"),
            "neutral bands export as OFF: {text}"
        );
    }

    #[test]
    fn pipewire_output_is_key_value_pairs() {
        let curve = CurveFile::neutral();
        let text = render(&curve, ExportFormat::Pipewire);
        assert!(text.starts_with("preamp = -0.5"), "{text}");
        assert!(text.contains("band.1.type = off"), "{text}");
        assert!(text.contains("band.10.enabled = false"), "{text}");
    }

    #[test]
    fn active_bands_carry_their_numbers() {
        let curve = crate::presets::PRESETS[0].curve(); // "padrao"
        let text = render(&curve, ExportFormat::Apo);
        assert!(text.contains("Preamp: -2.8 dB"), "{text}");
        assert!(
            text.contains("Filter 1: ON PK Fc 25 Hz Gain -0.8 dB Q 0.30"),
            "first band: {text}"
        );
    }
}

//! The magnitude response of the current curve, drawn as braille dots.
//!
//! The curve is the sum of the ten *quantized* sections — the words the chip
//! would actually receive, not the ideal RBJ coefficients — plus the preamp, so
//! what you see is what the gate measured.

use auri_codec::{Band, Section, design_section, encode_section};
use auri_gate::{from_words, section_db};

/// Left edge of the plot, in Hz.
pub const PLOT_MIN_HZ: f64 = 20.0;

/// Right edge of the plot, in Hz.
pub const PLOT_MAX_HZ: f64 = 20_000.0;

/// One sampled point: `(frequency in Hz, level in dB)`.
pub type Sample = (f64, f64);

/// The quantized section for a band: `OFF` and every other band go through the
/// same encoder the wire uses, so the plot cannot disagree with the payload.
#[must_use]
pub fn quantized_section(band: Band) -> Section {
    design_section(band)
        .ok()
        .and_then(|section| encode_section(section, 0).ok())
        .map(from_words)
        .unwrap_or_else(Section::identity)
}

/// Samples the response `points` times on a logarithmic scale between
/// [`PLOT_MIN_HZ`] and [`PLOT_MAX_HZ`], both endpoints included.
#[must_use]
pub fn sample(bands: &[Band], preamp_db: f64, points: usize) -> Vec<Sample> {
    let sections: Vec<Section> = bands.iter().copied().map(quantized_section).collect();
    let points = points.max(2);
    let decades = (PLOT_MAX_HZ / PLOT_MIN_HZ).ln();
    let mut out = Vec::with_capacity(points);
    for index in 0..points {
        let t = f64::from(index as u32) / f64::from((points - 1) as u32);
        let hz = PLOT_MIN_HZ * (decades * t).exp();
        let mut db = preamp_db;
        for section in &sections {
            db += section_db(*section, hz);
        }
        out.push((hz, db));
    }
    out
}

/// A rendered braille bitmap: one `char` per terminal cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Braille {
    /// Characters per row.
    pub width: usize,
    /// Rows.
    pub height: usize,
    /// `width * height` characters, row-major.
    pub cells: Vec<char>,
}

impl Braille {
    /// The character at `(column, row)`, or a space when out of bounds.
    #[must_use]
    pub fn at(&self, column: usize, row: usize) -> char {
        if column >= self.width || row >= self.height {
            return ' ';
        }
        self.cells[row * self.width + column]
    }

    /// The rows as strings, ready to be turned into [`ratatui::text::Line`]s.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        (0..self.height)
            .map(|row| (0..self.width).map(|column| self.at(column, row)).collect())
            .collect()
    }
}

/// The dot bit of `(column, row)` inside a braille cell, per Unicode U+2800.
/// Row 3 is the bottom row of the cell, which the block places at bits 6/7.
const BRAILLE_BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// Draws `values` into a `width` × `height` braille grid, mapping `hi` to the
/// top row and `lo` to the bottom row. Values outside `[lo, hi]` are clamped
/// onto the edge, so a runaway curve still draws instead of wrapping.
#[must_use]
pub fn braille(values: &[f64], width: usize, height: usize, lo: f64, hi: f64) -> Braille {
    let width = width.max(1);
    let height = height.max(1);
    let dot_width = width * 2;
    let dot_height = height * 4;
    let mut dots = vec![0u8; dot_width * dot_height];

    let span = if hi > lo { hi - lo } else { 1.0 };
    for (index, &db) in values.iter().enumerate() {
        let column = if values.len() > 1 {
            index * (dot_width - 1) / (values.len() - 1)
        } else {
            0
        };
        let ratio = ((hi - db) / span).clamp(0.0, 1.0);
        let row = (ratio * f64::from((dot_height - 1) as u32)).round() as usize;
        dots[row.min(dot_height - 1) * dot_width + column.min(dot_width - 1)] = 1;
    }

    let mut cells = Vec::with_capacity(width * height);
    for cell_row in 0..height {
        for cell_column in 0..width {
            let mut bits = 0u8;
            for (dot_row, row_bits) in BRAILLE_BITS.iter().enumerate() {
                for (dot_column, &bit) in row_bits.iter().enumerate() {
                    let x = cell_column * 2 + dot_column;
                    let y = cell_row * 4 + dot_row;
                    if dots[y * dot_width + x] == 1 {
                        bits |= bit;
                    }
                }
            }
            cells.push(char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' '));
        }
    }

    Braille {
        width,
        height,
        cells,
    }
}

/// The plot's dB window: wide enough to read a flat curve, padded so nothing
/// touches the frame.
#[must_use]
pub fn window(values: &[f64]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &db in values {
        lo = lo.min(db);
        hi = hi.max(db);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (-6.0, 6.0);
    }
    let mut lo = lo.min(0.0) - 2.0;
    let mut hi = hi.max(0.0) + 2.0;
    if hi - lo < 12.0 {
        let middle = (hi + lo) / 2.0;
        lo = middle - 6.0;
        hi = middle + 6.0;
    }
    (lo, hi)
}

/// The row index (0-based, from the top) that holds 0 dB, or `None` when the
/// window does not include it.
#[must_use]
pub fn zero_row(lo: f64, hi: f64, height: usize) -> Option<usize> {
    if height == 0 || lo > 0.0 || hi < 0.0 {
        return None;
    }
    let ratio = (hi - 0.0) / (hi - lo);
    let row = (ratio * f64::from((height * 4 - 1) as u32)).round() as usize;
    Some((row / 4).min(height - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auri_codec::{BAND_COUNT, Band, NEUTRAL_BANDS};

    fn peaks(bands: &[Band]) -> f64 {
        bands.iter().map(|b| b.gain_db()).sum()
    }

    #[test]
    fn a_flat_curve_is_flat_and_sits_at_the_preamp() {
        let samples = sample(&NEUTRAL_BANDS, -0.5, 64);
        assert_eq!(samples.len(), 64);
        for (hz, db) in &samples {
            assert!(*hz >= PLOT_MIN_HZ && *hz <= PLOT_MAX_HZ);
            assert!((db - -0.5).abs() < 1e-9, "flat at {hz} Hz is {db} dB");
        }
    }

    #[test]
    fn a_peaking_band_lifts_the_middle_of_the_plot() {
        let mut bands = NEUTRAL_BANDS;
        bands[0] = Band::Pk {
            freq_hz: 1_000.0,
            gain_db: 6.0,
            q_factor: 1.0,
        };
        let samples = sample(&bands, -0.5, 200);
        let at_1k = samples
            .iter()
            .min_by(|a, b| (a.0 - 1_000.0).abs().total_cmp(&(b.0 - 1_000.0).abs()))
            .expect("at least one sample");
        assert!(
            at_1k.1 > 4.0,
            "a +6 dB peak should reach ~+5.5 dB after the preamp, got {}",
            at_1k.1
        );
        assert!(peaks(&bands) > 0.0);
    }

    #[test]
    fn the_off_band_encodes_as_an_exact_pass_through() {
        let section = quantized_section(Band::Off);
        assert_eq!(section, Section::identity());
        assert_eq!(section_db(section, 1_000.0), 0.0);
    }

    #[test]
    fn braille_dots_land_inside_the_frame() {
        let art = braille(&[-3.0, 0.0, 3.0], 8, 3, -6.0, 6.0);
        assert_eq!(art.width, 8);
        assert_eq!(art.height, 3);
        assert_eq!(art.cells.len(), 24);
        assert!(art.cells.iter().any(|&c| c != ' '), "something was drawn");
        assert_eq!(art.at(0, 0), art.at(0, 0));
        assert_eq!(art.at(99, 99), ' ');
        assert_eq!(art.rows().len(), 3);
    }

    #[test]
    fn an_empty_frame_does_not_panic() {
        let art = braille(&[], 4, 2, 0.0, 1.0);
        assert_eq!(art.cells.len(), 8);
        assert_eq!(braille(&[0.0], 0, 0, 0.0, 1.0).width, 1);
    }

    #[test]
    fn the_window_always_includes_zero_and_is_readable() {
        let (lo, hi) = window(&[-0.5, -0.5]);
        assert!(lo <= 0.0 && hi >= 0.0);
        assert!(hi - lo >= 12.0);

        let (lo, hi) = window(&[-40.0, 20.0]);
        assert!(lo <= -40.0);
        assert!(hi >= 20.0);

        assert_eq!(window(&[]), (-6.0, 6.0));
    }

    #[test]
    fn zero_row_reports_where_the_axis_falls() {
        assert_eq!(zero_row(-6.0, 6.0, 4), Some(2));
        assert_eq!(zero_row(1.0, 6.0, 4), None);
        assert_eq!(zero_row(-6.0, -1.0, 4), None);
        assert_eq!(zero_row(-6.0, 6.0, 0), None);
        assert_eq!(NEUTRAL_BANDS.len(), BAND_COUNT);
    }
}

//! The magnitude response of the current curve, drawn as a line with nodes.
//!
//! The curve is the sum of the ten *quantized* sections — the words the chip
//! would actually receive, not the ideal RBJ coefficients — plus the preamp, so
//! what you see is what the gate measured.
//!
//! Two renderers live here: the line drawing, used whenever there is room for
//! it, and the braille bitmap, kept as the fallback for cramped terminals.

use auri_codec::{Band, BandKind, Section, design_section, encode_section};
use auri_gate::{from_words, section_db};

use crate::tui::icons::Icons;

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

/// The quantized sections of a curve, one per band.
#[must_use]
pub fn sections(bands: &[Band]) -> Vec<Section> {
    bands.iter().copied().map(quantized_section).collect()
}

/// The response at one frequency, in dB, preamp included.
#[must_use]
pub fn response_at(sections: &[Section], preamp_db: f64, hz: f64) -> f64 {
    let mut db = preamp_db;
    for section in sections {
        db += section_db(*section, hz);
    }
    db
}

/// Samples the response `points` times on a logarithmic scale between
/// [`PLOT_MIN_HZ`] and [`PLOT_MAX_HZ`], both endpoints included.
#[must_use]
pub fn sample(bands: &[Band], preamp_db: f64, points: usize) -> Vec<Sample> {
    let sections = sections(bands);
    let points = points.max(2);
    let decades = (PLOT_MAX_HZ / PLOT_MIN_HZ).ln();
    let mut out = Vec::with_capacity(points);
    for index in 0..points {
        let t = f64::from(index as u32) / f64::from((points - 1) as u32);
        let hz = PLOT_MIN_HZ * (decades * t).exp();
        out.push((hz, response_at(&sections, preamp_db, hz)));
    }
    out
}

/// The column that shows `hz` on a logarithmic axis.
#[must_use]
pub fn column_for(hz: f64, width: usize) -> usize {
    if width <= 1 {
        return 0;
    }
    let decades = (PLOT_MAX_HZ / PLOT_MIN_HZ).ln();
    let ratio = (hz / PLOT_MIN_HZ).ln() / decades;
    let ratio = ratio.clamp(0.0, 1.0);
    ((ratio * (width - 1) as f64).round() as usize).min(width - 1)
}

/// The frequency a column shows on a logarithmic axis.
#[must_use]
pub fn frequency_at(column: usize, width: usize) -> f64 {
    if width <= 1 {
        return PLOT_MIN_HZ;
    }
    let decades = (PLOT_MAX_HZ / PLOT_MIN_HZ).ln();
    PLOT_MIN_HZ * (decades * column as f64 / (width - 1) as f64).exp()
}

/// The row that shows `db`, counting from the top.
#[must_use]
pub fn row_for(db: f64, lo: f64, hi: f64, height: usize) -> usize {
    if height <= 1 {
        return 0;
    }
    let span = if hi > lo { hi - lo } else { 1.0 };
    let ratio = ((hi - db) / span).clamp(0.0, 1.0);
    (ratio * (height - 1) as f64).round() as usize
}

/// The dB labels of the y axis: one per whole 6 dB step inside the window.
///
/// The label sits on the row the axis puts that value on, so the axis, the
/// 0 dB reference and the curve always agree.
#[must_use]
pub fn y_labels(lo: f64, hi: f64, height: usize) -> Vec<(usize, String)> {
    let mut labels = Vec::new();
    let mut db = (lo / 6.0).ceil() * 6.0;
    while db <= hi + 1e-9 {
        labels.push((row_for(db, lo, hi, height), format_db(db)));
        db += 6.0;
    }
    labels.sort_by_key(|&(row, _)| row);
    labels
}

/// A dB label: `+12`, `-6`, or a plain `0`.
#[must_use]
pub fn format_db(db: f64) -> String {
    let rounded = db.round();
    if rounded == 0.0 {
        "0".to_string()
    } else {
        format!("{rounded:+.0}")
    }
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

/// The plot's dB window: at least ±12 dB, rounded to whole 6 dB steps so the
/// axis labels land on round numbers.
#[must_use]
pub fn window(values: &[f64]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &db in values {
        lo = lo.min(db);
        hi = hi.max(db);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (-12.0, 12.0);
    }
    let lo = (lo.min(-12.0) / 6.0).floor() * 6.0;
    let hi = (hi.max(12.0) / 6.0).ceil() * 6.0;
    (lo, hi)
}

/// The row index (0-based, from the top) that holds 0 dB, or `None` when the
/// window does not include it.
#[must_use]
pub fn zero_row(lo: f64, hi: f64, height: usize) -> Option<usize> {
    if height == 0 || lo > 0.0 || hi < 0.0 {
        return None;
    }
    Some(row_for(0.0, lo, hi, height))
}

/// What a cell of the response canvas carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    /// Nothing at all.
    Blank,
    /// A reference line: the 0 dB row or a decade mark.
    Grid,
    /// The response curve.
    Curve,
    /// A band node.
    Node,
    /// The node of the band under the cursor.
    NodeSelected,
}

/// One cell of the canvas: the character to draw and what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The character to draw.
    pub ch: char,
    /// What the character means.
    pub ink: Ink,
}

impl Default for Mark {
    fn default() -> Self {
        Self {
            ch: ' ',
            ink: Ink::Blank,
        }
    }
}

/// The response, drawn.
#[derive(Debug, Clone)]
pub struct Canvas {
    /// Columns.
    pub width: usize,
    /// Rows.
    pub height: usize,
    /// The cells, row-major.
    pub cells: Vec<Mark>,
}

impl Canvas {
    /// The mark at `(column, row)`, or a blank one when out of bounds.
    #[must_use]
    pub fn at(&self, column: usize, row: usize) -> Mark {
        if column >= self.width || row >= self.height {
            return Mark::default();
        }
        self.cells[row * self.width + column]
    }

    /// Puts a mark at `(column, row)`, ignoring anything out of bounds.
    pub fn put(&mut self, column: usize, row: usize, mark: Mark) {
        if column >= self.width || row >= self.height {
            return;
        }
        self.cells[row * self.width + column] = mark;
    }
}

/// What the response canvas should draw.
#[derive(Debug, Clone, Copy)]
pub struct Response<'a> {
    /// The bands of the curve.
    pub bands: &'a [Band],
    /// The preamp, in dB.
    pub preamp_db: f64,
    /// The width of the plot, in columns.
    pub width: usize,
    /// The height of the plot, in rows.
    pub height: usize,
    /// The bottom of the dB window.
    pub lo: f64,
    /// The top of the dB window.
    pub hi: f64,
    /// The band the cursor is on, if any.
    pub selected: Option<usize>,
    /// The glyphs to draw with.
    pub icons: &'a Icons,
}

/// Draws the response: the 0 dB reference, the decade marks, the curve and
/// the node of every band.
#[must_use]
pub fn canvas(response: &Response<'_>) -> Canvas {
    let Response {
        bands,
        preamp_db,
        width,
        height,
        lo,
        hi,
        selected,
        icons,
    } = *response;
    let width = width.max(1);
    let height = height.max(1);
    let mut canvas = Canvas {
        width,
        height,
        cells: vec![Mark::default(); width * height],
    };
    let sections = sections(bands);

    // The 0 dB reference, under whatever the curve puts there.
    if let Some(zero) = zero_row(lo, hi, height) {
        for column in 0..width {
            canvas.put(
                column,
                zero,
                Mark {
                    ch: icons.rule,
                    ink: Ink::Grid,
                },
            );
        }
    }
    // The decade marks.
    for hz in [100.0, 1_000.0, 10_000.0] {
        let column = column_for(hz, width);
        for row in 0..height {
            if canvas.at(column, row).ink == Ink::Blank {
                canvas.put(
                    column,
                    row,
                    Mark {
                        ch: icons.vertical,
                        ink: Ink::Grid,
                    },
                );
            }
        }
    }

    // The curve.
    let rows: Vec<usize> = (0..width)
        .map(|column| {
            let db = response_at(&sections, preamp_db, frequency_at(column, width));
            row_for(db, lo, hi, height)
        })
        .collect();
    draw_curve(&mut canvas, &rows, icons);

    // The node of every band, on the curve.
    for (index, band) in bands.iter().enumerate() {
        if band.kind() == BandKind::Off {
            continue;
        }
        let Some(hz) = band.freq_hz() else {
            continue;
        };
        if !(PLOT_MIN_HZ..=PLOT_MAX_HZ).contains(&hz) {
            continue;
        }
        let column = column_for(hz, width);
        // The node sits exactly where the curve is drawn, so it never floats
        // off the line.
        let row = rows[column];
        let mark = if selected == Some(index) {
            Mark {
                ch: icons.node_selected,
                ink: Ink::NodeSelected,
            }
        } else {
            Mark {
                ch: icons.node,
                ink: Ink::Node,
            }
        };
        canvas.put(column, row, mark);
    }

    canvas
}

/// Draws the curve: one row per column, joined with the box-drawing corners.
fn draw_curve(canvas: &mut Canvas, rows: &[usize], icons: &Icons) {
    let width = canvas.width;
    for column in 0..width {
        let y = rows[column];
        let previous = if column > 0 {
            Some(rows[column - 1])
        } else {
            None
        };
        let next = if column + 1 < width {
            Some(rows[column + 1])
        } else {
            None
        };
        let ch = corner(previous, next, y, icons);
        canvas.put(
            column,
            y,
            Mark {
                ch,
                ink: Ink::Curve,
            },
        );
    }
    // A steep step leaves a gap: fill it with the vertical stroke, under the
    // corner that opens it.
    for column in 1..width {
        let from = rows[column - 1];
        let to = rows[column];
        let (low, high) = if from < to { (from, to) } else { (to, from) };
        for row in (low + 1)..high {
            canvas.put(
                column - 1,
                row,
                Mark {
                    ch: icons.vertical,
                    ink: Ink::Curve,
                },
            );
        }
    }
}

/// Which way a segment leaves a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conn {
    /// To the left.
    Left,
    /// To the right.
    Right,
    /// Upwards.
    Up,
    /// Downwards.
    Down,
}

/// The character that joins the incoming and outgoing strokes of a curve cell.
fn corner(previous: Option<usize>, next: Option<usize>, y: usize, icons: &Icons) -> char {
    let incoming = match previous {
        None => None,
        Some(p) if p == y => Some(Conn::Left),
        Some(p) if p < y => Some(Conn::Up),
        Some(_) => Some(Conn::Down),
    };
    let outgoing = match next {
        None => None,
        Some(n) if n == y => Some(Conn::Right),
        Some(n) if n < y => Some(Conn::Up),
        Some(_) => Some(Conn::Down),
    };
    match (incoming, outgoing) {
        (Some(Conn::Left), Some(Conn::Right)) => icons.rule,
        (Some(Conn::Left), Some(Conn::Up)) => icons.corners[3],
        (Some(Conn::Left), Some(Conn::Down)) => icons.corners[1],
        (Some(Conn::Up), Some(Conn::Right)) => icons.corners[2],
        (Some(Conn::Down), Some(Conn::Right)) => icons.corners[0],
        (Some(Conn::Left), None) | (None, Some(Conn::Right)) => icons.rule,
        (None, Some(Conn::Up)) | (None, Some(Conn::Down)) => icons.vertical,
        (Some(Conn::Up), None) | (Some(Conn::Down), None) => icons.vertical,
        (None, None) => ' ',
        _ => icons.vertical,
    }
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
        assert!(hi - lo >= 24.0, "the window is at least ±12 dB");

        let (lo, hi) = window(&[-40.0, 20.0]);
        assert!(lo <= -40.0);
        assert!(hi >= 20.0);

        assert_eq!(window(&[]), (-12.0, 12.0));
    }

    #[test]
    fn the_window_lands_on_whole_sixes() {
        let (lo, hi) = window(&[-41.0, 13.0]);
        assert_eq!(lo % 6.0, 0.0, "lo was {lo}");
        assert_eq!(hi % 6.0, 0.0, "hi was {hi}");
        assert!(lo <= -42.0);
        assert!(hi >= 18.0);
    }

    #[test]
    fn zero_row_reports_where_the_axis_falls() {
        assert_eq!(zero_row(-6.0, 6.0, 4), Some(2));
        assert_eq!(zero_row(1.0, 6.0, 4), None);
        assert_eq!(zero_row(-6.0, -1.0, 4), None);
        assert_eq!(zero_row(-6.0, 6.0, 0), None);
        assert_eq!(NEUTRAL_BANDS.len(), BAND_COUNT);
    }

    #[test]
    fn the_log_axis_maps_the_ends_where_they_belong() {
        assert_eq!(column_for(PLOT_MIN_HZ, 66), 0);
        assert_eq!(column_for(PLOT_MAX_HZ, 66), 65);
        assert!((frequency_at(0, 66) - PLOT_MIN_HZ).abs() < 1e-6);
        assert!((frequency_at(65, 66) - PLOT_MAX_HZ).abs() < 1e-6);
        assert_eq!(column_for(1_000.0, 66), 37);
    }

    #[test]
    fn a_row_maps_to_its_place() {
        assert_eq!(row_for(12.0, -12.0, 12.0, 25), 0);
        assert_eq!(row_for(-12.0, -12.0, 12.0, 25), 24);
        assert_eq!(row_for(0.0, -12.0, 12.0, 25), 12);
        assert_eq!(row_for(100.0, -12.0, 12.0, 25), 0, "clamped to the top");
        assert_eq!(
            row_for(-100.0, -12.0, 12.0, 25),
            24,
            "clamped to the bottom"
        );
    }

    #[test]
    fn the_axis_labels_land_on_whole_sixes() {
        let labels = y_labels(-12.0, 12.0, 25);
        let values: Vec<String> = labels.iter().map(|(_, text)| text.clone()).collect();
        assert_eq!(values, vec!["+12", "+6", "0", "-6", "-12"]);
        for (row, _) in &labels {
            assert!(*row < 25);
        }
    }

    #[test]
    fn the_canvas_draws_a_curve_and_a_node_per_band() {
        let icons = Icons::unicode();
        let mut bands = NEUTRAL_BANDS;
        bands[0] = Band::Pk {
            freq_hz: 1_000.0,
            gain_db: 6.0,
            q_factor: 1.0,
        };
        let values = sample(&bands, -0.5, 66)
            .iter()
            .map(|&(_, db)| db)
            .collect::<Vec<_>>();
        let (lo, hi) = window(&values);
        let art = canvas(&Response {
            bands: &bands,
            preamp_db: -0.5,
            width: 66,
            height: 24,
            lo,
            hi,
            selected: Some(0),
            icons: &icons,
        });

        assert_eq!(art.width, 66);
        assert_eq!(art.height, 24);
        assert_eq!(art.cells.len(), 66 * 24);
        assert!(
            art.cells.iter().any(|mark| mark.ink == Ink::Curve),
            "the curve must be drawn"
        );
        assert!(
            art.cells.iter().any(|mark| mark.ink == Ink::Grid),
            "the reference line must be drawn"
        );
        assert!(
            art.cells.iter().any(|mark| mark.ink == Ink::NodeSelected),
            "the selected band must have a node"
        );
        // The node of the selected band sits on the 1 kHz column.
        let column = column_for(1_000.0, 66);
        let node = (0..art.height)
            .map(|row| art.at(column, row))
            .find(|mark| mark.ink == Ink::NodeSelected);
        assert!(node.is_some(), "the node must be on the band frequency");
    }

    #[test]
    fn an_empty_curve_still_draws() {
        let icons = Icons::unicode();
        let art = canvas(&Response {
            bands: &[],
            preamp_db: -0.5,
            width: 10,
            height: 6,
            lo: -12.0,
            hi: 12.0,
            selected: None,
            icons: &icons,
        });
        assert_eq!(art.cells.len(), 60);
        assert_eq!(art.at(99, 99), Mark::default());
    }
}

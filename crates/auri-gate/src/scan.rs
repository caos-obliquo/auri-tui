//! The multi-section scan grids: how the gate finds the peak of a cascade of
//! biquads over a frequency domain.
//!
//! Port of `scanGrid`/`CascadeScan` in `src/safety/gate.mjs`. The JavaScript
//! caches grids in a `Map`; here every `CascadeScan` builds its own, which costs
//! about 0.8 ms per gate call and keeps the state trivially single-threaded.

use auri_codec::{SAMPLE_RATE, Section};

use crate::level::section_db_from_terms;
use crate::section::{Peak, refine, single_section_peak};

/// Log scan grid with the per-frequency terms of `section_db` precomputed.
struct ScanGrid {
    fs: Vec<f64>,
    re: Vec<f64>,
    im: Vec<f64>,
    u2re: Vec<f64>,
    u2im: Vec<f64>,
}

/// Scan grid: log from `min_hz || 0.001` to `max_hz`, exact ends, plus DC when
/// `min_hz` is 0, with the per-frequency terms of `section_db` precomputed.
fn scan_grid(intervals: usize, min_hz: f64, max_hz: f64) -> ScanGrid {
    let lo = if min_hz == 0.0 { 0.001 } else { min_hz };
    let mut fs: Vec<f64> = (0..=intervals)
        .map(|i| lo * (max_hz / lo).powf(f64::from(i as u32) / f64::from(intervals as u32)))
        .collect();
    fs[0] = lo;
    fs[intervals] = max_hz;
    if min_hz == 0.0 {
        fs.insert(0, 0.0);
    }

    let n = fs.len();
    let mut grid = ScanGrid {
        fs,
        re: vec![0.0; n],
        im: vec![0.0; n],
        u2re: vec![0.0; n],
        u2im: vec![0.0; n],
    };
    for j in 0..n {
        let w = (2.0 * std::f64::consts::PI * grid.fs[j]) / f64::from(SAMPLE_RATE);
        let re = 2.0 * (w / 2.0).sin().powi(2);
        let im = w.sin();
        grid.re[j] = re;
        grid.im[j] = im;
        grid.u2re[j] = re * re - im * im;
        grid.u2im[j] = 2.0 * re * im;
    }
    grid
}

/// Incremental cascade response on a scan grid: [`CascadeScan::add`] appends a
/// section, [`CascadeScan::peak`] returns the peak of the sections added so far.
pub(crate) struct CascadeScan {
    min_hz: f64,
    max_hz: f64,
    grid: Option<ScanGrid>,
    values: Vec<f64>,
    sections: Vec<Section>,
}

impl CascadeScan {
    /// `scan` says whether the grid is needed: more than one section will be
    /// added (one active section always uses the analytic peak).
    pub(crate) fn new(intervals: usize, min_hz: f64, max_hz: f64, scan: bool) -> Self {
        let grid = scan.then(|| scan_grid(intervals, min_hz, max_hz));
        let values = match &grid {
            Some(grid) => vec![0.0; grid.fs.len()],
            None => Vec::new(),
        };
        CascadeScan {
            min_hz,
            max_hz,
            grid,
            values,
            sections: Vec::new(),
        }
    }

    /// Appends a section, accumulating its contribution on the grid.
    pub(crate) fn add(&mut self, s: Section) {
        self.sections.push(s);
        let Some(grid) = &self.grid else {
            return;
        };
        for j in 0..self.values.len() {
            let db = section_db_from_terms(s, grid.re[j], grid.im[j], grid.u2re[j], grid.u2im[j]);
            self.values[j] += db;
        }
    }

    /// The peak of the sections added so far.
    pub(crate) fn peak(&self) -> Peak {
        if self.sections.is_empty() {
            return Peak {
                db: 0.0,
                frequency_hz: self.min_hz,
            };
        }
        if self.sections.len() == 1 {
            return single_section_peak(self.sections[0], self.min_hz, self.max_hz);
        }
        let grid = self
            .grid
            .as_ref()
            .expect("a multi-section peak needs a grid");
        let f = |frequency: f64| {
            self.sections.iter().fold(0.0, |db, section| {
                db + crate::level::section_db(*section, frequency)
            })
        };

        let mut best = Peak {
            db: self.values[0],
            frequency_hz: grid.fs[0],
        };
        for i in 1..grid.fs.len() {
            if self.values[i] > best.db {
                best = Peak {
                    db: self.values[i],
                    frequency_hz: grid.fs[i],
                };
            }
            if i < grid.fs.len() - 1
                && self.values[i] > self.values[i - 1]
                && self.values[i] >= self.values[i + 1]
            {
                let candidate = refine(f, grid.fs[i - 1], grid.fs[i + 1]);
                if candidate.db > best.db {
                    best = candidate;
                }
            }
        }
        // Not in the JavaScript reference: a peak inside the first or last
        // interval has no bracketing interior grid point, so refine both edges
        // too. A candidate only replaces `best` when strictly higher, so this
        // can only make the gate stricter, never laxer.
        let last = grid.fs.len() - 1;
        for (a, b) in [(0, 1), (last - 1, last)] {
            let candidate = refine(f, grid.fs[a], grid.fs[b]);
            if candidate.db > best.db {
                best = candidate;
            }
        }
        best
    }
}

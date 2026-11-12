//! Fitting an AutoEq-style headphone measurement to an Auri curve.
//!
//! AutoEq methodology only: measurement and target responses are normalised
//! around 1 kHz, interpolated on a shared log-frequency grid, and fitted with
//! a greedily regularised digital filter model. The curve is written to disk
//! by the caller after the ordinary safety gate says yes; no HID transport or
//! flash state is touched anywhere in this module.

use std::path::Path;

use auri_codec::{Band, BandKind};
use auri_gate::{quantized_preamp_db, quantized_section, section_analysis, section_db};

use crate::curve::CurveFile;

const MIN_FREQ_HZ: f64 = 20.0;
const MAX_FREQ_HZ: f64 = 20_000.0;
const GRID_POINTS: usize = 160;
const MAX_BANDS: usize = 10;

/// What the fit found, once both CSV files have been read.
#[derive(Debug)]
pub struct FitResult {
    /// The ten-section curve to write, or to refuse at the gate.
    pub curve: CurveFile,
    /// Points used on the shared response grid.
    pub points: usize,
    /// Error before fitting, in dB RMS.
    pub rms_before_db: f64,
    /// Error after fitting, in dB RMS.
    pub rms_after_db: f64,
    /// Largest absolute point error remaining, in dB.
    pub max_error_db: f64,
}

#[derive(Debug, Clone, Copy)]
struct Point {
    hz: f64,
    db: f64,
}

/// Reads and interpolates the two CSV files, then fits a curve.
///
/// # Errors
///
/// Unreadable or malformed CSV files, fewer than two shared frequencies, or a
/// fit that cannot produce a valid band.
pub fn fit_files(measurement: &Path, target: &Path) -> Result<FitResult, String> {
    let measurement = read_response(measurement)?;
    let target = read_response(target)?;
    fit_responses(&measurement, &target)
}

/// Writes the fitted curve as the same on-disk format `show` reads.
///
/// # Errors
///
/// JSON serialization failure.
pub fn write_curve(curve: &CurveFile, path: &Path) -> Result<(), String> {
    let text = curve.to_json()?;
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn fit_responses(measurement: &[Point], target: &[Point]) -> Result<FitResult, String> {
    let min_hz = measurement
        .iter()
        .map(|point| point.hz)
        .fold(f64::INFINITY, f64::min)
        .max(
            target
                .iter()
                .map(|point| point.hz)
                .fold(f64::INFINITY, f64::min),
        );
    let max_hz = measurement
        .iter()
        .map(|point| point.hz)
        .fold(0.0_f64, f64::max)
        .min(target.iter().map(|point| point.hz).fold(0.0_f64, f64::max));
    if max_hz <= min_hz {
        return Err(format!(
            "measurement and target do not share a usable frequency range \
             ({min_hz}–{max_hz} Hz)"
        ));
    }

    let grid = log_grid(min_hz, max_hz, GRID_POINTS);
    let measured: Vec<f64> = grid
        .iter()
        .map(|&hz| interpolate(measurement, hz))
        .collect();
    let desired: Vec<f64> = grid.iter().map(|&hz| interpolate(target, hz)).collect();
    let measured = normalise(&grid, &measured);
    let desired = normalise(&grid, &desired);

    // The amount of EQ the headphones need to land on the target, after the
    // shared 1 kHz reference has been taken out. Absolute level is the
    // preamp's job, not a filter's.
    let correction: Vec<f64> = desired
        .iter()
        .zip(&measured)
        .map(|(target, measured)| target - measured)
        .collect();

    let rms_before_db = rms(&correction, &vec![0.0; correction.len()]);
    let mut predicted = vec![0.0; correction.len()];
    let mut active: Vec<Band> = greedy_fit(&grid, &correction, &mut predicted)?;
    refine(&grid, &correction, &mut active, &mut predicted)?;

    active.sort_unstable_by(|left, right| {
        let kind_order = |band: &Band| usize::from(band.kind() == BandKind::Ls);
        kind_order(left).cmp(&kind_order(right)).then_with(|| {
            left.freq_hz()
                .partial_cmp(&right.freq_hz())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    active.truncate(MAX_BANDS);

    let mut eq_bands = active;
    while eq_bands.len() < MAX_BANDS {
        eq_bands.push(Band::Off);
    }

    let max_boost_db = peak_response_db(&eq_bands, min_hz, max_hz)?;
    // Leave a dB of room below the gate's full-scale output check. The gate
    // still has the final word, but a fit should not start above its own
    // loudness budget and ask the user to waive it.
    let preamp_db = (-0.5_f64).min(1.5 - max_boost_db).clamp(-25.5, -0.5);

    let curve = CurveFile {
        preamp_db,
        eq_bands,
    };
    curve.validate()?;

    let error: Vec<f64> = correction
        .iter()
        .zip(&predicted)
        .map(|(wanted, got)| wanted - got)
        .collect();
    let rms_after_db = rms(&correction, &predicted);
    let max_error_db = error.iter().copied().map(f64::abs).fold(0.0, f64::max);

    Ok(FitResult {
        curve,
        points: grid.len(),
        rms_before_db,
        rms_after_db,
        max_error_db,
    })
}

/// A section is only useful if the firmware can encode it and the ordinary
/// per-section gate checks accept it. The complete gate still has the final
/// word after the fit settles.
fn section_is_safe(band: Band) -> bool {
    section_analysis(band).is_ok_and(|analysis| analysis.reasons.is_empty())
}

fn greedy_fit(grid: &[f64], wanted: &[f64], predicted: &mut [f64]) -> Result<Vec<Band>, String> {
    let mut active: Vec<Band> = Vec::new();
    for _ in 0..MAX_BANDS {
        let current_error = rms(wanted, predicted);
        let mut best: Option<(f64, Band, Vec<f64>)> = None;

        for candidate in candidates() {
            if !section_is_safe(candidate) {
                continue;
            }
            if candidate.kind() == BandKind::Ls
                && active
                    .iter()
                    .filter(|band| band.kind() == BandKind::Ls)
                    .count()
                    >= 1
            {
                continue;
            }
            if candidate.kind() == BandKind::Pk
                && candidate
                    .freq_hz()
                    .is_some_and(|freq| freq > 16_000.0 && candidate.gain_db() > 0.0)
            {
                continue;
            }
            let contribution = response_at(grid, candidate)?;
            let trial: Vec<f64> = predicted
                .iter()
                .zip(&contribution)
                .map(|(current, addition)| current + addition)
                .collect();
            let error = rms(wanted, &trial);
            // AutoEq's practical penalty: a broad low-shelf or a very high-Q
            // boost must buy its keep, not collect whichever residue is left.
            let penalty = 0.015 * candidate.q_factor().unwrap_or(1.0) * candidate.gain_db().abs();
            let score = error + penalty;
            let replace = best
                .as_ref()
                .is_none_or(|(best_score, _, _)| score < *best_score);
            if replace {
                best = Some((score, candidate, contribution));
            }
        }

        let Some((_, band, contribution)) = best else {
            break;
        };
        if rms(wanted, &add(predicted, &contribution)) >= current_error - 0.025 {
            break;
        }
        for (current, addition) in predicted.iter_mut().zip(&contribution) {
            *current += *addition;
        }
        active.push(band);
    }
    Ok(active)
}

fn refine(
    grid: &[f64],
    wanted: &[f64],
    active: &mut [Band],
    predicted: &mut [f64],
) -> Result<(), String> {
    for band in active.iter_mut() {
        // Start the pass from the prediction with this band removed, so the
        // test neighbourhood compares like with like.
        let original = *band;
        let original_response = response_at(grid, original)?;
        for (current, original_value) in predicted.iter_mut().zip(&original_response) {
            *current -= *original_value;
        }

        let base = predicted.to_vec();
        let mut best_band = original;
        let mut best_response = original_response;
        let mut best_error = rms(wanted, &add(&base, &best_response));

        for frequency_factor in [0.85, 0.90, 0.95, 1.00, 1.05, 1.10, 1.18] {
            for q_factor in [0.75, 0.90, 1.11, 1.33] {
                for gain_delta in [-1.5, -1.0, -0.5, 0.5, 1.0, 1.5] {
                    let Some(candidate) = tweak(original, frequency_factor, q_factor, gain_delta)
                    else {
                        continue;
                    };
                    if !section_is_safe(candidate) {
                        continue;
                    }
                    let response = response_at(grid, candidate)?;
                    let trial = add(&base, &response);
                    let error = rms(wanted, &trial)
                        + 0.010 * candidate.q_factor().unwrap_or(1.0) * candidate.gain_db().abs();
                    if error < best_error {
                        best_error = error;
                        best_band = candidate;
                        best_response = response;
                    }
                }
            }
        }

        *band = best_band;
        for (current, best_value) in predicted.iter_mut().zip(&best_response) {
            *current += *best_value;
        }
    }
    Ok(())
}

fn tweak(original: Band, frequency_factor: f64, q_factor: f64, gain_delta: f64) -> Option<Band> {
    let frequency = (original.freq_hz()? * frequency_factor).clamp(MIN_FREQ_HZ, MAX_FREQ_HZ);
    let q = (original.q_factor()? * q_factor).clamp(0.1, 4.0);
    let gain = (original.gain_db() + gain_delta).clamp(-24.0, 9.2);
    Some(match original.kind() {
        BandKind::Pk => Band::Pk {
            freq_hz: frequency,
            gain_db: gain,
            q_factor: q,
        },
        BandKind::Ls => Band::Ls {
            freq_hz: frequency,
            gain_db: gain,
            q_factor: q,
        },
        _ => return None,
    })
}

fn candidates() -> Vec<Band> {
    let frequencies = [
        20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0,
        500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0,
        6300.0, 8000.0, 10_000.0, 12_500.0, 16_000.0,
    ];
    let gains = [-12.0, -9.0, -6.0, -4.0, -2.5, 2.5, 4.0, 6.0, 9.0];
    let q_values = [0.3, 0.5, 0.707, 1.0, 1.414, 2.0, 2.8];
    let mut candidates = Vec::with_capacity(frequencies.len() * gains.len() * q_values.len() + 500);

    for &hz in &frequencies[..12] {
        for &gain_db in &gains {
            for &q_factor in &[0.3, 0.5, 0.707, 1.0] {
                candidates.push(Band::Ls {
                    freq_hz: hz,
                    gain_db,
                    q_factor,
                });
            }
        }
    }
    for hz in frequencies {
        for gain_db in gains {
            for q_factor in q_values {
                candidates.push(Band::Pk {
                    freq_hz: hz,
                    gain_db,
                    q_factor,
                });
            }
        }
    }
    candidates
}

fn add(base: &[f64], addition: &[f64]) -> Vec<f64> {
    base.iter().zip(addition).map(|(a, b)| a + b).collect()
}

fn read_response(path: &Path) -> Result<Vec<Point>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut points: Vec<Point> = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        let fields: Vec<&str> = line
            .split([',', ';', '\t'])
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .collect();
        if fields.len() < 2 {
            continue;
        }
        let (Ok(hz), Ok(db)) = (fields[0].parse::<f64>(), fields[1].parse::<f64>()) else {
            // CSV headers are expected and skipped; anything else with two
            // numeric-looking fields must pass the same finite checks.
            continue;
        };
        if !hz.is_finite() || hz <= 0.0 || !db.is_finite() {
            return Err(format!(
                "{}:{} has a non-finite frequency or gain",
                path.display(),
                line_number + 1
            ));
        }
        points.push(Point { hz, db });
    }

    points.sort_unstable_by(|left, right| left.hz.total_cmp(&right.hz));
    points.dedup_by(|later, earlier| {
        if (later.hz - earlier.hz).abs() <= f64::EPSILON * earlier.hz.max(1.0) {
            earlier.db = (earlier.db + later.db) / 2.0;
            true
        } else {
            false
        }
    });
    if points.len() < 2 {
        return Err(format!(
            "{} needs at least two numeric frequency/dB rows",
            path.display()
        ));
    }
    Ok(points)
}

fn interpolate(points: &[Point], hz: f64) -> f64 {
    if hz <= points[0].hz {
        return points[0].db;
    }
    if hz >= points[points.len() - 1].hz {
        return points[points.len() - 1].db;
    }
    let right = points.partition_point(|point| point.hz < hz);
    let left = right - 1;
    let a = points[left];
    let b = points[right];
    let span = b.hz.ln() - a.hz.ln();
    if span <= f64::EPSILON {
        return b.db;
    }
    let ratio = (hz.ln() - a.hz.ln()) / span;
    a.db + ratio * (b.db - a.db)
}

fn normalise(grid: &[f64], response: &[f64]) -> Vec<f64> {
    let reference = if grid[0] <= 1000.0 && grid[grid.len() - 1] >= 1000.0 {
        interpolate_points(grid, response, 1000.0)
    } else {
        response.iter().sum::<f64>() / response.len() as f64
    };
    response.iter().map(|value| value - reference).collect()
}

fn interpolate_points(grid: &[f64], response: &[f64], hz: f64) -> f64 {
    let points: Vec<Point> = grid
        .iter()
        .copied()
        .zip(response.iter().copied())
        .map(|(hz, db)| Point { hz, db })
        .collect();
    interpolate(&points, hz)
}

fn log_grid(min_hz: f64, max_hz: f64, points: usize) -> Vec<f64> {
    if points <= 1 {
        return vec![min_hz];
    }
    let start = min_hz.ln();
    let end = max_hz.ln();
    (0..points)
        .map(|index| {
            let ratio = index as f64 / (points - 1) as f64;
            (start + ratio * (end - start)).exp()
        })
        .collect()
}

fn rms(wanted: &[f64], actual: &[f64]) -> f64 {
    let sum: f64 = wanted
        .iter()
        .zip(actual)
        .map(|(wanted, actual)| (wanted - actual).powi(2))
        .sum();
    (sum / wanted.len() as f64).sqrt()
}

fn response_at(grid: &[f64], band: Band) -> Result<Vec<f64>, String> {
    if band.kind() == BandKind::Off {
        return Ok(vec![0.0; grid.len()]);
    }
    let section = quantized_section(band)
        .map_err(|error| format!("cannot encode the {:?} band: {error:?}", band.kind()))?;
    Ok(grid.iter().map(|&hz| section_db(section, hz)).collect())
}

fn peak_response_db(bands: &[Band], min_hz: f64, max_hz: f64) -> Result<f64, String> {
    let grid = log_grid(min_hz, max_hz, 512);
    let mut response = vec![0.0_f64; grid.len()];
    for band in bands.iter().copied() {
        if band.kind() == BandKind::Off {
            continue;
        }
        let band_response = response_at(&grid, band)?;
        for (sample, value) in response.iter_mut().zip(band_response) {
            *sample += value;
        }
    }
    Ok(response.into_iter().fold(0.0, f64::max))
}

/// How the written curve compares to a target, with the chip's quantized math.
///
/// Both curves are compared after normalising at 1 kHz, matching how the fit
/// treats the absolute level as the preamp's job, not a filter's: the
/// preamp is an additive offset and is excluded from the shape comparison.
#[derive(Debug)]
pub struct CheckResult {
    /// RMS deviation over the shared grid, in dB.
    pub rms_db: f64,
    /// Largest absolute deviation, in dB.
    pub max_dev_db: f64,
    /// Frequency of the largest deviation, in Hz.
    ///
    /// Indicative only: both curves are normalised at 1 kHz, so a concentrated
    /// boost centered at 1 kHz is removed by construction and the reported
    /// maximum moves elsewhere. Read the magnitude from `max_dev_db`, not the
    /// location from `argmax_hz`.
    pub argmax_hz: f64,
    /// Preamp the chip will actually apply, in dB (Q23-quantized).
    pub preamp_applied_db: f64,
}

/// Compares a curve's on-the-wire frequency-response shape against a
/// two-column target CSV.
///
/// Uses the same quantized section math as the codec (`quantized_preamp_db` is
/// reported for reference, then excluded; `quantized_section`/`section_db`), so
/// the numbers reflect what the cable will apply. Both curves are normalised
/// at 1 kHz exactly like [`fit_files`]: the absolute level is the preamp's
/// job, not a filter's.
///
/// # Errors
///
/// Invalid curve, unreadable or malformed target CSV, or a band whose
/// coefficients cannot be encoded.
pub fn check_target(curve: &CurveFile, target: &Path) -> Result<CheckResult, String> {
    curve.validate()?;
    let target = read_response(target)?;
    let grid = log_grid(MIN_FREQ_HZ, MAX_FREQ_HZ, GRID_POINTS);
    let target_norm = normalise(
        &grid,
        &grid.iter().map(|&hz| interpolate(&target, hz)).collect::<Vec<_>>(),
    );
    let preamp_applied = quantized_preamp_db(curve.preamp_db).map_err(|e| format!("{e:?}"))?;
    let mut response = vec![0.0_f64; grid.len()];
    for band in curve.eq_bands.iter().copied() {
        let part = response_at(&grid, band)?;
        for (sample, value) in response.iter_mut().zip(&part) {
            *sample += value;
        }
    }
    let response = normalise(&grid, &response);

    let mut argmax = 0usize;
    let mut sum = 0.0_f64;
    let mut max_dev = 0.0_f64;
    for (index, (&value, &wanted)) in response.iter().zip(&target_norm).enumerate() {
        let deviation = (value - wanted).abs();
        sum += deviation * deviation;
        if deviation > max_dev {
            max_dev = deviation;
            argmax = index;
        }
    }
    Ok(CheckResult {
        rms_db: (sum / grid.len() as f64).sqrt(),
        max_dev_db: max_dev,
        argmax_hz: grid[argmax],
        preamp_applied_db: preamp_applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::write(path, contents).expect("test file");
    }

    #[test]
    fn parses_and_orders_numeric_csv_rows() {
        let dir = std::env::temp_dir().join(format!("auri-autoeq-parse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("measured.csv");
        write(&path, "frequency,dB\n1000, -3.5\n20;0\n100,-1\n\n");
        let points = read_response(&path).expect("rows");
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].hz, 20.0);
        assert_eq!(points[1].db, -1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_target_on_a_flat_curve_reports_zero_deviation() {
        let dir = std::env::temp_dir().join(format!("auri-autoeq-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let target = dir.join("target.csv");
        write(&target, "20,0\n100,0\n1000,0\n10000,0\n20000,0\n");
        let result = check_target(&CurveFile::neutral(), &target).expect("check");
        assert!(result.max_dev_db < 1e-9, "{}", result.max_dev_db);
        assert!(result.rms_db < 1e-9, "{}", result.rms_db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_target_flags_a_nine_db_boost_over_the_limit() {
        let dir = std::env::temp_dir().join(format!("auri-autoeq-check-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let target = dir.join("target.csv");
        write(&target, "20,0\n100,0\n1000,0\n10000,0\n20000,0\n");
        let boosted = Band::Pk {
            freq_hz: 1_000.0,
            gain_db: 9.0,
            q_factor: 1.0,
        };
        let mut eq_bands = vec![boosted];
        eq_bands.resize(10, Band::Off);
        let curve = CurveFile {
            preamp_db: -0.5,
            eq_bands,
        };
        let result = check_target(&curve, &target).expect("check");
        assert!(result.max_dev_db > 5.0, "{}", result.max_dev_db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_files_without_enough_shared_range() {
        let dir = std::env::temp_dir().join(format!("auri-autoeq-range-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let measurement = dir.join("measurement.csv");
        let target = dir.join("target.csv");
        write(&measurement, "20,0\n30,0\n");
        write(&target, "40,0\n100,0\n");
        let error = fit_files(&measurement, &target).expect_err("no overlap");
        assert!(error.contains("do not share"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flat_measurement_needs_no_correction() {
        let dir = std::env::temp_dir().join(format!("auri-autoeq-flat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let measurement = dir.join("measurement.csv");
        let target = dir.join("target.csv");
        let rows = (0..40)
            .map(|index| {
                let hz = 20.0 * (1000.0_f64).powf(index as f64 / 39.0);
                format!("{hz},0\n")
            })
            .collect::<String>();
        write(&measurement, &rows);
        write(&target, &rows);
        let result = fit_files(&measurement, &target).expect("fit");
        assert!(result.rms_before_db < 0.001, "{:?}", result.rms_before_db);
        assert!(result.rms_after_db < 0.001);
        assert_eq!(
            result
                .curve
                .eq_bands
                .iter()
                .filter(|band| band.kind() != BandKind::Off)
                .count(),
            0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_known_tilt_improves() {
        let dir = std::env::temp_dir().join(format!("auri-autoeq-tilt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("test dir");
        let measurement = dir.join("measurement.csv");
        let target = dir.join("target.csv");
        write(&target, "20,0\n100,0\n1000,0\n10000,0\n20000,0\n");
        write(&measurement, "20,6\n100,3\n1000,0\n10000,-3\n20000,-6\n");
        let result = fit_files(&measurement, &target).expect("fit");
        assert!(result.rms_after_db < result.rms_before_db * 0.75);
        assert_eq!(result.curve.eq_bands.len(), MAX_BANDS);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! The TUI's state machine: bands, cursor, profile and the write handshake.
//!
//! Everything here is pure — no terminal, no USB — so the whole interaction is
//! unit-testable. [`crate::tui::mod`] maps real key events onto [`Key`] and
//! performs the write [`Effect::Write`] asks for.

use auri_codec::{BAND_COUNT, Band, BandKind, NEUTRAL_BANDS};
use auri_gate::{GAIN_MAX_DB, GAIN_MIN_DB, GateOptions, GateResult, Profile, gate_for_write};

/// Smallest frequency the editor will step to, in Hz.
const FREQ_MIN: f64 = 1.0;

/// Largest frequency the editor will step to, in Hz (just under Nyquist).
const FREQ_MAX: f64 = 47_999.0;

/// Smallest Q the editor will step to.
const Q_MIN: f64 = 0.1;

/// Largest Q the editor will step to.
const Q_MAX: f64 = 20.0;

/// Gain step in dB.
const GAIN_STEP: f64 = 0.5;

/// Relative step for frequency and Q.
const LOG_STEP: f64 = 1.1;

/// The keys the TUI understands, already decoded from the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character.
    Char(char),
    /// Cursor up.
    Up,
    /// Cursor down.
    Down,
    /// Cursor left.
    Left,
    /// Cursor right.
    Right,
    /// Enter.
    Enter,
    /// Escape.
    Esc,
    /// Anything else.
    Other,
}

/// What the runner must do after a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing to do outside the app.
    Nothing,
    /// Leave the TUI.
    Quit,
    /// The user confirmed a write; the runner must send it.
    Write,
}

/// The four editable columns of the band table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    /// Filter shape.
    Kind,
    /// Frequency in Hz.
    Freq,
    /// Gain in dB.
    Gain,
    /// Quality factor.
    Q,
}

impl Col {
    /// The column's position, left to right (`kind` is 0, `q` is 3).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Col::Kind => 0,
            Col::Freq => 1,
            Col::Gain => 2,
            Col::Q => 3,
        }
    }

    /// The column at `index`, wrapping.
    #[must_use]
    pub const fn from_index(index: usize) -> Col {
        match index % 4 {
            0 => Col::Kind,
            1 => Col::Freq,
            2 => Col::Gain,
            _ => Col::Q,
        }
    }

    /// The column to the right (`l`), wrapping.
    #[must_use]
    pub const fn next(self) -> Col {
        Col::from_index(self.index() + 1)
    }

    /// The column to the left (`h`), wrapping.
    #[must_use]
    pub const fn prev(self) -> Col {
        Col::from_index(self.index() + 3)
    }

    /// The column heading as the table shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Col::Kind => "kind",
            Col::Freq => "freq",
            Col::Gain => "gain",
            Col::Q => "q",
        }
    }
}

/// The editable state of the TUI.
#[derive(Debug, Clone)]
pub struct App {
    /// The ten bands being edited.
    pub bands: Vec<Band>,
    /// The curve the TUI started from, for `r` (reset).
    pub original: Vec<Band>,
    /// Preamp in dB.
    pub preamp_db: f64,
    /// Selected band, `0..BAND_COUNT`.
    pub row: usize,
    /// Selected column.
    pub col: Col,
    /// Which limits judge the curve.
    pub profile: Profile,
    /// Whether HS/LP/HP are on the menu.
    pub experimental: bool,
    /// Whether `w` will send rather than arm.
    pub armed: bool,
    /// Set once the user asks to leave.
    pub quit: bool,
    /// The status line.
    pub status: String,
    /// The live verdict, recomputed after every edit.
    pub verdict: GateResult,
}

impl App {
    /// Builds the editor around a curve.
    #[must_use]
    pub fn new(bands: Vec<Band>, preamp_db: f64, yolo: bool) -> Self {
        let profile = if yolo {
            Profile::Yolo
        } else {
            Profile::Standard
        };
        let mut app = App {
            bands,
            original: Vec::new(),
            preamp_db,
            row: 0,
            col: Col::Kind,
            profile,
            experimental: false,
            armed: false,
            quit: false,
            status: String::new(),
            verdict: GateResult {
                ok: false,
                reasons: Vec::new(),
                waived: None,
                waived_reasons: None,
                profile: None,
                metrics: Default::default(),
            },
        };
        app.original = app.bands.clone();
        app.recompute();
        app.status = "ready".to_string();
        app
    }

    /// The ten pass-through sections at the official −0.5 dB preamp.
    ///
    /// Only the tests build a curve from nothing: the binary always starts
    /// from a [`crate::curve::CurveFile`], which already knows this default.
    #[cfg(test)]
    #[must_use]
    pub fn neutral(yolo: bool) -> Self {
        App::new(NEUTRAL_BANDS.to_vec(), -0.5, yolo)
    }

    /// Re-judges the current curve with the current options.
    pub fn recompute(&mut self) {
        self.verdict = gate_for_write(
            &self.bands,
            self.preamp_db,
            GateOptions::for_write(self.experimental, self.profile),
        );
    }

    /// Every edit invalidates an armed write: arming must be the last thing
    /// that happens before a send, never something that survives a change.
    fn edited(&mut self, status: impl Into<String>) {
        self.armed = false;
        self.recompute();
        self.status = status.into();
    }

    /// Handles one key and reports what the runner must do next.
    pub fn handle_key(&mut self, key: Key) -> Effect {
        match key {
            Key::Char('q') => {
                self.quit = true;
                Effect::Quit
            }
            Key::Esc => {
                if self.armed {
                    self.armed = false;
                    self.status = "write cancelled".to_string();
                } else {
                    self.quit = true;
                    return Effect::Quit;
                }
                Effect::Nothing
            }
            // Vim motions: h/l walk the columns, j/k walk the bands.
            Key::Char('j') | Key::Down => {
                self.row = (self.row + 1).min(BAND_COUNT - 1);
                Effect::Nothing
            }
            Key::Char('k') | Key::Up => {
                self.row = self.row.saturating_sub(1);
                Effect::Nothing
            }
            Key::Char('l') | Key::Right => {
                self.col = self.col.next();
                Effect::Nothing
            }
            Key::Char('h') | Key::Left => {
                self.col = self.col.prev();
                Effect::Nothing
            }
            Key::Char('g') => {
                self.row = 0;
                Effect::Nothing
            }
            Key::Char('G') => {
                self.row = BAND_COUNT - 1;
                Effect::Nothing
            }
            Key::Char('+') | Key::Char('=') => {
                self.adjust(1);
                Effect::Nothing
            }
            Key::Char('-') => {
                self.adjust(-1);
                Effect::Nothing
            }
            Key::Char('c') => {
                self.cycle_kind();
                Effect::Nothing
            }
            Key::Char('x') => {
                let band = self.bands[self.row];
                if band.kind() == BandKind::Off {
                    self.status = format!("band {} is already off", self.row + 1);
                } else {
                    self.bands[self.row] = Band::Off;
                    self.edited(format!("band {} set to OFF", self.row + 1));
                }
                Effect::Nothing
            }
            Key::Char('t') => {
                self.profile = match self.profile {
                    Profile::Standard => Profile::Yolo,
                    Profile::Yolo => Profile::Standard,
                };
                self.edited(format!("profile: {}", self.profile.as_str()));
                Effect::Nothing
            }
            Key::Char('e') => {
                self.experimental = !self.experimental;
                let layer = if self.experimental {
                    "engineering"
                } else {
                    "ajustar"
                };
                self.edited(format!("layer: {layer}"));
                Effect::Nothing
            }
            Key::Char('n') => {
                self.bands = NEUTRAL_BANDS.to_vec();
                self.preamp_db = -0.5;
                self.edited("loaded Neutro".to_string());
                Effect::Nothing
            }
            Key::Char('r') => {
                self.bands = self.original.clone();
                self.edited("restored the curve this session started from".to_string());
                Effect::Nothing
            }
            Key::Char('w') | Key::Enter if self.armed => {
                self.armed = false;
                self.status = "writing…".to_string();
                Effect::Write
            }
            Key::Char('w') | Key::Enter => {
                if !self.verdict.ok {
                    self.status = format!(
                        "gate refused {} reason(s) — nothing can be written",
                        self.verdict.reasons.len()
                    );
                } else {
                    self.armed = true;
                    self.status = "armed: press w again to write, Esc to cancel".to_string();
                }
                Effect::Nothing
            }
            Key::Char('?') => {
                self.status =
                    "hjkl move · +/- step · c kind · x off · t yolo · e layer · w write · q quit"
                        .to_string();
                Effect::Nothing
            }
            _ => Effect::Nothing,
        }
    }

    /// Steps the value under the cursor. `direction` is `+1` or `-1`.
    pub fn adjust(&mut self, direction: i32) {
        let row = self.row;
        let col = self.col;
        let band = activate(self.bands[row]);

        match col {
            Col::Kind => {
                self.cycle_kind();
                return;
            }
            Col::Freq => {
                let freq = band.freq_hz().unwrap_or(1_000.0) * LOG_STEP.powi(direction);
                self.bands[row] =
                    with_values(band, Some(freq.clamp(FREQ_MIN, FREQ_MAX)), None, None);
            }
            Col::Gain => {
                if !band.kind().has_gain() {
                    self.status = format!("{} takes no gain", band.kind().as_str());
                    return;
                }
                let gain = (band.gain_db() + GAIN_STEP * f64::from(direction))
                    .clamp(GAIN_MIN_DB, GAIN_MAX_DB);
                self.bands[row] = with_values(band, None, Some(gain), None);
            }
            Col::Q => {
                let q = band.q_factor().unwrap_or(1.0) * LOG_STEP.powi(direction);
                self.bands[row] = with_values(band, None, None, Some(q.clamp(Q_MIN, Q_MAX)));
            }
        }

        let label = col.label();
        let value = match col {
            Col::Freq => format!("{:.1} Hz", self.bands[row].freq_hz().unwrap_or_default()),
            Col::Gain => format!("{:+.1} dB", self.bands[row].gain_db()),
            Col::Q => format!("{:.2}", self.bands[row].q_factor().unwrap_or_default()),
            Col::Kind => String::new(),
        };
        self.edited(if value.is_empty() {
            format!("band {} {label}", row + 1)
        } else {
            format!("band {} {label} {value}", row + 1)
        });
    }

    /// Cycles the filter shape through the kinds the active layer allows.
    pub fn cycle_kind(&mut self) {
        let allowed: &[BandKind] = if self.experimental {
            &[
                BandKind::Pk,
                BandKind::Ls,
                BandKind::Off,
                BandKind::Hs,
                BandKind::Lp,
                BandKind::Hp,
            ]
        } else {
            &[BandKind::Pk, BandKind::Ls, BandKind::Off]
        };

        let current = self.bands[self.row].kind();
        let index = allowed.iter().position(|&k| k == current).unwrap_or(0);
        let next = allowed[(index + 1) % allowed.len()];
        self.bands[self.row] = retarget(self.bands[self.row], next);
        self.edited(format!("band {} → {}", self.row + 1, next.as_str()));
    }
}

/// Gives an `OFF` band real defaults so a column step has something to move.
#[must_use]
pub fn activate(band: Band) -> Band {
    match band {
        Band::Off => Band::Pk {
            freq_hz: 1_000.0,
            gain_db: 0.0,
            q_factor: 1.0,
        },
        other => other,
    }
}

/// Rebuilds a band, keeping only the fields the current shape actually has.
/// `None` means "leave that field alone".
#[must_use]
pub fn with_values(band: Band, freq: Option<f64>, gain: Option<f64>, q: Option<f64>) -> Band {
    let old_freq = band.freq_hz().unwrap_or(1_000.0);
    let old_gain = band.gain_db();
    let old_q = band.q_factor().unwrap_or(1.0);
    let freq = freq.unwrap_or(old_freq);
    let gain = gain.unwrap_or(old_gain);
    let q = q.unwrap_or(old_q);

    match band {
        Band::Pk { .. } => Band::Pk {
            freq_hz: freq,
            gain_db: gain,
            q_factor: q,
        },
        Band::Ls { .. } => Band::Ls {
            freq_hz: freq,
            gain_db: gain,
            q_factor: q,
        },
        Band::Hs { .. } => Band::Hs {
            freq_hz: freq,
            gain_db: gain,
            q_factor: q,
        },
        Band::Lp { .. } => Band::Lp {
            freq_hz: freq,
            q_factor: q,
        },
        Band::Hp { .. } => Band::Hp {
            freq_hz: freq,
            q_factor: q,
        },
        Band::Off => Band::Off,
    }
}

/// Changes a band's shape, carrying over whatever the new shape can hold.
#[must_use]
pub fn retarget(band: Band, kind: BandKind) -> Band {
    let freq = band.freq_hz().unwrap_or(1_000.0);
    let gain = band.gain_db();
    let q = band.q_factor().unwrap_or(1.0);

    match kind {
        BandKind::Pk => Band::Pk {
            freq_hz: freq,
            gain_db: gain,
            q_factor: q,
        },
        BandKind::Ls => Band::Ls {
            freq_hz: freq,
            gain_db: gain,
            q_factor: q,
        },
        BandKind::Hs => Band::Hs {
            freq_hz: freq,
            gain_db: gain,
            q_factor: q,
        },
        BandKind::Lp => Band::Lp {
            freq_hz: freq,
            q_factor: q,
        },
        BandKind::Hp => Band::Hp {
            freq_hz: freq,
            q_factor: q,
        },
        BandKind::Off => Band::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::neutral(false)
    }

    fn codes(app: &App) -> Vec<&'static str> {
        app.verdict
            .reasons
            .iter()
            .map(|reason| reason.code.as_str())
            .collect()
    }

    #[test]
    fn the_neutral_curve_starts_green() {
        let app = app();
        assert!(app.verdict.ok);
        assert!(codes(&app).is_empty());
        assert!(!app.armed);
        assert_eq!(app.row, 0);
        assert_eq!(app.col, Col::Kind);
    }

    #[test]
    fn vim_motions_stay_inside_the_table() {
        let mut app = app();
        for _ in 0..50 {
            app.handle_key(Key::Char('j'));
        }
        assert_eq!(app.row, BAND_COUNT - 1);
        for _ in 0..50 {
            app.handle_key(Key::Char('k'));
        }
        assert_eq!(app.row, 0);
        for _ in 0..19 {
            app.handle_key(Key::Char('l'));
        }
        assert_eq!(app.col, Col::Q, "l walks through all four columns");
        app.handle_key(Key::Char('l'));
        assert_eq!(app.col, Col::Kind);
        app.handle_key(Key::Char('h'));
        assert_eq!(app.col, Col::Q, "h wraps backwards too");

        app.handle_key(Key::Char('G'));
        assert_eq!(app.row, BAND_COUNT - 1);
        app.handle_key(Key::Char('g'));
        assert_eq!(app.row, 0);
        app.handle_key(Key::Down);
        assert_eq!(app.row, 1);
        app.handle_key(Key::Up);
        assert_eq!(app.row, 0);
    }

    #[test]
    fn arrow_keys_do_the_same_as_hjkl() {
        let mut app = app();
        app.handle_key(Key::Down);
        app.handle_key(Key::Right);
        assert_eq!((app.row, app.col), (1, Col::Freq));
        app.handle_key(Key::Up);
        app.handle_key(Key::Left);
        assert_eq!((app.row, app.col), (0, Col::Kind));
    }

    #[test]
    fn stepping_a_value_updates_the_status() {
        let mut app = app();
        app.handle_key(Key::Char('j'));
        app.handle_key(Key::Char('l'));
        assert_eq!(app.col, Col::Freq);
        app.handle_key(Key::Char('+'));
        assert!(app.status.contains("Hz"), "status was {}", app.status);
        assert!(
            app.verdict.ok,
            "a small step should still pass: {:?}",
            codes(&app)
        );
    }

    #[test]
    fn a_boost_that_breaks_the_envelope_is_refused_live() {
        let mut app = app();
        app.handle_key(Key::Char('c')); // band 1 → PK
        app.handle_key(Key::Char('l')); // → freq
        app.handle_key(Key::Char('l')); // → gain
        for _ in 0..80 {
            app.handle_key(Key::Char('+'));
        }
        assert!(!app.verdict.ok, "a huge boost must be refused");
        assert!(!codes(&app).is_empty());
        assert!(
            app.handle_key(Key::Char('w')) == Effect::Nothing,
            "arming is refused when the gate refuses"
        );
        assert!(!app.armed);
        assert!(app.status.contains("refused"), "status was {}", app.status);
    }

    #[test]
    fn arming_needs_the_gate_and_sending_needs_a_second_press() {
        let mut app = app();
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Nothing);
        assert!(app.armed);
        assert!(app.status.contains("armed"));
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Write);
        assert!(!app.armed, "a write consumes the arm");
        assert_eq!(app.handle_key(Key::Esc), Effect::Quit);
        assert!(app.quit);
    }

    #[test]
    fn esc_cancels_an_armed_write_before_it_quits() {
        let mut app = app();
        app.handle_key(Key::Char('w'));
        assert_eq!(app.handle_key(Key::Esc), Effect::Nothing);
        assert!(!app.armed);
        assert_eq!(app.handle_key(Key::Esc), Effect::Quit);
    }

    #[test]
    fn an_edit_disarms_but_a_cursor_move_does_not() {
        let mut app = app();
        app.handle_key(Key::Char('w'));
        assert!(app.armed);

        // Walking around changes nothing about the curve, so the arm survives.
        app.handle_key(Key::Char('j'));
        app.handle_key(Key::Char('l'));
        assert!(app.armed, "moving the cursor is not an edit");

        // Changing a value is, and the arm must not outlive it.
        app.handle_key(Key::Char('+'));
        assert!(!app.armed, "an edit must clear a pending write");

        // So does changing what judges the curve.
        app.handle_key(Key::Char('w'));
        assert!(app.armed);
        app.handle_key(Key::Char('t'));
        assert!(!app.armed, "switching profile must clear a pending write");
    }

    #[test]
    fn the_yolo_profile_waives_and_says_so() {
        let mut app = App::neutral(true);
        app.preamp_db = -26.0;
        app.recompute();
        assert!(app.verdict.ok, "yolo accepts what standard refuses");
        assert_eq!(app.verdict.waived.as_ref().map(Vec::len), Some(1));

        app.handle_key(Key::Char('t'));
        assert_eq!(app.profile, Profile::Standard);
        assert!(!app.verdict.ok, "standard refuses the same curve");
        assert!(app.verdict.waived.is_none());
    }

    #[test]
    fn experimental_unlocks_the_engineering_kinds() {
        let mut app = app();
        app.handle_key(Key::Char('c'));
        assert_eq!(app.bands[0].kind(), BandKind::Pk);
        app.handle_key(Key::Char('c'));
        assert_eq!(app.bands[0].kind(), BandKind::Ls);
        app.handle_key(Key::Char('c'));
        assert_eq!(app.bands[0].kind(), BandKind::Off, "ajustar stops at OFF");

        app.handle_key(Key::Char('e'));
        assert!(app.experimental);
        app.handle_key(Key::Char('c'));
        assert_eq!(
            app.bands[0].kind(),
            BandKind::Hs,
            "engineering carries on into HS"
        );
        assert!(
            !codes(&app).contains(&"k:kind-not-allowed"),
            "HS needs the engineering layer: {:?}",
            codes(&app)
        );
    }

    #[test]
    fn x_switches_a_band_off_and_the_curve_still_encodes() {
        let mut app = app();
        app.handle_key(Key::Char('c')); // band 1 is PK now
        assert_ne!(app.bands[0].kind(), BandKind::Off);
        app.handle_key(Key::Char('x'));
        assert_eq!(app.bands[0].kind(), BandKind::Off);
        assert!(app.verdict.metrics.payload_crc.is_some());
    }

    #[test]
    fn n_and_r_restore_known_curves() {
        let mut app = app();
        app.handle_key(Key::Char('c'));
        app.handle_key(Key::Char('r'));
        assert_eq!(app.bands, NEUTRAL_BANDS.to_vec());

        app.handle_key(Key::Char('c'));
        app.handle_key(Key::Char('n'));
        assert_eq!(app.bands, NEUTRAL_BANDS.to_vec());
        assert_eq!(app.preamp_db, -0.5);
        assert!(app.verdict.ok);
    }

    #[test]
    fn an_off_band_activates_before_it_is_stepped() {
        let mut app = app();
        assert_eq!(app.bands[0], Band::Off);
        app.handle_key(Key::Char('l')); // freq
        app.handle_key(Key::Char('+'));
        assert_ne!(app.bands[0], Band::Off);
        assert!(app.bands[0].freq_hz().unwrap() > 1_000.0);
    }

    #[test]
    fn gain_steps_stop_at_the_policy_limits() {
        let mut app = app();
        app.handle_key(Key::Char('c')); // PK
        app.handle_key(Key::Char('l'));
        app.handle_key(Key::Char('l')); // gain
        for _ in 0..200 {
            app.handle_key(Key::Char('+'));
        }
        assert!((app.bands[0].gain_db() - GAIN_MAX_DB).abs() < 1e-9);
        for _ in 0..400 {
            app.handle_key(Key::Char('-'));
        }
        assert!((app.bands[0].gain_db() - GAIN_MIN_DB).abs() < 1e-9);
    }

    #[test]
    fn a_pass_through_shape_reports_instead_of_moving() {
        let mut bands = NEUTRAL_BANDS.to_vec();
        bands[0] = Band::Lp {
            freq_hz: 1_000.0,
            q_factor: 0.707,
        };
        let mut app = App::new(bands, -0.5, false);
        app.col = Col::Gain;
        app.handle_key(Key::Char('+'));
        assert_eq!(
            app.bands[0],
            Band::Lp {
                freq_hz: 1_000.0,
                q_factor: 0.707
            },
            "LP takes no gain"
        );
        assert!(app.status.contains("takes no gain"), "{}", app.status);
    }

    #[test]
    fn an_off_band_does_move_when_stepped() {
        let mut app = app();
        assert_eq!(app.bands[0], Band::Off);
        app.col = Col::Gain;
        app.handle_key(Key::Char('+'));
        assert_ne!(app.bands[0], Band::Off, "OFF activates into a PK band");
        assert!(app.bands[0].gain_db() > 0.0);
    }

    #[test]
    fn column_labels_and_wrapping_are_consistent() {
        assert_eq!(Col::from_index(0), Col::Kind);
        assert_eq!(Col::from_index(3), Col::Q);
        assert_eq!(Col::from_index(4), Col::Kind, "the fourth step wraps");
        assert_eq!(Col::Kind.label(), "kind");
        assert_eq!(Col::Freq.label(), "freq");
        assert_eq!(Col::Gain.label(), "gain");
        assert_eq!(Col::Q.label(), "q");
        assert_eq!(Col::Kind.next(), Col::Freq);
        assert_eq!(Col::Q.next(), Col::Kind, "l wraps at the end");
        assert_eq!(Col::Kind.prev(), Col::Q, "h wraps at the start");
        assert_eq!(Col::Kind.index(), 0);
        assert_eq!(Col::Q.index(), 3);
    }

    #[test]
    fn retarget_and_with_values_respect_the_shape() {
        let pk = Band::Pk {
            freq_hz: 500.0,
            gain_db: -3.0,
            q_factor: 2.0,
        };
        assert_eq!(
            retarget(pk, BandKind::Lp),
            Band::Lp {
                freq_hz: 500.0,
                q_factor: 2.0
            }
        );
        assert_eq!(retarget(pk, BandKind::Off), Band::Off);
        assert_eq!(
            with_values(pk, Some(250.0), None, None),
            Band::Pk {
                freq_hz: 250.0,
                gain_db: -3.0,
                q_factor: 2.0
            }
        );
        assert_eq!(with_values(Band::Off, None, None, None), Band::Off);
        assert_eq!(
            activate(Band::Off),
            Band::Pk {
                freq_hz: 1_000.0,
                gain_db: 0.0,
                q_factor: 1.0
            }
        );
    }
}

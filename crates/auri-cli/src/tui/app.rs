//! The TUI's state machine: bands, cursor, profile and the write handshake.
//!
//! Everything here is pure — no terminal, no USB — so the whole interaction is
//! unit-testable. [`crate::tui::mod`] maps real key events onto [`Key`] and
//! performs the write [`Effect::Write`] asks for.

use std::path::PathBuf;

use auri_codec::{Band, BandKind, NEUTRAL_BANDS};
use auri_gate::{GAIN_MAX_DB, GAIN_MIN_DB, GateOptions, GateResult, Profile, gate_for_write};
use auri_policy::{
    ChecklistContext, ChecklistInput, ChecklistResult, Confirmations, FlashState, ItemId,
    JournalStatus, LOCKED_TEXT, evaluate_checklist, flash_path, journal_path, journal_read,
    last_status, last_written_level_db, load_flash, now_ms as policy_now_ms,
};

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
    /// Tab.
    Tab,
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

/// Which pane owns `j`/`k` and `Space`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The band table (the default).
    Bands,
    /// The pre-write checklist.
    Checklist,
}

/// The session's injected world: clock and state files. Tests build one with
/// fixed values, so nothing here touches the real clock or the real flash.
#[derive(Debug, Clone)]
pub struct SessionEnv {
    /// Epoch ms the editor starts at.
    pub now_ms: i64,
    /// Where the flash counters live.
    pub flash_path: PathBuf,
    /// Where the write journal lives.
    pub journal_path: PathBuf,
}

/// The editable state of the TUI.
#[derive(Debug, Clone)]
pub struct App {
    /// The ten bands being edited.
    pub bands: Vec<Band>,
    /// The curve the TUI started from, for `r` (reset).
    pub original: Vec<Band>,
    /// Position in `crate::presets::PRESETS`, while the curve is that preset
    /// verbatim. Any edit clears it; `{`/`}` cycle it.
    pub preset_index: Option<usize>,
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
    /// Which pane owns `j`/`k` and `Space`.
    pub focus: Focus,
    /// Cursor inside the checklist's confirmations.
    pub confirm_row: usize,
    /// The user's ticks.
    pub confirmations: Confirmations,
    /// Flash counters and the spacing anchor.
    pub flash: FlashState,
    /// `level_db` of the newest journaled write, if any.
    pub last_written_level_db: Option<f64>,
    /// Status of the newest journal entry, if any.
    pub last_journal_status: Option<JournalStatus>,
    /// The pre-write checklist, refreshed after every change.
    pub checklist: ChecklistResult,
    /// The injected clock, moved forward by [`App::tick`].
    pub now_ms: i64,
    /// What a write would go to (the runner updates it after opening a sink).
    pub target: String,
    /// The session's clock and state files.
    pub env: SessionEnv,
}

impl App {
    /// Builds the editor around a curve with the real clock and state files.
    #[must_use]
    pub fn new(bands: Vec<Band>, preamp_db: f64, yolo: bool) -> Self {
        Self::with_env(
            bands,
            preamp_db,
            yolo,
            SessionEnv {
                now_ms: policy_now_ms(),
                flash_path: flash_path(),
                journal_path: journal_path(),
            },
        )
    }

    /// Builds the editor around a curve and an injected session.
    #[must_use]
    pub fn with_env(bands: Vec<Band>, preamp_db: f64, yolo: bool, env: SessionEnv) -> Self {
        let profile = if yolo {
            Profile::Yolo
        } else {
            Profile::Standard
        };
        let flash = load_flash(&env.flash_path, env.now_ms);
        let (written_level, journal_status) = match journal_read(&env.journal_path) {
            Ok(entries) => (last_written_level_db(&entries), last_status(&entries)),
            // An unreadable journal is "no memory": conservative, never fatal.
            Err(_) => (None, None),
        };
        let placeholder = GateResult {
            ok: false,
            reasons: Vec::new(),
            waived: None,
            waived_reasons: None,
            profile: None,
            metrics: Default::default(),
        };
        let input = ChecklistInput {
            bands: &bands,
            preamp_db,
            profile,
            experimental: false,
            confirmations: Confirmations::default(),
        };
        let context = ChecklistContext {
            flash: &flash,
            now_ms: env.now_ms,
            last_written_level_db: written_level,
            last_journal_status: journal_status,
        };
        let checklist = evaluate_checklist(&input, &context, Some(&placeholder));
        let mut app = App {
            bands,
            original: Vec::new(),
            preset_index: None,
            preamp_db,
            row: 0,
            col: Col::Kind,
            profile,
            experimental: false,
            armed: false,
            quit: false,
            status: String::new(),
            verdict: placeholder,
            focus: Focus::Bands,
            confirm_row: 0,
            confirmations: Confirmations::default(),
            flash,
            last_written_level_db: written_level,
            last_journal_status: journal_status,
            checklist,
            now_ms: env.now_ms,
            target: "dry-run".to_string(),
            env,
        };
        app.original = app.bands.clone();
        app.clamp_row();
        app.recompute();
        app.status = "ready".to_string();
        app
    }

    /// The band under the cursor, or `None` when the curve has no bands.
    fn band(&self) -> Option<&Band> {
        self.bands.get(self.row)
    }

    /// The band under the cursor, mutably, or `None` when the curve has no bands.
    fn band_mut(&mut self) -> Option<&mut Band> {
        self.bands.get_mut(self.row)
    }

    /// Keeps `row` inside the curve after construction or a curve replacement.
    /// An empty curve keeps row 0; every edit then becomes a safe no-op.
    fn clamp_row(&mut self) {
        self.row = if self.bands.is_empty() {
            0
        } else {
            self.row.min(self.bands.len() - 1)
        };
    }

    /// The ten pass-through sections at the official −0.5 dB preamp.
    ///
    /// Only the tests build a curve from nothing: the binary always starts
    /// from a [`crate::curve::CurveFile`], which already knows this default.
    #[cfg(test)]
    #[must_use]
    pub fn neutral(yolo: bool) -> Self {
        App::with_env(NEUTRAL_BANDS.to_vec(), -0.5, yolo, test_env(0))
    }

    /// Re-judges the current curve with the current options, then refreshes
    /// the checklist from that same verdict (never a second gate).
    pub fn recompute(&mut self) {
        self.verdict = gate_for_write(
            &self.bands,
            self.preamp_db,
            GateOptions::for_write(self.experimental, self.profile),
        );
        self.refresh_checklist();
    }

    /// Moves the injected clock forward: spacing, counters and the checklist
    /// all follow it. The runner calls this every loop turn.
    pub fn tick(&mut self, now_ms: i64) {
        if now_ms != self.now_ms {
            self.now_ms = now_ms;
            self.refresh_checklist();
        }
    }

    /// Re-evaluates the checklist from the current verdict, flash state and
    /// ticks — one gate, one result, every screen.
    pub fn refresh_checklist(&mut self) {
        let result = {
            let input = ChecklistInput {
                bands: &self.bands,
                preamp_db: self.preamp_db,
                profile: self.profile,
                experimental: self.experimental,
                confirmations: self.confirmations,
            };
            let context = ChecklistContext {
                flash: &self.flash,
                now_ms: self.now_ms,
                last_written_level_db: self.last_written_level_db,
                last_journal_status: self.last_journal_status,
            };
            evaluate_checklist(&input, &context, Some(&self.verdict))
        };
        self.checklist = result;
    }

    /// The confirmations `Space` can tick: the required ones, in order.
    #[must_use]
    pub fn visible_confirmations(&self) -> Vec<ItemId> {
        self.checklist
            .items
            .iter()
            .filter(|item| {
                item.required
                    && matches!(
                        item.id,
                        ItemId::Volume
                            | ItemId::Connection
                            | ItemId::Experimental
                            | ItemId::YoloSession
                            | ItemId::YoloWrite
                    )
            })
            .map(|item| item.id)
            .collect()
    }

    /// Flips the confirmation under the checklist cursor. Unticking one that
    /// `can_write` needs disarms a pending write immediately.
    fn toggle_confirmation(&mut self) {
        let visible = self.visible_confirmations();
        if visible.is_empty() {
            return;
        }
        let index = self.confirm_row.min(visible.len() - 1);
        match visible[index] {
            ItemId::Volume => {
                self.confirmations.volume_lowered = !self.confirmations.volume_lowered;
            }
            ItemId::Connection => {
                self.confirmations.keep_connected = !self.confirmations.keep_connected;
            }
            ItemId::Experimental => {
                self.confirmations.experimental = !self.confirmations.experimental;
            }
            ItemId::YoloSession => {
                self.confirmations.yolo_session = !self.confirmations.yolo_session;
            }
            ItemId::YoloWrite => {
                self.confirmations.yolo_write = !self.confirmations.yolo_write;
            }
            ItemId::Gate | ItemId::Spacing => return,
        }
        self.refresh_checklist();
        if self.armed && !self.checklist.can_write {
            self.armed = false;
            self.status = "disarmed: an item is missing".to_string();
        }
    }

    /// Every edit invalidates an armed write: arming must be the last thing
    /// that happens before a send, never something that survives a change.
    /// An edit also ends the "this is an official preset" marker: the curve
    /// on screen is no longer the preset verbatim.
    fn edited(&mut self, status: impl Into<String>) {
        self.armed = false;
        self.preset_index = None;
        self.recompute();
        self.status = status.into();
    }

    /// Loads the preset at `index` into the editor. A preset load is an edit:
    /// it goes through [`App::edited`], so an armed write never survives it —
    /// the marker is set *after* `edited` cleared it. The loaded curve becomes
    /// the new `r` target, so `r` and the `[RO]` marker always agree.
    fn load_preset(&mut self, index: usize) {
        let Some(preset) = crate::presets::PRESETS.get(index) else {
            return;
        };
        self.bands = preset.bands.to_vec();
        self.preamp_db = preset.preamp_db;
        self.original = self.bands.clone();
        self.clamp_row();
        let label = preset.label;
        self.edited(format!("loaded preset {label}"));
        self.preset_index = Some(index);
    }

    /// Steps to the previous (`-1`) or next (`+1`) preset through the built-in
    /// list, wrapping at both ends. From a custom curve, `}` starts at the
    /// first preset and `{` at the last.
    fn cycle_preset(&mut self, direction: i32) {
        let len = i32::try_from(crate::presets::PRESETS.len()).unwrap_or(0);
        if len == 0 {
            return;
        }
        // From a custom curve `}` starts at the first preset and `{` at the
        // last: "no preset" counts as one step before the first (for `}`) or
        // at the first (for `{`).
        let current = self
            .preset_index
            .map_or(if direction >= 0 { -1 } else { 0 }, |index| {
                i32::try_from(index).unwrap_or(0)
            });
        let next = (current + direction).rem_euclid(len) as usize;
        self.load_preset(next);
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
            Key::Tab => {
                self.focus = match self.focus {
                    Focus::Bands => Focus::Checklist,
                    Focus::Checklist => Focus::Bands,
                };
                Effect::Nothing
            }
            Key::Char(' ') if self.focus == Focus::Checklist => {
                self.toggle_confirmation();
                Effect::Nothing
            }
            // In the checklist, j/k walk the confirmations instead.
            Key::Char('j') | Key::Down if self.focus == Focus::Checklist => {
                let len = self.visible_confirmations().len();
                self.confirm_row = if len == 0 {
                    0
                } else {
                    (self.confirm_row + 1).min(len - 1)
                };
                Effect::Nothing
            }
            Key::Char('k') | Key::Up if self.focus == Focus::Checklist => {
                self.confirm_row = self.confirm_row.saturating_sub(1);
                Effect::Nothing
            }
            // Vim motions: h/l walk the columns, j/k walk the bands.
            Key::Char('j') | Key::Down => {
                if !self.bands.is_empty() {
                    self.row = (self.row + 1).min(self.bands.len() - 1);
                }
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
                self.row = self.bands.len().saturating_sub(1);
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
                let Some(band) = self.band().copied() else {
                    return Effect::Nothing;
                };
                if band.kind() == BandKind::Off {
                    self.status = format!("band {} is already off", self.row + 1);
                } else {
                    if let Some(slot) = self.band_mut() {
                        *slot = Band::Off;
                    }
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
            Key::Char('{') => {
                self.cycle_preset(-1);
                Effect::Nothing
            }
            Key::Char('}') => {
                self.cycle_preset(1);
                Effect::Nothing
            }
            Key::Char('n') => {
                self.bands = NEUTRAL_BANDS.to_vec();
                self.preamp_db = -0.5;
                self.clamp_row();
                self.edited("loaded Neutro".to_string());
                Effect::Nothing
            }
            Key::Char('r') => {
                self.bands = self.original.clone();
                self.clamp_row();
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
                } else if !self.checklist.can_write {
                    self.status = LOCKED_TEXT.to_string();
                } else {
                    self.armed = true;
                    self.status = "armed: press w again to write, Esc to cancel".to_string();
                }
                Effect::Nothing
            }
            Key::Char('?') => {
                self.status = "hjkl move · +/- step · c kind · x off · { } preset · t yolo · e layer · Tab focus · Space tick · w write · q quit"
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
        let Some(current) = self.bands.get(row).copied() else {
            // An empty curve has nothing to step: a safe no-op.
            return;
        };
        let band = activate(current);

        match col {
            Col::Kind => {
                self.cycle_kind();
                return;
            }
            Col::Freq => {
                let freq = band.freq_hz().unwrap_or(1_000.0) * LOG_STEP.powi(direction);
                let next = with_values(band, Some(freq.clamp(FREQ_MIN, FREQ_MAX)), None, None);
                if let Some(slot) = self.bands.get_mut(row) {
                    *slot = next;
                }
            }
            Col::Gain => {
                if !band.kind().has_gain() {
                    self.status = format!("{} takes no gain", band.kind().as_str());
                    return;
                }
                let gain = (band.gain_db() + GAIN_STEP * f64::from(direction))
                    .clamp(GAIN_MIN_DB, GAIN_MAX_DB);
                let next = with_values(band, None, Some(gain), None);
                if let Some(slot) = self.bands.get_mut(row) {
                    *slot = next;
                }
            }
            Col::Q => {
                let q = band.q_factor().unwrap_or(1.0) * LOG_STEP.powi(direction);
                let next = with_values(band, None, None, Some(q.clamp(Q_MIN, Q_MAX)));
                if let Some(slot) = self.bands.get_mut(row) {
                    *slot = next;
                }
            }
        }

        let label = col.label();
        let value = match col {
            Col::Freq => format!(
                "{:.1} Hz",
                self.band().and_then(|b| b.freq_hz()).unwrap_or_default()
            ),
            Col::Gain => format!("{:+.1} dB", self.band().map_or(0.0, |b| b.gain_db())),
            Col::Q => format!(
                "{:.2}",
                self.band().and_then(|b| b.q_factor()).unwrap_or_default()
            ),
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

        let Some(current) = self.band().map(|b| b.kind()) else {
            return;
        };
        let index = allowed.iter().position(|&k| k == current).unwrap_or(0);
        let next = allowed[(index + 1) % allowed.len()];
        if let Some(slot) = self.band_mut() {
            *slot = retarget(*slot, next);
        }
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

/// A session that never touches the real clock or the real state files.
#[cfg(test)]
fn test_env(now_ms: i64) -> SessionEnv {
    let stem = std::env::temp_dir().join(format!("auri-app-test-{}", std::process::id()));
    SessionEnv {
        now_ms,
        flash_path: stem.with_extension("flash.json"),
        journal_path: stem.with_extension("journal.jsonl"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::neutral(false)
    }

    /// Neutral, spaced-in, confirmations ticked: everything the checklist
    /// demands is satisfied, so `w` arms.
    fn ready(yolo: bool) -> App {
        let mut app = App::with_env(NEUTRAL_BANDS.to_vec(), -0.5, yolo, test_env(0));
        app.tick(2_000);
        app.confirmations.volume_lowered = true;
        app.confirmations.keep_connected = true;
        if yolo {
            app.confirmations.yolo_session = true;
            app.confirmations.yolo_write = true;
        }
        app.refresh_checklist();
        app
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
        assert_eq!(app.row, auri_codec::BAND_COUNT - 1);
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
        assert_eq!(app.row, auri_codec::BAND_COUNT - 1);
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
        let mut app = ready(false);
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
        let mut app = ready(false);
        app.handle_key(Key::Char('w'));
        assert_eq!(app.handle_key(Key::Esc), Effect::Nothing);
        assert!(!app.armed);
        assert_eq!(app.handle_key(Key::Esc), Effect::Quit);
    }

    #[test]
    fn an_edit_disarms_but_a_cursor_move_does_not() {
        let mut app = ready(false);
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
        let mut app = App::with_env(bands, -0.5, false, test_env(0));
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

    #[test]
    fn arming_is_locked_until_the_checklist_is_complete() {
        let mut app = app(); // unticked confirmations, spacing still running
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Nothing);
        assert!(!app.armed);
        assert_eq!(app.status, LOCKED_TEXT);
        assert!(!app.checklist.can_write);
        assert!(
            app.checklist
                .item(ItemId::Volume)
                .is_some_and(|item| !item.satisfied)
        );
    }

    #[test]
    fn tab_focuses_the_checklist_and_space_ticks_the_row() {
        let mut app = App::with_env(NEUTRAL_BANDS.to_vec(), -0.5, false, test_env(0));
        app.tick(2_000);
        assert_eq!(app.focus, Focus::Bands);
        app.handle_key(Key::Tab);
        assert_eq!(app.focus, Focus::Checklist);

        // j walks the confirmations, Space ticks the one under the cursor.
        app.handle_key(Key::Char('j'));
        assert_eq!(app.confirm_row, 1, "connection is second");
        app.handle_key(Key::Char(' '));
        assert!(app.confirmations.keep_connected);
        app.handle_key(Key::Char('k'));
        app.handle_key(Key::Char(' '));
        assert!(app.confirmations.volume_lowered);
        assert!(app.checklist.can_write, "spacing passed at tick(2000)");
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Nothing);
        assert!(app.armed);
    }

    #[test]
    fn unticking_a_required_item_disarms_a_pending_write() {
        let mut app = ready(false);
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Nothing);
        assert!(app.armed);
        app.handle_key(Key::Tab);
        app.handle_key(Key::Char(' ')); // volume is the first row
        assert!(!app.confirmations.volume_lowered);
        assert!(!app.armed, "an unticked item must clear the arm");
        assert!(app.status.contains("disarmed"), "{}", app.status);
    }

    #[test]
    fn yolo_writes_need_their_own_confirmations() {
        let mut app = App::with_env(NEUTRAL_BANDS.to_vec(), -0.5, true, test_env(0));
        app.tick(2_000);
        app.confirmations.volume_lowered = true;
        app.confirmations.keep_connected = true;
        app.refresh_checklist();
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Nothing);
        assert!(!app.armed, "yolo-session and yolo-write are still unticked");
        assert_eq!(app.status, LOCKED_TEXT);

        app.handle_key(Key::Tab);
        app.handle_key(Key::Char('j'));
        app.handle_key(Key::Char('j')); // volume, connection → yolo-session
        app.handle_key(Key::Char(' '));
        app.handle_key(Key::Char('j'));
        app.handle_key(Key::Char(' '));
        assert!(
            app.checklist.can_write,
            "{:?}",
            app.checklist.missing().collect::<Vec<_>>()
        );
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Nothing);
        assert!(app.armed);
    }

    #[test]
    fn empty_band_list_is_a_safe_no_op() {
        let mut app = App::with_env(Vec::new(), -0.5, false, test_env(0));
        assert_eq!(app.row, 0);
        for key in [
            Key::Char('j'),
            Key::Char('k'),
            Key::Char('G'),
            Key::Char('g'),
            Key::Down,
            Key::Up,
            Key::Char('+'),
            Key::Char('-'),
            Key::Char('x'),
            Key::Char('n'),
            Key::Char('r'),
            Key::Char(','),
            Key::Char('.'),
        ] {
            app.handle_key(key);
        }
        app.adjust(1);
        app.adjust(-1);
        app.cycle_kind();
        // Row stays 0, no panic, verdict is a codec refusal (empty curve).
        assert_eq!(app.row, 0);
        assert!(!app.verdict.ok, "empty curve must not pass the gate");
    }

    #[test]
    fn single_band_curve_clamps_row() {
        let mut app = App::with_env(vec![Band::Off], -0.5, false, test_env(0));
        for _ in 0..20 {
            app.handle_key(Key::Char('j'));
        }
        assert_eq!(app.row, 0, "row clamps to the only band");
        app.handle_key(Key::Char('G'));
        assert_eq!(app.row, 0);
        app.handle_key(Key::Char('+'));
        app.adjust(1);
        app.cycle_kind();
    }

    #[test]
    fn preset_cycling_loads_official_curves_and_wraps() {
        let mut app = app();
        assert_eq!(app.preset_index, None, "file curve: no preset marker");
        // From a custom curve, `}` starts at the first preset.
        app.handle_key(Key::Char('}'));
        assert_eq!(app.preset_index, Some(0));
        assert_eq!(
            app.bands,
            crate::presets::PRESETS[0].bands.to_vec(),
            "the loaded curve must be the official one, verbatim"
        );
        assert_eq!(app.preamp_db, crate::presets::PRESETS[0].preamp_db);
        assert!(app.verdict.ok, "official presets pass the standard gate");

        app.handle_key(Key::Char('}'));
        assert_eq!(app.preset_index, Some(1));
        for _ in 0..crate::presets::PRESETS.len() {
            app.handle_key(Key::Char('}'));
        }
        assert_eq!(app.preset_index, Some(1), "cycling a full turn wraps");
        app.handle_key(Key::Char('{'));
        assert_eq!(app.preset_index, Some(0));
        app.handle_key(Key::Char('{'));
        assert_eq!(
            app.preset_index,
            Some(crate::presets::PRESETS.len() - 1),
            "`{{` wraps backwards to the last preset"
        );
    }

    #[test]
    fn loading_a_preset_disarms_and_an_edit_clears_the_marker() {
        let mut app = ready(false);
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Nothing);
        assert!(app.armed);
        app.handle_key(Key::Char('}'));
        assert!(!app.armed, "a preset load is an edit: it must disarm");
        assert_eq!(app.preset_index, Some(0));

        // Cursor motion is not an edit: the marker survives it.
        app.handle_key(Key::Char('j'));
        assert_eq!(app.preset_index, Some(0));
        // Stepping a value is an edit: the curve is no longer the preset.
        app.handle_key(Key::Char('+'));
        assert_eq!(app.preset_index, None);

        // `r` restores the preset just loaded (it became the session target),
        // and the marker is already gone — restore never resurrects a stale one.
        app.handle_key(Key::Char('r'));
        assert_eq!(app.bands, crate::presets::PRESETS[0].bands.to_vec());
        assert_eq!(app.preset_index, None);
    }
}

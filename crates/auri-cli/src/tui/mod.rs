//! The interactive editor: vim motions, a live gate verdict, a real write.
//!
//! The rules are the same ones the CLI follows — nothing is sent without the
//! gate, nothing is retried, and a refusal is a refusal — but here the verdict
//! is recomputed on every keystroke, so the curve can never get ahead of it.

mod app;
mod plot;
mod ui;

pub use app::{App, Effect, Key};

use std::time::Duration;

use auri_codec::encode_reports;
use auri_gate::level_db;
use auri_policy::{
    JournalEntry, JournalStatus, can_write_now, journal_append, now_ms, record_write, save_flash,
};
use auri_transport::{CurveWriter, HidSink, RealHid};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::curve::CurveFile;

/// How long to wait for a key before redrawing.
const TICK: Duration = Duration::from_millis(200);

/// Runs the TUI until the user quits.
///
/// # Errors
///
/// Terminal setup or I/O problems. A write that fails is reported on the
/// status line instead, so the editor never dies because a cable was
/// unplugged at the wrong moment.
pub fn run(
    curve: CurveFile,
    preset_index: Option<usize>,
    commit: bool,
    yolo: bool,
) -> Result<(), String> {
    let mut app = App::new(curve.eq_bands, curve.preamp_db, yolo);
    app.preset_index = preset_index;
    app.target = if commit { "the cable" } else { "dry-run" }.to_string();
    // try_init, never init: a missing or unusable TTY must be an error the
    // caller prints, not a panic — and a half-applied raw mode still gets
    // undone before we return.
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(format!("cannot open the terminal: {error}"));
        }
    };
    let outcome = event_loop(&mut terminal, &mut app, commit);
    ratatui::restore();
    outcome
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, commit: bool) -> Result<(), String> {
    loop {
        app.tick(now_ms());
        terminal
            .draw(|frame| ui::render(frame, app))
            .map_err(|error| error.to_string())?;

        if !event::poll(TICK).map_err(|error| error.to_string())? {
            continue;
        }
        let Event::Key(raw) = event::read().map_err(|error| error.to_string())? else {
            continue;
        };
        // Terminals that report key releases would otherwise fire every
        // binding twice.
        if raw.kind == KeyEventKind::Release {
            continue;
        }

        match app.handle_key(decode(raw)) {
            Effect::Nothing => {}
            Effect::Quit => break,
            Effect::Write => send(app, commit),
        }
    }
    Ok(())
}

/// Maps a terminal key onto the editor's small, testable alphabet.
///
/// Ctrl/Alt chords never reach the bindings: in raw mode Ctrl+C arrives as
/// a key event, and a stray Ctrl+W must not arm — let alone write — a curve.
/// Ctrl+C quits (quitting never writes); every other chord is ignored.
fn decode(raw: KeyEvent) -> Key {
    if raw
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return match (raw.modifiers, raw.code) {
            (modifiers, KeyCode::Char('c')) if modifiers.contains(KeyModifiers::CONTROL) => {
                Key::Char('q')
            }
            _ => Key::Other,
        };
    }
    match raw.code {
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        _ => Key::Other,
    }
}

/// Performs an armed write. Dry-run by default, exactly like every other
/// command in this binary; the gate has already said yes by the time we get
/// here, and it is re-checked because an arm never survives an edit.
fn send(app: &mut App, commit: bool) {
    app.recompute();
    if !app.verdict.ok {
        app.status = format!(
            "gate refused {} reason(s) — nothing was written",
            app.verdict.reasons.len()
        );
        return;
    }

    if !commit {
        // RAM only: no spacing, no counters, no journal — nothing touches flash.
        match encode_reports(&app.bands, app.preamp_db) {
            Ok(reports) => {
                let crc = crc_of(app);
                app.status = format!(
                    "dry-run: {} frames, CRC {crc} — pass --commit to send them",
                    reports.len()
                );
            }
            Err(error) => app.status = format!("encode failed: {error}"),
        }
        return;
    }

    // Only a commit waits on the flash spacing — and it never races it.
    let spacing = can_write_now(&app.flash, app.now_ms);
    if !spacing.ok {
        app.status = format!(
            "flash spacing: wait {} ms before the next write (nothing was written)",
            spacing.wait_ms
        );
        return;
    }

    let opened = RealHid::open().map_err(|error| error.to_string());
    match opened {
        Ok(sink) => {
            let mut writer = CurveWriter::new(sink);
            committed_write(app, &mut writer);
        }
        Err(error) => app.status = format!("no cable: {error}"),
    }
}

/// The CRC the gate reported for this curve, or `unknown`.
fn crc_of(app: &App) -> String {
    app.verdict
        .metrics
        .payload_crc
        .clone()
        .unwrap_or_else(|| "unknown".to_string())
}

/// One commit write through an open sink: never retried, always counted, and
/// journaled exactly once after the transport settles. A counter or journal
/// problem is a warning in the status line — it never sends anything again.
fn committed_write<S: HidSink>(app: &mut App, writer: &mut CurveWriter<S>) {
    let label = writer.sink().label().to_string();
    app.target = label.clone();
    let crc = crc_of(app);

    let reports = match encode_reports(&app.bands, app.preamp_db) {
        Ok(reports) => reports,
        Err(error) => {
            app.status = format!("encode failed: {error}");
            return;
        }
    };
    let outcome = writer.write_curve(&reports, true);
    let stamped = now_ms();

    // Spacing first: a commit attempt consumes it even when the transport
    // failed, so nothing can race the flash.
    let mut notes: Vec<String> = Vec::new();
    match record_write(&app.flash, stamped) {
        Ok(next) => app.flash = next,
        // An unusable clock loses the counters, never the spacing anchor.
        Err(_) => app.flash.last_write_at = Some(stamped),
    }
    if let Err(error) = save_flash(&app.env.flash_path, &app.flash) {
        notes.push(format!("counters not saved: {error}"));
    }

    let status = if outcome.is_ok() {
        JournalStatus::Ok
    } else {
        JournalStatus::Interrupted
    };
    let entry = JournalEntry {
        ts: stamped,
        status,
        crc: crc.clone(),
        bands: app.bands.clone(),
        preamp_db: app.preamp_db,
        profile: app.profile,
        label: label.clone(),
    };
    if let Err(error) = journal_append(&app.env.journal_path, &entry) {
        notes.push(format!("journal: {error}"));
    }

    let base = match outcome {
        Ok(()) => {
            app.last_written_level_db = level_db(&app.bands, app.preamp_db)
                .ok()
                .filter(|level| level.is_finite());
            app.last_journal_status = Some(JournalStatus::Ok);
            // Every write re-confirms YOLO.
            app.confirmations.yolo_write = false;
            format!("written to the cable. CRC {crc}.")
        }
        Err(error) => {
            app.last_journal_status = Some(JournalStatus::Interrupted);
            format!("write failed after {label}: {error}")
        }
    };
    app.status = if notes.is_empty() {
        base
    } else {
        format!("{base} · {}", notes.join(" · "))
    };
    app.refresh_checklist();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Neutral, spaced-in, confirmations ticked: `w` arms.
    fn ready() -> App {
        let mut app = App::neutral(false);
        app.tick(2_000);
        app.confirmations.volume_lowered = true;
        app.confirmations.keep_connected = true;
        app.refresh_checklist();
        app
    }

    #[test]
    fn the_decoder_only_passes_through_what_the_editor_knows() {
        assert_eq!(decode(key(KeyCode::Char('j'))), Key::Char('j'));
        assert_eq!(decode(key(KeyCode::Char('G'))), Key::Char('G'));
        assert_eq!(decode(key(KeyCode::Up)), Key::Up);
        assert_eq!(decode(key(KeyCode::Down)), Key::Down);
        assert_eq!(decode(key(KeyCode::Left)), Key::Left);
        assert_eq!(decode(key(KeyCode::Right)), Key::Right);
        assert_eq!(decode(key(KeyCode::Enter)), Key::Enter);
        assert_eq!(decode(key(KeyCode::Esc)), Key::Esc);
        assert_eq!(decode(key(KeyCode::Tab)), Key::Tab);
        assert_eq!(decode(key(KeyCode::Home)), Key::Other);
        assert_eq!(decode(key(KeyCode::F(5))), Key::Other);
        assert_eq!(decode(key(KeyCode::BackTab)), Key::Other);
        // SHIFT is not a chord: uppercase letters keep working.
        assert_eq!(
            decode(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Key::Char('G')
        );
    }

    #[test]
    fn chords_never_reach_the_bindings() {
        // In raw mode Ctrl+C/W arrive as key events: they must not cycle a
        // column, arm, or write — Ctrl+C quits (quitting never writes).
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(decode(ctrl_w), Key::Other);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(decode(ctrl_c), Key::Char('q'));
        let alt_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(decode(alt_x), Key::Other);
        let ctrl_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(decode(ctrl_enter), Key::Other);

        let mut app = ready();
        assert_eq!(app.handle_key(decode(ctrl_w)), Effect::Nothing);
        assert!(!app.armed, "a chord must never arm a write");
        assert_eq!(app.handle_key(decode(ctrl_c)), Effect::Quit);
    }

    #[test]
    fn opening_the_editor_creates_no_state_files() {
        let dir = std::env::temp_dir().join(format!("auri-startup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let env = app::SessionEnv {
            now_ms: 0,
            flash_path: dir.join("flash.json"),
            journal_path: dir.join("journal.jsonl"),
        };

        let app = App::with_env(auri_codec::NEUTRAL_BANDS.to_vec(), -0.5, false, env);

        assert_eq!(app.status, "ready");
        assert_eq!(
            std::fs::read_dir(&dir).expect("temp dir").count(),
            0,
            "startup is read-only: it must not create flash or journal files"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_reports_the_frames_and_the_crc() {
        let mut app = ready();
        app.handle_key(Key::Char('w'));
        assert_eq!(app.handle_key(Key::Char('w')), Effect::Write);
        send(&mut app, false);
        assert!(
            app.status.contains("dry-run") && app.status.contains("0x"),
            "status was {}",
            app.status
        );
        assert!(app.status.contains("--commit"), "{}", app.status);
    }

    #[test]
    fn a_refused_curve_never_reaches_the_transport() {
        let mut app = App::neutral(false);
        app.preamp_db = -0.2;
        app.recompute();
        // Even a forced Effect::Write must be caught by the re-check.
        send(&mut app, true);
        assert!(app.status.contains("gate refused"), "{}", app.status);
        assert!(
            !app.status.contains("written to"),
            "nothing may be written: {}",
            app.status
        );
    }

    /// A sink that always fails, standing in for an unplugged cable. The
    /// transport path is exercised without depending on whether a device is
    /// actually attached to the test machine.
    struct FailingHid;

    impl HidSink for FailingHid {
        fn label(&self) -> &str {
            "no cable"
        }

        fn write(&mut self, _bytes: &[u8]) -> Result<(), auri_transport::TransportError> {
            Err(auri_transport::TransportError::NoDevice)
        }
    }

    #[test]
    fn a_missing_cable_is_a_status_not_a_crash() {
        let mut app = ready();
        let mut writer = CurveWriter::new(FailingHid);
        committed_write(&mut app, &mut writer);
        assert!(!app.status.is_empty(), "the user must learn what happened");
        assert!(
            !app.status.contains("written to"),
            "no cable means no write: {}",
            app.status
        );
        assert!(
            app.status.contains("no Kuba Auri cable found"),
            "the transport error must reach the status: {}",
            app.status
        );
        assert_eq!(
            app.last_journal_status,
            Some(JournalStatus::Interrupted),
            "a failed write is interrupted, never ok"
        );
    }

    #[test]
    fn a_commit_write_waits_for_the_flash_spacing() {
        let mut app = App::neutral(false); // anchored at now = 0
        app.confirmations.volume_lowered = true;
        app.confirmations.keep_connected = true;
        app.refresh_checklist();
        send(&mut app, true);
        assert!(app.status.contains("flash spacing"), "{}", app.status);
        assert!(!app.status.contains("written to"), "{}", app.status);
    }

    #[test]
    fn a_journal_failure_never_sends_a_second_time() {
        use auri_transport::RecordingHid;

        let mut app = ready();
        // A directory cannot be appended to: the journal write must fail.
        let dir = std::env::temp_dir().join(format!("auri-journal-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        app.env.journal_path = dir.clone();

        let mut writer = CurveWriter::new(RecordingHid::new("test sink"));
        committed_write(&mut app, &mut writer);

        assert_eq!(
            writer.sink().len(),
            10,
            "nine reports plus one commit, exactly once"
        );
        assert!(app.status.contains("written to"), "{}", app.status);
        assert!(
            app.status.contains("journal"),
            "the failure must warn: {}",
            app.status
        );
        assert_eq!(app.last_journal_status, Some(JournalStatus::Ok));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_curve_file_starts_the_editor_where_it_says() {
        let curve = CurveFile::neutral();
        let app = App::new(curve.eq_bands, curve.preamp_db, false);
        assert!(app.verdict.ok);
        assert_eq!(app.preamp_db, -0.5);
        assert!(!app.quit);
    }
}

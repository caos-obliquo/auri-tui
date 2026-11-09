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
use auri_transport::{CurveWriter, HidSink, RealHid};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};

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
pub fn run(curve: CurveFile, commit: bool, yolo: bool) -> Result<(), String> {
    let mut app = App::new(curve.eq_bands, curve.preamp_db, yolo);
    let mut terminal = ratatui::init();
    let outcome = event_loop(&mut terminal, &mut app, commit);
    ratatui::restore();
    outcome
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, commit: bool) -> Result<(), String> {
    loop {
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
fn decode(raw: KeyEvent) -> Key {
    match raw.code {
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
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

    let crc = app
        .verdict
        .metrics
        .payload_crc
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let reports = match encode_reports(&app.bands, app.preamp_db) {
        Ok(reports) => reports,
        Err(error) => {
            app.status = format!("encode failed: {error}");
            return;
        }
    };

    if !commit {
        app.status = format!(
            "dry-run: {} frames, CRC {crc} — pass --commit to send them",
            reports.len()
        );
        return;
    }

    let opened = RealHid::open().map_err(|error| error.to_string());
    let mut writer = match opened {
        Ok(sink) => CurveWriter::new(sink),
        Err(error) => {
            app.status = format!("no cable: {error}");
            return;
        }
    };
    let label = writer.sink().label().to_string();

    match writer.write_curve(&reports, true) {
        Ok(()) => app.status = format!("written to {label}. CRC {crc}"),
        Err(error) => app.status = format!("write failed after {label}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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
        assert_eq!(decode(key(KeyCode::Home)), Key::Other);
        assert_eq!(decode(key(KeyCode::F(5))), Key::Other);
        assert_eq!(decode(key(KeyCode::BackTab)), Key::Other);
    }

    #[test]
    fn a_dry_run_reports_the_frames_and_the_crc() {
        let mut app = App::neutral(false);
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

    #[test]
    fn a_missing_cable_is_a_status_not_a_crash() {
        let mut app = App::neutral(false);
        app.handle_key(Key::Char('w'));
        app.handle_key(Key::Char('w'));
        send(&mut app, true);
        assert!(!app.status.is_empty(), "the user must learn what happened");
        assert!(
            !app.status.contains("written to"),
            "no cable means no write: {}",
            app.status
        );
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

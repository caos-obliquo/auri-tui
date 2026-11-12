//! `auri` — offline terminal control for the Kuba Auri DSP cable.
//!
//! Every command is dry-run by default: the bytes are printed, nothing is
//! sent. `--commit` runs the safety gate first and writes only a curve the
//! gate approves, because nothing reaches the cable without a gate.

mod autoeq;
mod curve;
mod export;
mod presets;
mod tui;

use std::io::Write;
use std::process::ExitCode;

use auri_gate::{GateOptions, GateResult, Profile, Reason, ReasonValue, gate_for_write, level_db};
use auri_policy::{
    FLASH_NOTICE_TEXT, JournalEntry, JournalStatus, LOUDNESS_WARNING_TEXT, LoudnessInput,
    can_write_now, counters, daily_notice, flash_path, journal_append, journal_path, journal_read,
    last_status, last_written_level_db, load_flash, loudness_warning, now_ms, record_write,
    save_flash,
};
use auri_transport::{CurveWriter, HidSink, RealHid};
use clap::{Parser, Subcommand};

use crate::curve::CurveFile;
use crate::export::ExportFormat;

/// Arguments shared by the AutoEq fit.
#[derive(Debug, clap::Args)]
struct FitArgs {
    /// Two-column CSV: frequency in Hz, measured response in dB.
    measurement: std::path::PathBuf,
    /// Two-column CSV: frequency in Hz, target response in dB.
    target: std::path::PathBuf,
    /// Where to write the fitted curve.
    #[arg(short, long, value_name = "PATH")]
    output: std::path::PathBuf,
    /// Judge under the yolo profile instead of standard.
    #[arg(long)]
    yolo: bool,
}

#[derive(Debug, Subcommand)]
enum AutoEqCommand {
    /// Fit an AutoEq-style measurement to a target and emit curve.json.
    Fit(FitArgs),
}

/// Offline terminal control for the Kuba Auri (CB5100) DSP cable.
#[derive(Debug, Parser)]
#[command(name = "auri", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the cables this user can open.
    Devices,
    /// Fit an AutoEq-style measurement to a target curve.
    #[command(name = "autoeq")]
    AutoEq {
        #[command(subcommand)]
        command: AutoEqCommand,
    },
    /// Show a curve as the nine reports and its safety verdict.
    Show {
        /// Path to a JSON curve file ({"preampDb": .., "eqBands": [..]}).
        #[arg(required_unless_present = "preset", conflicts_with = "preset")]
        file: Option<std::path::PathBuf>,
        /// Built-in preset id instead of a file (see `auri presets`).
        #[arg(long, value_name = "ID")]
        preset: Option<String>,
        /// Run the safety gate and print the verdict.
        #[arg(long)]
        commit: bool,
        /// Judge under the yolo profile.
        #[arg(long)]
        yolo: bool,
    },
    /// List the built-in Kuba Auri presets.
    Presets,
    /// Print a curve in another tool's format (never writes to the cable).
    Export {
        /// Path to a JSON curve file.
        #[arg(required_unless_present = "preset", conflicts_with = "preset")]
        file: Option<std::path::PathBuf>,
        /// Built-in preset id instead of a file (see `auri presets`).
        #[arg(long, value_name = "ID")]
        preset: Option<String>,
        /// Target format.
        #[arg(long, value_enum, default_value_t = ExportFormat::Apo)]
        format: ExportFormat,
    },
    /// Encode a curve and (dry-run) print the frames.
    Write {
        /// Path to a JSON curve file.
        #[arg(required_unless_present = "preset", conflicts_with = "preset")]
        file: Option<std::path::PathBuf>,
        /// Built-in preset id instead of a file (see `auri presets`).
        #[arg(long, value_name = "ID")]
        preset: Option<String>,
        /// Run the safety gate and write RAM, then flash. Refused if the gate
        /// refuses.
        #[arg(long)]
        commit: bool,
        /// Judge under the yolo profile: class-2 reasons are waived instead of
        /// blocking. Still a gate, still never a bypass of the hard reasons.
        #[arg(long)]
        yolo: bool,
    },
    /// Encode the neutral preset (ten pass-through sections).
    Neutral {
        /// Run the safety gate and write RAM, then flash.
        #[arg(long)]
        commit: bool,
        /// Judge under the yolo profile.
        #[arg(long)]
        yolo: bool,
    },
    /// Interactive editor: vim motions, a live gate verdict, a real write.
    Tui {
        /// Curve to start from (defaults to the neutral preset).
        #[arg(conflicts_with = "preset")]
        file: Option<std::path::PathBuf>,
        /// Built-in preset id to start from (see `auri presets`).
        #[arg(long, value_name = "ID")]
        preset: Option<String>,
        /// Send to the cable instead of dry-running. Still gated, still
        /// needs an explicit `w` twice.
        #[arg(long)]
        commit: bool,
        /// Judge under the yolo profile.
        #[arg(long)]
        yolo: bool,
        /// Draw the interface with plain ASCII instead of Unicode glyphs.
        #[arg(long)]
        ascii: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            err(format_args!("auri: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// Writes one line to stdout. A closed stdout (broken pipe) is not a panic:
/// the command simply has nobody left to print to.
#[allow(clippy::write_with_newline)]
fn out(args: std::fmt::Arguments<'_>) {
    let mut buf = Vec::new();
    let _ = write!(buf, "{args}\n");
    let _ = std::io::stdout().write_all(&buf);
}

/// Writes one line to stderr, with the same broken-pipe tolerance as [`out`].
#[allow(clippy::write_with_newline)]
fn err(args: std::fmt::Arguments<'_>) {
    let mut buf = Vec::new();
    let _ = write!(buf, "{args}\n");
    let _ = std::io::stderr().write_all(&buf);
}

fn run() -> Result<(), String> {
    match Cli::parse().command {
        Command::Devices => {
            let found = auri_transport::RealHid::list().map_err(|e| e.to_string())?;
            if found.is_empty() {
                out(format_args!(
                    "no cable found ({}:{:04x})",
                    auri_codec::USB_VID,
                    auri_codec::USB_PID
                ));
                out(format_args!(
                    "check the udev rules: 99-kuba-auri.rules, then replug"
                ));
            } else {
                for entry in found {
                    out(format_args!("{entry}"));
                }
            }
            Ok(())
        }
        Command::AutoEq {
            command:
                AutoEqCommand::Fit(FitArgs {
                    measurement,
                    target,
                    output,
                    yolo,
                }),
        } => {
            let result = autoeq::fit_files(&measurement, &target)?;
            let verdict = judge(&result.curve, yolo);
            print_verdict(&verdict, yolo);
            if !verdict.ok {
                return refused(&verdict);
            }
            autoeq::write_curve(&result.curve, &output)?;
            out(format_args!(
                "autoeq fit: {} points, RMS {:.2} dB (from {:.2} dB), max error {:.2} dB",
                result.points, result.rms_after_db, result.rms_before_db, result.max_error_db
            ));
            out(format_args!("wrote {}", output.display()));
            out(format_args!(
                "not written to the cable; use `auri write {} --commit`",
                output.display()
            ));
            Ok(())
        }
        Command::Presets => {
            out(format_args!(
                "{:<10}  {:<16}  {:>9}  crc",
                "id", "label", "preamp"
            ));
            for preset in &presets::PRESETS {
                out(format_args!(
                    "{:<10}  {:<16}  {:>+6.1} dB  0x{:04x}",
                    preset.id, preset.label, preset.preamp_db, preset.crc
                ));
            }
            out(format_args!(""));
            out(format_args!(
                "load one with: auri tui --preset <id>   (or auri write --preset <id>)"
            ));
            Ok(())
        }
        Command::Show {
            file,
            preset,
            commit: _,
            yolo: _,
        } => {
            let curve = match (file, preset) {
                (Some(path), None) => CurveFile::load(&path).map_err(|e| e.to_string())?,
                (None, Some(id)) => presets::find(&id)
                    .map(presets::Preset::curve)
                    .ok_or_else(|| format!("unknown preset {id:?}; try `auri presets`"))?,
                (None, None) => CurveFile::neutral(),
                (Some(_), Some(_)) => {
                    return Err("pass a curve file or --preset, not both".to_string());
                }
            };
            let verdict = judge(&curve, false);
            print_verdict(&verdict, false);
            print_report(curve.encode().map_err(|e| e.to_string())?);
            Ok(())
        }
        Command::Export {
            file,
            preset,
            format,
        } => {
            let curve = curve_arg(file, preset)?;
            out(format_args!(
                "{}",
                export::render(&curve, format).trim_end()
            ));
            Ok(())
        }
        Command::Write {
            file,
            preset,
            commit,
            yolo,
        } => {
            let curve = curve_arg(file, preset)?;
            send(&curve, commit, yolo)
        }
        Command::Neutral { commit, yolo } => send(&CurveFile::neutral(), commit, yolo),
        Command::Tui {
            file,
            preset,
            commit,
            yolo,
            ascii,
        } => {
            let preset_index = preset.as_deref().and_then(presets::index_of);
            let curve = curve_arg(file, preset)?;
            tui::run(curve, preset_index, commit, yolo, ascii)
        }
    }
}

/// The curve the user named: a built-in preset, a JSON file, or the neutral
/// curve when neither is given.
fn curve_arg(
    file: Option<std::path::PathBuf>,
    preset: Option<String>,
) -> Result<CurveFile, String> {
    match (file, preset) {
        (Some(path), None) => CurveFile::load(&path).map_err(|e| e.to_string()),
        (None, Some(id)) => presets::find(&id)
            .map(presets::Preset::curve)
            .ok_or_else(|| format!("unknown preset {id:?}; try `auri presets`")),
        (None, None) => Ok(CurveFile::neutral()),
        (Some(_), Some(_)) => Err("pass a curve file or --preset, not both".to_string()),
    }
}

/// The verdict this curve would get on the wire: always the `full` grid, the
/// layer the CLI edits in, and the profile the user asked for.
fn judge(curve: &CurveFile, yolo: bool) -> GateResult {
    let profile = if yolo {
        Profile::Yolo
    } else {
        Profile::Standard
    };
    gate_for_write(
        &curve.eq_bands,
        curve.preamp_db,
        GateOptions::for_write(false, profile),
    )
}

fn send(curve: &CurveFile, commit: bool, yolo: bool) -> Result<(), String> {
    let reports = curve.encode().map_err(|e| e.to_string())?;
    let verdict = judge(curve, yolo);
    print_verdict(&verdict, yolo);

    if !commit {
        out(format_args!(
            "dry-run: nothing written (pass --commit to write the cable)"
        ));
        print_report(reports);
        return refused(&verdict);
    }

    if !verdict.ok {
        return refused(&verdict);
    }

    // The flash counters: an unknown anchor means the full wait, never
    // "no cooldown", and a commit never races it.
    let now = now_ms();
    let flash_path = flash_path();
    let mut flash = load_flash(&flash_path, now);
    // Persist the anchor before the spacing check: a refused write must still
    // start the wait, or every fresh invocation re-anchors `last_write_at` to
    // its own start time and the first-ever commit can never succeed. Only the
    // anchor is written here — no counter moves and nothing is re-sent.
    if let Err(error) = save_flash(&flash_path, &flash) {
        err(format_args!("auri: flash counters not saved ({error})"));
    }
    let spacing = can_write_now(&flash, now);
    if !spacing.ok {
        return Err(format!(
            "flash spacing: wait {} ms before the next write (nothing was written)",
            spacing.wait_ms
        ));
    }

    // The journal's memory of the last write. Unreadable ⇒ no history, which
    // is the conservative answer for every question asked below.
    let journal_path = journal_path();
    let entries = match journal_read(&journal_path) {
        Ok(entries) => entries,
        Err(error) => {
            err(format_args!(
                "auri: journal unreadable ({error}); assuming no history"
            ));
            Vec::new()
        }
    };
    let last_level = last_written_level_db(&entries);
    let last_status = last_status(&entries);

    let level = level_db(&curve.eq_bands, curve.preamp_db)
        .ok()
        .filter(|value| value.is_finite());
    let loudness = loudness_warning(LoudnessInput {
        new_level_db: level,
        last_written_level_db: last_level,
        // This process has not written yet: first write of the session is
        // unknown state until the journal says otherwise.
        session_write_count: flash.session,
        last_journal_status: last_status,
    });

    let mut writer = CurveWriter::new(RealHid::open().map_err(|e| e.to_string())?);
    let label = writer.sink().label().to_string();
    out(format_args!("writing to {label} ..."));
    let outcome = writer.write_curve(&reports, true);
    let stamped = now_ms();

    // Counters first: a commit attempt consumes the spacing even when the
    // transport failed, so nothing can race the flash.
    match record_write(&flash, stamped) {
        Ok(next) => flash = next,
        // An unusable clock loses the counters, never the spacing anchor.
        Err(_) => flash.last_write_at = Some(stamped),
    }
    if let Err(error) = save_flash(&flash_path, &flash) {
        err(format_args!("auri: flash counters not saved ({error})"));
    }

    // Exactly one journal entry, after the transport settles. A failure here
    // warns — it never re-sends a write that already happened.
    let entry = JournalEntry {
        ts: stamped,
        status: if outcome.is_ok() {
            JournalStatus::Ok
        } else {
            JournalStatus::Interrupted
        },
        crc: verdict
            .metrics
            .payload_crc
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        bands: curve.eq_bands.clone(),
        preamp_db: curve.preamp_db,
        profile: if yolo {
            Profile::Yolo
        } else {
            Profile::Standard
        },
        label,
    };
    if let Err(error) = journal_append(&journal_path, &entry) {
        err(format_args!(
            "auri: journal not appended ({error}); nothing is re-sent"
        ));
    }

    outcome.map_err(|e| e.to_string())?;

    let crc = verdict.metrics.payload_crc.as_deref().unwrap_or("unknown");
    out(format_args!("written to the cable. CRC {crc}"));
    if let Some(level) = level {
        out(format_args!("predicted level 300 Hz–3 kHz: {level:+.2} dB"));
    }
    match last_level {
        Some(last) => out(format_args!("last written curve: {last:+.2} dB")),
        None => out(format_args!("last written curve: unknown")),
    }
    if let Some(delta) = loudness.delta_db {
        out(format_args!("jump: {delta:+.2} dB"));
    }
    if loudness.warn {
        out(format_args!("{LOUDNESS_WARNING_TEXT}"));
    }
    let counters = counters(&flash, stamped);
    out(format_args!(
        "flash counters: {} this session · {} today · {} in total",
        counters.session, counters.day, counters.total
    ));
    out(format_args!(
        "2 s spacing between writes respected (next write in 2.0 s)"
    ));
    if daily_notice(&flash, stamped) {
        out(format_args!("{FLASH_NOTICE_TEXT}"));
    }
    Ok(())
}

/// A dry run still prints its frames; only a refusal of the *write* is fatal.
fn refused(verdict: &GateResult) -> Result<(), String> {
    if verdict.ok {
        return Ok(());
    }
    Err(format!(
        "the safety gate refused this curve ({} blocking reason(s)); \
         nothing was written",
        verdict.reasons.len()
    ))
}

fn print_verdict(verdict: &GateResult, yolo: bool) {
    let profile = if yolo { "yolo" } else { "standard" };
    if verdict.ok {
        out(format_args!(
            "gate: PASS ({profile} profile), CRC {}",
            verdict.metrics.payload_crc.as_deref().unwrap_or("unknown")
        ));
    } else {
        out(format_args!("gate: REFUSED ({profile} profile)"));
    }
    for reason in &verdict.reasons {
        out(format_args!("  {}", describe(reason)));
    }
    for reason in verdict.waived_reasons.iter().flatten() {
        out(format_args!("  waived: {}", describe(reason)));
    }
}

fn describe(reason: &Reason) -> String {
    let mut out = reason.code.as_str().to_string();
    if let Some(stage) = reason.stage {
        out.push_str(&format!(" at band {stage}"));
    }
    match &reason.value {
        ReasonValue::Number(value) => out.push_str(&format!(" = {value}")),
        ReasonValue::Text(text) => out.push_str(&format!(" = {text}")),
        ReasonValue::Null(_) => {}
    }
    if let Some(limit) = reason.limit {
        out.push_str(&format!(" (limit {limit})"));
    }
    out
}

fn print_report(reports: Vec<[u8; auri_codec::REPORT_SIZE]>) {
    for report in &reports {
        let mut hex = String::with_capacity(report.len() * 3);
        for byte in report {
            hex.push_str(&format!("{byte:02x} "));
        }
        out(format_args!(
            "report {} ({:02x}): {hex}",
            report[0],
            auri_codec::HID_REPORT_ID
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_neutral_preset_passes_the_gate() {
        let verdict = judge(&CurveFile::neutral(), false);
        assert!(
            verdict.ok,
            "neutral must be writable: {:?}",
            verdict.reasons
        );
        assert_eq!(
            verdict.metrics.payload_crc.as_deref(),
            Some("0x4fff"),
            "the neutral CRC the web app shows before writing"
        );
    }

    #[test]
    fn a_refusal_names_its_reasons() {
        let mut curve = CurveFile::neutral();
        curve.preamp_db = -0.2;
        let verdict = judge(&curve, false);
        assert!(!verdict.ok);
        let text = verdict
            .reasons
            .iter()
            .map(describe)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("g:preamp"), "{text}");
        assert!(text.contains("-0.5"), "{text}");
    }

    #[test]
    fn yolo_waives_but_never_silently() {
        let mut curve = CurveFile::neutral();
        curve.preamp_db = -26.0;
        let standard = judge(&curve, false);
        let yolo = judge(&curve, true);
        assert!(!standard.ok);
        assert!(yolo.ok);
        assert_eq!(
            yolo.waived.as_deref(),
            Some([auri_gate::ReasonCode::Preamp].as_slice())
        );
    }
}

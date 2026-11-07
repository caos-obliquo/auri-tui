//! `auri` — offline terminal control for the Kuba Auri DSP cable.
//!
//! Every command is dry-run by default: the bytes are printed, nothing is
//! sent. `--commit` runs the safety gate first and writes only a curve the
//! gate approves, because nothing reaches the cable without a gate.

mod curve;

use std::process::ExitCode;

use auri_gate::{GateOptions, GateResult, Profile, Reason, ReasonValue, gate_for_write};
use auri_transport::{CurveWriter, HidSink, RealHid};
use clap::{Parser, Subcommand};

use crate::curve::CurveFile;

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
    /// Show a curve as the nine reports and its safety verdict.
    Show {
        /// Path to a JSON curve file ({"preampDb": .., "eqBands": [..]}).
        file: std::path::PathBuf,
    },
    /// Encode a curve and (dry-run) print the frames.
    Write {
        /// Path to a JSON curve file.
        file: std::path::PathBuf,
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
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("auri: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    match Cli::parse().command {
        Command::Devices => {
            let found = auri_transport::RealHid::list().map_err(|e| e.to_string())?;
            if found.is_empty() {
                println!(
                    "no cable found ({}:{:04x})",
                    auri_codec::USB_VID,
                    auri_codec::USB_PID
                );
                println!("check the udev rules: 99-kuba-auri.rules, then replug");
            } else {
                for entry in found {
                    println!("{entry}");
                }
            }
            Ok(())
        }
        Command::Show { file } => {
            let curve = CurveFile::load(&file).map_err(|e| e.to_string())?;
            let verdict = judge(&curve, false);
            print_verdict(&verdict, false);
            print_report(curve.encode().map_err(|e| e.to_string())?);
            Ok(())
        }
        Command::Write { file, commit, yolo } => {
            let curve = CurveFile::load(&file).map_err(|e| e.to_string())?;
            send(&curve, commit, yolo)
        }
        Command::Neutral { commit, yolo } => send(&CurveFile::neutral(), commit, yolo),
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
        println!("dry-run: nothing written (pass --commit to write the cable)");
        print_report(reports);
        return refused(&verdict);
    }

    if !verdict.ok {
        return refused(&verdict);
    }

    let mut writer = CurveWriter::new(RealHid::open().map_err(|e| e.to_string())?);
    println!("writing to {} ...", writer.sink().label());
    writer
        .write_curve(&reports, true)
        .map_err(|e| e.to_string())?;
    println!(
        "written to the cable. CRC {}",
        verdict.metrics.payload_crc.as_deref().unwrap_or("unknown")
    );
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
        println!(
            "gate: PASS ({profile} profile), CRC {}",
            verdict.metrics.payload_crc.as_deref().unwrap_or("unknown")
        );
    } else {
        println!("gate: REFUSED ({profile} profile)");
    }
    for reason in &verdict.reasons {
        println!("  {}", describe(reason));
    }
    for reason in verdict.waived_reasons.iter().flatten() {
        println!("  waived: {}", describe(reason));
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
        println!(
            "report {} ({:02x}): {hex}",
            report[0],
            auri_codec::HID_REPORT_ID
        );
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

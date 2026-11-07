//! `auri` — offline terminal control for the Kuba Auri DSP cable.
//!
//! Every command is dry-run by default: the bytes are printed, nothing is
//! sent. `--commit` writes RAM and only then flash, and it stays refused until
//! the safety gate is ported (M2), because nothing reaches the cable without
//! a gate.

mod curve;

use std::process::ExitCode;

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
    /// Show a curve as the nine reports that would be written.
    Show {
        /// Path to a JSON curve file ({"preampDb": .., "eqBands": [..]}).
        file: std::path::PathBuf,
    },
    /// Encode a curve and (dry-run) print the frames.
    Write {
        /// Path to a JSON curve file.
        file: std::path::PathBuf,
        /// Actually write to the cable. Refused until the safety gate lands.
        #[arg(long)]
        commit: bool,
    },
    /// Encode the neutral preset (ten pass-through sections).
    Neutral {
        /// Actually write to the cable. Refused until the safety gate lands.
        #[arg(long)]
        commit: bool,
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
                println!("check the udev rules: 70-open-auri.rules, then replug");
            } else {
                for entry in found {
                    println!("{entry}");
                }
            }
            Ok(())
        }
        Command::Show { file } => {
            let curve = CurveFile::load(&file).map_err(|e| e.to_string())?;
            print_report(curve.encode().map_err(|e| e.to_string())?);
            Ok(())
        }
        Command::Write { file, commit } => {
            let curve = CurveFile::load(&file).map_err(|e| e.to_string())?;
            send(curve.encode().map_err(|e| e.to_string())?, commit)
        }
        Command::Neutral { commit } => {
            let curve = CurveFile::neutral();
            send(curve.encode().map_err(|e| e.to_string())?, commit)
        }
    }
}

fn send(reports: Vec<[u8; auri_codec::REPORT_SIZE]>, commit: bool) -> Result<(), String> {
    if commit {
        return Err(
            "refusing --commit: the safety gate is not ported yet (M2). \
             Nothing reaches the cable without a gate. \
             Drop --commit to see the dry-run frames."
                .to_string(),
        );
    }
    println!("dry-run: nothing written (pass --commit once the gate exists)");
    print_report(reports);
    Ok(())
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

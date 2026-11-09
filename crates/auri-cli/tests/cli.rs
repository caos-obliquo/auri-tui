//! Integration tests for the `auri` binary.
//!
//! The EPIPE test is the important one: a closed stdout must not panic.
//! `out()` buffers to a Vec and ignores write errors, so the process
//! terminates cleanly.

use std::io::Read;
use std::process::{Command, Stdio};

fn auri_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_auri"))
}

#[test]
fn closed_stdout_does_not_panic() {
    let mut child = Command::new(auri_bin())
        .arg("presets")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn auri");
    // Drop the read end immediately: the child has nowhere to write.
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut buf = [0u8; 1];
    let _ = stdout.read(&mut buf);
    drop(stdout);
    let output = child.wait_with_output().expect("wait for auri");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Broken pipe"),
        "closed stdout must not panic, stderr was: {stderr}"
    );
}

#[test]
fn show_preset_passes_gate_and_prints_crc() {
    let output = Command::new(auri_bin())
        .args(["show", "--preset", "neutro"])
        .output()
        .expect("run auri show");
    assert!(output.status.success(), "show --preset neutro: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gate: PASS"), "stdout: {stdout}");
    assert!(stdout.contains("0x4fff"), "stdout: {stdout}");
}

#[test]
fn show_preset_unknown_id_is_named() {
    let output = Command::new(auri_bin())
        .args(["show", "--preset", "does-not-exist"])
        .output()
        .expect("run auri show");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does-not-exist"), "stderr: {stderr}");
}

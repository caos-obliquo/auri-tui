//! Byte-exact parity against the open-auri oracles.
//!
//! The oracles are AGPL-3.0 artefacts and are **not** vendored here. They are
//! read from a sibling `open-auri` checkout, or from `AURI_FIXTURES_DIR`, and
//! every test in this file skips when they are absent.

use std::path::PathBuf;

use auri_codec::{
    Band, COMMIT_COMMAND, HID_REPORT_ID, PAYLOAD_SIZE, PREAMBLE, REPORT_COUNT, REPORT_SIZE,
    SAMPLE_RATE, USB_PID, USB_VID, commit_report, encode_payload, encode_reports,
    reports_from_payload,
};

/// Where the AGPL oracles live, if they are anywhere.
fn fixtures_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AURI_FIXTURES_DIR") {
        let dir = PathBuf::from(dir);
        return dir.join("eq0xcb.reference.json").exists().then_some(dir);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/auri-codec -> crates -> <repo> -> builds
    let sibling = manifest
        .join("../../../open-auri/test/fixtures")
        .canonicalize()
        .ok()?;
    sibling
        .join("eq0xcb.reference.json")
        .exists()
        .then_some(sibling)
}

fn load_reference() -> Option<serde_json::Value> {
    let dir = fixtures_dir()?;
    let text = std::fs::read_to_string(dir.join("eq0xcb.reference.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn bands_from(json: &[serde_json::Value]) -> Vec<Band> {
    serde_json::from_value(serde_json::Value::Array(json.to_vec())).expect("fixture bands parse")
}

#[test]
fn constants_match_the_reference() {
    let Some(reference) = load_reference() else {
        eprintln!("SKIP: open-auri fixtures not found (set AURI_FIXTURES_DIR)");
        return;
    };
    let constants = &reference["constants"];
    assert_eq!(constants["usbVid"].as_u64().unwrap(), u64::from(USB_VID));
    assert_eq!(constants["usbPid"].as_u64().unwrap(), u64::from(USB_PID));
    assert_eq!(
        constants["hidReportId"].as_u64().unwrap(),
        u64::from(HID_REPORT_ID)
    );
    assert_eq!(
        constants["sampleRate"].as_u64().unwrap(),
        u64::from(SAMPLE_RATE)
    );
    let preamble: Vec<u8> = constants["preamble"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(preamble, PREAMBLE);
    let commit: Vec<u8> = constants["commitCommand"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(commit, COMMIT_COMMAND);
}

#[test]
fn every_preset_encodes_byte_for_byte() {
    let Some(reference) = load_reference() else {
        eprintln!("SKIP: open-auri fixtures not found (set AURI_FIXTURES_DIR)");
        return;
    };
    let presets = reference["presets"].as_object().unwrap();
    assert_eq!(presets.len(), 11, "expected the 11 official presets");

    for (name, preset) in presets {
        let bands = bands_from(preset["eqBands"].as_array().unwrap());
        let preamp = preset["preampDb"].as_f64().unwrap();

        let expected_payload: Vec<u8> = preset["payload"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(expected_payload.len(), PAYLOAD_SIZE, "preset {name}");

        let payload = encode_payload(&bands, preamp)
            .unwrap_or_else(|e| panic!("preset {name} failed to encode: {e}"));
        if payload != expected_payload {
            let first = payload
                .iter()
                .zip(&expected_payload)
                .position(|(a, b)| a != b)
                .unwrap();
            panic!(
                "preset {name}: payload diverges at byte {first}: got {:#04x}, want {:#04x}",
                payload[first], expected_payload[first]
            );
        }

        let expected_reports: Vec<[u8; REPORT_SIZE]> = preset["reports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|report| {
                let mut out = [0_u8; REPORT_SIZE];
                for (slot, byte) in report.as_array().unwrap().iter().enumerate() {
                    out[slot] = byte.as_u64().unwrap() as u8;
                }
                out
            })
            .collect();
        let reports = encode_reports(&bands, preamp).unwrap();
        assert_eq!(reports, expected_reports, "preset {name} reports");

        // Round trip: our own payload must re-split to the same reports.
        assert_eq!(reports_from_payload(&payload).unwrap(), expected_reports);
    }
}

#[test]
fn commit_report_is_the_reference_commit_zero_padded() {
    let report = commit_report();
    assert_eq!(&report[..COMMIT_COMMAND.len()], &COMMIT_COMMAND);
    assert!(report[COMMIT_COMMAND.len()..].iter().all(|&b| b == 0));
    assert_eq!(report.len(), REPORT_SIZE);
    assert_eq!(REPORT_COUNT, 9);
}

#[test]
fn neutral_encodes_without_a_device() {
    let reports = encode_reports(&auri_codec::NEUTRAL_BANDS, 0.0).unwrap();
    assert_eq!(reports.len(), REPORT_COUNT);
    assert_eq!(reports[0][0], 0);
}

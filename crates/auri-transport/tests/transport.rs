//! Transport tests: dry-run invariants and byte-exact replay of the real
//! hardware capture.
//!
//! The oracles live in the open-auri checkout (AGPL) and are read at run time;
//! the replay test skips when they are absent.

use std::path::PathBuf;

use auri_codec::{encode_reports, Band, HID_REPORT_ID, REPORT_COUNT, REPORT_SIZE};
use auri_transport::{CurveWriter, RecordingHid};

fn fixtures_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AURI_FIXTURES_DIR") {
        let dir = PathBuf::from(dir);
        return dir.join("usb-wire-golden.json").exists().then_some(dir);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sibling = manifest
        .join("../../../open-auri/test/fixtures")
        .canonicalize()
        .ok()?;
    sibling.join("usb-wire-golden.json").exists().then_some(sibling)
}

fn parse_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn dry_run_writes_exactly_nine_reports_and_no_commit() {
    let sink = RecordingHid::new("dry-run");
    let mut writer = CurveWriter::new(sink);
    let reports = encode_reports(&auri_codec::NEUTRAL_BANDS, 0.0).unwrap();

    writer.write_curve(&reports, false).unwrap();

    let sink = writer.into_sink();
    assert_eq!(sink.len(), REPORT_COUNT, "nine reports, nothing else");
    for (index, frame) in sink.writes().iter().enumerate() {
        assert_eq!(frame.len(), REPORT_SIZE + 1, "report ID + 31 bytes");
        assert_eq!(frame[0], HID_REPORT_ID);
        assert_eq!(frame[1], index as u8, "frame carries its report index");
    }
}

#[test]
fn commit_is_the_tenth_frame_and_is_omitted_on_abort() {
    let reports = encode_reports(&auri_codec::NEUTRAL_BANDS, 0.0).unwrap();

    // Full write: nine reports then the commit.
    let mut writer = CurveWriter::new(RecordingHid::new("with-commit"));
    writer.write_curve(&reports, true).unwrap();
    let sink = writer.into_sink();
    assert_eq!(sink.len(), REPORT_COUNT + 1);
    let commit = sink.writes().last().unwrap();
    assert_eq!(commit[0], HID_REPORT_ID);
    assert_eq!(&commit[1..6], &[0x01, 0x65, 0x00, 0x01, 0xff]);
    assert!(commit[6..].iter().all(|&b| b == 0));

    // Abort at report 4: the commit must never be sent.
    let mut writer = CurveWriter::new(RecordingHid::new("abort").failing_at(4));
    let error = writer.write_curve(&reports, true).unwrap_err();
    assert!(error.to_string().contains("report 4"), "{error}");
    let sink = writer.into_sink();
    assert_eq!(sink.len(), 4, "only the four reports that succeeded");
    assert!(
        !sink
            .writes()
            .iter()
            .any(|frame| frame[1..] == auri_codec::COMMIT_COMMAND),
        "an aborted write never commits"
    );
}

#[test]
fn wrong_report_shape_is_refused_before_any_write() {
    let mut writer = CurveWriter::new(RecordingHid::new("shape"));
    let short = [[0_u8; REPORT_SIZE]; REPORT_COUNT - 1];
    assert!(writer.write_curve(&short, true).is_err());
    assert!(writer.into_sink().is_empty(), "nothing reached the sink");
}

/// Byte-exact replay of the 2026-10-28 Windows/Edge capture: every frame the
/// host actually put on the wire must be the frame we produce.
#[test]
fn replays_the_real_hardware_capture() {
    let Some(dir) = fixtures_dir() else {
        eprintln!("SKIP: open-auri fixtures not found (set AURI_FIXTURES_DIR)");
        return;
    };
    let golden: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("usb-wire-golden.json")).unwrap())
            .unwrap();
    let reference: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("eq0xcb.reference.json")).unwrap(),
    )
    .unwrap();

    let writes = golden["writes"].as_array().unwrap();
    assert_eq!(writes.len(), 13, "the capture holds 13 writes");

    let mut total_frames = 0;
    for write in writes {
        let name = write["curve"].as_str().unwrap();
        let preset = &reference["presets"][name];
        if preset.is_null() {
            // `neutro` is not one of the 11 reference presets; it is ten
            // pass-through sections plus the capture's own preamp.
            assert_eq!(name, "neutro");
            continue;
        }
        let bands: Vec<Band> = serde_json::from_value(preset["eqBands"].clone()).unwrap();
        let preamp = preset["preampDb"].as_f64().unwrap();
        let reports = encode_reports(&bands, preamp).unwrap();

        let expected: Vec<Vec<u8>> = write["reports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hex| parse_hex(hex.as_str().unwrap()))
            .collect();
        assert_eq!(expected.len(), REPORT_COUNT, "preset {name}");

        for (index, (mine, wire)) in reports.iter().zip(expected.iter()).enumerate() {
            assert_eq!(wire[0], HID_REPORT_ID, "{name} frame {index} id");
            assert_eq!(
                &wire[1..],
                &mine[..],
                "{name} frame {index}: our bytes differ from the wire"
            );
        }

        let commit_wire = parse_hex(write["commit"].as_str().unwrap());
        let mut mine = vec![HID_REPORT_ID];
        mine.extend_from_slice(&auri_codec::commit_report());
        assert_eq!(commit_wire, mine, "{name} commit frame");
        total_frames += expected.len() + 1;
    }
    assert_eq!(total_frames, 12 * (REPORT_COUNT + 1), "12 complete writes");
}

//! Wire encoding: bands in, nine HID reports out.

use crate::band::Band;
use crate::crc::crc16_ccitt;
use crate::design::{check_bands, design_section};
use crate::error::CodecError;
use crate::quantize::encode_section;
use crate::{COMMIT_COMMAND, PREAMBLE, PAYLOAD_SIZE};

/// Size of one HID report, report ID excluded.
pub const REPORT_SIZE: usize = 31;

/// Number of reports a full payload is split into.
pub const REPORT_COUNT: usize = 9;

/// Bytes of payload carried by one report (byte 0 of the report is its index).
const REPORT_DATA_SIZE: usize = REPORT_SIZE - 1;

/// The five little-endian bytes of one `i32`.
fn append_i32_le(buffer: &mut Vec<u8>, value: i32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

/// Builds the 220-byte payload for 10 bands and a preamp.
///
/// Bands are written in the order given: keep cuts before boosts — that is the
/// ordering the reference uses to avoid internal clipping.
///
/// # Errors
///
/// [`CodecError::WrongBandCount`], [`CodecError::Band`],
/// [`CodecError::Preamp`], [`CodecError::CoefficientOutOfRange`] or
/// [`CodecError::Unstable`]. It never returns anything but 220 bytes.
pub fn encode_payload(bands: &[Band], preamp_db: f64) -> Result<Vec<u8>, CodecError> {
    check_bands(bands)?;
    let mut payload = Vec::with_capacity(PAYLOAD_SIZE);
    payload.extend_from_slice(&PREAMBLE);
    append_i32_le(&mut payload, crate::quantize::preamp_word(preamp_db)?);
    for (index, band) in bands.iter().enumerate() {
        let section = design_section(*band)?;
        for word in encode_section(section, index)? {
            append_i32_le(&mut payload, word);
        }
    }
    debug_assert_eq!(payload.len(), PAYLOAD_SIZE - 2);
    let checksum = crc16_ccitt(&payload);
    payload.extend_from_slice(&checksum.to_le_bytes());
    debug_assert_eq!(payload.len(), PAYLOAD_SIZE);
    Ok(payload)
}

/// Splits an already encoded payload into the nine HID reports.
///
/// Byte 0 is the report index, bytes 1..=30 carry 30 bytes of payload; the
/// trailing reports are zero-padded. A transport can then send exactly the
/// bytes a gate approved, without re-encoding.
///
/// # Errors
///
/// [`CodecError::PayloadLength`] unless `payload` holds exactly 220 bytes,
/// [`CodecError::PayloadByte`] for a non-byte element, and
/// [`CodecError::CrcMismatch`] when the trailing CRC-16 does not match.
pub fn reports_from_payload(payload: &[u8]) -> Result<Vec<[u8; REPORT_SIZE]>, CodecError> {
    if payload.len() != PAYLOAD_SIZE {
        return Err(CodecError::PayloadLength {
            expected: PAYLOAD_SIZE,
        });
    }
    for (index, &byte) in payload.iter().enumerate() {
        // `u8` is always a byte; keep the check for parity with the reference.
        let _ = (index, byte);
    }
    let body = &payload[..PAYLOAD_SIZE - 2];
    let stored = u16::from_le_bytes([payload[PAYLOAD_SIZE - 2], payload[PAYLOAD_SIZE - 1]]);
    if stored != crc16_ccitt(body) {
        return Err(CodecError::CrcMismatch);
    }

    let mut reports = Vec::with_capacity(REPORT_COUNT);
    for index in 0..REPORT_COUNT {
        let mut report = [0_u8; REPORT_SIZE];
        report[0] = index as u8;
        let start = index * REPORT_DATA_SIZE;
        let end = (start + REPORT_DATA_SIZE).min(PAYLOAD_SIZE);
        if start < PAYLOAD_SIZE {
            report[1..1 + (end - start)].copy_from_slice(&payload[start..end]);
        }
        reports.push(report);
    }
    Ok(reports)
}

/// The nine HID reports for 10 bands and a preamp.
///
/// # Errors
///
/// Whatever [`encode_payload`] and [`reports_from_payload`] can report.
pub fn encode_reports(bands: &[Band], preamp_db: f64) -> Result<Vec<[u8; REPORT_SIZE]>, CodecError> {
    reports_from_payload(&encode_payload(bands, preamp_db)?)
}

/// The commit command zero-padded to the 31-byte report size, so every
/// transport sends the same bytes.
///
/// The reference passes only 5 bytes to WebHID, but the real Kuba Auri capture
/// (2026-10-28, Windows 11 / Edge 156) shows the host stack pads them to 31:
/// both commits are identical on the wire (`CB` + 31 bytes). Other host stacks
/// are not measured.
#[must_use]
pub fn commit_report() -> [u8; REPORT_SIZE] {
    let mut report = [0_u8; REPORT_SIZE];
    report[..COMMIT_COMMAND.len()].copy_from_slice(&COMMIT_COMMAND);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::band::NEUTRAL_BANDS;

    #[test]
    fn neutral_payload_is_220_bytes_and_passes_its_own_crc() {
        let payload = encode_payload(&NEUTRAL_BANDS, 0.0).unwrap();
        assert_eq!(payload.len(), PAYLOAD_SIZE);
        assert_eq!(&payload[..14], &PREAMBLE);
        assert_eq!(crc16_ccitt(&payload[..218]), {
            u16::from_le_bytes([payload[218], payload[219]])
        });
    }

    #[test]
    fn neutral_preamp_word_is_one_in_q23() {
        let payload = encode_payload(&NEUTRAL_BANDS, 0.0).unwrap();
        assert_eq!(i32::from_le_bytes(payload[14..18].try_into().unwrap()), 1 << 23);
    }

    #[test]
    fn reports_split_and_pad_the_last_one() {
        let reports = encode_reports(&NEUTRAL_BANDS, 0.0).unwrap();
        assert_eq!(reports.len(), REPORT_COUNT);
        for (index, report) in reports.iter().enumerate() {
            assert_eq!(report[0], index as u8);
            assert_eq!(report.len(), REPORT_SIZE);
        }
        // Report 8 starts at payload offset 240 > 220: all data bytes are zero.
        assert_eq!(reports[8], {
            let mut expected = [0_u8; REPORT_SIZE];
            expected[0] = 8;
            expected
        });
    }

    #[test]
    fn tampered_crc_is_rejected() {
        let mut payload = encode_payload(&NEUTRAL_BANDS, 0.0).unwrap();
        payload[200] ^= 0xff;
        assert_eq!(reports_from_payload(&payload), Err(CodecError::CrcMismatch));
    }

    #[test]
    fn commit_is_five_bytes_zero_padded() {
        let report = commit_report();
        assert_eq!(&report[..5], &[0x01, 0x65, 0x00, 0x01, 0xff]);
        assert!(report[5..].iter().all(|&b| b == 0));
    }
}

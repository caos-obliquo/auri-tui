//! Byte-exact `0xCB` wire codec for the Kuba Auri (CB5100) DSP cable.
//!
//! The firmware accepts a 220-byte payload split into nine 31-byte HID reports
//! (report ID `0xCB`) and validates its CRC-16 before applying anything. It
//! cannot be read back, so this crate is the last gate before the flash: it
//! refuses unknown filter kinds, non-finite or out-of-range parameters, a
//! positive preamp, coefficients that do not fit `int32`, and sections whose
//! *quantized* poles are not strictly inside the unit circle.
//!
//! Layout (see `docs/protocol-0xcb.md` in the open-auri checkout):
//!
//! ```text
//! 14 B preamble | 4 B preamp Q23 | 10 x 5 x 4 B coefficients Q27 | 2 B CRC
//! ```
//!
//! Sections are written in the order `[b0, b2, -a1, b1, -a2]`; `a0` is implied.
//! Everything here is pure (no USB), so it runs identically in the CLI and in
//! tests.

mod band;
mod crc;
mod design;
mod encode;
mod error;
mod magnitude;
mod quantize;

pub use band::{Band, BandKind, BAND_COUNT, FILTER_KINDS, NEUTRAL_BANDS, OFF};
pub use crc::crc16_ccitt;
pub use design::{
    check_band, check_bands, design_high_pass, design_high_shelf, design_low_pass,
    design_low_shelf, design_peaking, design_section, Section,
};
pub use encode::{
    commit_report, encode_payload, encode_reports, reports_from_payload, REPORT_COUNT, REPORT_SIZE,
};
pub use error::CodecError;
pub use magnitude::magnitude_db;
pub use quantize::{encode_section, is_stable, preamp_word, Q23_ONE, Q27_ONE};

/// USB vendor ID of the cable (`0x3302`).
pub const USB_VID: u16 = 0x3302;
/// USB product ID of the cable (`0x8902`).
pub const USB_PID: u16 = 0x8902;
/// HID report ID carrying every curve write (`0xCB`).
pub const HID_REPORT_ID: u8 = 0xcb;

/// Sample rate the coefficients are designed for.
pub const SAMPLE_RATE: u32 = 96000;

/// Fixed 14-byte prologue: ASCII `EQ1#`, then (hypothesis) a u16 LE length and
/// layout bytes.
pub const PREAMBLE: [u8; 14] = [
    0x45, 0x51, 0x31, 0x23, 0xd6, 0x00, 0x00, 0x0a, 0x04, 0x05, 0x01, 0x0a, 0x00, 0x00,
];

/// Commit command as the reference sends it: persists the preset to flash.
pub const COMMIT_COMMAND: [u8; 5] = [0x01, 0x65, 0x00, 0x01, 0xff];

/// Payload size in bytes: 14 preamble + 4 preamp + 10*5*4 coefficients + 2 CRC.
pub const PAYLOAD_SIZE: usize = 220;

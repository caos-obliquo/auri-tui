//! HID transport for the `0xCB` curve writes.
//!
//! Three interchangeable sinks behind one trait:
//!
//! - [`RealHid`] — the actual cable, over `/dev/hidraw*`.
//! - [`RecordingHid`] — a fake that only records what it was given; used by
//!   tests and by `--dry-run`, the default everywhere.
//!
//! Invariants carried over from the reference transport:
//!
//! - The safety gate runs *before* any byte is handed to a sink, so a rejected
//!   curve never reaches the wire.
//! - A write is never retried. The firmware validates the CRC and rejects a
//!   bad payload silently; resending would only race the device.
//! - An abort means the commit command is never sent, so the flash keeps the
//!   previous preset.

mod fake;
pub mod hidraw;
mod real;
mod writer;

pub use fake::RecordingHid;
pub use real::RealHid;
pub use writer::{CurveWriter, REPORT_GAP_MS, SETTLE_MS, TransportError, WRITE_SPACING_MS};

/// Anything that can consume one report-sized write.
///
/// `bytes` always carries the report ID first: `[0xCB, 31 payload bytes]`,
/// 32 in total, which is what the real capture shows on the wire.
pub trait HidSink {
    /// Human-readable identity, for logs and dry-run output.
    fn label(&self) -> &str;

    /// Send one report. Errors are terminal: nothing is retried.
    fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError>;
}

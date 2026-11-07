//! The write sequence: reports first, commit last, never a retry.

use auri_codec::{commit_report, HID_REPORT_ID, REPORT_COUNT, REPORT_SIZE};

use crate::HidSink;

/// Gap the reference leaves between two reports of one curve.
pub const REPORT_GAP_MS: u64 = 16;
/// Settle time the reference leaves before the commit command.
pub const SETTLE_MS: u64 = 150;
/// Minimum spacing between two commits to flash.
pub const WRITE_SPACING_MS: u64 = 2000;

/// Everything that can stop a curve write.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// No cable with the known VID/PID is present.
    #[error("no Kuba Auri cable found ({vid:04x}:{pid:04x})", vid = auri_codec::USB_VID, pid = auri_codec::USB_PID)]
    NoDevice,

    /// The device could not be opened.
    #[error("could not open the cable: {message}")]
    Open { message: String },

    /// A write failed. Nothing is retried.
    #[error("write failed: {message}")]
    Write { message: String },

    /// The sink refused the write; the commit will not be sent.
    #[error("aborted before commit: {message}")]
    Aborted { message: String },

    /// Wrong report count or size from the caller.
    #[error("expected {expected} reports of {size} bytes, got {got}")]
    BadShape {
        expected: usize,
        size: usize,
        got: usize,
    },
}

/// Drives the nine reports of one curve, then optionally the commit.
///
/// The gate belongs upstream: by the time a `CurveWriter` sees bytes, they are
/// bytes a gate already approved.
pub struct CurveWriter<S: HidSink> {
    sink: S,
}

impl<S: HidSink> CurveWriter<S> {
    /// Wrap a sink (real or recording).
    #[must_use]
    pub fn new(sink: S) -> Self {
        CurveWriter { sink }
    }

    /// Borrow the sink, for inspection in tests.
    #[must_use]
    pub fn sink(&self) -> &S {
        &self.sink
    }

    /// Consume the writer and return the sink.
    #[must_use]
    pub fn into_sink(self) -> S {
        self.sink
    }

    /// Write the nine reports of one curve, then the commit.
    ///
    /// `commit == false` writes RAM only — the curve is live until the device
    /// loses power, and the flash keeps whatever was committed before. That is
    /// the safe mode: a bad curve costs one unplug, not a permanent preset.
    ///
    /// # Errors
    ///
    /// [`TransportError::BadShape`] for a malformed report list, and whatever
    /// the sink reports. On any error the commit is **not** sent.
    pub fn write_curve(
        &mut self,
        reports: &[[u8; REPORT_SIZE]],
        commit: bool,
    ) -> Result<(), TransportError> {
        if reports.len() != REPORT_COUNT {
            return Err(TransportError::BadShape {
                expected: REPORT_COUNT,
                size: REPORT_SIZE,
                got: reports.len(),
            });
        }
        for (index, report) in reports.iter().enumerate() {
            if report[0] != index as u8 {
                return Err(TransportError::BadShape {
                    expected: REPORT_COUNT,
                    size: REPORT_SIZE,
                    got: report[0] as usize,
                });
            }
            let mut frame = [0_u8; REPORT_SIZE + 1];
            frame[0] = HID_REPORT_ID;
            frame[1..].copy_from_slice(report);
            if let Err(error) = self.sink.write(&frame) {
                return Err(TransportError::Aborted {
                    message: format!("report {index}: {error}"),
                });
            }
            pause(REPORT_GAP_MS);
        }
        if commit {
            pause(SETTLE_MS);
            let mut frame = [0_u8; REPORT_SIZE + 1];
            frame[0] = HID_REPORT_ID;
            frame[1..].copy_from_slice(&commit_report());
            self.sink.write(&frame)?;
        }
        Ok(())
    }
}

/// Sleep between frames. Kept as a function so tests can swap in a no-op.
fn pause(milliseconds: u64) {
    if milliseconds == 0 {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(milliseconds));
}

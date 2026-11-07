//! A sink that only records: the dry-run default and every transport test.

use crate::writer::TransportError;
use crate::HidSink;

/// Records every write instead of performing it.
#[derive(Debug, Default)]
pub struct RecordingHid {
    label: String,
    writes: Vec<Vec<u8>>,
    /// Fail the *n*-th write (0-based) with this message, to exercise abort
    /// paths. `None` means never fail.
    fail_at: Option<usize>,
}

impl RecordingHid {
    /// A recorder with a descriptive label.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        RecordingHid {
            label: label.into(),
            writes: Vec::new(),
            fail_at: None,
        }
    }

    /// Make the write at `index` fail, so abort behaviour can be tested.
    #[must_use]
    pub fn failing_at(mut self, index: usize) -> Self {
        self.fail_at = Some(index);
        self
    }

    /// Everything written so far, in order.
    #[must_use]
    pub fn writes(&self) -> &[Vec<u8>] {
        &self.writes
    }

    /// Number of writes performed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.writes.len()
    }

    /// Whether nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }
}

impl HidSink for RecordingHid {
    fn label(&self) -> &str {
        &self.label
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        if self.fail_at == Some(self.writes.len()) {
            return Err(TransportError::Write {
                message: "simulated device failure".to_string(),
            });
        }
        self.writes.push(bytes.to_vec());
        Ok(())
    }
}

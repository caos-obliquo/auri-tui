//! Error type shared by every encoding entry point.

/// Everything that can stop a curve from reaching the wire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// A band failed validation (range, kind, missing field).
    #[error("band {index}: {message}")]
    Band { index: usize, message: String },

    /// The band list is not exactly [`crate::BAND_COUNT`] elements long.
    #[error("expected {expected} bands")]
    WrongBandCount { expected: usize },

    /// A coefficient quantized outside `int32`.
    #[error("band {index}: coefficient does not fit Q27 in int32")]
    CoefficientOutOfRange { index: usize },

    /// Quantized poles on or outside the unit circle.
    #[error("band {index}: unstable section: poles on or outside the unit circle")]
    Unstable { index: usize },

    /// Preamp is not finite, or is positive.
    #[error("preamp must be <= 0 dB, got {got}")]
    Preamp { got: String },

    /// A value that must be `i32` is not.
    #[error("value does not fit int32: {got}")]
    Int32OutOfRange { got: String },

    /// Payload has the wrong length or a non-byte element.
    #[error("payload must be {expected} bytes")]
    PayloadLength { expected: usize },

    /// A payload byte is not in `0..=255`.
    #[error("payload byte {index} is not a byte: {got}")]
    PayloadByte { index: usize, got: String },

    /// Trailing CRC-16 does not match the payload body.
    #[error("payload CRC mismatch")]
    CrcMismatch,

    /// Unknown filter kind.
    #[error("unknown filter kind {got}")]
    UnknownKind { got: String },
}

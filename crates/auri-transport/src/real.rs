//! The real cable, over `/dev/hidraw*`.

use std::fs::File;

use auri_codec::{USB_PID, USB_VID};

use crate::HidSink;
use crate::hidraw::{self, DeviceInfo};
use crate::writer::TransportError;

/// A handle on the physical cable.
pub struct RealHid {
    device: File,
    label: String,
}

/// The cables this crate will talk to.
fn matches(info: &DeviceInfo) -> bool {
    info.vendor_id == USB_VID && info.product_id == USB_PID
}

fn describe(info: &DeviceInfo) -> String {
    info.label()
}

impl RealHid {
    /// Opens the first cable that matches the known VID/PID.
    ///
    /// # Errors
    ///
    /// [`TransportError::NoDevice`] when no cable is plugged in (or the udev
    /// rules do not let this user open it), and [`TransportError::Open`] when
    /// the open itself fails.
    pub fn open() -> Result<Self, TransportError> {
        let info = hidraw::enumerate()?
            .into_iter()
            .find(matches)
            .ok_or(TransportError::NoDevice)?;
        let device = hidraw::open(&info.node)?;
        Ok(RealHid {
            device,
            label: describe(&info),
        })
    }

    /// Lists every cable present, without opening any of them.
    ///
    /// # Errors
    ///
    /// [`TransportError::Open`] when the backend cannot start.
    pub fn list() -> Result<Vec<String>, TransportError> {
        Ok(hidraw::enumerate()?
            .iter()
            .filter(|info| matches(info))
            .map(describe)
            .collect())
    }
}

impl HidSink for RealHid {
    fn label(&self) -> &str {
        &self.label
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        hidraw::write(&mut self.device, bytes)
    }
}

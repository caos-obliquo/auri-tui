//! The real cable, through `hidapi`.

use auri_codec::{USB_PID, USB_VID};

use crate::writer::TransportError;
use crate::HidSink;

/// A handle on the physical cable.
pub struct RealHid {
    device: hidapi::HidDevice,
    label: String,
}

/// `VID:PID` of every cable this crate will talk to.
fn matches(info: &hidapi::DeviceInfo) -> bool {
    info.vendor_id() == USB_VID && info.product_id() == USB_PID
}

fn describe(info: &hidapi::DeviceInfo) -> String {
    format!(
        "{} | serial {} | {:?} {:04x}:{:04x} | usage page 0x{:04x} interface {}",
        info.product_string().unwrap_or("unknown"),
        info.serial_number().unwrap_or("-"),
        info.bus_type(),
        info.vendor_id(),
        info.product_id(),
        info.usage_page(),
        info.interface_number()
    )
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
        let api = hidapi::HidApi::new().map_err(|error| TransportError::Open {
            message: error.to_string(),
        })?;
        let info = api
            .device_list()
            .find(|info| matches(info))
            .ok_or(TransportError::NoDevice)?;
        let label = describe(info);
        let device = api
            .open_path(info.path())
            .map_err(|error| TransportError::Open {
                message: error.to_string(),
            })?;
        Ok(RealHid { device, label })
    }

    /// Lists every cable present, without opening any of them.
    ///
    /// # Errors
    ///
    /// [`TransportError::Open`] when the HID API cannot start.
    pub fn list() -> Result<Vec<String>, TransportError> {
        let api = hidapi::HidApi::new().map_err(|error| TransportError::Open {
            message: error.to_string(),
        })?;
        Ok(api.device_list().filter(|info| matches(info)).map(describe).collect())
    }
}

impl HidSink for RealHid {
    fn label(&self) -> &str {
        &self.label
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let written = self
            .device
            .write(bytes)
            .map_err(|error| TransportError::Write {
                message: error.to_string(),
            })?;
        if written != bytes.len() {
            return Err(TransportError::Write {
                message: format!("short write: {written} of {} bytes", bytes.len()),
            });
        }
        Ok(())
    }
}

//! The Linux hidraw backend: `/dev/hidraw*` without libudev.
//!
//! Everything the cable's identity needs already lives in sysfs, so this
//! module reads it straight from there: the HID device's `uevent` (bus,
//! vendor, product, serial), the report descriptor (usage page), and the USB
//! ancestors (product name, manufacturer, interface number). Nothing is
//! compiled from C and nothing links `libudev`, so the binary can be linked
//! statically.
//!
//! Writes are plain `write(2)` calls on the character device: the sink hands
//! over the bytes and they go out verbatim, report id first.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::writer::TransportError;

/// Where the kernel lists the hidraw nodes.
const HIDRAW_CLASS: &str = "/sys/class/hidraw";

/// The bus a HID device sits on, as the kernel's `BUS_*` numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusType {
    /// USB.
    Usb,
    /// Bluetooth.
    Bluetooth,
    /// I2C.
    I2C,
    /// SPI.
    SPI,
    /// A bus this backend does not name.
    Unknown(u32),
}

impl BusType {
    /// Maps a kernel `BUS_*` number onto a name.
    #[must_use]
    pub const fn from_bus(bus: u32) -> Self {
        match bus {
            0x03 => BusType::Usb,
            0x05 => BusType::Bluetooth,
            0x18 => BusType::I2C,
            0x15 => BusType::SPI,
            other => BusType::Unknown(other),
        }
    }
}

/// What the backend knows about one hidraw node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The character device to open.
    pub node: PathBuf,
    /// The USB vendor id.
    pub vendor_id: u16,
    /// The USB product id.
    pub product_id: u16,
    /// The bus the device sits on.
    pub bus_type: BusType,
    /// The USB product name, when the device has one.
    pub product_string: Option<String>,
    /// The USB manufacturer name, when the device has one.
    pub manufacturer_string: Option<String>,
    /// The device's serial number.
    pub serial_number: Option<String>,
    /// The HID usage page of the first report.
    pub usage_page: u16,
    /// The USB interface number, or `-1` when the device has none.
    pub interface_number: i32,
}

impl DeviceInfo {
    /// The one-line identity the journal records.
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "{} | serial {} | {:?} {:04x}:{:04x} | usage page 0x{:04x} interface {}",
            self.product_string.as_deref().unwrap_or("unknown"),
            self.serial_number.as_deref().unwrap_or("-"),
            self.bus_type,
            self.vendor_id,
            self.product_id,
            self.usage_page,
            self.interface_number,
        )
    }
}

/// Lists the hidraw nodes this backend can talk to.
#[cfg(target_os = "linux")]
pub fn enumerate() -> Result<Vec<DeviceInfo>, TransportError> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(HIDRAW_CLASS).map_err(|error| TransportError::Open {
        message: format!("cannot read {HIDRAW_CLASS}: {error}"),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| TransportError::Open {
            message: format!("cannot read {HIDRAW_CLASS}: {error}"),
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("hidraw") {
            continue;
        }
        if let Some(info) = describe(&entry.path()) {
            found.push(info);
        }
    }
    found.sort_by(|left, right| left.node.cmp(&right.node));
    Ok(found)
}

/// Lists the hidraw nodes this backend can talk to.
#[cfg(not(target_os = "linux"))]
pub fn enumerate() -> Result<Vec<DeviceInfo>, TransportError> {
    Err(TransportError::Open {
        message: "the hidraw backend only works on Linux".to_string(),
    })
}

/// Opens a hidraw node for reading and writing.
#[cfg(target_os = "linux")]
pub fn open(node: &Path) -> Result<File, TransportError> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .map_err(|error| TransportError::Open {
            message: format!("cannot open {}: {error}", node.display()),
        })
}

/// Opens a hidraw node for reading and writing.
#[cfg(not(target_os = "linux"))]
pub fn open(_node: &Path) -> Result<File, TransportError> {
    Err(TransportError::Open {
        message: "the hidraw backend only works on Linux".to_string(),
    })
}

/// Writes one report to an open hidraw node, verbatim.
///
/// A short write is an error: the device never saw the whole report, and
/// resending it would race the device, so the write is never retried.
#[cfg(target_os = "linux")]
pub fn write(node: &mut File, bytes: &[u8]) -> Result<(), TransportError> {
    let written = node.write(bytes).map_err(|error| TransportError::Write {
        message: error.to_string(),
    })?;
    if written != bytes.len() {
        return Err(TransportError::Write {
            message: format!("short write: {written} of {} bytes", bytes.len()),
        });
    }
    Ok(())
}

/// Writes one report to an open hidraw node, verbatim.
#[cfg(not(target_os = "linux"))]
pub fn write(_node: &mut File, _bytes: &[u8]) -> Result<(), TransportError> {
    Err(TransportError::Write {
        message: "the hidraw backend only works on Linux".to_string(),
    })
}

/// Reads everything the identity of one hidraw node needs.
#[cfg(target_os = "linux")]
fn describe(node: &Path) -> Option<DeviceInfo> {
    // `/sys/class/hidraw/hidrawN/device` points at the HID device itself.
    let device = std::fs::canonicalize(node.join("device")).ok()?;
    let uevent = std::fs::read_to_string(device.join("uevent")).ok()?;
    let (bus, vendor_id, product_id) = parse_hid_id(&uevent)?;
    let bus_type = BusType::from_bus(bus);
    if !matches!(
        bus_type,
        BusType::Usb | BusType::Bluetooth | BusType::I2C | BusType::SPI
    ) {
        // A bus this backend cannot drive is not a device we report.
        return None;
    }

    let usb_device = ancestor_with(&device, "idVendor");
    let usb_interface = ancestor_with(&device, "bInterfaceNumber");

    let (product_string, manufacturer_string) = match (bus_type, usb_device.as_deref()) {
        (BusType::Usb, Some(usb)) => (
            read_trimmed(&usb.join("product")),
            read_trimmed(&usb.join("manufacturer")),
        ),
        _ => (read_trimmed_value(&uevent, "HID_NAME"), None),
    };

    let serial_number = read_trimmed_value(&uevent, "HID_UNIQ").or_else(|| {
        usb_device
            .as_deref()
            .and_then(|usb| read_trimmed(&usb.join("serial")))
    });

    let usage = read_descriptor(&device).map_or(0, |bytes| usage_page(&bytes));
    let interface_number = usb_interface
        .as_deref()
        .and_then(|interface| read_trimmed(&interface.join("bInterfaceNumber")))
        .and_then(|text| parse_hex(&text))
        .unwrap_or(-1);

    Some(DeviceInfo {
        node: PathBuf::from(format!("/dev/{}", node.file_name()?.to_string_lossy())),
        vendor_id,
        product_id,
        bus_type,
        product_string,
        manufacturer_string,
        serial_number,
        usage_page: usage,
        interface_number,
    })
}

/// The nearest ancestor directory that holds `marker`.
#[cfg(target_os = "linux")]
fn ancestor_with(start: &Path, marker: &str) -> Option<PathBuf> {
    start
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.join(marker).is_file())
        .map(Path::to_path_buf)
}

/// Reads a sysfs attribute, trimmed.
#[cfg(target_os = "linux")]
fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Reads one `KEY=value` line out of a uevent file.
#[cfg(target_os = "linux")]
fn read_trimmed_value(uevent: &str, key: &str) -> Option<String> {
    uevent.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        if name != key {
            return None;
        }
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Reads the report descriptor of a HID device.
#[cfg(target_os = "linux")]
fn read_descriptor(device: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(device.join("report_descriptor")).ok()?;
    if bytes.is_empty() { None } else { Some(bytes) }
}

/// Parses the `HID_ID` line of a uevent file.
///
/// The line reads `HID_ID=0003:00003302:00008902`: the bus, the vendor id
/// and the product id, all in hex.
#[must_use]
pub fn parse_hid_id(uevent: &str) -> Option<(u32, u16, u16)> {
    let value = uevent
        .lines()
        .find_map(|line| line.strip_prefix("HID_ID="))?
        .trim();
    let mut parts = value.split(':');
    let bus = u32::from_str_radix(parts.next()?.trim(), 16).ok()?;
    let vendor_id = u16::from_str_radix(parts.next()?.trim(), 16).ok()?;
    let product_id = u16::from_str_radix(parts.next()?.trim(), 16).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((bus, vendor_id, product_id))
}

/// The usage page of the first report in a HID report descriptor.
///
/// The descriptor is a run of HID items; a *global* item with tag 0 is the
/// usage page, and its value is one, two or four bytes, little endian.
#[must_use]
pub fn usage_page(descriptor: &[u8]) -> u16 {
    let mut position = 0usize;
    while position < descriptor.len() {
        let prefix = descriptor[position];
        if prefix == 0xff {
            // The end of the descriptor.
            break;
        }
        if (prefix & 0xf0) == 0xf0 {
            // A long item: the prefix, a length byte, the data and a
            // terminator.
            let Some(&length) = descriptor.get(position + 1) else {
                break;
            };
            position = position
                .saturating_add(usize::from(length))
                .saturating_add(3);
            continue;
        }
        let data_len = match prefix & 0x03 {
            0 => 0usize,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        let data_start = position + 1;
        let data_end = data_start.saturating_add(data_len).min(descriptor.len());
        let item_type = (prefix >> 2) & 0x03;
        let tag = prefix >> 4;
        if item_type == 1 && tag == 0 {
            let mut value: u16 = 0;
            for (index, byte) in descriptor[data_start..data_end].iter().enumerate().take(2) {
                value |= u16::from(*byte) << (8 * index);
            }
            return value;
        }
        position = data_end.max(position + 1);
    }
    0
}

/// Parses a hexadecimal sysfs number.
#[must_use]
pub fn parse_hex(text: &str) -> Option<i32> {
    i32::from_str_radix(text.trim(), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AURI_UEVENT: &str = "DRIVER=hid-generic\nHID_ID=0003:00003302:00008902\nHID_NAME=Kuba Kuba Auri\nHID_PHYS=usb-0000:e5:00.3-1/input3\nHID_UNIQ=33028902260831\n";

    #[test]
    fn the_hid_id_line_carries_bus_vendor_and_product() {
        assert_eq!(parse_hid_id(AURI_UEVENT), Some((0x03, 0x3302, 0x8902)));
    }

    #[test]
    fn a_malformed_hid_id_is_refused() {
        assert_eq!(parse_hid_id("HID_ID=nonsense"), None);
        assert_eq!(parse_hid_id("HID_ID=0003:00003302"), None);
        assert_eq!(parse_hid_id("nothing here"), None);
    }

    #[test]
    fn the_bus_numbers_map_to_names() {
        assert_eq!(BusType::from_bus(0x03), BusType::Usb);
        assert_eq!(BusType::from_bus(0x05), BusType::Bluetooth);
        assert_eq!(BusType::from_bus(0x18), BusType::I2C);
        assert_eq!(BusType::from_bus(0x15), BusType::SPI);
        assert_eq!(BusType::from_bus(0x99), BusType::Unknown(0x99));
    }

    #[test]
    fn the_usage_page_is_the_first_global_usage_page() {
        // The Auri's descriptor: `05 0c` is a one byte global usage page.
        let descriptor = [0x05, 0x0c, 0x09, 0x01, 0xa1, 0x01, 0x85, 0x01];
        assert_eq!(usage_page(&descriptor), 0x000c);
    }

    #[test]
    fn a_two_byte_usage_page_is_read_little_endian() {
        // HID global items are little endian: `06 01 0d` is page 0x0d01.
        assert_eq!(usage_page(&[0x06, 0x01, 0x0d]), 0x0d01);
    }

    #[test]
    fn a_descriptor_without_a_usage_page_has_none() {
        assert_eq!(usage_page(&[0x09, 0x01, 0xa1, 0x01, 0xc0]), 0);
        assert_eq!(usage_page(&[]), 0);
    }

    #[test]
    fn the_interface_number_is_hexadecimal() {
        assert_eq!(parse_hex("03"), Some(3));
        assert_eq!(parse_hex(" 0a \n"), Some(10));
        assert_eq!(parse_hex("zz"), None);
    }

    #[test]
    fn the_label_is_the_one_the_journal_already_records() {
        let info = DeviceInfo {
            node: PathBuf::from("/dev/hidraw5"),
            vendor_id: 0x3302,
            product_id: 0x8902,
            bus_type: BusType::Usb,
            product_string: Some("Kuba Auri".to_string()),
            manufacturer_string: Some("Kuba".to_string()),
            serial_number: Some("33028902260831".to_string()),
            usage_page: 0x000c,
            interface_number: 3,
        };
        assert_eq!(
            info.label(),
            "Kuba Auri | serial 33028902260831 | Usb 3302:8902 | usage page 0x000c interface 3"
        );
    }

    #[test]
    fn a_device_without_names_still_gets_a_label() {
        let info = DeviceInfo {
            node: PathBuf::from("/dev/hidraw0"),
            vendor_id: 0x046d,
            product_id: 0xc534,
            bus_type: BusType::Usb,
            product_string: None,
            manufacturer_string: None,
            serial_number: None,
            usage_page: 0,
            interface_number: -1,
        };
        assert_eq!(
            info.label(),
            "unknown | serial - | Usb 046d:c534 | usage page 0x0000 interface -1"
        );
    }
}

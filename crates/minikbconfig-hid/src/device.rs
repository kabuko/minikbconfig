//! Locating the right HID interface.
//!
//! The pad exposes three interfaces; only one is the vendor configuration
//! interface, identified by usage page `0xFF00` usage `0x02` in its report
//! descriptor. hidapi reports that usage page on every platform, so the fast
//! path is a plain filter over the device list; the descriptor is consulted as a
//! fallback for backends that do not populate it.

use anyhow::Context;

use minikbconfig_core::{DeviceId, SUPPORTED, VENDOR_USAGE_PAGE};

/// How large a report descriptor we are willing to read.
///
/// The pad's largest descriptor is 205 bytes; this is generous headroom.
const DESCRIPTOR_BUF_LEN: usize = 4096;

/// Which of the pad's input interfaces a handle refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// The boot-keyboard interface: 8-byte reports with no report ID.
    Boot,
    /// The report-ID interface: 32-byte reports whose first byte is a report ID.
    Numbered,
}

/// An opened input interface, tagged with the layout its reports use.
pub struct InputInterface {
    pub handle: hidapi::HidDevice,
    pub kind: InputKind,
}

/// Open one interface for reading or writing.
/// The model an interface belongs to, if it is one we support.
pub fn model_of(info: &hidapi::DeviceInfo) -> Option<DeviceId> {
    SUPPORTED
        .iter()
        .copied()
        .find(|d| d.vendor_id == info.vendor_id() && d.product_id == info.product_id())
}

pub fn open(info: &hidapi::DeviceInfo) -> anyhow::Result<hidapi::HidDevice> {
    let api = hidapi::HidApi::new().context("enumerate HID devices")?;
    api.open_path(info.path())
        .with_context(|| format!("open HID interface {}", info.path().to_string_lossy()))
}

/// Every vendor configuration interface currently attached, one entry per pad.
///
/// Entries are deduplicated by device path, because hidapi emits one
/// [`hidapi::DeviceInfo`] per top-level usage in a descriptor and a single pad
/// exposes several interfaces — matching on vendor/product alone would count
/// one pad as three.
pub fn find_config_interfaces() -> anyhow::Result<Vec<hidapi::DeviceInfo>> {
    let api = hidapi::HidApi::new().context("enumerate HID devices")?;
    let mut found: Vec<hidapi::DeviceInfo> = Vec::new();

    for info in api.device_list().filter(|i| is_supported(i)) {
        if info.usage_page() == VENDOR_USAGE_PAGE {
            push_unique(&mut found, info.clone());
        }
    }
    if !found.is_empty() {
        return Ok(found);
    }

    // Fall back to reading the descriptor directly.
    for info in api.device_list().filter(|i| is_supported(i)) {
        let Ok(handle) = api.open_path(info.path()) else {
            continue;
        };
        if is_config_descriptor(&handle) {
            push_unique(&mut found, info.clone());
        }
    }

    if found.is_empty() {
        anyhow::bail!(
            "no vendor configuration interface (usage page 0xFF00) found for the pad; \
             it may need replugging after the udev rule was installed"
        );
    }
    Ok(found)
}

/// Open every input interface of every attached pad, tagged with its layout.
///
/// The configuration interface is skipped: it carries replies, not keystrokes.
pub fn open_input_interfaces() -> anyhow::Result<Vec<InputInterface>> {
    let api = hidapi::HidApi::new().context("enumerate HID devices")?;
    let mut open = Vec::new();
    for info in api.device_list().filter(|i| is_supported(i)) {
        let Ok(handle) = api.open_path(info.path()) else {
            continue;
        };
        if is_config_descriptor(&handle) {
            continue;
        }
        open.push(InputInterface {
            kind: interface_kind(info, &handle),
            handle,
        });
    }
    Ok(open)
}

fn is_supported(info: &hidapi::DeviceInfo) -> bool {
    model_of(info).is_some()
}

/// Record an interface unless that device path is already present.
fn push_unique(found: &mut Vec<hidapi::DeviceInfo>, info: hidapi::DeviceInfo) {
    if !found.iter().any(|f| f.path() == info.path()) {
        found.push(info);
    }
}

/// Read a device's full report descriptor.
fn read_descriptor(handle: &hidapi::HidDevice) -> Vec<u8> {
    let mut buf = vec![0u8; DESCRIPTOR_BUF_LEN];
    match handle.get_report_descriptor(&mut buf) {
        Ok(len) => {
            buf.truncate(len);
            buf
        }
        Err(_) => Vec::new(),
    }
}

fn is_config_descriptor(handle: &hidapi::HidDevice) -> bool {
    is_vendor_config_descriptor(&read_descriptor(handle))
}

/// Which report layout an input interface uses.
///
/// The pad's interface 1 is the boot keyboard (8-byte reports, no report ID);
/// every other input interface is the report-ID one (DESIGN.md §1). Deciding
/// this per interface rather than by sniffing report bytes matters: a boot
/// report beginning with a modifier byte looks exactly like a report ID, so
/// `ctrl+f18` would otherwise be misread as report 1 and silently dropped.
fn interface_kind(info: &hidapi::DeviceInfo, handle: &hidapi::HidDevice) -> InputKind {
    match info.interface_number() {
        1 => InputKind::Boot,
        n if n >= 0 => InputKind::Numbered,
        // No interface number available: fall back to the declared report
        // length, which is 8 bytes for boot keyboard and 32 for report IDs.
        _ => {
            let len = report_len(handle);
            if len == Some(8) {
                InputKind::Boot
            } else {
                InputKind::Numbered
            }
        }
    }
}

/// The report length an interface declares, from its descriptor's input item.
fn report_len(handle: &hidapi::HidDevice) -> Option<usize> {
    let descriptor = read_descriptor(handle);
    let mut report_size = 0usize;
    let mut report_count = 0usize;
    for item in descriptor.windows(2) {
        match item {
            // Report Size (0x75), Report Count (0x95)
            [0x75, size] => report_size = *size as usize,
            [0x95, count] => report_count = *count as usize,
            _ => {}
        }
    }
    (report_size > 0 && report_count > 0).then(|| report_size * report_count)
}

/// True if a HID report descriptor declares the vendor configuration collection.
///
/// The collection starts `06 00 ff 09 02 a1 01`: usage page `0xFF00`, usage
/// `0x02`, application collection.
pub fn is_vendor_config_descriptor(descriptor: &[u8]) -> bool {
    const PREFIX: [u8; 6] = [0x06, 0x00, 0xFF, 0x09, 0x02, 0xA1];
    descriptor.len() >= PREFIX.len() && descriptor[..PREFIX.len()] == PREFIX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_real_config_descriptor() {
        // Captured from the pad: usage page 0xFF00, usage 0x02, 64-byte reports
        let descriptor = [
            0x06, 0x00, 0xFF, 0x09, 0x02, 0xA1, 0x01, 0x19, 0x00, 0x2A, 0xFF, 0x00, 0x15, 0x00,
            0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x40, 0x81, 0x00, 0x19, 0x00, 0x2A, 0xFF, 0x00,
            0x91, 0x00, 0xC0,
        ];
        assert!(is_vendor_config_descriptor(&descriptor));
    }

    #[test]
    fn rejects_the_boot_keyboard_descriptor() {
        let boot: Vec<u8> = vec![
            0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00,
            0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
        ];
        assert!(!is_vendor_config_descriptor(&boot));
    }

    #[test]
    fn rejects_an_empty_or_truncated_descriptor() {
        assert!(!is_vendor_config_descriptor(&[]));
        assert!(!is_vendor_config_descriptor(&[0x06, 0x00, 0xFF]));
    }
}

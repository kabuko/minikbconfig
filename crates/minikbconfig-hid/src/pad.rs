//! Talking to the pad: device discovery and the request/reply exchange.
//!
//! The configuration interface has both an interrupt IN and an OUT endpoint, so
//! hidraw/hidapi is sufficient on every platform. No libusb, no driver install.

use std::time::{Duration, Instant};

use minikbconfig_core::{
    Emitted, PRODUCT_ID, VENDOR_ID,
    keymap::{self, CHUNK_KEYS, Command, KEY_SLOTS, KeyTable},
    light::{self, LightConfig},
    protocol::{REPORT_LEN, RESPONSE_PAYLOAD_OFFSET, Request, is_reply_to},
    response::{DeviceInfo, Response},
};

use crate::device::{self, InputKind};

/// How long to wait for the device to answer a request.
///
/// The device replies immediately in practice; this is a generous ceiling for
/// when it is busy or a stale report is still queued.
const REPLY_TIMEOUT: Duration = Duration::from_millis(1500);

/// How long one interface is polled before the watcher rotates to the next.
const POLL_SLICE: Duration = Duration::from_millis(10);

/// Convert a remaining duration into hidapi's millisecond timeout, never 0.
fn timeout_ms(remaining: Duration) -> i32 {
    i32::try_from(remaining.as_millis())
        .unwrap_or(i32::MAX)
        .max(1)
}

/// A pad whose configuration interface is open.
pub struct Pad {
    handle: hidapi::HidDevice,
    vendor_id: u16,
    product_id: u16,
    product_name: String,
}

impl Pad {
    /// Find the single supported pad and open its configuration interface.
    pub fn open() -> anyhow::Result<Pad> {
        let interfaces = device::find_config_interfaces()?;
        match interfaces.len() {
            0 => anyhow::bail!(
                "no supported pad found (expected USB {:04x}:{:04x}); \
                 is it plugged in, and is the udev rule installed?",
                VENDOR_ID,
                PRODUCT_ID
            ),
            1 => Pad::open_interface(&interfaces[0]),
            n => anyhow::bail!("{n} pads found; expected 1. Disconnect the others and retry."),
        }
    }

    fn open_interface(info: &hidapi::DeviceInfo) -> anyhow::Result<Pad> {
        let handle = device::open(info)?;
        Ok(Pad {
            handle,
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            product_name: info.product_string().unwrap_or_default().to_string(),
        })
    }

    pub fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    pub fn product_id(&self) -> u16 {
        self.product_id
    }

    pub fn product_name(&self) -> &str {
        &self.product_name
    }

    /// Send a request and wait for the matching reply, returning the payload.
    pub fn transact(&self, request: &Request) -> anyhow::Result<Vec<u8>> {
        let report = self.exchange(request)?;
        Ok(report[RESPONSE_PAYLOAD_OFFSET..].to_vec())
    }

    /// Send a request and wait for the matching reply, returning the raw report.
    ///
    /// Stale input is drained first, device-initiated `AA FA` notifications are
    /// skipped, and a reply carrying a different offset is treated as a leftover
    /// from an earlier request. Replies are paired by their echoed offset rather
    /// than their command byte, which the device does not echo faithfully.
    pub fn exchange(&self, request: &Request) -> anyhow::Result<Vec<u8>> {
        let report = request.to_report()?;
        self.drain_stale_input();
        self.handle.write(&report)?;

        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!(
                    "no reply to command 0x{:02x} within {:?}",
                    request.command,
                    REPLY_TIMEOUT
                );
            }
            let Some(bytes) = self.read_report(timeout_ms(remaining))? else {
                continue;
            };
            if Response::parse(&bytes)?.is_none() {
                continue; // unsolicited notification; keep waiting
            }
            if !is_reply_to(request, &bytes) {
                continue; // leftover reply to an earlier request
            }
            return Ok(bytes);
        }
    }

    /// Read the pad's device info.
    pub fn device_info(&self) -> anyhow::Result<DeviceInfo> {
        let report = self.exchange(&Request::new(Command::GET_CONFIG))?;
        Ok(DeviceInfo::parse(&report)?)
    }

    /// Read the whole key table for a layer.
    pub fn read_keys(&self, layer: u8) -> anyhow::Result<KeyTable> {
        let mut table = KeyTable::new();
        for chunk in 0..(KEY_SLOTS as usize).div_ceil(CHUNK_KEYS as usize) {
            let first_index = (chunk * CHUNK_KEYS as usize) as u8;
            let offset = (chunk * keymap::CHUNK_PAYLOAD_LEN) as u16;
            let body = self.transact(&keymap::read_key_chunk(offset, layer)?)?;
            let chunk_table = KeyTable::from_chunk(&body, first_index)?;
            for index in first_index..KEY_SLOTS {
                if let Some(action) = chunk_table.get(index) {
                    table.set(index, action);
                }
            }
            table.absorb_unrecognised(&chunk_table);
        }
        Ok(table)
    }

    /// Program one key slot on a layer.
    pub fn write_key(&self, index: u8, action: keymap::KeyType, layer: u8) -> anyhow::Result<()> {
        let request = keymap::write_key(index, action)?.with_layer(layer);
        self.transact(&request)?;
        Ok(())
    }

    /// Read one key slot.
    ///
    /// Reads the 56-byte chunk that contains the index rather than starting the
    /// read at the index's own byte offset. Both address the same record if the
    /// device honours the offset exactly, but only the aligned read stays
    /// correct if it ever returns a boundary-aligned chunk instead.
    pub fn read_key(&self, index: u8, layer: u8) -> anyhow::Result<keymap::KeyType> {
        let chunk = index as usize / CHUNK_KEYS as usize;
        let first_index = (chunk * CHUNK_KEYS as usize) as u8;
        let offset = (chunk * keymap::CHUNK_PAYLOAD_LEN) as u16;
        let body = self.transact(&keymap::read_key_chunk(offset, layer)?)?;
        let table = KeyTable::from_chunk(&body, first_index)?;
        table
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("device did not return a record for key {index}"))
    }

    /// Read the pad's current lighting state.
    pub fn light_config(&self) -> anyhow::Result<LightConfig> {
        let report = light::read_light_config();
        let reply = self.exchange_bytes(&report, 0x0A)?;
        Ok(LightConfig::parse(&reply)?)
    }

    /// Write the pad's lighting state and read it back to confirm.
    pub fn set_light_config(&self, config: LightConfig) -> anyhow::Result<LightConfig> {
        let report = light::write_light_config(config)?;
        self.exchange_bytes(&report, 0x0B)?;
        let stored = self.light_config()?;
        if !config.applied(stored) {
            let differs = config.differences(stored).join(", ");
            anyhow::bail!(
                "lighting did not take effect; the device differs on: {differs}\n  \
                 wanted  {config:?}\n  \
                 device  {stored:?}"
            );
        }
        Ok(stored)
    }

    /// Select the current profile (command `0xFB`).
    ///
    /// Profiles hold independent key tables, so this changes which table
    /// `read_keys` and `write_key` act on.
    pub fn set_profile(&self, index: u8) -> anyhow::Result<()> {
        let report = minikbconfig_core::protocol::set_profile(index);
        self.exchange_bytes(&report, 0xFB)?;
        Ok(())
    }

    /// Set one key's colour.
    pub fn set_key_color(&self, index: u8, rgb: [u8; 3]) -> anyhow::Result<()> {
        let report = light::set_key_color(index, rgb)?;
        self.exchange_bytes(&report, 0x14)?;
        Ok(())
    }

    /// Send a pre-built report and return the reply.
    ///
    /// The light commands do not fit the generic [`Request`] builder: their
    /// records start at offset 5 rather than 8.
    fn exchange_bytes(&self, report: &[u8], command: u8) -> anyhow::Result<Vec<u8>> {
        self.drain_stale_input();
        self.handle.write(report)?;

        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("no reply to command 0x{command:02x} within {REPLY_TIMEOUT:?}");
            }
            let Some(bytes) = self.read_report(timeout_ms(remaining))? else {
                continue;
            };
            if Response::parse(&bytes)?.is_none() {
                continue; // unsolicited notification
            }
            // Offset is always 0 for these commands, so the echoed offset is
            // the same in every reply; the command byte is the only better
            // discriminator here, and these commands *are* echoed correctly.
            let echoed = bytes.get(1).copied().unwrap_or(0);
            if echoed != command {
                continue;
            }
            return Ok(bytes);
        }
    }

    /// Read one input report, or `None` if the timeout expired.
    fn read_report(&self, timeout: i32) -> anyhow::Result<Option<Vec<u8>>> {
        let mut buf = [0u8; REPORT_LEN];
        match self.handle.read_timeout(&mut buf, timeout)? {
            0 => Ok(None),
            len => Ok(Some(buf[..len].to_vec())),
        }
    }

    /// Discard input reports that are already queued.
    fn drain_stale_input(&self) {
        let mut buf = [0u8; REPORT_LEN];
        while matches!(self.handle.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
    }
}

/// A one-line description of the connected pad.
pub fn describe(pad: &Pad) -> String {
    let name = if pad.product_name().is_empty() {
        "SDCX SIDE-KEYBOARD"
    } else {
        pad.product_name()
    };
    format!(
        "{name} (USB {:04x}:{:04x})",
        pad.vendor_id(),
        pad.product_id()
    )
}

/// Stream emitted keystrokes from the pad's input interfaces.
///
/// Interface 0 is matched by its numbered report descriptors, interface 1 by the
/// boot-keyboard descriptor. Both are needed: keyboard actions come out of the
/// boot interface on this device and consumer actions out of interface 0.
pub struct Watcher {
    interfaces: Vec<device::InputInterface>,
}

impl Watcher {
    /// Open every input interface belonging to the pad.
    pub fn open() -> anyhow::Result<Watcher> {
        let interfaces = device::open_input_interfaces()?;
        if interfaces.is_empty() {
            anyhow::bail!(
                "found the pad but none of its input interfaces could be opened; \
                 on Linux check the udev rule and that you are in the seat group"
            );
        }
        Ok(Watcher { interfaces })
    }

    /// Block until the pad emits something other than a key release.
    ///
    /// hidapi offers no way to wait on several interfaces at once, so this
    /// round-robins with a short slice per interface. Giving any single
    /// interface the whole remaining budget would starve the others: a blocking
    /// read returns only on its own device, so the first interface would consume
    /// the whole window and the boot interface would only ever see the final
    /// millisecond. Keyboard events come from the boot interface and consumer
    /// events from the numbered one, so starving either loses half the pad.
    pub fn next_press(&self, timeout: Duration) -> anyhow::Result<Option<Emitted>> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; REPORT_LEN];
        loop {
            for interface in &self.interfaces {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                let slice = remaining.min(POLL_SLICE);
                let Ok(len) = interface.handle.read_timeout(&mut buf, timeout_ms(slice)) else {
                    continue;
                };
                if len == 0 {
                    continue;
                }
                let event = match interface.kind {
                    InputKind::Boot => Emitted::from_boot(&buf[..len]),
                    InputKind::Numbered => Emitted::from_numbered(&buf[..len]),
                };
                if !event.is_release() {
                    return Ok(Some(event));
                }
            }
        }
    }
}

const _: () = assert!(minikbconfig_core::VENDOR_USAGE == 0x0002);

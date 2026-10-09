//! Parsing of the device's replies.
//!
//! See `PROTOCOL.md` §2 and §3.

use crate::protocol::{
    ProtocolError, CONFIG_PAYLOAD_OFFSET, REPORT_LEN, RESPONSE_MARKER, RESPONSE_PAYLOAD_OFFSET,
};

/// A decoded reply from the configuration interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub command: u8,
    pub status: u8,
    pub body: Vec<u8>,
}

impl Response {
    /// Parse a 64-byte input report from the configuration interface.
    ///
    /// Returns `Ok(None)` for device-initiated notifications (those beginning
    /// `AA FA`), which are not replies to any request.
    pub fn parse(report: &[u8]) -> Result<Option<Response>, ProtocolError> {
        if report.len() < REPORT_LEN {
            return Err(ProtocolError::ResponseTooShort { got: report.len() });
        }
        if report[0] == crate::protocol::ASYNC_MARKER.0 && report[1] == crate::protocol::ASYNC_MARKER.1 {
            return Ok(None);
        }
        if report[0] != RESPONSE_MARKER {
            return Err(ProtocolError::BadMarker { marker: report[0] });
        }
        Ok(Some(Response {
            command: report[1],
            status: report[2],
            body: report[RESPONSE_PAYLOAD_OFFSET..].to_vec(),
        }))
    }

    /// The payload of a `get_key_layout` reply: 56 bytes of key records.
    pub fn key_chunk(&self) -> &[u8] {
        &self.body[..self.body.len().min(crate::keymap::CHUNK_PAYLOAD_LEN)]
    }
}

/// Device identity and settings, from the `get_key_layout` (`05`) reply.
///
/// Note this command answers at offset 5, not 8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub protocol_version: u16,
    pub product_id: u16,
    pub firmware_version: u16,
    pub work_mode: u8,
    pub link_status: u8,
    pub battery_percent: u8,
    pub charge_state: u8,
    pub profile_count: u8,
    pub current_profile: u8,
    pub layer_count: u8,
    /// Reported by the device in a **1-based** field. Writes use 0-based layers.
    pub current_layer: u8,
}

impl DeviceInfo {
    /// Parse the reply to `get_config` (`05`).
    pub fn parse(report: &[u8]) -> Result<DeviceInfo, ProtocolError> {
        if report.len() < CONFIG_PAYLOAD_OFFSET + 14 {
            return Err(ProtocolError::ResponseTooShort { got: report.len() });
        }
        if report[0] != RESPONSE_MARKER {
            return Err(ProtocolError::BadMarker { marker: report[0] });
        }
        let f = &report[CONFIG_PAYLOAD_OFFSET..];
        Ok(DeviceInfo {
            protocol_version: u16::from_le_bytes([f[0], f[1]]),
            product_id: u16::from_le_bytes([f[2], f[3]]),
            firmware_version: u16::from_le_bytes([f[4], f[5]]),
            work_mode: f[6],
            link_status: f[7],
            battery_percent: f[8],
            charge_state: f[9],
            profile_count: f[10],
            current_profile: f[11],
            layer_count: f[12],
            current_layer: f[13],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real response captured from a `6d7b:dcfa` device.
    const REAL_CONFIG_REPLY: [u8; REPORT_LEN] = {
        // aa 05 0e 00 00 | 01 00 fa dc 68 00 00 01 64 00 06 00 00 01 ...
        let mut r = [0u8; REPORT_LEN];
        r[0] = 0xAA;
        r[1] = 0x05;
        r[2] = 0x0E;
        r[5] = 0x01; // protocol version lo
        r[6] = 0x00; // protocol version hi
        r[7] = 0xFA; // product id lo
        r[8] = 0xDC; // product id hi
        r[9] = 0x68; // firmware lo
        r[10] = 0x00; // firmware hi
        r[11] = 0x00; // work mode
        r[12] = 0x01; // link status
        r[13] = 0x64; // battery = 100
        r[14] = 0x00; // charge state
        r[15] = 0x06; // profile count
        r[16] = 0x00; // current profile
        r[17] = 0x00; // layer count
        r[18] = 0x01; // current layer (1-based, as the device reports it)
        r
    };

    #[test]
    fn config_reply_yields_the_expected_device_info() {
        let info = DeviceInfo::parse(&REAL_CONFIG_REPLY).unwrap();
        assert_eq!(info.product_id, 0xDCFA, "product ID should read back as 0xDCFA");
        assert_eq!(info.firmware_version, 0x0068);
        assert_eq!(info.battery_percent, 100);
        assert_eq!(info.profile_count, 6);
        // The device reports layer_count 0 while current_layer is 1. Both are
        // reproduced verbatim: we have not established what layer_count means,
        // and normalising it here would hide the real device behaviour.
        assert_eq!(info.layer_count, 0);
        assert_eq!(info.current_layer, 1);
    }

    #[test]
    fn short_response_is_an_error() {
        assert_eq!(
            Response::parse(&[0xAA, 0x05]),
            Err(ProtocolError::ResponseTooShort { got: 2 })
        );
    }

    #[test]
    fn a_non_aa_first_byte_is_rejected() {
        let mut bad = [0u8; REPORT_LEN];
        bad[0] = 0x12;
        assert_eq!(
            Response::parse(&bad),
            Err(ProtocolError::BadMarker { marker: 0x12 })
        );
    }

    #[test]
    fn async_notifications_are_not_mistaken_for_replies() {
        let mut async_report = [0u8; REPORT_LEN];
        async_report[0] = 0xAA;
        async_report[1] = 0xFA;
        assert_eq!(Response::parse(&async_report).unwrap(), None);
    }

    #[test]
    fn key_chunk_reply_is_recognised_and_trimmed() {
        let mut reply = [0u8; REPORT_LEN];
        reply[0] = 0xAA;
        reply[1] = 0x07;
        reply[2] = 0x3A;
        for i in 0..crate::keymap::CHUNK_PAYLOAD_LEN {
            reply[8 + i] = 0x13;
        }
        let response = Response::parse(&reply).unwrap().unwrap();
        assert_eq!(response.command, 0x07);
        assert_eq!(response.key_chunk().len(), crate::keymap::CHUNK_PAYLOAD_LEN);
        assert!(response.key_chunk().iter().all(|b| *b == 0x13));
    }
}
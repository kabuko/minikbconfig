//! Report framing for the SDCX vendor configuration interface.
//!
//! Every request is a fixed 64-byte report. See `PROTOCOL.md` §2.

/// Size of a configuration report in both directions.
pub const REPORT_LEN: usize = 64;

/// Device prefix: the first byte of every request.
pub const PREFIX: u8 = 0x06;

/// Marker the device puts in byte 0 of a command response.
pub const RESPONSE_MARKER: u8 = 0xAA;

/// First byte of a device-initiated (unsolicited) notification, e.g. LED events.
pub const ASYNC_MARKER: (u8, u8) = (0xAA, 0xFA);

/// Byte offset at which a response payload begins.
pub const RESPONSE_PAYLOAD_OFFSET: usize = 8;

/// Byte offset at which a device-config (`05`) payload begins.
///
/// This command answers at offset 5 rather than 8 — the one exception.
pub const CONFIG_PAYLOAD_OFFSET: usize = 5;

/// Maximum payload bytes that fit in one report.
pub const MAX_PAYLOAD: usize = REPORT_LEN - 8;

/// A configuration request, before it is serialised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub command: u8,
    pub offset: u16,
    pub layer: u8,
    pub payload: Vec<u8>,
    /// Set for the few commands whose length byte is not `3 + payload`.
    pub fixed_length: Option<u8>,
}

impl Request {
    /// A request with no payload, e.g. `get_keyboard_config` (`05`).
    pub fn new(command: u8) -> Self {
        Self { command, offset: 0, layer: 0, payload: Vec::new(), fixed_length: None }
    }

    /// A request addressing a block offset or key index.
    pub fn at(command: u8, offset: u16) -> Self {
        Self { command, offset, layer: 0, payload: Vec::new(), fixed_length: None }
    }

    pub fn with_layer(mut self, layer: u8) -> Self {
        self.layer = layer;
        self
    }

    pub fn with_payload(mut self, payload: impl Into<Vec<u8>>) -> Self {
        self.payload = payload.into();
        self
    }

    /// Override the length byte, for the few commands that do not follow the
    /// `3 + payload` rule.
    ///
    /// Key-table reads always announce 58 (`0x3A`) regardless of how much data
    /// the device actually returns.
    pub fn with_fixed_length(mut self, length: u8) -> Self {
        self.fixed_length = Some(length);
        self
    }

    /// The length byte: `3 + payload length`, unless overridden by
    /// [`Self::with_fixed_length`].
    pub fn length_byte(&self) -> Result<u8, ProtocolError> {
        if let Some(fixed) = self.fixed_length {
            return Ok(fixed);
        }
        let len = 3 + self.payload.len();
        u8::try_from(len).map_err(|_| ProtocolError::PayloadTooLong { len })
    }

    /// Serialise to the 64-byte wire report.
    pub fn to_report(&self) -> Result<[u8; REPORT_LEN], ProtocolError> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(ProtocolError::PayloadTooLong { len: self.payload.len() });
        }
        let mut buf = [0u8; REPORT_LEN];
        buf[0] = PREFIX;
        buf[1] = self.command;
        buf[2] = self.length_byte()?;
        buf[3] = self.offset as u8;
        buf[4] = (self.offset >> 8) as u8;
        buf[6] = self.layer;
        buf[8..8 + self.payload.len()].copy_from_slice(&self.payload);
        Ok(buf)
    }
}

/// True if a reply belongs to a given request.
///
/// The command byte cannot be used to pair them: the device does **not**
/// faithfully echo it. A key-layout read sent as `08 3A <off> <layer>` comes
/// back as `AA 07 3A <off> ...`, with `07` in the command slot, so requiring a
/// match would discard every valid reply and time out instead.
///
/// The offset field is echoed faithfully — bytes 3-4 of the reply hold exactly
/// the offset that was sent — so that is what identifies the reply. See
/// `PROTOCOL.md`.
pub fn is_reply_to(request: &Request, reply: &[u8]) -> bool {
    reply.len() >= RESPONSE_PAYLOAD_OFFSET
        && reply[3] == request.offset as u8
        && reply[4] == (request.offset >> 8) as u8
}

/// Errors produced while building or parsing reports.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("payload of {len} bytes does not fit in a report (max {max})", max = MAX_PAYLOAD)]
    PayloadTooLong { len: usize },

    #[error("response was {got} bytes, too short to contain a header")]
    ResponseTooShort { got: usize },

    #[error("expected a response marker of 0xAA, got 0x{marker:02x}")]
    BadMarker { marker: u8 },

    #[error("key type 0x{ty:02x} is not known to this build")]
    UnknownKeyType { ty: u8 },

    #[error("key index {index} is out of range (max {max})")]
    KeyIndexOutOfRange { index: u8, max: u8 },

    #[error("'{0}' is not a #RRGGBB colour")]
    BadColour(String),

    #[error(
        "light mode {mode} is not one the firmware accepts ({min}-{max}); \
         the device silently discards the whole record"
    )]
    InvalidLightMode { mode: u8, min: u8, max: u8 },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte vectors from `PROTOCOL.md`, cross-checked against the Python
    /// reference implementation in `tools/probe_sdcx.py`.
    fn hex(report: &[u8; REPORT_LEN]) -> String {
        report
            .iter()
            .take(12)
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn device_info_request_matches_documented_bytes() {
        let r = Request::new(Command::GET_CONFIG).to_report().unwrap();
        assert_eq!(&r[..8], &[0x06, 0x05, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn read_key_layout_chunk_request_matches_documented_bytes() {
        // Reads announce a fixed length of 0x3A, so they use read_key_chunk.
        let r = crate::keymap::read_key_chunk(0, 0).unwrap().to_report().unwrap();
        assert_eq!(&r[..8], &[0x06, 0x08, 0x3A, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn read_key_layout_second_chunk_uses_little_endian_offset() {
        let r = crate::keymap::read_key_chunk(56, 0).unwrap().to_report().unwrap();
        assert_eq!(&r[..8], &[0x06, 0x08, 0x3A, 0x38, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn single_key_write_matches_documented_bytes() {
        // key index 0 -> 'a': type 32 (Standard), no modifiers, keycode 0x04
        let r = write_key(0, KeyAction::key(KeyCode::A))
            .unwrap()
            .to_report()
            .unwrap();
        assert_eq!(hex(&r), "06 10 07 00 00 00 00 00 20 00 04 00");
    }

    #[test]
    fn key_index_is_multiplied_by_four_for_the_offset_field() {
        // index 16 (knob 1 press) -> offset 0x40
        let r = write_key(16, KeyAction::media(0xE2))
            .unwrap()
            .to_report()
            .unwrap();
        assert_eq!(hex(&r), "06 10 07 40 00 00 00 00 30 e2 00 00");
    }

    #[test]
    fn layer_is_written_at_byte_six() {
        let r = write_key(0, KeyAction::key(KeyCode::A))
            .unwrap()
            .with_layer(1)
            .to_report()
            .unwrap();
        assert_eq!(r[6], 1);
    }

    #[test]
    fn length_byte_counts_the_three_byte_header() {
        let r = Request::new(Command::GET_CONFIG).to_report().unwrap();
        assert_eq!(r[2], 3);
        let r = write_key(0, KeyAction::key(KeyCode::A)).unwrap().to_report().unwrap();
        assert_eq!(r[2], 7); // 3 + 4 byte key record
    }

    #[test]
    fn oversized_payload_is_rejected_rather_than_silently_truncated() {
        let payload = vec![0u8; MAX_PAYLOAD + 1];
        let err = Request::new(Command::WRITE_KEY_LAYOUT_BULK).with_payload(payload).to_report();
        assert_eq!(
            err,
            Err(ProtocolError::PayloadTooLong { len: MAX_PAYLOAD + 1 })
        );
    }

    #[test]
    fn exactly_max_payload_still_fits() {
        let payload = vec![0u8; MAX_PAYLOAD];
        assert!(Request::new(Command::WRITE_KEY_LAYOUT_BULK)
            .with_payload(payload)
            .to_report()
            .is_ok());
    }

    // imported late so the tests above read as protocol examples
    use crate::keymap::{write_key, Command, KeyAction, KeyCode};

    // Real reply captured from the pad, for a key-layout read sent as
    // `06 08 3a 00 00 00 00 00`. Note byte 1 is 0x07, not the 0x08 that was
    // sent, while the offset in bytes 3-4 comes back unchanged.
    const REAL_KEY_LAYOUT_REPLY: [u8; REPORT_LEN] = {
        let mut r = [0u8; REPORT_LEN];
        r[0] = 0xAA;
        r[1] = 0x07; // echoed as 0x07 even though the request used 0x08
        r[2] = 0x3A;
        r[3] = 0x00; // offset low, echoed
        r[4] = 0x00; // offset high, echoed
        r[8] = 0x20; // key 0: SPACE
        r[9] = 0x00;
        r[10] = 0x2C;
        r[11] = 0x00;
        r
    };

    #[test]
    fn a_reply_is_matched_on_the_echoed_offset_not_the_command_byte() {
        let request = crate::keymap::read_key_chunk(0, 0).unwrap();
        assert_eq!(request.command, 0x08, "the request uses command 0x08");
        assert_eq!(
            REAL_KEY_LAYOUT_REPLY[1],
            0x07,
            "but the device answers with 0x07 in that slot"
        );
        assert!(is_reply_to(&request, &REAL_KEY_LAYOUT_REPLY));
    }

    #[test]
    fn a_reply_for_a_different_offset_is_not_matched() {
        let request = crate::keymap::read_key_chunk(56, 0).unwrap();
        assert!(!is_reply_to(&request, &REAL_KEY_LAYOUT_REPLY));
    }

    #[test]
    fn a_reply_too_short_to_hold_an_offset_is_not_matched() {
        let request = crate::keymap::read_key_chunk(0, 0).unwrap();
        // Bytes 3-4 carry the offset, so anything shorter cannot be matched.
        assert!(!is_reply_to(&request, &REAL_KEY_LAYOUT_REPLY[..4]));
    }

    #[test]
    fn the_second_chunk_reply_matches_its_own_offset() {
        let mut reply = REAL_KEY_LAYOUT_REPLY;
        reply[3] = 0x38; // 56
        assert!(is_reply_to(&crate::keymap::read_key_chunk(56, 0).unwrap(), &reply));
    }
}
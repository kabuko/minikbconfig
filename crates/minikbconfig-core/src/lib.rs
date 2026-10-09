//! Wire protocol and key model for SDCX SIDE-KEYBOARD macro pads (`6d7b:dcfa`).
//!
//! Pure logic with no I/O: report framing, the command set, the 4-byte-per-key
//! encoding, and decoding of both replies and emitted keystrokes.
//!
//! The protocol was reverse-engineered from the vendor's shipped JavaScript and
//! verified against hardware; see `PROTOCOL.md`. There is no official
//! specification, so treat it as observed behaviour rather than documented fact.
//!
//! ```
//! use minikbconfig_core::{keymap, protocol::Request};
//!
//! // Program key slot 0 to emit 'a'.
//! let request = keymap::write_key(0, keymap::KeyType::key(keymap::KeyCode::A)).unwrap();
//! let report = request.to_report().unwrap();
//! assert_eq!(&report[..12], &[0x06, 0x10, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x04, 0x00]);
//!
//! // Read device info.
//! let request = Request::new(keymap::Command::GET_CONFIG);
//! assert_eq!(&request.to_report().unwrap()[..3], &[0x06, 0x05, 0x03]);
//! ```

pub mod emitted;
pub mod keymap;
pub mod light;
pub mod protocol;
pub mod response;

pub use emitted::Emitted;
pub use keymap::{KeyCode, KeyTable, KeyType, Modifiers};
pub use light::LightConfig;
pub use protocol::{ProtocolError, Request};
pub use response::{DeviceInfo, Response};

/// USB vendor and product IDs for a supported pad.
///
/// Matched as a pair: the family ships under more than one vendor ID, and
/// `0816` has no relationship to `6D7B`, so a prefix or range test would be
/// guesswork. Both entries present the same three interfaces in the same order
/// — numbered reports, boot keyboard, and the vendor config interface at usage
/// page `0xFF00` usage `0x02` — so they speak the same protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceId {
    pub vendor_id: u16,
    pub product_id: u16,
    /// How many physical keys the model has, which occupy slots 0..keys.
    ///
    /// The device does not report this, so it cannot be discovered: it has to
    /// be known per model. Slots at or above this are not keys on that board.
    pub keys: u8,
}

/// Every pad this build knows how to drive.
pub const SUPPORTED: &[DeviceId] = &[
    // 3 keys + 1 knob. Verified end to end on 2026-10-09.
    DeviceId {
        vendor_id: 0x6D7B,
        product_id: 0xDCFA,
        keys: 3,
    },
    // 6 keys + rotary. Same interface layout, protocol and knob slots.
    DeviceId {
        vendor_id: 0x0816,
        product_id: 0x246F,
        keys: 6,
    },
];

/// The 3-key + 1-knob SIDE-KEYBOARD, and the original target of this tool.
pub const VENDOR_ID: u16 = 0x6D7B;
pub const PRODUCT_ID: u16 = 0xDCFA;

/// Usage page of the vendor configuration interface.
pub const VENDOR_USAGE_PAGE: u16 = 0xFF00;

/// Usage ID of the vendor configuration interface.
pub const VENDOR_USAGE: u16 = 0x02;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supported_list_has_no_duplicates() {
        // A duplicate would make the pad count wrong: Pad::open refuses when it
        // sees more than one, and a repeated entry would look like two pads.
        for (i, a) in SUPPORTED.iter().enumerate() {
            for b in &SUPPORTED[i + 1..] {
                assert_ne!(a, b, "duplicate entry {a:?}");
            }
        }
    }

    #[test]
    fn the_original_device_is_still_supported() {
        assert!(SUPPORTED.contains(&DeviceId {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            keys: 3
        }));
    }
}

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
pub mod protocol;
pub mod response;

pub use emitted::Emitted;
pub use keymap::{KeyTable, KeyType, KeyCode, Modifiers};
pub use protocol::{ProtocolError, Request};
pub use response::{DeviceInfo, Response};

/// USB vendor ID for the supported device family.
pub const VENDOR_ID: u16 = 0x6D7B;

/// USB product ID for the 3-key + 1-knob SIDE-KEYBOARD.
pub const PRODUCT_ID: u16 = 0xDCFA;

/// Usage page of the vendor configuration interface.
pub const VENDOR_USAGE_PAGE: u16 = 0xFF00;

/// Usage ID of the vendor configuration interface.
pub const VENDOR_USAGE: u16 = 0x02;
//! Device access for SDCX SIDE-KEYBOARD macro pads.
//!
//! Everything that touches hardware lives here: finding the right HID interface,
//! the request/reply exchange on the vendor configuration interface, and the
//! watcher that decodes emitted keystrokes. Nothing in this crate knows about
//! command-line arguments or terminal output, so both the CLI and the TUI can
//! drive a pad through it.
//!
//! The protocol itself — report framing, the key model, reply and emitted-event
//! decoding — is in `minikbconfig-core`, which is pure and has no I/O.

pub mod device;
pub mod pad;

pub use device::{find_config_interfaces, InputInterface, InputKind};
pub use pad::{describe, Pad, Watcher};

/// Vendor ID for the supported device family.
pub use minikbconfig_core::VENDOR_ID;

/// Product ID for the 3-key + 1-knob SIDE-KEYBOARD.
pub use minikbconfig_core::PRODUCT_ID;

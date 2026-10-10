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
pub mod diagnose;
pub mod pad;

pub use device::{InputInterface, InputKind, find_config_interfaces};
pub use diagnose::{Diagnosis, RULE_PATH, diagnose};
pub use pad::{Pad, Watcher, describe};

pub use minikbconfig_core::{DeviceId, SUPPORTED};

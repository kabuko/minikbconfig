//! How a pad configuration is expressed as text and as a file.
//!
//! `minikbconfig-core` is what the device speaks: report framing, the key
//! model, reply decoding. This crate is how a *person* speaks about it:
//!
//! - [`parse_action`] turns `key:ctrl-c` into a binding, and [`describe`] turns
//!   a binding back into that same grammar
//! - [`ConfigFile`] is the snapshot format `snapshot` writes and `apply` reads,
//!   and [`Plan`] is the difference between a file and what a pad currently holds
//!
//! It exists as its own crate so both the CLI and the TUI can drive a pad the
//! same way, and so that neither has to depend on the other's internals. Keeping
//! it out of core also keeps `serde` off the crate that defines the wire
//! protocol: core's only dependency is `thiserror`, and that is deliberate.
//!
//! Nothing here performs I/O beyond reading a file, and nothing here prints —
//! a library that writes to stdout stops being usable from another frontend.

pub mod action;
pub mod config;
pub mod error;

pub use action::{ParseError, describe, parse_action};
pub use config::{Change, ConfigFile, Device, Entry, Plan};
pub use error::ConfigError;

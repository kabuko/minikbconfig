//! Errors from reading, validating or serialising a configuration file.
//!
//! A library should describe its own failures rather than hand back a generic
//! one, so both frontends can tell a malformed file from an unreadable one and
//! say something useful.

use minikbconfig_core::protocol::ProtocolError;

/// Something that went wrong with a configuration file or an entry in one.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} is not a valid config file: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("could not parse the configuration file: {0}")]
    Malformed(#[source] serde_json::Error),

    #[error("could not serialise the configuration: {0}")]
    Serialize(#[source] serde_json::Error),

    #[error("key index {index} is out of range (max {max})")]
    IndexOutOfRange { index: u8, max: u8 },

    #[error("key index {index} appears more than once")]
    DuplicateIndex { index: u8 },

    /// A problem with one entry, with the slot named so a file with several
    /// bad entries can be fixed one at a time.
    #[error("key[{index}]: {reason}")]
    Entry {
        index: u8,
        reason: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    #[error("key[{index}] holds a record this build cannot decode: {source}")]
    Undecodable {
        index: u8,
        #[source]
        source: ProtocolError,
    },
}

impl ConfigError {
    /// Attach a slot index to an entry-level problem.
    pub(crate) fn entry(
        index: u8,
        reason: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        ConfigError::Entry {
            index,
            reason: reason.into(),
        }
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(source: serde_json::Error) -> Self {
        ConfigError::Malformed(source)
    }
}

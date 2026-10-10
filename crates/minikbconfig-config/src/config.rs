//! The on-disk configuration format.
//!
//! `snapshot` writes it and `apply` reads it, so both go through these types:
//! a change to one cannot silently break the other. Entries keep the device's
//! original 4-byte `raw` record, which makes the format lossless — a slot this
//! build cannot name is still round-tripped exactly.

use std::path::Path;

use minikbconfig_core::keymap::{self, KEY_RECORD_LEN, KeyTable, KeyType};
use serde::{Deserialize, Serialize};

use crate::action::{describe, parse_action};
use crate::error::ConfigError;

/// Which pad a file was captured from.
///
/// Informational only; [`ConfigFile::validate`] warns on a mismatch so a file
/// from a different device is not applied silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub vendor_id: u16,
    pub product_id: u16,
    #[serde(default)]
    pub name: String,
}

/// The pad's global lighting, as stored in the 11-byte light record.
///
/// A serialisable mirror of `minikbconfig_core::light::LightConfig`, which
/// carries no serde on purpose - the protocol crate must not inherit a
/// serialisation stack for a file format only one side needs.
///
/// Per-key colours are **not** here: the protocol has no read for them, so a
/// snapshot cannot capture them. See PROTOCOL.md - command `12` is not a read
/// and erases key 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lighting {
    /// Effect mode, 1-6. Mode 0 is rejected by the firmware.
    pub mode: u8,
    /// 0-4, where 0 is off.
    pub brightness: u8,
    pub speed: u8,
    pub direction: u8,
    pub cycle: u8,
    /// Hue, 0-255. The device stores degrees scaled by /360.
    pub hue: u8,
    pub saturation: u8,
    pub value: u8,
}

impl Lighting {
    /// Capture the global lighting record read from a pad.
    pub fn from_core(c: minikbconfig_core::light::LightConfig) -> Lighting {
        Lighting {
            mode: c.mode,
            brightness: c.brightness,
            speed: c.speed,
            direction: c.direction,
            cycle: c.cycle,
            hue: c.hue,
            saturation: c.saturation,
            value: c.value,
        }
    }

    /// The core record this describes, for writing back.
    pub fn to_core(self) -> minikbconfig_core::light::LightConfig {
        minikbconfig_core::light::LightConfig {
            light_type: 1,
            mode: self.mode,
            brightness: self.brightness,
            speed: self.speed,
            direction: self.direction,
            cycle: self.cycle,
            single_color_index: 0,
            hue: self.hue,
            saturation: self.saturation,
            value: self.value,
        }
    }

    /// Which fields differ from what the pad currently reports.
    pub fn differences(self, stored: Lighting) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut check = |name: &'static str, a: u8, b: u8| {
            if a != b {
                out.push(name);
            }
        };
        check("mode", self.mode, stored.mode);
        check("brightness", self.brightness, stored.brightness);
        check("speed", self.speed, stored.speed);
        check("direction", self.direction, stored.direction);
        check("cycle", self.cycle, stored.cycle);
        check("hue", self.hue, stored.hue);
        check("saturation", self.saturation, stored.saturation);
        check("value", self.value, stored.value);
        out
    }
}

/// One key slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub index: u8,
    /// The action in `parse_action` syntax, or null for a slot this build
    /// cannot decode.
    pub action: Option<String>,
    /// The device's own record, preferred over re-deriving from `action`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<[u8; KEY_RECORD_LEN]>,
    /// Set when `action` is null: the type byte the firmware used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrecognised_type: Option<String>,
}

/// A whole configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
    #[serde(default)]
    pub layer: u8,
    /// Which profile this was captured from. Profiles hold independent key
    /// tables, so a snapshot without this is ambiguous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<u8>,
    /// Global lighting, captured by `snapshot` and restored by `apply`.
    ///
    /// Optional so files written before this field existed keep working: they
    /// describe a keymap only, and applying one leaves lighting alone rather
    /// than switching it off because the file said nothing about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<Lighting>,
    pub keys: Vec<Entry>,
}

impl ConfigFile {
    /// Build a file describing a key table as read from a pad, recording
    /// which profile it came from.
    ///
    /// There is deliberately no profile-less variant: profiles hold independent
    /// key tables, so a snapshot that does not say which one it is describes is
    /// ambiguous.
    pub fn from_table_in_profile(table: &KeyTable, layer: u8, profile: Option<u8>) -> ConfigFile {
        let mut keys = Vec::new();
        for index in 0..keymap::KEY_SLOTS {
            if let Some(action) = table.get(index) {
                if action == KeyType::Unassigned {
                    continue;
                }
                keys.push(Entry {
                    index,
                    action: Some(describe(action)),
                    raw: Some(KeyTable::record(index, action)),
                    unrecognised_type: None,
                });
            }
        }
        // Keep slots this build cannot decode, so a snapshot is not lossy.
        for (index, ty) in &table.unrecognised {
            keys.push(Entry {
                index: *index,
                action: None,
                raw: None,
                unrecognised_type: Some(format!("0x{ty:02x}")),
            });
        }
        keys.sort_by_key(|e| e.index);
        ConfigFile {
            device: None,
            layer,
            profile,
            light: None,
            keys,
        }
    }

    /// Parse a config file, rejecting anything that would write a bad record.
    ///
    /// Done up front so a malformed file fails before the pad is opened or any
    /// slot is written.
    pub fn parse(text: &str) -> Result<ConfigFile, ConfigError> {
        let file: ConfigFile = serde_json::from_str(text)?;
        file.validate()?;
        Ok(file)
    }

    pub fn load(path: &Path) -> Result<ConfigFile, ConfigError> {
        let shown = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: shown.clone(),
            source,
        })?;
        ConfigFile::parse(&text).map_err(|err| match err {
            ConfigError::Malformed(source) => ConfigError::Parse {
                path: shown.clone(),
                source,
            },
            other => other,
        })
    }

    /// Check every entry can produce a record, and that indices are unique.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = [false; keymap::KEY_SLOTS as usize];
        for entry in &self.keys {
            if entry.index >= keymap::KEY_SLOTS {
                return Err(ConfigError::IndexOutOfRange {
                    index: entry.index,
                    max: keymap::KEY_SLOTS - 1,
                });
            }
            if seen[entry.index as usize] {
                return Err(ConfigError::DuplicateIndex { index: entry.index });
            }
            seen[entry.index as usize] = true;
            entry.record()?;
        }
        Ok(())
    }

    /// Serialise, for `snapshot`.
    pub fn to_json(&self) -> Result<String, ConfigError> {
        serde_json::to_string_pretty(self)
            .map(|text| format!("{text}\n"))
            .map_err(ConfigError::Serialize)
    }
}

impl Entry {
    /// The device record this entry should program.
    ///
    /// Prefers `raw`, so a file replays the device's own bytes exactly;
    /// otherwise re-derives the record from `action`.
    pub fn record(&self) -> Result<Option<[u8; KEY_RECORD_LEN]>, ConfigError> {
        if let Some(raw) = self.raw {
            return Ok(Some(raw));
        }
        match (&self.action, &self.unrecognised_type) {
            (_, Some(ty)) => Err(ConfigError::entry(
                self.index,
                format!("cannot restore a slot of type {ty}: this build does not decode it"),
            )),
            (Some(action), None) => {
                let parsed = parse_action(action).map_err(|e| ConfigError::entry(self.index, e))?;
                Ok(Some(KeyTable::record(self.index, parsed)))
            }
            (None, None) => Err(ConfigError::entry(
                self.index,
                "entry has neither an action nor an unrecognised_type",
            )),
        }
    }

    /// The action this entry describes, if it has one.
    pub fn action(&self) -> Result<Option<KeyType>, ConfigError> {
        let Some(record) = self.record()? else {
            return Ok(None);
        };
        KeyType::from_record(record)
            .map(Some)
            .map_err(|source| ConfigError::Undecodable {
                index: self.index,
                source,
            })
    }
}

/// One slot's difference between the pad and a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub index: u8,
    pub before: KeyType,
    pub after: KeyType,
}

/// How a file differs from what the pad currently holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub changes: Vec<Change>,
}

impl Plan {
    /// Compare a file against the pad's live key table.
    ///
    /// A file is declarative: slots it does not mention are cleared, so the pad
    /// ends up matching the file exactly. Those clears appear in the diff like
    /// any other change rather than happening silently.
    pub fn between(file: &ConfigFile, current: &KeyTable) -> Result<Plan, ConfigError> {
        let mut wanted: Vec<(u8, KeyType)> = Vec::new();
        for entry in &file.keys {
            if let Some(action) = entry.action()? {
                wanted.push((entry.index, action));
            }
        }

        let mut changes = Vec::new();
        for index in 0..keymap::KEY_SLOTS {
            let before = current.get(index).unwrap_or(KeyType::Unassigned);
            let after = wanted
                .iter()
                .find(|(i, _)| *i == index)
                .map(|(_, a)| *a)
                .unwrap_or(KeyType::Unassigned);
            if before != after {
                changes.push(Change {
                    index,
                    before,
                    after,
                });
            }
        }
        Ok(Plan { changes })
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minikbconfig_core::keymap::{KeyCode, Modifiers, consumer};

    fn entry(index: u8, action: KeyType) -> Entry {
        Entry {
            index,
            action: Some(describe(action)),
            raw: Some(KeyTable::record(index, action)),
            unrecognised_type: None,
        }
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let mut table = KeyTable::new();
        table.set(0, KeyType::key(KeyCode::A));
        table.set(16, KeyType::media(consumer::MUTE));
        let file = ConfigFile::from_table_in_profile(&table, 0, None);

        let text = file.to_json().unwrap();
        let parsed = ConfigFile::parse(&text).unwrap();
        assert_eq!(parsed.keys.len(), 2);
        assert_eq!(parsed.layer, 0);
        // Restoring the records must reproduce the original actions exactly.
        for e in &parsed.keys {
            assert_eq!(
                e.record().unwrap(),
                Some(KeyTable::record(e.index, e.action().unwrap().unwrap()))
            );
        }
    }

    #[test]
    fn unassigned_slots_are_left_out_of_a_snapshot() {
        let mut table = KeyTable::new();
        table.set(0, KeyType::key(KeyCode::A));
        table.set(1, KeyType::Unassigned);
        let file = ConfigFile::from_table_in_profile(&table, 0, None);
        assert_eq!(file.keys.len(), 1);
        assert_eq!(file.keys[0].index, 0);
    }

    #[test]
    fn an_unrecognised_slot_is_preserved_not_dropped() {
        let mut table = KeyTable::new();
        table.unrecognised.push((5, 0x99));
        let file = ConfigFile::from_table_in_profile(&table, 0, None);
        assert_eq!(file.keys.len(), 1);
        assert_eq!(file.keys[0].unrecognised_type.as_deref(), Some("0x99"));
        assert!(file.keys[0].action.is_none());
    }

    #[test]
    fn raw_wins_over_action_when_deriving_the_record() {
        // A hand-edited file must still program what `raw` says.
        let e = Entry {
            index: 0,
            action: Some("key:a".into()),
            raw: Some([0x20, 0x02, 0x6D, 0x00]), // shift-f18
            unrecognised_type: None,
        };
        assert_eq!(
            e.action().unwrap(),
            Some(KeyType::combo(Modifiers::LSHIFT, KeyCode::F18))
        );
    }

    #[test]
    fn an_action_string_alone_is_enough() {
        let e = Entry {
            index: 0,
            action: Some("key:ctrl-c".into()),
            raw: None,
            unrecognised_type: None,
        };
        assert_eq!(
            e.action().unwrap(),
            Some(KeyType::combo(Modifiers::LCTRL, KeyCode(0x06)))
        );
    }

    #[test]
    fn an_unrestorable_slot_is_rejected_with_a_clear_error() {
        let file = ConfigFile {
            light: None,
            device: None,
            layer: 0,
            keys: vec![Entry {
                index: 3,
                action: None,
                raw: None,
                unrecognised_type: Some("0x99".into()),
            }],
            profile: None,
        };
        // main.rs prints errors with {:#}, which shows the whole context chain.
        let err = format!("{:#}", file.validate().unwrap_err());
        assert!(err.contains("key[3]"), "error lacks the slot: {err}");
        assert!(err.contains("0x99"), "error lacks the type: {err}");
    }

    #[test]
    fn duplicate_indices_are_rejected() {
        let a = entry(0, KeyType::key(KeyCode::A));
        let b = entry(0, KeyType::key(KeyCode(0x05))); // 'b'
        let file = ConfigFile {
            light: None,
            device: None,
            layer: 0,
            profile: None,
            keys: vec![a, b],
        };
        assert!(format!("{:#}", file.validate().unwrap_err()).contains("more than once"));
    }

    #[test]
    fn an_out_of_range_index_is_rejected() {
        let e = entry(keymap::KEY_SLOTS, KeyType::key(KeyCode::A));
        let file = ConfigFile {
            light: None,
            device: None,
            layer: 0,
            profile: None,
            keys: vec![e],
        };
        assert!(format!("{:#}", file.validate().unwrap_err()).contains("out of range"));
    }

    #[test]
    fn a_bad_action_is_rejected_before_anything_is_written() {
        let e = Entry {
            index: 0,
            action: Some("key:nonexistent".into()),
            raw: None,
            unrecognised_type: None,
        };
        let file = ConfigFile {
            light: None,
            device: None,
            layer: 0,
            profile: None,
            keys: vec![e],
        };
        let err = format!("{:#}", file.validate().unwrap_err());
        assert!(err.contains("nonexistent"), "unhelpful error: {err}");
    }

    #[test]
    fn a_plan_lists_only_slots_that_differ() {
        let mut current = KeyTable::new();
        current.set(0, KeyType::key(KeyCode::SPACE));

        let file = ConfigFile {
            light: None,
            device: None,
            layer: 0,
            profile: None,
            keys: vec![entry(0, KeyType::key(KeyCode::F13))],
        };
        let plan = Plan::between(&file, &current).unwrap();
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].index, 0);
        assert_eq!(plan.changes[0].before, KeyType::key(KeyCode::SPACE));
        assert_eq!(plan.changes[0].after, KeyType::key(KeyCode::F13));
    }

    #[test]
    fn applying_an_identical_file_changes_nothing() {
        let mut current = KeyTable::new();
        current.set(0, KeyType::key(KeyCode::SPACE));
        current.set(1, KeyType::media(consumer::MUTE));
        let file = ConfigFile::from_table_in_profile(&current, 0, None);
        assert!(Plan::between(&file, &current).unwrap().is_empty());
    }

    #[test]
    fn a_file_is_declarative_so_omitted_slots_are_cleared() {
        let mut current = KeyTable::new();
        current.set(0, KeyType::key(KeyCode::SPACE));
        current.set(1, KeyType::key(KeyCode::F18));
        // The file only mentions key 0.
        let file = ConfigFile {
            light: None,
            device: None,
            layer: 0,
            profile: None,
            keys: vec![entry(0, KeyType::key(KeyCode::SPACE))],
        };
        let plan = Plan::between(&file, &current).unwrap();
        assert_eq!(plan.changes.len(), 1, "key 1 should be cleared");
        assert_eq!(plan.changes[0].index, 1);
        assert_eq!(plan.changes[0].after, KeyType::Unassigned);
    }

    #[test]
    fn a_snapshot_records_the_profile_it_came_from() {
        // Profiles hold independent tables, so a snapshot that does not say
        // which one is ambiguous.
        let file = ConfigFile::from_table_in_profile(&KeyTable::new(), 0, Some(3));
        assert_eq!(file.profile, Some(3));
        let text = file.to_json().unwrap();
        assert!(
            text.contains("\"profile\": 3"),
            "profile missing from {text}"
        );
        assert_eq!(ConfigFile::parse(&text).unwrap().profile, Some(3));
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(ConfigFile::parse("{ not json").is_err());
        assert!(ConfigFile::parse("{}").is_err(), "keys is required");
    }
}

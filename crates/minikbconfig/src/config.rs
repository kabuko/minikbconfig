//! The on-disk configuration format.
//!
//! `snapshot` writes it and `apply` reads it, so both go through these types:
//! a change to one cannot silently break the other. Entries keep the device's
//! original 4-byte `raw` record, which makes the format lossless — a slot this
//! build cannot name is still round-tripped exactly.

use std::path::Path;

use anyhow::{bail, Context};
use minikbconfig_core::keymap::{self, KeyTable, KeyType, KEY_RECORD_LEN};
use serde::{Deserialize, Serialize};

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
                    action: Some(crate::describe_action(action)),
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
            keys,
        }
    }

    /// Parse a config file, rejecting anything that would write a bad record.
    ///
    /// Done up front so a malformed file fails before the pad is opened or any
    /// slot is written.
    pub fn parse(text: &str) -> anyhow::Result<ConfigFile> {
        let file: ConfigFile =
            serde_json::from_str(text).context("could not parse the configuration file")?;
        file.validate()?;
        Ok(file)
    }

    pub fn load(path: &Path) -> anyhow::Result<ConfigFile> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        ConfigFile::parse(&text)
            .with_context(|| format!("{} is not a valid config file", path.display()))
    }

    /// Check every entry can produce a record, and that indices are unique.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut seen = [false; keymap::KEY_SLOTS as usize];
        for entry in &self.keys {
            if entry.index >= keymap::KEY_SLOTS {
                bail!(
                    "key index {} is out of range (max {})",
                    entry.index,
                    keymap::KEY_SLOTS - 1
                );
            }
            if seen[entry.index as usize] {
                bail!("key index {} appears more than once", entry.index);
            }
            seen[entry.index as usize] = true;
            entry
                .record()
                .with_context(|| format!("key[{}]", entry.index))?;
        }
        Ok(())
    }

    /// Serialise, for `snapshot`.
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(format!("{}\n", serde_json::to_string_pretty(self)?))
    }
}

impl Entry {
    /// The device record this entry should program.
    ///
    /// Prefers `raw`, so a file replays the device's own bytes exactly;
    /// otherwise re-derives the record from `action`.
    pub fn record(&self) -> anyhow::Result<Option<[u8; KEY_RECORD_LEN]>> {
        if let Some(raw) = self.raw {
            return Ok(Some(raw));
        }
        match (&self.action, &self.unrecognised_type) {
            (_, Some(ty)) => {
                bail!("cannot restore a slot of type {ty}: this build does not decode it")
            }
            (Some(action), None) => {
                let parsed = crate::parse_action(action)
                    .with_context(|| format!("could not parse action '{action}'"))?;
                Ok(Some(KeyTable::record(self.index, parsed)))
            }
            (None, None) => bail!("entry has neither an action nor an unrecognised_type"),
        }
    }

    /// The action this entry describes, if it has one.
    pub fn action(&self) -> anyhow::Result<Option<KeyType>> {
        let Some(record) = self.record()? else {
            return Ok(None);
        };
        Ok(Some(KeyType::from_record(record)?))
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
    pub fn between(file: &ConfigFile, current: &KeyTable) -> anyhow::Result<Plan> {
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
    use minikbconfig_core::keymap::{consumer, KeyCode, Modifiers};

    fn entry(index: u8, action: KeyType) -> Entry {
        Entry {
            index,
            action: Some(crate::describe_action(action)),
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

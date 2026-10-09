//! The key model: what a single key slot can be bound to, and how it is encoded.
//!
//! Each key occupies 4 bytes on the device, `(type, code1, code2, code3)`.
//! See `PROTOCOL.md` §4.

use crate::protocol::{ProtocolError, Request};

/// Bytes per key slot in the device's key table.
pub const KEY_RECORD_LEN: usize = 4;

/// Bytes of key table returned per read chunk (14 whole keys).
pub const CHUNK_PAYLOAD_LEN: usize = 56;

/// Whole key records contained in one read chunk.
pub const CHUNK_KEYS: u8 = (CHUNK_PAYLOAD_LEN / KEY_RECORD_LEN) as u8;

/// Key slots addressable on this device model.
pub const KEY_SLOTS: u8 = 28;

/// Modifiers, as encoded in `code1` for [`KeyType::Standard`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(pub u8);

impl std::fmt::Display for Modifiers {
    /// Canonical rendering, e.g. `ctrl+shift`. Empty when no modifier is set.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const LCTRL: Modifiers = Modifiers(0x01);
    pub const LSHIFT: Modifiers = Modifiers(0x02);
    pub const LALT: Modifiers = Modifiers(0x04);
    pub const LGUI: Modifiers = Modifiers(0x08);
    pub const RCTRL: Modifiers = Modifiers(0x10);
    pub const RSHIFT: Modifiers = Modifiers(0x20);
    pub const RALT: Modifiers = Modifiers(0x40);
    pub const RGUI: Modifiers = Modifiers(0x80);

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    /// Render as a canonical string, e.g. `ctrl+shift`.
    ///
    /// Bare names for the left-hand modifier; see [`Modifiers::parse`].
    pub fn render(self) -> String {
        const TABLE: [(u8, &str); 8] = [
            (0x01, "ctrl"),
            (0x02, "shift"),
            (0x04, "alt"),
            (0x08, "gui"),
            (0x10, "rctrl"),
            (0x20, "rshift"),
            (0x40, "ralt"),
            (0x80, "rgui"),
        ];
        let parts: Vec<&str> = TABLE
            .iter()
            .filter(|(bit, _)| self.0 & bit != 0)
            .map(|(_, name)| *name)
            .collect();
        parts.join("+")
    }

    /// Parse `ctrl+shift` or `lctrl`. Bare names map to the left-hand modifier.
    pub fn parse(s: &str) -> Option<Modifiers> {
        let mut bits = 0u8;
        for part in s.split(['+', '-']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            bits |= match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "lctrl" | "leftctrl" => 0x01,
                "shift" | "lshift" | "leftshift" => 0x02,
                "alt" | "opt" | "option" | "lalt" | "leftalt" => 0x04,
                "gui" | "win" | "super" | "cmd" | "meta" | "lgui" => 0x08,
                "rctrl" | "rightctrl" => 0x10,
                "rshift" | "rightshift" => 0x20,
                "ralt" | "ropt" | "rightalt" => 0x40,
                "rgui" | "rwin" | "rcmd" => 0x80,
                _ => return None,
            };
        }
        Some(Modifiers(bits))
    }
}

/// What a key slot is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// No mapping. The device reports this for unused slots.
    Unassigned,
    /// Relative cursor motion.
    MouseMove(MouseMove),
    /// An ordinary keystroke.
    Standard { modifiers: Modifiers, code: KeyCode },
    /// A Consumer-page usage such as volume or play/pause.
    Media { usage: u16 },
    /// A macro invocation; `index` selects which recorded macro to run.
    Macro { index: u8 },
    /// A mouse wheel click; `up` selects the direction.
    Wheel { up: bool },
    /// Launch a URL.
    OpenWebsite,
    /// A user-defined combination.
    CustomCombination,
}

impl KeyType {
    /// A plain keystroke with no modifiers.
    pub fn key(code: KeyCode) -> KeyType {
        KeyType::Standard { modifiers: Modifiers::NONE, code }
    }

    /// A keystroke with modifiers.
    pub fn combo(modifiers: Modifiers, code: KeyCode) -> KeyType {
        KeyType::Standard { modifiers, code }
    }

    /// A Consumer-page usage such as volume up/down.
    pub fn media(usage: u16) -> KeyType {
        KeyType::Media { usage }
    }

    /// The byte the device stores as the type discriminator.
    pub fn as_byte(self) -> u8 {
        match self {
            KeyType::Unassigned => 0x13,
            KeyType::MouseMove(_) => 0x11,
            KeyType::Standard { .. } => 0x20,
            KeyType::Media { .. } => 0x30,
            KeyType::Macro { .. } => 0x60,
            KeyType::Wheel { .. } => 0x70,
            KeyType::OpenWebsite => 0x80,
            KeyType::CustomCombination => 0xFF,
        }
    }

    /// Decode from the device's stored 4-byte record.
    pub fn from_record(record: [u8; KEY_RECORD_LEN]) -> Result<KeyType, ProtocolError> {
        let [ty, c1, c2, c3] = record;
        Ok(match ty {
            0x13 => KeyType::Unassigned,
            0x11 => KeyType::MouseMove(MouseMove::from_record(c1, c2, c3)),
            0x20 => KeyType::Standard {
                modifiers: Modifiers(c1),
                code: KeyCode(c2),
            },
            0x30 => KeyType::Media {
                usage: u16::from_le_bytes([c1, c2]),
            },
            0x60 => KeyType::Macro { index: c1 },
            0x70 => KeyType::Wheel { up: c1 != 0xFF },
            0x80 => KeyType::OpenWebsite,
            0xFF => KeyType::CustomCombination,
            other => return Err(ProtocolError::UnknownKeyType { ty: other }),
        })
    }
}

/// Relative cursor motion.
///
/// `code1` carries the direction and the high bits of each magnitude; `code2`
/// and `code3` carry the low bytes. See `PROTOCOL.md` §4 for the bit layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseMove {
    pub dx: i16,
    pub dy: i16,
}

impl MouseMove {
    fn from_record(c1: u8, c2: u8, c3: u8) -> Self {
        let vertical = (((c1 & 0x07) as u16) << 8) | c3 as u16;
        let horizontal = (((c1 & 0x70) as u16) << 8) | c2 as u16;
        // Bit 0x08 set means the vertical axis moves up, i.e. positive.
        let dy = if c1 & 0x08 != 0 {
            vertical as i16
        } else {
            -(vertical as i16)
        };
        let dx = if c1 & 0x80 != 0 {
            -(horizontal as i16)
        } else {
            horizontal as i16
        };
        Self { dx, dy }
    }

    fn to_record(self) -> [u8; KEY_RECORD_LEN] {
        let vertical = self.dy.unsigned_abs();
        let horizontal = self.dx.unsigned_abs();
        let mut c1 = ((vertical >> 8) as u8 & 0x07) | ((horizontal >> 8) as u8 & 0x70);
        if self.dy > 0 {
            c1 |= 0x08;
        }
        if self.dx < 0 {
            c1 |= 0x80;
        }
        [0x11, c1, horizontal as u8, vertical as u8]
    }
}

/// A HID keyboard usage ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyCode(pub u8);

impl KeyCode {
    pub const A: KeyCode = KeyCode(0x04);
    pub const Z: KeyCode = KeyCode(0x1D);
    pub const DIGIT_1: KeyCode = KeyCode(0x1E);
    pub const DIGIT_0: KeyCode = KeyCode(0x27);
    pub const ENTER: KeyCode = KeyCode(0x28);
    pub const ESCAPE: KeyCode = KeyCode(0x29);
    pub const SPACE: KeyCode = KeyCode(0x2C);
    pub const F1: KeyCode = KeyCode(0x3A);
    pub const F12: KeyCode = KeyCode(0x45);
    pub const F13: KeyCode = KeyCode(0x68);
    pub const F18: KeyCode = KeyCode(0x6D);
    pub const F19: KeyCode = KeyCode(0x6E);
    pub const F24: KeyCode = KeyCode(0x73);
    pub const CAPS_LOCK: KeyCode = KeyCode(0x39);
    pub const LEFT_SHIFT: KeyCode = KeyCode(0xE1);

    /// A human-readable name, or the raw code if unnamed.
    pub fn name(self) -> String {
        KEYCODE_NAMES
            .iter()
            .find(|(code, _)| *code == self.0)
            .map(|(_, name)| (*name).to_string())
            .unwrap_or_else(|| format!("0x{:02x}", self.0))
    }

    /// Look up a key by name, case-insensitively. Also accepts `0xNN`.
    pub fn parse(s: &str) -> Option<KeyCode> {
        let lower = s.trim().to_ascii_lowercase();
        if let Some(hex) = lower.strip_prefix("0x") {
            return u8::from_str_radix(hex, 16).ok().map(KeyCode);
        }
        KEYCODE_NAMES
            .iter()
            .find(|(_, name)| *name == lower)
            .map(|(code, _)| KeyCode(*code))
    }

    /// All names this build knows, sorted by keycode.
    pub fn all() -> impl Iterator<Item = KeyCode> {
        let mut codes: Vec<KeyCode> = KEYCODE_NAMES.iter().map(|(c, _)| KeyCode(*c)).collect();
        codes.sort();
        codes.dedup();
        codes.into_iter()
    }
}

/// Consumer-page usages the device is known to emit.
pub mod consumer {
    pub const PLAY_PAUSE: u16 = 0xCD;
    pub const NEXT: u16 = 0xB5;
    pub const PREVIOUS: u16 = 0xB6;
    pub const STOP: u16 = 0xB7;
    pub const MUTE: u16 = 0xE2;
    pub const VOLUME_UP: u16 = 0xE9;
    pub const VOLUME_DOWN: u16 = 0xEA;
    pub const BRIGHTNESS_UP: u16 = 0x6F;
    pub const BRIGHTNESS_DOWN: u16 = 0x70;

    /// Name for a usage, or `0xNNNN` if unnamed.
    pub fn name(usage: u16) -> String {
        match usage {
            PLAY_PAUSE => "play/pause".into(),
            NEXT => "next".into(),
            PREVIOUS => "previous".into(),
            STOP => "stop".into(),
            MUTE => "mute".into(),
            VOLUME_UP => "volume-up".into(),
            VOLUME_DOWN => "volume-down".into(),
            BRIGHTNESS_UP => "brightness-up".into(),
            BRIGHTNESS_DOWN => "brightness-down".into(),
            other => format!("0x{other:04x}"),
        }
    }

    /// Look up a usage by name, case-insensitively. Also accepts `0xNNNN`.
    pub fn parse(s: &str) -> Option<u16> {
        let lower = s.trim().to_ascii_lowercase();
        if let Some(hex) = lower.strip_prefix("0x") {
            return u16::from_str_radix(hex, 16).ok();
        }
        Some(match lower.as_str() {
            "play/pause" | "playpause" | "play" => PLAY_PAUSE,
            "next" | "nexttrack" => NEXT,
            "previous" | "prev" | "previoustrack" => PREVIOUS,
            "stop" => STOP,
            "mute" => MUTE,
            "volume-up" | "volup" | "up" => VOLUME_UP,
            "volume-down" | "voldown" | "down" => VOLUME_DOWN,
            "brightness-up" | "briup" => BRIGHTNESS_UP,
            "brightness-down" | "bridown" => BRIGHTNESS_DOWN,
            _ => return None,
        })
    }
}

/// Mouse button bits, as they appear in an emitted HID mouse report.
///
/// This is a *report* field, not a key type: the device has no mouse-button
/// key binding. Type 96 is a macro index and type 112 is the wheel.
pub mod mouse {
    pub const LEFT: u8 = 0x01;
    pub const RIGHT: u8 = 0x02;
    pub const MIDDLE: u8 = 0x04;

    /// Parse a button mask, accepting names, `left+middle`, or `0xNN`.
    ///
    /// The hex form matters because `name` falls back to it for bits outside
    /// the three named buttons, and HID reports can carry others.
    pub fn parse(s: &str) -> Option<u8> {
        let lower = s.trim().to_ascii_lowercase();
        if let Some(hex) = lower.strip_prefix("0x") {
            return u8::from_str_radix(hex, 16).ok();
        }
        let mut bits = 0u8;
        for part in lower.split(['+', '-']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            bits |= match part {
                "left" | "l" => LEFT,
                "right" | "r" => RIGHT,
                "middle" | "m" => MIDDLE,
                _ => return None,
            };
        }
        Some(bits)
    }

    pub fn name(buttons: u8) -> String {
        let mut parts = Vec::new();
        if buttons & LEFT != 0 {
            parts.push("left");
        }
        if buttons & RIGHT != 0 {
            parts.push("right");
        }
        if buttons & MIDDLE != 0 {
            parts.push("middle");
        }
        if parts.is_empty() {
            format!("0x{buttons:02x}")
        } else {
            parts.join("+")
        }
    }
}

/// What to bind to a key: the type plus the payload fields, without the slot.
pub type KeyAction = KeyType;

/// A whole key table read back from the device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyTable {
    pub entries: Vec<Option<KeyType>>,
    /// Slots whose type byte this build does not recognise, as
    /// `(index, type)`. Recorded rather than dropped, so one exotic slot cannot
    /// abort a whole table read and so the data can still be reported.
    pub unrecognised: Vec<(u8, u8)>,
}

impl KeyTable {
    pub fn new() -> Self {
        Self {
            entries: (0..KEY_SLOTS).map(|_| None).collect(),
            unrecognised: Vec::new(),
        }
    }

    /// Parse 56 bytes of key records from a read response payload.
    ///
    /// A record with an unknown type byte is recorded in
    /// [`Self::unrecognised`] rather than treated as an error: one slot the
    /// firmware knows about and this build does not must not stop the rest of
    /// the pad from being read.
    pub fn from_chunk(payload: &[u8], first_index: u8) -> Result<Self, ProtocolError> {
        let mut table = KeyTable::new();
        for (i, record) in payload.chunks_exact(KEY_RECORD_LEN).enumerate() {
            let index = first_index + i as u8;
            if index as usize >= KEY_SLOTS as usize {
                break;
            }
            let mut bytes = [0u8; KEY_RECORD_LEN];
            bytes.copy_from_slice(record);
            match KeyType::from_record(bytes) {
                Ok(action) => table.entries[index as usize] = Some(action),
                Err(ProtocolError::UnknownKeyType { ty }) => {
                    table.entries[index as usize] = None;
                    table.unrecognised.push((index, ty));
                }
                Err(err) => return Err(err),
            }
        }
        Ok(table)
    }

    /// Merge another table's unrecognised-slot notes into this one.
    pub fn absorb_unrecognised(&mut self, other: &KeyTable) {
        self.unrecognised.extend_from_slice(&other.unrecognised);
    }

    pub fn get(&self, index: u8) -> Option<KeyType> {
        self.entries.get(index as usize).copied().flatten()
    }

    pub fn set(&mut self, index: u8, action: KeyType) {
        if let Some(slot) = self.entries.get_mut(index as usize) {
            *slot = Some(action);
        }
    }

    /// The 4-byte device record for a slot.
    pub fn record(_index: u8, action: KeyType) -> [u8; KEY_RECORD_LEN] {
        match action {
            KeyType::MouseMove(m) => m.to_record(),
            KeyType::Standard { modifiers, code } => [0x20, modifiers.bits(), code.0, 0x00],
            KeyType::Media { usage } => [0x30, usage as u8, (usage >> 8) as u8, 0x00],
            KeyType::Macro { index } => [0x60, index, 0x00, 0x00],
            KeyType::Wheel { up } => [0x70, if up { 0x01 } else { 0xFF }, 0x00, 0x00],
            KeyType::OpenWebsite => [0x80, 0x00, 0x00, 0x00],
            KeyType::CustomCombination => [0xFF, 0x00, 0x00, 0x00],
            KeyType::Unassigned => [0x13, 0x00, 0x00, 0x00],
        }
    }
}

/// The device-side command identifiers.
pub mod command {
    pub const GET_CONFIG: u8 = 0x05;
    pub const READ_KEY_LAYOUT_BULK: u8 = 0x07;
    pub const READ_KEY_LAYOUT: u8 = 0x08;
    pub const WRITE_KEY_LAYOUT_BULK: u8 = 0x09;
    pub const GET_LIGHT_CONFIG: u8 = 0x0A;
    pub const SET_LIGHT_CONFIG: u8 = 0x0B;
    pub const GET_MACRO_DATA: u8 = 0x0C;
    pub const SET_MACRO_DATA: u8 = 0x0D;
    pub const RESET_MACRO_DATA: u8 = 0x0F;
    pub const RESTORE_FACTORY: u8 = 0x0F;
    pub const WRITE_KEY: u8 = 0x10;
    pub const READ_KEY_COLORS_BULK: u8 = 0x12;
    pub const WRITE_KEY_COLORS_BULK: u8 = 0x13;
    pub const WRITE_KEY_COLOR: u8 = 0x14;
    pub const SET_LIGHT_EFFECT: u8 = 0x16;
    pub const WRITE_URL_CONFIG: u8 = 0x40;
    pub const READ_URL_CONFIG: u8 = 0x41;
    pub const FIRMWARE_UPDATE: u8 = 0x55;
    pub const ENTER_IAP: u8 = 0x5A;
    pub const SET_PROFILE: u8 = 0xFB;
    pub const SET_AUTO_SLEEP: u8 = 0xFC;
}

/// Re-exported for convenience.
pub use command as Command;

/// Build the request that programs one key slot.
///
/// Note this uses command `0x10`, not the bulk write `0x09`: a short payload
/// sent to the bulk command is stored but never applied.
pub fn write_key(index: u8, action: KeyType) -> Result<Request, ProtocolError> {
    if index >= KEY_SLOTS {
        return Err(ProtocolError::KeyIndexOutOfRange {
            index,
            max: KEY_SLOTS - 1,
        });
    }
    Ok(Request::at(
        Command::WRITE_KEY,
        u16::from(index) * KEY_RECORD_LEN as u16,
    )
    .with_payload(KeyTable::record(index, action)))
}

/// Build the request that reads one 56-byte chunk of the key table.
pub fn read_key_chunk(offset: u16, layer: u8) -> Result<Request, ProtocolError> {
    // Reads use a fixed length byte of 58 (0x3A) regardless of how much data the
    // device returns, so it cannot be derived from the payload length.
    Ok(Request::at(Command::READ_KEY_LAYOUT, offset)
        .with_layer(layer)
        .with_fixed_length(KEY_TABLE_READ_LENGTH))
}

/// The fixed length byte (58 / 0x3A) used by key-table reads.
pub const KEY_TABLE_READ_LENGTH: u8 = 58;

const KEYCODE_NAMES: &[(u8, &str)] = &[
    (0x04, "a"), (0x05, "b"), (0x06, "c"), (0x07, "d"), (0x08, "e"), (0x09, "f"),
    (0x0A, "g"), (0x0B, "h"), (0x0C, "i"), (0x0D, "j"), (0x0E, "k"), (0x0F, "l"),
    (0x10, "m"), (0x11, "n"), (0x12, "o"), (0x13, "p"), (0x14, "q"), (0x15, "r"),
    (0x16, "s"), (0x17, "t"), (0x18, "u"), (0x19, "v"), (0x1A, "w"), (0x1B, "x"),
    (0x1C, "y"), (0x1D, "z"),
    (0x1E, "1"), (0x1F, "2"), (0x20, "3"), (0x21, "4"), (0x22, "5"),
    (0x23, "6"), (0x24, "7"), (0x25, "8"), (0x26, "9"), (0x27, "0"),
    (0x28, "enter"), (0x29, "escape"), (0x2A, "backspace"), (0x2B, "tab"),
    (0x2C, "space"), (0x2D, "minus"), (0x2E, "equal"), (0x2F, "left-bracket"),
    (0x30, "right-bracket"), (0x31, "backslash"), (0x33, "semicolon"),
    (0x34, "quote"), (0x35, "grave"), (0x36, "comma"), (0x37, "dot"),
    (0x38, "slash"), (0x39, "capslock"),
    (0x3A, "f1"), (0x3B, "f2"), (0x3C, "f3"), (0x3D, "f4"),
    (0x3E, "f5"), (0x3F, "f6"), (0x40, "f7"), (0x41, "f8"),
    (0x42, "f9"), (0x43, "f10"), (0x44, "f11"), (0x45, "f12"),
    (0x46, "printscreen"), (0x47, "scrolllock"), (0x48, "pause"),
    (0x49, "insert"), (0x4A, "home"), (0x4B, "pageup"),
    (0x4C, "delete"), (0x4D, "end"), (0x4E, "pagedown"),
    (0x4F, "right"), (0x50, "left"), (0x51, "down"), (0x52, "up"),
    (0x53, "numlock"), (0x54, "kp-slash"), (0x55, "kp-asterisk"),
    (0x56, "kp-minus"), (0x57, "kp-plus"), (0x58, "kp-enter"),
    (0x59, "kp-1"), (0x5A, "kp-2"), (0x5B, "kp-3"), (0x5C, "kp-4"),
    (0x5D, "kp-5"), (0x5E, "kp-6"), (0x5F, "kp-7"), (0x60, "kp-8"),
    (0x61, "kp-9"), (0x62, "kp-0"), (0x63, "kp-dot"),
    (0x65, "app"), (0x68, "f13"), (0x69, "f14"), (0x6A, "f15"), (0x6B, "f16"),
    (0x6C, "f17"), (0x6D, "f18"), (0x6E, "f19"), (0x6F, "f20"),
    (0x70, "f21"), (0x71, "f22"), (0x72, "f23"), (0x73, "f24"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_key_record_matches_documented_bytes() {
        // Ctrl+C: modifier bit 0 is Ctrl (0x01)
        let record = KeyTable::record(0, KeyType::Standard {
            modifiers: Modifiers::LCTRL,
            code: KeyCode(0x03),
        });
        assert_eq!(record, [0x20, 0x01, 0x03, 0x00]);
    }

    #[test]
    fn media_record_matches_documented_bytes() {
        // knob press = MUTE, from PROTOCOL.md
        let record = KeyTable::record(16, KeyType::Media { usage: consumer::MUTE });
        assert_eq!(record, [0x30, 0xE2, 0x00, 0x00]);
    }

    #[test]
    fn media_usage_spans_two_bytes_little_endian() {
        let record = KeyTable::record(17, KeyType::Media { usage: consumer::VOLUME_UP });
        assert_eq!(record, [0x30, 0xE9, 0x00, 0x00]);
        assert_eq!(KeyType::from_record(record).unwrap(), KeyType::Media { usage: 0xE9 });
    }

    #[test]
    fn records_round_trip_through_the_decoder() {
        let cases = [
            KeyType::Unassigned,
            KeyType::Standard { modifiers: Modifiers::LALT, code: KeyCode::A },
            KeyType::Standard { modifiers: Modifiers(0x01), code: KeyCode(0x03) },
            KeyType::Media { usage: consumer::VOLUME_DOWN },
            KeyType::Macro { index: 3 },
            KeyType::Wheel { up: true },
            KeyType::Wheel { up: false },
            KeyType::OpenWebsite,
            KeyType::CustomCombination,
            KeyType::MouseMove(MouseMove { dx: 10, dy: -5 }),
            KeyType::MouseMove(MouseMove { dx: 10, dy: 5 }),
            KeyType::MouseMove(MouseMove { dx: -10, dy: -5 }),
        ];
        for action in cases {
            let record = KeyTable::record(0, action);
            assert_eq!(KeyType::from_record(record).unwrap(), action, "round trip {action:?}");
        }
    }

    #[test]
    fn unknown_type_is_an_error_not_a_silent_default() {
        assert_eq!(
            KeyType::from_record([0x99, 0, 0, 0]),
            Err(ProtocolError::UnknownKeyType { ty: 0x99 })
        );
    }

    #[test]
    fn unassigned_filler_decodes_to_unassigned() {
        assert_eq!(
            KeyType::from_record([0x13, 0x00, 0x00, 0x00]).unwrap(),
            KeyType::Unassigned
        );
    }

    #[test]
    fn read_chunk_request_uses_the_fixed_length_byte() {
        // 0x3A = 58, the length the device expects for key-table reads
        let r = read_key_chunk(0, 0).unwrap().to_report().unwrap();
        assert_eq!(r[2], 0x3A);
        assert_eq!(&r[..8], &[0x06, 0x08, 0x3A, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn chunk_parsing_indexes_from_the_given_start() {
        let mut payload = vec![0x13u8; CHUNK_PAYLOAD_LEN];
        // index 0 -> 'a', index 16 within this chunk -> F18
        payload[0..4].copy_from_slice(&KeyTable::record(0, KeyType::key(KeyCode::A)));
        payload[4..8].copy_from_slice(&KeyTable::record(1, KeyType::key(KeyCode::F18)));
        let table = KeyTable::from_chunk(&payload, 0).unwrap();
        assert_eq!(table.get(0), Some(KeyType::key(KeyCode::A)));
        assert_eq!(table.get(1), Some(KeyType::key(KeyCode::F18)));
        assert_eq!(table.get(2), Some(KeyType::Unassigned));
    }

    #[test]
    fn second_chunk_is_indexed_at_fourteen() {
        let payload = vec![0x13u8; CHUNK_PAYLOAD_LEN];
        let table = KeyTable::from_chunk(&payload, 14).unwrap();
        assert_eq!(table.get(14), Some(KeyType::Unassigned));
    }

    #[test]
    fn modifier_parsing_round_trips() {
        // parse accepts aliases; render always produces the canonical name
        for (text, expected, canonical) in [
            ("ctrl", Modifiers::LCTRL, "ctrl"),
            ("control", Modifiers::LCTRL, "ctrl"),
            ("ctrl+shift", Modifiers(0x03), "ctrl+shift"),
            ("lalt", Modifiers::LALT, "alt"),
            ("opt", Modifiers::LALT, "alt"),
            ("win", Modifiers::LGUI, "gui"),
            ("cmd", Modifiers::LGUI, "gui"),
            ("rctrl+rshift", Modifiers(0x30), "rctrl+rshift"),
        ] {
            assert_eq!(Modifiers::parse(text), Some(expected), "parse {text}");
            assert_eq!(expected.to_string(), canonical, "render {expected:?}");
        }
        assert_eq!(Modifiers::parse("nonsense"), None);
        assert_eq!(Modifiers::parse(""), Some(Modifiers::NONE));
        assert_eq!(Modifiers::NONE.to_string(), "");
    }

    #[test]
    fn keycode_names_round_trip() {
        for code in KeyCode::all() {
            let name = code.name();
            assert_eq!(KeyCode::parse(&name), Some(code), "round trip {name}");
        }
    }

    #[test]
    fn keycode_parse_accepts_hex_and_is_case_insensitive() {
        assert_eq!(KeyCode::parse("F18"), Some(KeyCode::F18));
        assert_eq!(KeyCode::parse("f18"), Some(KeyCode::F18));
        assert_eq!(KeyCode::parse("0x6d"), Some(KeyCode::F18));
        assert_eq!(KeyCode::parse("nonexistent"), None);
    }

    #[test]
    fn consumer_and_mouse_names_round_trip() {
        for usage in [
            consumer::PLAY_PAUSE,
            consumer::NEXT,
            consumer::PREVIOUS,
            consumer::MUTE,
            consumer::VOLUME_UP,
            consumer::VOLUME_DOWN,
        ] {
            assert_eq!(consumer::parse(&consumer::name(usage)), Some(usage));
        }
        for buttons in [mouse::LEFT, mouse::RIGHT, mouse::MIDDLE, 0x07, 0x08] {
            assert_eq!(mouse::parse(&mouse::name(buttons)), Some(buttons));
        }    }

    #[test]
    fn the_vertical_sign_bit_is_set_so_the_cursor_moves_the_intuitive_way() {
        // Traps this in both directions. What matters is not what the emitted
        // HID report says but which way the cursor goes: on this pad a report
        // of dy=+5 moves the cursor *down*, the opposite of the HID convention.
        // So `move:,5` has to set bit 0x08, which makes the pad emit dy=-5 and
        // sends the cursor up. Inverting this to match the report sign -- which
        // looks obviously right -- sends the cursor the wrong way.
        let up = KeyTable::record(0, KeyType::MouseMove(MouseMove { dx: 10, dy: 5 }));
        assert_eq!(up[1] & 0x08, 0x08, "dy positive sets bit 0x08");
        assert_eq!(
            KeyType::from_record(up).unwrap(),
            KeyType::MouseMove(MouseMove { dx: 10, dy: 5 })
        );

        let down = KeyTable::record(0, KeyType::MouseMove(MouseMove { dx: 10, dy: -5 }));
        assert_eq!(down[1] & 0x08, 0x00, "dy negative leaves it clear");
        assert_eq!(
            KeyType::from_record(down).unwrap(),
            KeyType::MouseMove(MouseMove { dx: 10, dy: -5 })
        );

        // Horizontal uses the opposite convention: dx negative sets 0x80.
        let left = KeyTable::record(0, KeyType::MouseMove(MouseMove { dx: -10, dy: 5 }));
        assert_eq!(left[1] & 0x80, 0x80);
        assert_eq!(
            KeyType::from_record(left).unwrap(),
            KeyType::MouseMove(MouseMove { dx: -10, dy: 5 })
        );
    }

    #[test]
    fn wheel_direction_uses_the_vendor_encoding() {
        // The vendor stores wheel up as value1 1 and wheel down as 0xff.
        assert_eq!(KeyTable::record(0, KeyType::Wheel { up: true })[1], 0x01);
        assert_eq!(KeyTable::record(0, KeyType::Wheel { up: false })[1], 0xFF);
        assert_eq!(
            KeyType::from_record([0x70, 0x01, 0, 0]).unwrap(),
            KeyType::Wheel { up: true }
        );
        assert_eq!(
            KeyType::from_record([0x70, 0xFF, 0, 0]).unwrap(),
            KeyType::Wheel { up: false }
        );
    }

    #[test]
    fn macro_type_holds_the_macro_index() {
        // Type 96 is Macro, not a mouse button mask: the vendor's decode path
        // renders it as "M" + index, and its key list is labelled MACRO(n).
        assert_eq!(KeyTable::record(0, KeyType::Macro { index: 3 })[1], 3);
        assert_eq!(
            KeyType::from_record([0x60, 3, 0, 0]).unwrap(),
            KeyType::Macro { index: 3 }
        );
    }

    #[test]
    fn writing_an_out_of_range_slot_is_rejected() {
        assert_eq!(
            write_key(KEY_SLOTS, KeyType::Unassigned),
            Err(ProtocolError::KeyIndexOutOfRange {
                index: KEY_SLOTS,
                max: KEY_SLOTS - 1
            })
        );
    }
}
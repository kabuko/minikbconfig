//! Parsing of the keystrokes the pad emits, used to verify writes.
//!
//! The configuration interface only carries replies. Actual keystrokes arrive
//! on the pad's input interfaces, and checking them is the only way to confirm a
//! write took effect: reads can succeed while behaviour is unchanged.
//! See `PROTOCOL.md` §6.

use crate::protocol::REPORT_LEN;

/// A decoded event from one of the pad's input interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Emitted {
    /// A keystroke on the boot-keyboard interface (unnumbered report).
    Key { modifiers: u8, codes: Vec<u8> },
    /// A keystroke on the report-ID interface (report 1).
    NumberedKey { modifiers: u8, codes: Vec<u8> },
    /// A Consumer-page usage (report 3 or 5).
    Consumer { usage: u16 },
    /// A mouse event (report 2).
    Mouse {
        buttons: u8,
        dx: i16,
        dy: i16,
        wheel: i8,
    },
    /// A System-control usage (report 4).
    SystemControl { usage: u8 },
    /// A report this build does not recognise.
    Unknown { report_id: u8, data: Vec<u8> },
}

impl Emitted {
    /// Decode a report from the boot-keyboard interface.
    ///
    /// Layout: byte 0 modifiers, byte 1 pad, bytes 2–7 keycodes.
    pub fn from_boot(report: &[u8]) -> Emitted {
        let modifiers = report.first().copied().unwrap_or(0);
        let codes = report
            .get(2..8)
            .map(|s| s.iter().copied().filter(|c| *c != 0).collect())
            .unwrap_or_default();
        Emitted::Key { modifiers, codes }
    }

    /// Decode a report from the report-ID interface.
    pub fn from_numbered(report: &[u8]) -> Emitted {
        let Some(&report_id) = report.first() else {
            return Emitted::Unknown {
                report_id: 0,
                data: Vec::new(),
            };
        };
        let data = &report[1..];
        match report_id {
            1 => {
                let modifiers = data.first().copied().unwrap_or(0);
                let codes = data
                    .get(2..8)
                    .map(|s| s.iter().copied().filter(|c| *c != 0).collect())
                    .unwrap_or_default();
                Emitted::NumberedKey { modifiers, codes }
            }
            2 => {
                let buttons = data.first().copied().unwrap_or(0) & 0x1F;
                let dx = read_i16(data, 1);
                let dy = read_i16(data, 3);
                let wheel = data.get(5).copied().unwrap_or(0) as i8;
                Emitted::Mouse {
                    buttons,
                    dx,
                    dy,
                    wheel,
                }
            }
            3 | 5 => {
                let usage = u16::from_le_bytes([
                    data.first().copied().unwrap_or(0),
                    *data.get(1).unwrap_or(&0),
                ]);
                Emitted::Consumer { usage }
            }
            4 => Emitted::SystemControl {
                usage: data.first().copied().unwrap_or(0) & 0x07,
            },
            other => Emitted::Unknown {
                report_id: other,
                data: data.to_vec(),
            },
        }
    }

    /// True for an all-zero "key up" report, which should be ignored when
    /// waiting for a real press.
    pub fn is_release(&self) -> bool {
        match self {
            Emitted::Key { modifiers, codes } | Emitted::NumberedKey { modifiers, codes } => {
                *modifiers == 0 && codes.is_empty()
            }
            Emitted::Consumer { usage } => *usage == 0,
            Emitted::Mouse {
                buttons,
                dx,
                dy,
                wheel,
            } => *buttons == 0 && *dx == 0 && *dy == 0 && *wheel == 0,
            Emitted::SystemControl { usage } => *usage == 0,
            Emitted::Unknown { data, .. } => data.iter().all(|b| *b == 0),
        }
    }

    /// The Consumer usage this event represents, if any.
    pub fn consumer_usage(&self) -> Option<u16> {
        match self {
            Emitted::Consumer { usage } => Some(*usage),
            _ => None,
        }
    }

    /// The primary keycode, if this is a keystroke.
    pub fn keycode(&self) -> Option<u8> {
        match self {
            Emitted::Key { codes, .. } | Emitted::NumberedKey { codes, .. } => {
                codes.first().copied()
            }
            _ => None,
        }
    }

    /// Human-readable rendering.
    pub fn describe(&self) -> String {
        match self {
            Emitted::Key { modifiers, codes } | Emitted::NumberedKey { modifiers, codes } => {
                let mods = crate::keymap::Modifiers(*modifiers).to_string();
                let keys = codes
                    .iter()
                    .map(|c| crate::keymap::KeyCode(*c).name())
                    .collect::<Vec<_>>()
                    .join("+");
                if mods.is_empty() {
                    keys
                } else if keys.is_empty() {
                    format!("{mods}+")
                } else {
                    format!("{mods}+{keys}")
                }
            }
            Emitted::Consumer { usage } => {
                format!("media:{}", crate::keymap::consumer::name(*usage))
            }
            Emitted::Mouse {
                buttons,
                dx,
                dy,
                wheel,
            } => format!(
                "mouse:{} dx={dx} dy={dy} wheel={wheel}",
                crate::keymap::mouse::name(*buttons)
            ),
            Emitted::SystemControl { usage } => format!("system:0x{usage:02x}"),
            Emitted::Unknown { report_id, .. } => format!("unknown report {report_id}"),
        }
    }
}

fn read_i16(data: &[u8], offset: usize) -> i16 {
    match data.get(offset..offset + 2) {
        Some(slice) => i16::from_le_bytes([slice[0], slice[1]]),
        None => 0,
    }
}

/// Length of an input report on the boot-keyboard interface.
pub const BOOT_REPORT_LEN: usize = 8;

const _: () = {
    // Guard against the boot report length drifting out of sync with the code.
    assert!(BOOT_REPORT_LEN <= REPORT_LEN);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_report_decodes_a_keycode() {
        // Real capture: SPACE on the boot interface
        let report = [0x00, 0x00, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x00];
        let event = Emitted::from_boot(&report);
        assert_eq!(event.keycode(), Some(0x2C));
        assert_eq!(event.describe(), "space");
    }

    #[test]
    fn boot_report_decodes_f18_and_f19() {
        for (code, name) in [(0x6Du8, "f18"), (0x6Eu8, "f19")] {
            let report = [0x00, 0x00, code, 0x00, 0x00, 0x00, 0x00, 0x00];
            let event = Emitted::from_boot(&report);
            assert_eq!(event.keycode(), Some(code));
            assert_eq!(event.describe(), name);
        }
    }

    #[test]
    fn consumer_report_decodes_a_media_usage() {
        // Real captures from the knob
        for (report, expected) in [
            ([0x03, 0xEA, 0x00], "volume-down"),
            ([0x03, 0xE2, 0x00], "mute"),
            ([0x03, 0xE9, 0x00], "volume-up"),
        ] {
            let event = Emitted::from_numbered(&report);
            assert_eq!(event.describe(), format!("media:{expected}"));
        }
    }

    #[test]
    fn all_zero_reports_are_releases() {
        assert!(Emitted::from_boot(&[0; 8]).is_release());
        assert!(Emitted::from_numbered(&[0x03, 0x00, 0x00]).is_release());
        assert!(Emitted::from_numbered(&[0x00; 8]).is_release());
    }

    #[test]
    fn a_real_press_is_not_a_release() {
        assert!(!Emitted::from_boot(&[0, 0, 0x2C, 0, 0, 0, 0, 0]).is_release());
        assert!(!Emitted::from_numbered(&[0x03, 0xEA, 0x00]).is_release());
    }

    #[test]
    fn modifiers_render_in_canonical_order() {
        // modifiers 0x03 = Ctrl|Shift, keycode 0x06 = 'c'
        let report = [0x03, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(Emitted::from_boot(&report).describe(), "ctrl+shift+c");
    }

    #[test]
    fn multiple_simultaneous_codes_are_all_reported() {
        let report = [0x00, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(Emitted::from_boot(&report).describe(), "a+b");
    }

    #[test]
    fn mouse_report_decodes_signed_axes() {
        // buttons=1, x=10, y=-5, wheel=3
        let report = [0x02, 0x01, 0x0A, 0x00, 0xFB, 0xFF, 0x03, 0x00];
        let expected = Emitted::Mouse {
            buttons: 1,
            dx: 10,
            dy: -5,
            wheel: 3,
        };
        assert_eq!(Emitted::from_numbered(&report), expected);
        assert_eq!(
            Emitted::from_numbered(&report).describe(),
            "mouse:left dx=10 dy=-5 wheel=3"
        );
    }

    #[test]
    fn unknown_report_ids_are_surfaced_not_dropped() {
        let event = Emitted::from_numbered(&[0x42, 0x01, 0x02]);
        assert_eq!(
            event,
            Emitted::Unknown {
                report_id: 0x42,
                data: vec![0x01, 0x02]
            }
        );
    }

    #[test]
    fn short_input_is_handled_without_panicking() {
        assert_eq!(
            Emitted::from_boot(&[]),
            Emitted::Key {
                modifiers: 0,
                codes: Vec::new()
            }
        );
        let event = Emitted::from_numbered(&[0x02]);
        assert!(matches!(event, Emitted::Mouse { dx: 0, dy: 0, .. }));
    }
}

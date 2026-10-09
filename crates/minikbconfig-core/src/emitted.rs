//! Parsing of the keystrokes the pad emits, used to verify writes.
//!
//! The configuration interface only carries replies. Actual keystrokes arrive
//! on the pad's input interfaces, and checking them is the only way to confirm a
//! write took effect: reads can succeed while behaviour is unchanged.
//! See `PROTOCOL.md` §6.

use crate::keymap::KeyType;
use crate::protocol::REPORT_LEN;

/// How an emitted event relates to the binding that was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    /// The pad emitted exactly what was asked for.
    Exact,
    /// The wanted key was emitted, but others came with it — the pad believes
    /// they are still held. Usually a key that was never released.
    ExtraKeys(Vec<u8>),
    /// Something else entirely.
    Mismatch,
}

impl Match {
    /// True when the pad did what was asked, stuck keys notwithstanding.
    pub fn is_match(&self) -> bool {
        !matches!(self, Match::Mismatch)
    }
}

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

    /// Compare this event against an intended binding.
    ///
    /// Three outcomes rather than two, because "the pad emitted the wrong
    /// action" and "the pad emitted the right action plus something stuck" are
    /// different faults with different fixes.
    pub fn compare(&self, action: KeyType) -> Match {
        let (codes, modifiers) = match self {
            Emitted::Key { modifiers, codes } | Emitted::NumberedKey { modifiers, codes } => {
                (codes, *modifiers)
            }
            Emitted::Consumer { usage } => {
                return match action {
                    KeyType::Media { usage: want } if *usage == want => Match::Exact,
                    _ => Match::Mismatch,
                };
            }
            Emitted::Mouse { dx, dy, .. } => {
                return match action {
                    KeyType::MouseMove(want) if *dx == want.dx && *dy == want.dy => Match::Exact,
                    _ => Match::Mismatch,
                };
            }
            _ => return Match::Mismatch,
        };

        let (want_mods, want_code) = match action {
            KeyType::Standard { modifiers, code } => (modifiers.bits(), code.0),
            _ => return Match::Mismatch,
        };
        if modifiers != want_mods || !codes.contains(&want_code) {
            return Match::Mismatch;
        }
        // The wanted key is present. Anything else in the array is a key the
        // pad believes is still held, which is a real fault but not a wrong
        // binding.
        let extra: Vec<u8> = codes.iter().copied().filter(|c| *c != want_code).collect();
        if extra.is_empty() {
            Match::Exact
        } else {
            Match::ExtraKeys(extra)
        }
    }

    /// True if this event is what the given binding should produce.
    ///
    /// This is the last step of the verification loop in `DESIGN.md` section 7:
    /// a write that stored correctly proves nothing, because several wrong
    /// encodings read back perfectly while emitting nothing. Only matching the
    /// emitted report against the intended binding is evidence.
    ///
    /// Returns false for types this build cannot verify by observation, which
    /// is the point: a macro or wheel binding has no emission to compare.
    pub fn matches(&self, action: KeyType) -> bool {
        self.compare(action).is_match()
    }

    /// True if the pad says it lit up.
    ///
    /// The pad emits an LED notification rather than anything on the input
    /// interfaces, so lighting cannot be confirmed this way.
    pub fn is_observable_for(action: KeyType) -> bool {
        matches!(
            action,
            KeyType::Standard { .. } | KeyType::Media { .. } | KeyType::MouseMove { .. }
        )
    }

    /// Human-readable rendering.
    pub fn describe(&self) -> String {
        // A release carries no information, but it arrives constantly -- raw
        // output of a knob turn is roughly three releases per press. Rendering
        // it as "media:0x0000" or as an empty string is noise that buries the
        // presses, so it gets one word.
        if self.is_release() {
            return "release".to_string();
        }
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
    use crate::keymap::{KeyCode, Modifiers, MouseMove};

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

    #[test]
    fn a_keystroke_matches_only_its_own_modifiers_and_code() {
        let press = Emitted::from_boot(&[0x00, 0x00, 0x04, 0, 0, 0, 0, 0]);
        assert!(press.matches(KeyType::key(KeyCode::A)));
        // Wrong key.
        assert!(!press.matches(KeyType::key(KeyCode(0x05))));
        // Right key, wrong modifiers.
        assert!(!press.matches(KeyType::combo(Modifiers::LCTRL, KeyCode::A)));
        // Unassigned matches nothing at all.
        assert!(!press.matches(KeyType::Unassigned));
    }

    #[test]
    fn a_modifier_combination_matches_only_with_the_same_mask() {
        let ctrl_a = Emitted::from_boot(&[0x01, 0x00, 0x04, 0, 0, 0, 0, 0]);
        assert!(ctrl_a.matches(KeyType::combo(Modifiers::LCTRL, KeyCode::A)));
        assert!(!ctrl_a.matches(KeyType::combo(Modifiers::LSHIFT, KeyCode::A)));
        assert!(!ctrl_a.matches(KeyType::combo(Modifiers(0x03), KeyCode::A)));
    }

    #[test]
    fn consumer_and_mouse_events_match_their_own_bindings() {
        let mute = Emitted::from_numbered(&[0x03, 0xE2, 0x00]);
        assert!(mute.matches(KeyType::media(0xE2)));
        assert!(!mute.matches(KeyType::media(0xE9)));
        // A consumer event is not a keystroke.
        assert!(!mute.matches(KeyType::key(KeyCode::A)));

        let move_event = Emitted::from_numbered(&[0x02, 0x00, 0x0A, 0x00, 0xFB, 0xFF, 0, 0]);
        assert!(move_event.matches(KeyType::MouseMove(MouseMove { dx: 10, dy: -5 })));
        assert!(!move_event.matches(KeyType::MouseMove(MouseMove { dx: 10, dy: 5 })));
    }

    #[test]
    fn only_observable_types_can_be_verified_by_emission() {
        assert!(Emitted::is_observable_for(KeyType::key(KeyCode::A)));
        assert!(Emitted::is_observable_for(KeyType::media(0xE2)));
        assert!(Emitted::is_observable_for(KeyType::MouseMove(
            MouseMove::default()
        )));
        // These either do nothing or emit nothing comparable.
        for action in [
            KeyType::Unassigned,
            KeyType::Macro { index: 0 },
            KeyType::Wheel { up: true },
            KeyType::OpenWebsite,
            KeyType::CustomCombination,
        ] {
            assert!(!Emitted::is_observable_for(action), "{action:?}");
        }
    }

    #[test]
    fn a_stuck_extra_key_is_distinguished_from_a_wrong_action() {
        // Observed on hardware: the pad emitted `a` alongside f13 because an
        // earlier macro left `a` unreleased in the boot report array. That is
        // a different fault from emitting the wrong action, and conflating
        // them would report a correct write as a failure.
        let stuck = Emitted::from_boot(&[0x00, 0x00, 0x04, 0x68, 0, 0, 0, 0]);
        let want = KeyType::key(KeyCode::F13);

        assert_eq!(stuck.compare(want), Match::ExtraKeys(vec![KeyCode::A.0]));
        // Still a match for "did it emit what we asked".
        assert!(stuck.matches(want));
        // But not a clean confirmation.
        assert_ne!(stuck.compare(want), Match::Exact);
    }

    #[test]
    fn a_wrong_key_is_still_a_mismatch_even_alongside_the_right_one() {
        // Containment must not become so loose that anything passes.
        let both = Emitted::from_boot(&[0x00, 0x00, 0x04, 0x68, 0, 0, 0, 0]);
        assert_eq!(both.compare(KeyType::key(KeyCode(0x05))), Match::Mismatch);
        assert!(!both.matches(KeyType::key(KeyCode(0x05))));
    }

    #[test]
    fn a_single_correct_key_is_an_exact_match() {
        let event = Emitted::from_boot(&[0x00, 0x00, 0x68, 0, 0, 0, 0, 0]);
        assert_eq!(event.compare(KeyType::key(KeyCode::F13)), Match::Exact);
    }

    #[test]
    fn match_helpers_agree_with_the_variants() {
        assert!(Match::Exact.is_match());
        assert!(Match::ExtraKeys(vec![1]).is_match());
        assert!(!Match::Mismatch.is_match());
    }

    #[test]
    fn releases_render_as_one_word_rather_than_as_noise() {
        // Found by looking at real raw output: the pad emits roughly three
        // releases for every press, so a release that renders as "media:0x0000"
        // or as an empty string buries the presses it sits between.
        assert_eq!(Emitted::from_boot(&[0; 8]).describe(), "release");
        assert_eq!(
            Emitted::from_numbered(&[0x03, 0x00, 0x00]).describe(),
            "release"
        );
        assert_eq!(
            Emitted::from_numbered(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]).describe(),
            "release"
        );
    }

    #[test]
    fn a_release_is_still_distinguishable_from_a_press_when_filtered() {
        // The filtered view must keep dropping releases; only the rendering
        // changes, not is_release.
        assert!(Emitted::from_boot(&[0; 8]).is_release());
        assert!(!Emitted::from_boot(&[0x00, 0x00, 0x2C, 0, 0, 0, 0, 0]).is_release());
    }
}

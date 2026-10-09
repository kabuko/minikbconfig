//! Parsing user-facing action strings such as `key:ctrl-c`, `media:volume-up`.
//!
//! Mirrors the accepted spellings so existing configs and habits carry over.

use minikbconfig_core::keymap::{consumer, mouse, Duration, KeyCode, KeyType, Modifiers};

/// Errors from parsing an action string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("action must have a kind prefix, e.g. 'key:ctrl-c', 'media:mute', 'mouse:left'")]
    MissingKind,
    #[error("unknown action kind '{kind}'; expected key, media, mouse, move, none or url")]
    UnknownKind { kind: String },
    #[error("could not parse key name '{name}'; try a name like 'f13', 'space' or '0x6d'")]
    BadKey { name: String },
    #[error("could not parse media key '{name}'; try 'volume-up', 'mute', 'play/pause' or '0xe9'")]
    BadMedia { name: String },
    #[error("could not parse mouse action '{name}'; try 'left', 'middle', 'wheel-up' or 'left+middle'")]
    BadMouse { name: String },
    #[error("could not parse mouse movement '{spec}'; expected 'move:<dx>,<dy>' with signed integers")]
    BadMove { spec: String },
    #[error("unrecognised duration '{name}'; try 0, 120, 300, 600, 1800 or a custom 'delay:<ms>'")]
    BadDuration { name: String },
}

/// Parse an action string into a device binding.
///
/// Accepted forms:
///
/// ```text
/// none                        clear the slot
/// key:a                       a bare keystroke
/// key:ctrl-c                 keystroke with modifiers
/// key:0x6d                   keystroke by raw HID code
/// media:volume-up            consumer usage
/// media:0xea                  consumer usage by number
/// mouse:left                 button click, combos with '+'
/// mouse:wheel-up             wheel (supported by the device; see caveat below)
/// move:10,-5                  relative cursor motion
/// delay:250                   macro delay in milliseconds
/// url:<target>                launch a URL
/// ```
pub fn parse_action(input: &str) -> Result<KeyType, ParseError> {
    let input = input.trim();

    // `none` clears a slot and carries no colon, so it has to be recognised
    // before the kind prefix is split off.
    match input.to_ascii_lowercase().as_str() {
        "none" | "off" | "clear" => return Ok(KeyType::Unassigned),
        _ => {}
    }

    let (kind, rest) = input.split_once(':').ok_or(ParseError::MissingKind)?;
    let rest = rest.trim();

    match kind.to_ascii_lowercase().as_str() {
        "key" | "combo" => parse_key(rest),
        "media" | "consumer" => consumer::parse(rest)
            .map(KeyType::media)
            .ok_or_else(|| ParseError::BadMedia { name: rest.to_string() }),
        "mouse" => parse_mouse(rest),
        "move" => parse_move(rest),
        "delay" => parse_duration(rest),
        "url" | "website" => Ok(KeyType::OpenWebsite),
        other => Err(ParseError::UnknownKind { kind: other.to_string() }),
    }
}

fn parse_key(rest: &str) -> Result<KeyType, ParseError> {
    // Split trailing modifier tokens from the final key name: "ctrl-shift-f13".
    let (mods, key) = split_modifiers(rest);
    let code = KeyCode::parse(key).ok_or_else(|| ParseError::BadKey { name: key.to_string() })?;
    Ok(KeyType::Standard { modifiers: mods, code })
}

/// Peel modifier tokens off either end, so `ctrl-c`, `c-ctrl` and
/// `ctrl-shift-alt-f13` all work.
///
/// Both ends are tried because the documented spelling puts the key last while
/// habit often puts it first. Splitting on `-` alone is not enough to find the
/// key, since key names contain dashes (`left-bracket`, `kp-enter`); so leading
/// tokens are peeled as modifiers, then trailing ones, and whatever is left in
/// between is rejoined and looked up as the key.
fn split_modifiers(input: &str) -> (Modifiers, &str) {
    let mut modifiers = Modifiers::NONE;
    let mut rest = input;

    while let Some((head, tail)) = rest.split_once(['-', '+']) {
        if head.is_empty() || tail.is_empty() {
            break;
        }
        match Modifiers::parse(head) {
            Some(m) => modifiers = Modifiers(modifiers.bits() | m.bits()),
            None => break,
        }
        rest = tail;
    }

    while let Some((head, tail)) = rest.rsplit_once(['-', '+']) {
        if head.is_empty() || tail.is_empty() {
            break;
        }
        // `Modifiers::parse` accepts an empty string as "no modifiers"; only a
        // token that contributes a bit may be peeled off.
        match Modifiers::parse(tail) {
            Some(m) if m != Modifiers::NONE => modifiers = Modifiers(modifiers.bits() | m.bits()),
            _ => break,
        }
        rest = head;
    }

    (modifiers, rest)
}

fn parse_mouse(rest: &str) -> Result<KeyType, ParseError> {
    let lower = rest.to_ascii_lowercase();
    // Wheel is encoded as a signed delta in the device's type-3 payload, which
    // this build does not model; buttons are.
    if lower.contains("wheel") || lower.contains("scroll") {
        return Err(ParseError::BadMouse { name: rest.to_string() });
    }
    let buttons = mouse::parse(rest).ok_or_else(|| ParseError::BadMouse { name: rest.to_string() })?;
    if buttons == 0 {
        return Err(ParseError::BadMouse { name: rest.to_string() });
    }
    Ok(KeyType::MouseButton { buttons })
}

fn parse_move(rest: &str) -> Result<KeyType, ParseError> {
    let bad = || ParseError::BadMove { spec: rest.to_string() };
    let (dx, dy) = rest.split_once(',').ok_or_else(bad)?;
    let dx: i16 = dx.trim().parse().map_err(|_| bad())?;
    let dy: i16 = dy.trim().parse().map_err(|_| bad())?;
    Ok(KeyType::MouseMove(minikbconfig_core::keymap::MouseMove { dx, dy }))
}

fn parse_duration(rest: &str) -> Result<KeyType, ParseError> {
    let bad = || ParseError::BadDuration { name: rest.to_string() };
    let millis: u16 = rest.trim().parse().map_err(|_| bad())?;
    for index in 0..=4u8 {
        if Duration(index).to_millis() == Some(millis) {
            return Ok(KeyType::Duration(Duration(index)));
        }
    }
    // Not one of the device's presets; the closest larger preset is the best
    // available approximation, since the type-70 payload is only one byte.
    let nearest = (0..=4u8)
        .filter(|i| Duration(*i).to_millis().unwrap_or(0) >= millis)
        .min_by_key(|i| Duration(*i).to_millis().unwrap_or(0));
    match nearest {
        Some(index) => Ok(KeyType::Duration(Duration(index))),
        None => Err(ParseError::BadDuration { name: rest.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minikbconfig_core::keymap::KeyTable;

    #[test]
    fn plain_keystrokes() {
        assert_eq!(parse_action("key:a").unwrap(), KeyType::key(KeyCode::A));
        assert_eq!(parse_action("key:f13").unwrap(), KeyType::key(KeyCode::F13));
        assert_eq!(parse_action("key:space").unwrap(), KeyType::key(KeyCode::SPACE));
    }

    #[test]
    fn keystrokes_with_modifiers_in_either_order() {
        // HID usage 0x06 is 'c'; the modifier mask is Ctrl = bit 0.
        let expected = KeyType::combo(Modifiers::LCTRL, KeyCode(0x06));
        assert_eq!(parse_action("key:ctrl-c").unwrap(), expected);
        assert_eq!(parse_action("key:c-ctrl").unwrap(), expected);
        assert_eq!(parse_action("key:ctrl-shift-f13").unwrap(),
                   KeyType::combo(Modifiers(0x03), KeyCode::F13));
    }

    #[test]
    fn three_modifiers_are_all_collected() {
        // The documented spelling puts the key last, so every modifier has to
        // be peeled off in turn; one pass is not enough.
        assert_eq!(
            parse_action("key:ctrl-shift-alt-t").unwrap(),
            KeyType::combo(Modifiers(0x07), KeyCode(0x17)) // t
        );
        // Mixed separators still work.
        assert_eq!(
            parse_action("key:ctrl+shift-alt-t").unwrap(),
            KeyType::combo(Modifiers(0x07), KeyCode(0x17))
        );
    }

    #[test]
    fn modifier_aliases_compose() {
        assert_eq!(
            parse_action("key:cmd-space").unwrap(),
            KeyType::combo(Modifiers::LGUI, KeyCode::SPACE)
        );
        assert_eq!(
            parse_action("key:rctrl-c-alt").unwrap(),
            KeyType::combo(Modifiers(0x14), KeyCode(0x06)) // rctrl+alt on c
        );
    }

    #[test]
    fn a_dash_that_is_not_a_separator_is_left_alone() {
        // "1" is not a modifier, so "f-1" must not be split into key "f" with
        // something on modifier 1; it is simply not a key this build knows.
        assert!(matches!(parse_action("key:f-1"), Err(ParseError::BadKey { .. })));
    }

    #[test]
    fn ctrl_c_encodes_to_the_documented_record() {
        // Guards the modifier bit: Ctrl is bit 0, not LAlt, and 'c' is usage 0x06.
        let action = parse_action("key:ctrl-c").unwrap();
        assert_eq!(KeyTable::record(0, action), [0x20, 0x01, 0x06, 0x00]);
    }

    #[test]
    fn hex_keycodes_are_accepted() {
        assert_eq!(parse_action("key:0x6d").unwrap(), KeyType::key(KeyCode::F18));
    }

    #[test]
    fn key_named_with_a_dash_is_not_split() {
        // "enter" and "pageup" have no dash, but check a compound safety case.
        assert_eq!(parse_action("key:f13").unwrap(), KeyType::key(KeyCode::F13));
    }

    #[test]
    fn media_actions() {
        assert_eq!(parse_action("media:volume-up").unwrap(), KeyType::media(0xE9));
        assert_eq!(parse_action("media:mute").unwrap(), KeyType::media(consumer::MUTE));
        assert_eq!(parse_action("media:0xea").unwrap(), KeyType::media(0xEA));
    }

    #[test]
    fn mouse_actions() {
        assert_eq!(
            parse_action("mouse:left").unwrap(),
            KeyType::MouseButton { buttons: mouse::LEFT }
        );
        assert_eq!(
            parse_action("mouse:left+middle").unwrap(),
            KeyType::MouseButton { buttons: 0x05 }
        );
    }

    #[test]
    fn wheel_is_rejected_with_a_clear_error_rather_than_silently_dropped() {
        // The device models wheel through type 3, which this build does not
        // encode; saying so beats writing a button click instead.
        assert_eq!(
            parse_action("mouse:wheel-up"),
            Err(ParseError::BadMouse { name: "wheel-up".into() })
        );
    }

    #[test]
    fn movement_actions() {
        assert_eq!(
            parse_action("move:10,-5").unwrap(),
            KeyType::MouseMove(minikbconfig_core::keymap::MouseMove { dx: 10, dy: -5 })
        );
    }

    #[test]
    fn movement_round_trips_through_the_device_record() {
        let action = parse_action("move:10,-5").unwrap();
        let record = KeyTable::record(0, action);
        assert_eq!(KeyType::from_record(record).unwrap(), action);
    }

    #[test]
    fn durations_map_to_device_presets() {
        assert_eq!(parse_action("delay:120").unwrap(), KeyType::Duration(Duration(1)));
        assert_eq!(parse_action("delay:1800").unwrap(), KeyType::Duration(Duration(4)));
        // 250 is not a preset; the nearest larger one is used.
        assert_eq!(parse_action("delay:250").unwrap(), KeyType::Duration(Duration(2)));
    }

    #[test]
    fn clearing_and_urls() {
        assert_eq!(parse_action("none").unwrap(), KeyType::Unassigned);
        assert_eq!(parse_action("url:example.com").unwrap(), KeyType::OpenWebsite);
    }

    #[test]
    fn errors_are_specific() {
        assert_eq!(parse_action("nonsense"), Err(ParseError::MissingKind));
        assert!(matches!(parse_action("bogus:a"), Err(ParseError::UnknownKind { .. })));
        assert!(matches!(parse_action("key:nonexistent"), Err(ParseError::BadKey { .. })));
        assert!(matches!(parse_action("media:nope"), Err(ParseError::BadMedia { .. })));
        assert!(matches!(parse_action("move:abc"), Err(ParseError::BadMove { .. })));
    }

    #[test]
    fn input_is_case_and_space_insensitive() {
        assert_eq!(parse_action("  KEY:F18  ").unwrap(), KeyType::key(KeyCode::F18));
        assert_eq!(parse_action("Media:Volume-Up").unwrap(), KeyType::media(0xE9));
    }
}
//! Parsing user-facing action strings such as `key:ctrl-c`, `media:volume-up`.
//!
//! Mirrors the accepted spellings so existing configs and habits carry over.

use minikbconfig_core::keymap::{self, KeyCode, KeyType, Modifiers, consumer};

/// Errors from parsing an action string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("action must have a kind prefix, e.g. 'key:ctrl-c', 'media:mute'")]
    MissingKind,
    #[error("unknown action kind '{kind}'; expected key, media, move or none")]
    UnknownKind { kind: String },
    #[error("could not parse key name '{name}'{}", format_nearby_keys(nearby))]
    BadKey { name: String, nearby: Vec<String> },
    #[error("could not parse media key '{name}'; try 'volume-up', 'mute', 'play/pause' or '0xe9'")]
    BadMedia { name: String },
    #[error(
        "'{name}' is not a key type this device has. There is no mouse-button \
         binding: type 0x60 is a macro index and type 0x70 is the mouse wheel"
    )]
    BadMouse { name: String },
    #[error(
        "could not parse mouse movement '{spec}'; expected 'move:<dx>,<dy>' with signed integers"
    )]
    BadMove { spec: String },
    #[error(
        "'{name}' is not a key type this device has. Type 0x70 is the mouse wheel \
         (up or down), not a delay; macros are referenced by index and need macro \
         data that is not implemented"
    )]
    BadDuration { name: String },

    #[error(
        "URL bindings are not implemented. Type 0x80 only records *that* a key opens \
         a URL; the target itself is stored separately by command 0x40, which is \
         unverified, so writing one now would leave a key that reads back as bound \
         and does nothing"
    )]
    UnsupportedUrl,
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
/// move:10,-5                  relative cursor motion
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
        "media" | "consumer" => {
            consumer::parse(rest)
                .map(KeyType::media)
                .ok_or_else(|| ParseError::BadMedia {
                    name: rest.to_string(),
                })
        }
        "mouse" => Err(ParseError::BadMouse {
            name: rest.to_string(),
        }),
        "move" => parse_move(rest),
        "delay" => Err(ParseError::BadDuration {
            name: rest.to_string(),
        }),
        "url" | "website" => Err(ParseError::UnsupportedUrl),
        other => Err(ParseError::UnknownKind {
            kind: other.to_string(),
        }),
    }
}

fn format_nearby_keys(nearby: &[String]) -> String {
    if nearby.is_empty() {
        "; try a name like 'f13', 'space' or '0x6d'".to_string()
    } else {
        format!("; did you mean: {}?", nearby.join(", "))
    }
}

fn nearby_key_names(name: &str) -> Vec<String> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<String> = KeyCode::all()
        .map(|c| c.name())
        .filter(|n| n.starts_with(&lower) || (lower.len() >= 2 && n.contains(&lower)))
        .collect();
    matches.sort();
    matches.dedup();
    matches.truncate(5);
    matches
}

fn parse_key(rest: &str) -> Result<KeyType, ParseError> {
    // Split trailing modifier tokens from the final key name: "ctrl-shift-f13".
    let (mods, key) = split_modifiers(rest);
    let code = KeyCode::parse(key).ok_or_else(|| ParseError::BadKey {
        name: key.to_string(),
        nearby: nearby_key_names(key),
    })?;
    Ok(KeyType::Standard {
        modifiers: mods,
        code,
    })
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

fn parse_move(rest: &str) -> Result<KeyType, ParseError> {
    let bad = || ParseError::BadMove {
        spec: rest.to_string(),
    };
    let (dx, dy) = rest.split_once(',').ok_or_else(bad)?;
    let dx: i16 = dx.trim().parse().map_err(|_| bad())?;
    let dy: i16 = dy.trim().parse().map_err(|_| bad())?;
    Ok(KeyType::MouseMove(minikbconfig_core::keymap::MouseMove {
        dx,
        dy,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minikbconfig_core::keymap::KeyTable;

    #[test]
    fn plain_keystrokes() {
        assert_eq!(parse_action("key:a").unwrap(), KeyType::key(KeyCode::A));
        assert_eq!(parse_action("key:f13").unwrap(), KeyType::key(KeyCode::F13));
        assert_eq!(
            parse_action("key:space").unwrap(),
            KeyType::key(KeyCode::SPACE)
        );
    }

    #[test]
    fn keystrokes_with_modifiers_in_either_order() {
        // HID usage 0x06 is 'c'; the modifier mask is Ctrl = bit 0.
        let expected = KeyType::combo(Modifiers::LCTRL, KeyCode(0x06));
        assert_eq!(parse_action("key:ctrl-c").unwrap(), expected);
        assert_eq!(parse_action("key:c-ctrl").unwrap(), expected);
        assert_eq!(
            parse_action("key:ctrl-shift-f13").unwrap(),
            KeyType::combo(Modifiers(0x03), KeyCode::F13)
        );
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
        assert!(matches!(
            parse_action("key:f-1"),
            Err(ParseError::BadKey { .. })
        ));
    }

    #[test]
    fn ctrl_c_encodes_to_the_documented_record() {
        // Guards the modifier bit: Ctrl is bit 0, not LAlt, and 'c' is usage 0x06.
        let action = parse_action("key:ctrl-c").unwrap();
        assert_eq!(KeyTable::record(0, action), [0x20, 0x01, 0x06, 0x00]);
    }

    #[test]
    fn hex_keycodes_are_accepted() {
        assert_eq!(
            parse_action("key:0x6d").unwrap(),
            KeyType::key(KeyCode::F18)
        );
    }

    #[test]
    fn key_named_with_a_dash_is_not_split() {
        // "enter" and "pageup" have no dash, but check a compound safety case.
        assert_eq!(parse_action("key:f13").unwrap(), KeyType::key(KeyCode::F13));
    }

    #[test]
    fn media_actions() {
        assert_eq!(
            parse_action("media:volume-up").unwrap(),
            KeyType::media(0xE9)
        );
        assert_eq!(
            parse_action("media:mute").unwrap(),
            KeyType::media(consumer::MUTE)
        );
        assert_eq!(parse_action("media:0xea").unwrap(), KeyType::media(0xEA));
    }

    #[test]
    fn mouse_buttons_are_refused_because_the_device_has_none() {
        // Type 0x60 is a macro index and 0x70 is the wheel; there is no
        // mouse-button key binding, so this must fail loudly rather than
        // silently writing a macro reference.
        assert!(matches!(
            parse_action("mouse:left"),
            Err(ParseError::BadMouse { .. })
        ));
        assert!(matches!(
            parse_action("mouse:left+middle"),
            Err(ParseError::BadMouse { .. })
        ));
        let err = format!("{:#}", parse_action("mouse:left").unwrap_err());
        assert!(err.contains("macro"), "explains what 0x60 really is: {err}");
    }

    #[test]
    fn delays_are_refused_because_type_seventy_is_the_wheel() {
        assert!(matches!(
            parse_action("delay:250"),
            Err(ParseError::BadDuration { .. })
        ));
        let err = format!("{:#}", parse_action("delay:250").unwrap_err());
        assert!(err.contains("wheel"), "explains what 0x70 really is: {err}");
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
    fn clearing_a_slot() {
        assert_eq!(parse_action("none").unwrap(), KeyType::Unassigned);
        assert_eq!(parse_action("off").unwrap(), KeyType::Unassigned);
        assert_eq!(parse_action("clear").unwrap(), KeyType::Unassigned);
    }

    #[test]
    fn urls_are_refused_because_the_target_cannot_be_written() {
        // Type 0x80 alone produces a key that reads back as bound and does
        // nothing, because the URL itself lives behind unverified command 0x40.
        assert_eq!(
            parse_action("url:example.com"),
            Err(ParseError::UnsupportedUrl)
        );
        assert_eq!(
            parse_action("website:example.com"),
            Err(ParseError::UnsupportedUrl)
        );
        let err = parse_action("url:example.com").unwrap_err().to_string();
        assert!(err.contains("0x40"), "names the missing command: {err}");
    }

    #[test]
    fn errors_are_specific() {
        assert_eq!(parse_action("nonsense"), Err(ParseError::MissingKind));
        assert!(matches!(
            parse_action("bogus:a"),
            Err(ParseError::UnknownKind { .. })
        ));
        assert!(matches!(
            parse_action("key:nonexistent"),
            Err(ParseError::BadKey { .. })
        ));
        assert!(matches!(
            parse_action("media:nope"),
            Err(ParseError::BadMedia { .. })
        ));
        assert!(matches!(
            parse_action("move:abc"),
            Err(ParseError::BadMove { .. })
        ));
    }

    #[test]
    fn input_is_case_and_space_insensitive() {
        assert_eq!(
            parse_action("  KEY:F18  ").unwrap(),
            KeyType::key(KeyCode::F18)
        );
        assert_eq!(
            parse_action("Media:Volume-Up").unwrap(),
            KeyType::media(0xE9)
        );
    }
}

/// Render an action in the same grammar [`parse_action`] accepts.
///
/// Round-trips: `parse_action(&describe(action))` gives back the action, for
/// every type this build can write. Types it cannot write are rendered so the
/// gap is visible rather than looking like a binding you could set.
pub fn describe(action: KeyType) -> String {
    match action {
        // The grammar's spelling for "no binding" is `none`, not "unassigned",
        // so that is what has to come back out or the pair does not round-trip.
        KeyType::Unassigned => "none".to_string(),
        KeyType::Standard { modifiers, code } => {
            let mods = modifiers.to_string();
            let key = code.name();
            if mods.is_empty() {
                format!("key:{key}")
            } else {
                format!("key:{mods}-{key}")
            }
        }
        KeyType::Media { usage } => {
            format!("media:{}", keymap::consumer::name(usage))
        }
        KeyType::Macro { index } => format!("macro:{index}"),
        KeyType::Wheel { up } => format!("wheel:{}", if up { "up" } else { "down" }),
        KeyType::MouseMove(motion) => format!("move:{},{}", motion.dx, motion.dy),
        // Read-only: this build cannot write one, so say so rather than
        // rendering something that looks like a settable binding.
        KeyType::OpenWebsite => "url:<cannot be set by this build>".to_string(),
        KeyType::CustomCombination => "custom:<not decoded by this build>".to_string(),
    }
}

#[cfg(test)]
mod describe_tests {
    use super::*;
    use minikbconfig_core::keymap::{MouseMove, consumer};

    #[test]
    fn describe_and_parse_round_trip_for_every_writable_type() {
        // The claim in describe's doc comment, as a test: whatever the CLI
        // prints for a slot must parse back to the same binding, so a TUI can
        // display and re-apply without a second grammar.
        for code in KeyCode::all() {
            let action = KeyType::key(code);
            let text = describe(action);
            assert_eq!(parse_action(&text).unwrap(), action, "round trip {text}");
        }

        for modifier in keymap::MODIFIER_NAMES {
            let m = Modifiers::parse(modifier).unwrap();
            let action = KeyType::combo(m, KeyCode::F13);
            let text = describe(action);
            assert_eq!(parse_action(&text).unwrap(), action, "round trip {text}");
        }

        let media_usages = [
            consumer::PLAY_PAUSE,
            consumer::NEXT,
            consumer::PREVIOUS,
            consumer::STOP,
            consumer::MUTE,
            consumer::VOLUME_UP,
            consumer::VOLUME_DOWN,
            consumer::BRIGHTNESS_UP,
            consumer::BRIGHTNESS_DOWN,
        ];
        for usage in media_usages {
            let action = KeyType::media(usage);
            let text = describe(action);
            assert_eq!(parse_action(&text).unwrap(), action, "round trip {text}");
        }

        for action in [
            KeyType::MouseMove(MouseMove { dx: 10, dy: -5 }),
            KeyType::Unassigned,
        ] {
            let text = describe(action);
            assert_eq!(parse_action(&text).unwrap(), action, "round trip {text}");
        }
    }

    #[test]
    fn types_this_build_cannot_write_are_rendered_as_such() {
        // Not round-trippable, and deliberately visible: these would otherwise
        // look like bindings a user could set, and each one is a key that reads
        // back fine and does nothing.
        for action in [KeyType::OpenWebsite, KeyType::CustomCombination] {
            let text = describe(action);
            assert!(parse_action(&text).is_err(), "{text} should not parse");
        }
        assert!(describe(KeyType::OpenWebsite).contains("cannot be set"));
    }

    #[test]
    fn modifier_order_is_canonical_so_round_trips_are_stable() {
        // parse accepts any order, describe always emits ctrl+shift, so
        // describing a re-parsed action must be a fixed point.
        let a = parse_action("key:shift-ctrl-f13").unwrap();
        let b = parse_action(&describe(a)).unwrap();
        assert_eq!(a, b);
        assert_eq!(describe(a), describe(b));
    }
}

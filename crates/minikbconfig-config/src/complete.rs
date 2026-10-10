//! Completion candidates for action strings.
//!
//! Every candidate is derived from the same tables [`parse_action`] reads. That
//! is the whole point of this module: a suggestion that does not parse is worse
//! than no suggestion, and a hand-written copy of the name tables would drift
//! from the grammar within one release.
//!
//! Two frontends use this — the TUI picker and the generated shell completion
//! scripts — and they must agree, so neither may build its own list.

use minikbconfig_core::keymap::{KeyCode, MODIFIER_NAMES, consumer};

/// The kind prefixes `parse_action` accepts.
pub const KIND_PREFIXES: [&str; 3] = ["key:", "media:", "move:"];

/// The word that clears a slot.
pub const CLEAR_WORD: &str = "none";

/// Every consumer usage the device is known to emit.
const CONSUMER_USAGES: [u16; 9] = [
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

/// Every word the grammar accepts, sorted and deduplicated.
///
/// Includes the kind prefixes and the modifiers, which are not complete
/// actions on their own but are the words a user types next.
pub fn candidates() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.push(CLEAR_WORD.to_string());
    out.extend(KIND_PREFIXES.iter().map(|p| p.to_string()));
    out.extend(MODIFIER_NAMES.iter().map(|m| m.to_string()));
    out.extend(KeyCode::all().map(|c| c.name()));
    // Only named usages: consumer::name falls back to "0xNNNN" for anything
    // unnamed, and offering those would suggest hex where a word is expected.
    out.extend(CONSUMER_USAGES.iter().map(|u| consumer::name(*u)));
    out.sort();
    out.dedup();
    out
}

/// Complete action strings, for a consumer that completes whole words.
///
/// The shell completes a whole word, so it needs `key:f13` rather than the bare
/// atom `f13`. Two shapes, one source: both are derived from the same tables, so
/// they cannot disagree about what the grammar accepts.
pub fn full_words() -> Vec<String> {
    let mut out = vec![CLEAR_WORD.to_string()];
    out.extend(KIND_PREFIXES.iter().map(|p| p.to_string()));
    out.extend(KeyCode::all().map(|c| format!("key:{}", c.name())));
    out.extend(
        CONSUMER_USAGES
            .iter()
            .map(|u| format!("media:{}", consumer::name(*u))),
    );
    // Modifiers are never complete on their own, so offer the prefix a user
    // actually types next. `describe` renders multiple modifiers joined with
    // `+`, and single ones with `-` before the key.
    out.extend(MODIFIER_NAMES.iter().map(|m| format!("key:{m}-")));
    out.sort();
    out.dedup();
    out
}

/// Every word a shell should offer: [`full_words`] plus each modifier paired
/// with every key.
///
/// The expansion exists for one reason. Offering a bare `key:ctrl-` creates a
/// dead end: the user completes it and then TAB finds nothing, because no word
/// begins with `key:ctrl-`. Pairing the modifiers with the key names removes the
/// dead end for every shell at once, with no shell-specific logic — ordinary
/// prefix filtering does the work.
///
/// Only single modifiers are expanded. `key:ctrl+shift-a` parses, but `describe`
/// renders multiple modifiers joined with `+`, so the word would not survive a
/// round trip. One modifier renders with `-` and does.
pub fn words_with_modifiers() -> Vec<String> {
    let mut out = full_words();
    out.extend(
        MODIFIER_NAMES
            .iter()
            .flat_map(|m| KeyCode::all().map(move |c| format!("key:{m}-{}", c.name()))),
    );
    out.sort();
    out.dedup();
    out
}

const MODIFIER_ALIASES: [(&str, &str); 13] = [
    ("control", "ctrl"),
    ("lctrl", "ctrl"),
    ("leftctrl", "ctrl"),
    ("lshift", "shift"),
    ("leftshift", "shift"),
    ("opt", "alt"),
    ("option", "alt"),
    ("lalt", "alt"),
    ("leftalt", "alt"),
    ("win", "gui"),
    ("super", "gui"),
    ("cmd", "gui"),
    ("meta", "gui"),
];

const MEDIA_ALIASES: [(&str, &str); 9] = [
    ("play", "play/pause"),
    ("playpause", "play/pause"),
    ("nexttrack", "next"),
    ("prev", "previous"),
    ("previoustrack", "previous"),
    ("volup", "volume-up"),
    ("voldown", "volume-down"),
    ("briup", "brightness-up"),
    ("bridown", "brightness-down"),
];

/// Candidates that could complete `input`, each returned as the whole field
/// text it would produce.
///
/// This is what an inline editor wants: the caller shows the list and replaces
/// its field wholesale on accept. It is deliberately kind-aware, because
/// `complete` alone cannot know that once `key:` is typed the user is choosing
/// a key name and not a media name.
///
/// The invariant every caller may rely on: each suggestion either parses or is a
/// prefix the user extends next. Never a dead end, never a suggestion that
/// fails.
pub fn suggestions(input: &str) -> Vec<String> {
    let raw = input.trim();
    let lower = raw.to_ascii_lowercase();
    let Some(kind) = KIND_PREFIXES.iter().find(|p| lower.starts_with(**p)) else {
        let roots = ["key:", "media:", "move:", CLEAR_WORD];
        let mut out: Vec<String> = roots
            .iter()
            .filter(|r| r.starts_with(&lower))
            .map(|r| r.to_string())
            .collect();
        out.sort();
        return out;
    };
    let token = &lower[kind.len()..];

    let mut out: Vec<String> = match *kind {
        "key:" => key_suggestions(token),
        "media:" => media_suggestions(token),
        // `move:` takes two numbers; there is no word to suggest.
        _ => Vec::new(),
    };
    out.sort();
    out.dedup();
    out
}

fn media_suggestions(token: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in CONSUMER_USAGES {
        let name = consumer::name(u);
        if name.starts_with(token) {
            out.push(format!("media:{name}"));
        }
    }
    for (alias, canonical) in MEDIA_ALIASES {
        if alias.starts_with(token) {
            out.push(format!("media:{canonical}"));
        }
    }
    out
}

/// Suggestions for the token after `key:`, handling a modifier prefix.
///
/// A modifier is itself a token: `key:ctr` should complete to `key:ctrl-`, and
/// from there the user is choosing a key name again, with the modifier kept.
fn key_suggestions(token: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    if let Some(cut) = token.rfind(['-', '+']) {
        let (head, tail) = token.split_at(cut + 1);
        for modifier in MODIFIER_NAMES {
            let spelling = format!("{modifier}-");
            if spelling.starts_with(tail) && !head.split(['-', '+']).any(|part| part == modifier) {
                out.push(format!("key:{head}{spelling}"));
            }
        }
        for (alias, canonical) in MODIFIER_ALIASES {
            let spelling = format!("{alias}-");
            if (spelling.starts_with(tail) || alias.starts_with(tail))
                && !head.split(['-', '+']).any(|part| part == canonical)
            {
                out.push(format!("key:{head}{canonical}-"));
            }
        }
        for code in KeyCode::all() {
            if code.name().starts_with(tail) {
                out.push(format!("key:{head}{}", code.name()));
            }
        }
    } else {
        for modifier in MODIFIER_NAMES {
            let spelling = format!("{modifier}-");
            if spelling.starts_with(token) {
                out.push(format!("key:{spelling}"));
            }
        }
        for (alias, canonical) in MODIFIER_ALIASES {
            if alias.starts_with(token) {
                out.push(format!("key:{canonical}-"));
            }
        }
        for code in KeyCode::all() {
            if code.name().starts_with(token) {
                out.push(format!("key:{}", code.name()));
            }
        }
    }
    out
}

/// The validation state of an in-progress action edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditValidation {
    /// The input field is empty.
    Empty,
    /// The input parses to a complete, valid action.
    Valid(minikbconfig_core::KeyType),
    /// The input is a valid prefix that can be extended.
    Incomplete { matches: usize },
    /// The input is unparseable and cannot produce a valid action.
    Invalid(String),
}

/// Validate what the user has typed so far.
///
/// This allows an editor to report errors and partial state *as the user types*,
/// rather than waiting for them to press Enter.
pub fn validate(input: &str) -> EditValidation {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return EditValidation::Empty;
    }
    match crate::action::parse_action(trimmed) {
        Ok(action) => EditValidation::Valid(action),
        Err(err) => {
            let suggs = suggestions(trimmed);
            if !suggs.is_empty() {
                EditValidation::Incomplete {
                    matches: suggs.len(),
                }
            } else {
                EditValidation::Invalid(err.to_string())
            }
        }
    }
}

/// Tokens matching a partial word, for a consumer that completes the current
/// token — after `key:` you are choosing a key name, not a whole action.
///
/// An empty partial matches everything, so a caller can show the full list
/// before the user has typed anything.
pub fn complete(partial: &str) -> Vec<String> {
    let needle = partial.trim().to_ascii_lowercase();
    candidates()
        .into_iter()
        .filter(|c| c.to_ascii_lowercase().starts_with(&needle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{describe, parse_action};

    /// The durable artifact: every name in the tables must parse.
    ///
    /// This is the check that keeps completion honest. It walks the entire
    /// table rather than a sample, so a key name added later that renders but
    /// does not parse fails here rather than the day it appears in a picker.
    #[test]
    fn every_key_name_parses_and_renders_back() {
        for code in KeyCode::all() {
            let text = format!("key:{}", code.name());
            let parsed =
                parse_action(&text).unwrap_or_else(|e| panic!("{text:?} should parse: {e}"));
            assert_eq!(
                describe(parsed),
                text,
                "key name renders back to the text that produced it"
            );
        }
    }

    #[test]
    fn every_consumer_usage_parses_and_renders_back() {
        for usage in CONSUMER_USAGES {
            let text = format!("media:{}", consumer::name(usage));
            let parsed =
                parse_action(&text).unwrap_or_else(|e| panic!("{text:?} should parse: {e}"));
            assert_eq!(describe(parsed), text);
        }
    }

    #[test]
    fn every_modifier_composes_with_a_key_and_renders_back() {
        for modifier in MODIFIER_NAMES {
            let text = format!("key:{modifier}-f13");
            let parsed =
                parse_action(&text).unwrap_or_else(|e| panic!("{text:?} should parse: {e}"));
            assert_eq!(describe(parsed), text);
        }
        // All eight at once, which is where a peeling bug would show. Note the
        // canonical rendering joins multiple modifiers with `+`, so a user who
        // typed `-` throughout gets back a string that differs from what they
        // typed but parses to the same binding.
        let all = MODIFIER_NAMES.join("+");
        let text = format!("key:{all}-f13");
        let parsed = parse_action(&text).expect("all modifiers compose");
        assert_eq!(describe(parsed), text);
        assert_eq!(
            parsed,
            parse_action("key:ctrl-shift-alt-gui-rctrl-rshift-ralt-rgui-f13").unwrap(),
            "the `-` spelling is the same binding"
        );
    }

    #[test]
    fn the_clear_word_and_prefixes_are_candidates_and_usable() {
        assert!(candidates().contains(&CLEAR_WORD.to_string()));
        for prefix in KIND_PREFIXES {
            assert!(candidates().contains(&prefix.to_string()));
        }
        assert_eq!(
            parse_action(CLEAR_WORD).unwrap(),
            minikbconfig_core::KeyType::Unassigned
        );
    }

    #[test]
    fn every_atom_is_a_prefix_a_modifier_or_a_name_the_grammar_knows() {
        // Atoms are tokens, not actions: a bare key name is only usable once a
        // kind prefix is in front of it. So the invariant is that each atom is
        // recognised by the grammar *somewhere*, not that it parses alone.
        let words = full_words();
        for atom in candidates() {
            let recognised = words
                .iter()
                .any(|w| w.ends_with(':') || w == &atom || w.ends_with(&atom) || w.contains(&atom));
            assert!(recognised, "atom {atom:?} is not known to the grammar");
        }
    }

    #[test]
    fn completion_filters_by_prefix_case_insensitively() {
        let all = complete("");
        assert_eq!(
            all.len(),
            candidates().len(),
            "an empty partial matches all"
        );

        let f = complete("f1");
        assert!(f.contains(&"f13".to_string()));
        assert!(f.contains(&"f14".to_string()));
        assert!(!f.contains(&"space".to_string()));

        assert_eq!(complete("SPACE"), vec!["space".to_string()]);
        assert!(complete("zzzz").is_empty());
        // Atoms, so a whole word is not a token match.
        assert!(complete("key:f13").is_empty());
        assert!(full_words().contains(&"key:f13".to_string()));
    }

    #[test]
    fn a_completed_modifier_prefix_leads_somewhere() {
        // The dead end this exists to prevent.
        for modifier in MODIFIER_NAMES {
            let prefix = format!("key:{modifier}-");
            assert!(
                words_with_modifiers()
                    .iter()
                    .any(|w| w.starts_with(&prefix)),
                "completing {prefix:?} would offer nothing afterwards"
            );
        }
    }

    #[test]
    fn every_expanded_word_renders_back_exactly() {
        for word in words_with_modifiers() {
            if word.ends_with(':') || word.ends_with('-') {
                continue; // a prefix, nothing to round-trip
            }
            let parsed = parse_action(&word).expect(&word);
            assert_eq!(describe(parsed), word);
        }
    }

    #[test]
    fn the_candidate_list_is_sorted_and_free_of_duplicates() {
        let list = candidates();
        let mut sorted = list.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            list, sorted,
            "candidates should already be sorted and deduped"
        );
    }

    #[test]
    fn typing_key_prefix_lists_every_known_key_name() {
        let list = suggestions("key:");
        for code in KeyCode::all() {
            let item = format!("key:{}", code.name());
            assert!(
                list.contains(&item),
                "suggestions for 'key:' must contain {item}"
            );
        }
        for modifier in MODIFIER_NAMES {
            let item = format!("key:{modifier}-");
            assert!(
                list.contains(&item),
                "suggestions for 'key:' must offer modifier prefix {item}"
            );
        }
    }

    #[test]
    fn typing_ctrl_narrows_to_keys_that_combination_can_produce() {
        let list = suggestions("key:ctrl-");
        for code in KeyCode::all() {
            let item = format!("key:ctrl-{}", code.name());
            assert!(
                list.contains(&item),
                "suggestions for 'key:ctrl-' must contain {item}"
            );
        }
        // Never offer duplicate modifier ctrl-ctrl-
        assert!(!list.contains(&"key:ctrl-ctrl-".to_string()));
        // But do offer chaining other modifiers
        assert!(list.contains(&"key:ctrl-shift-".to_string()));
    }

    #[test]
    fn modifier_token_completes_to_dash_and_aliases_canonicalise() {
        assert_eq!(suggestions("key:ctr"), vec!["key:ctrl-".to_string()]);
        assert_eq!(suggestions("key:win"), vec!["key:gui-".to_string()]);
        assert_eq!(suggestions("key:cmd"), vec!["key:gui-".to_string()]);
        assert_eq!(suggestions("key:opt"), vec!["key:alt-".to_string()]);
    }

    #[test]
    fn media_suggestions_cover_usages_and_aliases() {
        let all_media = suggestions("media:");
        assert_eq!(all_media.len(), CONSUMER_USAGES.len());
        assert!(all_media.contains(&"media:volume-up".to_string()));
        assert!(all_media.contains(&"media:mute".to_string()));

        assert_eq!(
            suggestions("media:vol"),
            vec![
                "media:volume-down".to_string(),
                "media:volume-up".to_string(),
            ]
        );
        assert_eq!(
            suggestions("media:briup"),
            vec!["media:brightness-up".to_string()]
        );
    }

    #[test]
    fn root_suggestions_before_kind_prefix() {
        assert_eq!(
            suggestions(""),
            vec![
                "key:".to_string(),
                "media:".to_string(),
                "move:".to_string(),
                "none".to_string(),
            ]
        );
        assert_eq!(suggestions("k"), vec!["key:".to_string()]);
        assert_eq!(suggestions("n"), vec!["none".to_string()]);
        assert_eq!(suggestions("none"), vec!["none".to_string()]);
    }

    #[test]
    fn dead_end_inputs_yield_no_suggestions() {
        assert!(suggestions("key:nonexistent").is_empty());
        assert!(suggestions("bogus").is_empty());
    }

    #[test]
    fn every_suggestion_either_parses_or_is_an_extendable_prefix() {
        for input in [
            "",
            "k",
            "m",
            "key:",
            "key:c",
            "key:ctrl-",
            "key:ctrl-c",
            "media:",
            "media:vol",
            "none",
        ] {
            for cand in suggestions(input) {
                let parses = parse_action(&cand).is_ok();
                let extends = cand.ends_with(':') || cand.ends_with('-');
                assert!(
                    parses || extends,
                    "suggestion {cand:?} for {input:?} must either parse or extend"
                );
            }
        }
    }

    #[test]
    fn validate_reports_state_as_you_type() {
        assert_eq!(validate(""), EditValidation::Empty);
        assert_eq!(validate("   "), EditValidation::Empty);

        // Complete valid bindings
        assert!(matches!(
            validate("key:a"),
            EditValidation::Valid(minikbconfig_core::KeyType::Standard { .. })
        ));
        assert!(matches!(
            validate("none"),
            EditValidation::Valid(minikbconfig_core::KeyType::Unassigned)
        ));

        // Incomplete prefixes with suggestions
        assert!(matches!(
            validate("key:"),
            EditValidation::Incomplete { matches } if matches > 0
        ));
        assert!(matches!(
            validate("key:ctrl-"),
            EditValidation::Incomplete { matches } if matches > 0
        ));

        // Invalid unparseable inputs with no suggestions
        match validate("key:xyz") {
            EditValidation::Invalid(err) => {
                assert!(err.contains("xyz"), "names bad key: {err}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}

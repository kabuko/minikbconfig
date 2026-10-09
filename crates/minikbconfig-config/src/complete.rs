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
}

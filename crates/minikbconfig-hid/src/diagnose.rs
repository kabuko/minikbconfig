//! Explaining why a pad could not be opened.
//!
//! The most likely first-run failure is permissions: `/dev/hidraw*` is root-owned
//! by default, so without the udev rule the pad is present and unusable. The
//! hidapi errno for that is `13`, which tells a user nothing they can act on.
//!
//! This module turns the three distinguishable situations into three specific
//! messages. It deliberately reports what it observed rather than guessing:
//! hidapi enumerates interfaces it cannot open, so "present but forbidden" is a
//! fact we can establish, not an inference.

use std::fmt;

use minikbconfig_core::SUPPORTED;

/// Where the bundled udev rule lives in the repository.
pub const RULE_PATH: &str = "udev/99-minikbconfig.rules";

/// What went wrong, as specifically as the evidence allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnosis {
    /// No supported USB ID was seen at all.
    NotPresent,
    /// Supported pads are attached, but no vendor configuration interface could
    /// be opened.
    Forbidden { interfaces: Vec<String> },
    /// Interfaces were found but none is the vendor configuration interface —
    /// unusual, and often a sign of a stale session rather than permissions.
    NoConfigInterface { interfaces: Vec<String> },
    /// The interface opened but the exchange failed, which usually means the
    /// wrong node was opened or the firmware is mid-reboot.
    ProtocolMismatch { detail: String },
}

impl fmt::Display for Diagnosis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Diagnosis::NotPresent => {
                let wanted: Vec<String> = SUPPORTED
                    .iter()
                    .map(|d| format!("USB {:04x}:{:04x}", d.vendor_id, d.product_id))
                    .collect();
                write!(
                    f,
                    "No supported pad found (looking for {}).\n\n\
                     Check the cable is a data cable and the pad is plugged in, then run:\n    \
                     lsusb | grep -iE '6d7b|0816'\n\n\
                     If the pad is listed there but not found above, unplug it and plug it back \
                     in — a pad attached before the udev rule was installed keeps its old \
                     permissions.",
                    wanted.join(" or ")
                )
            }
            Diagnosis::Forbidden { interfaces } => write!(
                f,
                "Found the pad, but could not open {} (permission denied).\n\n\
                 /dev/hidraw* is root-owned by default. Install the udev rule and replug:\n    \
                 sudo cp {RULE_PATH} /etc/udev/rules.d/\n    \
                 sudo udevadm control --reload-rules && sudo udevadm trigger\n\n\
                 Then log out and back in — group membership is read at login, and this is the \
                 step people miss. Until then you can work around it with sudo.\n\n\
                 Interfaces seen: {}",
                plural(interfaces.len(), "interface"),
                bullet(interfaces)
            ),
            Diagnosis::NoConfigInterface { interfaces } => write!(
                f,
                "Found {} but none is the vendor configuration interface \
                 (usage page 0xFF00).\n\n\
                 This is usually a stale session rather than a permissions problem: a pad \
                 attached before the udev rule was installed exposes its old node. Replug it \
                 and retry.\n\n\
                 Interfaces seen: {}",
                plural(interfaces.len(), "interface"),
                bullet(interfaces)
            ),
            Diagnosis::ProtocolMismatch { detail } => write!(
                f,
                "Opened a pad interface, but the protocol exchange failed: {detail}\n\n\
                 Two pads on one machine can leave the wrong node open. Disconnect the other \
                 pad and retry; if it persists, unplug this one too so its interfaces are \
                 re-enumerated."
            ),
        }
    }
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn bullet(items: &[String]) -> String {
    if items.is_empty() {
        return "(none)".to_string();
    }
    items
        .iter()
        .map(|i| format!("\n      {i}"))
        .collect::<Vec<_>>()
        .concat()
}

/// Work out why no pad is available.
///
/// `enumerated` is every supported interface hidapi listed, whether or not it
/// could be opened; `opened` is how many of those opened successfully. Keeping
/// the two counts apart is what makes "present but forbidden" distinguishable
/// from "not present".
pub fn diagnose(enumerated: &[String], opened: usize, last_error: Option<&str>) -> Diagnosis {
    if enumerated.is_empty() {
        return Diagnosis::NotPresent;
    }
    if opened > 0 {
        // Interfaces work; whatever failed was the conversation, not the door.
        return Diagnosis::ProtocolMismatch {
            detail: last_error.unwrap_or("no detail available").to_string(),
        };
    }
    Diagnosis::Forbidden {
        interfaces: enumerated.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes() -> Vec<String> {
        vec!["/dev/hidraw4 (0816:246f)".to_string()]
    }

    #[test]
    fn nothing_enumerated_is_not_present() {
        assert_eq!(diagnose(&[], 0, None), Diagnosis::NotPresent);
    }

    #[test]
    fn seen_but_none_opened_is_forbidden() {
        // The case that matters: the pad is right there and hidapi lists it, so
        // this is not a cable or a pairing problem.
        assert_eq!(
            diagnose(&nodes(), 0, None),
            Diagnosis::Forbidden {
                interfaces: nodes()
            }
        );
    }

    #[test]
    fn opened_but_failed_later_is_a_protocol_problem_not_permissions() {
        // Distinct advice: telling someone to install the udev rule when the
        // door already opened sends them down the wrong path entirely.
        assert_eq!(
            diagnose(&nodes(), 1, Some("expected 0xAA, got 0x06")),
            Diagnosis::ProtocolMismatch {
                detail: "expected 0xAA, got 0x06".into()
            }
        );
    }

    #[test]
    fn the_permission_message_names_the_rule_and_the_install_command() {
        let text = Diagnosis::Forbidden {
            interfaces: nodes(),
        }
        .to_string();
        assert!(text.contains(RULE_PATH), "names the rule: {text}");
        assert!(text.contains("udevadm"), "gives the reload: {text}");
        assert!(text.contains("log out"), "warns about login: {text}");
        assert!(text.contains("/dev/hidraw4"), "lists what was seen: {text}");
    }

    #[test]
    fn the_missing_pad_message_lists_the_ids_it_looks_for() {
        let text = Diagnosis::NotPresent.to_string();
        assert!(text.contains("6d7b:dcfa"), "{text}");
        assert!(text.contains("0816:246f"), "{text}");
        assert!(text.contains("lsusb"), "{text}");
    }

    #[test]
    fn the_missing_pad_message_does_not_pretend_it_is_permissions() {
        // Nothing was seen, so suggesting the rule would be a guess.
        assert!(!Diagnosis::NotPresent.to_string().contains(RULE_PATH));
    }

    #[test]
    fn no_config_interface_mentions_the_rule_only_as_a_stale_session() {
        // A third case: the pad is openable, so permissions are fine, and saying
        // otherwise sends the user to reinstall a rule they already installed.
        let text = Diagnosis::NoConfigInterface {
            interfaces: nodes(),
        }
        .to_string();
        assert!(text.contains("stale"), "{text}");
        assert!(text.contains("0xFF00"), "{text}");
    }

    #[test]
    fn the_protocol_message_does_not_suggest_permissions_either() {
        let text = Diagnosis::ProtocolMismatch {
            detail: "boom".into(),
        }
        .to_string();
        assert!(!text.contains("udev rule"), "{text}");
        assert!(text.contains("boom"));
    }

    #[test]
    fn grammar_follows_the_count() {
        assert_eq!(plural(1, "interface"), "1 interface");
        assert_eq!(plural(2, "interface"), "2 interfaces");
        assert_eq!(plural(0, "interface"), "0 interfaces");
    }

    #[test]
    fn an_empty_interface_list_still_renders() {
        assert_eq!(bullet(&[]), "(none)");
        assert_eq!(bullet(&["/dev/hidraw0".into()]), "\n      /dev/hidraw0");
    }
}

//! Hardware tests. Skipped unless `MINIKBDEVICE_TEST=1`.
//!
//! Tier one only: does every documented command answer, and does every writable
//! area round-trip what we sent. The things that need eyes - whether the pad is
//! actually lit, whether mode 3 sticks - are not here and cannot be, because the
//! protocol has no lighting read-back.
//!
//! Run with:
//!     MINIKBDEVICE_TEST=1 cargo test -p minikbconfig-hid --test hardware
//!
//! The pad's configuration is captured once and restored afterwards, including
//! when a test panics, so running this does not leave bindings on your pad.

mod support;

use support::pad;

/// Every command the protocol documents, and whether it must answer.
///
/// This is the test that would have caught the URL commands being unimplemented.
/// PROTOCOL.md lists them; nothing checked that the firmware actually responds.
/// A table entry that never answers is a fact, and asserting it is cheaper than
/// finding out by hand.
const MUST_ANSWER: &[(u8, &str)] = &[
    (0x07, "read key layout (bulk)"),
    (0x0A, "get light config"),
    (0x0C, "get macro data"),
];

/// Documented in PROTOCOL.md, and measured to be silent on both models.
///
/// Recorded as *expected* silence rather than omitted, so this doubles as a
/// canary: if a future firmware implements them, the test fails and says to
/// update the docs - which is the moment someone should expose `url:` bindings.
const KNOWN_SILENT: &[(u8, &str)] = &[(0x40, "set URL config"), (0x41, "get URL config")];

#[test]
#[ignore = "needs a real pad; run with MINIKBDEVICE_TEST=1 cargo test -- --ignored"]
fn every_documented_command_answers() {
    let Some(pad) = pad() else { return };
    let mut silent = Vec::new();

    for (cmd, name) in MUST_ANSWER.iter().copied() {
        // A read command with a zero offset and a short length: harmless, and
        // the reply marker is the thing under test.
        let mut report = [0u8; 64];
        report[0] = 0x06;
        report[1] = cmd;
        report[2] = 1;
        match support::exchange(&pad, &report) {
            Ok(reply) => assert_eq!(
                reply.first().copied(),
                Some(0xAA),
                "{name} ({cmd:#04x}) replied without the 0xAA marker"
            ),
            Err(_) => silent.push(format!("{name} ({cmd:#04x})")),
        }
    }

    assert!(
        silent.is_empty(),
        "these documented commands gave no reply at all: {}\n\n\
         Each one is a PROTOCOL.md table entry. Either the entry is wrong or the \
         firmware does not implement it on this model; both are worth recording \
         rather than leaving to be discovered by hand.",
        silent.join(", ")
    );
}

#[test]
#[ignore = "needs a real pad; run with MINIKBDEVICE_TEST=1 cargo test -- --ignored"]
fn the_url_commands_are_still_silent() {
    let Some(pad) = pad() else { return };
    for (cmd, name) in KNOWN_SILENT.iter().copied() {
        let mut report = [0u8; 64];
        report[0] = 0x06;
        report[1] = cmd;
        report[2] = 1;
        assert!(
            support::exchange(&pad, &report).is_err(),
            "{name} ({cmd:#04x}) answered. That is good news - the firmware may now \
             implement it. Update PROTOCOL.md, and consider exposing url: bindings."
        );
    }
}

#[test]
#[ignore = "needs a real pad; run with MINIKBDEVICE_TEST=1 cargo test -- --ignored"]
fn a_binding_round_trips() {
    let Some(pad) = pad() else { return };
    let _guard = support::preserve(&pad).expect("capture the pad's current state");
    let index = 0;

    let original = pad.read_key(index, 0).expect("read before write");
    let probe = minikbconfig_core::KeyType::Standard {
        modifiers: Default::default(),
        code: minikbconfig_core::keymap::KeyCode::F13,
    };

    pad.write_key(index, probe, 0).expect("write");
    assert_eq!(
        pad.read_key(index, 0).expect("read after write"),
        probe,
        "the pad did not store what was written"
    );

    pad.write_key(index, original, 0).expect("restore");
    assert_eq!(pad.read_key(index, 0).unwrap(), original);
}

#[test]
#[ignore = "needs a real pad; run with MINIKBDEVICE_TEST=1 cargo test -- --ignored"]
fn the_light_record_round_trips() {
    let Some(pad) = pad() else { return };
    let _guard = support::preserve(&pad).expect("capture the pad's current state");

    let original = pad.light_config().expect("read light config");

    // A mode inside the documented 1-6 range, since the firmware discards the
    // whole record otherwise - which would fail this test for a reason that has
    // nothing to do with round-tripping.
    let probe = minikbconfig_core::light::LightConfig {
        mode: 2,
        brightness: 3,
        hue: 85,
        saturation: 255,
        value: 255,
        ..original
    };

    let stored = pad.set_light_config(probe).expect("write light config");
    let read_back = pad.light_config().expect("read light config");

    assert_eq!(
        read_back.mode, probe.mode,
        "mode did not round-trip - an out-of-range mode is discarded silently"
    );
    assert_eq!(read_back.hue, probe.hue, "hue did not round-trip");
    assert_eq!(read_back.saturation, probe.saturation);
    assert_eq!(read_back.value, probe.value);

    // The write's own return value is the device's answer to the write; the
    // read-back above is the thing worth trusting, so `stored` is only kept to
    // document that both exist.
    let _ = stored;
}

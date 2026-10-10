//! Shared setup for the hardware tests.
//!
//! Two things matter here. The tests must not run unless asked, because
//! `cargo test` has to stay green on a machine with no pad. And they must put
//! the pad back, including when a test panics, because a hardware test that
//! leaves bindings on your pad is worse than no test at all.

use minikbconfig_hid::Pad;

/// Set this to `1` to run against real hardware.
pub const ENV: &str = "MINIKBDEVICE_TEST";

/// The pad, or `None` when hardware tests are not enabled.
///
/// The tests are also `#[ignore]`d, so `cargo test` reports them as ignored
/// rather than as passing having run nothing. This second gate covers the case
/// where someone runs `cargo test -- --ignored` on a machine with no pad.
pub fn pad() -> Option<Pad> {
    if std::env::var(ENV).as_deref() != Ok("1") {
        eprintln!("skipping: set {ENV}=1 to run against real hardware");
        return None;
    }
    match Pad::open() {
        Ok(pad) => Some(pad),
        Err(err) => {
            // A pad that cannot be opened is a reason to skip, not to fail -
            // these tests are opt-in precisely because most machines have none.
            eprintln!("skipping: no pad available ({err:#})");
            None
        }
    }
}

/// Restores the pad's bindings when dropped, panic or not.
///
/// Holds no pad handle: `Drop` cannot borrow one, so it reopens. That is fine
/// here - the restore is the last thing the process does either way.
pub struct Preserve {
    table: minikbconfig_core::keymap::KeyTable,
}

impl Preserve {
    pub fn capture(pad: &Pad) -> anyhow::Result<Preserve> {
        Ok(Preserve {
            table: pad.read_keys(0)?,
        })
    }
}

impl Drop for Preserve {
    fn drop(&mut self) {
        let Ok(pad) = Pad::open() else {
            eprintln!("warning: could not reopen the pad to restore it");
            return;
        };
        for index in 0..minikbconfig_core::keymap::KEY_SLOTS {
            if let Some(action) = self.table.get(index) {
                if let Err(err) = pad.write_key(index, action, 0) {
                    eprintln!("warning: could not restore key[{index}]: {err:#}");
                }
            }
        }
    }
}

/// Capture the pad's bindings and arrange for them to be restored on drop.
pub fn preserve(pad: &Pad) -> anyhow::Result<Preserve> {
    Preserve::capture(pad)
}

/// Send a pre-built report and wait for the reply.
///
/// Thin wrapper so tests read as protocol rather than as plumbing.
pub fn exchange(pad: &Pad, report: &[u8]) -> anyhow::Result<Vec<u8>> {
    pad.raw_exchange(report, report[1])
}

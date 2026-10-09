//! Interactive editor for SDCX SIDE-KEYBOARD macro pads.
//!
//! A second frontend over the same crates the CLI uses: `minikbconfig-hid` for
//! the device, `minikbconfig-config` for the action grammar, `minikbconfig-core`
//! for the protocol. Nothing here reimplements any of it.
//!
//! Currently the slot list, editing with confirmation, refresh and quit. The
//! live emitted-event pane is tracked separately.

mod app;
mod ui;

use anyhow::{Context, Result, bail};

use app::App;

const USAGE: &str = "\
minikbconfig-tui — interactive editor for SDCX SIDE-KEYBOARD macro pads

USAGE:
    minikbconfig-tui

KEYS:
    up/down, j/k    move between slots
    enter           edit the selected slot
    r               re-read every slot from the pad
    q, esc          quit
    esc             cancel an edit, or give up waiting for a confirmation

Needs a terminal and access to the pad; see udev/99-minikbconfig.rules.
";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // The terminal is restored before we get here, including on the
            // panic path, so this prints on a usable terminal.
            eprintln!("error: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    // Reject stray arguments rather than starting a full-screen app and then
    // ignoring them, which is both surprising and how you get a stack trace
    // where a usage message belongs.
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => bail!("unexpected argument '{other}'\n\n{USAGE}"),
        }
    }
    if args.next().is_some() {
        bail!("this program takes no arguments\n\n{USAGE}");
    }

    // Open the pad before touching the terminal: if it is unreachable the
    // failure is a plain message rather than a full-screen app that then quits.
    let mut app = App::new()?;

    // `try_init` rather than `init`, because `init` panics when there is no
    // terminal — which is exactly what happens over a pipe or a non-interactive
    // SSH session. It also installs a panic hook that restores the terminal, so
    // a crash cannot leave a wrecked shell.
    let mut terminal =
        ratatui::try_init().context("could not initialise the terminal (is stdout a tty?)")?;
    let result = app.run(&mut terminal);
    ratatui::restore();

    result
}

//! Interactive editor for SDCX SIDE-KEYBOARD macro pads.
//!
//! A second frontend over the same crates the CLI uses: `minikbconfig-hid` for
//! the device, `minikbconfig-config` for the action grammar, `minikbconfig-core`
//! for the protocol. Nothing here reimplements any of it.
//!
//! Currently the slot list, refresh and quit. Editing bindings and the live
//! emitted-event pane are tracked separately.

mod app;
mod ui;

use anyhow::Result;

use app::App;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // The terminal is restored by `run` before we get here, including
            // on the panic path, so this prints on a usable terminal.
            eprintln!("error: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut app = App::new()?;

    // `ratatui::run` installs a panic hook that restores the terminal, and
    // restores it on the normal return path too, so a crash cannot leave the
    // user with a wrecked shell.
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();

    result
}

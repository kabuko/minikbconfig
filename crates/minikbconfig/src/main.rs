//! Command-line configurator for SDCX SIDE-KEYBOARD macro pads.
//!
//! A browser-free replacement for the vendor's WebHID app.

use std::io::IsTerminal;
use std::time::Duration;

use anyhow::Context;
use minikbconfig_core::keymap::{self, KeyTable, KeyType};
use minikbconfig_core::light::{self, LightConfig};
use minikbconfig_hid::pad::{self, Pad};

mod shell;

use minikbconfig_config::{ConfigFile, Plan, describe, parse_action};

const USAGE: &str = "\
minikbconfig — configure SDCX SIDE-KEYBOARD macro pads (USB 6d7b:dcfa)

USAGE:
    minikbconfig <COMMAND> [OPTIONS]

COMMANDS:
    info                 Show pad details and live configuration
    dump                 Print the pad's current key map
    keys                 List supported key and media names
    set <INDEX> <ACTION> Bind one key slot, e.g. `set 1 key:ctrl-c`
    led                  Show the current lighting state
    led on|off           Turn the backlight on or off
    led key <N> <#RRGGBB> Set one key's backlight colour
    snapshot <FILE>      Save the current configuration as JSON
    apply <FILE>         Program a configuration saved by `snapshot`
    profile [N]          Show or switch the active profile
    watch [SECONDS]      Show what the pad emits as you press it
                         (--raw shows every report, with hex and interface)
    completions <SHELL>  Print a completion script for bash, zsh or fish

ACTIONS:
    none                    clear the slot
    key:<name>              keystroke, e.g. key:a, key:f13, key:0x6d
    key:<mods>-<name>       with modifiers, e.g. key:ctrl-c, key:ctrl-shift-t
    media:<name>            e.g. media:volume-up, media:mute, media:0xea
    move:<dx>,<dy>          relative cursor motion

LED OPTIONS (for `led on`):
    -b, --brightness <0-4>  Brightness; 0 is off
    -m, --mode <1-6>        Effect mode. Modes outside 1-6 are discarded
    -c, --color <#RRGGBB>   Colour, e.g. --color '#00ff88'
        --speed <N>         Effect speed
        --direction <0-3>   Effect direction

OPTIONS:
    -l, --layer <N>         Layer to read or write (default 0)
    --verify                With `set`: wait for the keypress and check the
                            pad emits the new action. Also spelled -V here,
                            but only with `set`: on its own -V prints the
                            version, so `minikbconfig -V` never reaches it.
    -n, --dry-run           With `apply`: show the diff without writing
    -y, --yes               With `apply`: skip the confirmation prompt
    -h, --help              Show this help
    --version               Show version (also -V or `version`)

EXAMPLES:
    minikbconfig info
    minikbconfig completions bash >> ~/.bashrc
    minikbconfig set 1 key:f13
    minikbconfig led
    minikbconfig led on --color '#ff0000'
    minikbconfig led on --mode 2 --brightness 4 --speed 20
    minikbconfig led off
    minikbconfig snapshot pad.json
    minikbconfig apply pad.json --dry-run
    minikbconfig watch 30

A configuration file is declarative: applying one makes the pad match it
exactly, so slots the file does not mention are cleared. Run with --dry-run
first to see the whole diff; nothing is written until you confirm.
";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        print!("{USAGE}");
        return Ok(());
    }
    if matches!(args[0].as_str(), "-V" | "--version" | "version") {
        println!("minikbconfig {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let command = args[0].clone();
    let rest = &args[1..];
    check_flags(rest)?;
    let layer = parse_layer_flag(rest)?;

    match command.as_str() {
        "info" => cmd_info(layer),
        "dump" => cmd_dump(layer),
        "keys" | "keys-list" => cmd_keys(),
        "set" => cmd_set(rest, layer),
        "led" => match rest.first().map(String::as_str) {
            Some("key") => cmd_led_key(&rest[1..]),
            _ => cmd_led(rest),
        },
        "snapshot" => cmd_snapshot(rest, layer),
        "apply" => cmd_apply(rest),
        "profile" => cmd_profile(rest),
        "watch" => cmd_watch(rest),
        "completions" => cmd_completions(rest),
        other => {
            eprint!("{USAGE}");
            anyhow::bail!("unknown command '{other}'");
        }
    }
}

/// Every flag the CLI understands.
///
/// Checked up front because a mistyped flag is otherwise swallowed silently:
/// `watch --raw15` is not `--raw 15`, and it was quietly treated as no flag at
/// all, giving non-raw output for the wrong duration with nothing to say so.
const FLAGS: &[&str] = &[
    "-h",
    "--help",
    "-V",
    "--version",
    "-l",
    "--layer",
    "--dry-run",
    "-n",
    "--yes",
    "-y",
    "--raw",
    "-R",
    "--verify",
    "-V",
    // `led on` options. Same failure as --verify: parsed in cmd_led, listed in
    // the help text, and never admitted here, so every one of them was refused.
    "--brightness",
    "-b",
    "--mode",
    "-m",
    "--color",
    "--colour",
    "-c",
    "--speed",
    "--direction",
];

/// Reject any flag we do not recognise, rather than ignoring it.
fn check_flags(args: &[String]) -> anyhow::Result<()> {
    for arg in args {
        if !arg.starts_with('-') || arg == "-" {
            continue;
        }
        // Allow --flag=value, and a negative-looking value such as a duration
        // is not a flag we accept either.
        let name = arg.split('=').next().unwrap_or(arg);
        if !FLAGS.contains(&name) {
            anyhow::bail!("unknown option '{arg}'; try --help");
        }
    }
    Ok(())
}

/// Pull `--layer N` / `-l N` out of the argument list.
fn parse_layer_flag(args: &[String]) -> anyhow::Result<u8> {
    let mut layer = 0u8;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "-l" || arg == "--layer" {
            let value = args
                .get(i + 1)
                .context("--layer needs a value, e.g. --layer 0")?;
            layer = value
                .parse()
                .with_context(|| format!("layer '{value}' is not a number"))?;
            i += 2;
        } else if let Some(value) = arg.strip_prefix("--layer=") {
            layer = value
                .parse()
                .with_context(|| format!("layer '{value}' is not a number"))?;
            i += 1;
        } else {
            i += 1;
        }
    }
    Ok(layer)
}

fn cmd_info(layer: u8) -> anyhow::Result<()> {
    let pad = Pad::open()?;
    let info = pad.device_info()?;

    println!("{}", pad::describe(&pad));
    println!(
        "  firmware {}.{:03}   protocol {}.{:03}   battery {}%{}",
        info.firmware_version >> 8,
        info.firmware_version & 0xFF,
        info.protocol_version >> 8,
        info.protocol_version & 0xFF,
        info.battery_percent,
        if info.charge_state != 0 {
            " (charging)"
        } else {
            ""
        }
    );
    println!(
        "  profiles {} (current {})   layers reported {} (current {}, device counts from 1)",
        info.profile_count, info.current_profile, info.layer_count, info.current_layer
    );
    println!("\n  reading layer {layer}:\n");

    let table = pad.read_keys(layer)?;
    print_table(&table);
    Ok(())
}

fn cmd_dump(layer: u8) -> anyhow::Result<()> {
    let pad = Pad::open()?;
    let table = pad.read_keys(layer)?;
    print_table(&table);
    Ok(())
}

fn print_table(table: &KeyTable) {
    for index in 0..keymap::KEY_SLOTS {
        let Some(action) = table.get(index) else {
            continue;
        };
        if action == KeyType::Unassigned {
            continue;
        }
        println!("  key[{index:2}]  {}", describe(action));
    }
    // Slots this build cannot name are reported rather than hidden: dropping
    // them silently would make the dump look emptier than the pad really is.
    for (index, ty) in &table.unrecognised {
        println!("  key[{index:2}]  <unrecognised type 0x{ty:02x}>");
    }
}

fn cmd_keys() -> anyhow::Result<()> {
    println!("Key names (HID usage IDs):");
    let codes: Vec<keymap::KeyCode> = keymap::KeyCode::all().collect();
    for (i, code) in codes.iter().enumerate() {
        if i % 6 == 0 {
            print!("  ");
        }
        print!("{:<14}", code.name());
        if i % 6 == 5 {
            println!();
        }
    }
    if codes.len() % 6 != 0 {
        println!();
    }

    println!("\nMedia names (Consumer page usages):");
    for (usage, name) in [
        (keymap::consumer::PLAY_PAUSE, "play/pause"),
        (keymap::consumer::NEXT, "next"),
        (keymap::consumer::PREVIOUS, "previous"),
        (keymap::consumer::STOP, "stop"),
        (keymap::consumer::MUTE, "mute"),
        (keymap::consumer::VOLUME_UP, "volume-up"),
        (keymap::consumer::VOLUME_DOWN, "volume-down"),
        (keymap::consumer::BRIGHTNESS_UP, "brightness-up"),
        (keymap::consumer::BRIGHTNESS_DOWN, "brightness-down"),
    ] {
        println!("  {name:<16} 0x{usage:04x}");
    }

    println!("\nModifiers: ctrl shift alt gui rctrl rshift ralt rgui");
    println!("Mouse buttons: left right middle");
    Ok(())
}

/// Wait for the user to press a control and check the pad emits what was written.
///
/// The read-back above only proves the device stored the record. This is the
/// step that proves it changed behaviour, and it is the one the CLI used to
/// leave entirely to the user -- while telling them to do it.
fn verify_by_press(index: u8, action: KeyType) -> anyhow::Result<()> {
    use minikbconfig_core::emitted::Match;
    use minikbconfig_hid::pad::Watcher;

    if !minikbconfig_core::emitted::Emitted::is_observable_for(action) {
        println!(
            "\n  Nothing observable can confirm {}; lighting and macro bindings \
             emit nothing comparable.",
            describe(action)
        );
        return Ok(());
    }

    println!("\n  Press the control (10s)…");
    let watcher = Watcher::open().context("could not open the input interfaces")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("nothing pressed; the write is stored but unconfirmed");
        }
        match watcher.next_press(remaining.min(std::time::Duration::from_millis(200)))? {
            Some(event) => {
                println!("  emitted {}", event.describe());
                return match event.compare(action) {
                    Match::Exact => {
                        println!("  confirmed: the pad emits {}", describe(action));
                        Ok(())
                    }
                    Match::ExtraKeys(extra) => {
                        let names: Vec<String> = extra
                            .iter()
                            .map(|c| minikbconfig_core::keymap::KeyCode(*c).name())
                            .collect();
                        println!(
                            "  warning: confirmed, but the pad also emitted {} -- \
                             that looks like a key it never released; unplug it to clear",
                            names.join("+")
                        );
                        Ok(())
                    }
                    Match::Mismatch => {
                        anyhow::bail!("key[{index}] was stored but the pad emitted something else")
                    }
                };
            }
            None => continue,
        }
    }
}

fn cmd_completions(rest: &[String]) -> anyhow::Result<()> {
    let shell = rest.first().map(String::as_str).unwrap_or("");
    let Some(script) = shell::script_for(shell) else {
        anyhow::bail!(
            "unknown shell '{shell}'; expected one of {}",
            shell::SHELLS.join(", ")
        );
    };
    print!("{script}");
    Ok(())
}

fn cmd_set(args: &[String], layer: u8) -> anyhow::Result<()> {
    let verify = args.iter().any(|a| a == "--verify" || a == "-V");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let index: u8 = positional
        .first()
        .copied()
        .context("usage: minikbconfig set <INDEX> <ACTION>")?
        .parse()
        .context("key index must be a number, e.g. 1 or 16")?;
    let action_text = positional
        .get(1)
        .copied()
        .context("usage: minikbconfig set <INDEX> <ACTION>")?;

    let action = parse_action(action_text)
        .with_context(|| format!("could not parse action '{action_text}'"))?;

    let pad = Pad::open()?;

    // Read before writing so a wrong index is caught before anything changes.
    let before = pad.read_key(index, layer)?;

    println!("  key[{index:2}] layer {layer}");
    println!("    before: {}", describe(before));
    println!("    after : {}", describe(action));

    pad.write_key(index, action, layer)
        .with_context(|| format!("could not program key {index}"))?;

    // Confirm the device stored it. Necessary, and not sufficient: several
    // wrong encodings read back perfectly while emitting nothing.
    let stored = pad.read_key(index, layer)?;
    if stored == action {
        println!("    stored: ok");
        if verify {
            verify_by_press(index, action)?;
        } else {
            println!("\n  Pass --verify to have this wait for the keypress and check it.");
        }
    } else {
        println!(
            "    stored: MISMATCH — device returned {}",
            describe(stored)
        );
        anyhow::bail!("write did not take effect; the device kept its previous value");
    }
    Ok(())
}

/// Show or change the pad's lighting.
///
/// With no arguments this reports the current state. Otherwise it patches the
/// fields named by flags and writes the whole record back, which is what the
/// vendor's own `setLightEffectConfig` does: read, patch, write.
fn cmd_led(args: &[String]) -> anyhow::Result<()> {
    let pad = Pad::open()?;
    let mut config = pad.light_config()?;
    let before = config;

    let mut brightness = None;
    let mut mode = None;
    let mut colour = None;
    let mut speed = None;
    let mut direction = None;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        // Each branch consumes its own arity: flags that take a value consume
        // two, bare words like `on` consume one.
        let (name, arity) = match arg {
            "on" => {
                brightness = Some(3);
                ("on", 1)
            }
            "off" => {
                brightness = Some(0);
                config.value = 0;
                ("off", 1)
            }
            "--brightness" | "-b" => ("--brightness", 2),
            "--mode" | "-m" => ("--mode", 2),
            "--color" | "--colour" | "-c" => ("--color", 2),
            "--speed" => ("--speed", 2),
            "--direction" => ("--direction", 2),
            other => anyhow::bail!("unknown led option '{other}'"),
        };
        let value = |i: usize| -> anyhow::Result<String> {
            args.get(i + 1)
                .cloned()
                .with_context(|| format!("{name} needs a value"))
        };
        if arity == 2 {
            match name {
                "--brightness" => {
                    brightness = Some(value(i)?.parse().context("brightness must be 0-4")?)
                }
                "--mode" => mode = Some(value(i)?.parse().context("mode must be a number")?),
                "--color" => colour = Some(value(i)?),
                "--speed" => speed = Some(value(i)?.parse().context("speed must be a number")?),
                "--direction" => {
                    direction = Some(value(i)?.parse().context("direction must be 0-3")?)
                }
                _ => unreachable!(),
            }
        }
        i += arity;
    }

    let changed = brightness.is_some()
        || mode.is_some()
        || colour.is_some()
        || speed.is_some()
        || direction.is_some();

    if !changed {
        print_light(&before);
        return Ok(());
    }

    if let Some(b) = brightness {
        if b > 4 {
            anyhow::bail!("brightness must be 0-4 (got {b})");
        }
        config.brightness = b;
        // Turning on needs a colour to show: a black one has value 0, and a
        // valid mode, because the firmware drops the record without one.
        if b > 0 && colour.is_none() && config.value == 0 {
            let [h, s, v] = light::hex_to_hsv("#ffffff")?;
            config.hue = h;
            config.saturation = s;
            config.value = v;
        }
    }
    if let Some(m) = mode {
        // Refuse rather than substitute: silently turning mode 9 into mode 1
        // would write something the user did not ask for.
        if !(light::MIN_MODE..=light::MAX_MODE).contains(&m) {
            anyhow::bail!(
                "mode {m} is not one the firmware accepts ({}-{}); \
                 the device discards the whole record for any other value",
                light::MIN_MODE,
                light::MAX_MODE
            );
        }
        config.mode = m;
    }
    // The record is only stored for modes 1-6, so a pad whose stored mode is 0
    // or unknown needs one before any write will land. Only pick one when the
    // caller did not ask for a specific mode.
    if !config.has_valid_mode() {
        config.mode = LightConfig::default_mode();
    }
    if config.mode == 0 {
        config.cycle = 0; // the vendor clears `color` when mode is 0
    }
    if config.is_lit() {
        config.cycle = 1;
    }
    if let Some(hex) = &colour {
        let [h, s, v] = light::hex_to_hsv(hex)?;
        config.hue = h;
        config.saturation = s;
        config.value = v;
        config.cycle = 1;
    }
    if let Some(s) = speed {
        config.speed = s;
    }
    if let Some(d) = direction {
        config.direction = d;
    }

    print_light(&before);
    if config == before {
        println!("  no changes");
        return Ok(());
    }
    println!("  ->");
    print_light(&config);

    let stored = pad.set_light_config(config)?;
    println!(
        "\n  stored: ok (brightness {}, value {})",
        stored.brightness, stored.value
    );
    if !stored.is_lit() && config.is_lit() {
        println!("  note: the pad reports this as dark — check the LEDs are present");
    }
    Ok(())
}

fn print_light(config: &LightConfig) {
    println!(
        "  type {}  single-colour index {}",
        config.light_type, config.single_color_index
    );
    println!(
        "  brightness {}  mode {}  speed {}  direction {}  cycle {}",
        config.brightness, config.mode, config.speed, config.direction, config.cycle
    );
    println!(
        "  hue {}  saturation {}  value {}  -> {}",
        config.hue,
        config.saturation,
        config.value,
        if config.is_lit() { "lit" } else { "dark" }
    );
}

/// Set a single key's backlight colour.
fn cmd_led_key(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let index: u8 = positional
        .first()
        .copied()
        .context("usage: minikbconfig led key <INDEX> <#RRGGBB>")?
        .parse()
        .context("key index must be a number")?;
    let hex = positional
        .get(1)
        .copied()
        .context("usage: minikbconfig led key <INDEX> <#RRGGBB>")?;
    let rgb = parse_rgb(hex)?;

    let pad = Pad::open()?;
    pad.set_key_color(index, rgb)?;
    println!("  key[{index:2}] colour set to {hex}");
    Ok(())
}

fn parse_rgb(hex: &str) -> anyhow::Result<[u8; 3]> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("'{hex}' is not a #RRGGBB colour");
    }
    Ok([
        u8::from_str_radix(&s[0..2], 16).unwrap(),
        u8::from_str_radix(&s[2..4], 16).unwrap(),
        u8::from_str_radix(&s[4..6], 16).unwrap(),
    ])
}

fn cmd_snapshot(args: &[String], layer: u8) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let path = positional
        .first()
        .copied()
        .context("usage: minikbconfig snapshot <FILE>")?;

    // --dry-run belongs to `apply`. Honouring it here would mean writing the
    // snapshot anyway, which is the worst reading of "dry": the user asked for
    // no side effect and the file they were protecting gets overwritten.
    if args.iter().any(|a| a == "--dry-run" || a == "-n") {
        anyhow::bail!(
            "--dry-run applies to `apply`, not `snapshot`. \
             A snapshot's only effect is writing the file, so there is nothing to dry-run about; \
             omit the flag to write {path}."
        );
    }

    let pad = Pad::open()?;
    let table = pad.read_keys(layer)?;

    let info = pad.device_info()?;
    let mut file = ConfigFile::from_table_in_profile(&table, layer, Some(info.current_profile));
    file.device = Some(minikbconfig_config::Device {
        vendor_id: pad.vendor_id(),
        product_id: pad.product_id(),
        name: pad.product_name().to_string(),
    });

    std::fs::write(path, file.to_json()?).with_context(|| format!("could not write {path}"))?;
    println!(
        "wrote {path} ({} keys on layer {layer}, profile {})",
        file.keys.len(),
        file.profile.unwrap_or(0)
    );
    Ok(())
}

/// Show the active profile, or switch to another.
///
/// Profiles hold independent key tables, so the profile in force decides which
/// table `dump`, `set` and `apply` read and write.
fn cmd_profile(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let pad = Pad::open()?;
    let info = pad.device_info()?;

    let Some(arg) = positional.first() else {
        println!(
            "  profile {} of {} (the device counts from 0)",
            info.current_profile, info.profile_count
        );
        let table = pad.read_keys(0)?;
        let assigned = (0..keymap::KEY_SLOTS)
            .filter(|i| table.get(*i).is_some_and(|a| a != KeyType::Unassigned))
            .count();
        println!("  {assigned} key(s) bound on layer 0 of this profile");
        return Ok(());
    };

    let target: u8 = arg
        .parse()
        .with_context(|| format!("profile '{arg}' is not a number"))?;
    if info.profile_count > 0 && target >= info.profile_count {
        anyhow::bail!(
            "profile {target} does not exist; this pad has {} (0-{})",
            info.profile_count,
            info.profile_count - 1
        );
    }

    pad.set_profile(target)?;
    // Read the device back rather than trusting the write.
    let after = pad.device_info()?;
    if after.current_profile != target {
        anyhow::bail!(
            "could not switch to profile {target}: device still reports {}",
            after.current_profile
        );
    }
    println!("  now on profile {}", after.current_profile);
    Ok(())
}

/// Compare a file against the pad, show the difference, then apply it.
fn cmd_apply(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let path = positional
        .first()
        .copied()
        .context("usage: minikbconfig apply <FILE> [--dry-run] [--yes]")?;
    let dry_run = args.iter().any(|a| a == "--dry-run" || a == "-n");
    let assume_yes = args.iter().any(|a| a == "--yes" || a == "-y");

    let file = ConfigFile::load(std::path::Path::new(path))?;
    let pad = Pad::open()?;

    if let Some(device) = &file.device {
        if device.product_id != pad.product_id() || device.vendor_id != pad.vendor_id() {
            println!(
                "  warning: {} was captured from {:04x}:{:04x}, this pad is {:04x}:{:04x}",
                path,
                device.vendor_id,
                device.product_id,
                pad.vendor_id(),
                pad.product_id()
            );
        }
    }

    // The file's layer wins over --layer: applying a file to a different layer
    // than the one it describes would be surprising.
    let info = pad.device_info()?;
    if let Some(want) = file.profile {
        if want != info.current_profile {
            println!(
                "  warning: {} was captured on profile {want}, the pad is on profile {}; \
                 switch with `minikbconfig profile {want}` or apply anyway",
                path, info.current_profile
            );
        }
    }
    let layer = file.layer;
    let current = pad.read_keys(layer)?;
    let plan = Plan::between(&file, &current)?;

    println!("  applying {path} to layer {layer}:");
    if plan.is_empty() {
        println!("    no changes");
        return Ok(());
    }

    // Every change is shown before any is written, including the slots the file
    // omits: a config file is declarative, so those are cleared too.
    for change in &plan.changes {
        println!(
            "    key[{:2}]  {} -> {}",
            change.index,
            describe(change.before),
            describe(change.after)
        );
    }

    if dry_run {
        println!(
            "\n  {} change(s); not applied (--dry-run)",
            plan.changes.len()
        );
        return Ok(());
    }
    if !assume_yes && !confirm(&format!("apply {} change(s)?", plan.changes.len()))? {
        println!("  aborted; nothing was written");
        return Ok(());
    }

    // Single-key writes only. The bulk write command stores a short payload
    // without applying it, which is the trap recorded in PROTOCOL.md §3.
    let mut applied = 0usize;
    for change in &plan.changes {
        pad.write_key(change.index, change.after, layer)
            .with_context(|| format!("could not program key {}", change.index))?;

        // Read back: proves the device stored what we asked for.
        let stored = pad.read_key(change.index, layer)?;
        if stored != change.after {
            anyhow::bail!(
                "key[{}] did not take effect: wanted {}, device returned {}",
                change.index,
                describe(change.after),
                describe(stored)
            );
        }
        applied += 1;
    }

    println!("\n  applied and verified {applied} change(s)");
    Ok(())
}

/// Ask for confirmation, refusing to proceed when stdin is not a terminal.
fn confirm(question: &str) -> anyhow::Result<bool> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        anyhow::bail!("refusing to apply without confirmation; pass --yes to proceed");
    }
    print!("  {question} [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut answer = String::new();
    std::io::BufRead::read_line(&mut stdin.lock(), &mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn cmd_watch(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let raw = args.iter().any(|a| a == "--raw" || a == "-R");
    let seconds: u64 = positional
        .first()
        .copied()
        .map(|s| s.parse())
        .transpose()
        .context("watch duration must be a number of seconds")?
        .unwrap_or(30);

    // The pad must be present for its input interfaces to be openable.
    let _pad = Pad::open()?;
    let watcher = pad::Watcher::open()?;

    if raw {
        println!("listening for {seconds}s — press keys or turn the knob (raw reports)");
    } else {
        println!("listening for {seconds}s — press keys or turn the knob");
    }
    let mut seen = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let slice = remaining.min(Duration::from_millis(250));
        if raw {
            match watcher.next_report(slice)? {
                Some(report) => {
                    println!("  {}", report.describe());
                    seen += 1;
                }
                None => continue,
            }
        } else {
            match watcher.next_press(slice)? {
                Some(event) => {
                    println!("  {}", event.describe());
                    seen += 1;
                }
                None => continue,
            }
        }
    }
    if raw {
        println!("{seen} report(s)");
    } else {
        println!("{seen} event(s)");
    }
    Ok(())
}

#[cfg(test)]
mod flag_tests {
    use super::*;

    #[test]
    fn a_mistyped_flag_is_rejected_rather_than_ignored() {
        // Found by using it: `watch --raw15` is not `--raw 15`, and it used to
        // be swallowed, silently giving non-raw output for the wrong duration.
        assert!(check_flags(&["--raw".into(), "15".into()]).is_ok());
        assert!(check_flags(&["--raw15".into()]).is_err());
        assert!(check_flags(&["--dry-run".into()]).is_ok());
        assert!(check_flags(&["-y".into()]).is_ok());
        assert!(check_flags(&["--layer=2".into()]).is_ok());
        // Positional arguments are not flags, and neither is a lone dash.
        assert!(check_flags(&["pad.json".into(), "0".into()]).is_ok());
        assert!(check_flags(&["-".into()]).is_ok());
        assert!(check_flags(&["--nope".into()]).is_err());
        // --verify is the flag this test exists for. It was implemented,
        // documented and tested in isolation, and rejected by check_flags on
        // every use, because the implementation reading a flag and the
        // allowlist admitting it are two separate lists.
        assert!(check_flags(&["--verify".into()]).is_ok());
        assert!(check_flags(&["-V".into()]).is_ok());
    }

    #[test]
    fn snapshot_refuses_dry_run_rather_than_writing_anyway() {
        // Found the hard way: `snapshot <file> --dry-run` was run against a
        // committed snapshot during development and overwrote it, because the
        // flag was accepted by check_flags and then ignored here.
        assert!(cmd_snapshot(&["pad.json".into(), "--dry-run".into()], 0).is_err());
    }

    #[test]
    fn every_flag_in_the_help_text_is_accepted() {
        // The generic form of the bug above: a flag documented in USAGE but
        // missing from FLAGS fails at runtime, for anyone, forever.
        for line in USAGE.lines() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('-') {
                continue;
            }
            for token in trimmed.split_whitespace() {
                if !token.starts_with("--") {
                    continue; // short forms are grouped with the long one
                }
                let name = token.trim_end_matches(',');
                assert!(
                    FLAGS.contains(&name),
                    "help documents {name} but check_flags would reject it"
                );
            }
        }
    }

    #[test]
    fn every_flag_check_accepts_is_documented() {
        // The other direction: an allowlisted flag nobody can discover.
        for flag in FLAGS {
            if USAGE.contains(flag) {
                continue;
            }
            // Short forms may be documented only in the summary line above
            // them, so accept an undocumented short form only if the long one
            // it pairs with is present.
            assert!(
                USAGE.contains(flag.trim_start_matches('-')),
                "{flag} is accepted but never mentioned in help"
            );
        }
    }
}

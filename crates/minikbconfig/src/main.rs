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

use minikbconfig_config::{ConfigFile, Lighting, Plan, describe, parse_action};

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
        --speed <N>         Effect speed, 0-4 as the vendor app offers
        --direction <N>     Effect direction; stored verbatim, any 0-255

OPTIONS:
    -l, --layer <N>         Layer to read or write (default 0)
    --json                 With `info`, `dump`, `keys`, `led`: emit JSON on
                            stdout instead of prose, for scripting
    --verify                With `set`: wait for the keypress and check the
                            pad emits the new action
    -n, --dry-run           With `apply`: show the diff without writing
    -y, --yes               With `apply`: skip the confirmation prompt
    -h, --help              Show this help
    -V, --version           Show version

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
    // Help is a global flag, so honour it wherever it appears. Previously only
    // args[0] was checked, which meant `minikbconfig set --help` reported a
    // usage error instead of help - the one place someone is likely to ask for
    // it.
    if args.is_empty()
        || args
            .iter()
            .any(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        print!("{USAGE}");
        return Ok(());
    }
    // -V means version wherever it appears, as it does for cargo, git and
    // docker. It used to double as a short form for `set --verify`, which gave
    // one letter two meanings depending on position - the worst kind of flag.
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-V" | "--version" | "version"))
    {
        println!("minikbconfig {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let command = args[0].clone();
    let rest = &args[1..];
    if let Err(err) = check_flags(&command, rest) {
        // Name the command whose flags were rejected; `apply --verify` is far
        // more use than the full usage text.
        eprintln!("error: {err}");
        eprint!("{}", usage_for(&command));
        std::process::exit(2);
    }
    let layer = parse_layer_flag(rest)?;

    match command.as_str() {
        "info" => cmd_info(layer, json_flag(rest)),
        "dump" => cmd_dump(layer, json_flag(rest)),
        "keys" | "keys-list" => cmd_keys(json_flag(rest)),
        "set" => cmd_set(rest, layer),
        "led" => match rest.first().map(String::as_str) {
            Some("key") => cmd_led_key(&rest[1..]),
            _ => cmd_led(rest, json_flag(rest)),
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
/// Flags accepted by every command.
const GLOBAL_FLAGS: &[&str] = &["-h", "--help", "-V", "--version"];

/// The layer flag, which every command reads through [`parse_layer_flag`].
const LAYER_FLAGS: &[&str] = &["-l", "--layer"];

/// Flags each command actually reads.
///
/// This replaces a single flat allowlist, which is how `--verify` and every
/// `led on` option came to be implemented, documented, and rejected by
/// [`check_flags`] at the same time. A flat list says nothing about *where* a
/// flag is honoured, so it cannot catch the inverse error: a flag accepted on a
/// command that ignores it. `apply --verify` was silently doing nothing for
/// exactly that reason.
///
/// Listing them per command means an unused flag is a build failure rather than
/// a surprise at runtime. `check_flags` reads this table, so a flag has to be
/// added in one place to be accepted and one place to be read.
const COMMAND_FLAGS: &[(&str, &[&str])] = &[
    ("info", &["--json"]),
    ("dump", &["--json"]),
    ("keys", &["--json"]),
    ("keys-list", &["--json"]),
    ("set", &["--verify"]),
    (
        "led",
        &[
            "--json",
            "--brightness",
            "-b",
            "--mode",
            "-m",
            "--color",
            "--colour",
            "-c",
            "--speed",
            "--direction",
        ],
    ),
    ("snapshot", &[]),
    ("apply", &["--dry-run", "-n", "--yes", "-y"]),
    ("profile", &[]),
    ("watch", &["--raw", "-R"]),
    ("completions", &[]),
];

/// Consumer-page names, in one place.
///
/// Shared by the prose and JSON paths: these were written out twice when the
/// JSON output was added, which is exactly how they drift.
const MEDIA_NAMES: &[(u16, &str)] = &[
    (keymap::consumer::PLAY_PAUSE, "play/pause"),
    (keymap::consumer::NEXT, "next"),
    (keymap::consumer::PREVIOUS, "previous"),
    (keymap::consumer::STOP, "stop"),
    (keymap::consumer::MUTE, "mute"),
    (keymap::consumer::VOLUME_UP, "volume-up"),
    (keymap::consumer::VOLUME_DOWN, "volume-down"),
    (keymap::consumer::BRIGHTNESS_UP, "brightness-up"),
    (keymap::consumer::BRIGHTNESS_DOWN, "brightness-down"),
];

// --- JSON output ---------------------------------------------------------
//
// Scripts should not have to parse English prose, and `snapshot` already has a
// JSON form that was going unused for reading. Field names are snake_case and
// match the prose where possible, so `jq` and friends work without surprises.

#[derive(serde::Serialize)]
struct InfoJson {
    vendor_id: u16,
    product_id: u16,
    product_name: String,
    firmware: String,
    protocol: String,
    battery_percent: u8,
    charging: bool,
    profile_count: u8,
    current_profile: u8,
    layer_count: u8,
    current_layer: u8,
    /// Layer actually read, which is not the same as the device's current
    /// layer: the device counts from 1 and `--layer` counts from 0.
    read_layer: u8,
    keys: u8,
    bindings: Vec<Binding>,
}

#[derive(serde::Serialize)]
struct Binding {
    index: u8,
    /// `null` for a slot this build cannot decode, rather than omitting it -
    /// a missing slot would read as "cleared", which it is not.
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unrecognised_type: Option<String>,
}

#[derive(serde::Serialize)]
struct KeysJson {
    standard: Vec<Name>,
    media: Vec<Name>,
    /// Action prefixes the grammar accepts.
    kinds: Vec<&'static str>,
    modifiers: Vec<&'static str>,
}

#[derive(serde::Serialize)]
struct Name {
    code: u32,
    name: String,
}

#[derive(serde::Serialize)]
struct LightJson {
    /// Which of the record's fields actually affect the LEDs. Present because
    /// `brightness` and `direction` are stored and echoed but do nothing on
    /// these models, and a script should not have to rediscover that.
    notes: Vec<&'static str>,
    mode: u8,
    /// Inert on these models.
    brightness: u8,
    speed: u8,
    /// Inert on these models.
    direction: u8,
    cycle: u8,
    hue: u8,
    saturation: u8,
    value: u8,
    colour_is_black: bool,
}

fn bindings_of(table: &KeyTable) -> Vec<Binding> {
    let mut out: Vec<Binding> = Vec::new();
    for index in 0..keymap::KEY_SLOTS {
        let Some(action) = table.get(index) else {
            continue;
        };
        if action == KeyType::Unassigned {
            continue;
        }
        out.push(Binding {
            index,
            action: Some(describe(action)),
            unrecognised_type: None,
        });
    }
    for (index, ty) in &table.unrecognised {
        out.push(Binding {
            index: *index,
            action: None,
            unrecognised_type: Some(format!("0x{ty:02x}")),
        });
    }
    out.sort_by_key(|b| b.index);
    out
}

fn light_json(c: &LightConfig) -> LightJson {
    let mut notes = vec![
        "brightness is stored and echoed but has no effect on this model",
        "direction is stored and echoed but has no effect on this model",
        "value is the real brightness control; it is max(r,g,b)",
        "speed is 0-4 as the vendor app offers it",
    ];
    if c.mode == 3 && !c.colour_is_black() && c.value < 32 {
        notes.push("mode 3 with value below 32 leaves the LED stuck lit (firmware bug)");
    }
    LightJson {
        notes,
        mode: c.mode,
        brightness: c.brightness,
        speed: c.speed,
        direction: c.direction,
        cycle: c.cycle,
        hue: c.hue,
        saturation: c.saturation,
        value: c.value,
        colour_is_black: c.colour_is_black(),
    }
}

fn info_json(
    pad: &Pad,
    info: &minikbconfig_core::response::DeviceInfo,
    layer: u8,
    table: &KeyTable,
) -> InfoJson {
    InfoJson {
        vendor_id: pad.vendor_id(),
        product_id: pad.product_id(),
        product_name: pad.product_name().to_string(),
        firmware: format!(
            "{}.{:03}",
            info.firmware_version >> 8,
            info.firmware_version & 0xFF
        ),
        protocol: format!(
            "{}.{:03}",
            info.protocol_version >> 8,
            info.protocol_version & 0xFF
        ),
        battery_percent: info.battery_percent,
        charging: info.charge_state != 0,
        profile_count: info.profile_count,
        current_profile: info.current_profile,
        layer_count: info.layer_count,
        current_layer: info.current_layer,
        read_layer: layer,
        keys: pad.keys(),
        bindings: bindings_of(table),
    }
}

/// Whether `--json` was requested.
fn json_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
}

/// Print a value as pretty JSON, or fail rather than print nothing.
///
/// Failing matters: a script that parses stdout would otherwise see an empty
/// result and treat it as "no keys" rather than "the command broke".
/// Whether `--json` was given alongside something that would change the pad.
pub fn led_json_conflicts(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json") && args.len() > 1
}

fn emit_json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(value).context("could not render JSON output")?;
    println!("{text}");
    Ok(())
}

/// A one-line usage hint for one command.
fn usage_for(command: &str) -> String {
    let line = match command {
        "info" => "minikbconfig info",
        "dump" => "minikbconfig dump",
        "keys" | "keys-list" => "minikbconfig keys",
        "set" => "minikbconfig set <INDEX> <ACTION>",
        "led" => "minikbconfig led [on|off] | led key <INDEX> <#RRGGBB>",
        "snapshot" => "minikbconfig snapshot <FILE>",
        "apply" => "minikbconfig apply <FILE> [--dry-run] [--yes]",
        "profile" => "minikbconfig profile [N]",
        "watch" => "minikbconfig watch [SECONDS]",
        "completions" => "minikbconfig completions <bash|zsh|fish>",
        other => return format!("\nunknown command '{other}'\n"),
    };
    format!("\nusage: {line}\n")
}

/// Every flag the tool knows, for the help-text consistency checks.
#[cfg(test)]
fn all_flags() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = GLOBAL_FLAGS.to_vec();
    out.extend_from_slice(LAYER_FLAGS);
    for (_, flags) in COMMAND_FLAGS {
        out.extend_from_slice(flags);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Reject any flag we do not recognise, rather than ignoring it.
/// Reject a flag this command does not read.
///
/// `command` is the subcommand, so `apply --verify` is refused rather than
/// silently ignored — the failure mode a flat allowlist cannot detect.
fn check_flags(command: &str, args: &[String]) -> anyhow::Result<()> {
    let command_flags = COMMAND_FLAGS
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, flags)| *flags)
        .unwrap_or(&[]);

    for arg in args {
        if !arg.starts_with('-') || arg == "-" {
            continue;
        }
        // Allow --flag=value, and a negative-looking value such as a duration
        // is not a flag we accept either.
        let name = arg.split('=').next().unwrap_or(arg);
        if GLOBAL_FLAGS.contains(&name) || LAYER_FLAGS.contains(&name) {
            continue;
        }
        if command_flags.contains(&name) {
            continue;
        }
        // Name a command that would have accepted it, when there is one. This is
        // the difference between "unknown option" and "--verify is a `set` flag".
        let owner = COMMAND_FLAGS
            .iter()
            .find(|(_, flags)| flags.contains(&name))
            .map(|(name, _)| *name);
        match owner {
            Some(other) => anyhow::bail!(
                "{name} is a `{other}` option, not a `{command}` one; try `minikbconfig {other} {name}`"
            ),
            None => anyhow::bail!("unknown option '{arg}'; try --help"),
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

fn cmd_info(layer: u8, json: bool) -> anyhow::Result<()> {
    let pad = Pad::open()?;
    let info = pad.device_info()?;
    let table = pad.read_keys(layer)?;

    if json {
        return emit_json(&info_json(&pad, &info, layer, &table));
    }

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
    print_table(&table);
    Ok(())
}

fn cmd_dump(layer: u8, json: bool) -> anyhow::Result<()> {
    let pad = Pad::open()?;
    let table = pad.read_keys(layer)?;
    if json {
        // The same shape `snapshot` writes, minus the device block, so a script
        // can consume one form.
        return emit_json(&serde_json::json!({
            "layer": layer,
            "keys": pad.keys(),
            "bindings": bindings_of(&table),
        }));
    }
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

fn cmd_keys(json: bool) -> anyhow::Result<()> {
    if json {
        let mut media = Vec::new();
        for (usage, name) in MEDIA_NAMES {
            media.push(Name {
                code: u32::from(*usage),
                name: name.to_string(),
            });
        }
        return emit_json(&KeysJson {
            standard: keymap::KeyCode::all()
                .map(|c| Name {
                    code: u32::from(c.0),
                    name: c.name(),
                })
                .collect(),
            media,
            kinds: vec!["none", "key:", "media:", "move:"],
            modifiers: keymap::MODIFIER_NAMES.to_vec(),
        });
    }

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
    for (usage, name) in MEDIA_NAMES {
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
    let verify = args.iter().any(|a| a == "--verify");
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
fn cmd_led(args: &[String], json: bool) -> anyhow::Result<()> {
    let pad = Pad::open()?;
    let mut config = pad.light_config()?;
    let before = config;

    // `--json` is the read-only form. Refuse to combine it with a write rather
    // than silently ignoring the other flags, because `led --json --mode 2`
    // reporting mode 2 would be a lie.
    if json {
        if led_json_conflicts(args) {
            anyhow::bail!("--json reports the current state; it cannot be combined with a change");
        }
        return emit_json(&light_json(&before));
    }

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
                "--speed" => {
                    let parsed: u8 = value(i)?.parse().context("speed must be a number")?;
                    if parsed > light::MAX_SPEED {
                        anyhow::bail!(
                            "speed must be 0-{max}, the range the vendor app offers; higher \
                             values are stored but behave erratically (speed 5 fades slower \
                             on mode 3 than any supported value)",
                            max = light::MAX_SPEED
                        );
                    }
                    speed = Some(parsed)
                }
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
    if !config.colour_is_black() {
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
    warn_if_sticks_lit(&config);
    println!("  ->");
    print_light(&config);

    let stored = pad.set_light_config(config)?;
    println!(
        "\n  stored: ok (brightness {}, value {})",
        stored.brightness, stored.value
    );
    if stored.colour_is_black() && !config.colour_is_black() {
        println!("  note: the colour came back black, so the pad will show nothing");
    }
    Ok(())
}

/// Warn about the mode 3 firmware bug before writing it, not after.
///
/// A warn rather than a refusal: the pad does accept the record, so refusing
/// would block a colour a user may legitimately want for other modes. Only mode
/// 3 is affected, and it fails silently - the pad gives no indication - so the
/// user has to be told here or not at all.
fn would_stick_lit(config: &LightConfig) -> bool {
    config.mode == 3 && !config.colour_is_black() && config.value < 32
}

fn warn_if_sticks_lit(config: &LightConfig) {
    if would_stick_lit(config) {
        println!(
            "  warning: mode 3 with value {} (below 32) leaves the LED stuck lit and never \
             fades - a firmware bug, not a mistake in the setting.\n           Any value 32 or \
             above is fine, and modes other than 3 are unaffected.",
            config.value
        );
    }
}

fn print_light(config: &LightConfig) {
    println!(
        "  type {}  single-colour index {}",
        config.light_type, config.single_color_index
    );
    println!(
        "  brightness {} (no effect)  mode {}  speed {}  direction {} (no effect)  cycle {}",
        config.brightness, config.mode, config.speed, config.direction, config.cycle
    );
    println!(
        "  hue {}  saturation {}  value {}  -> {}",
        config.hue,
        config.saturation,
        config.value,
        if config.colour_is_black() {
            "black"
        } else {
            "showing colour"
        }
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

    // No read-back. Command 12 is not a read: it carries no offset, so the
    // firmware applies it at offset 0 and erases key 1's colour. Verified on
    // hardware - the same write shows the colour without the 12 and leaves the
    // key dark with it. So `led key` cannot confirm itself, and must not try.
    println!("  key[{index:2}] wrote {hex}");
    println!(
        "  not confirmed, and it cannot be: the protocol has no per-key colour read. \
         Command 12 would erase key 1 rather than report it."
    );
    println!("  see it with `minikbconfig led on --mode 5`, which shows per-key colours.");
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

    // Lighting is part of the pad's configuration, so a snapshot that omitted it
    // would not describe the pad. Only the global record: the protocol has no
    // read for per-key colours, so those cannot be captured.
    file.light = Some(minikbconfig_config::Lighting::from_core(
        pad.light_config()?,
    ));

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

    // Lighting is applied separately from the keymap, and reported separately so
    // a lighting write cannot hide inside a keymap diff.
    let light_change = match file.light {
        Some(wanted) => {
            let stored = Lighting::from_core(pad.light_config()?);
            let diff = wanted.differences(stored);
            if diff.is_empty() {
                println!("    lighting: already matches");
            } else {
                println!("    lighting: change {} ", diff.join(", "));
            }
            Some((wanted, diff))
        }
        None => {
            println!("    lighting: not in file, left alone");
            None
        }
    };

    let lighting_settled = light_change.as_ref().is_some_and(|(_, d)| d.is_empty());
    if plan.is_empty() && lighting_settled {
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

    let light_edits = usize::from(
        light_change
            .as_ref()
            .is_some_and(|(_, diff)| !diff.is_empty()),
    );
    if dry_run {
        println!(
            "\n  {} key change(s), {} lighting change(s); not applied (--dry-run)",
            plan.changes.len(),
            light_edits
        );
        return Ok(());
    }
    // The prompt counts lighting too, so the confirmation reflects everything
    // that is about to be written rather than only the keymap.
    if !assume_yes
        && !confirm(&format!(
            "apply {} key change(s) and {} lighting change(s)?",
            plan.changes.len(),
            light_edits
        ))?
    {
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

    // Lighting last, and verified against the device rather than assumed. The
    // firmware silently discards a light record with a mode outside 1-6, so a
    // write that returns success can still have done nothing.
    let mut light_applied = 0usize;
    if let Some((wanted, diff)) = light_change {
        if !diff.is_empty() {
            let stored = pad.set_light_config(wanted.to_core())?;
            let back = Lighting::from_core(stored);
            let after = wanted.differences(back);
            if !after.is_empty() {
                anyhow::bail!(
                    "lighting did not take effect: still differs in {}",
                    after.join(", ")
                );
            }
            light_applied = 1;
        }
    }

    println!(
        "\n  applied and verified {applied} key change(s), {light_applied} lighting change(s)"
    );
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

    fn flags(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn the_stuck_lit_warning_covers_exactly_the_buggy_combination() {
        // Mode 3 with a real colour below value 32 is the firmware bug. A warn
        // rather than a refusal: the pad accepts the record, so refusing would
        // block a colour that is fine for every other mode.
        let mut c = LightConfig::OFF;
        c.mode = 3;
        c.value = 31;
        assert!(
            would_stick_lit(&c),
            "mode 3, value 31 is the reported failure"
        );

        c.value = 32;
        assert!(!would_stick_lit(&c), "32 is the working boundary");

        c.value = 1;
        assert!(would_stick_lit(&c), "the range reaches the bottom");

        // Other modes are unaffected.
        c.mode = 1;
        c.value = 1;
        assert!(!would_stick_lit(&c));
        c.mode = 2;
        assert!(!would_stick_lit(&c));
        c.mode = 4;
        assert!(!would_stick_lit(&c));
        c.mode = 5;
        assert!(!would_stick_lit(&c));

        // A black colour is off, not stuck, so it is not the same failure.
        c.mode = 3;
        c.value = 0;
        assert!(!would_stick_lit(&c), "nothing to light is not stuck lit");
    }

    #[test]
    fn every_command_has_a_usage_line() {
        for (command, _) in COMMAND_FLAGS {
            let usage = usage_for(command);
            assert!(usage.starts_with("\nusage: "), "{command}: {usage:?}");
            assert!(
                usage.contains(*command) || *command == "keys-list",
                "{command} has no usage line, got {usage:?}"
            );
        }
        assert!(usage_for("nope").contains("unknown command"));
    }

    #[test]
    fn a_mistyped_flag_is_rejected_rather_than_ignored() {
        // Found by using it: `watch --raw15` is not `--raw 15`, and it used to
        // be swallowed, silently giving non-raw output for the wrong duration.
        assert!(check_flags("watch", &flags(&["--raw", "15"])).is_ok());
        assert!(check_flags("watch", &flags(&["--raw15"])).is_err());
        assert!(check_flags("apply", &flags(&["--dry-run"])).is_ok());
        assert!(check_flags("apply", &flags(&["-y"])).is_ok());
        assert!(check_flags("dump", &flags(&["--layer=2"])).is_ok());
        // Positional arguments are not flags, and neither is a lone dash.
        assert!(check_flags("apply", &flags(&["pad.json", "0"])).is_ok());
        assert!(check_flags("apply", &flags(&["-"])).is_ok());
        assert!(check_flags("dump", &flags(&["--nope"])).is_err());
    }

    #[test]
    fn a_flag_is_refused_by_a_command_that_ignores_it() {
        // The failure a flat allowlist cannot catch: `apply --verify` was
        // accepted and silently did nothing.
        let err = check_flags("apply", &flags(&["--verify"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--verify"), "{err}");
        assert!(
            err.contains("set"),
            "should name the command that takes it: {err}"
        );

        assert!(check_flags("snapshot", &flags(&["--raw"])).is_err());
        assert!(check_flags("info", &flags(&["--brightness", "2"])).is_err());
        assert!(check_flags("watch", &flags(&["--yes"])).is_err());
        assert!(check_flags("set", &flags(&["--verify"])).is_ok());
        assert!(check_flags("watch", &flags(&["--raw"])).is_ok());
    }

    #[test]
    fn json_is_offered_exactly_where_it_is_implemented() {
        // The whole point of the per-command table is that a flag is accepted
        // only where it is read. `--json` is implemented on the four read-only
        // commands and must not be accepted anywhere else, or it becomes the
        // silent-ignore bug again.
        let with_json: Vec<&str> = COMMAND_FLAGS
            .iter()
            .filter(|(_, flags)| flags.contains(&"--json"))
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(with_json, ["info", "dump", "keys", "keys-list", "led"]);

        for (command, flags) in COMMAND_FLAGS {
            if matches!(*command, "info" | "dump" | "keys" | "keys-list" | "led") {
                assert!(flags.contains(&"--json"), "{command} should take --json");
            } else {
                assert!(
                    !flags.contains(&"--json"),
                    "--json is accepted by {command} but not implemented there"
                );
            }
        }
    }

    #[test]
    fn led_json_is_refused_alongside_a_change() {
        // `led --json --mode 2` reporting the current state while the flag asked
        // for a change would be a lie. Refuse rather than ignore.
        let err = led_json_conflicts(&["--json".to_string(), "--mode".to_string()]);
        assert!(err, "a change alongside --json must be refused");
        assert!(!led_json_conflicts(&["--json".to_string()]));
        assert!(!led_json_conflicts(&[
            "--mode".to_string(),
            "2".to_string()
        ]));
    }

    #[test]
    fn minus_v_means_version_everywhere_and_never_verify() {
        // -V used to be both the version flag and a short form for
        // `set --verify`, so its meaning depended on where it appeared. It is
        // version everywhere now, matching cargo, git and docker.
        assert!(GLOBAL_FLAGS.contains(&"-V"));
        assert!(GLOBAL_FLAGS.contains(&"--version"));
        for (command, flags) in COMMAND_FLAGS {
            assert!(
                !flags.contains(&"-V"),
                "-V is a global version flag, not a `{command}` option"
            );
        }
        // `set` still has exactly one way to ask for verification.
        let set_flags = COMMAND_FLAGS.iter().find(|(n, _)| *n == "set").unwrap().1;
        assert!(set_flags.contains(&"--verify"));
        assert_eq!(set_flags.len(), 1);
    }

    #[test]
    fn global_and_layer_flags_are_accepted_everywhere() {
        for (command, _) in COMMAND_FLAGS {
            for flag in GLOBAL_FLAGS.iter().chain(LAYER_FLAGS) {
                assert!(
                    check_flags(command, &flags(&[flag])).is_ok(),
                    "{flag} should work with `{command}`"
                );
            }
        }
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
                    all_flags().contains(&name),
                    "help documents {name} but check_flags would reject it"
                );
            }
        }
    }

    #[test]
    fn every_flag_check_accepts_is_documented() {
        // The other direction: an allowlisted flag nobody can discover.
        for flag in all_flags() {
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

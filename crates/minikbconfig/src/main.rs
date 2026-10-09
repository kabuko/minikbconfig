//! Command-line configurator for SDCX SIDE-KEYBOARD macro pads.
//!
//! A browser-free replacement for the vendor's WebHID app.

mod action;
mod device;
mod pad;

use std::time::Duration;

use anyhow::Context;
use minikbconfig_core::keymap::{self, KeyTable, KeyType};

use action::parse_action;
use pad::Pad;

const USAGE: &str = "\
minikbconfig — configure SDCX SIDE-KEYBOARD macro pads (USB 6d7b:dcfa)

USAGE:
    minikbconfig <COMMAND> [OPTIONS]

COMMANDS:
    info                 Show pad details and live configuration
    dump                 Print the pad's current key map
    keys                 List supported key and media names
    set <INDEX> <ACTION> Bind one key slot, e.g. `set 1 key:ctrl-c`
    led <MODE>           Set the backlight mode (0, 1 or 2)
    snapshot <FILE>      Save the current configuration as JSON
    watch [SECONDS]      Show what the pad emits as you press it
    keys-list            (alias for `keys`)

ACTIONS:
    none                    clear the slot
    key:<name>              keystroke, e.g. key:a, key:f13, key:0x6d
    key:<mods>-<name>       with modifiers, e.g. key:ctrl-c, key:ctrl-shift-t
    media:<name>            e.g. media:volume-up, media:mute, media:0xea
    mouse:<buttons>         e.g. mouse:left, mouse:left+middle
    move:<dx>,<dy>          relative cursor motion
    delay:<ms>              macro delay

OPTIONS:
    -l, --layer <N>         Layer to read or write (default 0)
    -h, --help              Show this help
    -V, --version           Show version

EXAMPLES:
    minikbconfig info
    minikbconfig set 1 key:f13
    minikbconfig set 16 media:mute --layer 0
    minikbconfig watch 30
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
    let layer = parse_layer_flag(rest)?;

    match command.as_str() {
        "info" => cmd_info(layer),
        "dump" => cmd_dump(layer),
        "keys" | "keys-list" => cmd_keys(),
        "set" => cmd_set(rest, layer),
        "led" => cmd_led(rest),
        "snapshot" => cmd_snapshot(rest, layer),
        "watch" => cmd_watch(rest),
        other => {
            eprint!("{USAGE}");
            anyhow::bail!("unknown command '{other}'");
        }
    }
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
        if info.charge_state != 0 { " (charging)" } else { "" }
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
        println!("  key[{index:2}]  {}", describe_action(action));
    }
    // Slots this build cannot name are reported rather than hidden: dropping
    // them silently would make the dump look emptier than the pad really is.
    for (index, ty) in &table.unrecognised {
        println!("  key[{index:2}]  <unrecognised type 0x{ty:02x}>");
    }
}

fn describe_action(action: KeyType) -> String {
    match action {
        KeyType::Unassigned => "unassigned".to_string(),
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
        KeyType::MouseButton { buttons } => {
            format!("mouse:{}", keymap::mouse::name(buttons))
        }
        KeyType::MouseMove(motion) => format!("move:{},{}", motion.dx, motion.dy),
        KeyType::Duration(d) => format!(
            "delay:{}",
            d.to_millis().map(|m| m.to_string()).unwrap_or_else(|| "custom".into())
        ),
        KeyType::OpenWebsite => "url:...".to_string(),
        KeyType::CustomCombination => "custom:...".to_string(),
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

fn cmd_set(args: &[String], layer: u8) -> anyhow::Result<()> {
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
    println!("    before: {}", describe_action(before));
    println!("    after : {}", describe_action(action));

    pad.write_key(index, action, layer)
        .with_context(|| format!("could not program key {index}"))?;

    // Confirm the device stored it, and tell the user how to verify behaviour.
    let stored = pad.read_key(index, layer)?;
    if stored == action {
        println!("    stored: ok");
        println!("\n  Press that control to confirm it emits the new action.");
    } else {
        println!("    stored: MISMATCH — device returned {}", describe_action(stored));
        anyhow::bail!("write did not take effect; the device kept its previous value");
    }
    Ok(())
}

fn cmd_led(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let mode: u8 = positional
        .first()
        .copied()
        .context("usage: minikbconfig led <0|1|2>")?
        .parse()
        .context("LED mode must be 0, 1 or 2")?;
    if mode > 2 {
        anyhow::bail!("LED mode must be 0, 1 or 2 (got {mode})");
    }
    let _pad = Pad::open()?;
    anyhow::bail!(
        "LED control is not implemented: the device's light protocol (commands 0A/0B/16) \
         is documented in PROTOCOL.md but has not been verified against hardware. \
         Use the vendor app for lighting until it is."
    );
}

fn cmd_snapshot(args: &[String], layer: u8) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let path = positional
        .first()
        .copied()
        .context("usage: minikbconfig snapshot <FILE>")?;

    let pad = Pad::open()?;
    let table = pad.read_keys(layer)?;

    let mut entries = Vec::new();
    for index in 0..keymap::KEY_SLOTS {
        let Some(action) = table.get(index) else {
            continue;
        };
        if action == KeyType::Unassigned {
            continue;
        }
        entries.push(serde_json::json!({
            "index": index,
            "action": describe_action(action),
            "raw": KeyTable::record(index, action),
        }));
    }
    // Keep slots this build cannot decode, so a snapshot is not quietly lossy.
    for (index, ty) in &table.unrecognised {
        entries.push(serde_json::json!({
            "index": index,
            "action": serde_json::Value::Null,
            "unrecognised_type": format!("0x{ty:02x}"),
        }));
    }
    entries.sort_by_key(|e| e["index"].as_u64().unwrap_or_default());

    let document = serde_json::json!({
        "device": {
            "vendor_id": pad.vendor_id(),
            "product_id": pad.product_id(),
            "name": pad.product_name(),
        },
        "layer": layer,
        "keys": entries,
    });

    let text = serde_json::to_string_pretty(&document)?;
    std::fs::write(path, format!("{text}\n"))
        .with_context(|| format!("could not write {path}"))?;
    println!("wrote {path} ({} keys on layer {layer})", entries.len());
    Ok(())
}

fn cmd_watch(args: &[String]) -> anyhow::Result<()> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
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

    println!("listening for {seconds}s — press keys or turn the knob");
    let mut seen = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match watcher.next_press(remaining.min(Duration::from_millis(250)))? {
            Some(event) => {
                println!("  {}", event.describe());
                seen += 1;
            }
            None => continue,
        }
    }
    println!("{seen} event(s)");
    Ok(())
}
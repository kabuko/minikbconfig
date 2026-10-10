# minikbconfig

A native, browser-free configurator for SDCX **SIDE-KEYBOARD** macro pads.

The vendor ships a WebHID web app at `www.sdcx-tech.com`, which requires Chrome
or Edge and cannot be scripted. This is a CLI and a TUI that speak the same
protocol directly, so a pad can be configured over SSH and a configuration can
be kept in version control.

| | |
|---|---|
| `6d7b:dcfa` | 3 keys + rotary |
| `0816:246f` | 6 keys + rotary |

Both were verified end to end on hardware. They speak one protocol and differ
only in key count.

## Install

```sh
git clone <this repo> && cd minikbconfig
./scripts/install.sh
```

That builds and installs both binaries to `~/.cargo/bin`, then installs the udev
rule to `/etc/udev/rules.d` and reloads it. Use `--no-udev` to skip the sudo
steps.

Requirements: a Rust toolchain (edition 2024, so 1.85 or newer) and, on Linux,
permission to write to `/etc/udev/rules.d`.

### After installing

**Replug the pad.** A pad that was already attached when the rule was installed
keeps its old permissions until it is unplugged and plugged back in. This is the
step that most often looks like the rule did not take effect.

**Then log out and back in.** Group membership is read at login. Until it takes
effect, `sudo minikbconfig` works.

```sh
minikbconfig info        # should name the pad, not complain about permissions
```

If `info` says the pad is *forbidden*, it found the device and could not open it —
that is a permissions problem, not a missing pad. It will name the rule and the
command to install it. If it says *no supported pad found*, check `lsusb | grep
-iE '6d7b|0816'` and replug.

## Use

```
minikbconfig info                 enumerate pads, show interface + live config
minikbconfig dump                 print the pad's current key map
minikbconfig keys                 list supported key and media names
minikbconfig set <index> <action> bind one key, e.g. `set 1 key:ctrl-c`
minikbconfig led                  show lighting state
minikbconfig led on|off           backlight on or off
minikbconfig led key <n> <#rrggbb>  set one key's colour
minikbconfig snapshot <file>      save the current configuration
minikbconfig apply <file>         program a saved configuration
minikbconfig profile [n]          show or switch the active profile
minikbconfig watch [seconds]      show what the pad emits as you press it
minikbconfig completions <shell>  print a completion script
```

Every flag is checked up front, and a flag used with the wrong command is
refused rather than ignored:

```
$ minikbconfig apply pad.json --verify
error: --verify is a `set` option, not a `apply` one; try `minikbconfig set --verify`
```

### Actions

```
none                    clear the slot
key:<name>              a keystroke, e.g. key:a, key:f13, key:0x6d
key:<mods>-<name>       with modifiers, e.g. key:ctrl-c, key:ctrl-shift-t
media:<name>            e.g. media:volume-up, media:mute
move:<dx>,<dy>          relative cursor motion
```

### Confirming a write

`set` proves the device stored the value by reading it back. It cannot prove the
pad *emits* it, so:

```sh
minikbconfig set 1 key:ctrl-c --verify
```

waits for you to press the key and reports what the pad actually emitted. This
is the difference between "the device accepted it" and "it works".

## The TUI

```sh
minikbconfig-tui
```

```
up/down, j/k    move between slots
enter           edit the selected slot (tab completes, arrows choose)
L               lighting panel
r               re-read every slot
v, c            show/hide the live report pane, clear it
q, esc          quit
```

Edits are written and read back the same way the CLI does, and the confirmation
step is the same `--verify` behaviour.

## Snapshots

A configuration file is **declarative**: applying one makes the pad match it
exactly, so slots the file does not mention are cleared. Every change is shown
before anything is written:

```sh
minikbconfig snapshot pad.json
minikbconfig apply pad.json --dry-run     # see the whole diff
minikbconfig apply pad.json
```

Lighting is captured and restored too. Per-key colours are not, because the
protocol has no read for them.

## Shell completion

```sh
minikbconfig completions bash >> ~/.bashrc
minikbconfig completions zsh  > "${fpath[1]}/_minikbconfig"
minikbconfig completions fish > ~/.config/fish/completions/minikbconfig.fish
```

The word lists are generated from the same tables the parser reads, so
completion cannot offer a spelling the grammar rejects.

## What this does not do

**Macros.** The format is decoded and documented in [PROTOCOL.md](PROTOCOL.md),
but a macro step can only express mouse and scroll actions — keystrokes are a
separate mechanism. Exposing a macro editor that cannot type would be worse
than none.

**URL bindings.** Also decoded. The vendor's codec loses case and rewrites `?`
to `/`, so a bound key would open a different address than configured.

**Firmware update and bootloader entry** (`55` and `5A`) are in the protocol but
deliberately not exposed. They are untested and destructive.

## Known limitations

These are measured on hardware, not assumptions — see
[PROTOCOL.md](PROTOCOL.md) for the evidence.

- **Brightness has no effect on these models.** `brightness` 0 and 4 look
  identical. The byte is still written so state stays compatible with the vendor
  app, but do not expect it to do anything.
- **The output does not dim.** `value` 0 is dark and anything above it is full
  brightness.
- **`direction` has no effect.** It is stored and echoed, and appears unused by
  the firmware.
- **Per-key colour is only visible in effect mode 5.** In modes 1-4 every key is
  painted with the global colour, so a per-key write lands and is invisible.
- **Per-key colour cannot be verified.** There is no read for it; command `12`
  looks like one but erases key 1. `led key` says so rather than claiming
  success.
- **Effect mode 6 is not a supported mode.** The firmware tolerates it and shows
  an alternating pattern, but the vendor never writes it.

## Layout

| crate | |
|---|---|
| `minikbconfig-core` | wire protocol and key model, pure |
| `minikbconfig-config` | action grammar, snapshot format, apply plan |
| `minikbconfig-hid` | hidapi backend, device discovery |
| `minikbconfig` | the CLI |
| `minikbconfig-tui` | the TUI |

[DESIGN.md](DESIGN.md) covers why it is built this way.
[PROTOCOL.md](PROTOCOL.md) is the reverse-engineered reference, with hardware
measurements and the traps that cost real time.
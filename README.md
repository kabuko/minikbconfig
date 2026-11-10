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
git clone https://github.com/kabuko/minikbconfig && cd minikbconfig
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

`--verify` has no short form. `-V` means version everywhere, as it does for
cargo and git — it used to double as a short form for this flag, so one letter
meant two different things depending on where it appeared.

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

## Scripting

The read-only commands take `--json`, so nothing has to parse English prose:

```sh
minikbconfig info --json                  # device, firmware, battery, bindings
minikbconfig dump --json                  # the key map
minikbconfig keys --json                  # every supported name and code
minikbconfig led --json                   # lighting state, with caveats
```

```sh
# battery level
minikbconfig info --json | jq .battery_percent

# what is bound to key 2
minikbconfig dump --json | jq -r '.bindings[] | select(.index == 2) | .action'

# every media name the grammar accepts
minikbconfig keys --json | jq -r '.media[].name'
```

`dump --json` emits the same shape `snapshot` writes, minus the device block, so
one parser handles both.

Two details worth knowing:

- A slot this build cannot decode appears as `{"index": n, "action": null,
  "unrecognised_type": "0x..."}` rather than being omitted — a missing slot
  would read as "cleared", which it is not.
- `led --json` carries a `notes` array, because `brightness` and `direction` are
  stored and echoed but do nothing on these models, and a script should not have
  to rediscover that. It also warns about the mode 3 stick-lit range.

`--json` is refused on commands that would write, rather than accepted and
ignored.

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
- **Brightness comes from the colour, not the brightness byte.** The light
  record stores **HSV**, and its `value` — which is `max(r,g,b)` — is the actual
  dimming control: 0 is off, 0-255 is the brightness range. The separate
  `brightness` byte does nothing on these models. The vendor app implements
  brightness by scaling the value byte client-side; we report the byte rather
  than copy that, since it would put our stored state out of step with a pad
  configured in their app. Note per-key colour is stored as **raw RGB**, not HSV.
- **`direction` has no effect.** It is stored and echoed, and appears unused by
  the firmware.
- **Very dark colours are below the display threshold.** An HSV `value` below 32
  does not light the pad — defensible behaviour, and what mode 2 does correctly.
  **Mode 3 gets it wrong:** it turns on such a colour and then never fades it, so
  the pad stays lit. `led` warns before writing that combination. Workaround:
  bind the colour per-key and use mode 5, which bypasses HSV entirely.
- **Per-key colour is only visible in effect mode 5.** In modes 1-4 every key is
  painted with the global colour, so a per-key write lands and is invisible.
- **Per-key colour cannot be verified.** There is no read for it; command `12`
  looks like one but erases key 1. `led key` says so rather than claiming
  success.
- **Effect mode 6 is not a supported mode.** The firmware tolerates it and shows
  an alternating pattern, but the vendor never writes it.

## Running the hardware tests

```sh
MINIKBDEVICE_TEST=1 cargo test -p minikbconfig-hid --test hardware -- --ignored
```

They are `#[ignore]`d, so `cargo test` stays green without a pad and reports
them as ignored rather than as passing having run nothing. The pad's bindings are
captured before each test and restored afterwards, including on panic.

Four things are checked:

- every command PROTOCOL.md documents as answering, does answer
- the URL commands are *still* silent — if they start replying, the docs and
  `url:` support both need revisiting
- key bindings round-trip through write and read
- the light record round-trips, which is what catches a mode the firmware
  silently discards

What they cannot check is whether the pad is *visibly* lit. The protocol has no
lighting read-back, so mode 3 sticking on, brightness being inert and per-key
colour being invisible outside mode 5 are all findings that came from a person
looking at the pad, and none of them would fail here.

## What has been tested on hardware

Being straight about this, since it is easy to overstate:

**Verified against a real pad:**

- reading and writing key bindings, both models
- `set --verify` confirming an emitted keystroke by observation
- snapshots and apply, including a byte-for-byte round trip
- lighting: all six effect modes, the colour conversion across the hue wheel,
  effect speed on modes 3 and 4, and per-key colour writes on all six keys
- shell completion, driven end to end through the generated bash script

**Not verified against hardware:**

- **the TUI.** It is unit-tested, and its behaviour has been exercised manually
  since, but no TUI change has an automated check against a real pad. Treat it
  as the least-proven part of the tool.

**Known not to work on these models**, measured rather than assumed: brightness
has no effect, `direction` has no effect, brightness comes from the colour's
value rather than the brightness byte, and per-key
colour is visible only in effect mode 5. See
[PROTOCOL.md](PROTOCOL.md) for the evidence.

## Layout

| crate | |
|---|---|
| `minikbconfig-core` | wire protocol and key model, pure |
| `minikbconfig-config` | action grammar, snapshot format, apply plan |
| `minikbconfig-hid` | hidapi backend, device discovery |
| `minikbconfig` | the CLI |
| `minikbconfig-tui` | the TUI |

[CHANGELOG.md](CHANGELOG.md) records what is in each release, including the
findings that changed the implementation.

[DESIGN.md](DESIGN.md) covers why it is built this way.
[PROTOCOL.md](PROTOCOL.md) is the reverse-engineered reference, with hardware
measurements and the traps that cost real time.

## Licence

MIT. See [LICENSE](LICENSE).
# minikbconfig

A native, browser-free configurator for SDCX "SIDE-KEYBOARD" macro pads
(3 keys + rotary encoder, USB `6d7b:dcfa`).

Replaces the vendor's browser-based WebHID app (`www.sdcx-tech.com`) with a
command-line/TUI tool. Named for what it does: *mini keyboard configurator*.

**Status: protocol solved and verified against hardware, 2026-10-09.**
Read *and* write both confirmed end-to-end by observing emitted keystrokes.

---

## 1. Target device

`6d7b:dcfa` "SDINNOVATION SIDE-KEYBOARD", 3 keys + 1 rotary encoder,
USB 1.10, serial `125522678182`. Same physical family as the `1189:8890`
CH55x pads, but a **different firmware and a different protocol** — see §6.

Three HID interfaces:

| Iface | Report descriptor | Endpoints | hidraw | Purpose |
|---|---|---|---|---|
| 0 | 205 B, report IDs 1–5 | `0x82` IN, 32 B | `hidraw7` | keyboard / mouse / consumer |
| 1 | 66 B, boot | `0x85` IN, 8 B | `hidraw8` | boot keyboard + LED output |
| 2 | 31 B, usage page `0xFF00` usage `0x02` | `0x83` IN, `0x03` OUT, **64 B** | `hidraw9` | **vendor config** |

The config interface descriptor in full:

```
06 00 ff  Usage Page (Vendor Defined 0xFF00)
09 02     Usage (0x02)
a1 01     Collection (Application)
75 08     Report Size (8)
95 40     Report Count (64)
81 00     Input  (64 bytes)
91 00     Output (64 bytes)
c0        End Collection
```

**No report IDs.** 64-byte reports, both directions. Because interface 2 has an
interrupt IN endpoint, the kernel's `usbhid` binds it and a `hidraw` node exists —
so **hidapi/hidraw works on every OS with no driver, no Zadig, no USBDK**. This is
the opposite of the `1189:8890` family (config interface is OUT-only → no hidraw
node → libusb mandatory). See §6.

## 2. Wire format

Full protocol reference: **[PROTOCOL.md](PROTOCOL.md)**. Summary:

64-byte output report on the vendor interface. Derived from the vendor's own
`webhid_send_command` in `www.sdcx-tech.com`:

```js
// a = 6 (device prefix), l = command array
let u = [0, a, ...l], v = Array(65).fill(0);
u.forEach((e,a) => v[a] = e);
await hid.write(v);        // -> sendReport(0, v.slice(1))
```

```
byte  0   0x06              device prefix (every report starts with this)
byte  1   command
byte  2   length            = 3 + payload length
byte  3   offset low        \
byte  4   offset high        > u16 LE block offset / or key index * 4
byte  5   0
byte  6   layer             (0-based; layer 0 is live on this device)
byte  7   0
byte  8+  payload           (56 bytes max)
```

**Response** (interrupt IN, also 64 bytes):

```
byte  0   0xAA              response marker
byte  1   command           echoed
byte  2   length / status
byte 3-7  echoed request fields
byte  8+  payload
```

Asynchronous device notifications begin `AA FA` (LED events).

## 3. Commands

| Cmd | Name | Dir | Notes |
|---|---|---|---|
| `05` | get keyboard config | R | device info, payload at offset 5 |
| `07 38 <off>` | read key layout (bulk) | R | 56 B payload |
| `08 3A <off> <layer>` | read key layout | R | used for per-layer reads |
| `09 3B <off> <layer>` | write key layout (bulk) | W | len byte = 3 + payload |
| `0A` | get light config | R | |
| `0B <len> …` | set light config | W | |
| `0C <len> <off>` | get macro data | R | |
| `0D <len> <off> …` | set macro data | W | |
| `0F 04` / `0F FF` | reset macros / restore factory | W | |
| `10 07 <idx> <layer>` | **write one key** | W | single-key write |
| `12 <len> <off>` | get key colours (bulk) | R | |
| `13 3B <off>` | write key colours (bulk) | W | |
| `14 03 <idx>` | set one key colour | W | |
| `16 00 00 00 01 00 <mode>` | set light effect mode | W | |
| `40 <len> <off> …` | set URL config | W | |
| `41 <len> <off>` | get URL config | R | |
| `55 …` | firmware update / DFU | W | |
| `5A A0` | enter IAP | W | |
| `FB <profile>` | set current profile | W | |
| `FC …` | set auto-sleep time | W | |

## 4. Key model

**4 bytes per key**: `(type, code1, code2, code3)`.

| Type | Name | Meaning |
|---|---|---|
| `0x13` (19) | unassigned | filler |
| `0x11` (17) | MouseMove | `code1` bits: `0x08` cursor up / else down, `0x80` left / else right; `&7<<8\|code3` = vertical, `&0x70<<8\|code2` = horizontal. The emitted report's Y sign is inverted relative to this — see `PROTOCOL.md` §4.2 |
| `0x20` (32) | Standard keyboard | `code1` = modifier mask, `code2` = HID keycode |
| `0x30` (48) | Media / consumer | `code1\|code2<<8\|code3<<16` = 16-bit Consumer usage |
| `0x60` (96) | Macro | `code1` = macro index (not a mouse button) |
| `0x70` (112) | Mouse wheel | `code1` = 1 up, 255 down (not a delay) |
| `0x80` (128) | OpenWebsite | |
| `0xFF` (255) | CustomCombination | |

Modifier bits (`code1` for type 32): `01` LCTRL, `02` LSHIFT, `04` LALT,
`08` LWIN, `10` RCTRL, `20` RSHIFT, `40` RALT, `80` RWIN.

Key indices (28 user slots on this device), as read from the live layer:

| Index | Control |
|---|---|
| 0, 1, 2 | physical keys 1–3 |
| 16, 17, 18 | knob 1: press, cw, ccw |
| 19, 20, 21 | knob 2 (unused on this unit) |

**Layers are 0-based**; layer 0 is live. The vendor's React code calls
`setCurrentLayer(0)` but never sends the layer to the device, and the device's own
config reports `layer = 1` in its own 1-based field — a real trap, and the reason
an entire round of writes appeared to be accepted (they read back fine) while doing
nothing. See §7.

Key types verified end-to-end on hardware: `0x20` Standard, `0x30` Media and
`0x11` MouseMove. Types `0x60` and `0x70` store correctly but emit nothing, so
`mouse:` and `delay:` actions are refused. `0x80` and `0xFF` are unverified.

Original configuration of the test device, for reference:

```
key[ 0] 20 00 2c 00  SPACE       key[16] 30 e2 00 00  MUTE
key[ 1] 20 00 6d 00  F18         key[17] 30 e9 00 00  VOL_UP
key[ 2] 20 00 6e 00  F19         key[18] 30 ea 00 00  VOL_DOWN
key[19..21] same triple (knob 2, unused)
```

## 5. Why native beats the browser here

- **No Chromium's protected-collection restriction.** WebHID strips input/output
  reports of Generic Desktop Keyboard/Mouse/Keypad collections (c/4450937), which is
  why `cho45`'s README reports Chrome being unreliable.
- **No secure context, no user gesture** — scriptable, works headless and over SSH.
- **No Firefox/Safari dead end.**
- **Read-back**, which the web app also has but no prior-art native tool does: the
  tool can display the pad's *actual* current configuration, so a local config file
  is a cache rather than the source of truth. `restore_factory_settings` (`0F FF`)
  exists but is a blunt instrument; reading the table is the safe way to snapshot.
- **hidraw/hidapi on all three platforms**, no libusb, no driver install, no risk of
  breaking the normal keyboard interface.

## 6. Why the CH57x approach failed (kept as a warning)

The reference project in the task (`cho45/…`, VID `1189` PID `8890`) is a
*different device family*. Its protocol is the CH57x one: 8-byte commands keyed by
`[slot, type, …]` with `AA AA` flash commits, sent to an OUT-only `mi_01` interface
that has no hidraw node, so libusb is mandatory on Linux.

`6d7b:dcfa` shares the physical form factor (3 keys + encoder, slots 1–3 and 13–15)
but not the protocol. Ten CH57x candidate encodings, including the
"leading report-ID byte" framing, were tried against it and **all failed silently** —
the pad accepted the writes and echoed them back while emitting unchanged
keystrokes. Key indicators that it is not CH57x:

- different VID/PID and firmware, `bcdDevice 1.00` vs `0x01`
- config interface declares a generic vendor blob (`0xFF00/0x02`), not `mi_01`
- config interface **has** an IN endpoint (CH57x's does not)
- consumer outputs are emitted outside the device's own declared descriptor range
  (`0xE2/0xE9/0xEA` vs declared `0xC6..0xC8`), proving the mapping is programmed
- different key encoding entirely: 4 bytes/key with a type enum, vs `[slot, type, …]`

Also worth recording: `kriomant/ch57x-keyboard-tool` sends 64-byte reports where
`devkev`'s trace of the real vendor tool shows 65 — a discrepancy that goes
unnoticed because the firmware ignores the tail.

## 7. Verification method

`hidraw` exposes the config interface for both reading and writing, and the *input*
interfaces carry the pad's actual keystrokes. That gives a closed verification loop
that needs no UI and no trust in the device's own status replies:

1. write a candidate value to a key index,
2. read the table back and compare,
3. press the physical control and decode the emitted HID report,
4. assert the emitted code equals what was written.

Every claim in §4 is verified this way. The step that mattered most was step 3:
multiple write encodings were *stored and read back correctly* while changing
nothing, so read-back alone would have been a false positive.

## 8. Design

```
minikbconfig-core   protocol encode/decode + key model (pure, unit-tested)
minikbconfig-hid    hidapi backend + device discovery + udev rules
minikbconfig        CLI binary
minikbconfig-tui    ratatui editor
```

Platform: **hidapi** everywhere (Linux hidraw, Windows, macOS IOHIDDevice). No
libusb dependency at all, which removes the USBDK/Zadig problem on Windows and the
root/udev complexity on Linux beyond a single hidraw rule.

CLI:

```
minikbconfig info                 enumerate pads, show interface + descriptor + live config
minikbconfig dump                 print the device's current key map
minikbconfig keys                 list supported types and code names
minikbconfig set <index> <action> set one key, e.g. `minikbconfig set 1 key:ctrl-c`
minikbconfig set-knob <part> <action>
minikbconfig led               show lighting state; `led on|off`, `--color`, `--mode`
minikbconfig snapshot <file>      save the current configuration
minikbconfig apply <file>         program a configuration
minikbconfig watch                live decode of what the pad emits
minikbconfig tui                  interactive editor
```

`snapshot` and `apply` share one JSON format, so a file always round-trips.
Each entry keeps the device's own 4-byte `raw` record alongside the human
readable action, which makes the format lossless: a slot this build cannot
decode is still restored exactly rather than silently dropped.

`apply` is declarative — the pad ends up matching the file exactly, so slots
the file does not mention are cleared. Every change, including those clears,
is printed before anything is written; `--dry-run` shows the diff and stops,
and without `--yes` the command refuses to run when stdin is not a terminal.
Writes go through command `10` one slot at a time, never the bulk write (§3),
and each is read back before the next begins.

`minikbconfig watch` is the same trick as §7 step 3 and is the reason the tool can
promise verifiable writes rather than "sent successfully".

Permissions, Linux — install `udev/99-minikbconfig.rules`:

```
sudo cp udev/99-minikbconfig.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then replug the pad. The rule uses `TAG+="uaccess"`, so whoever is logged in
at the seat gets access without group membership.

## 9. Provenance and caveats

- The protocol was reverse-engineered from the **vendor's own shipped JavaScript**
  (`sdcx-tech.com/_next/static/chunks/app/page-*.js`), by reading minified code —
  **not from a specification**. Framing, command IDs and the type enum are read
  directly off the code; offset and field *meanings* are inferred from how the code
  uses them. Treat as "working but undocumented".
- Two known upstream bugs in the vendor's implementation, deliberately **not**
  copied: `RSoft.MacroPad`'s `ExtendedReport.CreateMouse` passes `KeyType.Multimedia`
  (2) instead of `Mouse` (3); its bulk key write reuses the length byte for
  payload length while reads use a fixed `58`.
- Firmware update commands (`55`, `5A`) are mapped but **untested** — deliberately
  not exercised, as flashing is destructive and unrecoverable if interrupted.
- The lighting commands (`0A`, `0B`, `14`, `16`) are verified working. The record
  lives at report offset 5, brightness starts at 0, and only modes 1-6 are
  stored — see `PROTOCOL.md` §3 for the traps.
- The `6d7b:dcfa` device has 6 profiles (`profileCnt = 6`) and the tool can read and
  select them (`FB`), but per-profile key storage has not been exercised beyond
  profile 0.
- Layers are 0-based and layer 0 is live. Layers 1-5 read back empty; a write to
  layer 1 stores and reads back correctly without affecting layer 0, and the pad
  still emits layer 0's binding. Verified — see `PROTOCOL.md` §5.

## 10. Tools

`tools/` holds the Python reference implementation of the protocol — the scripts
the protocol was actually solved with, kept as executable documentation and usable as
regression checks against the Rust implementation:

- `tools/probe_sdcx.py` — protocol implementation: `info`, `keys`, `sweep`, `test`, `map`
- `tools/restore_device.py` — restores the original configuration and verifies it
- `tools/probe_map.py` — decodes emitted HID input reports; used by `test` and `map`
  to verify a write actually changed behaviour

The CH57x-era exploratory scripts (`probe_hidraw.py`, `probe_sweep.py`, which
enumerated ten wrong-protocol candidate encodings) are deliberately not included;
§6 records what they established.
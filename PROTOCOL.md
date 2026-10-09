# SDCX macro pad configuration protocol

Wire protocol for SDCX "SIDE-KEYBOARD" macro pads (3 keys + rotary encoder),
USB `6d7b:dcfa`.

Reverse-engineered 2026-10-09 from the vendor's own shipped JavaScript
(`www.sdcx-tech.com`), then verified against hardware. See `DESIGN.md` for
project context and `tools/` for a working reference implementation.

**No official specification exists.** This document is inferred from minified
vendor code plus empirical testing. Treat it as "observed and verified", not
"documented by the vendor".

---

## 1. Device identification

```
Vendor ID   0x6D7B  "SDINNOVATION"
Product ID  0xDCFA  "SIDE-KEYBOARD"
bcdDevice   0x0100
USB         1.10, full speed
```

The configuration interface is matched by **usage page + usage**, not VID/PID.
The vendor's WebHID filter list is:

```js
{ vendorId: 0x6D7B, productId: 0xDCFA, usagePage: 0xFF00, usage: 0x02 }
```

The vendor supports many products via this same filter pattern; other devices
in their list also use usage page `0xFF00` usage `2`.

### Interfaces

| Iface | Report descriptor | Endpoints | Purpose |
|---|---|---|---|
| 0 | 205 B, report IDs 1–5 | `0x82` IN, 32 B | keyboard, mouse, consumer, system control |
| 1 | 66 B, boot | `0x85` IN, 8 B | boot keyboard + LED output |
| 2 | 31 B, `0xFF00/0x02` | `0x83` IN + `0x03` OUT, **64 B** | **configuration** |

Configuration report descriptor, in full:

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

**No report IDs.** Reports are 64 bytes in both directions.

Because interface 2 has an interrupt IN endpoint, the kernel `usbhid` driver
binds it and a `/dev/hidrawN` node is created. This makes hidraw/hidapi
sufficient on all platforms. (Contrast the `1189:8890` CH57x family, whose
config interface is OUT-only and therefore has no hidraw node, forcing libusb.)

On Linux the node can be located via the sysfs `report_descriptor` starting
with `06 00 ff`, or by VID/PID plus interface enumeration.

## 2. Report framing

### Request (host → device, interrupt OUT)

Derived from the vendor's `webhid_send_command`:

```js
// a = 6 (device prefix), l = command array
let u = [0, a, ...l], v = Array(65).fill(0);
u.forEach((e,a) => v[a] = e);
await hid.write(v);          // -> sendReport(0, v.slice(1))
```

The transport builds a 65-byte buffer whose first byte is discarded, so the
wire report is **64 bytes**:

| Offset | Field | Notes |
|---|---|---|
| 0 | `0x06` | device prefix — every report starts with this |
| 1 | command | see §3 |
| 2 | length | `3 + payload length` |
| 3 | offset low | u16 LE byte offset, or `key_index * 4` |
| 4 | offset high | |
| 5 | `0x00` | unused |
| 6 | layer | 0-based; layer 0 is live on this device |
| 7 | `0x00` | unused |
| 8+ | payload | up to 56 bytes |

Offset is reused as a key index for single-key operations.

### Response (device → host, interrupt IN)

Also 64 bytes:

| Offset | Field | Notes |
|---|---|---|
| 0 | `0xAA` | response marker |
| 1 | command | **not** a faithful echo — see below |
| 2 | length / status | varies by command |
| 3–7 | echoed request fields | offset, layer |
| 8+ | payload | |

Device-initiated asynchronous notifications begin `AA FA` (LED state changes).
Distinguish these from command responses before parsing.

### The command byte in a reply is not the command that was sent

Byte 1 must **not** be used to pair a reply with its request. A key-layout read
sent as `08 3A <off> <layer>` is answered with `07` in that slot:

```
request:  06 08 3a 00 00 00 00 00  ...        command 0x08, offset 0
response: aa 07 3a 00 00 00 00 00  ...        0x07, but offset echoed as 00 00

request:  06 08 3a 38 00 00 00 00  ...        command 0x08, offset 56
response: aa 07 3a 38 00 00 00 00  ...        offset echoed as 38 00
```

Verified against hardware on 2026-10-09. Matching replies on byte 1 discards
every valid key-layout reply and shows up as a timeout.

The **offset** in bytes 3–4 is echoed faithfully, so that is what identifies
the reply to a request. `minikbconfig_core::protocol::is_reply_to` does this.

Requests and responses are otherwise one-for-one: write a report, read the next
input report. There is no request ID.

## 3. Commands

Verified working commands are marked ✓. Others are present in the vendor code
but untested.

| Cmd | Name | Dir | Status |
|---|---|---|---|
| `05` | get keyboard config | R | ✓ |
| `07 38 <off>` | read key layout (bulk) | R | ✓ |
| `08 3A <off> <layer>` | read key layout | R | ✓ |
| `09 3B <off> <layer>` | write key layout (bulk) | W | ✓ |
| `0A` | get light config | R | ✓ |
| `0B <len> <off> …` | set light config | W | ✓ |
| `0C <len> <off>` | get macro data | R | ✓ |
| `0D <len> <off> …` | set macro data | W | ✓ |
| `0F 04` | reset macro data | W | ✓ |
| `0F FF` | restore factory settings | W | ✓ |
| `10 07 <idx> <layer>` | **write one key** | W | ✓ |
| `12 <len> <off>` | read key colours (bulk) | R | ✓ |
| `13 3B <off>` | write key colours (bulk) | W | ✓ |
| `14 03 <idx>` | set one key colour | W | ✓ |
| `16 00 00 00 01 00 <mode>` | set light effect mode | W | ✓ |
| `40 <len> <off> …` | write URL config | W | unverified |
| `41 <len> <off>` | read URL config | R | unverified |
| `55 …` | firmware update / DFU | W | **untested, destructive** |
| `5A A0` | enter IAP (bootloader) | W | **untested, destructive** |
| `FB <profile>` | set current profile | W | unverified |
| `FC 02 00 00 <hi> <lo>` | set auto-sleep time | W | unverified |

The device reports 6 profiles but only profile 0 has been exercised.

### Lighting

Field layout and call shapes come from the vendor's JavaScript, which is the
authoritative source for this protocol:

```js
getLightConfig()   -> sendDeviceData(6, [10]) then .slice(5, 16)
setLightConfig(e)  -> sendDeviceData(6, [11, e.length, 0, 0, ...fields])
setKeyColorInfo(e) -> l[1]=3, l[2..3]=3*key_index, l[7..9]=R,G,B
```

Three things here are easy to get wrong, and all three cost real time:

**The record starts at report offset 5, not 8.** Like the device-config reply
(`05`), lighting answers and is written at offset 5. Reading it from offset 8 —
where payloads normally start — returns an all-zero record, which looks
exactly like "this pad has no lighting".

```
aa 0a 0b 00 00 01 00 00 00 00 00 00 ...   reply to 0x0A
                  ^^ offset 5: type=1, mode=0, brightness=0, value=0
```

**Brightness is byte 3 of the record** (report byte 8), and a fresh pad has it
at 0. A colour with `value == 0` — black, or unset — lights nothing however
bright the brightness setting, so both must be raised.

**Only modes 1-6 are stored.** Sweeping modes 0-8, every write outside 1-6 is
acknowledged and echoed back correctly, then discarded: a following read
reports an all-zero record. Mode 0 is not "solid colour", it means the record
is rejected. So turning the lights on requires choosing a mode as well.

| Record byte | Field |
|---|---|
| 0 | type (the vendor always writes 1) |
| 1 | unused |
| 2 | mode, 1-6 |
| 3 | brightness, 0-4 (0 is off) |
| 4 | speed |
| 5 | direction |
| 6 | colour/cycle flag (the vendor clears it when mode is 0) |
| 7 | single-colour index; the vendor writes 0, the device reports 255 |
| 8-10 | hue, saturation, value, each 0-255 |

Colour bytes come from `#RRGGBB` converted to HSV with hue in degrees and s/v
as percentages, each then scaled by /360 or /100 into a byte. Because value is
`max(r,g,b)`, it is simply the brightest channel.

**`16` is a query, not the write.** The vendor's `setLightEffectConfig` sends
`[22, 0, 0, 0, 1, 0, mode]`, reads the reply, patches byte 2 of the returned
config with the mode, and writes the whole record back with `0B`. So `16` on
its own does nothing persistent, and its reply does not vary with the mode
value — which is exactly what a naive probe sees.

Per-key colours are written with `14`, offset **`3 * key_index`** and length
byte **3**, because colours are stored one RGB triple per key. A bare index
offset writes to the wrong key.

Verified on hardware 2026-10-09: `minikbconfig led on --color '#00ccff'` lit
the pad cyan at brightness 4, mode 2, and `led off` returned it to dark.

## 4. Key model

Each key occupies **4 bytes**: `(type, code1, code2, code3)`.

### Types

| Type | Name | Meaning |
|---|---|---|
| `0x11` (17) | MouseMove | relative cursor motion; see §4.2 for the axis polarity |
| `0x13` (19) | unassigned | filler / no mapping |
| `0x20` (32) | Standard keyboard | ordinary keystroke |
| `0x30` (48) | Media / consumer | Consumer-page usage |
| `0x60` (96) | Macro | `code1` = macro index |
| `0x70` (112) | Mouse wheel | `code1` = 1 up, 255 down |
| `0x80` (128) | OpenWebsite | launch a URL |
| `0xFF` (255) | CustomCombination | user-defined sequence |

Types 0x60 and 0x70 were previously documented here as "mouse button" and
"duration". Both are wrong; see §4.1.

### 4.2 The MouseMove vertical axis is inverted relative to HID

A stored move of `dy = +5` makes the pad emit a report of `dy = -5`, and a
report of `dy = +5` moves the cursor **down** — the opposite of the HID
convention, where positive Y is up. The horizontal axis is not inverted: a
request for `dx = -10` produces a report of `dx = -10` and the cursor moves
left.

So `code1` bit `0x08` set means the cursor moves up, and `move:<dx>,<dy>` reads
intuitively even though the emitted report's sign is flipped. Getting this
backwards is easy to miss because the encoding round-trips perfectly either
way — confirmed on hardware 2026-10-09 by watching the cursor rather than the
report.

### 4.1 Types 0x60 and 0x70 are macro and wheel, not mouse and delay

An earlier draft of this document labelled type 96 a mouse-button mask and type
112 a macro delay. Neither is right. The vendor's JavaScript settles it:

- type 96 is listed under `"label":"Macro"` with `"code":"MACRO(n)"`, and the
  decode path renders it as `"M" + code1` — `M0`, `M1`, … — so `code1` is a
  **macro index**.
- type 112 is listed as wheel up/down (`上滚轮` with `value1` 1, `下滚轮` with
  `value1` 255), and the decode path maps 1 and 255 to those two names. So
  `code1` = 1 is **wheel up**, 255 is **wheel down**.
- type 31 (not modelled here) is the device's "Special" set: LED mode, LED
  colour, profile switch, sleep, 2.4G pairing and so on.

There is no mouse-button key type on this device. The `mouse` button mask exists
only in emitted HID reports, which is a different thing.

Both types **store without error and do nothing**. Writing 96 or 112 through
command `10` reads back correctly and emits no report at all, so read-back
cannot detect the problem — only an emitted keystroke can. Neither is usable
until macro data (`0C`/`0D`) or the mouse path is understood, so `minikbconfig`
refuses `mouse:` and `delay:` actions rather than writing dead bindings.

### Type 0x20 — Standard keyboard

| Field | Contents |
|---|---|
| `code1` | modifier bitmask |
| `code2` | HID keyboard usage ID (keycode) |

Modifier bits in `code1`:

| Bit | Modifier |
|---|---|
| `0x01` | Left Ctrl |
| `0x02` | Left Shift |
| `0x04` | Left Alt |
| `0x08` | Left GUI / Win |
| `0x10` | Right Ctrl |
| `0x20` | Right Shift |
| `0x40` | Right Alt |
| `0x80` | Right GUI / Win |

Example — Ctrl+C: `20 01 03 00`.

### Type 0x30 — Media / consumer

Usage is a little-endian integer across the three code bytes:

```
usage = code1 | (code2 << 8) | (code3 << 16)
```

Observed values: `0xE2` mute, `0xE9` volume up, `0xEA` volume down,
`0xCD` play/pause, `0xB5` next, `0xB6` previous.

Note the device emits usages **outside the range declared in its own report
descriptor** (which claims `0xC6..0xC8`), so descriptor parsing will not
enumerate the full usable set.

### Type 0x11 — MouseMove

`code1` is a direction/flag byte and `code2`/`code3` hold magnitudes:

| Bit | Meaning |
|---|---|
| `0x08` | up (else down) |
| `0x80` | left (else right) |
| `0x07` | high bits of vertical distance |
| `0x70` | high bits of horizontal distance |

Decoding, matching the vendor's implementation:

```
vertical   = ((code1 & 0x07) << 8) | code3
horizontal = ((code1 & 0x70) << 8) | code2
```

### Key indices

28 user slots. As read from a live 3-key/1-knob device:

| Index | Control |
|---|---|
| 0, 1, 2 | physical keys 1–3 (left to right) |
| 16, 17, 18 | knob 1: press, cw, ccw |
| 19, 20, 21 | knob 2 — present in the table, no hardware on this unit |
| 3–15, 22–27 | unassigned on this model |

The index→control mapping is a property of the device's key layout JSON, not
of the protocol. Other models in the family will differ; discover it
empirically by writing distinct codes and observing which control emits them.

## 5. Layers

**Layers are 0-based, and layer 0 is live on this device.**

This is the single most dangerous ambiguity in the protocol. The device's own
config reply (`05`) reports `layer = 1` in a **1-based** field, while the
vendor's UI calls `setCurrentLayer(0)` and never sends the layer to the device
at all. The layer goes in **report byte 6**.

Verified on hardware 2026-10-09:

- Layers 1-5 read back as empty; only layer 0 carries a configuration.
- Writing key 0 on layer 1 stored `a` there and read back correctly, while
  layer 0 kept its own binding for the same slot. The layers are independent.
- Pressing that physical control still emitted layer 0's binding, confirming
  layer 0 is the live one and that byte 6 selects the layer being written.

So the earlier note that "writing to layer 1 stores and reads back perfectly
yet has no effect" is a statement about *behaviour*, not about the write
failing: a non-zero layer is genuinely stored, it simply is not the layer the
pad emits from. Read-back alone cannot tell you which layer is live — only an
emitted keystroke can.

Always confirm by observing an emitted keystroke, not by reading the table
back. See §7.

## 6. Verification method

The input interfaces carry the pad's actual keystrokes, so writes can be
verified end to end without trusting any status reply:

1. write a value to a key index,
2. read the table back and compare,
3. press the physical control,
4. decode the emitted HID report from interface 0 or 1,
5. confirm the emitted code matches what was written.

Step 3 is what caught the layer problem in §5. Steps 1–2 succeeded while
behaviour was unchanged, so read-back alone is a false positive for success.

Reported keystrokes are decoded as:

- interface 1 (boot keyboard): byte 0 modifiers, byte 1 pad, bytes 2–7 keycodes
- interface 0: report ID 1 = keyboard, 2 = mouse, 3/5 = consumer (u16 LE at bytes 1–2), 4 = system control

## 7. Provenance and caveats

Reverse-engineered from `www.sdcx-tech.com/_next/static/chunks/app/page-*.js`
by reading minified code. Command IDs and framing are read directly off the
source; field meanings are inferred from usage in that code.

Two bugs in the vendor's own implementation, deliberately not reproduced:

- `RSoft.MacroPad`'s `ExtendedReport.CreateMouse` passes `KeyType.Multimedia`
  (2) where it should pass `Mouse` (3).
- Its bulk key write overloads the length byte with the payload length while
  reads use a fixed `58`.

Untested and potentially destructive: firmware update (`55`) and IAP (`5A`).
Interrupting a flash can render the device unusable.

## 8. Reference implementation

`tools/probe_sdcx.py` implements everything in this document:

```sh
sudo python3 tools/probe_sdcx.py info        # device config
sudo python3 tools/probe_sdcx.py keys 0      # read the live key map
sudo python3 tools/probe_sdcx.py test 0 32 00 04 00 0 10
                                           # key 0 → 'a', then verify by pressing
sudo python3 tools/probe_sdcx.py map 6 0    # discover index → control mapping
```

`tools/restore_device.py` writes a known configuration and verifies it.

Both require root on Linux because `/dev/hidraw*` is `0600 root:root` by
default:

```
# /etc/udev/rules.d/99-minikbconfig.rules
KERNEL=="hidraw*", ATTRS{idVendor}=="6d7b", ATTRS{idProduct}=="dcfa", MODE="0660", TAG+="uaccess"
```
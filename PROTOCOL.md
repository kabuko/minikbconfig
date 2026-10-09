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
| 1 | command | echoed |
| 2 | length / status | varies by command |
| 3–7 | echoed request fields | offset, layer |
| 8+ | payload | |

Device-initiated asynchronous notifications begin `AA FA` (LED state changes).
Distinguish these from command responses before parsing.

Requests and responses are strictly one-for-one: write a report, read the
next input report. There is no request ID.

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

### Key read / write in detail

Read one chunk of the key table (56 bytes = 14 keys):

```
06 08 3A <off_lo> <off_hi> 00 <layer> 00 …
```

Response payload (offset 8) is 56 bytes of key records.

Write a single key (4-byte record):

```
06 10 07 <idx_lo> <idx_hi> 00 <layer> 00 <type> <code1> <code2> <code3>
```

where `idx = key_index * 4`. This command commits immediately — there is no
separate flash-write or save step.

Bulk write uses `09` with a 56-byte payload and length byte `3 + 56 = 59`
(`0x3B`). Sending a short payload with a short length byte to `09` stores the
value but does **not** apply it; use `10` for single-key writes.

### Device config (`05`)

Response fields begin at offset 5, not 8:

| Offset | Field |
|---|---|
| 5–6 | protocol version (u16 LE) |
| 7–8 | product ID (u16 LE) |
| 9–10 | firmware version (u16 LE) |
| 11 | work mode |
| 12 | link status |
| 13 | battery percent |
| 14 | charge state |
| 15 | profile count |
| 16 | current profile |
| 17 | layer count |
| 18 | current layer (**1-based**) |

Sample response from a real device:

```
aa 05 0e 00 00 01 00 fa dc 68 00 00 01 64 00 06 00 00 01 64 00 06 …
                │  │  │  │  │  │  │  │  └── battery = 100
                │  │  │  │  │  │  │  └───── profile count = 6
                │  │  │  │  │  │  └──────── current layer = 1
                │  │  │  │  │  └─────────── layer count = 1
                │  │  │  │  └────────────── PID 0xDCFA ✓
                │  │  │  └───────────────── firmware 0x0068
```

## 4. Key model

Each key occupies **4 bytes**: `(type, code1, code2, code3)`.

### Types

| Type | Name | Meaning |
|---|---|---|
| `0x11` (17) | MouseMove | relative cursor motion |
| `0x13` (19) | unassigned | filler / no mapping |
| `0x20` (32) | Standard keyboard | ordinary keystroke |
| `0x30` (48) | Media / consumer | Consumer-page usage |
| `0x60` (96) | Mouse button | button click |
| `0x70` (112) | Duration / delay | macro delay |
| `0x80` (128) | OpenWebsite | launch a URL |
| `0xFF` (255) | CustomCombination | user-defined sequence |

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

Example — Ctrl+C: `20 04 03 00`.

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
at all. Writing to layer 1 stores and reads back perfectly yet has no effect
on behaviour.

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
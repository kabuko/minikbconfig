# SDCX macro pad configuration protocol

Wire protocol for SDCX "SIDE-KEYBOARD" macro pads: USB `6d7b:dcfa` (3 keys +
rotary) and `0816:246f` (6 keys + rotary). One protocol across both; only the
key count differs. See §1.

Reverse-engineered 2026-10-09 from the vendor's own shipped JavaScript
(`www.sdcx-tech.com`), then verified against hardware. See `DESIGN.md` for
project context and `tools/` for a working reference implementation.

**No official specification exists.** This document is inferred from minified
vendor code plus empirical testing. Treat it as "observed and verified", not
"documented by the vendor".

---

## 1. Device identification

```
Model A      Vendor 0x6D7B  Product 0xDCFA  "SIDE-KEYBOARD"  3 keys + rotary
Model B      Vendor 0x0816  Product 0x246F  "SIDE-KEYBOARD"  6 keys + rotary
bcdDevice    0x0100
USB          1.10, full speed
```

Both models were compared directly and present the **same three interfaces in
the same order**, with identical descriptor prefixes:

```
hidraw7  iface 0   05 01 09 06 a1 01 85 01 ...   numbered reports
hidraw8  iface 1   05 01 09 06 a1 01 05 07 ...   boot keyboard
hidraw9  iface 2   06 00 ff 09 02 a1 01 19 ...   vendor config, 0xFF00/0x02
```

So the protocol is one protocol across the family; only the key count differs
(§4, Key indices). Note that `0816` shares no prefix or range with `6d7b`, so
identification is by exact vendor/product pair — never a prefix test.

The configuration interface is matched by **usage page + usage** as well as by
ID, which is what lets interface selection work on hardware this build has never
seen. The vendor's WebHID filter list is:

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
| `0C <len> <off>` | get macro data | R | ✓ (see §5b) |
| `0D <len> <off> …` | set macro data | W | ✓ (see §5b) |
| `0F 04` | reset macro data | W | ✓ |
| `0F FF` | restore factory settings | W | ✓ |
| `10 07 <idx> <layer>` | **write one key** | W | ✓ |
| `12 <len> <off>` | read key colours (bulk) | R | ✓ |
| `13 3B <off>` | write key colours (bulk) | W | ✓ |
| `14 03 <idx>` | set one key colour | W | ✓ |
| `16 00 00 00 01 00 <mode>` | set light effect mode | W | ✓ |
| `40 <len> <off> …` | write URL config | W | 59-byte write chunks, §4c |
| `41 <len> <off>` | read URL config | R | 128-byte area, 56-byte read chunks, §4c |
| `55 …` | firmware update / DFU | W | **untested, destructive** |
| `5A A0` | enter IAP (bootloader) | W | **untested, destructive** |
| `FB <profile>` | set current profile (index in byte 2) | W | ✓ |
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
| 5 | direction; stored verbatim for every value tried (0-255), unlike mode |
| 6 | colour/cycle flag (the vendor clears it when mode is 0) |
| 7 | single-colour index; the vendor writes 0, the device reports 255 |
| 8-10 | hue, saturation, value, each 0-255 |

Colour bytes come from `#RRGGBB` converted to HSV with hue in degrees and s/v
as percentages, each then scaled by /360 or /100 into a byte. Because value is
`max(r,g,b)`, it is simply the brightest channel.

### Effect modes, measured

Swept on the 6-key model by setting one mode at a time and watching the pad.
Brightness, colour, speed and direction were held fixed; only the mode varied.

The vendor's per-device config (`0816_246f.json`, identical for `6d7b_dcfa`)
names every mode and declares which parameters each one uses:

| mode | name | brightness | speed | direction | colour | observed |
|---|---|---|---|---|---|---|
| 0 | 关闭 off | – | – | – | – | off |
| 1 | 常亮 steady | ✓ | – | – | ✓ | steady, all keys lit |
| 2 | 呼吸 breathe | ✓ | ✓ | – | ✓ | slow pulse |
| 3 | 按亮 press | ✓ | ✓ | – | ✓ | dark until a key is held |
| 4 | 潮汐 tide | ✓ | ✓ | – | ✓ | colour fade, all keys together |
| 5 | custom | ✓ | – | – | – | **per-key colours** |

**There are exactly six modes, 0-5. There is no mode 6.** Our mode 6 — an
alternating pattern of even keys — reproduces, but the vendor never writes it,
so it is a byte the firmware tolerates rather than a supported effect. Do not
expose it.

**`direction` is false for every mode.** No mode in the vendor config uses it,
and the app has no control for it; the 4-way arrow selector in the UI is driven
by a per-device `direction` flag that is absent here. So `direction` is a
vestigial field rather than one whose effect we failed to provoke. The value
0-3 originally documented was never justified, and the pad stores any value
0-255 verbatim.

Mode 6 lights keys 2, 4, 6 and leaves 1, 3, 5 dark, painted in the *global*
colour rather than the stored per-key colours: with a rainbow stored in every
key it still showed green, and followed a global colour change to magenta. It
reproduces reliably.

It was recorded here as "not reproducible" for a while, on the strength of a
second run that showed all six keys lit. That second run was taken while the pad
was displaying mode 1 — the question was asked after switching modes, so the
observation belonged to a different mode. A pattern that appeared once and then
not at all is worth re-checking by looking at what is actually on screen; the
mode was in fact deterministic the whole time.

Mode 6 also ignores stored per-key colour, painting the global colour on even
keys. It looks superficially like mode 5 from a distance, because a per-key
table full of black reads as alternating darkness — which is what mode 6 was
mistaken for before the rainbow existed.

**Mode 4 is a global fade, not a per-key pattern.** All six keys show the same
colour and fade together. `speed` *is* verifiable there — speed 1 is a slow
fade, speed 255 visibly faster — and it is the only lighting parameter
confirmed to affect anything beyond what is stored.

Mode 6 is static, and swept direction 0-4 on it changed nothing. Given the
vendor config declares `direction: false` for every mode, that is the expected
result rather than a failed experiment: there is no mode on which direction
could act, so the sweep could not have succeeded.

`speed` applies only to modes 2, 3 and 4, and only mode 4 was confirmed to
animate. `brightness` applies to modes 1-5 and is enforced 0-4 by the tool.

Modes 5 and 6 can leave some keys dark, so "brightness 3" in a read-back does
not mean every key is lit. The record describes the global configuration, not
the per-key result, and the tool has no way to read which keys are dark.

**Mode 5 is the only mode that displays stored per-key colours**, and it is the
only way to see a `14` write land. See the per-key colour section below — this
is the single most important thing to know when testing lighting by eye.

### Colour is verified across the wheel

Swept on the 6-key model by setting a colour and looking at the pad. Every
observation lands where the standard `byte / 255 * 360` mapping predicts:

| byte | degrees | observed |
|---|---|---|
| 0 | 0 | red |
| 22 | 31 | orange, yellow-leaning |
| 85 | 120 | green |
| 94 | 133 | cyan, green side |
| 107 | 151 | spring-cyan |
| 117 | 165 | pale blue |
| 170 | 240 | blue |

So `hex_to_hsv` and the `/360` scaling are both correct, and the device renders
the hue byte the way the vendor writes it. Two earlier readings of this looked
like bugs and were not:

- `#00ff88` (byte 107, 151 degrees) reads as light blue. It is spring-cyan, and
  that description is defensible.
- Cyan-greens near a hue boundary are **bad test colours**. `#00ff00`,
  `#ff0000` and `#0000ff` are the discriminators; anything in the 130-165 degree
  band will be called "blue" by one person and "green" by another, which sends
  you looking for a fault in the arithmetic that is not there.

The failure mode to avoid: one surprising colour read as a broken hue mapping,
then a second theory invented to explain the first theory being wrong. Collect
the whole band before concluding anything.

**`16` is a query, not the write.** The vendor's `setLightEffectConfig` sends
`[22, 0, 0, 0, 1, 0, mode]`, reads the reply, patches byte 2 of the returned
config with the mode, and writes the whole record back with `0B`. So `16` on
its own does nothing persistent, and its reply does not vary with the mode
value — which is exactly what a naive probe sees.

Per-key colours are written with `14`, offset **`3 * key_index`** and length
byte **3**, because colours are stored one RGB triple per key. A bare index
offset writes to the wrong key.

**Per-key colour works, but only effect mode 5 reveals it.** `14` writes
correctly at `3 * index` for every key including index 0 — there is no special
case for the first key. The trap is that modes 1 and 4 paint every key with the
*global* colour and hide the per-key data completely. A write can land
perfectly and look like nothing happened.

So a per-key colour write must be checked in mode 5:

```
minikbconfig led on --mode 5          # reveals per-key colours
minikbconfig led key 1 '#0000ff'      # physical key 2 turns blue
minikbconfig led on --mode 1          # every key is green again - write is hidden
```

Measured on the 6-key model, one write at a time with nothing else in between:
key 1 written red was red in mode 5 and green again in mode 1, and keys 1-6 all
accept writes including index 0.

Getting this wrong is easy, and it was got wrong repeatedly here. Modes 1 and 4
hide per-key writes completely, so "the write did nothing" is the default
observation for a write that worked perfectly. Two rounds of testing were
discarded before one write was made and observed on its own: an earlier batch had
probe reports landing on neighbouring keys and overwriting what had just been
set, which made it look as though index 0 and index 1 were both broken.

### Command `12` has no read framing. It erases key 1.

**Never send `12`.** The vendor app contains no per-key colour *read* — the
`getKeyRGBBytes` helper is just the generic `getDeviceData` primitive, used for
layout and macro reads, and is never called for colours. So there is no correct
framing to find. Our `12` request carries no offset, and the firmware applies it
at offset 0, where it lands on the colour-write path and zeroes key 1.

Proven directly, one variable at a time:

```
14 write, offset 0, red   -> key 1 is red
14 write, offset 0, red
  then 12 read            -> key 1 goes dark
```

This is the single most damaging thing found about this protocol, and it was
sitting in our own verification path. `led key` wrote the colour and then read
it back, and the read-back **erased key 1's colour**. Every `led key` invocation
was destroying index 0 immediately after setting it, which is why index 0
appeared to be an unwritable key.

It also explains the symptom that pointed here: the colour flashed red and then
went off. The write landed; the read-back killed it.

Note what this is *not*: a colour read with the wrong shape. The vendor never
reads per-key colours at all, so there is nothing to fix. `led key` cannot be
given verification, and that is a property of the protocol rather than a gap in
our implementation.

Consequences, all now in place:

- `led key` does not attempt a read-back. There is no usable one to attempt.
- `Pad::key_colors` and `light::read_key_colors` are kept only so this finding
  can be reproduced. Both carry `# Warning` docs saying so. Nothing in the CLI or
  TUI may call them.
- Writes cannot be confirmed programmatically. Confirm with
  `minikbconfig led on --mode 5`, which is the only view that shows per-key
  colours.

All six keys accept writes, index 0 included. A six-colour rainbow writes
correctly.

The wrong conclusion, recorded because it is easy to repeat: testing a per-key
write in a global-display mode, seeing no change, and concluding the feature is
inert. The write was acknowledged correctly the whole time. Do not treat
`14`'s acknowledgement as proof — it is the same one the firmware sends for
records it discards, see the mode range above — but equally, do not treat the
*absence* of a visible change in mode 1 as proof it failed. Check mode 5.

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
until the macro action encoding (§5b) is understood, `minikbconfig` refuses
`mouse:`, `delay:` and macro bindings rather than writing keys that read back
correctly and do nothing.

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

28 user slots, and the numbering is **not contiguous**: the keys occupy
`0..keys`, then nothing until the knobs at 16.

| Index | Control |
|---|---|
| `0..keys` | physical keys, left to right — 3 on `6d7b:dcfa`, 6 on `0816:246f` |
| 16, 17, 18 | knob 1: press, cw, ccw |
| 19, 20, 21 | knob 2 — present in the table on both, no hardware seen yet |
| `keys..15`, 22–27 | unassigned |

Confirmed on both models: the knob slots are identical, and only the key count
differs. The device does not report that count — `getConfig` has no such field —
so it is carried per model rather than discovered, and a 3-key board is never
shown a "key 4".

The index→control mapping is a property of the device's key layout, not of the
protocol, so it is the one part that varies across the family. Discover it
empirically by writing distinct codes and observing which control emits them.

## 4c. URL bindings

The URL area is **128 bytes** and the URL is **not stored as text**. It is a
substitution cipher over a 62-symbol alphabet: each character becomes one byte,
`0xFF` terminates.

| Cmd | Hex | Direction | Shape |
|---|---|---|---|
| 64 | `0x40` | W | `[64, len, off_lo, off_hi, …]` |
| 65 | `0x41` | R | `[65, len, off_lo, off_hi]` — payload at reply byte 8 |

Writes step by 59 bytes, reads by 56.

### Alphabet

The table maps each character to a byte. Several characters deliberately share a
byte, so the mapping is many-to-one:

| byte | characters |
|---|---|
| 4-29 | `a`-`z`, case-insensitive |
| 30-39 | digits `1`-`9`, `0` last |
| 40 | newline |
| 44 | space |
| 45 | `-`, `_` |
| 46 | `=` |
| 47-48 | `[`, `]` |
| 50 | `\` |
| 51 | `;`, `:` |
| 52 | `'` |
| 53 | `` ` `` |
| 54 | `,` |
| 55 | `.`, `>` |
| 56 | `/`, `?` |

Decoding reverses it, and applies three substitutions on the way: `;`→`:`,
`?`→`/`, `>`→`.`. Anything not in the table is skipped.

### Two defects in the vendor's own codec

**Case is unrecoverable.** `a` and `A` both encode to byte 4, and decoding
lowercases everything. `https://Example.COM` round-trips to
`https://example.com`.

**`?` is not `?`.** `?` encodes to the same byte as `/` (56), and decoding maps
that byte to `/`. So a query string is silently rewritten: `www.a.co/x?y=1`
round-trips to `www.a.co/x/y=1`. Combined with case folding, any URL needing
distinct `?` or uppercase cannot survive a round trip.

This is the vendor's bug, not ours — reproducing the codec exactly reproduces
both. Worth knowing before offering `url:` bindings, since a bound key would open
a subtly different address than the one configured.

The bound key is type `0x80` (OpenWebsite) with all three code bytes zero; the
URL itself lives only in this 128-byte area.

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

## 5a. Profiles

The pad reports 6 profiles, 0-based, selected with command `FB`.

**Profiles hold independent key tables.** Switching changes which table
`read_keys` and `write_key` act on, so `--layer` alone does not identify a slot:
layer 0 of profile 0 and layer 0 of profile 4 are different bindings. Verified
on hardware 2026-10-09 — profile 0 has `space`/`f18`/`f19` on keys 1-3 while
profile 4 has `ctrl+z` on all three. Confirmed behaviourally as well as by
read-back: on profile 4 the physical key emits `ctrl+z`, and back on profile 0 it
emits `space` again.

### The profile index goes in byte 2, not in a payload

The vendor sends `[251, index]` with **no length byte**:

```js
async setCurrentProfile(e){ await this.api.sendDeviceData(6, [251, e]) }
```

so the index lands in byte 2 — the position that carries a *length* for every
other command. This is a live trap, hit while writing this tool:

```
06 FB 04 ...   length byte 4 read as "select profile 4"
```

A probe meant to select profile 1 sent its index as an ordinary payload with
`length = 4`, and the firmware read **4** as the profile number. It accepted the
command without complaint and switched the pad to profile 4, where it stayed.
Every subsequent read then returned profile 4's table, which looked like the
key table had been destroyed. It had not — profile 0 was intact and switching
back restored it.

This is the `DESIGN.md` §7 failure mode at its worst: a write that reports
success, reads back plausibly, and silently means something else. `minikbconfig
profile N` reads the device back after switching rather than trusting the reply.

### The vendor app does not read the layout from the device

Worth recording, since it explains why the device read path is undocumented:

```js
async getKeyboardLayout(){
  return await l(3264)("./" + vendorId_hex + "_" + productId_hex + ".json")
}
```

The vendor app fetches a bundled `00006d7b_0000dcfa.json` rather than issuing a
layout read. Everything else — config, lighting, macro, key writes — goes to the
device. So the key-table read that this tool relies on is not something the
vendor exercises, and its framing had to be established independently.

The filename is `<vid>_<pid>.json`, so the bundle is per-model — a 6-key board
would have its own `00000816_0000246f.json`. Only the 3-key model's has been
seen, so whether the vendor ships a bundle for the 6-key model, and what its
`keys` entries say about the extra three slots, is unknown.

## 5b. Macros

The macro data area is **4096 bytes**, read with `0C` and written with `0D`:

```
bytes 0..63     16 x u16-LE offsets, one per macro M0..M15; 0xFFFF = empty
bytes 64..4095  steps, 4 bytes each
```

Offsets and lengths use the ordinary framing (`length` in byte 2, offset in
bytes 3-4), but **the payload is read from report byte 5, not byte 8** — the
same quirk as the light record in §3. Measured 2026-10-09: writing a known
4096-byte pattern with the payload at byte 8 lands it 3 bytes late at every
offset from 0 to 2000, and with the payload at byte 5 the delta is exactly 0
throughout.

Reads return their payload at byte 8 as usual, so the two directions differ.
The vendor writes in 59-byte chunks and reads in 56.

Per the vendor's decoder, a step is:

```
bytes 0-1   delay, u16 LE
byte  2     flags: &63 = type, bit 6 = action, bit 7 = last step
byte  3     code
```

### What is verified, and what is not

Verified on hardware 2026-10-09:

- The 4096-byte area reads back exactly what was written, byte for byte.
- A key bound to type `0x60` with `code1` = macro index **does** run that macro
  when pressed: the pad emitted real HID input, confirmed by keystrokes
  arriving in a live editor.
- The vendor writes type 96 with `value2` = 1 alongside the index.

Per the vendor's encoder, byte 2 is built as:

```
n = 0
action == 1  ->  n |= 64      (bit 6)
type == 1     ->  n |= 3
type == 2     ->  n |= 2
type == 3     ->  n |= 4
type == 4     ->  n |= 5
last step     ->  n |= 128     (bit 7)
```

and its decoder inverts that exactly: `&63 == 2` is type 2, `==4` is type 3,
`==5` is type 4, anything else is type 1. **The encoder and decoder are mutually
consistent** — 1→3→1, 2→2→2, 3→4→3, 4→5→4. So the format is self-describing and
the byte-2 layout is settled.

Only four low-bit values are reachable: **{2, 3, 4, 5}**.

### Code semantics per type

The same bundle resolves a step's code by type, and this is what explains the
hardware result:

- **type 1** — indexed into a fixed mouse table `D`. It is a **mouse button
  mask, not a keyboard code**: `1` L, `2` R, `4` **middle**, `8` forward,
  `16` back.
- **type 2** — MouseMove, resolved from the per-device entry for that type.
- **type 3** — scroll, `0xFF` for down else up.
- **type 4** — scroll, `0xFF` for left else right.

A keystroke is **not** one of these four types. Keys are bound with type
`0x20` (Standard), which is handled by a separate path outside the macro step
decoder.

### The eight-step experiment was not reporting an encoder bug

The earlier result — one step per raw byte-2 value 0-7, code `0x04`, every step
emitting a mouse *middle* click — is now fully explained, and the encoder was
never wrong.

`0x04` is `D[4]` = middle button, correct for type 1. And **every byte-2 value
that is not 2, 4 or 5 falls through to type 1**, so 0, 1, 6 and 7 all resolve as
MouseMove-with-a-mouse-button. That is exactly what the vendor's own decoder
does, and it matches the device byte for byte.

So the device **agrees with the vendor JS**. The experiment was simply invalid:
it used four byte-2 values the encoder cannot produce, and a code that means
"middle mouse button" rather than a keystroke.

A valid experiment writes byte-2 values 2, 3, 4 and 5 with a code appropriate
to each type, and compares against the table above.

Still outstanding: a keystroke inside a macro. Since no macro type carries a
keyboard code, that is either a separate step type not in the decoder, or the
vendor composes keystrokes from `CustomCombination` (type 4). Until that is
identified, `minikbconfig` does not expose macros, because a binding that stores
and reads back perfectly while emitting the wrong thing is the same failure mode
as §7.

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

### An unreleased keycode persists in the boot report

The boot report carries up to six simultaneous keycodes, and the pad does not
always clear them. Observed 2026-10-09: after a macro emitted repeated `a`
keystrokes with no matching release, every subsequent press reported its own key
**plus** a stray `a` — pressing key 1 gave `a` and `f13` together.

The stray code is not a held key. With nothing pressed, no report arrives at
all, so it is stale state in the report array rather than a latched modifier.
It appears on every control, which rules out a per-key binding.

Two consequences:

- **Comparing reports must check for the wanted key, not for exact equality.**
  A binding of `f13` is working when the report contains `f13`; requiring the
  array to equal `[f13]` reports a good write as a failure. But the extra codes
  are worth reporting separately, because a stuck key is a real fault with a
  different fix — `minikbconfig` distinguishes "wrong action" from "right
  action plus a stray key" for exactly this reason.
- **An unplug clears it.** It does not survive power-cycling, and
  `restore_factory_settings` is not needed for it.

This is a good illustration of why step 3 cannot be skipped. In this case
read-back had already confirmed the write, and only observing the emitted report
revealed anything at all.

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

Both require access to `/dev/hidraw*`, which is `0600 root:root` by default.
Install `udev/99-minikbconfig.rules`, which covers both models and carries a
group fallback as well as `uaccess`:

```
sudo cp udev/99-minikbconfig.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Two ways in, on purpose. `TAG+="uaccess"` grants the logged-in seat's user
access with no group setup, but **only where `/dev` supports POSIX ACLs** — a
devtmpfs mounted without the `acl` option cannot store them and the tag is then
silently ignored, which is the case on the machine this was developed on.
`GROUP="plugdev"` works everywhere but needs `usermod -aG plugdev "$USER"` and
a fresh login.

Until then, `newgrp plugdev -c "minikbconfig dump"` works around the stale group
list in an already-open shell.

`tools/restore_device.py` hardcodes the **3-key board's** factory bindings and
will overwrite the 6-key board's defaults with the wrong model's. Prefer a
snapshot — see `snapshots/`, one file per board, taken from the device so it
cannot be the wrong model:

```
minikbconfig snapshot snapshots/six_key
minikbconfig apply snapshots/six_key --dry-run
```
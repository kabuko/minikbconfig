# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

This is the first release: the version has been `0.1.0` across every commit, and
nothing has been tagged or published before.

This is the first release: the version has been `0.1.0` across every commit, and
nothing has been tagged or published before.

## [0.1.0] — 2026-10-10

First release. The protocol was reverse-engineered from the vendor's own
JavaScript and verified against hardware; this release contains everything
learned, including the parts that turned out to be worse than expected.

### Supported devices

- `6d7b:dcfa` — 3 keys + rotary, verified end to end
- `0816:246f` — 6 keys + rotary, verified end to end

Both speak one protocol and differ only in key count. The tool matches on exact
vendor/product pairs, never a prefix test — the two USB IDs share no range.

### CLI

- `info`, `dump`, `keys`, `set`, `led`, `snapshot`, `apply`, `profile`, `watch`,
  `completions`
- `set --verify` waits for a keypress and reports what the pad actually emitted,
  rather than only that the device stored it
- `watch --raw` shows every report with hex and interface
- `--json` on `info`, `dump`, `keys` and `led`, so the tool can be scripted
- Generated shell completion for bash, zsh and fish, from the same tables the
  parser reads
- Unknown flags are rejected rather than ignored, and a flag used with the wrong
  command names the command that does accept it

### TUI

- Slot list with per-model key labels
- Editing with read-back verification
- Tab completion over the full action grammar
- Lighting panel
- Live report pane showing what the pad emits
- Three-way diagnosis when the pad is unreachable: absent, forbidden, or a
  protocol mismatch — each with the advice that actually fits

### Lighting

All six effect modes characterised on hardware, plus the colour conversion
verified across the hue wheel against the standard `byte / 255 * 360` mapping.

Findings that shaped the implementation:

- **Mode 3 is press-lit**: lit while held, fading after release, with the fade
  restarting on another press. `speed` (0-4) sets that fade.
- **Per-key colour is only visible in mode 5.** In modes 1-4 every key is
  painted with the global colour, so a per-key write lands and is invisible —
  which looks exactly like a failed write.
- **Command `12` is not a read.** It carries no offset, so the firmware applies
  it at offset 0 and erases key 1's colour. There is no per-key colour read, and
  `led key` therefore cannot verify its writes and says so.
- **The brightness byte is inert on these models.** `value` — the colour's HSV
  value, which is `max(r,g,b)` — is the real brightness control. The vendor app
  works around the same limitation by scaling the value client-side.
- **Mode 6 is not a supported mode.** The firmware tolerates it; the vendor
  never writes it.

### Known issues

- **Mode 3 sticks lit below HSV value 32.** A colour too dim to display does not
  light in other modes, which is defensible — but mode 3 turns it on and never
  fades it. `led` warns before writing that combination. Reachable from the
  vendor's own brightness slider, so it is not only an API-level concern.
  Workaround: bind the colour per-key and use mode 5, which bypasses HSV.

### Not supported, and why

- **Macros.** The format is fully decoded, but a macro step can only express
  mouse and scroll actions — keystrokes are a separate mechanism, confirmed
  against the vendor's own key table. A macro editor that cannot type would
  mislead.
- **URL bindings.** The commands `40`/`41` are not implemented on either
  supported model — they give no reply at all, while other config commands reply
  normally. A key bound to OpenWebsite stores and does nothing. Neither per-device
  config declares a URL capability, so the vendor UI never offers it either.
- **Firmware update and bootloader entry** (`55`, `5A`). Present in the
  protocol, untested and destructive, never exposed.

### Fixes worth calling out

- Flags that were implemented, documented and then rejected by validation —
  including every `led on` option and `set --verify` — now work.
- `snapshot --dry-run` wrote the file anyway, which destroyed a committed
  snapshot; it is now refused.
- `minikbconfig-tui` now shows per-key lighting only in the mode that displays
  it, and labels the effect modes by the vendor's own names.

### Verification

Everything in this release was checked against real hardware. What was *not*
verified is stated plainly in the README: the TUI has no automated hardware
check, only manual exercise.

<!-- No link definitions: this repository has no remote configured yet. Add one
     here when it does, so releases resolve. -->
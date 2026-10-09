#!/usr/bin/env python3
"""
Restore the SIDE-KEYBOARD to the exact configuration it had before probing,
then read it back to verify.

Original live state (layer 0), captured by reading the device before any writes:
    key[ 0] 20 00 2c 00   SPACE        (physical key 1)
    key[ 1] 20 00 6d 00   F18          (physical key 2)
    key[ 2] 20 00 6e 00   F19          (physical key 3)
    key[16] 30 e2 00 00   MUTE         (knob press)
    key[17] 30 e9 00 00   VOL_UP       (knob cw)
    key[18] 30 ea 00 00   VOL_DOWN     (knob ccw)
    key[19] 30 e2 00 00   MUTE         (knob 2, unused)
    key[20] 30 e9 00 00   VOL_UP       (knob 2, unused)
    key[21] 30 ea 00 00   VOL_DOWN     (knob 2, unused)

Probing also wrote indices 0..5 on layer 1 (a scratch layer this device does not
use). Those are reset to the unassigned filler 13 00 00 00.

    sudo python3 restore_device.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from probe_sdcx import build, find_vendor_node, read_chunk, transact  # noqa: E402

SINGLE = 0x10
FILLER = bytes([0x13, 0x00, 0x00, 0x00])

ORIGINAL_LAYER0 = {
    0: bytes([0x20, 0x00, 0x2C, 0x00]),
    1: bytes([0x20, 0x00, 0x6D, 0x00]),
    2: bytes([0x20, 0x00, 0x6E, 0x00]),
    16: bytes([0x30, 0xE2, 0x00, 0x00]),
    17: bytes([0x30, 0xE9, 0x00, 0x00]),
    18: bytes([0x30, 0xEA, 0x00, 0x00]),
    19: bytes([0x30, 0xE2, 0x00, 0x00]),
    20: bytes([0x30, 0xE9, 0x00, 0x00]),
    21: bytes([0x30, 0xEA, 0x00, 0x00]),
}

DIRTY_LAYER1 = range(0, 6)


def write_key(node, index, value, layer):
    rep = build(SINGLE, offset=4 * index, layer=layer, payload=value, length=7)
    return transact(node, rep)


def read_key(node, index, layer):
    body = read_chunk(node, 0x08, 4 * index, layer, 58)
    return body[:4] if body else None


def main():
    node = find_vendor_node()
    if node is None:
        print("vendor interface not found")
        return 1
    print(f"vendor interface: {node}\n")

    print("restoring layer 0 (live):")
    ok = True
    for index, value in sorted(ORIGINAL_LAYER0.items()):
        write_key(node, index, value, 0)
        got = read_key(node, index, 0)
        match = got == value
        ok = ok and match
        print(f"  key[{index:2d}] want {value.hex(' ')}  got "
              f"{got.hex(' ') if got else 'no reply'}  {'OK' if match else 'MISMATCH'}")

    print("\nclearing probe junk on layer 1 (indices 0..5 -> unassigned):")
    for index in DIRTY_LAYER1:
        write_key(node, index, FILLER, 1)
        got = read_key(node, index, 1)
        match = got == FILLER
        ok = ok and match
        print(f"  key[{index:2d}] got {got.hex(' ') if got else 'no reply'}  "
              f"{'OK' if match else 'MISMATCH'}")

    print("\nverify the whole live layer:")
    for chunk in range(2):
        body = read_chunk(node, 0x08, chunk * 56, 0, 58)
        if not body:
            print("  read failed")
            ok = False
            continue
        for i in range(0, len(body) - 3, 4):
            idx = chunk * 14 + i // 4
            t, c1, c2, c3 = body[i:i + 4]
            want = ORIGINAL_LAYER0.get(idx)
            flag = "" if want is None else ("  <- restored" if body[i:i + 4] == want else "  <- MISMATCH")
            if want is not None and body[i:i + 4] != want:
                ok = False
            if t != 0x13 or want is not None:
                print(f"  key[{idx:2d}] type={t:3d} c1={c1:#04x} c2={c2:#04x} c3={c3:#04x}{flag}")

    print("\nRESULT:", "device restored and verified" if ok else "SOME MISMATCHES - see above")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
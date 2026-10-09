#!/usr/bin/env python3
"""
Read/verify the SDINNOVATION SIDE-KEYBOARD protocol, reverse-engineered from
https://www.sdcx-tech.com (vendor WebHID app).

Wire format: 64-byte output report on the vendor interface (usage page 0xFF00).
  wire[0] = 0x06                       device/command prefix
  wire[1] = command
  wire[2] = 3 + payload length
  wire[3..4] = offset (u16 LE)
  wire[5] = 0
  wire[6] = layer
  wire[7] = 0
  wire[8..] = payload (56 bytes max)

Commands used here:
  05                get keyboard/device config      (read)
  08 3a <off> <layer>   read 56 bytes of key layout (4 bytes/key: type,c1,c2,c3)
  13 3a <off>           read 56 bytes of per-key RGB (3 bytes/key)
  10 07 <idx> <layer>   write one key

READ-ONLY unless `test`/`map` is used.

Layers are 0-based; layer 0 is the live one on this device. The key index goes in
the offset field as index*4.

    sudo python3 tools/probe_sdcx.py info           # command 05, device info
    sudo python3 tools/probe_sdcx.py keys 0         # read the live key map
    sudo python3 tools/probe_sdcx.py test 0 32 00 04 00 0 10
                                                    # key 0 -> 'a'
                                                    # (index, type, code1, code2,
                                                    #  code3, layer, cmd)
    sudo python3 tools/probe_sdcx.py map 6 0        # identify the index <-> control mapping
"""

import os
import select
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

VID, PID = 0x6D7B, 0xDCFA
REPORT_LEN = 64

KEY_TYPES = {17: "MouseMove", 19: "unassigned", 32: "Standard", 48: "Media", 96: "MouseBtn",
             112: "Duration", 128: "OpenWebsite", 255: "CustomCombination"}
MODS = {0x01: "LCTRL", 0x02: "LSHIFT", 0x04: "LALT", 0x08: "LWIN",
        0x10: "RCTRL", 0x20: "RSHIFT", 0x40: "RALT", 0x80: "RWIN"}


def sysfs_dir(node):
    return os.path.realpath(os.path.join("/sys/class/hidraw", os.path.basename(node), "device"))


def find_vendor_node():
    import glob
    for node in sorted(glob.glob("/dev/hidraw*")):
        path = sysfs_dir(node)
        try:
            desc = open(os.path.join(path, "report_descriptor"), "rb").read()
        except OSError:
            continue
        if desc[:2] == b"\x06\x00":          # usage page 0xFF00
            ids = None
            p = path
            while p and p != "/":
                v, q = os.path.join(p, "idVendor"), os.path.join(p, "idProduct")
                if os.path.exists(v) and os.path.exists(q):
                    ids = (int(open(v).read().strip(), 16), int(open(q).read().strip(), 16))
                    break
                p = os.path.dirname(p)
            if ids == (VID, PID):
                return node
    return None


def build(cmd, offset=0, layer=0, payload=b"", length=None):
    """Assemble the 64-byte report exactly as the vendor's webhid_send_command does."""
    if length is None:
        length = 3 + len(payload)
    buf = bytearray(REPORT_LEN)
    buf[0] = 0x06
    buf[1] = cmd
    buf[2] = length
    buf[3] = offset & 0xFF
    buf[4] = (offset >> 8) & 0xFF
    buf[5] = 0
    buf[6] = layer
    buf[7] = 0
    for i, b in enumerate(payload):
        if 8 + i < REPORT_LEN:
            buf[8 + i] = b
    return bytes(buf)


def transact(node, report, timeout=1.5):
    """Write a report, then return the first input report the device sends back."""
    fd = os.open(node, os.O_RDWR | os.O_NONBLOCK)
    try:
        # drain stale input so we do not report an old reply
        while True:
            r, _, _ = select.select([fd], [], [], 0.05)
            if not r or not os.read(fd, 128):
                break
        os.write(fd, report)
        deadline = time.time() + timeout
        while time.time() < deadline:
            r, _, _ = select.select([fd], [], [], max(0.0, deadline - time.time()))
            if not r:
                continue
            data = os.read(fd, 128)
            if data:
                return data
        return None
    finally:
        os.close(fd)


def hexdump(data, width=16, indent="    "):
    for off in range(0, len(data), width):
        chunk = data[off:off + width]
        h = " ".join(f"{b:02x}" for b in chunk).ljust(width * 3 - 1)
        t = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
        print(f"{indent}{off:04x}  {h}  |{t}|")


def decode_key(idx, t, c1, c2, c3):
    kind = KEY_TYPES.get(t, f"type{t}")
    if t == 32:                                   # Standard keyboard
        m = "+".join(v for k, v in MODS.items() if c1 & k)
        return f"{kind:20s} code=0x{c2:02x} mods={c1:02x}({m or '-'})"
    if t == 48:                                   # consumer/media: u32 LE usage
        usage = c1 | (c2 << 8) | (c3 << 16)
        from probe_map import CONSUMER
        return f"{kind:20s} usage=0x{usage:04x} [{CONSUMER.get(usage, '?')}]"
    if t == 96:
        return f"{kind:20s} buttons=0x{c1:02x}"
    if t == 17:                                   # MouseMove
        up = ((c1 & 7) << 8) | c3
        lr = ((c1 & 0x70) << 8) | c2
        d = []
        if c1 & 0x08:
            d.append(f"up {up}")
        elif up:
            d.append(f"down {up}")
        if c1 & 0x80:
            d.append(f"left {lr}")
        elif lr:
            d.append(f"right {lr}")
        return f"{kind:20s} " + ", ".join(d or ["(none)"])
    if t == 112:
        return f"{kind:20s} code1={c1} code2={c2}"
    return f"{kind:20s} c1=0x{c1:02x} c2=0x{c2:02x} c3=0x{c3:02x}"


def cmd_info(node):
    rep = build(0x05)
    print("--> " + rep[:8].hex(" ") + " ...   (cmd 0x05 get keyboard config)")
    reply = transact(node, rep)
    if reply is None:
        print("!! no reply")
        return
    print(f"<-- {len(reply)} bytes")
    hexdump(reply)
    if len(reply) > 5:
        cfg = reply[5:43]
        print("    parsed fields (offset 5..):")
        names = ["version_lo", "version_hi", "pid_lo", "pid_hi", "fw_lo", "fw_hi",
                 "workMode", "linkStatus", "battery", "charge", "profileCnt", "profile",
                 "layerCnt", "layer", "autoSleep_lo", "autoSleep_hi"]
        for i, b in enumerate(cfg):
            print(f"      [{i:2d}] {names[i] if i < len(names) else '?':16s} = {b:3d} (0x{b:02x})")


def read_chunk(node, cmd, offset, layer, length):
    rep = build(cmd, offset=offset, layer=layer, length=length)
    reply = transact(node, rep)
    if reply is None:
        return None
    return reply[8:8 + 56] if len(reply) >= 8 else b""


def cmd_keys(node, layer):
    print(f"reading key layout, layer {layer}")
    for chunk in range(2):
        rep = build(0x08, offset=chunk * 56, layer=layer, length=58)
        print(f"\n--> {rep[:8].hex(' ')} ...  (cmd 0x08 len=58 off={chunk*56})")
        reply = transact(node, rep)
        if reply is None:
            print("!! no reply")
            return
        print(f"<-- {len(reply)} bytes")
        hexdump(reply)
        if len(reply) >= 8:
            body = reply[8:8 + 56]
            for i in range(0, len(body) - 3, 4):
                idx = chunk * 14 + i // 4
                t, c1, c2, c3 = body[i:i + 4]
                print(f"      key[{idx:2d}] {decode_key(idx, t, c1, c2, c3)}")


def cmd_sweep(node, layer):
    """Scan the whole layout space for chunks that are not the all-0x13 filler.

    The vendor reads 576 bytes in 56-byte chunks; every chunk so far comes back
    as 13 00 00 00 repeated, so the real key table must be elsewhere (or the
    filler means 'unassigned' and the pad is on firmware defaults).
    """
    filler = bytes([0x13, 0, 0, 0] * 14)
    print(f"sweeping offsets 0..575 (cmd 0x07/len 56 and cmd 0x08/len 58), layer {layer}\n")
    for cmd, length in ((0x07, 56), (0x08, 58)):
        print(f"### cmd 0x{cmd:02x}, length byte {length}")
        for off in range(0, 576, 56):
            body = read_chunk(node, cmd, off, layer, length)
            if body is None:
                print(f"  off {off:3d}: no reply")
                continue
            flat = body == filler
            groups = [body[i:i + 4] for i in range(0, min(len(body), 56) - 3, 4)]
            summary = " ".join(f"{g.hex()}" for g in groups[:4])
            print(f"  off {off:3d}: {'FILLER' if flat else 'DATA  '} {summary}"
                  f"{' ...' if len(groups) > 4 else ''}")
        print()


SINGLE_CMD = 0x10   # vendor's single-key write (setKeyInfo); 0x09 is the bulk write


def cmd_test(node, index, t, c1, c2, c3, layer, cmd=SINGLE_CMD):
    """Write one key, then watch what the pad actually emits."""
    off = 4 * index
    before = read_chunk(node, 0x08, off, layer, 58)
    print(f"key[{index}] before: {before[:8].hex(' ') if before else 'no reply'}")
    payload = bytes([t, c1, c2, c3])
    rep = build(cmd, offset=off, layer=layer, payload=payload, length=7)
    print("--> " + rep[:14].hex(" ") + f" ...  (cmd 0x{cmd:02x}, type={t} c1={c1:#04x} c2={c2:#04x} c3={c3:#04x})")
    reply = transact(node, rep)
    print(f"<-- {'no reply' if reply is None else reply[:14].hex(' ')}")
    after = read_chunk(node, 0x08, off, layer, 58)
    print(f"key[{index}] after : {after[:8].hex(' ') if after else 'no reply'}"
          f"   {'CHANGED' if before != after else '(unchanged)'}")
    print()
    from probe_map import wait_press
    got = wait_press(30, "press that control now")
    if got:
        print(f"  emitted: {got[2].hex(' ')}")


def cmd_map(node, layer, count):
    """Write a distinct keycode to each index, then identify each physical control.

    Simultaneously proves that writes take effect and reveals the
    key_index -> physical control mapping that the vendor's layout JSON encodes.
    """
    from probe_map import wait_press, decode

    print(f"writing index -> keycode 0x04.. (a, b, c, ...) for {count} indices on layer {layer}\n")
    for i in range(count):
        payload = bytes([32, 0, 0x04 + i, 0])
        rep = build(SINGLE_CMD, offset=4 * i, layer=layer, payload=payload, length=7)
        reply = transact(node, rep)
        print(f"  index {i:2d} -> keycode 0x{0x04+i:02x} ({chr(ord('a')+i)})   "
              f"cmd 0x{SINGLE_CMD:02x} reply: {'none' if reply is None else reply[:12].hex(' ')}")

    print("\nverify what was stored:")
    for i in range(count):
        body = read_chunk(node, 0x08, 4 * i, layer, 58)
        got = body[:4].hex(" ") if body else "no reply"
        print(f"  index {i:2d}: {got}")

    print("\nNow press each physical control in turn.")
    print("Each press should emit the letter assigned to its index.\n")
    seen = {}
    for n in range(count):
        got = wait_press(45, f"press physical control {n+1}")
        if got is None:
            break
        kind, data = got[1], got[2]
        print(f"    emitted {data.hex(' ')}   {decode(kind, data)}\n")
        seen[n] = data.hex(" ")
    print("\nsummary of what each press emitted:")
    for n, v in seen.items():
        print(f"  physical control {n+1}: {v}")
    print("\nmatch a pressed control's code back to its index: the letter it typed")
    print("tells you which index it is (a=index0, b=index1, ...)")


def cmd_write(node, index, t, c1, c2, c3, layer):
    off = 4 * index
    payload = bytes([t, c1, c2, c3])
    rep = build(0x09, offset=off, layer=layer, payload=payload, length=7)
    print("--> " + rep[:12].hex(" ") + " ...  (cmd 0x09 single-key write)")
    hexdump(rep[:16])
    reply = transact(node, rep)
    print(f"<-- {'no reply' if reply is None else reply.hex(' ')}")


def main():
    node = find_vendor_node()
    if node is None:
        print(f"vendor interface for {VID:04x}:{PID:04x} not found")
        return 1
    print(f"vendor interface: {node}\n")
    argv = sys.argv[1:]
    if not argv or argv[0] == "info":
        cmd_info(node)
    elif argv[0] == "keys":
        cmd_keys(node, int(argv[1], 10) if len(argv) > 1 else 1)
    elif argv[0] == "map":
        # map [count] [layer]
        cmd_map(node, int(argv[2], 10) if len(argv) > 2 else 1,
                int(argv[1], 10) if len(argv) > 1 else 6)
    elif argv[0] == "sweep":
        cmd_sweep(node, int(argv[1], 10) if len(argv) > 1 else 1)
    elif argv[0] == "test":
        # test <key_index> <type> <code1> <code2> <code3> [layer]
        # test <index> <type> <c1> <c2> <c3> [layer] [cmd]
        cmd_test(node, int(argv[1], 10), int(argv[2], 10),
                 int(argv[3], 16), int(argv[4], 16), int(argv[5], 16),
                 int(argv[6], 10) if len(argv) > 6 else 1,
                 int(argv[7], 16) if len(argv) > 7 else SINGLE_CMD)
    else:
        print(__doc__)
    return 0


if __name__ == "__main__":
    sys.exit(main())
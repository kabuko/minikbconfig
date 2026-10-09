#!/usr/bin/env python3
"""
Watch what the SIDE-KEYBOARD (6d7b:dcfa) actually emits, and try to reprogram it.

    sudo python3 probe_map.py watch 20           # decode presses on the normal interface
    sudo python3 probe_map.py key 1 04 --listen   # slot 1 -> 'a', then watch
    sudo python3 probe_map.py key 13 e9 00       # knob-ccw -> volume up
    sudo python3 probe_map.py led 1 --listen
    sudo python3 probe_map.py send aa aa         # raw command bytes

The "normal" input interface is hidraw7 (report IDs 1..5); the vendor config
interface is hidraw9 (64-byte unnumbered reports).
"""

import os
import select
import sys
import threading
import time

VID, PID = 0x6D7B, 0xDCFA

KEYS = {
    0x04: "A", 0x05: "B", 0x06: "C", 0x07: "D", 0x08: "E", 0x09: "F", 0x0A: "G",
    0x0B: "H", 0x0C: "I", 0x0D: "J", 0x0E: "K", 0x0F: "L", 0x10: "M", 0x11: "N",
    0x12: "O", 0x13: "P", 0x14: "Q", 0x15: "R", 0x16: "S", 0x17: "T", 0x18: "U",
    0x19: "V", 0x1A: "W", 0x1B: "X", 0x1C: "Y", 0x1D: "Z", 0x1E: "1", 0x1F: "2",
    0x20: "3", 0x21: "4", 0x22: "5", 0x23: "6", 0x24: "7", 0x25: "8", 0x26: "9",
    0x27: "0", 0x28: "ENTER", 0x29: "ESC", 0x2A: "BKSP", 0x2B: "TAB", 0x2C: "SPACE",
    0x2D: "MINUS", 0x2E: "EQUAL", 0x2F: "LBRACK", 0x30: "RBRACK", 0x31: "BSLASH",
    0x33: "SEMI", 0x34: "QUOTE", 0x35: "GRAVE", 0x36: "COMMA", 0x37: "DOT",
    0x38: "SLASH", 0x39: "CAPS", 0x3A: "F1", 0x3B: "F2", 0x3C: "F3", 0x3D: "F4",
    0x3E: "F5", 0x3F: "F6", 0x40: "F7", 0x41: "F8", 0x42: "F9", 0x43: "F10",
    0x44: "F11", 0x45: "F12", 0x46: "PRTSC", 0x47: "SCROLL", 0x48: "PAUSE",
    0x49: "INS", 0x4A: "HOME", 0x4B: "PGUP", 0x4C: "DEL", 0x4D: "END", 0x4E: "PGDN",
    0x4F: "RIGHT", 0x50: "LEFT", 0x51: "DOWN", 0x52: "UP",
    0x53: "NUM", 0x54: "KP/", 0x55: "KP*", 0x56: "KP-", 0x57: "KP+", 0x58: "KPENT",
    0x59: "KP1", 0x5A: "KP2", 0x5B: "KP3", 0x5C: "KP4", 0x5D: "KP5", 0x5E: "KP6",
    0x5F: "KP7", 0x60: "KP8", 0x61: "KP9", 0x62: "KP0", 0x63: "KPDOT",
    0x64: "NONUSBS", 0x65: "APP", 0x66: "POWER", 0x67: "KPEQ",
    0x68: "F13", 0x69: "F14", 0x6A: "F15", 0x6B: "F16", 0x6C: "F17", 0x6D: "F18",
    0x6E: "F19", 0x6F: "F20", 0x70: "F21", 0x71: "F22", 0x72: "F23", 0x73: "F24",
    0x6F: "BRIGHT_UP", 0x70: "BRIGHT_DOWN",
    0x74: "EXECUTE", 0x75: "HELP", 0x76: "MENU", 0x77: "SELECT", 0x78: "STOP",
    0x79: "AGAIN", 0x7A: "UNDO", 0x7B: "CUT", 0x7C: "COPY", 0x7D: "PASTE",
    0x7E: "FIND", 0x7F: "MUTE_KB", 0x80: "VOL_UP_KB", 0x81: "VOL_DOWN_KB",
    0x7E: "KBD_RESETHALF", 0x7F: "KBD_RESETFULL",
}
MODS = {0x01: "LCTRL", 0x02: "LSHIFT", 0x04: "LALT", 0x08: "LWIN",
        0x10: "RCTRL", 0x20: "RSHIFT", 0x40: "RALT", 0x80: "RWIN"}
CONSUMER = {0xB5: "NEXT", 0xB6: "PREV", 0xB7: "STOP", 0xCD: "PLAY/PAUSE",
            0xE2: "MUTE", 0xE9: "VOL_UP", 0xEA: "VOL_DOWN",
            0x6F: "BRIGHT_UP", 0x70: "BRIGHT_DOWN",
            0x182: "FAVORITES", 0x192: "CALCULATOR", 0x19E: "SCREEN_LOCK"}
SYSTEM = {0x81: "POWER_DOWN", 0x82: "SLEEP", 0x83: "WAKE"}


def sysfs_dir(node):
    return os.path.realpath(os.path.join("/sys/class/hidraw", os.path.basename(node), "device"))


def find_node(predicate):
    import glob

    def usb_ids(path):
        while path and path != "/":
            vend = os.path.join(path, "idVendor")
            prod = os.path.join(path, "idProduct")
            if os.path.exists(vend) and os.path.exists(prod):
                return (int(open(vend).read().strip(), 16),
                        int(open(prod).read().strip(), 16))
            path = os.path.dirname(path)
        return None

    for node in sorted(glob.glob("/dev/hidraw*")):
        path = sysfs_dir(node)
        if usb_ids(path) != (VID, PID):
            continue
        try:
            desc = open(os.path.join(path, "report_descriptor"), "rb").read()
        except OSError:
            desc = b""
        if predicate(desc):
            return node
    return None


def decode(kind, report):
    """Decode one input report. kind is 'reportid' (interface 0) or 'boot' (interface 1)."""
    if kind == "boot":
        if len(report) < 2:
            return f"boot short {report.hex(' ')}"
        mods = report[0]
        codes = [c for c in report[2:8] if c]
        return f"KEY   mods={mods:02x}({mods_str(mods) or '-'}) keys={codes} [{keys_str(codes)}]"
    rid = report[0]
    data = report[1:]
    if rid == 1:  # keyboard: 8 modifier bits, 1 pad byte, then keycodes
        if len(data) < 1:
            return f"report1 short {report.hex(' ')}"
        mods = data[0]
        codes = [c for c in data[2:8] if c]
        return f"KEY   mods={mods:02x}({mods_str(mods) or '-'}) codes={codes} [{keys_str(codes)}]"
    if rid == 2:  # mouse: 5 buttons + pad, X i16, Y i16, wheel i8, acpan i8
        if len(data) < 7:
            return f"report2 short {report.hex(' ')}"
        buttons = data[0] & 0x1F
        x = int.from_bytes(data[1:3], "little", signed=True)
        y = int.from_bytes(data[3:5], "little", signed=True)
        wheel = int.from_bytes(data[5:6], "little", signed=True)
        pan = int.from_bytes(data[6:7], "little", signed=True)
        return (f"MOUSE buttons={buttons:02x} x={x} y={y} wheel={wheel} acpan={pan}"
                if (buttons or x or y or wheel or pan) else "MOUSE  (all zero / release)")
    if rid in (3, 5):  # consumer control: 16-bit usage, little endian, no offset
        if len(data) < 2:
            return f"report{rid} short {report.hex(' ')}"
        usage = int.from_bytes(data[0:2], "little")
        return f"CONSUMER usage=0x{usage:04x} [{CONSUMER.get(usage, '?')}]"
    if rid == 4:  # system control: 3 bits
        usage = data[0] & 0x07
        return f"SYSTEM  usage=0x{usage:02x} [{SYSTEM.get(usage, '?')}]"
    return f"report_id={rid} raw={report.hex(' ')}"


def mods_str(m):
    return "+".join(v for k, v in MODS.items() if m & k)


def keys_str(codes):
    return "+".join(KEYS.get(c, f"0x{c:02x}") for c in codes)


def input_nodes():
    """All non-vendor interfaces of this device: report-ID (iface 0) and boot (iface 1)."""
    import glob

    def usb_ids(path):
        while path and path != "/":
            vend = os.path.join(path, "idVendor")
            prod = os.path.join(path, "idProduct")
            if os.path.exists(vend) and os.path.exists(prod):
                return (int(open(vend).read().strip(), 16),
                        int(open(prod).read().strip(), 16))
            path = os.path.dirname(path)
        return None

    out = []
    for node in sorted(glob.glob("/dev/hidraw*")):
        path = sysfs_dir(node)
        if usb_ids(path) != (VID, PID):
            continue
        try:
            desc = open(os.path.join(path, "report_descriptor"), "rb").read()
        except OSError:
            desc = b""
        if desc[:2] == b"\x06\x00":  # vendor config interface
            continue
        out.append((node, "reportid" if len(desc) > 100 else "boot"))
    return out


def watch(seconds, label=""):
    nodes = input_nodes()
    if not nodes:
        print("no input interfaces found")
        return
    fds = {os.open(n, os.O_RDONLY | os.O_NONBLOCK): (n, k) for n, k in nodes}
    print(f"watching {', '.join(n for n, _ in nodes)} for {seconds:.0f}s {label}"
          f" -- press keys / turn the knob")
    deadline = time.time() + seconds
    try:
        while time.time() < deadline:
            ready, _, _ = select.select(list(fds), [], [], max(0.0, deadline - time.time()))
            for fd in ready:
                node, kind = fds[fd]
                data = os.read(fd, 128)
                if not data:
                    continue
                print(f"  {node[-8:]:>8}  {data.hex(' '):<36} {decode(kind, data)}")
    finally:
        for fd in fds:
            os.close(fd)


def send(payloads):
    node = find_node(lambda d: len(d) < 100 and d[:2] == b"\x06\x00")
    if node is None:
        print("vendor config interface (usage page 0xFF00) not found")
        return
    fd = os.open(node, os.O_WRONLY)
    try:
        for payload in payloads:
            os.write(fd, payload + bytes(64 - len(payload)))
            print(f"  -> {payload.hex(' ')}")
            time.sleep(0.02)
    finally:
        os.close(fd)


def parse_hex(text):
    return bytes.fromhex(text.replace(" ", "").replace("0x", ""))


def slot(text):
    """Key slot is decimal in every tool for this hardware (1..3, 13..15)."""
    return int(text, 10)


def code(text):
    """Keycode / usage / mode is hex; base 16 so '04' and '0x2c' both work."""
    return int(text, 16)


def key_command(slot, code, mods=0x00):
    """CH55x sequence for one key slot -> one keystroke, report-id-0 encoding."""
    return [
        bytes([slot, 0x01, 0x01, 0x00, mods, 0x00, 0x00, 0x00]),   # header entry
        bytes([slot, 0x01, 0x01, 0x01, mods, code, 0x00, 0x00]),   # the keystroke
        bytes([0xAA, 0xAA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),   # commit
    ]


def media_command(slot, usage):
    return [
        bytes([slot, 0x02, usage & 0xFF, usage >> 8, 0, 0, 0, 0]),
        bytes([0xAA, 0xAA, 0, 0, 0, 0, 0, 0]),
    ]


def is_release(kind, data):
    """True for the all-zero 'key up' report, so we can ignore it and wait for a real press."""
    if kind == "boot":
        return len(data) < 2 or not any(data[:8])
    rid, body = data[0], data[1:]
    if rid in (3, 5):
        return len(body) < 2 or not any(body[0:2])
    if rid == 2:
        return len(body) < 7 or not any(body[:7])
    return not any(data)


def wait_press(seconds, label):
    """Block until the pad emits a real press. Returns (node, kind, data) or None."""
    nodes = input_nodes()
    fds = {os.open(n, os.O_RDONLY | os.O_NONBLOCK): (n, k) for n, k in nodes}
    print(f"  {label}: press a control now (waiting up to {seconds:.0f}s)...")
    deadline = time.time() + seconds
    hit = None
    try:
        while time.time() < deadline and hit is None:
            ready, _, _ = select.select(list(fds), [], [], max(0.0, deadline - time.time()))
            for fd in ready:
                node, kind = fds[fd]
                data = os.read(fd, 128)
                if not data or is_release(kind, data):
                    continue
                print(f"  {label}: got {node[-8:]}  {data.hex(' ')}")
                hit = (node, kind, data)
    finally:
        for fd in fds:
            os.close(fd)
    if hit is None:
        print(f"  {label}: TIMEOUT, nothing pressed")
    return hit


def main():
    argv = sys.argv[1:]
    before, after = 5.0, 8.0
    for flag in ("--before", "--after"):
        if flag in argv:
            i = argv.index(flag)
            argv.pop(i)
            if i < len(argv) and _is_num(argv[i]):
                before = float(argv.pop(i)) if flag == "--before" else before
                if flag == "--after":
                    after = float(argv[i])
                    argv.pop(i)
    argv = [a for a in argv if a not in ("--listen", "--now")]

    if not argv:
        print(__doc__)
        return 1

    cmd = argv.pop(0)
    payloads = []
    if cmd == "watch":
        watch(float(argv[0]) if argv else 20)
        return 0

    if cmd == "try":
        target_slot, target_code = slot(argv[0]), code(argv[1])
        before = wait_press(float(argv[2]) if len(argv) > 2 else 30, "BEFORE")
        print("writing:")
        send(key_command(target_slot, target_code))
        time.sleep(0.4)
        after = wait_press(float(argv[2]) if len(argv) > 2 else 30, "AFTER")
        print()
        print(f"  slot {target_slot} -> 0x{target_code:02x}")
        if before and after:
            same = before[2] == after[2]
            print(f"  before: {before[2].hex(' ')}  {decode(before[1], before[2])}")
            print(f"  after : {after[2].hex(' ')}  {decode(after[1], after[2])}")
            print()
            if not same:
                print("  => MAPPING CHANGED: the CH55x protocol works on this device.")
            else:
                print("  => unchanged: the write did not take effect.")
        return 0

    if cmd == "key":
        payloads = key_command(slot(argv[0]), code(argv[1]),
                               code(argv[2]) if len(argv) > 2 else 0)
    elif cmd == "media":
        payloads = media_command(slot(argv[0]), code(argv[1]))
    elif cmd == "led":
        payloads = [bytes([0xB0, 0x18, code(argv[0]), 0, 0, 0, 0, 0]),
                    bytes([0xAA, 0xA1, 0, 0, 0, 0, 0, 0])]
    elif cmd == "send":
        payloads = [parse_hex(a) for a in argv]
    else:
        print(__doc__)
        return 1

    if before:
        watch(before, "(BEFORE -- press the control you are about to change)")
    print("writing:")
    send(payloads)
    time.sleep(0.3)
    if after:
        watch(after, "(AFTER -- press the same control again)")
    return 0


def _is_num(s):
    try:
        float(s)
        return True
    except ValueError:
        return False


if __name__ == "__main__":
    sys.exit(main())
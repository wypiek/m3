#!/usr/bin/env python3
"""Minimal QMP client for driving the m3 test VM.

    qmp.py SOCK screendump FILE.png
    qmp.py SOCK type "text"          # types ASCII text (US layout)
    qmp.py SOCK key ctrl alt f2      # a key chord (QEMU qcode names)
    qmp.py SOCK move X Y             # absolute pointer, 0..32767
    qmp.py SOCK click [left|right]
    qmp.py SOCK quit
"""
import json, socket, sys, time

def connect(path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(path)
    f = s.makefile("rw")
    json.loads(f.readline())  # greeting
    cmd(f, "qmp_capabilities")
    return f

def cmd(f, name, **args):
    f.write(json.dumps({"execute": name, "arguments": args}) + "\n")
    f.flush()
    while True:
        r = json.loads(f.readline())
        if "event" in r:
            continue
        if "error" in r:
            raise SystemExit(f"{name}: {r['error']['desc']}")
        return r.get("return")

SHIFTED = {'!': '1', '@': '2', '#': '3', '$': '4', '%': '5', '^': '6', '&': '7', '*': '8', '(': '9', ')': '0',
           '_': 'minus', '+': 'equal', '{': 'bracket_left', '}': 'bracket_right', ':': 'semicolon',
           '"': 'apostrophe', '<': 'comma', '>': 'dot', '?': 'slash', '|': 'backslash', '~': 'grave_accent'}
PLAIN = {' ': 'spc', '-': 'minus', '=': 'equal', '[': 'bracket_left', ']': 'bracket_right', ';': 'semicolon',
         "'": 'apostrophe', ',': 'comma', '.': 'dot', '/': 'slash', '\\': 'backslash', '`': 'grave_accent',
         '\n': 'ret'}

def keys(f, names):
    ev = lambda n, d: {"type": "key", "data": {"down": d, "key": {"type": "qcode", "data": n}}}
    cmd(f, "input-send-event", events=[ev(n, True) for n in names])
    time.sleep(0.03)
    cmd(f, "input-send-event", events=[ev(n, False) for n in reversed(names)])
    time.sleep(0.03)

def main():
    f = connect(sys.argv[1])
    op, args = sys.argv[2], sys.argv[3:]
    if op == "screendump":
        cmd(f, "screendump", filename=args[0], format="png")
    elif op == "type":
        for ch in args[0]:
            if ch.isalpha():
                keys(f, (["shift"] if ch.isupper() else []) + [ch.lower()])
            elif ch.isdigit():
                keys(f, [ch])
            elif ch in SHIFTED:
                keys(f, ["shift", SHIFTED[ch]])
            else:
                keys(f, [PLAIN[ch]])
    elif op == "key":
        keys(f, args)
    elif op == "move":
        cmd(f, "input-send-event", events=[
            {"type": "abs", "data": {"axis": "x", "value": int(args[0])}},
            {"type": "abs", "data": {"axis": "y", "value": int(args[1])}}])
    elif op == "click":
        b = args[0] if args else "left"
        for down in (True, False):
            cmd(f, "input-send-event", events=[{"type": "btn", "data": {"down": down, "button": b}}])
            time.sleep(0.05)
    elif op == "quit":
        cmd(f, "quit")
    else:
        raise SystemExit(__doc__)

main()

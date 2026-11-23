# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""QMP input for the VM (Sodium Remote's tests): keys and the pointer, through the VM's own input devices.
  qmp.py keys ctrl+alt+shift+esc     (qcodes joined by +)
  qmp.py move X Y                    (screen pixels, 1920x1080)
  qmp.py click X Y                   (move, then press and release the left button)
"""
import json
import os
import socket
import sys
import time

s = socket.socket(socket.AF_UNIX)
s.connect(os.path.expanduser("~/VMs/windows/qmp.sock"))
f = s.makefile("rw")
f.readline()
f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n")
f.flush()
f.readline()


def cmd(c, a=None):
    f.write(json.dumps({"execute": c, **({"arguments": a} if a else {})}) + "\n")
    f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r:
            return r


def move(x, y):
    ax, ay = x * 32767 // 1919, y * 32767 // 1079
    return cmd("input-send-event", {"events": [{"type": "abs", "data": {"axis": "x", "value": ax}}, {"type": "abs", "data": {"axis": "y", "value": ay}}]})


a = sys.argv[1:]
if a[0] == "keys":
    print(cmd("send-key", {"keys": [{"type": "qcode", "data": k} for k in a[1].split("+")], "hold-time": 80}))
elif a[0] == "move":
    print(move(int(a[1]), int(a[2])))
elif a[0] == "rclick":
    print(move(int(a[1]), int(a[2])))
    time.sleep(0.2)
    print(cmd("input-send-event", {"events": [{"type": "btn", "data": {"button": "right", "down": True}}]}))
    time.sleep(0.1)
    print(cmd("input-send-event", {"events": [{"type": "btn", "data": {"button": "right", "down": False}}]}))
elif a[0] == "click":
    print(move(int(a[1]), int(a[2])))
    time.sleep(0.2)
    print(cmd("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": True}}]}))
    time.sleep(0.1)
    print(cmd("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": False}}]}))

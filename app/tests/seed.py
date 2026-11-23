#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Test data for the nested bed's screenshots (tests/e2e.sh): four made-up computers with what the app learned about
them (system, last route, round trip) in app.json and a small picture each, so the list looks like the design board.
Never used outside the bed.

    seed.py DATADIR
"""
import json
import os
import sys

from PIL import Image, ImageDraw

data = sys.argv[1]
os.makedirs(os.path.join(data, 'thumbs'), exist_ok=True)

HOSTS = {
    'FAKE-STUDIO': ('windows', 'lan', 2.0, ((14, 70, 90), (40, 130, 150))),
    'FAKE-MAC': ('mac', 'relay', 26.0, ((40, 30, 80), (90, 70, 150))),
    'FAKE-LAB': ('windows', 'direct', 9.0, ((20, 60, 80), (60, 120, 140))),
    'FAKE-OLD': ('linux', 'lan', 0.0, ((10, 10, 20), (30, 30, 50))),
}


def picture(path, colours):
    w, h = 480, 300
    img = Image.new('RGB', (w, h))
    d = ImageDraw.Draw(img)
    (r0, g0, b0), (r1, g1, b1) = colours
    for y in range(h):
        t = y / (h - 1)
        d.line([(0, y), (w, y)], fill=(int(r0 + (r1 - r0) * t), int(g0 + (g1 - g0) * t), int(b0 + (b1 - b0) * t)))
    d.rounded_rectangle((60, 40, 300, 220), 8, fill=(34, 36, 46))
    d.rectangle((60, 40, 300, 62), fill=(46, 48, 60))
    for i in range(6):
        d.rectangle((76, 80 + i * 20, 240 - (i % 3) * 30, 88 + i * 20), fill=(86, 90, 110))
    d.rounded_rectangle((200, 100, 420, 260), 8, fill=(28, 30, 38))
    d.rounded_rectangle((350, 232, 396, 246), 6, fill=(150, 150, 255))
    d.rounded_rectangle((120, 270, 360, 288), 9, fill=(60, 62, 76))
    img.save(path)


hosts = {}
for hid, (os_, route, rtt, colours) in HOSTS.items():
    hosts[hid] = {'os': os_, 'route': route, 'rtt': rtt}
    picture(os.path.join(data, 'thumbs', hid + '.png'), colours)
json.dump({'settings': {}, 'hosts': hosts}, open(os.path.join(data, 'app.json'), 'w'))

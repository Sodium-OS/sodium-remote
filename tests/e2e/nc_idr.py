#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""netchange.sh run: the host's IDRs (keyframes in host-frames.csv) in ms after each event, the client's keyframe
requests (its summary.json), and the longest gap in the frames the client got around each event.
   python3 -I nc_idr.py RUN"""
import csv
import glob
import json
import sys

run = sys.argv[1]
ev = {}
for ln in list(open(f"{run}/events.csv"))[1:]:
    k, v = ln.strip().split(",")
    ev[k] = int(v)
marks = {k: v for k, v in ev.items() if k.startswith(("cut", "back", "move"))}
rows = list(csv.DictReader(open(f"{run}/host/host-frames.csv")))
print("host IDRs (frame, class, bytes) at ms after each event:")
for r in rows:
    if r["keyframe"] == "1":
        t = int(r["t_sent_ns"])
        print("  ", r["frame_id"], r["class"], r["bytes"], {k: round((t - v) / 1e6) for k, v in marks.items() if 0 <= t - v < 5e9})
sj = json.load(open(glob.glob(f"{run}/client/session-*-summary.json")[0]))
print("client summary keys with 'key' or 'request':", {k: v for k, v in sj.items() if "key" in k.lower() or "request" in k.lower()})
print("client decode_errors", sj.get("decode_errors"), "frames_lost", sj.get("frames_lost"), "frames_decoded", sj.get("frames_decoded"))

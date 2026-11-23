#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Present intervals of a run's presented frames (client frames.csv): how many are over 1.5 refreshes (late or
duplicated), and how many decoded frames were superseded before a vsync.

Intervals that span a host render hole are left out of that count and the host's holes are printed on a line of their own: a hole is render_holes.py's, a gap over 40 ms between the t_rendered of the non-refine frames in
host-frames.csv (the host compositor presented the same picture for that long: the bed's stall, not the client's). An
interval spans a hole when the frame ids it covers (previous presented id, this id] overlap the hole's (id before, id after].
   python3 -I present_gaps.py RUN REFRESH_HZ [TRIM_START_S TRIM_END_S]"""
import csv
import glob
import sys

run, hz = sys.argv[1], float(sys.argv[2])
rows = list(csv.DictReader(ln for ln in open(glob.glob(f"{run}/client/session-*-frames.csv")[0]) if not ln.startswith("#")))
shown = [(int(r["present_us"]), int(r["frame_id"])) for r in rows if r["present_us"]]
if len(sys.argv) > 3:  # TRIM_START_S TRIM_END_S: leave out the start-up and the stop phase
    shown = [p for p in shown if shown[0][0] + float(sys.argv[3]) * 1e6 <= p[0] <= shown[-1][0] - float(sys.argv[4]) * 1e6]

host = list(csv.DictReader(open(f"{run}/host/host-frames.csv")))
live = [r for r in host if r["class"] != "2"]
holes = [(int(a["frame_id"]), int(b["frame_id"]), (int(b["t_rendered_ns"]) - int(a["t_rendered_ns"])) / 1e6)
         for a, b in zip(live, live[1:]) if int(b["t_rendered_ns"]) - int(a["t_rendered_ns"]) > 40e6]
t0 = int(live[0]["t_rendered_ns"]) if live else 0
refine = sum(r["class"] == "2" for r in host)

period = 1000 / hz
gaps, spans = [], 0
for (pa, ia), (pb, ib) in zip(shown, shown[1:]):
    if any(ia < hb and ib > ha for ha, hb, _ in holes):
        spans += 1
        continue
    gaps.append((pb - pa) / 1000)
late = [g for g in gaps if g > 1.5 * period]
print(f"{run}: {len(rows)} decoded, {len(shown)} presented, {len(rows) - len(shown)} superseded; "
      f"{len(late)} of {len(gaps)} present intervals over 1.5 refreshes ({1.5 * period:.1f} ms), "
      f"{spans} more spanning a host hole left out; max {max(gaps, default=0):.1f} ms")
tr = {int(r["frame_id"]): int(r["t_rendered_ns"]) for r in live}
print(f"   host: {len(host)} frames, {refine} refine; {len(holes)} render holes over 40 ms "
      f"(time s, ms): {[(round((tr[hb] - t0) / 1e9, 2), round(ms)) for _, hb, ms in holes[:20]]}")

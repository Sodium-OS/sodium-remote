#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Where the host stopped producing new pictures, and what its refinement frames were: from a run's host-frames.csv.
  - render holes: gaps over 40 ms between the plugin render times (t_rendered) of the non-refine frames. A hole with the
    plugin still sending a FRAME every vsync (host/plugin-debug.json frames) means frames whose pixels were identical
    (remoted drops those as "damaged but identical"), i.e. the host compositor presented the same picture for that long;
  - refine frames: count, bytes, and the base QP media recovered for them (base_qp is the rate controller's QP for the frame's
    coded cells, 51 = it can't tell; `qp` is NVENC's average over all cells, which the +28/+30 skip cells push to 35-50).
python3 -I render_holes.py RUN [RUN...]"""
import collections
import csv
import sys

for run in sys.argv[1:]:
    rows = list(csv.DictReader(open(f"{run}/host/host-frames.csv")))
    live = [r for r in rows if r["class"] != "2"]
    t0 = int(live[0]["t_rendered_ns"])
    holes = [(round((int(b["t_rendered_ns"]) - t0) / 1e9, 2), round((int(b["t_rendered_ns"]) - int(a["t_rendered_ns"])) / 1e6))
             for a, b in zip(live, live[1:]) if int(b["t_rendered_ns"]) - int(a["t_rendered_ns"]) > 40e6]
    ref = [r for r in rows if r["class"] == "2"]
    print(f"{run}: {len(rows)} frames, {len(ref)} refine ({sum(int(r['bytes']) for r in ref)} B), "
          f"base QP of refine frames {sorted(collections.Counter(int(r['base_qp']) for r in ref).items())[:8]}")
    print(f"   {len(holes)} render holes over 40 ms (time s, ms): {holes[:20]}")

#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Bitrate of a run from the host's frame log: per-second frames and bytes by class (0 passive, 1 interactive, 2 refine),
and the totals/percentiles over the click phase.   python3 -I rate_by_class.py RUN [RUN...]"""
import collections
import csv
import sys

for run in sys.argv[1:]:
    rows = list(csv.DictReader(open(f"{run}/host/host-frames.csv")))
    t0 = int(rows[0]["t_sent_ns"])
    per = collections.defaultdict(lambda: collections.defaultdict(lambda: [0, 0]))
    for r in rows:
        s = (int(r["t_sent_ns"]) - t0) // 10**9
        per[s][r["class"]][0] += 1
        per[s][r["class"]][1] += int(r["bytes"])
    secs = sorted(per)
    kbps = [sum(v[1] for v in per[s].values()) * 8 / 1000 for s in secs[2:-2]]
    kbps.sort()
    cls = {c: [sum(per[s][c][0] for s in secs), sum(per[s][c][1] for s in secs)] for c in "012"}
    print(f"{run}: {len(rows)} frames over {len(secs)} s; by class (frames, bytes) {cls}")
    if kbps:
        n = len(kbps)
        print(f"   per-second kbit/s (without the first and last 2 s): min {kbps[0]:.0f}  p50 {kbps[n // 2]:.0f}  p95 {kbps[int(n * .95)]:.0f}  max {kbps[-1]:.0f}; mean {sum(kbps) / n:.0f}")
        print("   frames/s by class, first 14 s:", [(s, {c: per[s][c][0] for c in "012" if per[s][c][0]}) for s in secs[:14]])

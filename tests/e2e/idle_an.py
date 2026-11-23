#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Idle-bandwidth study, one run: python3 -I idle_an.py STUDYDIR N  (pkt-N.csv from pktlog.py next to run-N/).
Host -> client datagrams (the biggest UDP flow on lo) in 100 ms bins after the click test's last click, the host's
frames after it, and what falls in the 3 s idle window under the possible snapshot lags."""
import collections
import csv
import glob
import sys

d, i = sys.argv[1], sys.argv[2]
pk = [(int(r["mono_ns"]), int(r["sport"]), int(r["dport"]), int(r["len"])) for r in csv.DictReader(open(f"{d}/pkt-{i}.csv"))]
cnt = collections.Counter()
for t, s, dp, ln in pk:
    cnt[(s, dp)] += ln
(hs, cs), _ = cnt.most_common(1)[0]
rx = [(t, ln) for t, s, dp, ln in pk if s == hs and dp == cs]
cf = glob.glob(f"{d}/run-{i}/client/session-*-clicks.csv")[0]
ep = int(open(cf).readline().split("=")[1])
c = list(csv.DictReader(ln for ln in open(cf) if not ln.startswith("#")))
last = ep + int(c[-1]["t_click_us"]) * 1000
print(f"flow host->client {hs}->{cs}: {len(rx)} datagrams, {sum(ln for _, ln in rx)} bytes")
hf = list(csv.DictReader(open(f"{d}/run-{i}/host/host-frames.csv")))
after = [((int(r["t_sent_ns"]) - last) / 1e9, r["class"], int(r["bytes"])) for r in hf if (int(r["t_sent_ns"]) - last) / 1e9 > -0.05]
print("host frames from the last click on (first 4 / those after 1 s, up to 12 s):")
for k, (t, cl, b) in enumerate(after):
    if k < 4 or 1.0 < t < 12:
        print(f"   t={t:.3f} class {cl} {b} B")
c2 = [t for t, cl, b in after if cl == "2" and t < 8]
print(f"   frames after the last click {len(after)}; last class-2 (refinement) frame at +{max(c2) if c2 else 0:.3f} s")
bins = collections.defaultdict(lambda: [0, 0])
for t, ln in rx:
    s = (t - last) / 1e9
    if 0 <= s < 8:
        b = int(s * 10) / 10
        bins[b][0] += 1
        bins[b][1] += ln
print("host->client datagrams after the last click, 100 ms bins as t:count/bytes (empty bins omitted)")
print(" ".join(f"{b:.1f}:{bins[b][0]}/{bins[b][1]}" for b in sorted(bins)))
print("seconds with traffic 2.5-8 s after the last click (t, len):")
print([(round((t - last) / 1e9, 3), ln) for t, ln in rx if 2.5 < (t - last) / 1e9 < 8][:40])

#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""netchange.sh's summary: video gaps after each network event, on CLOCK_MONOTONIC.

  netchange.py OUTDIR
"""
import csv
import glob
import os
import re
import sys


def main(out):
    events = list(csv.DictReader(open(os.path.join(out, "events.csv"))))
    frames_csv = sorted(glob.glob(os.path.join(out, "client", "session-*-frames.csv")))[-1]
    with open(frames_csv) as f:
        epoch = int(f.readline().split("=")[1].split()[0])
        rows = list(csv.DictReader(f))
    arrivals = sorted(epoch + int(r["last_pkt_us"]) * 1000 for r in rows if r["last_pkt_us"])
    clog = open(os.path.join(out, "client", "client.log")).read()
    rlog = open(os.path.join(out, "host", "remoted.log")).read()
    md = ["# Network change (netchange.sh)\n", f"Frames received: {len(arrivals)}\n",
          "| Event | Video back after (ms) | Longest gap in the 5 s after (ms) |", "|---|---|---|"]
    for e in events:
        if e["event"] in ("paired", "connected", "end"):
            continue
        t = int(e["t_mono_ns"])
        after = [a for a in arrivals if a > t]
        back = (after[0] - t) / 1e6 if after else float("nan")
        window = [t] + [a for a in after if a - t < 5e9]
        gap = max((b - a for a, b in zip(window, window[1:])), default=0) / 1e6
        md.append(f"| {e['event']} | {back:.0f} | {gap:.0f} |")
    md.append("")
    routes = re.findall(r"route (\S+(?: udp| tcp| tls)?) (\S+) ->", clog)
    conns = len(re.findall(r"[)] connected", rlog))
    md.append(f"- client: sessions connected {clog.count('connected through')}, routes {routes}")
    md.append(f"- host: client connections {conns} (resumed {rlog.count('resumed')}), sessions ended {rlog.count(' left: ')}")
    for l in clog.splitlines():
        if "timeline" in l or "Reconnecting" in l or "signalling" in l:
            md.append(f"- {l.strip()}")
    print("\n".join(md))


if __name__ == "__main__":
    main(sys.argv[1])

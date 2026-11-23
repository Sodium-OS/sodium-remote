#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""UDP packet log for the idle-bandwidth study: every UDP datagram on `lo` (the
e2e runs talk over this machine's own addresses) as `mono_ns,sport,dport,len`, CLOCK_MONOTONIC like the frame logs.
Needs root (AF_PACKET):  sudo python3 -I pktlog.py OUT.csv   (stop with SIGINT/SIGTERM)."""
import signal, socket, struct, sys, time

running = True
def stop(*_):
    global running
    running = False
signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGTERM, stop)
s = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(3))
s.bind(("lo", 0))
s.settimeout(0.2)
with open(sys.argv[1], "w") as f:
    f.write("mono_ns,sport,dport,len\n")
    while running:
        try:
            data, addr = s.recvfrom(65535)
        except socket.timeout:
            continue
        t = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
        if addr[2] != 4:  # PACKET_OUTGOING: one copy of each datagram on lo
            continue
        if len(data) < 14 + 20 + 8 or data[12:14] != b"\x08\x00" or data[23] != 17:
            continue
        ihl = (data[14] & 15) * 4
        sp, dp, ln = struct.unpack("!HHH", data[14 + ihl : 14 + ihl + 6])
        f.write(f"{t},{sp},{dp},{ln - 8}\n")

#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Joins one run.sh run's logs into a per-click breakdown on CLOCK_MONOTONIC.

Host and client run on one machine here, so every log shares the clock:
  client/session-*-clicks.csv   the client's clicks and the frames that answered them
  host/host-input.csv           when remoted received and injected each input event
  host/host-frames.csv          every frame remoted sent (plugin FRAME time, ready, encoded, sent)
  host/test-surface.csv         when the test surface got each press, committed, and was presented
Prints the client's own summary followed by the full chain, and the
phase-lock check (integration.md 9):

  analyze.py OUTDIR [--deadline-ms D]

The lead the client reports is "ready" to the presenter's deadline for the vsync the frame
was shown at, modulo the period. The plain ready -> present p50 of the click frames must be
within max(3 ms, the lead's spread) of the converged lead p50 plus D, the client's
deadline-to-vsync (0 on the nested headless output: the compositor renders at the vsync; on a
display link, targetTimestamp to targetPresentationTimestamp). The spread is the lead's p50 - p05:
the two medians come from different frame sets (the click frames, every frame in the reports),
which can differ by up to the spread on a jittery path. A whole period's gap means
the client is a frame late, and still fails.
"""
import csv
import glob
import json
import os
import re
import sys


def rows(path):
    with open(path) as f:
        lines = [l for l in f if not l.startswith("#")]
    return list(csv.DictReader(lines))


def epoch_of(path):
    with open(path) as f:
        first = f.readline()
    return int(first.split("=")[1].split()[0]) if first.startswith("# epoch_mono_ns=") else None


def pct(v, p):
    if not v:
        return float("nan")
    v = sorted(v)
    return v[round((len(v) - 1) * p)]


def num(s):
    return int(s) if s not in ("", None) else None


def phase_check(out, plain, deadline_ms):
    """Lines for the phase-lock check: the plain ready -> present p50 of the click frames
    against the converged lead p50 plus the deadline-to-vsync, within max(3 ms, the lead's spread
    p50 - p05). Both numbers are printed. The lead is
    what the client reported: the host logs every report that moved the frame clock
    (remoted.log, "frame clock phase"), the client logs every tenth ("vsync report");
    the first four host reports are the lock converging."""
    leads, period = [], 16.667
    hostlog = os.path.join(out, "host", "remoted.log")
    if os.path.exists(hostlog):
        for l in open(hostlog):
            m = re.search(r"frame clock phase .*\(lead p50 (-?\d+) µs, p05 (-?\d+) µs, (\d+) frames\)", l)
            if m:
                leads.append((int(m.group(1)) / 1000, int(m.group(2)) / 1000))
        leads = leads[4:]
    clog = os.path.join(out, "client", "client.log")
    if len(leads) < 2 and os.path.exists(clog):
        for l in open(clog):
            m = re.search(r"vsync report #(\d+): lead p50 (-?\d+) µs, p05 (-?\d+) µs, (\d+) frames, period (\d+) ns", l)
            if m and int(m.group(1)) > 1:
                leads.append((int(m.group(2)) / 1000, int(m.group(3)) / 1000))
                period = int(m.group(5)) / 1e6
    md = ["\n## Phase-lock check (integration.md 9)\n"]
    if not plain or len(leads) < 2:
        md.append("- not checked: " + ("no presented click frames" if not plain else "fewer than two converged vsync reports"))
        return md, None
    lead = pct([x[0] for x in leads], 0.5)
    p05 = pct([x[1] for x in leads], 0.5)
    spread = lead - p05
    tol = max(3.0, spread)
    diff = pct(plain, 0.5) - (lead + deadline_ms)
    k = round(diff / period)
    ok = abs(diff) <= tol
    md.append(f"- plain ready → present p50 {pct(plain, 0.5):.2f} ms (n {len(plain)} click frames); converged lead p50 {lead:.2f} ms "
              f"(median of {len(leads)} reports, p05 {p05:.2f} ms); deadline-to-vsync {deadline_ms:.2f} ms")
    md.append(f"- tolerance max(3 ms, lead p50 − p05) = max(3.00 ms, {spread:.2f} ms) = {tol:.2f} ms")
    verdict = f"pass (within {tol:.2f} ms)" if ok else (f"FAIL: off by {k} period(s), the client is {k} frame(s) late" if k and abs(diff - k * period) <= tol
                                                          else f"FAIL (not within {tol:.2f} ms)")
    md.append(f"- plain − (lead + deadline-to-vsync) = {diff:+.2f} ms: **{verdict}**")
    return md, ok


def link_lines(out):
    """The controller from the host's side (host/host-link.csv: core's session stats about once a second):
    the target after connect, the probes and their ceiling."""
    path = os.path.join(out, "host", "host-link.csv")
    if not os.path.exists(path):
        return []
    lk = rows(path)
    if not lk:
        return []
    t0 = int(lk[0]["t_ns"])
    at = lambda r: (int(r["t_ns"]) - t0) / 1e9
    md = ["\n## The controller (host-link.csv, about once a second from the session's start)\n"]
    marks = []
    for s in (0.5, 1, 1.5, 2, 2.5, 3, 5, 10):
        r = next((r for r in lk if at(r) >= s), None)
        if r:
            marks.append(f"{s:g} s {int(r['target_bps']) // 1000}")
    md.append("- target kbit/s after the start: " + ", ".join(marks))
    demand = max(int(r["demand_bps"]) for r in lk)
    reach = next((at(r) for r in lk if demand and int(r["target_bps"]) >= 0.98 * demand), None)
    md.append(f"- demand {demand // 1000} kbit/s; the target reached it at "
              + (f"**{reach:.2f} s**" if reach is not None else "**never**")
              + f"; final target {int(lk[-1]['target_bps']) // 1000} kbit/s")
    last = lk[-1]
    changes = [at(r) for p, r in zip(lk, lk[1:]) if int(r["probes"]) > int(p["probes"])]
    md.append(f"- probes {last['probes']}: accepted {last['probes_accepted']}, refused {last['probes_refused']}, "
              f"over 6 ms (decrease) {last['probes_decreased']}; the largest standing queue a probe left "
              f"{float(last['probe_queue_max_ms']):.2f} ms; the last probe went at "
              + (f"{changes[-1]:.2f} s" if changes else "-")
              + f"; probe ceiling {int(last['probe_ceiling_bps']) // 1000 or 'none'}"
              + f"; C_est ever {max(int(r['c_est_bps']) for r in lk) // 1000 or 'none'}")
    tpath = os.path.join(out, "host", "host-target.csv")
    tg = rows(tpath) if os.path.exists(tpath) else []
    if tg:
        # The encoder's budget at every change (remoted's 5 ms tick), from the connection's first one.
        g0 = int(tg[0]["t_ns"])
        steps = [((int(r["t_ns"]) - g0) / 1e6, int(r["encoder_bps"]) // 1000) for r in tg]
        hit = next((t for t, k in steps if demand and k * 1000 >= 0.98 * demand), None)
        hf = os.path.join(out, "host", "host-frames.csv")
        first = (int(rows(hf)[0]["t_rendered_ns"]) - g0) / 1e6 if os.path.exists(hf) and rows(hf) else None
        md.append("- the encoder budget (ms after the connection's first budget → kbit/s): "
                  + ", ".join(f"{t:.0f} → {k}" for t, k in steps[:14]) + (" …" if len(steps) > 14 else ""))
        md.append(f"- at the demand **{hit:.0f} ms** after the connection's first budget" if hit is not None else "- never at the demand")
        if first is not None:
            md[-1] += f"; the first frame was rendered at {first:.0f} ms"
    probing = [r for r in lk if reach is None or at(r) <= reach + 0.5]
    md.append(f"- while probing: standing queue max {max(float(r['queue_delay_ms']) for r in probing):.2f} ms, "
              f"states {sorted({r['cc_state'] for r in probing})}")
    return md


def text_row(out, frames_csv):
    """The text row end to end (run.sh --text): per flip, the
    flip's frame t_rendered (host) to the client's present of the frame that completed it, the
    one whose lossless_ms (or settled_ms) counts back to the flip's frame. Joined by frame id with
    the client's frames CSV; a completing frame the client skipped is shown by the next presented
    one. The ending: "stop" when the flip completes by its lossless_ms (the stop
    only), else host-frames.csv's `settled` with settled_ms (parked or cap). In a CSV from before
    the `settled` column, a lossless_ms at the pass cap (about a second) reads "cap"."""
    gof = os.path.join(out, "host", "text-go.ns")
    if not os.path.exists(gof):
        return []
    go = int(open(gof).read().strip())
    hf = rows(os.path.join(out, "host", "host-frames.csv"))
    md = ["\n## Text row, end to end\n"]
    after = [(i, r) for i, r in enumerate(hf) if int(r["t_rendered_ns"]) >= go - 100_000_000]
    cand = [(i, r) for i, r in after if r["class"] != "2" and r.get("changed_cells")]
    top = max((int(r["changed_cells"]) for _, r in cand), default=0)
    flips = []
    for i, r in cand:
        if top and int(r["changed_cells"]) >= 0.5 * top and (not flips or int(r["t_rendered_ns"]) - int(flips[-1][1]["t_rendered_ns"]) > 1e9):
            flips.append((i, r))
    epoch = epoch_of(frames_csv)
    crow = rows(frames_csv)
    pres = sorted((int(r["frame_id"]), epoch + int(r["present_us"]) * 1000) for r in crow if r["present_us"])
    has_settled = bool(hf) and "settled" in hf[0]
    res = []
    for i, f in flips:
        t0 = int(f["t_rendered_ns"])
        # the frame whose lossless_ms (the stop) or settled_ms (parked, cap) counts back to the flip's frame
        hit = lambda r, col: r.get(col) and abs(int(r["t_encoded_ns"]) - float(r[col]) * 1e6 - t0) <= 3e6
        m = next(((j, r, col) for j, r in enumerate(hf[i:], i) for col in ("lossless_ms", "settled_ms") if hit(r, col)), None)
        if m is None:
            res.append((f, None, None, None, None, []))
            continue
        j, g, col = m
        # the refinement passes (class 2) between the flip's frame and the completing frame, by bytes
        passes = [int(r["bytes"]) for r in hf[i + 1:j + 1] if r["class"] == "2"]
        v = float(g[col])
        if col == "settled_ms":
            ending = g["settled"] if g.get("settled") not in (None, "", "stop") else "parked/cap"
        else:
            ending = "stop" if has_settled or v < 1000 else "cap"
        gid = int(g["frame_id"])
        shown = next(((fid, t) for fid, t in pres if fid >= gid), None)
        e2e = (shown[1] - t0) / 1e6 if shown else None
        res.append((f, g, v, ending, e2e, passes))
    md.append(f"- {len(flips)} flips after the go ({len(cand)} non-refine frames with changes, the flips' at ≥ half the largest: {top} cells)")
    md.append("\n| Flip | frame | kbps | host ms to the ending | ending | passes | pass kB | completing frame | end to end ms |\n|---|---|---|---|---|---|---|---|---|")
    for n, (f, g, v, ending, e2e, passes) in enumerate(res, 1):
        sizes = f"{min(passes) / 1e3:.0f}-{max(passes) / 1e3:.0f}" if passes else "-"
        md.append(f"| {n} | {f['frame_id']} | {f.get('kbps', '')} | {'' if v is None else f'{v:.1f}'} | {ending or '-'} | "
                  f"{len(passes)} | {sizes} | {g['frame_id'] if g else '-'} | {'-' if e2e is None else f'{e2e:.1f}'} |")
    e2es = [r[4] for r in res]
    if res and all(e is not None for e in e2es):
        rest = e2es[1:]
        md.append(f"- **first flip {e2es[0]:.1f} ms** end to end at **{res[0][0].get('kbps', '?')} kbit/s**; the rest: median "
                  f"{pct(rest, 0.5):.1f} ms, worst {max(rest) if rest else float('nan'):.1f} ms (n {len(rest)})")
        lo, hi = int(res[0][0]["frame_id"]), int(res[-1][1]["frame_id"])
        dec = [(int(r["decoded_us"]) - int(r["last_pkt_us"])) / 1e3 for r in crow if lo <= int(r["frame_id"]) <= hi]
        md.append(f"- client decode (last packet → decoded) over the flips: p50 {pct(dec, 0.5):.2f} ms, p95 {pct(dec, 0.95):.2f} ms (n {len(dec)})")
        kbps = int(res[0][0].get("kbps") or 0)
        md.append(f"- verdict: every flip within 300 ms: **{'yes' if max(e2es) <= 300 else 'no'}**; every ending the stop: "
                  f"**{'yes' if all(r[3] == 'stop' for r in res) else 'no'}**; kbps at the first flip ≥ 50 000: **{'yes' if kbps >= 50000 else 'no'}**; "
                  f"8 flips: **{'yes' if len(res) == 8 else 'no'}**")
    else:
        md.append("- not every flip has a completing frame presented on the client")
    # after 3 s of the connection's frames, interactive frames carry split 1 and refine frames 0
    if hf and "split" in hf[0]:
        t3 = int(hf[0]["t_rendered_ns"]) + 3_000_000_000
        late = [r for r in hf if int(r["t_rendered_ns"]) >= t3]
        inter = [r for r in late if r["class"] == "1"]
        refine = [r for r in late if r["class"] == "2"]
        ok_i = sum(r["split"] == "1" for r in inter)
        ok_r = sum(r["split"] == "0" for r in refine)
        md.append(f"- split after 3 s: interactive frames split {ok_i}/{len(inter)}, refine frames unsplit {ok_r}/{len(refine)}: "
                  f"**{'yes' if ok_i == len(inter) and ok_r == len(refine) and inter and refine else 'no'}**")
    return md


def main(out, deadline_ms=0.0):
    clicks_csv = sorted(glob.glob(os.path.join(out, "client", "session-*-clicks.csv")))
    if not clicks_csv:
        print("no client clicks CSV in", out)
        return 1
    clicks_csv = clicks_csv[-1]
    base = clicks_csv[: -len("-clicks.csv")]
    epoch = epoch_of(clicks_csv)
    mono = lambda us: None if us is None else epoch + us * 1000
    clicks = rows(clicks_csv)
    hin = rows(os.path.join(out, "host", "host-input.csv"))
    surf = rows(os.path.join(out, "host", "test-surface.csv"))
    downs = [r for r in hin if r["event"].startswith("Button") and "down: true" in r["event"]]
    presses = [r for r in surf if r["kind"] == "press"]
    presents = {r["id"]: int(r["t_present_ns"]) for r in surf if r["kind"] == "present" and r["t_present_ns"]}

    seg_names = [
        ("input, client → host", "click", "recv"),
        ("host: decode event → injected", "recv", "inj"),
        ("host compositor → test surface gets the press", "inj", "ev"),
        ("test surface: press → commit", "ev", "commit"),
        ("host compositor: commit → presented (frame wait + render)", "commit", "hpres"),
        ("rendered → remoted has it (plugin capture)", "rendered", "captured"),
        ("captured → encoded (convert + NVENC)", "captured", "encoded"),
        ("encoded → sent", "encoded", "send"),
        ("sent → last packet (pacing + loopback)", "send", "last"),
        ("last packet → decoded", "last", "decoded"),
        ("decoded → in system memory", "decoded", "ready"),
        ("ready → presented on the client", "ready", "cpres"),
        ("**input → decoded**", "click", "ready"),
        ("**input → presented on the client**", "click", "cpres"),
    ]
    segs = {n: [] for n, _, _ in seg_names}
    chains = []
    for c in clicks:
        if not c["frame_id"]:
            continue
        t = {
            "click": mono(num(c["t_click_us"])),
            "rendered": mono(num(c["host_rendered_us"])),
            "captured": mono(num(c["host_captured_us"])),
            "encoded": mono(num(c["host_encoded_us"])),
            "send": mono(num(c["send_us"])),
            "last": mono(num(c["last_pkt_us"])),
            "decoded": mono(num(c["decoded_us"])),
            "ready": mono(num(c["ready_us"])),
            "cpres": mono(num(c["present_us"])),
        }
        # The first button-down the host received after the click was handed over.
        d = next((r for r in downs if 0 <= int(r["t_recv_ns"]) - t["click"] < 200_000_000), None)
        if d is not None:
            t["recv"], t["inj"] = int(d["t_recv_ns"]), int(d["t_injected_ns"])
            p = next((p for p in presses if int(p["t_event_ns"]) >= t["inj"] and int(p["t_event_ns"]) - t["inj"] < 200_000_000), None)
            if p:
                t["ev"], t["commit"] = int(p["t_event_ns"]), int(p["t_commit_ns"])
                t["hpres"] = presents.get(p["id"])
        chains.append(t)
        for n, a, b in seg_names:
            if t.get(a) is not None and t.get(b) is not None:
                segs[n].append((t[b] - t[a]) / 1e6)

    md = []
    lat = base + "-latency.md"
    if os.path.exists(lat):
        md.append(open(lat).read())
    md.append(f"\n## The whole chain, host and client logs joined (CLOCK_MONOTONIC, {len(chains)} clicks)\n")
    md.append("| Segment | p50 ms | p95 ms | n |\n|---|---|---|---|")
    for n, _, _ in seg_names:
        v = segs[n]
        md.append(f"| {n} | {pct(v, 0.5):.2f} | {pct(v, 0.95):.2f} | {len(v)} |")
    plain = [(t["cpres"] - t["ready"]) / 1e6 for t in chains if t.get("cpres") and t.get("ready")]
    md.extend(phase_check(out, plain, deadline_ms)[0])
    summ = base + "-summary.json"
    if os.path.exists(summ):
        s = json.load(open(summ))
        md.append("\n## Session\n")
        for k in ("decoder", "stream", "route", "frames_decoded", "frames_presented", "frames_skipped", "frames_lost",
                  "decode_errors", "rtt_ms", "idle_kbps", "idle_secs", "idle_worst_kbps", "present_interval_ms_p50", "refresh_ms"):
            if k in s:
                md.append(f"- {k}: {s[k]}")
    hf = os.path.join(out, "host", "host-frames.csv")
    if os.path.exists(hf):
        fr = rows(hf)
        enc = [(int(r["t_encoded_ns"]) - int(r["t_ready_ns"])) / 1e6 for r in fr]
        cap = [(int(r["t_ready_ns"]) - int(r["t_rendered_ns"])) / 1e6 for r in fr if r["class"] != "2"]
        size = [int(r["bytes"]) for r in fr]
        by_class = {c: sum(1 for r in fr if r["class"] == c) for c in ("0", "1", "2")}
        readable = [float(r["readable_ms"]) for r in fr if r["readable_ms"]]
        lossless = [float(r["lossless_ms"]) for r in fr if r["lossless_ms"]]
        md.append(f"- host frames sent: {len(fr)} (passive {by_class['0']}, interactive {by_class['1']}, refine {by_class['2']}); "
                  f"convert + NVENC p50 {pct(enc, 0.5):.2f} ms, p95 {pct(enc, 0.95):.2f} ms; capture p50 {pct(cap, 0.5):.2f} ms; "
                  f"frame size p50 {pct(size, 0.5)} B, max {max(size) if size else 0} B")
        if readable or lossless:
            md.append(f"- refinement: readable p50 {pct(readable, 0.5):.0f} ms (n {len(readable)}), lossless p50 {pct(lossless, 0.5):.0f} ms (n {len(lossless)})")
    md.extend(link_lines(out))
    frames_csv = base + "-frames.csv"
    if os.path.exists(frames_csv) and os.path.exists(hf):
        md.extend(text_row(out, frames_csv))
    # Input check (windowed runs): what the client sent against what the test surface got.
    inlog = os.path.join(out, "client", "input.csv")
    if os.path.exists(inlog):
        sent = [l.strip().split(",", 1)[1] for l in open(inlog) if "," in l]
        keys_sent = [l for l in sent if l.startswith("Key") and "down: true" in l]
        axes_sent = [l for l in sent if l.startswith("Axis")]
        got_keys = [r["detail"].split()[0].split("=")[1] for r in surf if r["kind"] == "key" and "Pressed" in r["detail"]]
        got_axis = [(r["kind"], r["detail"]) for r in surf if r["kind"].startswith("axis")]
        md.append("\n## Input check (client window → host test surface)\n")
        md.append(f"- keys the client sent (HID usages, presses): {', '.join(k.split('usage: ')[1].split(';')[0] for k in keys_sent)}")
        md.append(f"- keys the test surface got (evdev, presses): {', '.join(got_keys)}")
        md.append(f"- scroll events the client sent: {len(axes_sent)}; first: {axes_sent[0] if axes_sent else '-'}; last: {axes_sent[-1] if axes_sent else '-'}")
        rel_sent = sum(1 for l in sent if l.startswith("PointerRel"))
        rel_host = sum(1 for r in hin if r["event"].startswith("PointerRel"))
        md.append(f"- pointer lock: the client sent {rel_sent} relative motions, the host injected {rel_host}")
        srcs = sorted({d for k, d in got_axis if k == 'axis_source'})
        md.append(f"- scroll at the test surface: {sum(1 for k, _ in got_axis if k == 'axis')} axis, "
                  f"{sum(1 for k, _ in got_axis if k == 'axis120')} value120, {sum(1 for k, _ in got_axis if k == 'axis_stop')} stop; sources {srcs}")
    print("\n".join(md))
    return 0


if __name__ == "__main__":
    args = sys.argv[1:]
    dl = 0.0
    if "--deadline-ms" in args:
        i = args.index("--deadline-ms")
        dl = float(args[i + 1])
        del args[i : i + 2]
    sys.exit(main(args[0] if args else ".", dl))

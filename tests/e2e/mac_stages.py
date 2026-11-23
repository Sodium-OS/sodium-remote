#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Per-stage times of a Remote for Mac session frames file (~/Library/Logs/Remote/session-*-frames.csv): mac_stages.py FILE [SECONDS_FROM_END]"""
import sys, statistics as st
cols="frame_id,keyframe,bytes,host_rendered_us,host_captured_us,host_converted_us,host_encoded_us,send_us,first_pkt_us,last_pkt_us,decode_start_us,decoded_us,ready_us,transfer_ms,commit_us,present_us,skipped,deadline_us,target_present_us".split(',')
rows=[]
for line in open(sys.argv[1]):
    v=line.strip().split(',')
    if len(v)!=len(cols): continue
    try: rows.append({k:float(x) for k,x in zip(cols,v)})
    except ValueError: pass
allrows=rows
rows=[r for r in rows if r['skipped']==0 and r['commit_us']>0 and r['present_us']>0]
if len(sys.argv)>2 and rows:
    end=rows[-1]['commit_us']; rows=[r for r in rows if r['commit_us']>=end-float(sys.argv[2])*1e6]
print(f"{len(allrows)} frames, {sum(1 for r in allrows if r['skipped']!=0)} skipped; {len(rows)} presented in the window")
def stage(a,b):
    d=sorted((r[b]-r[a])/1000 for r in rows if r[a]>0 and r[b]>0)
    return f"p50 {st.median(d):6.2f}  p95 {d[int(len(d)*0.95)]:6.2f}" if d else "n/a"
for a,b in [('host_captured_us','host_encoded_us'),('send_us','last_pkt_us'),('decode_start_us','decoded_us'),('ready_us','commit_us'),('commit_us','present_us'),('host_rendered_us','present_us')]:
    print(f"  {a[:-3]:15s} -> {b[:-3]:13s} {stage(a,b)}")

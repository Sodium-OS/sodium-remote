#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Makes the decode probe's sample bitstreams on a Linux machine with an NVIDIA GPU,
# with the NVENC probe (NVENC_PROBE): the same NVENC settings the host uses (P1, ultra-low
# latency, CBR, one-frame VBV, forced 2-way split), 2940 × 1848 (the Mac's
# backing pixels), a synthetic desktop, 100 Mbit/s.
#
#   Tools/make-s6-samples.sh [OUT_DIR] [FRAMES] [WxH]
#
# Writes OUT_DIR/yuv444.hevc (Main 4:4:4, from YUV 4:4:4 input) and
# OUT_DIR/nv12.hevc (Main, 4:2:0), cut to the first FRAMES frames (default 4,
# what Tools/samples/ holds: about 1.2 MB each at 2940 × 1848; 10 for the whole
# run). WxH defaults to 2940x1848; 2560x1664 gives the first samples' size.
# NVENC_PROBE: the probe's source, nvenc_probe.c (default: spikes/remote-video/nvenc_probe.c in the project's
# own tree; not part of the public repository).
# Needs ffnvcodec-headers and the NVIDIA driver; runs NVENC for about a second
# and writes about 20 MB of scratch files (TMPDIR).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
OUT=${1:-$HERE/samples}
FRAMES=${2:-4}
SIZE=${3:-2940x1848}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

PROBE=${NVENC_PROBE:-$ROOT/spikes/remote-video/nvenc_probe.c}
[ -f "$PROBE" ] || { echo "make-s6-samples.sh: $PROBE not found (set NVENC_PROBE)" >&2; exit 1; }
cc -O2 -w -o "$work/nvenc_probe" "$PROBE" -ldl -lm
"$work/nvenc_probe" "$work" argb "$SIZE" | tail -n +5
mkdir -p "$OUT"
for name in yuv444 nv12; do
  # The probe writes each frame's size to NAME.hevc.sizes: keep the first FRAMES.
  bytes=$(head -n "$FRAMES" "$work/$name.hevc.sizes" | awk '{s += $2} END {print s}')
  head -c "$bytes" "$work/$name.hevc" > "$OUT/$name.hevc"
  echo "$OUT/$name.hevc: $FRAMES frames, $bytes bytes"
done
if command -v ffprobe >/dev/null; then
  for name in yuv444 nv12; do
    ffprobe -v error -show_entries stream=profile,pix_fmt,width,height,color_range -of compact "$OUT/$name.hevc"
  done
fi

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Linux end to end with the fake backend:
# sodium-remote-host (fake) on this PC, paired by the probe over the LAN listener (no
# mDNS, so nothing is announced on the network), then client-linux headless for SECS
# seconds, then the probe's scripted checks. Separate data dirs under remote/build/e2e;
# never the live remoted's.
#   host-desktop/tests/e2e-linux.sh [ENCODER] [SECS]     ENCODER: hevc_nvenc (default) or openh264
set -euo pipefail
R=$(cd "$(dirname "$0")/../.." && pwd)
ENC=${1:-hevc_nvenc}
SECS=${2:-20}
T=$R/target/debug
OUT=$R/build/e2e/$ENC
rm -rf "$OUT"
mkdir -p "$OUT"
PORT=$((47400 + RANDOM % 400))

"$T/sodium-remote-host" --backend fake --encoder "$ENC" --data "$OUT/host" --port $PORT --no-mdns --pair \
    --input-log "$OUT/host-input.csv" >"$OUT/host.out" 2>"$OUT/host.log" &
HOST=$!
trap 'kill $HOST 2>/dev/null || true' EXIT
for _ in $(seq 100); do [ -s "$OUT/host/pairing-code.txt" ] && break; sleep 0.1; done
ID=$(sed -n 's/^id=//p' "$OUT/host/host.txt")
CODE=$(cat "$OUT/host/pairing-code.txt")
HINT="$ID@127.0.0.1:$PORT"

cargo run -q --manifest-path "$R/Cargo.toml" -p host-desktop --example probe -- \
    --data "$OUT/client" pair --code "$CODE" --lan-hint "$HINT" >"$OUT/pair.out" 2>&1
echo "paired: $(cat "$OUT/pair.out")"

"$T/remote-client" --data "$OUT/client" connect --headless --no-rendezvous --no-mdns --lan-hint "$HINT" \
    --secs "$SECS" --size 1920x1080 --stats-dir "$OUT/client-stats" >"$OUT/client.out" 2>"$OUT/client.log" || true
echo "--- client-linux:"
grep -E "welcome|host is|decoding|first picture" "$OUT/client.log" || true
tail -n 30 "$OUT/client.log" | grep -iE "frames|fps|decoded|lost" || true

echo "--- probe:"
cargo run -q --manifest-path "$R/Cargo.toml" -p host-desktop --example probe -- \
    --data "$OUT/client" --lan-hint "$HINT" --no-mdns --secs 10 --dump "$OUT/probe.bit" --save-clip "$OUT/clip" \
    --type "Hello, Windows!" --commit "日本語" --click 400,300 --scroll 3 --clip-text "from the probe" \
    --select-display 2 --wait 1500 --select-display 1 2>"$OUT/probe.log" | tee "$OUT/probe.out"
echo "--- host input log: $(wc -l <"$OUT/host-input.csv") events"
grep -c "Key" "$OUT/host-input.csv" | sed 's/^/keys: /'
grep -E "Text|Button|Axis" "$OUT/host-input.csv" | head -5
echo "--- host log:"
grep -E "encoding|welcome|clipboard|now display|fps" "$OUT/host.log" | tail -n 12
ffprobe -v error -count_frames -show_entries stream=codec_name,width,height,nb_read_frames "$OUT/probe.bit" 2>&1 | tr '\n' ' '; echo

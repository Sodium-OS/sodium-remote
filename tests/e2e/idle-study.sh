#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# The idle-bandwidth study: N short e2e runs, each with pktlog.py recording every
# UDP datagram on lo. Needs passwordless sudo for the capture only.
#   idle-study.sh OUTDIR N [run.sh options...]
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(realpath -m "${1:?usage: idle-study.sh OUTDIR N [run.sh options]}")
n=${2:?N}
shift 2
mkdir -p "$out"
for i in $(seq "$n"); do
    sudo -n python3 -I "$here/pktlog.py" "$out/pkt-$i.csv" &
    cp=$!
    sleep 1
    "$here/run.sh" "$out/run-$i" "$@" >"$out/run-$i.out" 2>&1
    sudo -n pkill -INT -f "pktlog.py $out/pkt-$i.csv"
    wait "$cp"
    grep -E "Idle bandwidth" "$out/run-$i.out"
done

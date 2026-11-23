#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# The screenshot matrix: tests/e2e.sh for every language and theme given, each against a freshly started nested host
# (a failed session leaves the nested host's capture in a state that needs a restart; docs/reference/client-app.md).
#
#   tests/matrix.sh OUTDIR "en de ja" "dark light" [SCRIPT]
#
# OUTDIR/<lang>-<theme>/ gets each run's data, log and shots/. Never touches the live desktop: two nested Hyprland
# instances at a time (the host's and the client screen's).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
out=$(realpath -m "${1:?OUTDIR}")
langs=${2:-en}
themes=${3:-dark}
script=${4:-$here/scripts/main.txt}
mkdir -p "$out"
host=$out/host

start_host() {
    rm -rf "$host"
    (nohup "$repo/spikes/remote-e2e/run.sh" "$host" --host-only >"$out/host-run.log" 2>&1 &)
    for _ in $(seq 180); do [[ -f $host/host-only.env && -s $host/pair-code ]] && return 0; sleep 1; done
    echo "the nested host didn't come up" >&2
    return 1
}
stop_host() {
    touch "$host/stop"
    for _ in $(seq 30); do pgrep -f "remote-e2e/run.sh $host" >/dev/null || break; sleep 1; done
}

for lang in $langs; do
    for theme in $themes; do
        start_host || exit 1
        "$here/e2e.sh" "$host" "$out/$lang-$theme" "$lang" "$theme" "$script" 2>&1 | tail -n 1
        stop_host
    done
done

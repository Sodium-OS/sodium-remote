#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# The Remote app end to end, in the nested bed only (never on the live desktop, never against the live remoted):
#
#   tests/e2e.sh HOSTDIR OUTDIR LANG THEME SCRIPT
#
# HOSTDIR is a running `spikes/remote-e2e/run.sh HOSTDIR --host-only` (the nested host: its own Hyprland with the
# sodiumwm plugin, a private bus, remoted). OUTDIR gets the run's data dir, the app's log and the screenshots.
# LANG is a Sodium UI language (en, de, ja ...), THEME dark or light. SCRIPT is an app script (--script; see
# qml/Main.qml's runStep) with {HOST} and {CODE} replaced; a `log shot NAME` step takes a screenshot of the client
# screen (grim on the nested output) at that moment.
#
# One nested Hyprland (the client screen: a headless 1600x1000 output) for the run, and nothing else.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)   # the SodiumOS checkout (this folder is remote/app/tests)
host=$(realpath "${1:?HOSTDIR}")
out=$(realpath -m "${2:?OUTDIR}")
lang=${3:-en}
theme=${4:-dark}
script=$(realpath "${5:?SCRIPT}")
app=$repo/apps/build/bin/sodium-remote

[[ -f $host/host-only.env ]] || { echo "no host: start spikes/remote-e2e/run.sh $host --host-only first"; exit 1; }
# shellcheck disable=SC1091
source "$host/host-only.env"
bus=$(head -n1 "$host/bus.addr")
rm -rf "$out"
mkdir -p "$out/shots" "$out/state/theme" "$out/data"
python3 -I "$here/seed.py" "$out/data"
printf '{"theme":"%s"}\n' "$theme" >"$out/state/settings.json"
python3 -I "$repo/tokens/generate.py" --settings "$out/state/settings.json" --out "$out/state/theme" >/dev/null 2>&1 || true

# A fresh pairing code from the nested host's private bus, for the script's {CODE}.
code=$(busctl --address="$bus" call org.sodiumos.Remote /org/sodiumos/Remote org.sodiumos.Remote OpenPairing --json=short | jq -r '.data[0]')
now=$(date +%s)
sed -e "s/{T_TODAY}/$((now - 600))/g" -e "s/{T_EARLIER}/$((now - 3600))/g" -e "s/{T_OLD}/$((now - 1900000))/g" -e "s/{HOST}/$HOST_ID/g" -e "s/{CODE}/$code/g" -e "s/{LANPORT}/$LAN_PORT/g" "$script" >"$out/script.txt"

csig=$("$repo/scripts/nested-hyprland.sh" start "$here/nested.lua")
[[ -n $csig && $csig != "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || { echo "bad nested signature" >&2; exit 1; }
cleanup() { "$repo/scripts/nested-hyprland.sh" stop "$csig" >/dev/null 2>&1 || true; }
trap cleanup EXIT
sock=$(hyprctl instances -j | jq -r --arg s "$csig" '.[] | select(.instance == $s) | .wl_socket')
c() { hyprctl -i "$csig" "$@"; }
c output create headless CLIENT-1 >/dev/null
c eval 'hl.monitor({ output = "CLIENT-1", mode = "1600x1000@60", position = "0x0", scale = "1" })' >/dev/null
c eval 'hl.monitor({ output = "WAYLAND-1", disabled = true })' >/dev/null
sleep 1

env -u DISPLAY WAYLAND_DISPLAY="$sock" HYPRLAND_INSTANCE_SIGNATURE="$csig" QT_QPA_PLATFORM=wayland QT_FORCE_STDERR_LOGGING=1 SODIUMOS_I18N_AUDIT=1 \
    SODIUMOS_STATE="$out/state" SODIUMOS_UI_LANGUAGE="$lang" QML_IMPORT_PATH="$repo/ui" RUST_LOG=${RUST_LOG:-warn} \
    SODIUM_REMOTE_TEST=1 "$app" --data "$out/data" --lan-only --script "$out/script.txt" >"$out/app.log" 2>&1 &
apid=$!
# Screenshots on the script's markers.
(
    tail -n +1 -F "$out/app.log" 2>/dev/null | while read -r line; do
        case $line in
        *"remote-script: shot "*)
            name=${line##*remote-script: shot }
            WAYLAND_DISPLAY="$sock" grim -o CLIENT-1 "$out/shots/$name.png"
            ;;
        esac
    done
) >/dev/null 2>&1 &
tpid=$!
for _ in $(seq 600); do kill -0 $apid 2>/dev/null || break; sleep 0.5; done
kill $apid 2>/dev/null
sleep 1
kill $tpid 2>/dev/null
pkill -f "tail -n +1 -F $out/app.log" 2>/dev/null || true
echo "done: $out ($(ls "$out/shots" | wc -l) shots)"

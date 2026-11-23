#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Sodium Remote for Windows (in the VM) against the nested SodiumOS host: pairs by bare address, connects, takes the
# screenshots of computers.txt, copies them to OUTDIR/<lang>-<theme>/. Hold the VM lock first (win.sh lock integration).
#
#   run-computers.sh HOSTDIR OUTDIR LANG THEME     (LANG: en_US, de_DE, ja_JP; THEME: dark, light)
#
# HOSTDIR is a running `spikes/remote-e2e/run.sh HOSTDIR --host-only`. Nothing here touches the live desktop or host.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
source "$here/../config.sh"
WIN=${WIN:-$here/../win.sh}
host=$(realpath "${1:?HOSTDIR}")
out=$(realpath -m "${2:?OUTDIR}")/${3:?LANG}-${4:?THEME}
lang=$3 theme=$4
# shellcheck disable=SC1091
source "$host/host-only.env"
bus=$(head -n1 "$host/bus.addr")
code=$(busctl --address="$bus" call org.sodiumos.Remote /org/sodiumos/Remote org.sodiumos.Remote OpenPairing --json=short | jq -r '.data[0]')
mkdir -p "$out"
sed -e "s/{HOST}/$HOST_ID/g" -e "s/{CODE}/$code/g" -e "s/{ADDR}/$BR_IP:$LAN_PORT/g" -e "s/{DEAD}/$REMOTE_VM_NET.77:47311/g" "$here/computers.txt" >"$out/script.txt"
$WIN scp-to "$here/run-app.ps1" "$GF/integration/run-app.ps1" >/dev/null
$WIN scp-to "$here/shots.ps1" "$GF/integration/shots.ps1" >/dev/null
$WIN scp-to "$out/script.txt" "$GF/integration/computers-$lang-$theme.txt" >/dev/null
$WIN ssh "Remove-Item $GD\\integration\\shots\\* -ErrorAction SilentlyContinue; Remove-Item -Recurse -Force $GD\\integration\\data-computers -ErrorAction SilentlyContinue"
$WIN ssh "powershell -File $GD\\integration\\run-app.ps1 -Lang $lang -Theme $theme -Name computers -Data $GD\\integration\\data-computers -Script $GD\\integration\\computers-$lang-$theme.txt -AppArgs --lan-only"
# The script is about 60 s long.
for _ in $(seq 120); do
    sleep 2
    $WIN ssh 'if (Get-Process SodiumRemote -ErrorAction SilentlyContinue) { "running" } else { "gone" }' | grep -q gone && break
done
$WIN scp-from "$GF/integration/shots/*" "$out/" 2>&1 | tail -1
$WIN scp-from "$GF/integration/out/computers.log" "$out/app.log" 2>&1 | tail -1
ls "$out" | wc -l

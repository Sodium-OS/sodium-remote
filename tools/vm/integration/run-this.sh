#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Sodium Remote for Windows, This computer, in the VM: the page's states, the "being controlled" notice and the tray menu,
# with this PC's probe (host-desktop's example) as the controlling device. Hold the VM lock first.
#
#   run-this.sh OUTDIR LANG THEME     (LANG: en_US, de_DE, ja_JP; THEME: dark, light)
#
# The screen coordinates are the 1920x1080 guest's: the tray's overflow caret and its icon, the notice's and the menu's
# Disconnect buttons. Needs target/debug/examples/probe (cargo build -p host-desktop --example probe) and a probe data
# dir with a paired device (OUTDIR/../probeB, copied from a remote-client pairing). Never touches the live desktop.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
# shellcheck disable=SC1091
source "$here/../config.sh"
WIN=${WIN:-$here/../win.sh}
Q=$repo/remote/tools/vm/windows-app/qmp.py
probe=$repo/remote/target/debug/examples/probe
out=$(realpath -m "${1:?OUTDIR}")/${2:?LANG}-${3:?THEME}
lang=$2 theme=$3
probedata=$(realpath -m "$out/../probeB")
hid=${HOST_ID:?set HOST_ID to the nested host's device ID}
mkdir -p "$out"
shot() { $WIN screenshot "$out/$1.png" >/dev/null; }
$WIN scp-to "$here/run-app.ps1" "$GF/integration/run-app.ps1" >/dev/null
$WIN scp-to "$here/shots.ps1" "$GF/integration/shots.ps1" >/dev/null
$WIN scp-to "$here/this.txt" "$GF/integration/this.txt" >/dev/null
$WIN ssh "Remove-Item $GD\\integration\\shots\\* -ErrorAction SilentlyContinue"
# The probe is paired by seeding the host's paired.json (the page's own code can't be read from outside); the app is
# stopped while the file is written. PROBE_ID is the id of $probedata/device.key (remote-client prints it when pairing).
pid=${PROBE_ID:?set PROBE_ID to the probe data dir's device ID}
$WIN ssh 'Get-Process SodiumRemote -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep 2'
$WIN ssh "New-Item -ItemType Directory -Force $GD\\integration\\data-this\\host | Out-Null; '{\"seq\":0,\"devices\":[{\"id\":\"$pid\",\"name\":\"Probe\",\"paired_at\":1794926379,\"last_connected\":null,\"named_at_desk\":false,\"lan_addr\":null}]}' | Set-Content -Encoding ascii $GD\\integration\\data-this\\host\\paired.json"
$WIN ssh "powershell -File $GD\\integration\\run-app.ps1 -Lang $lang -Theme $theme -Name this -Data $GD\\integration\\data-this -Script $GD\\integration\\this.txt"
sleep 16
$WIN scp-from "$GF/integration/shots/this-*.png" "$out/" >/dev/null 2>&1
# Notepad is the window under control: bring it to the front (a click on its empty area), then connect.
python3 "$Q" click 1500 700 >/dev/null
(timeout 60 "$probe" --data "$probedata" --lan-hint "$hid@$GUEST_IP:47311" --no-mdns --secs 50 --wait 45000 >"$out/probe.log" 2>&1 &)
sleep 5
shot this-notice
python3 "$Q" click 1694 1056 >/dev/null; sleep 1
python3 "$Q" rclick 1674 994 >/dev/null; sleep 1.5
shot tray-controlled
python3 "$Q" click 1673 804 >/dev/null; sleep 3          # Disconnect on the menu's card
python3 "$Q" click 1694 1056 >/dev/null; sleep 1
python3 "$Q" rclick 1674 994 >/dev/null; sleep 1.5
shot tray-on
python3 "$Q" click 900 300 >/dev/null; sleep 1
python3 "$Q" click 1226 1056 >/dev/null; sleep 2           # the app's window
shot this-paired
ls "$out" | wc -l

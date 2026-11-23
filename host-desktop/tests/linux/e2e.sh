#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# The generic Linux host end to end: sodium-remote-host
# (portal backend) in the arch-desktops VM (tools/vm/linux.sh) on GNOME or KDE Plasma,
# Wayland; the probe and client-linux on this PC over the host-only bridge. Never
# touches this PC's own desktop.
#
#   host-desktop/tests/linux/e2e.sh gnome|kde [--fresh]
#
# The VM must be running that desktop (`linux.sh start gnome|kde`) and you should hold
# its lock. --fresh forgets the pairing and the portal's restore token first, so the
# first connection meets the sharing dialog: GNOME's is answered by clicks through QMP
# (Allow Remote Interaction, then Share); KDE's is skipped by pre-authorising the app ID
# in the permission store (kde-authorized / remote-desktop).
#
# Three connections:
#   1. the probe: picture, pointer (moves come back as the cursor's position), a click,
#      typing and an IME commit (the guest's witness window records them), clipboard
#      both ways, audio (a tone played in the guest);
#   2. the probe again: no dialog (the restore token), picture;
#   3. client-linux headless: the share is stopped at the computer (GNOME: the top bar's
#      indicator; KDE: the tray item's End) and the client sees "Ended at the computer".
# Output and evidence in remote/build/e2e-linux/<desktop>/; a PASS/FAIL table at the end.
set -uo pipefail
R=$(cd "$(dirname "$0")/../../.." && pwd)
DESK=${1:?gnome or kde}
FRESH=${2:-}
V=$R/tools/vm/linux.sh
# A target dir of its own: client-windows also names a binary remote-client (a stub off
# Windows), and a workspace build in target/ can leave that one at target/debug/remote-client.
export CARGO_TARGET_DIR=$R/target/e2e-linux
T=$CARGO_TARGET_DIR/debug
OUT=$R/build/e2e-linux/$DESK
EV=$OUT/evidence
APP=org.sodiumos.Remote
UNIT="app-$APP@e2e"
# shellcheck source=../../../tools/vm/config.sh
source "$R/tools/vm/config.sh"
GUEST=$LX_GUEST_IP
mkdir -p "$OUT" "$EV"
rm -f "$OUT"/*.out "$OUT"/*.log "$EV"/*
declare -A RESULT

g() { "$V" ssh "export XDG_RUNTIME_DIR=/run/user/1000; $*"; }
shot() { "$V" screenshot "$EV/$1.png" >/dev/null 2>&1 || true; }
pass() { RESULT[$1]="PASS $2"; }
fail() { RESULT[$1]="FAIL $2"; }

echo "== build and deploy"
cargo build -q --manifest-path "$R/Cargo.toml" -p host-desktop --bin sodium-remote-host --example probe || exit 1
cargo build -q --manifest-path "$R/Cargo.toml" -p remote-client --bin remote-client || exit 1
"$V" deploy "$T/sodium-remote-host" >/dev/null
"$V" scp-to "$R/host-desktop/tests/linux/sr-testapp.py" bin/sr-testapp.py >/dev/null
g "systemctl --user stop 'app-$APP@*' sr-witness sr-witness2 sr-tone 2>/dev/null; pkill -f \"\$HOME/[b]in/sodium-remote-host\"; pkill -x wl-copy; sleep 1; systemctl --user reset-failed 2>/dev/null; rm -rf ~/sr-test; mkdir -p ~/sr-test; true"
if [ "$FRESH" = --fresh ]; then
    g "rm -rf ~/.local/share/sodium-remote/host"
    rm -rf "$OUT/client"
fi
# The app's desktop entry, as the Remote app installs it: portals name and trust host apps
# by the unit's app ID only when it has one.
g "mkdir -p ~/.local/share/applications; printf '[Desktop Entry]\nType=Application\nName=Sodium Remote\nExec=%s/bin/sodium-remote-host\nNoDisplay=true\n' \"\$HOME\" > ~/.local/share/applications/$APP.desktop"
if [ "$DESK" = kde ]; then
    # Pre-authorise the app ID (the systemd unit's app-<ID>@ name gives it): no dialog.
    g "busctl --user call org.freedesktop.impl.portal.PermissionStore /org/freedesktop/impl/portal/PermissionStore \
        org.freedesktop.impl.portal.PermissionStore SetPermission sbssas kde-authorized true remote-desktop $APP 1 yes"
fi
# A 440 Hz tone at 0.3 for the audio check.
g "python3 - <<'PY'
import math, os, struct, wave
w = wave.open(os.path.expanduser('~/sr-test/tone.wav'), 'wb'); w.setnchannels(2); w.setsampwidth(2); w.setframerate(48000)
w.writeframes(b''.join(struct.pack('<hh', *(2 * [int(0.3 * 32767 * math.sin(2 * math.pi * 440 * i / 48000))])) for i in range(48000 * 8)))
PY"
g "systemd-run -q --user --unit=sr-witness -p StandardOutput=truncate:\$HOME/sr-test/witness.log -p StandardError=truncate:\$HOME/sr-test/witness.log python3 \$HOME/bin/sr-testapp.py"
sleep 3
[ "$DESK" = gnome ] && "$V" key esc >/dev/null
g "systemd-run -q --user --unit='$UNIT' -E SODIUM_REMOTE_HOST_DEBUG=1 -p StandardOutput=truncate:\$HOME/sr-test/host.out \
    -p StandardError=truncate:\$HOME/sr-test/host.log \$HOME/bin/sodium-remote-host --pair --no-mdns --bind $GUEST"
for _ in $(seq 100); do g "test -s ~/.local/share/sodium-remote/host/host.txt" && break; sleep 0.3; done
ID=$(g "sed -n 's/^id=//p' ~/.local/share/sodium-remote/host/host.txt")
PORT=$(g "sed -n 's/^port=//p' ~/.local/share/sodium-remote/host/host.txt")
HINT="$ID@$GUEST:$PORT"
if [ ! -d "$OUT/client" ]; then
    for _ in $(seq 50); do g "test -s ~/.local/share/sodium-remote/host/pairing-code.txt" && break; sleep 0.2; done
    CODE=$(g "cat ~/.local/share/sodium-remote/host/pairing-code.txt")
    "$T/examples/probe" --data "$OUT/client" pair --code "$CODE" --lan-hint "$HINT" >"$OUT/pair.out" 2>&1
fi
grep -q paired "$OUT/pair.out" 2>/dev/null || [ -d "$OUT/client" ] && pass pair "$HINT" || fail pair "see pair.out"

echo "== 1: the probe (input, clipboard, audio)"
# The guest's side, in step with the probe's script (times from its connect).
(
    if [ "$DESK" = gnome ] && [ "$FRESH" = --fresh ]; then
        sleep 5; shot gnome-dialog
        "$V" click 1212 463 >/dev/null; sleep 0.5; shot gnome-dialog-allowed
        "$V" click 1245 328 >/dev/null
    fi
    sleep 9; g "systemd-run -q --user --unit=sr-tone pw-play \$HOME/sr-test/tone.wav"
    sleep 9; g "echo read > ~/sr-test/cmd"
    # wl-copy, not the witness: a compositor takes a selection only with a recent input serial.
    sleep 2; g "export WAYLAND_DISPLAY=\$(ls /run/user/1000 | grep -E '^wayland-[0-9]+\$' | head -1); setsid wl-copy 'from the $DESK guest' </dev/null >/dev/null 2>&1 &"
    sleep 1; shot session1
) &
SIDE=$!
"$T/examples/probe" --data "$OUT/client" --lan-hint "$HINT" --no-mdns --secs 30 --save-clip "$OUT/clip" \
    --wait 8000 --move 400,300 --wait 600 --move 900,600 --wait 600 --click 960,540 --wait 400 \
    --type "Hello from Sodium Remote" --commit " ok" --wait 500 --clip-text "from the probe" --wait 12000 \
    >"$OUT/probe1.out" 2>"$OUT/probe1.log"
wait $SIDE
g "cat ~/sr-test/text.txt" >"$OUT/witness-text.txt" 2>/dev/null
g "cat ~/sr-test/clicks.txt" >"$OUT/witness-clicks.txt" 2>/dev/null
g "cat ~/sr-test/clip-in.txt" >"$OUT/witness-clip-in.txt" 2>/dev/null
frames=$(sed -n 's/^video: \([0-9]*\) frames.*/\1/p' "$OUT/probe1.out")
[ "${frames:-0}" -gt 0 ] && pass picture "$frames frames, $(grep -o 'welcome: .*' "$OUT/probe1.out" | cut -c1-80)" || fail picture "no frames"
grep -q "pointer at 900,600" "$OUT/probe1.log" && pass pointer "moved to 900,600 and the cursor came back there" || fail pointer "$(grep -c 'pointer at' "$OUT/probe1.log") positions"
[ -s "$OUT/witness-clicks.txt" ] && pass click "$(head -1 "$OUT/witness-clicks.txt")" || fail click "no press in the witness"
grep -q "Hello from Sodium Remote ok" "$OUT/witness-text.txt" && pass keyboard "witness: $(cat "$OUT/witness-text.txt")" || fail keyboard "witness: $(cat "$OUT/witness-text.txt" 2>/dev/null)"
grep -q "from the probe" "$OUT/witness-clip-in.txt" && pass clip-to-host "witness read: $(cat "$OUT/witness-clip-in.txt")" || fail clip-to-host "witness read: $(cat "$OUT/witness-clip-in.txt" 2>/dev/null)"
grep -q "from the $DESK guest" "$OUT/probe1.log" && pass clip-to-client "$(grep -m1 "from the $DESK guest" "$OUT/probe1.log" | cut -c1-90)" || fail clip-to-client "nothing offered"
audio=$(grep '^audio:' "$OUT/probe1.out")
pk=$(echo "$audio" | sed -n 's/^audio: \([0-9]*\) packets.*/\1/p')
[ "${pk:-0}" -gt 100 ] && pass audio "$audio" || fail audio "$audio"

echo "== 2: reconnect (restore token, no dialog)"
(sleep 4; shot reconnect) &
"$T/examples/probe" --data "$OUT/client" --lan-hint "$HINT" --no-mdns --secs 8 --wait 6000 --move 300,200 >"$OUT/probe2.out" 2>"$OUT/probe2.log"
wait
g "cat ~/sr-test/host.log" >"$OUT/host.log"
starts=$(grep -c "starting the portal session" "$OUT/host.log")
tok=$(grep -c "with the restore token" "$OUT/host.log")
f2=$(sed -n 's/^video: \([0-9]*\) frames.*/\1/p' "$OUT/probe2.out")
if [ "${f2:-0}" -gt 0 ] && [ "$starts" -ge 2 ] && [ "$(grep 'starting the portal session' "$OUT/host.log" | tail -1 | grep -c 'restore token')" = 1 ]; then
    pass reconnect "$f2 frames; $tok of $starts starts used the restore token"
else
    fail reconnect "$f2 frames; $tok of $starts starts used the token"
fi

echo "== 3: stop sharing at the computer"
"$T/remote-client" --data "$OUT/client" connect --headless --no-rendezvous --no-mdns --lan-hint "$HINT" --secs 40 \
    --size 1920x1080 --stats-dir "$OUT/client-stats" >"$OUT/client3.out" 2>"$OUT/client3.log" &
CL=$!
sleep 8
shot before-stop
if [ "$DESK" = gnome ]; then
    "$V" click 1796 16 >/dev/null     # the top bar's screen-sharing indicator: stop
else
    # Plasma's tray item for the share, then its End (1920x1080 layout, default panel).
    "$V" click 1682 1051 >/dev/null; sleep 1.5; shot kde-tray-menu
    "$V" click 1708 1015 >/dev/null
fi
for _ in $(seq 30); do kill -0 $CL 2>/dev/null || break; sleep 0.5; done
sleep 1; shot after-stop
kill $CL 2>/dev/null; wait $CL 2>/dev/null
g "cat ~/sr-test/host.log" >"$OUT/host.log"
if grep -q "disconnected: EndedAtComputer" "$OUT/client3.log" && grep -q "left: EndedAtComputer" "$OUT/host.log"; then
    pass stop-sharing "client: $(grep -m1 'disconnected:' "$OUT/client3.log")"
else
    fail stop-sharing "client: $(grep -m1 'disconnected' "$OUT/client3.log")"
fi
grep -E "encoder|encoding" "$OUT/host.log" | head -8 >"$OUT/encoders.txt"
pass encoder-fallback "$(grep -m1 'encoding' "$OUT/host.log" | sed 's/.*with //')"
g "systemctl --user stop '$UNIT' sr-witness 2>/dev/null; true"

echo
echo "== $DESK ($(date -u +%FT%TZ))"
for k in pair picture pointer click keyboard clip-to-host clip-to-client audio reconnect stop-sharing encoder-fallback; do
    printf '%-17s %s\n' "$k" "${RESULT[$k]:-FAIL not run}"
done | tee "$OUT/result.txt"
! grep -q '^[a-z-]* *FAIL' "$OUT/result.txt"

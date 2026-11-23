#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# The SodiumOS client's clipboard, sound and input-method text, end to end, away from the
# live session:
# - a nested Hyprland with a headless output, a private D-Bus and a private fcitx5 (own
#   HOME/XDG dirs) with Mozc and Rime Pinyin, typed into with wtype on the nested seat;
# - a private PipeWire (no session manager) whose only listener is a pw-cat recorder linked
#   to the client's stream, so nothing plays on this PC's speakers;
# - host-desktop's fake host on a random port (no mDNS): a 440 Hz tone, an in-memory
#   clipboard and an input log;
# - remote-client (windowed, in the nested compositor), paired by code over the LAN address;
#   or, with `app`, the SodiumOS app (apps/build/bin/remote: the engine's video as a subsurface
#   under Qt's window, with Qt's own input-method plug-ins loaded as on the live desktop).
# Checks: the host logs Text("日本語") and Text("你好") and keys for plain typing; the host's
# clipboard lands in the nested clipboard; text, HTML and PNG copied there reach the host;
# the recording's level matches the tone.
#
#   client-linux/tests/nested-io.sh [app]      output in remote/build/nested-io[-app]/
set -euo pipefail
R=$(cd "$(dirname "$0")/../.." && pwd)
REPO=$(cd "$R/.." && pwd)
T=$R/target/debug
MODE=${1:-client}
OUT=$R/build/nested-io${1:+-$1}
APP=$REPO/apps/build/bin/remote
[[ $MODE == client || -x $APP ]] || die "no $APP: cmake --build apps/build --target remote"
LIVE_RUNTIME=$XDG_RUNTIME_DIR
die() { echo "nested-io: $*" >&2; exit 1; }

cargo build -q --manifest-path "$R/Cargo.toml" -p remote-client -p host-desktop
rm -rf "$OUT"
mkdir -p "$OUT"/{home,config/fcitx5,data/fcitx5,cache,pw}
OUT=$(cd "$OUT" && pwd)
RUN=$LIVE_RUNTIME/sodium-remote-nio
rm -rf "$RUN"
mkdir -m 700 "$RUN"

pids=()
SIG=
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
    sleep 0.3
    [[ -n $SIG && $SIG != "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] && "$REPO/scripts/nested-hyprland.sh" stop "$SIG" || true
    pkill -f "$OUT/home" 2>/dev/null || true
    rm -rf -- "$RUN"
}
trap cleanup EXIT

# ---- the private PipeWire, with a recorder for the client's stream ----------------
PWRUN=$(mktemp -u /tmp/nio-pw-XXXXXX)
ln -s "$OUT/pw" "$PWRUN"
chmod 700 "$OUT/pw"
pwenv=(env -u DBUS_SESSION_BUS_ADDRESS -u PIPEWIRE_CONFIG_DIR -u PIPEWIRE_CONFIG_NAME XDG_RUNTIME_DIR="$PWRUN" PIPEWIRE_RUNTIME_DIR="$PWRUN" PIPEWIRE_REMOTE=pipewire-0
    PIPEWIRE_PROPS="{ adapter.auto-port-config = { mode = dsp monitor = false position = preserve } }")
"${pwenv[@]}" pipewire -c "$REPO/apps/voicerecorder/tests/private-pipewire.conf" >"$OUT/pipewire.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do [[ -S $PWRUN/pipewire-0 ]] && break; sleep 0.1; done
[[ -S $PWRUN/pipewire-0 ]] || die "the private PipeWire didn't start"
"${pwenv[@]}" pw-cat --record -P '{ node.name = nio-rec }' --rate 48000 --channels 2 --format f32 "$OUT/rec.wav" >"$OUT/rec.log" 2>&1 &
REC=$!
pids+=($REC)

# ---- the fake host, and pairing -------------------------------------------------------
PORT=$((47400 + RANDOM % 400))
"$T/sodium-remote-host" --backend fake --encoder openh264 --data "$OUT/host" --port $PORT --no-mdns --pair \
    --input-log "$OUT/host-input.csv" >"$OUT/host.out" 2>"$OUT/host.log" &
pids+=($!)
for _ in $(seq 100); do [[ -s $OUT/host/pairing-code.txt ]] && break; sleep 0.1; done
ID=$(sed -n 's/^id=//p' "$OUT/host/host.txt")
CODE=$(cat "$OUT/host/pairing-code.txt")
"$T/remote-client" --data "$OUT/client" --no-rendezvous --no-mdns pair --code "$CODE" --address "127.0.0.1:$PORT" >"$OUT/pair.out" 2>&1 \
    || { cat "$OUT/pair.out"; die "pairing failed"; }

# ---- nested Hyprland, private bus, private fcitx5 ------------------------------------
"$REPO/scripts/nested-hyprland.sh" room
{
    echo "dofile(\"$REPO/spikes/nested/minimal.lua\")"
    echo 'hl.monitor({ output = "NIO-1", mode = "1280x800@60", position = "0x0", scale = "1" })'
    echo 'hl.monitor({ output = "WAYLAND-1", disabled = true })'
    echo 'hl.config({ animations = { enabled = false } })'
} >"$OUT/hypr.lua"
SIG=$("$REPO/scripts/nested-hyprland.sh" start "$OUT/hypr.lua")
[[ $SIG != "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || die "got the live compositor"
sock=$(hyprctl instances -j | python3 -I -c "import json,sys; print([i['wl_socket'] for i in json.load(sys.stdin) if i['instance']=='$SIG'][0])")
WD=$LIVE_RUNTIME/$sock
[[ $(basename "$WD") != "${WAYLAND_DISPLAY:-none}" ]] || die "that is the live display"
hyprctl -i "$SIG" output create headless NIO-1 >/dev/null
sleep 1

cat >"$OUT/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$RUN</listen>
  <auth>EXTERNAL</auth>
  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>
EOF
dbus-daemon --config-file="$OUT/bus.conf" --nofork --print-address=3 3>"$OUT/bus.addr" </dev/null >/dev/null 2>&1 &
pids+=($!)
for _ in $(seq 50); do [[ -s $OUT/bus.addr ]] && break; sleep 0.1; done
BUS=$(head -n1 "$OUT/bus.addr")
case $BUS in *"$LIVE_RUNTIME/bus"* | "") die "not a private bus ($BUS)" ;; esac

cat >"$OUT/config/fcitx5/profile" <<'EOF'
[Groups/0]
Name=Default
Default Layout=us
DefaultIM=keyboard-us

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/0/Items/1]
Name=mozc
Layout=

[Groups/0/Items/2]
Name=rime-pinyin-hans
Layout=

[GroupOrder]
0=Default
EOF
# Rime's compiled schemas (not the user's dictionary or sync data).
mkdir -p "$OUT/data/fcitx5/rime"
cp -r "$HOME/.local/share/fcitx5/rime/build" "$HOME/.local/share/fcitx5/rime/default.custom.yaml" "$OUT/data/fcitx5/rime/" 2>/dev/null || true

nested() {
    env -u DISPLAY -u XMODIFIERS -u QT_IM_MODULE -u QT_IM_MODULES -u GTK_IM_MODULE -u WAYLAND_SOCKET \
        HOME="$OUT/home" XDG_CONFIG_HOME="$OUT/config" XDG_DATA_HOME="$OUT/data" XDG_CACHE_HOME="$OUT/cache" \
        XDG_RUNTIME_DIR="$RUN" WAYLAND_DISPLAY="$WD" DBUS_SESSION_BUS_ADDRESS="$BUS" HYPRLAND_INSTANCE_SIGNATURE="$SIG" "$@"
}
nested fcitx5 --disable=xcb,xim >"$OUT/fcitx5.log" 2>&1 </dev/null &
pids+=($!)
for _ in $(seq 100); do
    nested gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner org.fcitx.Fcitx5 2>/dev/null | grep -q true && break
    sleep 0.1
done
sleep 1

# ---- the client, in the nested compositor, sound to the private PipeWire ---------------
pwclient=(PIPEWIRE_RUNTIME_DIR="$PWRUN" PIPEWIRE_REMOTE=pipewire-0 RUST_LOG=warn REMOTE_LOG_IME=1
    PIPEWIRE_PROPS="{ adapter.auto-port-config = { mode = dsp monitor = false position = preserve } }")
if [[ $MODE == app ]]; then
    mkdir -p "$OUT/state"
    printf '2500 address %s@127.0.0.1:%s\n1500 connect %s\n60000 quit\n' "$ID" "$PORT" "$ID" >"$OUT/script.txt"
    # As on the live desktop: Qt's text-input-v3 plug-in first, fcitx's (here on the private bus) after it.
    nested env "${pwclient[@]}" QT_QPA_PLATFORM=wayland QT_IM_MODULE=fcitx "QT_IM_MODULES=wayland;fcitx" QT_FORCE_STDERR_LOGGING=1 \
        SODIUMOS_STATE="$OUT/state" QML_IMPORT_PATH="$REPO/ui" REMOTE_AUDIO_LOG="$OUT/audio.csv" \
        "$APP" --data "$OUT/client" --lan-only --script "$OUT/script.txt" >"$OUT/client.out" 2>"$OUT/client.log" &
else
    nested env "${pwclient[@]}" \
        "$T/remote-client" --data "$OUT/client" connect --no-rendezvous --no-mdns --lan-hint "$ID@127.0.0.1:$PORT" \
        --decoder sw --no-hud --audio-log "$OUT/audio.csv" --stats-dir "$OUT" >"$OUT/client.out" 2>"$OUT/client.log" &
fi
CLIENT=$!
pids+=($CLIENT)
for _ in $(seq 300); do grep -q "audio: PipeWire" "$OUT/client.log" 2>/dev/null && break; sleep 0.1; done
grep -q "audio: PipeWire" "$OUT/client.log" || { tail -n 20 "$OUT/client.log"; die "the session never started"; }
sleep 2
# Link the client's stream to the recorder (no session manager here).
{ "${pwenv[@]}" pw-link -o; "${pwenv[@]}" pw-link -i; } >"$OUT/ports.txt" 2>&1 || true
for ch in FL FR; do "${pwenv[@]}" pw-link "sodium-remote:output_$ch" "nio-rec:input_$ch" 2>>"$OUT/rec.log" || true; done
nested hyprctl clients | grep -E "class|focus" >"$OUT/clients.txt" || true

# ---- input method -------------------------------------------------------------------
type_() { for item in "$@"; do case $item in key:*) nested wtype -k "${item#key:}" ;; *) nested wtype -d 30 -- "$item" ;; esac; sleep 0.15; done; }
im() { nested fcitx5-remote -s "$1"; sleep 0.5; echo "im: $(nested fcitx5-remote -n)"; }
im keyboard-us
type_ "abc"
im mozc
type_ "nihongo" key:space key:Return
sleep 0.5
im rime-pinyin-hans
type_ "nihao" key:space
sleep 0.5
im keyboard-us
type_ "xyz"
sleep 1
# Focus away and back (a window opens over the client, then closes), then compose again: the
# live desktop showed the input method working only until the first focus change.
nested kitty --config NONE -o confirm_os_window_close=0 sleep 30 >/dev/null 2>&1 &
KITTY=$!
# The stream window: the app's is titled with the host's name (its main window is "Remote" too).
[[ $MODE == app ]] && STREAMWIN="title:^Fake host$" || STREAMWIN="class:^remote-client$"
sleep 2
hyprctl -i "$SIG" dispatch "hl.dsp.focus({ window = \"$STREAMWIN\" })" >"$OUT/refocus.txt" 2>&1; hyprctl -i "$SIG" clients >>"$OUT/refocus.txt" 2>&1
sleep 1.5
im mozc
type_ "nihongo" key:space key:Return
sleep 0.5
im keyboard-us
kill $KITTY 2>/dev/null || true
if [[ $MODE == app ]]; then
    # Typing into the app's own main window during the session must not reach the host.
    hyprctl -i "$SIG" dispatch 'hl.dsp.focus({ window = "title:^Remote$" })' >/dev/null
    sleep 1
    type_ "qq"
    sleep 0.5
fi

# ---- clipboard -----------------------------------------------------------------------
echo "nested clipboard (from the host): $(nested timeout 3 wl-paste -n 2>&1 || true)"
nested wl-copy "from nested 日本 Ü"
sleep 1
nested wl-copy -t text/html "<b>bold</b> from nested"
sleep 1
python3 -I - "$OUT/one.png" <<'EOF'
import struct, sys, zlib
def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
raw = b"".join(b"\0" + b"\xff\x00\x00" * 4 for _ in range(4))
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 4, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))
EOF
nested wl-copy -t image/png <"$OUT/one.png"
sleep 2
kill $REC 2>/dev/null || true
sleep 0.3
kill $CLIENT 2>/dev/null || true
wait $CLIENT 2>/dev/null || true

# ---- results --------------------------------------------------------------------------
echo "--- host input log ($(wc -l <"$OUT/host-input.csv") events):"
grep -E "Text" "$OUT/host-input.csv" || echo "(no Text events)"
echo "commits: $(grep -c 'Text(' "$OUT/host-input.csv" || true) (3 expected: 日本語, 你好, 日本語 after the focus change)"
echo "keys: $(grep -c Key "$OUT/host-input.csv" || true) (12 expected: abc and xyz, down and up)"
echo "--- host clipboard:"
grep -E "clipboard" "$OUT/host.log" || true
echo "--- client:"
grep -E "clipboard|audio|text-input|error" "$OUT/client.log" | tail -n 12 || true
echo "--- recording:"
python3 -I - "$OUT/rec.wav" <<'EOF'
import math, struct, sys, wave
try:
    w = wave.open(sys.argv[1])
    n = w.getnframes(); data = w.readframes(n)
except Exception as e:
    # pw-cat writes float WAV, which the wave module refuses: read the samples after the header.
    raw = open(sys.argv[1], "rb").read()
    i = raw.find(b"data") + 8
    data = raw[i:]
s = struct.unpack("<%df" % (len(data) // 4), data[: len(data) // 4 * 4])
loud = [x for x in s if x != 0.0]
rms = math.sqrt(sum(x * x for x in s) / max(len(s), 1))
rms_loud = math.sqrt(sum(x * x for x in loud) / max(len(loud), 1))
print(f"{len(s) // 2 / 48000:.1f} s recorded, RMS {rms:.4f} overall, {rms_loud:.4f} where not silent (the tone: 0.1768)")
EOF

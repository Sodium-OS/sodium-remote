#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Remote end to end on the real stack, nested only:
#   - the host: a nested Hyprland with the sodiumwm plugin built from this tree
#     (compositor/plugin/build.sh --test) and the click test surface; `remoted run`
#     against it (no audio, no IME), on a private D-Bus bus with a stand-in
#     notification server, its key and state under the run's folder;
#   - pairing: remoted's OpenPairing (the code), then `remote-client pair`,
#     over the LAN (mDNS) or through a rendezvous server (--rendezvous;
#     the host key in --hostdata must be enrolled with the relay's enrolment);
#   - the reference client, headless, or full screen in a second nested Hyprland
#     whose only output is a headless CLIENT-1 ("the client screen", --windowed),
#     running the click-to-photon test, then idle.
#
#   run.sh OUTDIR [--windowed] [--fps N] [--clicks N] [--chroma auto|420|444]
#                 [--rendezvous] [--relay-only] [--hostdata DIR] [--ppp F]   (default 1.741: every
#                 run stands in for the 13" Air, REMOTE-1 at scale 1.6; integration.md 8)
#                 [--client-mode WxH] [--client-extra "ARGS"] [--hold-secs N]
#                 [--home mirror|privacy|off]   (the desk's mode for the session; then
#                 the run also switches it live and records `hyprctl remotelocalinput`)
#                 [--host-only]   (only the host side: for an external client, e.g. the Windows VM's
#                 remote-client.exe. Writes OUTDIR/host-only.env (HOST_ID, LAN_PORT), keeps a pairing code
#                 fresh in OUTDIR/pair-code, and runs until OUTDIR/stop exists. The test surface's log is
#                 OUTDIR/host/test-surface.csv)
#                 [--anim]   (a window on the host that animates every frame, for pacing)
#                 [--text]   (the text row, the dense page
#                 (spikes/remote-video/page.qml) floats on REMOTE-1 right of the click marker
#                 once the click test is over (the first picture with --clicks 0), and flips
#                 8 times 2.5 s apart, the first 10 s after the first frame and 5 s after the
#                 page opened (the click test ends 7.5 s after its last click), the client
#                 idle; analyze.py reports each flip end to end. The idle window is 2.5 s)
#
# At most two nested instances, only for the run. Leaves nothing running.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out=$(realpath -m "${1:?usage: run.sh OUTDIR [options]}")
shift
audio=0 fps=60 clicks=40 windowed=0 chroma=auto rdv=0 relay=0 hostdata="" ppp=1.741 client_mode=2560x1664 extra="" hold="" home="" anim=0 text=0 host_only=0
while (($#)); do
    case $1 in
    --windowed) windowed=1; shift ;;
    --fps) fps=$2; shift 2 ;;
    --clicks) clicks=$2; shift 2 ;;
    --chroma) chroma=$2; shift 2 ;;
    --rendezvous) rdv=1; shift ;;
    --relay-only) relay=1; shift ;;
    --hostdata) hostdata=$(realpath -m "$2"); shift 2 ;;
    --ppp) ppp=$2; shift 2 ;;
    --client-mode) client_mode=$2; shift 2 ;;
    --client-extra) extra=$2; shift 2 ;;
    --hold-secs) hold=$2; shift 2 ;;
    --home) home=$2; shift 2 ;;
    --anim) anim=1; shift ;;
    --text) text=1; shift ;;
    --host-only) host_only=1; shift ;;
    --audio) audio=1; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
rm -rf "$out"
mkdir -p "$out/host" "$out/client" "$out/run"
hostdata=${hostdata:-$out/hostdata}
clientdata=$out/clientdata
log() { echo "[$(date +%T.%3N)] $*" | tee -a "$out/run.log"; }

(cd "$repo/remote" && cargo build --release -q -p remoted -p remote-client) || { echo "build failed"; exit 1; }
(cd "$here" && cargo build --release -q) || { echo "harness build failed"; exit 1; }
so=$repo/compositor/plugin/build-test/sodiumwm.so
[[ -f $so ]] || "$repo/compositor/plugin/build.sh" --test >"$out/plugin-build.log" 2>&1 || { echo "plugin build failed"; exit 1; }
remoted=$repo/remote/target/release/remoted
client=$repo/remote/target/release/remote-client
bin=$here/target/release

sigs=() pids=() bus="" bus_pid="" rpid="" cpid="" pwdir=""
cleanup() {
    [[ -n $cpid ]] && kill -INT "$cpid" 2>/dev/null && sleep 1
    [[ -n $rpid ]] && kill "$rpid" 2>/dev/null && sleep 2
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
    sleep 0.3
    for p in "${pids[@]}" $rpid $cpid; do kill -9 "$p" 2>/dev/null || true; done
    [[ -n $bus_pid ]] && kill "$bus_pid" 2>/dev/null
    for s in "${sigs[@]}"; do "$repo/scripts/nested-hyprland.sh" stop "$s" >/dev/null 2>&1 || true; done
    rm -f "$sock"
    [[ -n $pwdir ]] && rm -rf "$pwdir"
}
trap cleanup EXIT
trap "exit 130" INT TERM

socket_of() { hyprctl instances -j | jq -r --arg s "$1" '.[] | select(.instance == $s) | .wl_socket'; }
mono_ns() { python3 -I -c 'import time; print(time.clock_gettime_ns(time.CLOCK_MONOTONIC))'; }

# ---- a private bus for remoted's D-Bus API, notifications and logind (never the live ones)
cat >"$out/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$XDG_RUNTIME_DIR</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
dbus-daemon --config-file="$out/bus.conf" --nofork --print-address=3 3>"$out/bus.addr" </dev/null >/dev/null 2>&1 &
bus_pid=$!
for _ in $(seq 50); do [[ -s $out/bus.addr ]] && break; sleep 0.1; done
bus=$(head -n1 "$out/bus.addr")
[[ -n $bus && $bus != "${DBUS_SESSION_BUS_ADDRESS:-}" && $bus != *"$XDG_RUNTIME_DIR/bus" ]] || { echo "no private bus"; exit 1; }
DBUS_SESSION_BUS_ADDRESS=$bus python3 -I "$repo/spikes/remote-session/fake-notifications.py" >"$out/host/notifications.jsonl" 2>/dev/null &
pids+=($!)
api() { busctl --address="$bus" call org.sodiumos.Remote /org/sodiumos/Remote org.sodiumos.Remote "$@" --json=short; }
prop() { busctl --address="$bus" get-property org.sodiumos.Remote /org/sodiumos/Remote org.sodiumos.Remote "$1" --json=short; }

# ---- the host
sock=$XDG_RUNTIME_DIR/rm-e2e-$$.sock
hsig=$(REMOTE_PLUGIN=$so SODIUMWM_REMOTE_SOCKET=$sock DESK_MODE=1920x1080 "$repo/scripts/nested-hyprland.sh" start "$here/host.lua")
[[ -n $hsig && $hsig != "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || { echo "bad nested signature" >&2; exit 1; }
sigs+=("$hsig")
hwd=$(socket_of "$hsig")
h() { hyprctl -i "$hsig" "$@"; }
sleep 1
h dismissnotify >/dev/null
for _ in $(seq 50); do [[ -S $sock ]] && break; sleep 0.1; done
[[ -S $sock ]] || { echo "the plugin's socket didn't appear"; h plugin list; exit 1; }
WAYLAND_DISPLAY=$hwd HYPRLAND_INSTANCE_SIGNATURE=$hsig "$bin/test-surface" --log "$out/host/test-surface.csv" >"$out/host/test-surface.log" 2>&1 &
pids+=($!)
if ((anim)); then
    WAYLAND_DISPLAY=$hwd QT_QPA_PLATFORM=wayland setsid /usr/lib/qt6/bin/qml "$repo/spikes/remote-plugin/pattern.qml" -- --anim \
        >"$out/host/anim.log" 2>&1 &
    pids+=($!)
fi
sleep 1

# --audio: a private PipeWire and WirePlumber (their own runtime dir, no device), so remoted's "Remote" sink and
# microphone never appear in the live one. A test tone: PIPEWIRE_RUNTIME_DIR=$(readlink OUTDIR/pw) pw-cat -p --target sodiumos-remote FILE (a sink exists only while a fresh session runs).
audio_flag=--no-audio
if ((audio)); then
    # The runtime dir is short (a socket path has 108 bytes at most; a worktree's OUTDIR is longer): OUTDIR/pw links to it.
    pwdir=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/remote-e2e-pw.XXXXXX")
    ln -sfn "$pwdir" "$out/pw"
    DBUS_SESSION_BUS_ADDRESS=$bus PIPEWIRE_RUNTIME_DIR=$pwdir pipewire >"$out/host/pipewire.log" 2>&1 &
    pids+=($!)
    for _ in $(seq 50); do [[ -S $pwdir/pipewire-0 ]] && break; sleep 0.1; done
    # A private WirePlumber beside it, for the "default" metadata remoted reads, with every device monitor off and
    # config, state and cache folders of its own: with the user's XDG_STATE_HOME it rewrites the live default-nodes.
    mkdir -p "$out/wp/config/wireplumber/wireplumber.conf.d" "$out/wp/state" "$out/wp/cache"
    cat >"$out/wp/config/wireplumber/wireplumber.conf.d/90-private.conf" <<'WP'
wireplumber.profiles = {
  main = {
    monitor.alsa = disabled
    monitor.alsa-midi = disabled
    monitor.bluez = disabled
    monitor.bluez-midi = disabled
    monitor.libcamera = disabled
    monitor.v4l2 = disabled
  }
}
WP
    DBUS_SESSION_BUS_ADDRESS=$bus PIPEWIRE_RUNTIME_DIR=$pwdir XDG_CONFIG_HOME=$out/wp/config \
        XDG_STATE_HOME=$out/wp/state XDG_CACHE_HOME=$out/wp/cache wireplumber >"$out/host/wireplumber.log" 2>&1 &
    pids+=($!)
    for _ in $(seq 50); do PIPEWIRE_RUNTIME_DIR=$pwdir pw-metadata -n default >/dev/null 2>&1 && break; sleep 0.1; done
    audio_flag=""
fi
pwenv=()
((audio)) && pwenv=(PIPEWIRE_RUNTIME_DIR="$pwdir")
net=()
((rdv)) || net+=(--no-rendezvous)
((relay)) && net+=(--relay-only)
env -u DISPLAY DBUS_SESSION_BUS_ADDRESS="$bus" DBUS_SYSTEM_BUS_ADDRESS="$bus" WAYLAND_DISPLAY="$hwd" \
    HYPRLAND_INSTANCE_SIGNATURE="$hsig" REMOTED_DATA_DIR="$hostdata" REMOTED_RUNTIME_DIR="$out/run" \
    "${pwenv[@]}" ${hold:+REMOTED_HOLD_SECS=$hold} RUST_LOG=${RUST_LOG:-info} \
    "$remoted" run $audio_flag --no-ime --socket "$sock" --log-dir "$out/host" --chroma "$chroma" "${net[@]}" \
    >"$out/host/remoted.log" 2>&1 &
rpid=$!
for _ in $(seq 100); do busctl --address="$bus" status org.sodiumos.Remote >/dev/null 2>&1 && break; sleep 0.1; done
busctl --address="$bus" status org.sodiumos.Remote >/dev/null 2>&1 || { echo "remoted didn't come up"; cat "$out/host/remoted.log"; exit 1; }
host_id=$(prop DeviceId | jq -r .data)
log "remoted up: $host_id"
if ((rdv)); then
    for _ in $(seq 100); do [[ $(prop Relays | jq -r '.data[0].state.data') == online ]] && break; sleep 0.1; done
    log "rendezvous: $(prop Relays | jq -c '.data[0].state.data')"
fi

if [[ -n $home ]]; then
    busctl --address="$bus" set-property org.sodiumos.Remote /org/sodiumos/Remote org.sodiumos.Remote HomeMode s "$home"
fi

if ((host_only)); then
    port=$(sed -n 's/.*LAN listener on port \([0-9]*\).*/\1/p' "$out/host/remoted.log" | head -1)
    printf 'HOST_ID=%s\nLAN_PORT=%s\n' "$host_id" "$port" >"$out/host-only.env"
    log "host only: $host_id, LAN port $port; touch $out/stop to end"
    while [[ ! -e $out/stop ]]; do
        api OpenPairing | jq -r '.data[0]' >"$out/pair-code"
        for _ in $(seq 240); do [[ -e $out/stop ]] && break; sleep 1; done
    done
    exit 0
fi

# ---- pairing (the code never leaves the client: SPAKE2 over mDNS/LAN or the rendezvous)
code=$(api OpenPairing | jq -r '.data[0]')
log "pairing window open"
pair_net=("${net[@]}")
((rdv)) && pair_net+=(--no-mdns) # through the rendezvous, not the LAN
t0=$(date +%s%N)
"$client" --data "$clientdata" pair --code "$code" "${pair_net[@]}" >"$out/client/pair.log" 2>&1
st=$?
log "pair exit $st in $(( ($(date +%s%N) - t0) / 1000000 )) ms: $(tail -n1 "$out/client/pair.log")"
((st == 0)) || exit 1
prop PairedDevices >"$out/host/paired.json"

# ---- the client
idle_ms=30000
((text)) && idle_ms=2500 # the text row's run measures the flips, not the idle window
args=(--data "$clientdata" connect --host "$host_id" --stats-dir "$out/client" --clicks "$clicks" --idle-ms "$idle_ms" --fps "$fps" "${net[@]}")
[[ -n $ppp ]] && args+=(--ppp "$ppp")
# shellcheck disable=SC2206
[[ -n $extra ]] && args+=($extra)
if ((windowed)); then
    csig=$("$repo/scripts/nested-hyprland.sh" start "$here/client.lua")
    [[ -n $csig && $csig != "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || { echo "bad nested signature" >&2; exit 1; }
    sigs+=("$csig")
    cwd=$(socket_of "$csig")
    c() { hyprctl -i "$csig" "$@"; }
    crun() { WAYLAND_DISPLAY=$cwd HYPRLAND_INSTANCE_SIGNATURE=$csig "$@"; }
    c output create headless CLIENT-1 >/dev/null
    c eval "hl.monitor({ output = \"CLIENT-1\", mode = \"$client_mode@$fps\", position = \"0x0\", scale = \"1\" })" >/dev/null
    c eval 'hl.monitor({ output = "WAYLAND-1", disabled = true })' >/dev/null
    sleep 1
    c monitors >"$out/client/monitors.txt"
    WAYLAND_DISPLAY=$cwd HYPRLAND_INSTANCE_SIGNATURE=$csig "$client" "${args[@]}" --fullscreen --stay --secs 300 \
        --input-log "$out/client/input.csv" >"$out/client/client.log" 2>&1 &
else
    "$client" "${args[@]}" --headless --size "$client_mode" --stay --secs 300 >"$out/client/client.log" 2>&1 &
fi
cpid=$!
log "client started"
if ((text)); then
    t_first=""
    for _ in $(seq 300); do
        grep -q "first picture" "$out/client/client.log" && { t_first=$(mono_ns); break; }
        kill -0 $cpid 2>/dev/null || break
        sleep 0.1
    done
    log "first picture (text row clock $t_first)"
fi
for _ in $(seq 600); do
    ((text && clicks == 0)) && break
    grep -q "click test done" "$out/client/client.log" && break
    kill -0 $cpid 2>/dev/null || break
    sleep 0.5
done
log "click test over"

# ---- the text row: the page floats right of the click marker, then flips
text_row() {
    [[ -n $t_first ]] || { log "text row: no first picture"; return 1; }
    WAYLAND_DISPLAY=$hwd QT_QPA_PLATFORM=wayland QML_XHR_ALLOW_FILE_READ=1 QT_FORCE_STDERR_LOGGING=1 setsid /usr/lib/qt6/bin/qml \
        "$repo/spikes/remote-video/page.qml" -- --go "$out/host/text-go" --flips 8 >"$out/host/page.log" 2>&1 &
    pids+=($!)
    local addr="" geo
    for _ in $(seq 100); do
        addr=$(h clients -j | jq -r '.[] | select(.title == "remote-page") | .address' | head -1)
        [[ -n $addr ]] && break
        sleep 0.1
    done
    [[ -n $addr ]] || { log "text row: the page window didn't open"; return 1; }
    # REMOTE-1's logical size; the marker is 64..320 stream pixels from the top left
    geo=$(h monitors -j | jq -r '.[] | select(.name == "REMOTE-1") | "\(.width) \(.height) \(.scale)"')
    read -r W H S <<<"$geo"
    read -r px pw ph < <(python3 -I -c 'import sys; w, h, s = map(float, sys.argv[1:4]); x = int(330 / s) + 10; print(x, int(w / s) - x - 20, int(h / s) - 40)' "$W" "$H" "$S")
    h eval "hl.dispatch(hl.dsp.window.resize({ x = $pw, y = $ph, window = \"address:$addr\" }))" >/dev/null
    h eval "hl.dispatch(hl.dsp.window.move({ x = $px, y = 20, window = \"address:$addr\" }))" >/dev/null
    local t_page now go
    t_page=$(mono_ns)
    go=$(( t_first + 10000000000 ))
    (( t_page + 5000000000 > go )) && go=$(( t_page + 5000000000 ))
    log "text row: page ${pw}x${ph} at ${px},20 on REMOTE-1 (${W}x${H} scale $S); first flip in $(( (go - t_page) / 1000000 )) ms"
    now=$(mono_ns)
    (( go > now )) && sleep "$(python3 -I -c "print(($go - $now) / 1e9)")"
    mono_ns >"$out/host/text-go.ns"
    echo go >"$out/host/text-go"
    log "text row: flipping"
    sleep 23 # 8 flips 2.5 s apart and the last one's refinement
    h monitors -j >"$out/host/text-monitors.json"
    log "text row: $(grep -c 'remote-page: flip' "$out/host/page.log") flips"
}
((text)) && { text_row || true; }
h monitors all >"$out/host/monitors.txt"
h repl 'hl.plugin.sodiumwm.remote_debug()' >"$out/host/plugin-debug.json" 2>/dev/null || true
WAYLAND_DISPLAY=$hwd grim -o REMOTE-1 "$out/host-screen.png" 2>/dev/null || true
if ((windowed)); then
    crun grim -o CLIENT-1 "$out/client-screen.png" || true
    # Input through the client window: keys (r e m o t e Enter, evdev through a
    # virtual keyboard), the wheel and a finger scroll; the test surface logs them.
    crun "$bin/vinput" move 700 500 || true
    sleep 0.3
    crun "$bin/vinput" keys 19 18 50 24 20 18 28 || true
    crun "$bin/vinput" wheel 2 || true
    sleep 0.2
    crun "$bin/vinput" finger 120 || true
    sleep 1
fi
prop Session >"$out/host/session.json"
if [[ -n $home ]]; then
    { echo "home mode $home at connect:"; h remotelocalinput; } >"$out/host/local-input.txt"
    for m in mirror privacy; do
        api SetSessionHomeMode s "$m" >/dev/null
        sleep 0.5
        { echo "after SetSessionHomeMode $m:"; h remotelocalinput; } >>"$out/host/local-input.txt"
    done
fi
kill -INT $cpid 2>/dev/null
wait $cpid
log "client exit $?"
cpid=""
for _ in $(seq 50); do [[ $(prop State | jq -r .data) == ready ]] && break; sleep 0.1; done
log "host state after disconnect: $(prop State | jq -r .data)"
h monitors all >"$out/host/monitors-after.txt"
kill "$rpid" 2>/dev/null
wait "$rpid"
rpid=""
python3 -I "$here/analyze.py" "$out" | tee "$out/summary.md"

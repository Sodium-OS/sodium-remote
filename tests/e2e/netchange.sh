#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Remote: a mid-stream network change on the real stack ("Network
# change: path switch or reconnect in under 2 s without restarting the session").
#
#   netchange.sh OUTDIR        (CUT=a|b forces which link is cut; default: the one the log says
#                              the stream uses. The default route is on link a: cutting it is the
#                              case where the host loses the client for a while.)
#   NARROW=20mbit netchange.sh OUTDIR   a narrow link instead:
#                              both links shaped host → client by netem (`rate 20mbit`, a deep
#                              FIFO), an idle desktop (no animating window: probes need a quiet
#                              link), the client at NARROW_SIZE (2940x1848: a 65 Mbit/s demand)
#                              for 20 s and no cuts; prints the controller from host-link.csv.
#
# remoted (against a nested Hyprland with the plugin from this tree and an
# animating window, so a frame comes every refresh) runs in network namespace
# rm1-h, the reference client (headless) in rm1-c. Two links join them, like a
# Mac with Wi-Fi and a second path:
#   rm1-h 10.10.1.1 ── link A ── 10.10.1.2 rm1-c
#   rm1-h 10.10.2.1 ── link B ── 10.10.2.2 rm1-c
# No rendezvous: pairing and signalling go over mDNS and remoted's LAN listener.
# Events (CLOCK_MONOTONIC, logged in events.csv):
#   1. cut: the link the stream uses goes down at the client: failover to the other;
#   2. back: it comes back;
#   3. move: link B goes down and the client's address on link A changes
#      (.2 → .3: a new DHCP lease), so no route survives and the client must
#      bring its new candidates to the host itself (signalling reopened).
# analyze: the gap in video after each event, and whether the session restarted.
# The namespaces are created inside themselves (`ip -n`): the host's own
# interfaces, routes and firewall are untouched. Everything stops on exit.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out=$(realpath -m "${1:?usage: netchange.sh OUTDIR}")
rm -rf "$out"
mkdir -p "$out/host" "$out/client" "$out/run" "$out/hostdata"
user=$(id -un)
sudo -n true || { echo "needs passwordless sudo for the namespaces"; exit 1; }
remoted=$repo/remote/target/release/remoted
client=$repo/remote/target/release/remote-client
so=$repo/compositor/plugin/build-test/sodiumwm.so
mono() { python3 -I -c 'import time; print(time.clock_gettime_ns(time.CLOCK_MONOTONIC))'; }
event() { echo "$1,$(mono)" >>"$out/events.csv"; echo "[$(date +%T.%3N)] $1"; }
as() { local n=$1; shift; sudo ip netns exec "$n" runuser -u "$user" -- "$@"; }

sig="" bus_pid="" rpid="" cpid="" qpid="" sock=""
cleanup() {
    [[ -n $cpid ]] && sudo kill -INT "$cpid" 2>/dev/null && sleep 1
    [[ -n $rpid ]] && sudo kill "$rpid" 2>/dev/null && sleep 2
    for p in $cpid $rpid $qpid; do sudo kill -9 "$p" 2>/dev/null || true; done
    pkill -f "rm1-netchange-anim" 2>/dev/null || true
    [[ -n $bus_pid ]] && kill "$bus_pid" 2>/dev/null
    [[ -n $sig ]] && "$repo/scripts/nested-hyprland.sh" stop "$sig" >/dev/null 2>&1
    sudo ip netns del rm1-h 2>/dev/null || true
    sudo ip netns del rm1-c 2>/dev/null || true
    rm -f "$sock"
}
trap cleanup EXIT
trap "exit 130" INT TERM

# ---- the namespaces
sudo ip netns del rm1-h 2>/dev/null; sudo ip netns del rm1-c 2>/dev/null
sudo ip netns add rm1-h && sudo ip netns add rm1-c
for n in rm1-h rm1-c; do sudo ip -n "$n" link set lo up; done
for l in a b; do
    i=$([[ $l == a ]] && echo 1 || echo 2)
    sudo ip -n rm1-h link add "$l-h" type veth peer name "$l-c" netns rm1-c
    sudo ip -n rm1-h addr add "10.10.$i.1/24" dev "$l-h" && sudo ip -n rm1-h link set "$l-h" up
    sudo ip -n rm1-c addr add "10.10.$i.2/24" dev "$l-c" && sudo ip -n rm1-c link set "$l-c" up
done
sudo ip -n rm1-c route add default via 10.10.1.1 dev a-c
if [[ -n ${NARROW:-} ]]; then
    for l in a b; do sudo ip netns exec rm1-h tc qdisc add dev "$l-h" root netem rate "$NARROW" limit 4000; done
fi
echo "event,t_mono_ns" >"$out/events.csv"

# ---- a private bus; the host's nested Hyprland with the plugin and an animating window
cat >"$out/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$XDG_RUNTIME_DIR</listen>
  <auth>EXTERNAL</auth>
  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
</busconfig>
EOF
dbus-daemon --config-file="$out/bus.conf" --nofork --print-address=3 3>"$out/bus.addr" </dev/null >/dev/null 2>&1 &
bus_pid=$!
for _ in $(seq 50); do [[ -s $out/bus.addr ]] && break; sleep 0.1; done
bus=$(head -n1 "$out/bus.addr")
[[ -n $bus && $bus != "${DBUS_SESSION_BUS_ADDRESS:-}" ]] || { echo "no private bus"; exit 1; }
DBUS_SESSION_BUS_ADDRESS=$bus python3 -I "$repo/spikes/remote-session/fake-notifications.py" >"$out/host/notifications.jsonl" 2>/dev/null &
api() { busctl --address="$bus" call org.sodiumos.Remote /org/sodiumos/Remote org.sodiumos.Remote "$@" --json=short; }
prop() { busctl --address="$bus" get-property org.sodiumos.Remote /org/sodiumos/Remote org.sodiumos.Remote "$1" --json=short; }

sock=$XDG_RUNTIME_DIR/rm-nc-$$.sock
sig=$(REMOTE_PLUGIN=$so SODIUMWM_REMOTE_SOCKET=$sock "$repo/scripts/nested-hyprland.sh" start "$here/host.lua")
[[ -n $sig && $sig != "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || { echo "bad nested signature"; exit 1; }
wd=$(hyprctl instances -j | jq -r --arg s "$sig" '.[] | select(.instance == $s) | .wl_socket')
sleep 1
hyprctl -i "$sig" dismissnotify >/dev/null
for _ in $(seq 50); do [[ -S $sock ]] && break; sleep 0.1; done
if [[ -z ${NARROW:-} ]]; then
    WAYLAND_DISPLAY=$wd QT_QPA_PLATFORM=wayland setsid /usr/lib/qt6/bin/qml "$repo/spikes/remote-plugin/pattern.qml" -- --anim rm1-netchange-anim \
        >"$out/host/qml.log" 2>&1 &
    qpid=$!
fi

# ---- remoted in rm1-h
sudo ip netns exec rm1-h runuser -u "$user" -- env -u DISPLAY DBUS_SESSION_BUS_ADDRESS="$bus" DBUS_SYSTEM_BUS_ADDRESS="$bus" \
    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" WAYLAND_DISPLAY="$wd" HYPRLAND_INSTANCE_SIGNATURE="$sig" HOME="$HOME" \
    REMOTED_DATA_DIR="$out/hostdata" REMOTED_RUNTIME_DIR="$out/run" RUST_LOG=info \
    "$remoted" run --no-audio --no-ime --no-rendezvous --socket "$sock" --log-dir "$out/host" --chroma 420 \
    >"$out/host/remoted.log" 2>&1 &
rpid=$!
for _ in $(seq 100); do busctl --address="$bus" status org.sodiumos.Remote >/dev/null 2>&1 && break; sleep 0.1; done
host_id=$(prop DeviceId | jq -r .data)
port=$(grep -oP 'LAN listener on port \K[0-9]+' "$out/host/remoted.log")
code=$(api OpenPairing | jq -r '.data[0]')
as rm1-c env HOME="$HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" "$client" --data "$out/clientdata" pair --code "$code" --no-rendezvous \
    >"$out/client/pair.log" 2>&1 || { echo "pairing failed"; cat "$out/client/pair.log"; exit 1; }
event paired

# ---- the client in rm1-c, headless, for 40 s
sudo ip netns exec rm1-c runuser -u "$user" -- env HOME="$HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" RUST_LOG=info \
    "$client" --data "$out/clientdata" connect --host "$host_id" --no-rendezvous --lan-hint "$host_id@10.10.1.1:$port" \
    --headless --size "$([[ -n ${NARROW:-} ]] && echo "${NARROW_SIZE:-2940x1848}" || echo 1920x1200)" --fps 60 --stats-dir "$out/client" --secs 40 \
    >"$out/client/client.log" 2>&1 &
cpid=$!
for _ in $(seq 100); do grep -q "connected through" "$out/client/client.log" && break; sleep 0.1; done
event connected
if [[ -n ${NARROW:-} ]]; then
    sleep 20
    event end
    sudo kill -INT "$cpid" 2>/dev/null
    wait "$cpid" 2>/dev/null
    cpid=""
    sudo kill "$rpid" 2>/dev/null && wait "$rpid" 2>/dev/null
    rpid=""
    sudo ip netns exec rm1-h tc -s qdisc show >"$out/host/qdisc.txt"
    python3 -I -c 'import sys; sys.path.insert(0, sys.argv[1]); import analyze; print("\n".join(analyze.link_lines(sys.argv[2])))' "$here" "$out" |
        tee "$out/summary.md"
    exit 0
fi
sleep 8
active=$(grep -oP 'route \S+(?: \S+)? 10\.10\.\K[12](?=\.)' "$out/client/client.log" | tail -n1)
link=${CUT:-$([[ $active == 2 ]] && echo b || echo a)}
other=$([[ $link == a ]] && echo b || echo a)
echo "the stream uses link $link"
event "cut-$link"
sudo ip -n rm1-c link set "$link-c" down
sleep 7
event "back-$link"
sudo ip -n rm1-c link set "$link-c" up
[[ $link == a ]] && sudo ip -n rm1-c route replace default via 10.10.1.1 dev a-c
sleep 7
event "move"
sudo ip -n rm1-c link set b-c down
sudo ip -n rm1-c addr del 10.10.1.2/24 dev a-c
sudo ip -n rm1-c addr add 10.10.1.3/24 dev a-c
sudo ip -n rm1-c route replace default via 10.10.1.1 dev a-c
sleep 10
event end
sudo kill -INT "$cpid" 2>/dev/null
wait "$cpid" 2>/dev/null
cpid=""
python3 -I "$here/netchange.py" "$out" | tee "$out/summary.md"

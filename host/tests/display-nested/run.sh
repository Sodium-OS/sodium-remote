#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Nested test of remoted's display manager with the real sodiumwm:
# windows on a 2560 x 1440-point DESK (free, at the right edge, too big for the
# stream, snapped left, maximised, minimised, on a second space), then a Mac
# connecting at 2940 x 1838 (scale 2: 1470 x 919 points), a live resize, back,
# and a disconnect.
#   remote/host/tests/display-nested/run.sh
# Needs: cargo build -p remoted --example display_bed; the sodiumwm plugin
# (SODIUMWM_PLUGIN, default compositor/plugin/build/sodiumwm.so of the main checkout).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
OUT=${OUT:-$repo/build/remote-display-nested}
rm -rf "$OUT"; mkdir -p "$OUT/run"
BED=${BED:-$repo/remote/target/debug/examples/display_bed}
export SODIUMWM_PLUGIN=${SODIUMWM_PLUGIN:-$HOME/Code/SodiumOS/compositor/plugin/build/sodiumwm.so}
export SODIUMWM_TEST_DIR=$OUT REMOTED_RUNTIME_DIR=$OUT/run GSK_RENDERER=cairo
unset DISPLAY
[[ -x $BED ]] || { echo "build it first: cargo build -p remoted --example display_bed"; exit 1; }

sig=$("$repo/scripts/nested-hyprland.sh" start "$here/nested.lua" | tail -n1)
[[ -n $sig && $sig != "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || { echo "bad signature '$sig'"; exit 1; }
h() { hyprctl -i "$sig" "$@"; }
cleanup() {
    h clients -j 2>/dev/null | python3 -I -c 'import json,sys; [print(c["pid"]) for c in json.load(sys.stdin)]' | sort -u | xargs -r kill -9 2>/dev/null
    "$repo/scripts/nested-hyprland.sh" stop "$sig" >/dev/null 2>&1
}
trap cleanup EXIT
fails=0

h output create headless DESK >/dev/null; sleep 0.3; h reload >/dev/null
for _ in $(seq 50); do
    h monitors -j | python3 -I -c '
import json,sys
m=json.load(sys.stdin)
sys.exit(0 if len(m) == 1 and m[0]["name"] == "DESK" and m[0]["width"] == 3840 else 1)' && break
    sleep 0.1
done
h dispatch 'hl.dsp.focus({ monitor = "DESK" })' >/dev/null
sleep 1.5
echo "configerrors: $(h configerrors | tr '\n' ' ')"
"$repo/scripts/nested-hyprland.sh" room || exit 1

open() { # open NAME W H: a test window, prints its address
    local before after
    before=$(h clients -j | python3 -I -c 'import json,sys; print(" ".join(c["address"] for c in json.load(sys.stdin)))')
    h dispatch "hl.dsp.exec_cmd(\"python3 $repo/spikes/3-sodiumwm/testclient.py window $1 $2 $3\")" >/dev/null
    for _ in $(seq 80); do
        after=$(h clients -j | python3 -I -c '
import json, sys
seen = set(sys.argv[1].split())
new = [c["address"] for c in json.load(sys.stdin) if c["address"] not in seen and c["mapped"]]
print(new[0] if new else "")' "$before")
        [[ -n $after ]] && { sleep 0.5; echo "$after"; return 0; }
        sleep 0.1
    done
}
place() { # place ADDR X Y W H
    h dispatch "hl.dsp.window.resize({ x = $4, y = $5, window = \"address:$1\" })" >/dev/null; sleep 0.2
    h dispatch "hl.dsp.window.move({ x = $2, y = $3, window = \"address:$1\" })" >/dev/null; sleep 0.2
}
snapshot() { sleep "${2:-0}"; h clients -j >"$OUT/$1.json"; h monitors all -j >"$OUT/$1.mon.json"; h workspaces -j >"$OUT/$1.ws.json"; }
bed() { echo "  display_bed $*"; "$BED" "$sig" "$@" || { echo "FAIL display_bed $*"; fails=$((fails+1)); }; }
check() { # check NAME PYTHON (c[k] the window now, base[k] at 0-base, U the usable area of its monitor)
    if python3 -I - "$OUT" "$1" <<PY
import json, sys
out, name = sys.argv[1], sys.argv[2]
def load(n):
    cl = {c["class"].split(".")[-1]: c for c in json.load(open(f"{out}/{n}.json"))}
    mons = {m["id"]: m for m in json.load(open(f"{out}/{n}.mon.json"))}
    return cl, mons
c, mons = load(name)
base, _ = load("0-base")
def area(k):
    m = mons[c[k]["monitor"]]; s = m["scale"]; r = m["reserved"]
    return dict(name=m["name"], x=m["x"]+r[0], y=m["y"]+r[1], w=round(m["width"]/s)-r[0]-r[2], h=round(m["height"]/s)-r[1]-r[3])
def inside(k):
    A = area(k); w = c[k]
    return w["at"][0] >= A["x"] and w["at"][1] >= A["y"] and w["at"][0]+w["size"][0] <= A["x"]+A["w"] and w["at"][1]+w["size"][1] <= A["y"]+A["h"]
def rect(k, d=None): w = (d or c)[k]; return w["at"] + w["size"]
def near(a, b, tol=2): return all(abs(x-y) <= tol for x, y in zip(a, b))
# workspaces without an id: named ones, as a focus by a space's name makes (outside sodiumwm's spaces)
def strays(): return [w["name"] for w in json.load(open(f"{out}/{name}.ws.json")) if w["type"] == "normal" and "id" not in w]
$2
PY
    then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails+1)); fi
}
homes() { h repl 'local o = {} for _, s in ipairs(sodiumwm.core.spaces.list) do o[#o+1] = s.id .. "=" .. tostring(s.home) .. (s.auto and "(auto)" or "") end return table.concat(o, " ")'; }
show() {
    python3 -I - "$OUT/$1.json" "$OUT/$1.mon.json" <<'PY'
import json, sys
mons = {m["id"]: m for m in json.load(open(sys.argv[2]))}
for m in mons.values():
    print(f"  monitor {m['name']}: {m['x']},{m['y']} {m['width']}x{m['height']} @{m['scale']} mirror={m['mirrorOf']} disabled={m['disabled']}")
for c in sorted(json.load(open(sys.argv[1])), key=lambda c: c["class"]):
    mon = mons.get(c["monitor"], {}).get("name", "?")
    print(f"  {c['class'].split('.')[-1]:8} {c['workspace']['name']:18} {mon:9} at {c['at']} size {c['size']} float={c['floating']} fs={c['fullscreen']}")
PY
}

free=$(open free 900 600); edge=$(open edge 600 400); big=$(open big 1800 1100)
snapped=$(open snapped 900 600); maxed=$(open maxed 900 600); mini=$(open mini 1000 700)
place "$free" 1300 500 900 600
place "$edge" 1940 1026 600 400
place "$big" 300 200 1800 1100
place "$mini" 200 600 1000 700
snapshot 0-pre 0.5
h repl "return sodiumwm.api.snap(\"$snapped\", \"left\")" >/dev/null
h repl "return sodiumwm.api.toggleMaximise(\"$maxed\")" >/dev/null
h repl "return sodiumwm.api.minimise(\"$mini\")" >/dev/null
# A second space with one window, not shown
h repl 'return sodiumwm.api.newSpace()' >/dev/null; sleep 0.5
other=$(open other 700 500); place "$other" 1700 800 700 500
h repl 'return sodiumwm.api.switchSpace(1)' >/dev/null
# Space 1 is homed on DESK, as the live spaces are on DP-2 (the bed starts on the host window's
# output); space 2 on REMOTE-1, as earlier sessions left the live ones. A reload (what puts new
# sodiumwm code live) repairs that.
h repl 'local C = sodiumwm.core for _, s in ipairs(C.spaces.list) do s.home = s.id == 1 and "DESK" or "REMOTE-1" end C.mark("spaces") C.commit() return "ok"' >/dev/null
h reload >/dev/null; sleep 2
[[ $(homes) == "1=DESK 2=DESK" ]] && echo "ok   homes repaired: $(homes)" || { echo "FAIL homes after reload: $(homes)"; fails=$((fails+1)); }
snapshot 0-base 1.5; echo "0-base"; show 0-base

bed connect 2940 1838
snapshot 1b-connected 2.5; echo "1b-connected (settled)"; show 1b-connected
check 1b-connected '
assert not strays(), strays()
for k in ("free", "edge", "big", "snapped", "maxed", "mini", "other"):
    assert area(k)["name"] == "REMOTE-1", (k, area(k))
# on screen, title bar too (the plugin draws it above the box)
for k in ("free", "edge", "big", "snapped", "other"):
    assert inside(k) and c[k]["at"][1] >= area(k)["y"] + 40, (k, rect(k), area(k))
A = area("snapped"); assert abs(c["snapped"]["size"][0] - A["w"]/2) < 40 and c["snapped"]["at"][0] - A["x"] < 30, (rect("snapped"), A)
assert c["maxed"]["fullscreen"] == 1, c["maxed"]
# kept its size, and still at the right edge (inside the margins)
assert c["edge"]["size"] == base["edge"]["size"], rect("edge")
assert c["edge"]["at"][0] + c["edge"]["size"][0] > area("edge")["x"] + area("edge")["w"] - 40, rect("edge")
# shrunk to fit
assert c["big"]["size"][0] < area("big")["w"] and c["big"]["size"][1] < area("big")["h"], rect("big")
'

bed resize 2940 1780
snapshot 2-resized 2.5; echo "2-resized"; show 2-resized
check 2-resized 'for k in ("free", "edge", "big", "snapped"): assert inside(k), (k, rect(k), area(k))'

bed resize 2940 1838
snapshot 3-back 2.5; echo "3-back"; show 3-back
check 3-back '
prev, _ = load("1b-connected")
for k in ("free", "edge", "big", "snapped"): assert near(rect(k), rect(k, prev)), (k, rect(k), rect(k, prev))'

bed disconnect
snapshot 4-restored 2.5; echo "4-restored"; show 4-restored
check 4-restored '
assert not strays(), strays()
for k in ("free", "edge", "big", "snapped", "maxed", "mini", "other"):
    assert area(k)["name"] == "DESK", (k, area(k))
    assert near(rect(k), rect(k, base)), (k, rect(k), rect(k, base))
    assert c[k]["workspace"]["name"] == base[k]["workspace"]["name"], (k, c[k]["workspace"])
'
h repl "return sodiumwm.api.restore(\"$mini\")" >/dev/null
snapshot 5-unminimised 1.5
check 5-unminimised '
pre, _ = load("0-pre")
assert near(rect("mini"), rect("mini", pre)), (rect("mini"), rect("mini", pre))'
[[ $(homes) == "1=DESK 2=DESK" ]] && echo "ok   homes: $(homes)" || { echo "FAIL homes: $(homes)"; fails=$((fails+1)); }
echo "fails: $fails"
exit $(( fails > 0 ))

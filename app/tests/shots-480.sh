#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Screenshots at the window's 480 px minimum width of Settings' six tabs and of "This computer" (Linux) in each state, in the
# tightest languages: offscreen in a private headless cage, no input method, private bus, software rendering.
#   tests/shots-480.sh BUILD_DIR OUT_DIR [LANG...]     BUILD_DIR holds bin/sodium-remote (a build with the Linux host); default de_DE ja_JP
#   Set SODIUMOS_I18N_DIR to the folder of i18n/i18n.sh release --out when the build has no translations of its own.
set -euo pipefail
build=$(realpath "${1:?BUILD_DIR}"); out=${2:?OUT_DIR}; shift 2 || true
langs=${*:-de_DE ja_JP}
mkdir -p "$out"; out=$(realpath "$out")
for L in $langs; do
  d=$out/run-$L; rm -rf "$d"; mkdir -p "$d/data" "$d/cfg"
  P='"ev":"host_consent","state"'
  {
    echo "700 size 480x760"
    echo "700 maintab 1"
    echo "700 host-event {$P:\"unavailable\"}"
    echo "1200 grab-page $out/pane-unavailable-$L.png"
    echo "700 host-event {$P:\"ask\"}"
    echo "1200 grab-page $out/pane-off-$L.png"
    echo "700 host-event {\"ev\":\"host\",\"state\":\"error\",\"code\":\"in_use\",\"detail\":\"locked\"}"
    echo "1200 grab-page $out/pane-in-use-$L.png"
    echo "700 host-event {\"ev\":\"host\",\"state\":\"on\",\"id\":\"ABCD2345\",\"port\":4433}"
    echo "700 host-event {\"ev\":\"host_pairing\",\"state\":\"open\",\"code\":\"482913\",\"expires_in\":300}"
    echo "1200 grab-page $out/pane-on-$L.png"
    echo "700 host-event {$P:\"asking\"}"
    echo "1200 grab-page $out/pane-asking-$L.png"
    echo "700 host-event {$P:\"declined\"}"
    echo "1200 grab-page $out/pane-declined-$L.png"
    echo "700 host-event {$P:\"remembered\"}"
    echo "700 host-event {\"ev\":\"host_status\",\"controlled\":true,\"client\":{\"id\":\"c1\",\"name\":\"MacBook Air\",\"os\":\"mac\",\"since\":1767000000,\"view_only\":true}}"
    echo "1200 grab-page $out/pane-live-$L.png"
    echo "700 host-event {\"ev\":\"host_status\",\"controlled\":false}"
    echo "700 forget-dialog"
    echo "1200 grab $out/pane-forget-dialog-$L.png"
    echo "700 maintab 0"
    echo "700 settings"
    for t in 0 1 2 3 4 5; do echo "700 tab $t"; echo "1200 grab $out/settings-$t-$L.png"; done
    echo "700 server add"
    echo "1200 grab $out/server-add-$L.png"
    echo "500 quit"
  } >"$d/cur.script"
  env -u QML_IMPORT_PATH -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE XDG_CONFIG_HOME="$d/cfg" XDG_DATA_HOME="$d/xdg" \
    WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
    QT_IM_MODULE=none QT_QPA_PLATFORM=wayland LIBGL_ALWAYS_SOFTWARE=1 SODIUMOS_UI_LANGUAGE=$L \
    timeout 240 dbus-run-session -- cage -- "$build/bin/sodium-remote" --data "$d/data" --lan-only --script "$d/cur.script" >"$d/log" 2>&1 || true
  python3 -I - "$out" "$L" <<'PY'
import sys, glob
from PIL import Image
for f in glob.glob(f"{sys.argv[1]}/*-{sys.argv[2]}.png"):
    im = Image.open(f).convert("RGBA"); bg = Image.new("RGBA", im.size, (28, 29, 38, 255)); bg.alpha_composite(im); bg.convert("RGB").save(f)
PY
  echo "shots: $L done ($d/log)"
done

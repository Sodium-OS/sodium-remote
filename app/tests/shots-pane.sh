#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Screenshots of "This computer" on Linux, each consent state and the unsupported desktop, driven by test events (no portal
# session is started): offscreen in a private headless cage, no input method, private bus, software rendering.
#   tests/shots-pane.sh BUILD_DIR [LANG...]     BUILD_DIR holds bin/sodium-remote; pictures go to BUILD_DIR/../shots/ (default en_US de_DE ja_JP)
set -euo pipefail
build=$(realpath "${1:?BUILD_DIR}"); shift || true
langs=${*:-en_US de_DE ja_JP}
shots=$(dirname "$build")/shots; mkdir -p "$shots"
for L in $langs; do
  d=$shots/run-$L; rm -rf "$d"; mkdir -p "$d/data" "$d/cfg"
  P='"ev":"host_consent","state"'
  cat >"$d/cur.script" <<SCRIPT
800 maintab 1
700 host-event {$P:"unavailable"}
1200 grab-page $shots/unavailable-$L.png
700 host-event {$P:"ask"}
1200 grab-page $shots/ask-off-$L.png
700 host-event {"ev":"host","state":"on","id":"ABCD2345","port":4433}
700 host-event {"ev":"host_pairing","state":"open","code":"482913","expires_in":300}
1200 grab-page $shots/ask-$L.png
700 host-event {$P:"asking"}
1200 grab-page $shots/asking-$L.png
700 host-event {$P:"declined"}
1200 grab-page $shots/declined-$L.png
700 host-event {$P:"remembered"}
700 host-event {"ev":"host_status","controlled":true,"client":{"id":"c1","name":"MacBook Air","os":"mac","since":1767000000,"view_only":true}}
1200 grab-page $shots/live-view-only-$L.png
700 host-event {"ev":"host_status","controlled":false}
1200 grab-page $shots/remembered-$L.png
700 forget-dialog
1200 grab $shots/forget-dialog-$L.png
500 quit
SCRIPT
  env -u QML_IMPORT_PATH -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE XDG_CONFIG_HOME="$d/cfg" XDG_DATA_HOME="$d/xdg" \
    WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
    QT_IM_MODULE=none QT_QPA_PLATFORM=wayland LIBGL_ALWAYS_SOFTWARE=1 SODIUMOS_UI_LANGUAGE=$L \
    SODIUM_REMOTE_TEST=1 timeout 180 dbus-run-session -- cage -- "$build/bin/sodium-remote" --data "$d/data" --lan-only --script "$d/cur.script" >"$d/log" 2>&1 || true
  python3 -I - "$shots" "$L" <<'PY'
import sys, glob
from PIL import Image
for f in glob.glob(f"{sys.argv[1]}/*-{sys.argv[2]}.png"):
    im = Image.open(f).convert("RGBA"); bg = Image.new("RGBA", im.size, (28, 29, 38, 255)); bg.alpha_composite(im); bg.convert("RGB").save(f)
PY
  echo "shots: $L done ($d/log)"
done

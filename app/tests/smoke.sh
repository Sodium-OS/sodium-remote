#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Offscreen smoke test of Settings › Server, About, the Show previews switch and, in a build with the Linux host, This computer: starts the app in a private headless Wayland session
# (cage, no input method, private bus) with a script that opens both pages and grabs them, and checks the pictures exist.
#   tests/smoke.sh BUILD_DIR [LANG]       BUILD_DIR holds bin/sodium-remote; pictures go to BUILD_DIR/smoke/
set -euo pipefail
build=$(realpath "${1:?BUILD_DIR}"); lang=${2:-en_US}
out=$build/smoke; rm -rf "$out"; mkdir -p "$out/data"
cat >"$out/smoke.script" <<SCRIPT
700 maintab 1
700 host-event {"ev":"host_consent","state":"remembered"}
1500 grab $out/this-computer.png
700 forget-consent
1500 grab $out/this-computer-forgotten.png
700 maintab 0
700 settings
700 tab 3
1500 grab $out/server.png
700 server add
1500 grab $out/server-add.png
700 close
700 tab 5
1500 grab $out/about.png
700 notices
1500 grab $out/notices.png
700 close
700 settings
700 tab 4
1500 grab $out/advanced.png
700 previews off
1500 grab $out/previews-off.png
500 quit
SCRIPT
XDG_CONFIG_HOME=$out/cfg env -u QML_IMPORT_PATH -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
  QT_IM_MODULE=none QT_QPA_PLATFORM=wayland LIBGL_ALWAYS_SOFTWARE=1 SODIUMOS_UI_LANGUAGE=$lang \
  SODIUM_REMOTE_TEST=1 timeout 120 dbus-run-session -- cage -- "$build/bin/sodium-remote" --data "$out/data" --lan-only --script "$out/smoke.script" >"$out/log" 2>&1 || true
for f in server server-add about notices this-computer this-computer-forgotten advanced previews-off; do [[ -s $out/$f.png ]] || { echo "smoke: no $f.png (see $out/log)"; exit 1; }; done
grep -q '"showPreviews":false' "$out/data/app.json" || { echo "smoke: the switch didn't save"; exit 1; }
echo "smoke: ok ($out)"

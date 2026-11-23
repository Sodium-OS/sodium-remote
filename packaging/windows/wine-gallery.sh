#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Runs the staged Sodium UI gallery (remote/build/win-gallery) under wine, headless: a private wine
# prefix (never the user's ~/.wine) and `cage`, a Wayland kiosk compositor with no output, so no
# window appears on the live desktop.
#
#   remote/packaging/windows/wine-gallery.sh TIMEOUT_SECONDS [gallery args...]
#   e.g.  wine-gallery.sh 90 --grab=remote/build/shots/wine.png
#
# Qt Quick's D3D11 backend runs on wine's wined3d here; it is a smoke test, the Windows VM is the
# real check.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
t=$1; shift
export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
export WINEPREFIX=${WINEPREFIX_SODIUMUI:-$REMOTE/build/wine-prefix-ui}
export WINEDEBUG=${WINEDEBUG:--all} WINEDLLOVERRIDES="mscoree,mshtml=d"
unset WAYLAND_DISPLAY DISPLAY
exec timeout "$t" cage -- wine "${SODIUMUI_GALLERY_EXE:-$REMOTE/build/win-gallery/sodiumui-gallery.exe}" "$@"

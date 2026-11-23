#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Screenshots of the staged gallery under wine (software OpenGL, headless): English, German and
# Japanese, dark and light, into remote/build/shots/wine-*.png. The window's own rendering is
# grabbed by the gallery itself (--grab), so wine's display does not matter.
#
#   remote/packaging/windows/gallery-shots.sh
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
SHOTS=$REMOTE/build/shots
STATE=$REMOTE/build/shot-state
mkdir -p "$SHOTS" "$STATE/dark" "$STATE/light/theme"
export QT_OPENGL=software QSG_RHI_BACKEND=opengl QT_FORCE_STDERR_LOGGING=1
# a light theme: the generator's output for settings {"theme":"light"} in a state dir of its own
echo '{"theme":"light"}' >"$STATE/light/settings.json"
python3 "$REMOTE/../tokens/generate.py" --settings "$STATE/light/settings.json" --out "$STATE/light/theme" >/dev/null
win() { printf 'Z:%s' "${1//\//\\}"; }

shot() {   # name language theme page
    local name=$1 lang=$2 theme=$3 page=$4
    SODIUMOS_UI_LANGUAGE=$lang SODIUMOS_STATE=$(win "$STATE/$theme") \
        "$HERE/wine-gallery.sh" 150 --page="$page" --grab="$(win "$SHOTS/wine-$name.png")" >"$SHOTS/wine-$name.log" 2>&1 || true
    [[ -f $SHOTS/wine-$name.png ]] && echo "ok   $name" || echo "FAIL $name (see $SHOTS/wine-$name.log)"
}
for theme in dark light; do
    for lang in en_US de_DE ja_JP; do
        shot "gallery-${lang%%_*}-$theme" $lang $theme gallery
    done
done
shot languages-de-dark de_DE dark languages
shot languages-ja-dark ja_JP dark languages

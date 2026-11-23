#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Builds Sodium Remote for Windows (stage-app.sh, no zip) and copies the pieces that change often to the VM's
# <guest folder>\windows-app\SodiumRemote (the whole package the first time: pass --all). Hold the VM lock first (win.sh lock windows-app).
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../../.." && pwd)
# shellcheck disable=SC1091
source "$HERE/../config.sh"
WIN=${WIN:-$HERE/../win.sh}
OUT=$REMOTE/build/win-app/SodiumRemote
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
env ${ICONS:+ICONS="$ICONS"} "$REMOTE/packaging/windows/stage-app.sh" ${NO_UI:+--no-ui} --no-zip >/dev/null
$WIN ssh 'Get-Process SodiumRemote -ErrorAction SilentlyContinue | Stop-Process -Force' || true
$WIN scp-to "$HERE/run-app.ps1" "$GF/windows-app/run-app.ps1" >/dev/null
if [[ ${1:-} == --all ]]; then
  $WIN scp-to "$OUT/." "$GF/windows-app/SodiumRemote/"
else
  $WIN scp-to "$OUT/SodiumRemote.exe" "$GF/windows-app/SodiumRemote/SodiumRemote.exe"
  $WIN scp-to "$OUT/SodiumUI" "$GF/windows-app/SodiumRemote/"
  $WIN scp-to "$OUT/translations" "$GF/windows-app/SodiumRemote/"
fi
echo deployed
